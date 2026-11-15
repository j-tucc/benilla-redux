#define_import_path redux::gfx_glow

// Fog light glow: the air round the firelight's lights scatters their light toward the eye, so a
// lamp far down a foggy road shows as a soft orb in the fog itself, before and beyond its model.
// Called from this crate's hook bodies under `GFX_GLOW`: `gfx_surface` adds the light scattered
// in front of a lit surface before the fog, and `gfx_fog` adds it to the fog colour, so the
// receiver's `mix(fog, surface, survival)` passes `glow · survival + glow · (1 − survival)`, the
// whole glow, through the main pass: resolved by MSAA and jittered by TAA like any surface. The
// sky takes the whole ray from `gfx_sky`. A receiver that calls only `gfx_fog` (water, particles)
// takes the fogged share alone.
//
// Each light is single scattering from a point source in homogeneous air of extinction σ, the
// closed form along the ray (no march, no grid):
//     L = σ · I / (4π) · ∫₀ᵈ T(t) / (r(t)² + a²) dt,   r(t)² = h² + (t − t₀)²
//       ≈ σ · I / (4π) · T(t̂) · (atan((d − t₀) / H) + atan(t₀ / H)) / H,   H² = h² + a²
// with t₀ the ray's closest approach to the light, h its distance there, `a` a soft core the size
// of a flame, and the transmittance T taken at the clamped closest point (the camera to it, then
// out to the light). A window on h ends each glow at `GLOW_RADIUS`, and lights fade out toward
// the far edge of the firelight's reach. The air's density is modulated by an animated 3D value
// noise sampled at the closest point, and each light keeps the firelight's flicker: Forever's
// shimmer, a pure function of position and the clock, so a still frame is the same under TAA's
// jitter and nothing rests on frame history to hide noise.
//
// Rows: the firelight's `FIRES` and `MISC` (`firelight.rs`); `GLOW_A` = (camera xyz, extinction
// σ per yd), `GLOW_B` = (gain, the noise clock, the scene fog end, 0) (`glow.rs`).

#import benilla::gfx_data::GfxData
#import redux::gfx_rows
#import redux::gfx_fire

// How far round a light its glow reaches (yd): the window on the ray's closest approach.
const GLOW_RADIUS: f32 = 16.0;
// The soft core (yd), a flame and its bright air: the closed form's 1/h stays finite at the light,
// and a figure holding a torch is not washed out by the air round it.
const GLOW_CORE: f32 = 1.2;
// Lights fade out between these distances from the camera (yd); the far end is `glow::REACH`,
// the firelight's reach while the glow is on.
const FADE_FROM: f32 = 180.0;
const FADE_TO: f32 = 250.0;
// The noise: its cell (yd), how far it swings the density (0..1), and its drift (cells/s, mostly
// up, the warm air over a flame).
const NOISE_CELL: f32 = 1.8;
const NOISE_DEPTH: f32 = 0.5;
const NOISE_DRIFT: vec3<f32> = vec3<f32>(0.12, 0.35, 0.07);

// An integer hash of a lattice point: exact at any distance from the origin.
fn glow_hash(p: vec3<i32>) -> f32 {
    let c = bitcast<vec3<u32>>(p);
    var h = (c.x * 73856093u) ^ (c.y * 19349663u) ^ (c.z * 83492791u);
    h = (h ^ (h >> 16u)) * 2246822519u;
    h = h ^ (h >> 13u);
    return f32(h & 0xffffu) / 65535.0;
}

// Value noise 0..1 at `p` (in cells), smooth between the lattice's eight corners.
fn glow_noise(p: vec3<f32>) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = mix(glow_hash(i), glow_hash(i + vec3<i32>(1, 0, 0)), u.x);
    let b = mix(glow_hash(i + vec3<i32>(0, 1, 0)), glow_hash(i + vec3<i32>(1, 1, 0)), u.x);
    let c = mix(glow_hash(i + vec3<i32>(0, 0, 1)), glow_hash(i + vec3<i32>(1, 0, 1)), u.x);
    let d = mix(glow_hash(i + vec3<i32>(0, 1, 1)), glow_hash(i + vec3<i32>(1, 1, 1)), u.x);
    return mix(mix(a, b, u.y), mix(c, d, u.y), u.z);
}

