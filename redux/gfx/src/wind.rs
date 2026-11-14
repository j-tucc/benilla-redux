//! Foliage wind (LOOK-12): one shared wind field that sways ground clutter and leaf cards, and
//! parts the grass around the player and nearby bodies. The engine's clutter and static shaders
//! call the `gfx_vertex_offset` hook, whose body runs [`wind.wgsl`](wind.wgsl) under `GFX_WIND` on
//! the rows this plugin writes (`WIND_A`, `WIND_B`, `MISC`, `BENDERS`). Which models are leaves and
//! which are rigid is this plugin's classifier, registered with the engine at build (engine patch
//! 0010 carries the clutter attribute and the static leaf bit it feeds).
//!
//! The offsets are a pure function of the vertex and the rows: the gusts and the veer are functions
//! of time, and the one integrated value, the wind's travel, is a row, so a motion-vector pass can
//! re-run the hook on the previous frame's rows (kept from `rows::PREVIOUS`) to find last frame's
//! position.
//!
//! Ported from the everwood graphics fork of Benilla: `benilla-world/src/wind/mod.rs` (the field,
//! the travel clock and the benders) and `static_gx/mod.rs` (`foliage_wind_batch`).
//!
//! Ported from WarcraftXL (https://github.com/WarcraftXL) by iThorgrim — module
//! wxl-experimental-wind, field/Wind.hpp and field/Wind.cpp.
use super::rows::{BENDERS, MAX_BENDERS, MISC, WIND_A, WIND_B};
use super::Enhanced;
use benilla_world::{
    foliage::{set_foliage_classifier, FoliageClassifier},
    lighting::{GfxData, LightingResolveSet, WowLighting},
    view::Viewer,
    world_unit::{ViewerUnit, WorldUnit},
};
use bevy::prelude::*;

const TAU: f64 = std::f64::consts::TAU;
/// The wander's three sines: period ratios and weights. Source: WarcraftXL `Wind.cpp`.
const RATIOS: [f64; 3] = [1.0, 0.437, 0.1913];
const WEIGHTS: [f64; 3] = [0.60, 0.28, 0.12];

/// The wind profile, yards, seconds and degrees. Source: WarcraftXL `Wind.hpp` defaults, as
/// everwood ships them.
const BASE_SPEED: f32 = 9.0;
const HEADING_DEG: f32 = 45.0;
const GUST: f32 = 0.55;
const GUST_PERIOD: f32 = 17.0;
const VEER_DEG: f32 = 22.0;
const VEER_PERIOD: f32 = 53.0;
const SEED: u32 = 0x5749_4E44;
/// A full storm blows 2.1× the clear speed. Source: everwood `wind::sample` (weather coupling 1).
const STORM_SPEED_GAIN: f32 = 1.1;
/// A storm swings the foliage up to 60 % harder. Source: everwood `wind::STORM_SWAY_GAIN`.
const STORM_SWAY_GAIN: f32 = 0.6;
/// The travel's wrap in yards: every wave in `wind.wgsl` makes whole cycles per wrap.
/// Source: everwood `wind::TRAVEL_WRAP`.
const TRAVEL_WRAP: f64 = 4096.0;
/// The player's parting radius and the nearby bodies' range and radii (yd). Source: everwood
/// `update_benders`.
const PLAYER_BEND_RADIUS: f32 = 1.75;
const BENDER_RANGE: f32 = 40.0;
const BODY_BEND_RADIUS: (f32, f32) = (1.5, 2.0);

/// Models whose names say rock or cage never bend, whatever directory holds them. Source:
/// everwood `wind::rigid_model`.
const RIGID_TERMS: [&str; 8] = [
    "rock", "stone", "boulder", "pebble", "gravel", "rubble", "cage", "prison",
];
/// Names that carry alpha cards but are not foliage: lamps, tree huts, stumps, logs, facades.
/// Source: everwood `static_gx::foliage_wind_batch`.
const NOT_LEAF_TERMS: [&str; 6] = ["street", "treehut", "stump", "log", "facade", "fallen"];
const LEAF_TERMS: [&str; 15] = [
    "tree", "bush", "shrub", "foliage", "fern", "palm", "plant", "willow", "canopy", "hedge",
    "thorn", "cactus", "vine", "reed", "kelp",
];

