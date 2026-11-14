#define_import_path benilla::gfx_hooks

// This crate's bodies for the engine's graphics hook points (engine patch 0009, whose stock module
// `benilla-assets/src/shaders/gfx_hooks.wgsl` this replaces at `materials::GFX_HOOKS_SHADER`; the
// import path, constants and signatures must stay the engine's). `hooks.rs` installs it with one
// shader def per effect that is on, `GFX_FOG`, `GFX_WIND`, `GFX_SKY`,
// `GFX_SHADOWS`, `GFX_MOONLIGHT`, `GFX_WET` and `GFX_FIRE`; an effect
// that is off compiles to the stock body. Every hook stays a pure function of its arguments.

#import benilla::gfx_data::GfxData
#import redux::gfx_rows
#ifdef GFX_FOG
#import redux::gfx_fog as fog_fx
#endif
#ifdef GFX_WIND
#import redux::gfx_wind as wind_fx
#endif
#ifdef GFX_SKY
#import redux::gfx_sky as sky_fx
#endif
#ifdef GFX_SHADOWS
#import redux::gfx_shadow as shadow_fx
#endif
#ifdef GFX_MOONLIGHT
#import redux::gfx_moon as moon_fx
#endif
#ifdef GFX_WET
#import redux::gfx_wet as wet_fx
#endif
#ifdef GFX_FIRE
#import redux::gfx_fire as fire_fx
#endif

const GFX_VERTEX_CLUTTER: u32 = 1u;
const GFX_VERTEX_STATIC: u32 = 2u;
const GFX_LIT_TERRAIN: u32 = 1u;
const GFX_LIT_MODEL: u32 = 2u;
const GFX_LIT_UNIT: u32 = 3u;
const GFX_LIT_STATIC: u32 = 4u;
// The vertex word's leaf-card bit (`static_gx::WORD_FOLIAGE_WIND`, engine patch 0010).
const WORD_FOLIAGE_WIND: u32 = 536870912u; // 1 << 29

fn gfx_fog_linear(eye_z: f32, span: vec2<f32>) -> f32 {
    let denom = max(span.y - span.x, 0.001);
    return clamp((span.y - eye_z) / denom, 0.0, 1.0);
}

fn gfx_fog(
    fog_rgb: vec3<f32>,
    span: vec2<f32>,
    eye_z: f32,
    world_pos: vec3<f32>,
    camera_pos: vec3<f32>,
    scene_colour: bool,
    gfx: GfxData,
) -> vec4<f32> {
#ifdef GFX_FOG
    return fog_fx::fog_sample(fog_rgb, span, eye_z, world_pos, camera_pos, scene_colour, gfx);
#else
    return vec4<f32>(fog_rgb, gfx_fog_linear(eye_z, span));
#endif
}

fn gfx_vertex_offset(
    kind: u32,
    world_pos: vec3<f32>,
    attr: vec4<f32>,
    camera_pos: vec3<f32>,
    gfx: GfxData,
) -> vec3<f32> {
#ifdef GFX_WIND
    if (kind == GFX_VERTEX_CLUTTER) {
        return wind_fx::grass_offset(world_pos, attr.x, attr.y, camera_pos, gfx);
    }
    if (kind == GFX_VERTEX_STATIC && (bitcast<u32>(attr.w) & WORD_FOLIAGE_WIND) != 0u) {
        return wind_fx::tree_offset(world_pos, attr.xyz, camera_pos, gfx);
    }
#endif
    return vec3<f32>(0.0);
}

fn gfx_sky(
    col: vec3<f32>,
    elev: f32,
    dir: vec3<f32>,
    stops: array<vec3<f32>, 6>,
    frag_xy: vec2<f32>,
    gfx: GfxData,
) -> vec3<f32> {
#ifdef GFX_SKY
    let fx = array<vec4<f32>, 3>(
        gfx.rows[gfx_rows::SKY], gfx.rows[gfx_rows::SKY + 1u], gfx.rows[gfx_rows::SKY + 2u],
    );
    let fog = array<vec4<f32>, 3>(
        gfx.rows[gfx_rows::FOG_B], gfx.rows[gfx_rows::FOG_C], gfx.rows[gfx_rows::FOG_D],
    );
    return sky_fx::shade_sky(elev, dir, stops, fx, fog, gfx.rows[gfx_rows::CLOCK].x, frag_xy);
#else
#ifdef GFX_FOG
    // The modern fog's low band on the stock gradient.
    return fog_fx::fog_sky_horizon(
        col, stops[0], dir,
        gfx.rows[gfx_rows::FOG_B], gfx.rows[gfx_rows::FOG_C], gfx.rows[gfx_rows::FOG_D],
    );
#else
    return col;
#endif
#endif
}

fn gfx_cloud(
    texel: vec4<f32>,
    tex: texture_2d<f32>,
    samp: sampler,
    uv: vec2<f32>,
    gfx: GfxData,
) -> vec4<f32> {
#ifdef GFX_SKY
    let fx = array<vec4<f32>, 2>(gfx.rows[gfx_rows::CLOUD], gfx.rows[gfx_rows::CLOUD + 1u]);
    return sky_fx::shade_cloud(tex, samp, uv, texel, fx, gfx.rows[gfx_rows::CLOCK].x);
#else
    return texel;
#endif
}

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
    var out = vec4<f32>(lit, 1.0);
    var vis = 1.0;
#ifdef GFX_SHADOWS
    vis = shadow_fx::visibility(
        kind == GFX_LIT_UNIT, world_pos, normal, camera_pos, shadow_map, shadow_cmp, gfx,
    );
    if (kind == GFX_LIT_TERRAIN) {
        out = vec4<f32>(shadow_fx::shade_whole(lit, vis, gfx), vis);
    } else {
        out = vec4<f32>(shadow_fx::shade(lit, ambient, vis, gfx), vis);
    }
    // The shadow-flicker probe's bare-visibility view (`shadows.rs`).
    if ((u32(gfx.rows[gfx_rows::SHADOW_TEXEL].w) & 1u) != 0u) {
        return vec4<f32>(vec3<f32>(vis), vis);
    }
#endif
#ifdef GFX_MOONLIGHT
    out = vec4<f32>(out.rgb + moon_fx::moon_light(normal, vis, gfx), out.w);
#endif
    return out;
}

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
    var out = rgb;
#ifdef GFX_WET
    // A unit's body keeps its look: the rain law is for the world's surfaces.
    if (kind != GFX_LIT_UNIT) {
        out = wet_fx::wet(
            kind == GFX_LIT_TERRAIN, rgb, albedo, light, direct, world_pos, normal, camera_pos,
            1.0, gfx.rows[gfx_rows::WET],
        );
    }
#endif
#ifdef GFX_FIRE
    // Firelight lights the albedo, units' bodies included.
    out += albedo * fire_fx::fire_light(world_pos, normal, gfx);
#endif
    return out;
}

fn gfx_baked_aside(world_pos: vec3<f32>, camera_pos: vec3<f32>, gfx: GfxData) -> f32 {
#ifdef GFX_SHADOWS
    return shadow_fx::baked_aside(world_pos, camera_pos, gfx);
#else
    return 0.0;
#endif
}
