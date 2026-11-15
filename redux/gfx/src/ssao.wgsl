// Ambient occlusion (LOOK-14): depth-only screen-space occlusion of the opaque scene, three
// stages chosen by shader def (see `ssao.rs`):
//   AO_OCCLUDE  half resolution: occlusion from a rotated spiral of depth taps (an Alchemy-style
//               cosine term with a range falloff), faded with distance and kept off bright pixels.
//   AO_BLUR     half resolution: a 4×4 depth-aware box; the 4×4 Bayer rotation cancels inside it.
//   AO_APPLY    full resolution: a joint-bilateral upsample, multiplied into the scene. The
//               opaque scene is already fogged in-shader, so only the share of each pixel that
//               survives the fog takes occlusion: a fogged wall keeps the flat fog colour.
// The half-resolution texel: r = occlusion, g = view distance (SKY_DIST on the sky), b = bright-
// pixel protection, a = surface smoothness after AO_OCCLUDE, 1 − foliage after AO_BLUR.
//
// Ported from the everwood graphics fork of Benilla (MIT OR Apache-2.0), revision `cdc7130b`,
// `benilla-app/src/shaders/ssao.wgsl`, without its debug views: the taps, the normal from the
// flatter neighbour, the foliage spike mask and its widened blur, the protection, the upsample.
#import bevy_render::view::View
#ifdef AO_APPLY
#import redux::gfx_fog
#import redux::gfx_rows
#endif

struct Ao {
    // Radius (yd), darkening strength, sample count, cosine bias.
    params: vec4<f32>,
    // Distance fade start and end (yd), protection ramp start and end (max channel).
    fade: vec4<f32>,
    // y = occlusion gain.
    gain: vec4<f32>,
}

const SKY_DIST: f32 = 60000.0;

#ifdef AO_BLUR
@group(0) @binding(0) var raw: texture_2d<f32>;
#else
#ifdef MULTISAMPLED
@group(0) @binding(0) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(0) var depth: texture_depth_2d;
#endif
// AO_OCCLUDE: the resolved opaque scene. AO_APPLY: the blurred half-resolution occlusion.
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var<uniform> view: View;
#endif
@group(0) @binding(3) var<uniform> ao: Ao;
#ifdef AO_APPLY
// The engine's shared light buffer, read as rows: 4 fog colour (w = on), 5 the scene fog span,
// 18 the interior fog colour (w = on), 19 its span, and `GfxData` from row `GFX_BASE`.
struct LightRows {
    rows: array<vec4<f32>>,
}
@group(0) @binding(4) var<storage, read> light: LightRows;
#endif

#ifndef AO_BLUR
// The main pass's rectangle in the full-size targets: smaller under an upscaler.
fn main_size() -> vec2<i32> {
    return vec2<i32>(view.main_pass_viewport.zw);
}

// Sample zero of a multisampled depth, clamped into the main pass's rectangle.
fn depth_at(p: vec2<i32>) -> f32 {
    let size = min(vec2<i32>(textureDimensions(depth)), main_size());
    return textureLoad(depth, clamp(p, vec2<i32>(0), size - vec2<i32>(1)), 0);
}

fn view_at(p: vec2<i32>, z: f32) -> vec3<f32> {
    let uv = (vec2<f32>(p) + 0.5 - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let c = view.view_from_clip * vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), z, 1.0);
    return c.xyz / c.w;
}

// Reverse-Z with an infinite far plane: the cleared 0 is the sky.
fn is_sky(z: f32) -> bool {
    return z <= 1e-7;
}
#endif

#ifdef AO_OCCLUDE
fn bayer4(p: vec2<i32>) -> f32 {
    var m = array<u32, 16>(0u, 8u, 2u, 10u, 12u, 4u, 14u, 6u, 3u, 11u, 1u, 9u, 15u, 7u, 13u, 5u);
    let x = u32(p.x) & 3u;
    let y = u32(p.y) & 3u;
    return f32(m[y * 4u + x]);
}

// A neighbour for the normal; the sky counts as infinitely far, so it is never chosen.
fn neighbour(p: vec2<i32>, centre: vec3<f32>) -> vec3<f32> {
    let z = depth_at(p);
    if (is_sky(z)) {
        return centre + vec3(0.0, 0.0, -1e4);
    }
    return view_at(p, z);
}

