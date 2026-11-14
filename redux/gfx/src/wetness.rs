//! Rain on surfaces (LOOK-13): how wet the world is, written into the [`WET`] row that this
//! crate's `gfx_surface` body (`wet.wgsl`) reads. Rain darkens and saturates exposed terrain,
//! buildings and trees, glosses their tops with a sun highlight and a sky reflection, and leaves
//! puddles on flat ground.
//!
//! - The wetness rises while it rains (full in about 90 s at full rain, slower in a drizzle,
//!   which never quite soaks) and dries once it stops (about 4 min from soaked). It runs on real
//!   time, like the weather's own channels, so a paused menu does not dry the ground.
//! - The rain rate is the weather's effect density while its effect is rain (`WOW_WEATHER` forces
//!   one for tests).
//! - `WOW_WETNESS=<0..1>` pins the wetness, for captures that cannot wait for the ramp.
//!
//! The row: `x` the wetness, then three packed words, each a `u32` carried in an `f32`'s bits:
//! the direction toward the sun (`pack4x8snorm`), the sun's diffuse colour and the fog colour
//! (`pack4x8unorm`). Their fourth byte is fixed at 0x40, so each word reads as a normal float.
//! Off, or dry, the row is zero and the hook returns the stock colour.
//!
//! Ported from the everwood graphics fork of Benilla (MIT OR Apache-2.0), revision `cdc7130b`,
//! `benilla-world/src/weather/wetness.rs` (the integrator, its rates and `WOW_WETNESS`). The rain
//! ripple clock and its rings on water are not ported: Elderoth has no enhanced water yet.
use super::rows::WET;
use super::Enhanced;
use benilla_world::lighting::{GfxData, WowLighting};
use benilla_world::weather::{WeatherKind, WeatherState};
use bevy::prelude::*;

/// Seconds from dry to soaked under full rain. Source: everwood `WET_UP_S`.
const WET_UP_S: f32 = 90.0;
/// Seconds from soaked to dry once the rain stops. Source: everwood `DRY_S`.
const DRY_S: f32 = 240.0;

pub(super) struct WetnessPlugin;

impl Plugin for WetnessPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Wetness>()
            .add_systems(PostUpdate, update_wetness);
    }
}

/// The wetness integrator's state, 0 dry .. 1 soaked.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct Wetness(pub f32);

impl Wetness {
    /// One real-time step. `rain` is the rain rate 0..1 (0 when not raining).
    fn step(&mut self, rain: f32, dt: f32) {
        if rain > 0.0 {
            // A drizzle wets slower and never quite soaks.
            let target = 0.55 + 0.45 * rain.min(1.0);
            let rate = (0.35 + 0.65 * rain.min(1.0)) / WET_UP_S;
            if self.0 < target {
                self.0 = (self.0 + rate * dt).min(target);
            } else {
                self.0 = (self.0 - dt / DRY_S).max(target);
            }
        } else {
            self.0 = (self.0 - dt / DRY_S).max(0.0);
        }
    }
}

/// `WOW_WETNESS`, read once: a pinned wetness for captures.
fn pinned() -> Option<f32> {
    static PIN: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *PIN.get_or_init(|| {
        let v = std::env::var("WOW_WETNESS")
            .ok()?
            .trim()
            .parse::<f32>()
            .ok()?;
        Some(v.clamp(0.0, 1.0))
    })
}

/// A unit vector as `pack4x8snorm`, with the fourth byte fixed (see the module doc).
fn pack_dir(v: Vec3) -> f32 {
    let b = |c: f32| ((c.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8) as u32;
    f32::from_bits(b(v.x) | (b(v.y) << 8) | (b(v.z) << 16) | (0x40 << 24))
}

/// A colour 0..1 as `pack4x8unorm`, with the fourth byte fixed.
fn pack_rgb(c: [f32; 3]) -> f32 {
    let b = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    f32::from_bits(b(c[0]) | (b(c[1]) << 8) | (b(c[2]) << 16) | (0x40 << 24))
}

/// The `WET` row for a wetness: zero when dry.
pub(super) fn wet_row(wetness: f32, lighting: &WowLighting) -> [f32; 4] {
    if wetness <= 0.0 {
        return [0.0; 4];
    }
    [
        wetness,
        pack_dir(lighting.celestial_dir().normalize_or(Vec3::Y)),
        pack_rgb(lighting.diffuse),
        pack_rgb(lighting.fog_color),
    ]
}

fn update_wetness(
    enhanced: Res<Enhanced>,
    time: Res<Time<Real>>,
    weather: Option<Res<WeatherState>>,
    lighting: Res<WowLighting>,
    mut wet: ResMut<Wetness>,
    mut frame: ResMut<GfxData>,
) {
    let rain = weather
        .filter(|w| w.effect_kind == WeatherKind::Rain)
        .map_or(0.0, |w| w.effect_density.clamp(0.0, 1.0));
    wet.step(rain, time.delta_secs());
    let wetness = if enhanced.wetness {
        pinned().unwrap_or(wet.0)
    } else {
        0.0
    };
    let row = wet_row(wetness, &lighting);
    // Write only on change, so the frame's change detection stays meaningful.
    if frame.rows[WET] != row {
        frame.rows[WET] = row;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_rain_soaks_in_about_ninety_seconds_and_dries_in_four_minutes() {
        let mut w = Wetness::default();
        for _ in 0..(89 * 10) {
            w.step(1.0, 0.1);
        }
        assert!(w.0 > 0.95 && w.0 < 1.0, "{}", w.0);
        for _ in 0..(20 * 10) {
            w.step(1.0, 0.1);
        }
        assert_eq!(w.0, 1.0);
        for _ in 0..(120 * 10) {
            w.step(0.0, 0.1);
        }
        assert!((w.0 - 0.5).abs() < 0.01, "half dry at 2 min: {}", w.0);
        for _ in 0..(121 * 10) {
            w.step(0.0, 0.1);
        }
        assert_eq!(w.0, 0.0);
    }

    #[test]
    fn a_drizzle_never_quite_soaks() {
        let mut w = Wetness::default();
        for _ in 0..(600 * 10) {
            w.step(0.2, 0.1);
        }
        assert!((w.0 - 0.64).abs() < 1e-4, "{}", w.0);
    }

    #[test]
    fn the_packed_words_are_normal_floats_and_unpack_as_the_shader_reads_them() {
        for v in [Vec3::Y, Vec3::NEG_Y, Vec3::new(0.6, -0.8, 0.0), Vec3::ZERO] {
            let f = pack_dir(v);
            assert!(f.is_normal(), "{v} -> {f}");
            let bits = f.to_bits();
            let c = |i: u32| (((bits >> (8 * i)) & 0xff) as u8 as i8) as f32 / 127.0;
            assert!((Vec3::new(c(0), c(1), c(2)) - v).abs().max_element() < 0.01);
        }
        for rgb in [[0.0; 3], [1.0; 3], [0.25, 0.5, 0.75]] {
            let f = pack_rgb(rgb);
            assert!(f.is_normal());
            let bits = f.to_bits();
            for (i, c) in rgb.iter().enumerate() {
                let got = ((bits >> (8 * i)) & 0xff) as f32 / 255.0;
                assert!((got - c).abs() < 0.003);
            }
        }
    }

    #[test]
    fn dry_is_a_zero_row() {
        assert_eq!(wet_row(0.0, &WowLighting::default()), [0.0; 4]);
    }
}
