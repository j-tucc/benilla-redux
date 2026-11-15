#define_import_path redux::gfx_fire

// Firelight (LOOK-16), called from this crate's `gfx_surface` body: fires, braziers and lanterns
// light the exterior surfaces round them per pixel, with a warm windowed inverse-square falloff,
// a wrapped facing term and a gentle flicker. The `FIRES` rows (`firelight.rs`) hold the nearest
// outdoor point lights of the engine's own table: (world xyz, packed colour, its sign set when the
// flicker is already in it). `MISC` y is how
// many are live, z the gain, w the flicker clock (seconds, wrapped where every flicker cycle is
// whole). A zero count adds nothing.
//
// The flicker is a pure function of the clock and the light's position, so a still frame is the
// same under TAA's jitter and the light never accumulates state.

#import benilla::gfx_data::GfxData
#import redux::gfx_rows

// How far a fire's light reaches (yd): the window takes it to zero there.
const FIRE_RADIUS: f32 = 12.0;
// The falloff's knee (yd): the light is halved this far out, then falls as the inverse square.
const FIRE_KNEE: f32 = 2.5;
// A named prop's light fades in over this far (yd), so the prop it sits in keeps its look.
const NAMED_CORE: f32 = 1.4;
// How far the light wraps past the terminator, so flat ground beside a low flame still takes it.
const FIRE_WRAP: f32 = 0.6;
// The flicker's three tones (Hz, each a whole number of cycles per `firelight::CLOCK_WRAP_S`) and
// depths.
const FLICKER_HZ: vec3<f32> = vec3<f32>(1.5, 2.5, 3.7);
const FLICKER_DEPTH: vec3<f32> = vec3<f32>(0.09, 0.06, 0.04);

// A packed colour word (`firelight::pack_rgb`): three unorm8 channels.
fn fire_rgb(word: f32) -> vec3<f32> {
    let bits = bitcast<u32>(word);
    return vec3<f32>(
        f32(bits & 0xffu), f32((bits >> 8u) & 0xffu), f32((bits >> 16u) & 0xffu),
    ) / 255.0;
}

// The flicker's wave at `clock` for a light whose phase comes from its position: its swing
// about 1, at full depth.
fn fire_wave(clock: f32, at: vec3<f32>) -> f32 {
    // An integer hash of the light's half-yard cell: exact at any distance from the origin.
    let cell = bitcast<vec3<u32>>(vec3<i32>(floor(at * 2.0)));
    var h = (cell.x * 73856093u) ^ (cell.y * 19349663u) ^ (cell.z * 83492791u);
    h = (h ^ (h >> 16u)) * 2246822519u;
    h = h ^ (h >> 13u);
    let phase = f32(h & 0xffffu) / 65535.0 * 6.2831853;
    let t = 6.2831853 * FLICKER_HZ * clock + vec3<f32>(phase, phase * 2.3, phase * 0.7);
    return dot(FLICKER_DEPTH, sin(t));
}

// The flicker at `clock` for a light whose phase comes from its position: warm lights (flames)
// flicker, cool ones (magic) hold steady.
fn fire_flicker(clock: f32, at: vec3<f32>, rgb: vec3<f32>) -> f32 {
    let warm = clamp((rgb.r - rgb.b) / max(rgb.r, 1e-3), 0.0, 1.0);
    return 1.0 + warm * fire_wave(clock, at);
}

// How deep a lantern's flicker is beside a flame's: a flame behind glass shimmers.
const LANTERN_DEPTH: f32 = 0.4;

// The flicker of a light by the kind its word carries in the fourth byte's low bits
// (`firelight::pack_light`, `locallights::Flicker`): 0 by hue, 1 a flame, 2 a lantern, 3 steady.
fn fire_flicker_of(clock: f32, at: vec3<f32>, word: f32, rgb: vec3<f32>) -> f32 {
    let kind = (bitcast<u32>(word) >> 24u) & 3u;
    if (kind == 0u) {
        return fire_flicker(clock, at, rgb);
    }
    let depth = select(select(0.0, LANTERN_DEPTH, kind == 2u), 1.0, kind == 1u);
    return 1.0 + depth * fire_wave(clock, at);
}

