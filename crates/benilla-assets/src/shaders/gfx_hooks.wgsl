#define_import_path benilla::gfx_hooks

// The program's graphics hook points (an Elderoth engine patch). The engine's shaders call these
// at fixed points; this file is the engine's own body, the stock image, and a program may replace
// the whole module (`materials::GFX_HOOKS_SHADER`, keeping this import path and every signature)
// with bodies of its own, each behind its own shader def. Every hook is a pure function of its
// arguments: a vertex hook can be called again with last frame's data (`GfxData`) to find motion.
// The stock bodies compile to nothing beyond the 1.12 law they return.

#import benilla::gfx_data::GfxData

// `gfx_vertex_offset` kinds.
const GFX_VERTEX_CLUTTER: u32 = 1u; // ground clutter: attr.xy = UV_1, attr.zw = 0
const GFX_VERTEX_STATIC: u32 = 2u;  // a retained static batch: attr.xyz = placement anchor, attr.w = the vertex word (bitcast)

// `gfx_lit` kinds: which receiver lane is asking.
const GFX_LIT_TERRAIN: u32 = 1u; // ADT terrain, lit per vertex
const GFX_LIT_MODEL: u32 = 2u;   // an exterior M2, WMO or clutter batch on the model lane
const GFX_LIT_UNIT: u32 = 3u;    // a skinned rig: `world_pos` is the rig's origin, one sample per body
const GFX_LIT_STATIC: u32 = 4u;  // an exterior batch of the retained static pass

// The 1.12 fog survival: GL_LINEAR on planar eye-Z, 1 = no fog, 0 = fully fogged.
fn gfx_fog_linear(eye_z: f32, span: vec2<f32>) -> f32 {
    let denom = max(span.y - span.x, 0.001);
    return clamp((span.y - eye_z) / denom, 0.0, 1.0);
}

// The fog a surface stands in: rgb = the fog colour, w = how much of the surface survives.
// `fog_rgb` is the colour the caller's policy picked, `scene_colour` true when that is the scene's
// own fog colour; `span` = (start, end) yd, `eye_z` the planar eye depth (yd), positions in Bevy
// world space. The WDL hull passes its own (0, 1) span.
fn gfx_fog(
    fog_rgb: vec3<f32>,
    span: vec2<f32>,
    eye_z: f32,
    world_pos: vec3<f32>,
    camera_pos: vec3<f32>,
    scene_colour: bool,
    gfx: GfxData,
) -> vec4<f32> {
    return vec4<f32>(fog_rgb, gfx_fog_linear(eye_z, span));
}

// A world-space offset added to a vertex before projection; zero is the stock geometry.
fn gfx_vertex_offset(
    kind: u32,
    world_pos: vec3<f32>,
    attr: vec4<f32>,
    camera_pos: vec3<f32>,
    gfx: GfxData,
) -> vec3<f32> {
    return vec3<f32>(0.0);
}

// The sky dome's colour along unit `dir` at `elev` degrees: `col` is the stock gradient, `stops`
// its six stops (horizon fog colour, rings 4 to 1 after the dawn warp, zenith), `frag_xy` the
// fragment's pixel.
fn gfx_sky(
    col: vec3<f32>,
    elev: f32,
    dir: vec3<f32>,
    stops: array<vec3<f32>, 6>,
    frag_xy: vec2<f32>,
    gfx: GfxData,
) -> vec3<f32> {
    return col;
}

// A cloud-dome texel (straight alpha, before the rim fade): `texel` is the stock sample of
// `tex` at `uv`.
fn gfx_cloud(
    texel: vec4<f32>,
    tex: texture_2d<f32>,
    samp: sampler,
    uv: vec2<f32>,
    gfx: GfxData,
) -> vec4<f32> {
    return texel;
}

// The light reaching an exterior surface: `lit` is the stock light sum (clamped, gamma), `ambient`
// the part of it that no sun ray carries (what a fully shadowed surface keeps), `world_pos` the
// receiving point (a unit's rig origin for `GFX_LIT_UNIT`), `normal` its surface normal.
// `shadow_map` and `shadow_cmp` are the view's directional shadow array and its comparison
// sampler; the stock body reads neither. Returns rgb = the light sum to use, w = how much of the
// direct light arrives (1 = all, the stock value), for a receiver's own sun-only terms.
fn gfx_lit(
    kind: u32,
    lit: vec3<f32>,
    ambient: vec3<f32>,
    world_pos: vec3<f32>,
    normal: vec3<f32>,
    camera_pos: vec3<f32>,
    shadow_map: texture_depth_2d_array,
    shadow_cmp: sampler_comparison,
    gfx: GfxData,
) -> vec4<f32> {
    return vec4<f32>(lit, 1.0);
}

// A lit exterior surface's colour (gamma, before fog): `rgb` is the stock colour, `albedo` the
// texel the receiver lit, `light` the light that multiplied it (so `albedo · light` is the lit
// part of `rgb`), `direct` how much direct light arrives (`gfx_lit`'s w), `world_pos` and `normal`
// the fragment's. Called by the same lanes as `gfx_lit`, with its kinds. Stock returns `rgb`.
fn gfx_surface(
    kind: u32,
    rgb: vec3<f32>,
    albedo: vec3<f32>,
    light: vec3<f32>,
    direct: f32,
    world_pos: vec3<f32>,
    normal: vec3<f32>,
    camera_pos: vec3<f32>,
    gfx: GfxData,
) -> vec3<f32> {
    return rgb;
}

// How far the baked shade (terrain MCSH, the doodads' 0.5 band, an entity's ground-shade byte)
// steps aside at `world_pos`, 0 = the stock bake, 1 = fully replaced by the program's own shadow.
fn gfx_baked_aside(world_pos: vec3<f32>, camera_pos: vec3<f32>, gfx: GfxData) -> f32 {
    return 0.0;
}