@fragment
fn occlude(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let h = vec2<i32>(frag.xy);
    let f = h * 2;
    let size = min(vec2<i32>(textureDimensions(source)), main_size());
    // Protection from the brightest pixel of the 2×2 footprint: lit windows, flames, sunlit sand.
    var bright = 0.0;
    for (var j = 0; j < 4; j += 1) {
        let q = clamp(f + vec2(j & 1, j >> 1u), vec2<i32>(0), size - vec2<i32>(1));
        let c = textureLoad(source, q, 0).rgb;
        bright = max(bright, max(c.r, max(c.g, c.b)));
    }
    let protect = smoothstep(ao.fade.z, ao.fade.w, bright);
    let z = depth_at(f);
    if (is_sky(z)) {
        return vec4(0.0, SKY_DIST, protect, 1.0);
    }
    let p = view_at(f, z);
    let dist = -p.z;
    let fade = 1.0 - smoothstep(ao.fade.x, ao.fade.y, dist);
    let radius = ao.params.x;
    let proj = 0.5 * view.main_pass_viewport.w * view.clip_from_view[1][1];
    let r_px = min(radius * proj / max(dist, 0.01), 0.12 * view.main_pass_viewport.w);
    if (fade <= 0.0 || r_px < 2.0) {
        return vec4(0.0, dist, protect, 1.0);
    }
    // The normal from the flatter neighbour on each axis, so silhouettes do not bend it.
    let pr = neighbour(f + vec2(1, 0), p);
    let pl = neighbour(f - vec2(1, 0), p);
    let pd = neighbour(f + vec2(0, 1), p);
    let pu = neighbour(f - vec2(0, 1), p);
    let dx = select(p - pl, pr - p, abs(pr.z - p.z) < abs(p.z - pl.z));
    let dy = select(p - pu, pd - p, abs(pd.z - p.z) < abs(p.z - pu.z));
    // Sky on both sides of an axis (a one-pixel pole against the sky) leaves no normal.
    let c = cross(dx, dy);
    let l2 = dot(c, c);
    if (l2 < 1e-12) {
        return vec4(0.0, dist, protect, 1.0);
    }
    var n = c * inverseSqrt(l2);
    if (dot(n, p) > 0.0) {
        n = -n;
    }
    // Cutout leaves scatter depth spikes through a canopy, where the taps are noise. The spike is
    // the smaller one-sided step two pixels out, capped by the second difference (about 0 on any
    // plane and on a silhouette); the blur turns their density into the foliage mask.
    let ax = neighbour(f + vec2(2, 0), p).z - p.z;
    let bx = p.z - neighbour(f - vec2(2, 0), p).z;
    let ay = neighbour(f + vec2(0, 2), p).z - p.z;
    let by = p.z - neighbour(f - vec2(0, 2), p).z;
    let spike = max(
        min(min(abs(ax), abs(bx)), abs(ax - bx)),
        min(min(abs(ay), abs(by)), abs(ay - by)),
    );
    let smooth_surface = 1.0 - smoothstep(0.01, 0.04, spike / dist);

    let count = u32(ao.params.z);
    let rot = (bayer4(h) + 0.5) * (6.2831853 / 16.0);
    let jitter = (bayer4(h + vec2(2, 1)) + 0.5) / 16.0;
    let r2 = radius * radius;
    var occ = 0.0;
    for (var i = 0u; i < count; i += 1u) {
        let t = (f32(i) + jitter) / f32(count);
        let a = f32(i) * 2.3999632 + rot;
        // A linear radius: taps crowd the centre, where contact shadows live.
        let off = vec2(cos(a), sin(a)) * mix(2.0, r_px, t);
        let q = f + vec2<i32>(round(off));
        let zq = depth_at(q);
        if (is_sky(zq)) {
            continue;
        }
        let v = view_at(q, zq) - p;
        let vv = dot(v, v);
        let w = saturate(1.0 - vv / r2);
        occ += w * max(0.0, dot(v, n) * inverseSqrt(vv + 1e-6) - ao.params.w);
    }
    occ = saturate(ao.gain.y * occ / f32(count)) * fade;
    return vec4(occ, dist, protect, smooth_surface);
}
#endif

