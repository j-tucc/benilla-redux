#define_import_path redux::gfx_moon

// Moonlight (LOOK-14), called by this crate's `gfx_lit` body: a cool wrapped-Lambert light from the
// white moon added to every exterior surface's light at night, shadowed by the shadow rig's map
// once it holds the moon. Rows `MOON_A` (direction to the moon, intensity) and `MOON_B` (tint, how
// much of the map holds the moon), filled by `moonlight.rs`.
//
// Ported from the everwood graphics fork of Benilla (MIT OR Apache-2.0), revision `cdc7130b`,
// `benilla-assets/src/shaders/moonlight_hook.wgsl` (`moon_light`, the wrap).

#import benilla::gfx_data::GfxData
#import redux::gfx_rows

// How far the light wraps past the terminator. Source: everwood `MOON_WRAP`.
const MOON_WRAP: f32 = 0.2;

// The moon's light on a surface with `normal`, `vis` the shadow map's visibility there.
fn moon_light(normal: vec3<f32>, vis: f32, gfx: GfxData) -> vec3<f32> {
    let a = gfx.rows[gfx_rows::MOON_A];
    if (a.w <= 0.0) {
        return vec3<f32>(0.0);
    }
    let b = gfx.rows[gfx_rows::MOON_B];
    let len2 = dot(normal, normal);
    var ndl = 0.5;
    if (len2 > 1e-12) {
        let n = normal * inverseSqrt(len2);
        ndl = max((dot(n, a.xyz) + MOON_WRAP) / (1.0 + MOON_WRAP), 0.0);
    }
    let shadowed = mix(1.0, vis, clamp(b.w, 0.0, 1.0));
    return b.rgb * (a.w * ndl * shadowed);
}
