// Light shafts (LOOK-14): the shadow rig's map ray-marched through the air in front of every
// pixel, three stages chosen by shader def (see `shafts.rs`):
//   SHAFTS_MARCH      half resolution: jittered samples along the view ray ask the map whether
//                     that air is lit; lit air in-scatters toward the eye.
//   SHAFTS_BLUR       half resolution: a depth-aware 4×4 gather that removes the jitter.
//   SHAFTS_COMPOSITE  full resolution: a depth-aware 2×2 upsample, coloured, lifted over the scene.
//
// Ported from the everwood graphics fork of Benilla (MIT OR Apache-2.0), revision `cdc7130b`,
// `benilla-app/src/volumetric_light.wgsl`: the quadratic step spacing, the interleaved-gradient-
// noise start (Jimenez 2014), the phase, the height falloff, the bilateral keys and the soft screen
// lift. The cascade lookup reads this pass's own uniform rather than Bevy's light array.

#import bevy_render::view::View

struct Shafts {
    // xyz = unit direction toward the light body, w = march steps.
    dir_steps: vec4<f32>,
    // rgb = in-scatter colour (gain and fades folded in), w = extinction per yard.
    colour: vec4<f32>,
    // x = march length cap (yd), y = march start (yd), z = anisotropy g, w = height falloff (yd).
    range: vec4<f32>,
    // x = depth bias along the light (yd), y = scattering per yard (at least the extinction).
    extra: vec4<f32>,
    // The cascade's clip-from-world, column by column.
    clip_from_world: mat4x4<f32>,
}

// Distance (yd) along the view ray to the scene at full-resolution pixel `p` with reverse-Z depth
// `z`; the sky (depth 0) is `cap` away. The march's and the composite's bilateral key.
fn ray_distance(view: View, p: vec2<f32>, z: f32, cap: f32) -> f32 {
    if (z <= 0.0) {
        return cap;
    }
    let uv = (p - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let c = view.view_from_clip * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), z, 1.0);
    return min(length(c.xyz / c.w), cap);
}

#ifdef SHAFTS_MARCH
#ifdef MULTISAMPLED
@group(0) @binding(0) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(0) var depth: texture_depth_2d;
#endif
@group(0) @binding(1) var<uniform> view: View;
@group(0) @binding(2) var shadow_map: texture_depth_2d_array;
@group(0) @binding(3) var shadow_cmp: sampler_comparison;
@group(0) @binding(4) var<uniform> shafts: Shafts;

// 1 lit, 0 shadowed, -1 outside the cascade (unknown).
fn visibility(p: vec3<f32>) -> f32 {
    let q = shafts.clip_from_world * vec4<f32>(p + shafts.dir_steps.xyz * shafts.extra.x, 1.0);
    let ndc = q.xyz / q.w;
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z < 0.0 || ndc.z > 1.0) {
        return -1.0;
    }
    return textureSampleCompareLevel(shadow_map, shadow_cmp, uv, 0, ndc.z);
}

// Interleaved gradient noise (Jimenez 2014): its 4×4 neighbourhoods cover the unit interval
// evenly, so the blur removes it.
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// Henyey-Greenstein, a quarter isotropic so side-on shafts stay faintly visible.
fn phase(cosine: f32) -> f32 {
    let g = shafts.range.z;
    let denom = 1.0 + g * g - 2.0 * g * cosine;
    let hg = 0.07957747 * (1.0 - g * g) / (denom * sqrt(denom));
    return mix(hg, 0.07957747, 0.25);
}