#ifdef AO_BLUR
@fragment
fn blur(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let h = vec2<i32>(frag.xy);
    let size = vec2<i32>(textureDimensions(raw));
    let c = textureLoad(raw, h, 0);
    if (c.g >= SKY_DIST) {
        return c;
    }
    // Foliage = spike density over the window. There the tolerance widens and the leftover
    // occlusion is scaled down; a window of smooth surfaces keeps the tight edge and full AO.
    var spikes = 0.0;
    for (var y = -2; y <= 1; y += 1) {
        for (var x = -2; x <= 1; x += 1) {
            let q = clamp(h + vec2(x, y), vec2<i32>(0), size - vec2<i32>(1));
            spikes += 1.0 - textureLoad(raw, q, 0).a;
        }
    }
    let foliage = saturate(spikes / 16.0 * 4.0);
    let tolerance = (0.04 * c.g + 0.05) * (1.0 + 10.0 * foliage);
    var sum = 0.0;
    var weight = 0.0;
    for (var y = -2; y <= 1; y += 1) {
        for (var x = -2; x <= 1; x += 1) {
            let s = textureLoad(raw, clamp(h + vec2(x, y), vec2<i32>(0), size - vec2<i32>(1)), 0);
            let w = saturate(1.0 - abs(s.g - c.g) / tolerance);
            sum += s.r * w;
            weight += w;
        }
    }
    return vec4(sum / max(weight, 1e-4) * (1.0 - 0.4 * foliage), c.g, c.b, 1.0 - foliage);
}
#endif

#ifdef AO_APPLY
// The survival (1 = no fog) a receiver gives span `span` at view position `p`: the Modern law for
// a span ending at the scene fog end while Modern is on, else the 1.12 linear ramp (`fog_sample`).
fn survival(span: vec2<f32>, p: vec3<f32>, world_y: f32) -> f32 {
    let base = u32(#{GFX_BASE});
    let sel = light.rows[base + gfx_rows::FOG_D];
    if (gfx_fog::fog_is_modern(sel) && abs(span.y - sel.x) <= 0.01) {
        return gfx_fog::fog_modern_survival(span, length(p), world_y, light.rows[base + gfx_rows::FOG_A]);
    }
    return gfx_fog::fog_linear(-p.z, span);
}

// How much of the surface at `p` the fog leaves. The pixel does not say whether it took the scene
// or the interior pair (they differ only with the camera in a WMO), so the denser one wins: a
// room's fogged far wall keeps no occlusion, and outdoors the two pairs are the same.
fn fog_left(p: vec3<f32>) -> f32 {
    let world_y = (view.world_from_view * vec4(p, 1.0)).y;
    var s = 1.0;
    if (light.rows[4].w > 0.5) {
        s = survival(light.rows[5].xy, p, world_y);
    }
    if (light.rows[18].w > 0.5) {
        s = min(s, survival(light.rows[19].xy, p, world_y));
    }
    return s;
}

@fragment
fn apply(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(frag.xy);
    let z = depth_at(p);
    if (is_sky(z)) {
        return vec4(1.0);
    }
    let pv = view_at(p, z);
    let dist = -pv.z;
    let size = vec2<i32>(textureDimensions(source));
    let hc = frag.xy * 0.5 - 0.5;
    let base = vec2<i32>(floor(hc));
    let t = hc - floor(hc);
    var occ = 0.0;
    var protect = 0.0;
    var weight = 0.0;
    for (var j = 0; j < 4; j += 1) {
        let o = vec2(j & 1, j >> 1u);
        let s = textureLoad(source, clamp(base + o, vec2<i32>(0), size - vec2<i32>(1)), 0);
        let b = mix(1.0 - t, t, vec2<f32>(o));
        let w = b.x * b.y / (0.02 + abs(s.g - dist) / dist) + 1e-5;
        occ += s.r * w;
        protect += s.b * w;
        weight += w;
    }
    occ /= weight;
    protect /= weight;
    // Fogged = mix(fog, surface, s): occlusion darkens only the surface's share.
    let k = 1.0 - ao.params.y * occ * (1.0 - protect) * fog_left(pv);
    return vec4(k, k, k, 1.0);
}
#endif
