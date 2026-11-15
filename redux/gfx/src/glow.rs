//! Fog light glow: the air round the firelight's lights scatters their light toward the eye, so a
//! lantern or brazier shows as a soft, shimmering orb in the fog, and a lamp far down a foggy road
//! glows before its model has loaded (its light is in [`super::locallights`]'s list). The law is
//! [`glow.wgsl`](glow.wgsl), the closed-form single scattering of a point light in homogeneous
//! air, run inside the world's own fog shading through this crate's hook bodies (`gfx_surface`,
//! `gfx_fog`, `gfx_sky`), never as a pass over the finished frame: it is resolved by MSAA and
//! jittered by TAA as the surfaces are, and needs no frame history.
//!
//! This module fills the two glow rows: where the camera stands (the sky hook has no camera
//! position), how thick the air is, the gain, the noise clock and the scene fog end. The air's
//! extinction is the haze's (`haze::density`: thicker at dawn, in weather, thin indoors) plus the
//! zone fog's own (the modern fog's exponential: 60 % survives at the middle of the zone's span),
//! so Duskwood's short fog glows more than Elwynn's long one, as fog does. The lights are the
//! firelight's rows: the [`super::rows::MAX_FIRES`] nearest within [`REACH`] while the glow is
//! on, fading out over its last stretch (`glow.wgsl`'s `FADE_FROM`..`FADE_TO`).
//!
//! It needs the firelight (its lights are the firelight's); off, the gain is zero and the hooks
//! add nothing. Deviation: 1.12 has no glow in its fog; a redux effect behind its own switch.
use super::rows::{GLOW_A, GLOW_B};
use super::Enhanced;
use benilla_world::{
    lighting::{GameClock, GfxData, WowLighting},
    view::WorldCamera,
    weather::WeatherState,
    wmo_portal::CameraInteriorClaim,
};
use bevy::prelude::*;

/// How far from the camera a light glows (yd); the firelight picks within it while the glow is
/// on. `glow.wgsl`'s `FADE_TO`. Deviation: no source value; past ~250 yd a lamp's orb is a pixel
/// or two under the far fog.
pub(super) const REACH: f32 = 250.0;
/// The glow's gain at night: σ·I/(4π) with the firelight's night gain folded in. Deviation: no
/// source value; tuned in stills against the WoW Forever reference, so a lamp in fog glows as a
/// soft orb a few yards across and a brazier lights the air over it.
const GAIN_NIGHT: f32 = 40.0;
/// The share of the gain left in full daylight: sunlit air drowns a lamp's glow.
/// Deviation: tuned in stills.
const DAY_SHARE: f32 = 0.1;
/// The modern fog's exponential at the middle of its span (`fog.wgsl`'s `FOG_EXP_MID`).
const FOG_EXP_MID: f32 = 1.021_651_2;
/// The noise clock's wrap (s): the drift slides the noise lattice, so a wrap jumps it; an hour
/// is never seen.
const CLOCK_WRAP_S: f64 = 3600.0;

pub(super) struct GlowPlugin;

impl Plugin for GlowPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, update_glow);
    }
}

/// The glow's gain for the sun's height (`sin(elevation)`): full at night, a tenth by day, on
/// the firelight's day curve.
fn gain(sun_y: f32) -> f32 {
    let t = ((sun_y + 0.05) / 0.3).clamp(0.0, 1.0);
    let day = t * t * (3.0 - 2.0 * t);
    night_gain() * (1.0 - (1.0 - DAY_SHARE) * day)
}

