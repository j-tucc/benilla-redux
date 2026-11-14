#define_import_path redux::gfx_fog

// The modern distance fog (LOOK-12; from the everwood graphics fork's `fog_hook.wgsl`), called by
// this crate's `benilla::gfx_hooks` bodies (`hooks.wgsl`) under `GFX_FOG`: `fog_sample` from every
// fog receiver (terrain, wow_model, static_gx, liquid, wow_effect, the WDL hull) and
// `fog_sky_horizon` from the sky. Pure functions: every value arrives as an argument.
//
// Classic, `FOG_D.x == 0`, returns the 1.12 law exactly as the receivers wrote it: GL_LINEAR on
// PLANAR eye-Z in gamma space, `factor = clamp((end - eye_z) / max(end - start, 0.001), 0, 1)`.
//
// Modern (`FOG_D.x` = the scene fog end, yd, when on):
// - survival: radial distance, exponential from the zone's start, fitted so 60 % survives at the
//   middle of the 1.12 span, closed linearly over the last stretch (from max(start, 0.3 end)) so
//   the fog still ends where the zone authored it; optional height fog below `FOG_A.y`;
//   `FOG_A.w` blends back toward the 1.12 linear ramp.
// - colour (the scene colour only, never the black, white or grey policies of additive, Mod and
//   Mod2x batches): shifts to the end-fog colour `FOG_C` by (d / end) cubed, then leans to the
//   sun-fog colour `FOG_B` inside a cubic lobe around the sun (cosine threshold `FOG_D.y`,
//   direction octahedral in `FOG_D.zw`).
// - only spans ending at the scene fog end take it: an interior WMO fog pair stays classic.
// - the WDL hull's own (0, 1) pair asks for the horizon: fully fogged, in the far colour.

#import benilla::gfx_data::GfxData
#import redux::gfx_rows

// The classic survival factor: 1 = no fog, 0 = fully fogged.
fn fog_linear(eye_z: f32, span: vec2<f32>) -> f32 {
    let denom = max(span.y - span.x, 0.001);
    return clamp((span.y - eye_z) / denom, 0.0, 1.0);
}

// exp survival at the span midpoint = 0.6, i.e. k (end - start) = -2 ln 0.6.
const FOG_EXP_MID: f32 = 1.0216512;

// The unit vector packed by `rows::oct_encode`.
fn fog_oct_decode(e: vec2<f32>) -> vec3<f32> {
    var v = vec3<f32>(e.x, 1.0 - abs(e.x) - abs(e.y), e.y);
    let t = max(-v.y, 0.0);
    v.x += select(t, -t, v.x >= 0.0);
    v.z += select(t, -t, v.z >= 0.0);
    return normalize(v);
}

// Whether the Modern model is on for this frame.
fn fog_is_modern(sel_row: vec4<f32>) -> bool {
    return sel_row.x > 0.5;
}

// The Modern fog colour seen along unit `dir` at radial distance `d` (yd), from the scene fog
// colour `base`: end-fog shift, then the sun lobe. `d` may be huge (the sky).
fn fog_modern_colour(
    base: vec3<f32>,
    dir: vec3<f32>,
    d: f32,
    sun_row: vec4<f32>,
    end_row: vec4<f32>,
    sel_row: vec4<f32>,
) -> vec3<f32> {
    var c = base;
    if (end_row.w > 0.0) {
        let t = clamp(d / end_row.w, 0.0, 1.0);
        c = mix(c, end_row.rgb, t * t * t);
    }
    if (sun_row.w > 0.0) {
        let sun = fog_oct_decode(sel_row.zw);
        let a = sel_row.y;
        let s = clamp((dot(dir, sun) - a) / max(1.0 - a, 0.001), 0.0, 1.0);
        c = mix(c, sun_row.rgb, sun_row.w * s * s * s);
    }
    return c;
}

