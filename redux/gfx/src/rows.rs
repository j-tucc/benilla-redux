//! What this crate keeps in the engine's `lighting::GfxData` rows: the per-frame inputs of its hook
//! bodies, Bevy world space (Y up, 1 unit = 1 yd). [`rows.wgsl`](rows.wgsl) mirrors the indices.
//! Each effect writes only its own rows, and zero rows when it is off.
//!
//! Rows [`USED`]..[`PREVIOUS`] are free; rows [`PREVIOUS`].. are kept for the previous frame's copy
//! of the rows the vertex hook reads ([`WIND_A`] up to [`SKY`]: the wind and the benders), which a
//! motion-vector pass would hand the vertex hook to find last frame's position. Nothing writes
//! them yet.
use bevy::prelude::*;

/// Height fog: density, height (world Y), falloff (1/yd), curve blend toward the 1.12 ramp.
pub const FOG_A: usize = 0;
/// Sun-fog colour (gamma 0..1), sun-fog strength.
pub const FOG_B: usize = 1;
/// End-fog colour (gamma 0..1), end-fog distance (yd).
pub const FOG_C: usize = 2;
/// The scene fog end (yd, 0 = the 1.12 fog), the sun lobe's cosine threshold, the sun direction
/// octahedral-encoded ([`oct_encode`]).
pub const FOG_D: usize = 3;
/// Wind direction (unit, world XZ), base heading (rad), gust 0..1.
pub const WIND_A: usize = 4;
/// Wind travel (yd, wrapped at 4096), sway strength, grass strength, tree strength.
pub const WIND_B: usize = 5;
/// Bender count, then firelight's live count, gain and flicker clock (`firelight.rs`).
pub const MISC: usize = 6;
/// The benders: world xyz and radius (yd); the first `MISC.x` are live.
pub const BENDERS: usize = 7;
/// How many benders the rows hold.
pub const MAX_BENDERS: usize = 8;
/// The sky dome's three rows: (tier, 0, night-sky alpha, glow), the sun, the glow colour.
pub const SKY: usize = 15;
/// The cloud dome's two rows: (tier, 0, glow direction on the sheet), (sun colour, march).
pub const CLOUD: usize = 18;
/// The sky clock: `x` seconds, wrapped at [`SKY_CLOCK_WRAP_S`].
pub const CLOCK: usize = 20;
/// The sun-shadow cascade's `clip_from_world`, four columns, written from the render world each
/// frame (after the light upload) so it matches the map Bevy draws that frame.
pub const SHADOW_CLIP: usize = 21;
/// The cascade's normal bias (yd), 1 when the map is live this frame, the dapples' weight and the
/// probe's bare-visibility view (`shadows.rs`); written with the columns.
pub const SHADOW_TEXEL: usize = 25;
/// Shadow weight on the light (0..1), shadow distance (yd), the sun floor, the bake step-aside.
pub const SHADOW_A: usize = 26;
/// The rig's direction toward its body (unit), the depth bias (yd).
pub const SHADOW_B: usize = 27;
/// Moonlight: direction toward the moon (unit), intensity.
pub const MOON_A: usize = 28;
/// Moonlight: tint (gamma), how much of the shadow map holds the moon (0..1).
pub const MOON_B: usize = 29;
/// Ground mist: density, base height (world Y), falloff (1/yd), the mist's reach (yd).
pub const MIST: usize = 30;
/// Rain on surfaces: wetness 0..1, then three packed words (`wetness.rs`, `wet.wgsl`): the
/// direction toward the sun (snorm8), the sun's diffuse colour and the fog colour (unorm8).
pub const WET: usize = 31;
/// Firelight: the nearest outdoor point lights, world xyz and a packed colour word
/// (`firelight.rs`); the first `MISC.y` are live.
pub const FIRES: usize = 32;
/// How many lights the firelight rows hold.
pub const MAX_FIRES: usize = 12;
/// Rows in use.
pub const USED: usize = FIRES + MAX_FIRES;
/// The first row of the previous frame's copy of the vertex hook's rows.
pub const PREVIOUS: usize = 48;

/// The sky clock's wrap: a day. The sky's oscillators make whole cycles per wrap.
pub const SKY_CLOCK_WRAP_S: f64 = 86_400.0;

const _: () = assert!(BENDERS + MAX_BENDERS == SKY);
const _: () = assert!(CLOCK < SHADOW_CLIP && SHADOW_CLIP + 4 == SHADOW_TEXEL);
const _: () = assert!(WET + 1 == FIRES && USED <= PREVIOUS);
const _: () = assert!(PREVIOUS + (SKY - WIND_A) <= benilla_world::lighting::GFX_ROWS);

/// Octahedral encoding of a unit vector into `[-1, 1]²` (the shaders' `fog_oct_decode`).
pub fn oct_encode(v: Vec3) -> [f32; 2] {
    let v = v.normalize_or(Vec3::Y);
    let n = v / (v.x.abs() + v.y.abs() + v.z.abs());
    if n.y >= 0.0 {
        [n.x, n.z]
    } else {
        let sx = if n.x >= 0.0 { 1.0 } else { -1.0 };
        let sz = if n.z >= 0.0 { 1.0 } else { -1.0 };
        [(1.0 - n.z.abs()) * sx, (1.0 - n.x.abs()) * sz]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rust_and_wgsl_indices_agree() {
        let src = include_str!("rows.wgsl");
        for (name, row) in [
            ("FOG_A", FOG_A),
            ("FOG_B", FOG_B),
            ("FOG_C", FOG_C),
            ("FOG_D", FOG_D),
            ("WIND_A", WIND_A),
            ("WIND_B", WIND_B),
            ("MISC", MISC),
            ("BENDERS", BENDERS),
            ("SKY", SKY),
            ("CLOUD", CLOUD),
            ("CLOCK", CLOCK),
            ("SHADOW_CLIP", SHADOW_CLIP),
            ("SHADOW_TEXEL", SHADOW_TEXEL),
            ("SHADOW_A", SHADOW_A),
            ("SHADOW_B", SHADOW_B),
            ("MOON_A", MOON_A),
            ("MOON_B", MOON_B),
            ("MIST", MIST),
            ("WET", WET),
            ("FIRES", FIRES),
            ("MAX_FIRES", MAX_FIRES),
        ] {
            assert!(
                src.contains(&format!("const {name}: u32 = {row}u;")),
                "{name}"
            );
        }
    }

    #[test]
    fn oct_encoding_round_trips_the_axes() {
        assert_eq!(oct_encode(Vec3::Y), [0.0, 0.0]);
        assert_eq!(oct_encode(Vec3::X), [1.0, 0.0]);
        assert_eq!(oct_encode(Vec3::NEG_Y), [1.0, 1.0]);
    }
}
