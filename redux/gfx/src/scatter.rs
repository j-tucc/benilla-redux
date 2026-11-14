//! Sunlit air (LOOK-16): the air scatters the sun's light toward the eye, so the sun glows through
//! the haze and fog, the sky brightens and tints around it, and distant trees fade into lit air
//! rather than flat fog colour. Strongest when the sun is low and the air thick.
//!
//! The haze pass draws it ([`super::haze`], `haze.wgsl`'s `sunlit_air`) from the two rows this
//! module builds: a forward-scattering Henyey-Greenstein phase (a tight core and a broad halo, the
//! usual two-lobe stand-in for Mie scattering by haze droplets) times the optical depth of the air
//! along the view ray, in the sun's colour leaned toward the zone's fog hue, so Duskwood's glow
//! stays green and a dusk's gold. The sky takes the whole path, so the horizon behind a far tree
//! glows as the tree's own air does.
//!
//! It reads only its own switch: off, both rows are zero and the pass adds nothing.
use bevy::prelude::*;

/// Henyey-Greenstein anisotropy of the core lobe: the bright disc of light round the sun.
/// Deviation: no source value; 0.85 is in the range published for haze and fog droplets
/// (0.7–0.9, e.g. Nishita et al. 1993, Mie aerosol g ≈ 0.76–0.85), tuned in stills against the
/// LOOK-16 references.
const G_CORE: f32 = 0.85;
/// Anisotropy of the broad lobe: the wide brightening of the sky and air on the sun's side.
/// Deviation: no source value; tuned in stills against the LOOK-16 references.
const G_WIDE: f32 = 0.45;
/// The core lobe's share of the phase. Deviation: tuned in stills.
const CORE_SHARE: f32 = 0.16;
/// The glow's gain with the sun on the horizon, and overhead. A low sun shines through more air.
/// Deviation: no source value; tuned in stills so dawn and dusk glow without blowing out noon.
const GAIN_LOW: f32 = 1.5;
const GAIN_HIGH: f32 = 0.35;
/// The scattering air's density, relative to the haze's extinction (`haze::density`).
/// Deviation: tuned in stills; the haze's own law (everwood) sets the shape over the day.
const DENSITY_SCALE: f32 = 1.5;
/// The most the scattering air holds per yard: the haze's dawn peak (2.5× its noon density) would
/// whiten the whole view toward a low sun. Deviation: tuned in stills.
const MAX_DENSITY: f32 = 0.009;
/// How far the in-scatter integrates along a ray (yd) at most: the sky's path.
/// Deviation: tuned so the horizon's glow matches the far terrain's under the 1.12 fog spans.
const REACH: f32 = 450.0;
/// How far the glow's colour leans from the sun's light toward the zone fog's hue.
/// Deviation: everwood's sky glow (`sky_fx_inputs`) leans the fog colour 0.6 toward the sun's
/// tint; the air's glow leans the sun's light 0.6 toward the fog's hue instead, at the sun's
/// luminance, so a strongly tinted zone keeps its colour without darkening the glow.
const FOG_LEAN: f32 = 0.6;

/// The glow's day fade from the visible sun's height (`sin(elevation)`): full once it clears the
/// horizon, gone a few degrees under it. Source: everwood `sky_fx::glow_day_fade`.
fn day_fade(sun_y: f32) -> f32 {
    let t = ((sun_y + 0.10) / 0.16).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The glow's gain for the visible sun's height and the storm blend: strongest on the horizon,
/// faded below it, and dimmed by storm cloud (everwood `glow_strength`'s 0.85 storm dim).
pub(super) fn gain(sun_y: f32, bcc: f32) -> f32 {
    let low = 1.0 - sun_y.clamp(0.0, 1.0);
    let height = GAIN_HIGH + (GAIN_LOW - GAIN_HIGH) * low * low;
    height * day_fade(sun_y) * (1.0 - 0.85 * bcc.clamp(0.0, 1.0))
}

/// The glow's colour: the sun's light, lifted a little toward white, leaned toward the fog's hue
/// at the sun's luminance.
pub(super) fn colour(sun: Vec3, fog: Vec3) -> Vec3 {
    let sun = sun.lerp(Vec3::ONE, 0.25);
    let luma = Vec3::new(0.2126, 0.7152, 0.0722);
    let l = fog.dot(luma);
    if l <= 1e-4 {
        return sun;
    }
    let hue = (fog / l).min(Vec3::splat(2.0));
    sun * Vec3::ONE.lerp(hue, FOG_LEAN)
}

/// The haze pass's two sunlit-air rows: (direction toward the visible sun, scattering density
/// per yard), (glow colour with the gain folded in, reach in yd). Zero when off or indoors.
pub(super) fn rows(
    on: bool,
    to_sun: Vec3,
    sun: Vec3,
    fog: Vec3,
    bcc: f32,
    density: f32,
    indoors: bool,
) -> [[f32; 4]; 2] {
    let to_sun = to_sun.normalize_or_zero();
    let g = gain(to_sun.y, bcc);
    if !on || indoors || g <= 0.0 || density <= 0.0 {
        return [[0.0; 4]; 2];
    }
    let c = colour(sun, fog) * g;
    [
        [
            to_sun.x,
            to_sun.y,
            to_sun.z,
            (density * DENSITY_SCALE).min(MAX_DENSITY),
        ],
        [c.x, c.y, c.z, REACH],
    ]
}

/// The phase lobes, for the shader's third row: (core g, broad g, core share, 0).
pub(super) const PHASE: [f32; 4] = [G_CORE, G_WIDE, CORE_SHARE, 0.0];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glow_is_strongest_low_and_gone_below_the_horizon() {
        assert_eq!(gain(-0.2, 0.0), 0.0);
        let dawn = gain(0.08, 0.0);
        let noon = gain(1.0, 0.0);
        assert!(dawn > 2.0 * noon, "{dawn} {noon}");
        assert!(gain(0.08, 1.0) < 0.2 * dawn);
    }

    #[test]
    fn the_glow_keeps_the_zone_fog_hue() {
        let white = Vec3::ONE;
        let green = colour(white, Vec3::new(0.2, 0.5, 0.2));
        assert!(green.y > green.x && green.y > green.z);
        let gold = colour(Vec3::new(1.0, 0.7, 0.4), Vec3::new(0.8, 0.5, 0.3));
        assert!(gold.x > gold.y && gold.y > gold.z);
    }

    #[test]
    fn off_or_indoors_the_rows_are_zero() {
        let up = Vec3::new(0.3, 0.2, 0.9);
        let fog = Vec3::splat(0.5);
        assert_eq!(
            rows(false, up, Vec3::ONE, fog, 0.0, 0.004, false),
            [[0.0; 4]; 2]
        );
        assert_eq!(
            rows(true, up, Vec3::ONE, fog, 0.0, 0.004, true),
            [[0.0; 4]; 2]
        );
        let on = rows(true, up, Vec3::ONE, fog, 0.0, 0.004, false);
        assert!(on[0][3] > 0.0 && on[1][0] > 0.0);
    }
}