// One light's share at `world_pos` facing `n` (`has_n` false when the surface authored none).
fn fire_one(row: vec4<f32>, world_pos: vec3<f32>, n: vec3<f32>, has_n: bool, clock: f32) -> vec3<f32> {
    // Both positions are absolute, so their difference is good to the f32 step (~1 mm at
    // Northshire), far finer than the light's falloff.
    let to_light = row.xyz - world_pos;
    let d2 = dot(to_light, to_light);
    if (d2 >= FIRE_RADIUS * FIRE_RADIUS) {
        return vec3<f32>(0.0);
    }
    let d = sqrt(d2);
    let edge = d2 / (FIRE_RADIUS * FIRE_RADIUS);
    let window = (1.0 - edge * edge) * (1.0 - edge * edge);
    var falloff = window / (1.0 + d2 / (FIRE_KNEE * FIRE_KNEE));
    // A light the map's list made from a prop's name (a flicker kind set) sits inside the prop:
    // its own post, bowl and glass would take the full gain and white out, so it fades in over
    // the first `NAMED_CORE` yards.
    if (((bitcast<u32>(row.w) >> 24u) & 3u) != 0u) {
        falloff *= smoothstep(0.25 * NAMED_CORE, NAMED_CORE, d);
    }
    // A surface without a normal (some M2s author none) takes the light half-wrapped.
    var facing = 0.5;
    if (has_n) {
        facing = max((dot(n, to_light / max(d, 1e-4)) + FIRE_WRAP) / (1.0 + FIRE_WRAP), 0.0);
    }
    let rgb = fire_rgb(row.w);
    // A carried light's word has its sign set: it moves, so `firelight.rs` put its flicker in the
    // colour on a phase of its own, which the position hash here would scramble.
    let steady = (bitcast<u32>(row.w) & 0x80000000u) != 0u;
    let flicker = select(fire_flicker_of(clock, row.xyz, row.w, rgb), 1.0, steady);
    return rgb * (falloff * facing * flicker);
}

// The firelight reaching a surface at `world_pos` facing `normal`, to multiply its albedo. The
// lights are read at constant row indices: `gfx` is passed by value, and a loop indexing it would
// copy all its rows into every fragment's local memory (measured at ~20 ms a frame at 4K).
fn fire_light(world_pos: vec3<f32>, normal: vec3<f32>, gfx: GfxData) -> vec3<f32> {
    let misc = gfx.rows[gfx_rows::MISC];
    let count = u32(misc.y);
    if (count == 0u) {
        return vec3<f32>(0.0);
    }
    let len2 = dot(normal, normal);
    let has_n = len2 > 1e-12;
    let n = select(vec3<f32>(0.0), normal * inverseSqrt(max(len2, 1e-12)), has_n);
    let clock = misc.w;
    var sum = vec3<f32>(0.0);
    if (count > 0u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 0u], world_pos, n, has_n, clock); }
    if (count > 1u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 1u], world_pos, n, has_n, clock); }
    if (count > 2u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 2u], world_pos, n, has_n, clock); }
    if (count > 3u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 3u], world_pos, n, has_n, clock); }
    if (count > 4u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 4u], world_pos, n, has_n, clock); }
    if (count > 5u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 5u], world_pos, n, has_n, clock); }
    if (count > 6u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 6u], world_pos, n, has_n, clock); }
    if (count > 7u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 7u], world_pos, n, has_n, clock); }
    if (count > 8u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 8u], world_pos, n, has_n, clock); }
    if (count > 9u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 9u], world_pos, n, has_n, clock); }
    if (count > 10u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 10u], world_pos, n, has_n, clock); }
    if (count > 11u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 11u], world_pos, n, has_n, clock); }
    if (count > 12u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 12u], world_pos, n, has_n, clock); }
    if (count > 13u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 13u], world_pos, n, has_n, clock); }
    if (count > 14u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 14u], world_pos, n, has_n, clock); }
    if (count > 15u) { sum += fire_one(gfx.rows[gfx_rows::FIRES + 15u], world_pos, n, has_n, clock); }
    return sum * misc.z;
}
