//! The modern fog (LOOK-12): fills the fog rows of the engine's `GfxData` every frame after the
//! light resolve, and turns on the FFX combine's sky dither, which hides the 8-bit banding the
//! smoother fog and sky gradients show (it runs in the combine, after the main image is final).
//! The law itself is [`fog.wgsl`](fog.wgsl), which the hook bodies call under `GFX_FOG`; the
//! full-screen haze is [`super::haze`].
//!
//! Ported from the everwood graphics fork of Benilla (`benilla-world/src/lighting/fog_model.rs`,
//! `derived_band` and `fill_frame`). Everwood can override the derivation from a modern client's
//! `LightFogBand.dbc`; Elderoth reads nothing from a modern client, so the derivation from the
//! 1.12 bands is the whole model. Everwood's farclip stretch is not ported: Elderoth keeps the
//! zone's fog distances. Off, every fog row is zero, which every receiver reads as the 1.12 fog.
use super::rows::{oct_encode, FOG_A, FOG_B, FOG_C, FOG_D};
use super::Enhanced;
use benilla_world::{
    ffx_glow::FfxDither,
    lighting::{GfxData, LightingResolveSet, WowLighting},
};
use bevy::prelude::*;

/// How far the sun-fog colour leans from the fog colour toward the sun colour (IntBand 9).
/// Source: everwood `DERIVED_SUN_FOG_LEAN`.
const SUN_FOG_LEAN: f32 = 0.4;
/// How far the end-fog colour leans from the fog colour toward the 1.8° sky ring. Source:
/// everwood `DERIVED_END_FOG_LEAN`.
const END_FOG_LEAN: f32 = 0.25;
/// The sun-fog lobe's strength. Source: everwood `DERIVED_SUN_FOG_STRENGTH`.
const SUN_FOG_STRENGTH: f32 = 0.25;
/// The cosine the sun-fog lobe starts at. Source: everwood `DERIVED_SUN_FOG_ANGLE`.
const SUN_FOG_ANGLE: f32 = 0.4;

pub(super) struct FogPlugin;

impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, fill_fog.after(LightingResolveSet));
    }
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// The sun-fog day factor from the visible sun's height (`sin(elevation)`): full from about 3°
/// up, gone by about 6° below the horizon. Source: everwood `sun_fog_day`.
pub(super) fn sun_fog_day(sun_y: f32) -> f32 {
    let t = ((sun_y + 0.1) / 0.15).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The fog rows for the resolved light; all zero when `on` is false.
fn fog_rows(on: bool, l: &WowLighting, frame: &mut GfxData) {
    if !on {
        frame.rows[FOG_A] = [0.0; 4];
        frame.rows[FOG_B] = [0.0; 4];
        frame.rows[FOG_C] = [0.0; 4];
        frame.rows[FOG_D] = [0.0; 4];
        return;
    }
    let sun = l.celestial_dir();
    let (_, end) = l.fog_span();
    let sun_rgb = mix3(l.fog_color, l.spec, SUN_FOG_LEAN);
    let end_rgb = mix3(l.fog_color, l.sky()[4], END_FOG_LEAN);
    let dir = oct_encode(sun);
    // No height fog and no blend back to the linear ramp: everwood's derived band.
    frame.rows[FOG_A] = [0.0; 4];
    frame.rows[FOG_B] = [
        sun_rgb[0],
        sun_rgb[1],
        sun_rgb[2],
        SUN_FOG_STRENGTH * sun_fog_day(sun.y),
    ];
    frame.rows[FOG_C] = [end_rgb[0], end_rgb[1], end_rgb[2], end];
    // The shader's "on" test is `FOG_D.x > 0.5`; a fog end under a yard never occurs outdoors.
    frame.rows[FOG_D] = [end.max(1.0), SUN_FOG_ANGLE, dir[0], dir[1]];
}

fn fill_fog(
    enhanced: Res<Enhanced>,
    lighting: Res<WowLighting>,
    mut frame: ResMut<GfxData>,
    mut dither: ResMut<FfxDither>,
) {
    let mut next = *frame;
    fog_rows(enhanced.fog, &lighting, &mut next);
    if *frame != next {
        *frame = next;
    }
    if dither.0 != enhanced.fog {
        dither.0 = enhanced.fog;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_leaves_every_fog_row_zero() {
        let mut f = GfxData::default();
        f.rows[FOG_D] = [1.0; 4];
        fog_rows(false, &WowLighting::default(), &mut f);
        assert_eq!(f, GfxData::default());
    }

    #[test]
    fn the_sun_lobe_fades_through_the_horizon() {
        assert_eq!(sun_fog_day(1.0), 1.0);
        assert_eq!(sun_fog_day(-0.2), 0.0);
        let dusk = sun_fog_day(0.0);
        assert!(dusk > 0.0 && dusk < 1.0);
    }
}
