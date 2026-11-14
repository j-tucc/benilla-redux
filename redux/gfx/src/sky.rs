//! The Enhanced sky (LOOK-12): a smooth gradient through the sky stops, a sun glow, a procedural
//! night sky, and detailed, sun-lit clouds. The engine's sky and cloud domes call the
//! `gfx_sky` and `gfx_cloud` hooks, whose bodies run [`sky.wgsl`](sky.wgsl) under `GFX_SKY` on the
//! rows this plugin writes (`SKY`, `CLOUD`, `CLOCK`); off, the rows are zero and the hooks return
//! the reference's sky. Everwood's zone skyboxes are not ported.
//!
//! Ported from the everwood graphics fork of Benilla: `benilla-world/src/sky_fx.rs`
//! (`glow_strength`), its `sky.rs` (`sky_fx_inputs`) and its `clouds/layer.rs`
//! (`update_cloud_fx`), at its High tier.
//!
//! The cloud sun march feed ([`cloud_rows`]):
//! Ported from WarcraftXL (https://github.com/WarcraftXL) by iThorgrim — module wxl-retail-clouds, Clouds.cpp.
use super::rows::{CLOCK, CLOUD, SKY, SKY_CLOCK_WRAP_S};
use super::Enhanced;
use benilla_world::{
    clouds::CloudCoverage,
    lighting::{GfxData, LightingConsumeSet, LightingResolveSet, WowLighting},
};
use bevy::prelude::*;

/// Everwood's High tier, kept in the rows' first value; `GFX_SKY` is what turns the shading on.
const TIER: f32 = 2.0;
/// The glow's strength at the sun. Source: everwood `sky_fx::GLOW_GAIN`.
const GLOW_GAIN: f32 = 0.22;
/// How far the glow colour leans from the fog colour to the sun's tint. Source: everwood
/// `sky_fx_inputs`.
const GLOW_TINT: f32 = 0.6;
/// The distance at which the glow samples the cloud cover toward the sun. Source: everwood
/// `sky_fx_inputs` (the glare's occlusion sample).
const COVER_SAMPLE: f32 = 12.0;

pub(super) struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        // Last frame's cloud cover: the tile ticks in the consume set, after this.
        app.add_systems(
            Update,
            fill_sky
                .after(LightingResolveSet)
                .before(LightingConsumeSet),
        );
    }
}

/// Rounded to 1/4096 so an idle frame writes the same rows and leaves the materials alone.
fn q(v: f32) -> f32 {
    (v * 4096.0).round() / 4096.0
}

/// The glow's day fade: full once the sun clears the horizon, gone a few degrees under it.
/// Source: everwood `sky_fx::glow_day_fade`.
fn glow_day_fade(sun_y: f32) -> f32 {
    let t = ((sun_y + 0.10) / 0.16).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The glow strength: the day fade, dimmed by the cloud cover over the sun and by storm weather.
/// Source: everwood `sky_fx::glow_strength`.
fn glow_strength(sun_y: f32, cover_at_sun: f32, bcc: f32) -> f32 {
    GLOW_GAIN
        * glow_day_fade(sun_y)
        * (1.0 - 0.8 * cover_at_sun.clamp(0.0, 1.0))
        * (1.0 - 0.85 * bcc.clamp(0.0, 1.0))
}

/// The sky dome's rows: `(tier, 0, night-sky alpha, glow strength)`, the sun, the glow colour.
fn sky_rows(l: &WowLighting, cover_at_sun: f32) -> [Vec4; 3] {
    let sun = l.celestial_dir().normalize_or(Vec3::Y);
    let bcc = l.storm_bcc();
    let strength = glow_strength(sun.y, cover_at_sun, bcc);
    let glow = Vec3::from(l.fog_color).lerp(Vec3::from(l.celestial_tint()), GLOW_TINT);
    // The night sky follows the star curve, thinned by storm weather.
    let night = (l.star_alpha() * (1.0 - bcc)).clamp(0.0, 1.0);
    [
        Vec4::new(TIER, 0.0, q(night), q(strength)),
        Vec4::new(q(sun.x), q(sun.y), q(sun.z), 0.0),
        Vec4::new(q(glow.x), q(glow.y), q(glow.z), 0.0),
    ]
}

/// The cloud dome's rows: `(tier, 0, the glow body's direction on the sheet)` and `(the cloud sun
/// colour, the march strength)`. The march walks toward the glow body (sun by day, moon by
/// night), scaled by how far it sits off the zenith and by the kernel's glow envelope.
fn cloud_rows(l: &WowLighting) -> [Vec4; 2] {
    let (body, track, colours) = l.cloud_glow();
    let flat = Vec2::new(body.x, body.z);
    let len = flat.length();
    let d = if len > 1e-3 { flat / len } else { Vec2::ZERO };
    let strength = (len * 1.6).min(1.0) * track * (1.0 - 0.75 * l.storm_bcc());
    let sun = colours[0];
    [
        Vec4::new(TIER, 0.0, q(d.x), q(d.y)),
        Vec4::new(q(sun[0]), q(sun[1]), q(sun[2]), q(strength.clamp(0.0, 1.0))),
    ]
}

/// The sky's animation clock: seconds since start, accumulated in f64 and wrapped at a day, so it
/// never steps.
#[derive(Default)]
struct SkyClock(f64);

fn fill_sky(
    time: Res<Time>,
    enhanced: Res<Enhanced>,
    lighting: Res<WowLighting>,
    clouds: Option<Res<CloudCoverage>>,
    mut frame: ResMut<GfxData>,
    mut clock: Local<SkyClock>,
) {
    let mut next = [[0.0; 4]; 6];
    if enhanced.sky {
        clock.0 = (clock.0 + time.delta_secs_f64()) % SKY_CLOCK_WRAP_S;
        let sun = lighting.celestial_dir().normalize_or(Vec3::Y);
        let cover = clouds.map_or(0.0, |c| c.coverage(sun * COVER_SAMPLE));
        let sky = sky_rows(&lighting, cover);
        let cloud = cloud_rows(&lighting);
        next = [
            sky[0].to_array(),
            sky[1].to_array(),
            sky[2].to_array(),
            cloud[0].to_array(),
            cloud[1].to_array(),
            [clock.0 as f32, 0.0, 0.0, 0.0],
        ];
    }
    const _: () = assert!(CLOUD == SKY + 3 && CLOCK == CLOUD + 2);
    if frame.rows[SKY..=CLOCK] != next {
        frame.rows[SKY..=CLOCK].copy_from_slice(&next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glow_fades_below_the_horizon_and_under_cloud() {
        assert_eq!(glow_strength(-0.2, 0.0, 0.0), 0.0);
        assert!((glow_strength(0.5, 0.0, 0.0) - GLOW_GAIN).abs() < 1e-6);
        assert!(glow_strength(0.5, 1.0, 0.0) < 0.25 * GLOW_GAIN);
        assert!(glow_strength(0.5, 0.0, 1.0) < 0.2 * GLOW_GAIN);
    }

    #[test]
    fn the_rows_turn_both_domes_on() {
        let l = WowLighting::default();
        assert!(sky_rows(&l, 0.0)[0].x >= 0.5);
        assert!(cloud_rows(&l)[0].x >= 1.5);
    }
}
