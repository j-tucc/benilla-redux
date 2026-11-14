//! Moonlight (LOOK-14): the white moon as a dim, cool light added to the stock night. Vanilla
//! has no moon light: its night is the night colours on the lighting sun, whose direction never
//! sets, so a night scene is lit from noon's compass point, only darker and bluer. This keeps that
//! night as the base and adds a wrapped-Lambert term from the moon the sky draws, which the
//! receivers evaluate per fragment (`moon.wgsl`, through this crate's `gfx_lit` body) and shadow
//! with the shadow rig's map once the rig holds the moon (`shadows.rs`).
//!
//! The rows: [`MOON_A`] = (direction to the moon, intensity), [`MOON_B`] = (tint, how much of
//! the map holds the moon); the rig writes `MOON_B.w`.
//!
//! Ported from the everwood graphics fork of Benilla (MIT OR Apache-2.0), revision `cdc7130b`,
//! `benilla-world/src/lighting/moonlight.rs` (the intensity law, its base, the storm dim) and
//! `benilla-assets/src/shaders/moonlight_hook.wgsl` (the tint).
use super::rows::{MOON_A, MOON_B};
use super::shadows::body_strength;
use super::Enhanced;
use benilla_world::lighting::{GfxData, WowLighting};
use bevy::prelude::*;

/// The moon's N·L = 1 light at a high full moon under a clear sky. Source: everwood
/// `MOON_LIGHT_BASE`.
pub const MOON_LIGHT_BASE: f32 = 0.16;
/// How much of it a full storm takes away. Source: everwood `MOON_STORM_DIM`.
const MOON_STORM_DIM: f32 = 0.75;
/// Cool moonlight: blue-white, never saturated blue. Source: everwood `MOON_TINT`.
const MOON_TINT: [f32; 3] = [0.62, 0.74, 1.0];

pub(super) struct MoonlightPlugin;

impl Plugin for MoonlightPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, update_moonlight);
    }
}

/// The moon's light this frame (before the tint) from the sine of each body's elevation and the
/// storm blend: none by day or with the moon set, ramping with the moon's rise and the sun's set
/// over the shadow rig's own 0..12° ramp.
pub(super) fn moon_intensity(sun_y: f32, moon_y: f32, storm: f32) -> f32 {
    let night = 1.0 - body_strength(sun_y);
    let risen = body_strength(moon_y);
    let sky = 1.0 - MOON_STORM_DIM * storm.clamp(0.0, 1.0);
    MOON_LIGHT_BASE * night * risen * sky
}

fn update_moonlight(
    enhanced: Res<Enhanced>,
    lighting: Res<WowLighting>,
    mut frame: ResMut<GfxData>,
) {
    let mut next = *frame;
    let moon = lighting.moon_dir().normalize_or_zero();
    let intensity = if enhanced.moonlight && moon != Vec3::ZERO {
        moon_intensity(lighting.celestial_dir().y, moon.y, lighting.storm_bcc())
    } else {
        0.0
    };
    if intensity > 0.0 {
        next.rows[MOON_A] = [moon.x, moon.y, moon.z, intensity];
        let held = next.rows[MOON_B][3];
        next.rows[MOON_B] = [MOON_TINT[0], MOON_TINT[1], MOON_TINT[2], held];
    } else {
        next.rows[MOON_A] = [0.0; 4];
    }
    if *frame != next {
        *frame = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_moonlight_by_day_or_with_the_moon_set() {
        assert_eq!(moon_intensity(0.7, 0.8, 0.0), 0.0);
        assert_eq!(moon_intensity(-0.9, -0.1, 0.0), 0.0);
        assert!((moon_intensity(-0.9, 0.8, 0.0) - MOON_LIGHT_BASE).abs() < 1e-6);
    }

    #[test]
    fn the_moon_rises_smoothly_and_storms_dim_it() {
        let mut prev = 0.0;
        for step in 0..=40 {
            let v = moon_intensity(-0.9, -0.05 + step as f32 * 0.01, 0.0);
            assert!(v + 1e-7 >= prev);
            prev = v;
        }
        let clear = moon_intensity(-0.9, 0.8, 0.0);
        let storm = moon_intensity(-0.9, 0.8, 1.0);
        assert!(storm > 0.0 && storm < clear * 0.5);
    }

    #[test]
    fn dawn_takes_the_moon_away_as_the_sun_rises() {
        let mut prev = f32::INFINITY;
        for step in 0..=25 {
            let v = moon_intensity(step as f32 * 0.01, 0.3, 0.0);
            assert!(v <= prev + 1e-7);
            prev = v;
        }
        assert_eq!(prev, 0.0);
    }
}