/// A ground-clutter model that stays rigid: its file name names a rock or a cage.
fn rigid(path: &str) -> bool {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    RIGID_TERMS.iter().any(|t| name.contains(t))
}

/// An alpha-tested model batch that sways as leaf cards: a plant-family path, not rigid, not a
/// lamp, hut, stump or facade.
fn leaf(path: &str) -> bool {
    if rigid(path) {
        return false;
    }
    let p = path.to_ascii_lowercase();
    !NOT_LEAF_TERMS.iter().any(|t| p.contains(t)) && LEAF_TERMS.iter().any(|t| p.contains(t))
}

/// One instant of the field in Bevy's XZ plane.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WindSample {
    dir: Vec2,
    speed: f32,
    /// The shaped gust envelope, 0 at a lull and 1 at a peak.
    gust: f32,
}

fn seeded_unit(seed: u32, index: i32) -> f64 {
    let mut h = seed ^ (index as u32).wrapping_mul(0x9E37_79B9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    f64::from(h & 0x00FF_FFFF) / f64::from(0x0100_0000u32)
}

/// WarcraftXL's three-sine band-limited wander in `[-1, 1]`.
fn wander(lane: i32, period: f32, seconds: f64) -> f32 {
    let period = f64::from(period.max(0.05));
    RATIOS
        .into_iter()
        .zip(WEIGHTS)
        .enumerate()
        .map(|(i, (ratio, weight))| {
            let phase = seeded_unit(SEED, lane * 97 + i as i32) * TAU;
            let turns = seconds / (period * ratio);
            weight * (turns.fract() * TAU + phase).sin()
        })
        .sum::<f64>() as f32
}

/// The field at `seconds`; `storm` 0 clear to 1 a full storm, which scales only the speed.
fn sample(seconds: f64, storm: f32) -> WindSample {
    let gust = ((wander(0, GUST_PERIOD, seconds) + 1.0) * 0.5).powf(1.7);
    let gust_gain = 1.0 + GUST * (gust - 0.5) * 2.0;
    let weather_gain = 1.0 + storm.clamp(0.0, 1.0) * STORM_SPEED_GAIN;
    let heading = (HEADING_DEG + VEER_DEG * wander(1, VEER_PERIOD, seconds)).to_radians();
    WindSample {
        dir: Vec2::new(heading.cos(), heading.sin()),
        speed: (BASE_SPEED * gust_gain * weather_gain).max(0.0),
        gust,
    }
}

/// The integrated travel: the waves' phase is the integral of the speed, never `speed × age`,
/// whose rate grows with the session. Kept in f64 and published wrapped.
#[derive(Default)]
struct WindClock {
    seconds: f64,
    travel: f64,
}

pub(super) struct WindPlugin;

impl Plugin for WindPlugin {
    fn build(&self, app: &mut App) {
        // The marks cost nothing while the rows are zero, so they are set whatever the switch:
        // turning wind on needs no re-bake.
        set_foliage_classifier(FoliageClassifier { leaf, rigid });
        app.add_systems(
            Update,
            (fill_wind, fill_benders).chain().after(LightingResolveSet),
        );
    }
}

fn fill_wind(
    time: Res<Time>,
    enhanced: Res<Enhanced>,
    lighting: Res<WowLighting>,
    mut frame: ResMut<GfxData>,
    mut clock: Local<WindClock>,
    mut frozen: Local<Option<bool>>,
) {
    if !enhanced.wind {
        if frame.rows[WIND_A] != [0.0; 4] || frame.rows[WIND_B] != [0.0; 4] {
            frame.rows[WIND_A] = [0.0; 4];
            frame.rows[WIND_B] = [0.0; 4];
        }
        return;
    }
    let storm = lighting.storm_bcc();
    // `REDUX_GFX_WIND_FREEZE=1` (the shadow-flicker probe) holds the field still, bent.
    let dt = if *frozen
        .get_or_insert_with(|| std::env::var("REDUX_GFX_WIND_FREEZE").as_deref() == Ok("1"))
    {
        0.0
    } else {
        time.delta_secs_f64()
    };
    clock.seconds += dt;
    let s = sample(clock.seconds, storm);
    clock.travel = (clock.travel + f64::from(s.speed) * dt).rem_euclid(TRAVEL_WRAP);
    frame.rows[WIND_A] = [s.dir.x, s.dir.y, HEADING_DEG.to_radians(), s.gust];
    frame.rows[WIND_B] = [
        clock.travel as f32,
        1.0 + STORM_SWAY_GAIN * storm,
        1.0, // grass
        1.0, // trees
    ];
}

/// The player first, then the seven nearest bodies within 40 yd. The bodies are the streamed
/// units, which exist only in the cells loaded around the player.
fn fill_benders(
    enhanced: Res<Enhanced>,
    viewer: Res<Viewer>,
    units: Query<(&GlobalTransform, &WorldUnit), Without<ViewerUnit>>,
    mut frame: ResMut<GfxData>,
) {
    let player = viewer.at.filter(|_| enhanced.wind);
    let Some(player) = player else {
        if frame.rows[MISC][0] != 0.0 {
            frame.rows[MISC][0] = 0.0;
        }
        return;
    };
    frame.rows[BENDERS] = [player.x, player.y, player.z, PLAYER_BEND_RADIUS];
    let mut count = 1;
    // A fixed insertion list: no per-frame allocation or sort.
    let mut nearby = [(f32::INFINITY, Vec3::ZERO, 0.0); MAX_BENDERS - 1];
    for (transform, unit) in &units {
        let at = transform.translation();
        // A difference of nearby positions, compared at yard scale: precise enough.
        let d2 = (at.xz() - player.xz()).length_squared();
        if d2 > BENDER_RANGE * BENDER_RANGE || d2 >= nearby[6].0 {
            continue;
        }
        let mut slot = 6;
        while slot > 0 && d2 < nearby[slot - 1].0 {
            nearby[slot] = nearby[slot - 1];
            slot -= 1;
        }
        let radius =
            (BODY_BEND_RADIUS.0 * unit.scale).clamp(BODY_BEND_RADIUS.0, BODY_BEND_RADIUS.1);
        nearby[slot] = (d2, at, radius);
    }
    for (_, at, radius) in nearby.into_iter().take_while(|(d2, _, _)| d2.is_finite()) {
        frame.rows[BENDERS + count] = [at.x, at.y, at.z, radius];
        count += 1;
    }
    frame.rows[MISC][0] = count as f32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rocks_stay_rigid_and_plants_sway() {
        for path in [
            "World/Plants/BushRock01.m2",
            "World\\Trees\\STONE01.mdx",
            "Pebble01.m2",
        ] {
            assert!(rigid(path), "{path}");
            assert!(!leaf(path), "{path}");
        }
        assert!(!rigid("World/Stonetalon/Trees/Pine01.m2"));
        assert!(leaf("World/Stonetalon/Trees/Pine01.m2"));
        assert!(leaf("World/Generic/Bush/ElwynnBush01.m2"));
        assert!(!leaf("World/Generic/Human/StreetLamp/Tree_StreetLamp.m2"));
        assert!(!leaf("World/Azeroth/Elwynn/TreeStump01.m2"));
        assert!(!leaf("World/Generic/Human/Barrel01.m2"));
    }

    #[test]
    fn the_field_is_pure_and_weather_scales_only_speed() {
        let clear = sample(123.456, 0.0);
        assert_eq!(clear, sample(123.456, 0.0));
        let storm = sample(123.456, 1.0);
        assert_eq!((clear.dir, clear.gust), (storm.dir, storm.gust));
        assert!((storm.speed / clear.speed - 2.1).abs() < 1e-5);
        for t in [0.0, 17.0, 53.0, 86_400.0] {
            let s = sample(t, 0.0);
            assert!((s.dir.length() - 1.0).abs() < 1e-5);
            assert!((0.0..=1.0).contains(&s.gust));
        }
    }
}
