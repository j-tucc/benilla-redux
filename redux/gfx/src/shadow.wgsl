#define_import_path redux::gfx_shadow

// The sun-shadow receiver (LOOK-14), called by this crate's `gfx_lit` and `gfx_baked_aside` hook
// bodies. The map is Bevy's directional shadow array, one cascade, which the engine's receivers
// pass in with its comparison sampler; the cascade matrix and the rig's weights are `GfxData`
// rows (`rows.rs`, `shadows.rs`).
//
// Ported from the everwood graphics fork of Benilla (MIT OR Apache-2.0), revision `cdc7130b`,
// `benilla-assets/src/shaders/shadow_hook.wgsl` (the edge band and the fades) and the receivers'
// apply (the sun floor on the direct light; a skinned unit sampled once above its origin). The
// filter is Bevy 0.18's `sample_shadow_map_castano_thirteen` (`bevy_pbr::shadow_sampling`, MIT OR
// Apache-2.0), with the texture passed in rather than read from Bevy's view bindings.

#import benilla::gfx_data::GfxData
#import redux::gfx_rows

// The shadow lightens over the last yards of its distance, so it fades in rather than popping.
const SHADOW_EDGE_BAND: f32 = 14.0;
// A unit samples once, this far (yd) toward its light from its origin, clear of its own body.
const UNIT_LIFT: f32 = 2.5;

// One hardware 2×2 comparison tap.
fn tap(map: texture_depth_2d_array, cmp: sampler_comparison, uv: vec2<f32>, depth: f32) -> f32 {
    return textureSampleCompareLevel(map, cmp, uv, 0, depth);
}

// Castaño's 9-tap Gaussian over the hardware taps (Bevy's `sample_shadow_map_castano_thirteen`).
fn castano(
    map: texture_depth_2d_array,
    cmp: sampler_comparison,
    light_local: vec2<f32>,
    depth: f32,
) -> f32 {
    let size = vec2<f32>(textureDimensions(map));
    let inv = 1.0 / size;
    let uv = light_local * size;
    var base_uv = floor(uv + 0.5);
    let s = (uv.x + 0.5 - base_uv.x);
    let t = (uv.y + 0.5 - base_uv.y);
    base_uv -= 0.5;
    base_uv *= inv;

    let uw0 = (4.0 - 3.0 * s);
    let uw1 = 7.0;
    let uw2 = (1.0 + 3.0 * s);
    let u0 = (3.0 - 2.0 * s) / uw0 - 2.0;
    let u1 = (3.0 + s) / uw1;
    let u2 = s / uw2 + 2.0;

    let vw0 = (4.0 - 3.0 * t);
    let vw1 = 7.0;
    let vw2 = (1.0 + 3.0 * t);
    let v0 = (3.0 - 2.0 * t) / vw0 - 2.0;
    let v1 = (3.0 + t) / vw1;
    let v2 = t / vw2 + 2.0;

    var sum = 0.0;
    sum += uw0 * vw0 * tap(map, cmp, base_uv + vec2(u0, v0) * inv, depth);
    sum += uw1 * vw0 * tap(map, cmp, base_uv + vec2(u1, v0) * inv, depth);
    sum += uw2 * vw0 * tap(map, cmp, base_uv + vec2(u2, v0) * inv, depth);
    sum += uw0 * vw1 * tap(map, cmp, base_uv + vec2(u0, v1) * inv, depth);
    sum += uw1 * vw1 * tap(map, cmp, base_uv + vec2(u1, v1) * inv, depth);
    sum += uw2 * vw1 * tap(map, cmp, base_uv + vec2(u2, v1) * inv, depth);
    sum += uw0 * vw2 * tap(map, cmp, base_uv + vec2(u0, v2) * inv, depth);
    sum += uw1 * vw2 * tap(map, cmp, base_uv + vec2(u1, v2) * inv, depth);
    sum += uw2 * vw2 * tap(map, cmp, base_uv + vec2(u2, v2) * inv, depth);
    return sum * (1.0 / 144.0);
}

