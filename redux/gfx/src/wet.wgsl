#define_import_path redux::gfx_wet

// Rain on surfaces (LOOK-13): the wet look of an exterior surface, called from this crate's
// `gfx_surface` body. The `WET` row (`wetness.rs`) is (wetness 0..1, the direction toward the sun
// as snorm8, the sun's diffuse colour and the fog colour as unorm8). A zero wetness returns the
// colour untouched.
//
// Ported from the everwood graphics fork of Benilla (MIT OR Apache-2.0), revision `cdc7130b`,
// `benilla-assets/src/shaders/wet_hook.wgsl`: `wet_amount`, `wet_surface`, `wet_puddles`,
// `wet_sheen` and the noise. The rings on water and the body wetness are not ported.

struct WetSurface {
    albedo: vec3<f32>,
    // Gloss weight for `wet_sheen`: 0 dry, ~1 a soaked top face, up to 2 in a terrain puddle (the
    // part above 1 is standing water: a flat mirror of the sky).
    boost: f32,
}

// How wet a surface is from its facing: none below, half on walls, full on tops.
fn wet_amount(n: vec3<f32>, exposure: f32, wetness: f32) -> f32 {
    let facing = smoothstep(-0.2, 0.25, n.y);
    return clamp(wetness, 0.0, 1.0) * clamp(exposure, 0.0, 1.0) * facing
        * mix(0.5, 1.0, smoothstep(0.3, 0.8, n.y));
}

// Wet albedo: darker (water fills the micro-pores) and a touch more saturated; the gloss weight
// is kept for up-facing faces only.
fn wet_surface(albedo: vec3<f32>, n: vec3<f32>, exposure: f32, wetness: f32) -> WetSurface {
    var o: WetSurface;
    o.albedo = albedo;
    o.boost = 0.0;
    if (wetness <= 0.0 || exposure <= 0.0) {
        return o;
    }
    let w = wet_amount(n, exposure, wetness);
    let luma = dot(albedo, vec3<f32>(0.299, 0.587, 0.114));
    let sat = max(mix(vec3<f32>(luma), albedo, 1.0 + 0.3 * w), vec3<f32>(0.0));
    o.albedo = sat * (1.0 - 0.36 * w);
    o.boost = w * smoothstep(0.35, 0.9, n.y);
    return o;
}

fn wet_hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(0.1031, 0.1030));
    let r = q + dot(q, q.yx + 33.33);
    return fract((r.x + r.y) * r.x);
}

fn wet_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(wet_hash(i), wet_hash(i + vec2<f32>(1.0, 0.0)), u.x),
        mix(wet_hash(i + vec2<f32>(0.0, 1.0)), wet_hash(i + vec2<f32>(1.0, 1.0)), u.x), u.y);
}

// Terrain only: puddle patches on flat ground once it is well soaked, darker and with a stronger
// gloss weight, so they read as standing water catching the sky.
//
// Deviation from everwood: its narrow flatness band (n.y 0.94..0.99) and 0.06 noise edge cut
// straight-edged sheets along the terrain's triangles, whose interpolated vertex normals are
// piecewise linear. A domain-warped three-octave field, a wide flatness ramp and a soft edge
// give puddles organic outlines instead.
fn wet_puddles(s: WetSurface, n: vec3<f32>, world_pos: vec3<f32>, exposure: f32,
    wetness: f32) -> WetSurface {
    var o = s;
    if (wetness <= 0.0 || exposure <= 0.0) {
        return o;
    }
    let xz = world_pos.xz;
    let warp = vec2<f32>(wet_noise(xz * 0.11 + 5.0), wet_noise(xz * 0.11 + 31.0)) * 4.0 - 2.0;
    let p = xz + warp;
    let field = 0.55 * wet_noise(p * 0.17) + 0.3 * wet_noise(p * 0.53 + 17.0)
        + 0.15 * wet_noise(p * 1.7 + 41.0);
    let level = smoothstep(0.82, 0.97, n.y);
    // Puddles grow with the wetness: the threshold falls as the ground soaks.
    let edge = mix(0.78, 0.62, smoothstep(0.4, 1.0, wetness));
    let puddle = smoothstep(edge - 0.04, edge + 0.12, field) * level
        * smoothstep(0.35, 0.8, wetness) * clamp(exposure, 0.0, 1.0);
    o.albedo = o.albedo * (1.0 - 0.45 * puddle);
    o.boost = max(o.boost, 2.0 * puddle);
    return o;
}

// The wet sheen, added to the lit gamma colour: a tight sun highlight (shadowed) and a Fresnel
// reflection of the sky/fog colour. Zero when `boost` is zero.
fn wet_sheen(boost: f32, n: vec3<f32>, to_view: vec3<f32>, to_light: vec3<f32>,
    sun_rgb: vec3<f32>, sky_rgb: vec3<f32>, shadow: f32) -> vec3<f32> {
    if (boost <= 0.0) {
        return vec3<f32>(0.0);
    }
    // Nudged so opposite light and view vectors never normalize a zero vector.
    let h = normalize(to_light + to_view + vec3<f32>(0.0, 1.0e-4, 0.0));
    let nl = max(dot(n, to_light), 0.0);
    let spec = pow(max(dot(n, h), 0.0), 80.0) * smoothstep(0.0, 0.15, nl);
    let fres = 0.03 + 0.97 * pow(1.0 - clamp(dot(n, to_view), 0.0, 1.0), 5.0);
    let gloss = min(boost, 1.0);
    let mirror = max(boost - 1.0, 0.0);
    return sun_rgb * ((0.9 * gloss + 1.2 * mirror) * spec * shadow)
        // Deviation: everwood's mirror floor of 0.22 painted puddles a flat grey under an overcast
        // fog colour; 0.1 keeps them reading as water.
        + sky_rgb * (gloss * min(0.25 * fres, 0.2) + mirror * (0.1 + 0.3 * fres));
}

// The wet colour of a lit surface: `rgb` the stock colour, `albedo · light` its lit part, `direct`
// how much sunlight arrives, `exposure` how open to the sky it is (1 open, 0 under cover).
fn wet(
    terrain: bool,
    rgb: vec3<f32>,
    albedo: vec3<f32>,
    light: vec3<f32>,
    direct: f32,
    world_pos: vec3<f32>,
    normal: vec3<f32>,
    camera_pos: vec3<f32>,
    exposure: f32,
    row: vec4<f32>,
) -> vec3<f32> {
    let wetness = row.x;
    if (wetness <= 0.0 || exposure <= 0.0) {
        return rgb;
    }
    let n = normalize(normal + vec3<f32>(0.0, 1.0e-6, 0.0));
    var s = wet_surface(albedo, n, exposure, wetness);
    if (terrain) {
        s = wet_puddles(s, n, world_pos, exposure, wetness);
    }
    let to_view = normalize(camera_pos - world_pos + vec3<f32>(0.0, 1.0e-4, 0.0));
    let to_sun = normalize(unpack4x8snorm(bitcast<u32>(row.y)).xyz + vec3<f32>(0.0, 1.0e-4, 0.0));
    let sun_rgb = unpack4x8unorm(bitcast<u32>(row.z)).rgb;
    let sky_rgb = unpack4x8unorm(bitcast<u32>(row.w)).rgb;
    return rgb + (s.albedo - albedo) * light
        + wet_sheen(s.boost, n, to_view, to_sun, sun_rgb, sky_rgb, direct);
}