/// [`GAIN_NIGHT`], or `$REDUX_GLOW_GAIN` for a tuning session.
fn night_gain() -> f32 {
    static GAIN: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *GAIN.get_or_init(|| {
        std::env::var("REDUX_GLOW_GAIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(GAIN_NIGHT)
    })
}

/// The air's extinction per yard: the haze's, plus the zone fog's exponential over its span.
fn extinction(haze: f32, fog_span: (f32, f32)) -> f32 {
    let len = fog_span.1 - fog_span.0;
    let fog = if len > 1.0 { FOG_EXP_MID / len } else { 0.0 };
    haze + fog
}

#[allow(clippy::too_many_arguments)]
fn update_glow(
    enhanced: Res<Enhanced>,
    time: Res<Time>,
    lighting: Res<WowLighting>,
    clock: Option<Res<GameClock>>,
    weather: Option<Res<WeatherState>>,
    interior: Option<Res<CameraInteriorClaim>>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    mut frame: ResMut<GfxData>,
    mut noise_clock: Local<f64>,
) {
    let on = enhanced.glow && enhanced.firelight;
    let rows = match camera.iter().next() {
        Some(eye) if on => {
            let eye = eye.translation();
            *noise_clock = (*noise_clock + time.delta_secs_f64()) % CLOCK_WRAP_S;
            let minute = clock.map_or(720.0, |c| c.minute as f32);
            let weather = weather.map_or(0.0, |w| w.sky_density.max(w.effect_density));
            let indoors = interior.is_some_and(|c| c.0.is_some());
            let span = lighting.fog_span();
            let sigma = extinction(super::haze::density(minute, weather, indoors), span);
            [
                [eye.x, eye.y, eye.z, sigma],
                [
                    gain(lighting.celestial_dir().y),
                    *noise_clock as f32,
                    span.1,
                    0.0,
                ],
            ]
        }
        _ => [[0.0; 4]; 2],
    };
    if frame.rows[GLOW_A] != rows[0] || frame.rows[GLOW_B] != rows[1] {
        frame.rows[GLOW_A] = rows[0];
        frame.rows[GLOW_B] = rows[1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gain_is_full_at_night_and_a_tenth_by_day() {
        assert!((gain(-0.5) - GAIN_NIGHT).abs() < 1e-6);
        assert!((gain(0.9) - GAIN_NIGHT * DAY_SHARE).abs() < 1e-6);
    }

    /// A short zone fog (Duskwood's) holds thicker air than a long one (Elwynn's), on top of the
    /// haze; no span adds nothing.
    #[test]
    fn a_shorter_fog_is_thicker_air() {
        let haze = 0.004;
        let short = extinction(haze, (20.0, 180.0));
        let long = extinction(haze, (100.0, 500.0));
        assert!(short > long && long > haze);
        assert_eq!(extinction(haze, (0.0, 0.0)), haze);
    }

    /// The shader's far fade ends at the reach the firelight picks within, and it reads every
    /// firelight slot at a constant row.
    #[test]
    fn the_shader_agrees_with_the_rows() {
        let src = include_str!("glow.wgsl");
        assert!(src.contains(&format!("const FADE_TO: f32 = {REACH:.1};")));
        for k in 0..super::super::rows::MAX_FIRES {
            assert!(
                src.contains(&format!(
                    "if (count > {k}u) {{ sum += glow_one(gfx.rows[gfx_rows::FIRES + {k}u]"
                )),
                "slot {k}"
            );
        }
    }

    /// Off (or with the firelight off) both rows are zero, so the hooks add nothing.
    #[test]
    fn off_the_rows_are_zero() {
        use bevy::ecs::system::RunSystemOnce;
        for (firelight, glow) in [(false, true), (true, false)] {
            let mut world = World::new();
            world.init_resource::<Time>();
            world.init_resource::<WowLighting>();
            let mut frame = GfxData::default();
            frame.rows[GLOW_B] = [1.0; 4];
            world.insert_resource(frame);
            world.insert_resource(Enhanced(super::super::prefs::GraphicsPrefs {
                firelight,
                glow,
                ..Default::default()
            }));
            world.spawn((WorldCamera, GlobalTransform::default()));
            world.run_system_once(update_glow).unwrap();
            let rows = world.resource::<GfxData>().rows;
            assert_eq!([rows[GLOW_A], rows[GLOW_B]], [[0.0; 4]; 2]);
        }
    }
}