// Dappled sun (LOOK-16). A tree's canopy is many overlapping leaf cards: alpha-tested in the
// map, so its rim is ragged, but closed overhead, so the ground under it is one flat shade. Where
// the map shows a canopy over a receiver (blockers well above it, at broken depths, as leaves are
// and roofs and walls are not), sun patches fall through in a two-octave noise laid on a level
// layer of leaves above the receiver, read where the ray toward the light crosses it, so each
// patch sits where the light came through the leaves.
//
// The layer is level and fixed in the world, so the patches hold still while the light re-aims:
// a re-aim only slides them by the layer's height times its angle (about 5 mm for the rig's
// 8e-4 rad snap). Noise laid on the plane square to the light instead turned with it about the
// world origin, ~9000 yd off, and every re-aim (several a second while the sun climbs) dealt the
// patches out afresh: the ground flickered.
//
// How far (texels) the canopy probe looks round the receiver's texel.
const DAPPLE_PROBE: i32 = 6;
// The blockers' depth spread (yd) that reads as leaves: none at the low end, all at the high.
const DAPPLE_SPREAD: vec2<f32> = vec2<f32>(0.3, 1.2);
// How far above the receiver (yd) the nearest blocker must be: a trunk or a hedge at the
// receiver's side casts its own whole shadow.
const DAPPLE_HEIGHT: vec2<f32> = vec2<f32>(1.5, 4.0);
// The patches' size (cycles per yard of the noise) and how much of the canopy they open.
// Deviation: no source value; tuned in stills against the blue-forest reference.
const DAPPLE_FREQ: f32 = 1.1;
const DAPPLE_OPEN: vec2<f32> = vec2<f32>(0.5, 0.58);
const DAPPLE_LIGHT: f32 = 0.9;
// The leaf layer's height above the receiver (yd), along the light. Deviation: no source value;
// about a Northshire oak's lowest leaves.
const DAPPLE_LAYER: f32 = 6.0;
// The patches sway with the crowns: the tree wind's 4 s crown wave (114 cycles per travel wrap,
// `wind.wgsl`) and its 34 yd wavelength, by up to this far (yd), gust-scaled. Zero wind rows hold
// them still. Deviation: no source value; a little under the crowns' own sway.
const DAPPLE_SWAY: f32 = 0.12;
const DAPPLE_SWAY_CYCLES: f32 = 114.0;

// An integer hash of a cell, exact at any distance from the origin.
fn dapple_hash(cell: vec2<i32>) -> f32 {
    let c = bitcast<vec2<u32>>(cell);
    var h = (c.x * 73856093u) ^ (c.y * 19349663u);
    h = (h ^ (h >> 16u)) * 2246822519u;
    h = h ^ (h >> 13u);
    return f32(h & 0xffffu) / 65535.0;
}

// Smooth value noise on the plane.
fn dapple_noise(p: vec2<f32>) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = dapple_hash(i);
    let b = dapple_hash(i + vec2<i32>(1, 0));
    let c = dapple_hash(i + vec2<i32>(0, 1));
    let d = dapple_hash(i + vec2<i32>(1, 1));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// How much of a canopy the map shows over the receiver from one texel: the probe of five raw
// depths round it, 0 when fewer than two of them block.
fn canopy_at(
    map: texture_depth_2d_array,
    centre: vec2<i32>,
    size: vec2<i32>,
    receiver: f32,
    per_yd: f32,
) -> f32 {
    var near = 1.0e9;
    var far = -1.0e9;
    var blockers = 0;
    for (var k = 0; k < 5; k += 1) {
        var off = vec2<i32>(0);
        if (k > 0) {
            off = vec2<i32>(select(-1, 1, (k & 1) == 0), select(-1, 1, k > 2)) * DAPPLE_PROBE;
        }
        let d = textureLoad(map, clamp(centre + off, vec2<i32>(0), size - 1), 0, 0);
        // Reverse depth: a blocker stands nearer the light, deeper in the map.
        if (d > receiver) {
            near = min(near, d);
            far = max(far, d);
            blockers += 1;
        }
    }
    if (blockers < 2) {
        return 0.0;
    }
    let height = (near - receiver) / per_yd;
    let spread = (far - near) / per_yd;
    return smoothstep(DAPPLE_SPREAD.x, DAPPLE_SPREAD.y, spread)
        * smoothstep(DAPPLE_HEIGHT.x, DAPPLE_HEIGHT.y, height);
}

