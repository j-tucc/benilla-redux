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