@fragment
fn march(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = min(vec2<i32>(textureDimensions(depth)), vec2<i32>(view.main_pass_viewport.zw));
    let hp = vec2<i32>(frag.xy);
    let full = min(hp * 2, dims - vec2<i32>(1));
    let z = textureLoad(depth, full, 0);
    let px = vec2<f32>(full) + 0.5;
    let cap = shafts.range.x;
    let dist = ray_distance(view, px, z, cap);
    // The ray, camera-relative and in world axes: free of the absolute position's f32 step.
    let uv = (px - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let c = view.view_from_clip
        * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), max(z, 1e-6), 1.0);
    let ray = normalize(mat3x3<f32>(
        view.world_from_view[0].xyz,
        view.world_from_view[1].xyz,
        view.world_from_view[2].xyz,
    ) * (c.xyz / c.w));
    let start = shafts.range.y;
    let span = dist - start;
    if (span <= 0.0) {
        return vec4<f32>(0.0, dist, 0.0, 1.0);
    }
    let count = u32(shafts.dir_steps.w);
    let n = f32(count);
    let sigma = shafts.colour.w;
    let jitter = ign(vec2<f32>(hp));
    var scatter = 0.0;
    var known = 0.0;
    var known_count = 0.0;
    for (var step = 0u; step < 64u; step += 1u) {
        if (step >= count) {
            break;
        }
        // Quadratic spacing: dense near the eye, where the shafts are.
        let u = (f32(step) + jitter) / n;
        let t = start + span * u * u;
        let dt = span * 2.0 * u / n;
        var seen = visibility(view.world_position + ray * t);
        if (seen >= 0.0) {
            known += seen;
            known_count += 1.0;
        } else {
            // Past the cascade: the mean of what this ray could see, never a hard ring.
            seen = select(0.5, known / known_count, known_count > 0.0);
        }
        // Thinner air above the eye (down to 2 yd under it, about the ground).
        let above = max(ray.y * t + 2.0, 0.0);
        let local = shafts.extra.y * exp(-above / shafts.range.w);
        scatter += seen * exp(-sigma * t) * local * dt;
    }
    scatter *= phase(dot(ray, shafts.dir_steps.xyz));
    return vec4<f32>(scatter, dist, 0.0, 1.0);
}
#endif

#ifdef SHAFTS_BLUR
@group(0) @binding(0) var marched: texture_2d<f32>;

@fragment
fn blur(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(marched));
    let p = vec2<i32>(frag.xy);
    let key = textureLoad(marched, p, 0).y;
    let tolerance = 0.08 * key + 0.75;
    var sum = 0.0;
    var weight = 0.0;
    for (var dy = -2; dy <= 1; dy += 1) {
        for (var dx = -2; dx <= 1; dx += 1) {
            let q = clamp(p + vec2<i32>(dx, dy), vec2<i32>(0), dims - vec2<i32>(1));
            let s = textureLoad(marched, q, 0);
            let w = exp(-abs(s.y - key) / tolerance);
            sum += s.x * w;
            weight += w;
        }
    }
    return vec4<f32>(sum / max(weight, 1e-5), key, 0.0, 1.0);
}
#endif

#ifdef SHAFTS_COMPOSITE
@group(0) @binding(0) var scene: texture_2d<f32>;
#ifdef MULTISAMPLED
@group(0) @binding(1) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(1) var depth: texture_depth_2d;
#endif
@group(0) @binding(2) var blurred: texture_2d<f32>;
@group(0) @binding(3) var<uniform> view: View;
@group(0) @binding(4) var<uniform> shafts: Shafts;

@fragment
fn composite(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(frag.xy);
    let source = textureLoad(scene, p, 0);
    let dims = min(vec2<i32>(textureDimensions(depth)), vec2<i32>(view.main_pass_viewport.zw));
    let z = textureLoad(depth, min(p, dims - vec2<i32>(1)), 0);
    let key = ray_distance(view, frag.xy, z, shafts.range.x);
    // The bilinear footprint in the half-resolution grid, each tap weighted by depth agreement.
    let half_dims = vec2<i32>(textureDimensions(blurred));
    let h = frag.xy * 0.5 - 0.5;
    let base = vec2<i32>(floor(h));
    let f = h - floor(h);
    let tolerance = 0.08 * key + 0.75;
    var sum = 0.0;
    var weight = 0.0;
    for (var j = 0; j <= 1; j += 1) {
        for (var i = 0; i <= 1; i += 1) {
            let q = clamp(base + vec2<i32>(i, j), vec2<i32>(0), half_dims - vec2<i32>(1));
            let s = textureLoad(blurred, q, 0);
            let bilinear = select(1.0 - f.x, f.x, i == 1) * select(1.0 - f.y, f.y, j == 1);
            let w = bilinear * exp(-abs(s.y - key) / tolerance) + 1e-4;
            sum += s.x * w;
            weight += w;
        }
    }
    let light = shafts.colour.rgb * max(sum / weight, 0.0);
    // The haze's soft screen curve: bright pixels saturate gently instead of clipping.
    let lifted = source.rgb
        + max(vec3<f32>(0.0), vec3<f32>(1.0) - source.rgb) * (vec3<f32>(1.0) - exp(-light));
    return vec4<f32>(lifted, source.a);
}
#endif