// How much sun a canopy lets through at `world_pos` (0 = none, or no canopy overhead), given the
// receiver's texel and depth in the map and the direction toward the light.
fn dapple(
    map: texture_depth_2d_array,
    light_local: vec2<f32>,
    receiver: f32,
    world_pos: vec3<f32>,
    to_light: vec3<f32>,
    clip_from_world: mat4x4<f32>,
    gfx: GfxData,
) -> f32 {
    let size = vec2<i32>(textureDimensions(map));
    // The map's depth per yard along the light.
    let per_yd = max(
        length(vec3<f32>(clip_from_world[0].z, clip_from_world[1].z, clip_from_world[2].z)), 1e-6,
    );
    // The canopy blended between the four nearest texel centres: one texel's probe alone is a
    // step at every texel edge, a staircase of hard blocks on the ground (0.08 yd a texel at
    // noon, stretched toward a low sun) that also jumped whenever the light re-aimed.
    let at = light_local * vec2<f32>(size) - 0.5;
    let i = vec2<i32>(floor(at));
    let f = at - floor(at);
    let c00 = canopy_at(map, i, size, receiver, per_yd);
    let c10 = canopy_at(map, i + vec2<i32>(1, 0), size, receiver, per_yd);
    let c01 = canopy_at(map, i + vec2<i32>(0, 1), size, receiver, per_yd);
    let c11 = canopy_at(map, i + vec2<i32>(1, 1), size, receiver, per_yd);
    let canopy = mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
    if (canopy <= 0.0) {
        return 0.0;
    }
    // Where the ray toward the light crosses the leaf layer. Absolute positions are ~9000 yd out:
    // the f32 step (~1 mm) is far under a patch, and nothing here turns about the origin.
    let l = normalize(to_light);
    let leaves = world_pos.xz + l.xz * (DAPPLE_LAYER / max(l.y, 0.2));
    // The crowns' sway, from the wind rows (`rows.rs`): its phase is the integrated travel.
    let wind_a = gfx.rows[gfx_rows::WIND_A];
    let wind_b = gfx.rows[gfx_rows::WIND_B];
    let base = vec2<f32>(cos(wind_a.z), sin(wind_a.z));
    let wave = sin(dot(base, leaves) * (6.28318530718 / 34.0)
        - fract(DAPPLE_SWAY_CYCLES * wind_b.x / 4096.0) * 6.28318530718);
    let sway = wind_a.xy * (DAPPLE_SWAY * (0.75 + 0.5 * wind_a.w) * wave * wind_b.y * wind_b.w);
    let q = (leaves + sway) * DAPPLE_FREQ;
    let n = 0.65 * dapple_noise(q) + 0.35 * dapple_noise(q * 2.3 + vec2<f32>(17.0, 5.0));
    return canopy * DAPPLE_LIGHT * smoothstep(DAPPLE_OPEN.x, DAPPLE_OPEN.y, n);
}

// How far into the edge band `world_pos` is: 0 inside, 1 at and past the shadow distance.
fn edge_fade(world_pos: vec3<f32>, camera_pos: vec3<f32>, gfx: GfxData) -> f32 {
    let range = gfx.rows[gfx_rows::SHADOW_A].y;
    return smoothstep(range - SHADOW_EDGE_BAND, range, distance(world_pos, camera_pos));
}

