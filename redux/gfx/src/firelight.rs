//! Firelight (LOOK-16): fires, braziers and lanterns light what is round them per pixel, with a
//! warm falloff and a gentle flicker, on top of the engine's per-vertex point lights (which light
//! a vertex from its unit's three nearest, too coarse to pool light on the ground).
//!
//! The lights are the engine's own: every outdoor [`WorldPointLight`] (an M2's light, a carried
//! torch), the same set its point table packs. Each frame this picks the [`MAX_FIRES`] nearest the
//! camera within [`REACH`] and writes them into the [`FIRES`] rows, which this crate's
//! `gfx_surface` body (`fire.wgsl`) walks per fragment, skipping every light farther than its
//! 12 yd radius: the cost is a short loop over what is near. Lights inside buildings (those with
//! [`LightRooms`]) are left out: interior surfaces never call the hook, and a hall's torches would
//! shine through its walls onto the ground outside.
//!
//! The rows: `FIRES + i` = (world xyz, the colour as three unorm8 channels in one word whose
//! fourth byte keeps it a normal float); `MISC` y = the count, z = the gain, w = the flicker
//! clock. Off, the count is zero and the hook adds nothing.
use super::rows::{FIRES, MAX_FIRES, MISC};
use super::Enhanced;
use benilla_world::lighting::{GfxData, LightRooms, WorldPointLight, WowLighting};
use benilla_world::view::WorldCamera;
use bevy::prelude::*;

/// How far from the camera a light is picked (yd). Past it, its pool is a few pixels under the
/// haze. Deviation: no source value; tuned in stills.
const REACH: f32 = 60.0;
/// The firelight's gain at night. Deviation: no source value; tuned in stills against the LOOK-16
/// campfire reference, so a campfire pools warm light a few yards round it.
const GAIN_NIGHT: f32 = 3.2;
/// The share of it left in full daylight, when the sun drowns a flame's light.
/// Deviation: no source value; tuned in stills.
const DAY_SHARE: f32 = 0.25;
/// How far a light's colour leans toward a flame's warm white at its own peak: the authored
/// campfire light (0.71, 0.20, 0) would tint green grass brown rather than gold.
/// Deviation: no source value; tuned in stills against the campfire reference.
const WARM_LEAN: f32 = 0.35;
const WARM_WHITE: Vec3 = Vec3::new(1.0, 0.72, 0.45);
/// The flicker clock's wrap (s): every flicker tone in `fire.wgsl` makes whole cycles in it, so
/// the wrap is seamless.
pub(super) const CLOCK_WRAP_S: f64 = 600.0;

pub(super) struct FirelightPlugin;

impl Plugin for FirelightPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, update_firelight);
    }
}

/// A colour 0..1 as three unorm8 channels, the fourth byte fixed at 0x40 so the word reads as a
/// normal float (as `wetness.rs` packs its colours).
fn pack_rgb(c: Vec3) -> f32 {
    let b = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    f32::from_bits(b(c.x) | (b(c.y) << 8) | (b(c.z) << 16) | (0x40 << 24))
}

/// A light's colour for the hook: hue leaned toward a flame's warm white at its own peak, scaled
/// into 0..1 (no authored light here exceeds 1; a brighter one keeps its hue).
fn fire_colour(rgb: [f32; 3]) -> Vec3 {
    let c = Vec3::from(rgb).max(Vec3::ZERO);
    let peak = c.max_element();
    if peak <= 1e-4 {
        return Vec3::ZERO;
    }
    let leaned = c.lerp(WARM_WHITE * peak, WARM_LEAN);
    leaned / leaned.max_element().max(1.0)
}

/// The gain for the sun's height (`sin(elevation)`): full at night, a quarter by day.
fn gain(sun_y: f32) -> f32 {
    let t = ((sun_y + 0.05) / 0.3).clamp(0.0, 1.0);
    let day = t * t * (3.0 - 2.0 * t);
    GAIN_NIGHT * (1.0 - (1.0 - DAY_SHARE) * day)
}

/// The nearest `MAX_FIRES` of `lights` (position, colour) within [`REACH`] of `eye`, nearest first.
/// No allocation: an insertion into a fixed array.
fn nearest(
    eye: Vec3,
    lights: impl Iterator<Item = (Vec3, [f32; 3])>,
) -> ([(f32, Vec3, Vec3); MAX_FIRES], usize) {
    let mut out = [(0.0, Vec3::ZERO, Vec3::ZERO); MAX_FIRES];
    let mut n = 0;
    for (at, rgb) in lights {
        let d2 = at.distance_squared(eye);
        if d2 >= REACH * REACH {
            continue;
        }
        let colour = fire_colour(rgb);
        if colour == Vec3::ZERO {
            continue;
        }
        let mut i = n.min(MAX_FIRES);
        while i > 0 && out[i - 1].0 > d2 {
            if i < MAX_FIRES {
                out[i] = out[i - 1];
            }
            i -= 1;
        }
        if i < MAX_FIRES {
            out[i] = (d2, at, colour);
            n = (n + 1).min(MAX_FIRES);
        }
    }
    (out, n)
}