// The Modern survival (1 = no fog) at radial distance `d` for the pair `span`.
fn fog_modern_survival(span: vec2<f32>, d: f32, world_y: f32, height_row: vec4<f32>) -> f32 {
    let len = max(span.y - span.x, 0.001);
    let k = FOG_EXP_MID / len;
    let x = max(d - span.x, 0.0);
    var expo = exp(-k * x);
    if (height_row.x > 0.0) {
        // Extra density at and below the plane, fading out `1 / falloff` yd above it.
        let hf = clamp(1.0 - (world_y - height_row.y) * height_row.z, 0.0, 1.0);
        expo = mix(expo, exp(-k * (1.0 + height_row.x) * x), hf);
    }
    let fade_from = max(span.x, 0.3 * span.y);
    let end_fade = clamp((span.y - d) / max(span.y - fade_from, 0.001), 0.0, 1.0);
    let linear = clamp((span.y - d) / len, 0.0, 1.0);
    return mix(min(expo, end_fade), linear, clamp(height_row.w, 0.0, 1.0));
}

// The sky dome's low band under Modern. `col` is the dome's own colour along unit `dir`,
// `horizon_rgb` the colour it uses at 0 degrees (the scene fog colour); the far fog colour along
// `dir` replaces the horizon and eases into the dome's own gradient by 15 degrees of elevation (a
// mix, never an offset). Classic returns `col` untouched. The rows are `FOG_B`, `FOG_C` and `FOG_D`.
fn fog_sky_horizon(
    col: vec3<f32>,
    horizon_rgb: vec3<f32>,
    dir: vec3<f32>,
    sun_row: vec4<f32>,
    end_row: vec4<f32>,
    sel_row: vec4<f32>,
) -> vec3<f32> {
    if (!fog_is_modern(sel_row)) {
        return col;
    }
    let far = fog_modern_colour(horizon_rgb, dir, 1.0e9, sun_row, end_row, sel_row);
    let elev = asin(clamp(dir.y, -1.0, 1.0));
    let w = 1.0 - smoothstep(0.0, 0.26179939, elev);
    return mix(col, far, w);
}

// The fog a surface stands in: rgb = the fog colour, w = how much of the surface survives (1 = no
// fog). `fog_rgb` is the colour the caller's policy picked; `scene_colour` is true when that is
// the scene's own fog colour. `span` = (start, end) yd, `eye_z` = planar eye depth (yd),
// `world_pos` and `camera_pos` in Bevy world space.
fn fog_sample(
    fog_rgb: vec3<f32>,
    span: vec2<f32>,
    eye_z: f32,
    world_pos: vec3<f32>,
    camera_pos: vec3<f32>,
    scene_colour: bool,
    gfx: GfxData,
) -> vec4<f32> {
    let classic = vec4<f32>(fog_rgb, fog_linear(eye_z, span));
    if (gfx.rows[gfx_rows::FOG_D].x > 0.5) {
        let delta = world_pos - camera_pos;
        let d = length(delta);
        let dir = delta / max(d, 0.0001);
        if (span.x == 0.0 && span.y == 1.0) {
            // The WDL hull's own pair: the horizon, fully fogged in the far colour.
            return vec4<f32>(fog_modern_colour(fog_rgb, dir, d, gfx.rows[gfx_rows::FOG_B], gfx.rows[gfx_rows::FOG_C], gfx.rows[gfx_rows::FOG_D]), 0.0);
        }
        if (abs(span.y - gfx.rows[gfx_rows::FOG_D].x) > 0.01) {
            // Not the scene fog (an interior WMO pair): classic.
            return classic;
        }
        var rgb = fog_rgb;
        if (scene_colour) {
            rgb = fog_modern_colour(fog_rgb, dir, d, gfx.rows[gfx_rows::FOG_B], gfx.rows[gfx_rows::FOG_C], gfx.rows[gfx_rows::FOG_D]);
        }
        return vec4<f32>(rgb, fog_modern_survival(span, d, world_pos.y, gfx.rows[gfx_rows::FOG_A]));
    }
    return classic;
}