// One light's glow along unit `ray` from `eye` over `d` yd, in air of extinction `sigma`, at the
// noise clock `noise_t` and the flicker clock `clock`.
fn glow_one(
    row: vec4<f32>,
    eye: vec3<f32>,
    ray: vec3<f32>,
    d: f32,
    sigma: f32,
    noise_t: f32,
    clock: f32,
) -> vec3<f32> {
    // Camera-relative from two absolute positions: exact to the f32 step.
    let c = row.xyz - eye;
    let t0 = dot(c, ray);
    let h2 = max(dot(c, c) - t0 * t0, 0.0);
    // The ray passes outside the glow, or the light stands behind the camera or the surface.
    if (h2 >= GLOW_RADIUS * GLOW_RADIUS || t0 < -GLOW_RADIUS || t0 > d + GLOW_RADIUS) {
        return vec3<f32>(0.0);
    }
    let fade = 1.0 - smoothstep(FADE_FROM, FADE_TO, length(c));
    let big_h = sqrt(h2 + GLOW_CORE * GLOW_CORE);
    let seen = (atan((d - t0) / big_h) + atan(t0 / big_h)) / big_h;
    let edge = h2 / (GLOW_RADIUS * GLOW_RADIUS);
    let window = (1.0 - edge) * (1.0 - edge);
    // The light reaching the eye from the closest point: the camera to it, then out to the light.
    let tc = clamp(t0, 0.0, d);
    let trans = exp(-sigma * (tc + sqrt(h2)));
    // The air's density at the closest point, drifting.
    let n = glow_noise((eye + ray * tc) / NOISE_CELL - NOISE_DRIFT * noise_t);
    let density = sigma * (1.0 + NOISE_DEPTH * (2.0 * n - 1.0));
    let rgb = gfx_fire::fire_rgb(row.w);
    // A carried light's flicker is already in its colour (`firelight.rs`).
    let steady = (bitcast<u32>(row.w) & 0x80000000u) != 0u;
    let flicker = select(gfx_fire::fire_flicker_of(clock, row.xyz, row.w, rgb), 1.0, steady);
    return rgb * (density * seen * window * trans * fade * flicker);
}

// The light the fog scatters toward the eye along unit `ray` from `eye` over `d` yd (the sky
// passes a huge `d`), from the firelight's lights, read at constant rows as `fire.wgsl` does.
fn glow_light(eye: vec3<f32>, ray: vec3<f32>, d: f32, gfx: GfxData) -> vec3<f32> {
    let count = u32(gfx.rows[gfx_rows::MISC].y);
    let glow_a = gfx.rows[gfx_rows::GLOW_A];
    let glow_b = gfx.rows[gfx_rows::GLOW_B];
    if (count == 0u || glow_b.x <= 0.0) {
        return vec3<f32>(0.0);
    }
    let s = glow_a.w;
    let nt = glow_b.y;
    let k = gfx.rows[gfx_rows::MISC].w;
    var sum = vec3<f32>(0.0);
    if (count > 0u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 0u], eye, ray, d, s, nt, k); }
    if (count > 1u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 1u], eye, ray, d, s, nt, k); }
    if (count > 2u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 2u], eye, ray, d, s, nt, k); }
    if (count > 3u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 3u], eye, ray, d, s, nt, k); }
    if (count > 4u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 4u], eye, ray, d, s, nt, k); }
    if (count > 5u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 5u], eye, ray, d, s, nt, k); }
    if (count > 6u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 6u], eye, ray, d, s, nt, k); }
    if (count > 7u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 7u], eye, ray, d, s, nt, k); }
    if (count > 8u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 8u], eye, ray, d, s, nt, k); }
    if (count > 9u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 9u], eye, ray, d, s, nt, k); }
    if (count > 10u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 10u], eye, ray, d, s, nt, k); }
    if (count > 11u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 11u], eye, ray, d, s, nt, k); }
    if (count > 12u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 12u], eye, ray, d, s, nt, k); }
    if (count > 13u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 13u], eye, ray, d, s, nt, k); }
    if (count > 14u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 14u], eye, ray, d, s, nt, k); }
    if (count > 15u) { sum += glow_one(gfx.rows[gfx_rows::FIRES + 15u], eye, ray, d, s, nt, k); }
    // σ·I/(4π): the 1/(4π) and the firelight's gain fold into the row's gain.
    return sum * glow_b.x;
}

// The glow in front of a surface at `world_pos`, seen from `camera_pos`.
fn glow_at(world_pos: vec3<f32>, camera_pos: vec3<f32>, gfx: GfxData) -> vec3<f32> {
    let delta = world_pos - camera_pos;
    let d = length(delta);
    if (d <= 1e-3) {
        return vec3<f32>(0.0);
    }
    return glow_light(camera_pos, delta / d, d, gfx);
}

// The glow `gfx_fog` adds to the fog colour for the fog pair `span`: the scene fog's, or the WDL
// hull's own (0, 1) pair. An interior WMO pair takes none: the lights are outdoors.
fn glow_fog(span: vec2<f32>, world_pos: vec3<f32>, camera_pos: vec3<f32>, gfx: GfxData) -> vec3<f32> {
    let hull = span.x == 0.0 && span.y == 1.0;
    if (!hull && abs(span.y - gfx.rows[gfx_rows::GLOW_B].z) > 0.01) {
        return vec3<f32>(0.0);
    }
    return glow_at(world_pos, camera_pos, gfx);
}

// The glow along the sky's unit `dir`, from the camera the row holds.
fn glow_sky(dir: vec3<f32>, gfx: GfxData) -> vec3<f32> {
    return glow_light(gfx.rows[gfx_rows::GLOW_A].xyz, normalize(dir), 1.0e6, gfx);
}