#[allow(clippy::type_complexity)]
fn update_firelight(
    enhanced: Res<Enhanced>,
    time: Res<Time>,
    lighting: Res<WowLighting>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    lights: Query<(&WorldPointLight, &GlobalTransform), Without<LightRooms>>,
    mut frame: ResMut<GfxData>,
    mut clock: Local<f64>,
) {
    let mut next = *frame;
    next.rows[MISC][1..4].copy_from_slice(&[0.0; 3]);
    if enhanced.firelight {
        *clock = (*clock + time.delta_secs_f64()) % CLOCK_WRAP_S;
        if let Ok(eye) = camera.single() {
            // The engine's packer divides by 4π to undo the `PointLight` convention.
            let s = 1.0 / (4.0 * std::f32::consts::PI);
            let (fires, n) = nearest(
                eye.translation(),
                lights.iter().map(|(l, t)| {
                    let k = l.intensity * s;
                    (t.translation(), l.color.map(|c| c * k))
                }),
            );
            for (i, (_, at, colour)) in fires.iter().take(n).enumerate() {
                next.rows[FIRES + i] = [at.x, at.y, at.z, pack_rgb(*colour)];
            }
            next.rows[MISC][1] = n as f32;
            next.rows[MISC][2] = gain(lighting.celestial_dir().y);
            next.rows[MISC][3] = *clock as f32;
        }
    }
    if *frame != next {
        *frame = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_lights_within_reach_are_kept_nearest_first() {
        // Northshire's coordinates: Bevy x = −WoW y, z = −WoW x.
        let eye = Vec3::new(185.0, 88.0, 8761.0);
        let fire = [0.71, 0.2, 0.0];
        let lights = (0..20)
            .map(|i| (eye + Vec3::new(3.0 * i as f32, 0.0, 0.0), fire))
            .rev()
            .chain(std::iter::once((eye + Vec3::X * 500.0, fire)));
        let (out, n) = nearest(eye, lights);
        assert_eq!(n, MAX_FIRES);
        for (i, (d2, at, _)) in out.iter().enumerate() {
            assert!((at.x - eye.x - 3.0 * i as f32).abs() < 1e-3, "{i} {at}");
            assert!(*d2 < REACH * REACH);
        }
        let (_, none) = nearest(eye, std::iter::once((eye + Vec3::X * 61.0, fire)));
        assert_eq!(none, 0);
    }

    #[test]
    fn the_colour_warms_and_packs_into_a_normal_float() {
        let c = fire_colour([0.71, 0.2, 0.0]);
        assert!(c.x > c.y && c.y > c.z && c.z > 0.0, "{c}");
        assert!(c.max_element() <= 1.0);
        let word = pack_rgb(c);
        assert!(word.is_normal());
        let bits = word.to_bits();
        assert_eq!(bits & 0xff, (c.x * 255.0).round() as u32);
        assert_eq!(fire_colour([0.0; 3]), Vec3::ZERO);
    }

    #[test]
    fn the_gain_is_full_at_night_and_a_quarter_by_day() {
        assert!((gain(-0.5) - GAIN_NIGHT).abs() < 1e-6);
        assert!((gain(0.9) - GAIN_NIGHT * DAY_SHARE).abs() < 1e-6);
        assert!(gain(0.05) < gain(-0.05));
    }

    #[test]
    fn the_flicker_clock_wraps_on_whole_cycles() {
        // `fire.wgsl`'s FLICKER_HZ.
        for hz in [1.5, 2.5, 3.7] {
            let cycles = hz * CLOCK_WRAP_S;
            assert!((cycles - cycles.round()).abs() < 1e-9, "{hz}");
        }
        let src = include_str!("fire.wgsl");
        assert!(src.contains("vec3<f32>(1.5, 2.5, 3.7)"));
    }

    /// `fire.wgsl` reads each light at a constant row (no loop over the by-value rows): one call
    /// per slot, every slot covered.
    #[test]
    fn the_shader_reads_every_slot() {
        let src = include_str!("fire.wgsl");
        for k in 0..MAX_FIRES {
            assert!(
                src.contains(&format!(
                    "if (count > {k}u) {{ sum += fire_one(gfx.rows[gfx_rows::FIRES + {k}u]"
                )),
                "slot {k}"
            );
        }
        assert!(!src.contains(&format!("count > {MAX_FIRES}u")));
    }
}