// The light the map lets through at `world_pos`, 1 = lit, edge-faded; 1 when no map is live.
// `unit` samples once above a skinned rig's origin, with an upward normal.
fn visibility(
    unit: bool,
    world_pos: vec3<f32>,
    normal: vec3<f32>,
    camera_pos: vec3<f32>,
    map: texture_depth_2d_array,
    cmp: sampler_comparison,
    gfx: GfxData,
) -> f32 {
    let texel = gfx.rows[gfx_rows::SHADOW_TEXEL];
    if (texel.y < 0.5) {
        return 1.0;
    }
    let edge = edge_fade(world_pos, camera_pos, gfx);
    if (edge >= 1.0) {
        return 1.0;
    }
    let b = gfx.rows[gfx_rows::SHADOW_B];
    let len2 = dot(normal, normal);
    var n = select(vec3<f32>(0.0, 1.0, 0.0), normal * inverseSqrt(len2), len2 > 1e-12);
    var p = world_pos;
    if (unit) {
        p = p + b.xyz * UNIT_LIFT;
        n = vec3<f32>(0.0, 1.0, 0.0);
    }
    // Bevy's directional biases: the normal offset in texels of the cascade, the depth offset
    // along the light.
    let offset = p + n * texel.x + b.xyz * b.w;
    let clip_from_world = mat4x4<f32>(
        gfx.rows[gfx_rows::SHADOW_CLIP],
        gfx.rows[gfx_rows::SHADOW_CLIP + 1u],
        gfx.rows[gfx_rows::SHADOW_CLIP + 2u],
        gfx.rows[gfx_rows::SHADOW_CLIP + 3u],
    );
    let clip = clip_from_world * vec4<f32>(offset, 1.0);
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    if (any(ndc.xy < vec2<f32>(-1.0)) || ndc.z < 0.0 || any(ndc > vec3<f32>(1.0))) {
        return 1.0;
    }
    let light_local = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    var lit = castano(map, cmp, light_local, ndc.z);
    if (!unit && lit < 0.95 && texel.z > 0.0) {
        lit = max(lit, texel.z * dapple(
            map, light_local, ndc.z, world_pos, b.xyz, clip_from_world, gfx,
        ));
    }
    return mix(lit, 1.0, edge);
}

// The stock light sum with the direct part shadowed: everwood's `ambient + (lit − ambient) ×
// mix(floor, 1, shadow)`, the shadow weighted by the sun's weight (none while the map holds the
// moon: the moon's shadow falls on the moon's light alone, `moon.wgsl`).
fn shade(lit: vec3<f32>, ambient: vec3<f32>, vis: f32, gfx: GfxData) -> vec3<f32> {
    let a = gfx.rows[gfx_rows::SHADOW_A];
    let s = 1.0 - (1.0 - vis) * a.w;
    let direct = max(lit - ambient, vec3<f32>(0.0));
    return min(ambient, lit) + direct * mix(a.z, 1.0, s);
}

// Terrain's law (everwood's terrain receiver): the whole light sum dimmed toward the floor, since
// the ground's vertex light carries the sun in its ambient part as well.
fn shade_whole(lit: vec3<f32>, vis: f32, gfx: GfxData) -> vec3<f32> {
    let a = gfx.rows[gfx_rows::SHADOW_A];
    let s = 1.0 - (1.0 - vis) * a.w;
    return lit * mix(a.z, 1.0, s);
}

// How far the bake steps aside: the sun's weight, faded with the map's edge.
fn baked_aside(world_pos: vec3<f32>, camera_pos: vec3<f32>, gfx: GfxData) -> f32 {
    var w = gfx.rows[gfx_rows::SHADOW_A].w;
    // The probe's bake-aside switch (`shadows.rs`), while the map is live.
    let texel = gfx.rows[gfx_rows::SHADOW_TEXEL];
    if (texel.y > 0.5 && (u32(texel.w) & 2u) != 0u) {
        w = 1.0;
    }
    if (w <= 0.0) {
        return 0.0;
    }
    return w * (1.0 - edge_fade(world_pos, camera_pos, gfx));
}
