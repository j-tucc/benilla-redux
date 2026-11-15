//! Local lights: the world's placed point lights as a list of their own, independent of the
//! models that carry them, the way a modern client keeps its lights as map data streamed with the
//! terrain. 1.12 has no such list: a light exists only once its prop's model has loaded and been
//! spawned. This derives one from the map instead.
//!
//! When an ADT tile loads, its doodad placements (MDDF) and its buildings' doodads (a WMO's MODD,
//! set 0 and the placement's set, exterior groups only) are walked for light-emitting models: the
//! M2's own point lights (`parse_m2_lights`, the engine's `casts()` gate) are the signal, and the
//! model's name picks how the light flickers. A model whose name says it carries a flame (a
//! brazier, torch, campfire, lantern, lamp post, forge) is read straight off the archives on a
//! worker, before the engine has decoded it; any other model's lights come from its `M2Model`
//! when the engine loads it. The list keeps every resident tile's lights: tiles are resident to
//! the stream's outer window (at least 8 chunks, 267 yd), where a prop the size of a brazier
//! fades out at 100–125 yd (`model_fade`), so a light is known before and beyond its model.
//!
//! [`super::firelight`] reads the list ([`LocalLights::lights`]) in place of the engine's own
//! placed lights, which it drops where a listed light stands ([`LocalLights::covers`]), so a
//! light whose model has also loaded is lit once.
use benilla_assets::coords::{placement_rotation, wmo_doodad_local, wow_to_bevy};
use benilla_assets::{m2_url, wmo_url, AdtTile, DoodadBase, M2Model, WmoModel, WorldAssets};
use benilla_formats::M2Light;
use bevy::asset::AssetEvent;
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, IoTaskPool, Task};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// How a light flickers: the shader's law per kind (`fire.wgsl`'s `fire_flicker_of`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Flicker {
    /// By hue: warm lights flicker as flames, cool ones hold. The engine's lights, whose model
    /// is not named.
    #[default]
    Warm = 0,
    /// A flame: a brazier, pyre, campfire, forge, torch or candle.
    Flame = 1,
    /// A flame behind glass: a lantern or lamp post, a gentler shimmer.
    Lantern = 2,
    /// A steady light.
    Steady = 3,
}

/// What a model's name says it carries: how its light flickers, its colour (linear, peak = its
/// strength), its reach (yd), and where it sits on a model with no flame emitter (the top of the
/// bounds, else their centre).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emits {
    pub flicker: Flicker,
    pub colour: [f32; 3],
    pub radius: f32,
    pub at_top: bool,
}

/// An open flame's colour. Deviation: no source value (1.12 authors these props no light); the
/// hue of the Elwynn campfire's own M2 light (0.71, 0.20, 0) lifted toward a flame's yellow, which
/// `firelight::fire_colour` leans further toward warm white.
const FLAME: [f32; 3] = [1.0, 0.55, 0.2];
/// A lamp's colour behind glass, a little paler. Deviation: no source value; tuned in stills.
const LAMP: [f32; 3] = [1.0, 0.7, 0.4];

/// Name fragments that say a model gives no light although a light word is in its name: a spent
/// candle or torch, an Ironforge banner, fireworks, firewood, fireflies, an elevator.
const DARK: [&str; 9] = [
    "off", "_out", "unlit", "broken", "banner", "firework", "firewood", "firefl", "elevator",
];

/// One light word: (fragment, flicker, colour, strength, reach, on top).
type Word = (&'static str, Flicker, [f32; 3], f32, f32, bool);

/// The light words, first match wins; a bare "lamp" last.
/// Deviation: 1.12 authors a light on almost none of these props (Goldshire's lamp posts, the
/// Stormwind braziers and street lamps, lanterns, pyres, torches, candles and the forge carry
/// none: of the props round Goldshire only a held torch does), so the name is the signal;
/// strengths tuned in stills against the Goldshire and Darkshire night.
const WORDS: [Word; 14] = [
    ("brazier", Flicker::Flame, FLAME, 0.8, 12.0, true),
    ("pyre", Flicker::Flame, FLAME, 0.8, 12.0, true),
    ("campfire", Flicker::Flame, FLAME, 0.8, 12.0, true),
    ("bonfire", Flicker::Flame, FLAME, 0.8, 12.0, true),
    ("firepit", Flicker::Flame, FLAME, 0.8, 12.0, true),
    ("fireplace", Flicker::Flame, FLAME, 0.7, 10.0, false),
    ("forge", Flicker::Flame, FLAME, 0.7, 10.0, false),
    ("torch", Flicker::Flame, FLAME, 0.6, 10.0, true),
    ("sconce", Flicker::Flame, FLAME, 0.5, 9.0, true),
    ("candle", Flicker::Flame, FLAME, 0.3, 5.0, true),
    ("streetlamp", Flicker::Lantern, LAMP, 0.55, 12.0, true),
    ("lamppost", Flicker::Lantern, LAMP, 0.55, 12.0, true),
    ("lantern", Flicker::Lantern, LAMP, 0.45, 9.0, false),
    ("lamp", Flicker::Lantern, LAMP, 0.45, 9.0, true),
];

impl Emits {
    /// What a model's path (lowercased) says it carries; `None` for any other model.
    pub fn of_name(path: &str) -> Option<Emits> {
        let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
        if DARK.iter().any(|k| file.contains(k)) {
            return None;
        }
        let (_, flicker, colour, strength, radius, at_top) =
            WORDS.iter().find(|w| file.contains(w.0))?;
        Some(Emits {
            flicker: *flicker,
            colour: colour.map(|c| c * strength),
            radius: *radius,
            at_top: *at_top,
        })
    }
}

/// One placed light, Bevy world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalLight {
    pub at: Vec3,
    /// Linear RGB × the intensity, the engine's point-table convention.
    pub colour: [f32; 3],
    /// The light's reach (yd): the authored attenuation end, or its kind's.
    pub radius: f32,
    pub flicker: Flicker,
}

/// One model's lights, model space (Bevy axes, unscaled), with the kind its name picks.
#[derive(Clone, Debug, Default, PartialEq)]
struct ModelLights {
    points: Vec<(Vec3, [f32; 3], f32)>,
    flicker: Flicker,
}

/// Emitters closer than this (yd, across) are one flame: a brazier's fire, embers and smoke.
const FLAME_CLUSTER: f32 = 1.5;
/// The most lights one model gives (a lamp post's lamps).
const MAX_PER_MODEL: usize = 4;

impl ModelLights {
    /// A model's lights. Its casting point lights, as the engine spawns them (`spawn_lights_for`:
    /// at the def's rest position, `diffuse_color × diffuse_intensity`), when it authors any; else,
    /// when its name says it carries a light ([`Emits`]), one at each cluster of its particle
    /// emitters (the flames), or on its bounds when it has none. WoW model space in.
    fn build<'a>(
        path: &str,
        authored: impl Iterator<Item = &'a M2Light>,
        emitters: impl Iterator<Item = [f32; 3]>,
        bounds: Option<([f32; 3], [f32; 3])>,
    ) -> Self {
        let emits = Emits::of_name(&path.to_ascii_lowercase());
        let flicker = emits.map_or(Flicker::Warm, |e| e.flicker);
        let points: Vec<_> = authored
            .filter(|l| l.casts())
            .map(|l| {
                let k = l.diffuse_intensity.max(0.0);
                (
                    wow_to_bevy(l.position),
                    l.diffuse_color.map(|c| c * k),
                    l.attenuation_end.max(0.0),
                )
            })
            .collect();
        if !points.is_empty() {
            return Self { points, flicker };
        }
        let Some(e) = emits else {
            return Self::default();
        };
        // Flames: emitter clusters across the ground plane (WoW Z is up), each light at its
        // cluster's lowest emitter, the fire under its smoke and sparks.
        let mut flames: Vec<Vec3> = Vec::new();
        for p in emitters.map(Vec3::from_array) {
            match flames
                .iter_mut()
                .find(|f| f.truncate().distance(p.truncate()) < FLAME_CLUSTER)
            {
                Some(f) if p.z < f.z => *f = p,
                Some(_) => {}
                None => flames.push(p),
            }
        }
        if flames.is_empty() {
            let Some((lo, hi)) = bounds.map(|(a, b)| (Vec3::from_array(a), Vec3::from_array(b)))
            else {
                return Self::default();
            };
            let mut at = (lo + hi) * 0.5;
            if e.at_top {
                at.z = hi.z - 0.15 * (hi.z - lo.z);
            }
            flames.push(at);
        }
        Self {
            points: flames
                .into_iter()
                .take(MAX_PER_MODEL)
                .map(|p| (wow_to_bevy(p.to_array()), e.colour, e.radius))
                .collect(),
            flicker,
        }
    }

    /// [`Self::build`] off a model's raw bytes.
    fn from_bytes(path: &str, bytes: &[u8]) -> Self {
        let lights = benilla_formats::parse_m2_lights(bytes);
        let emitters = benilla_formats::parse_m2_particle_emitters(bytes).unwrap_or_default();
        let bounds = benilla_formats::parse_m2_bounds(bytes)
            .ok()
            .map(|b| (b.bbox_min, b.bbox_max));
        Self::build(
            path,
            lights.iter(),
            emitters.iter().map(|e| e.position),
            bounds,
        )
    }

    /// [`Self::build`] off the engine's loaded model.
    fn from_model(path: &str, m: &M2Model) -> Self {
        Self::build(
            path,
            m.lights.iter().map(|l| &l.def),
            m.emitters.iter().map(|e| e.def.position),
            m.bounds.as_ref().map(|b| (b.bbox_min, b.bbox_max)),
        )
    }
}

/// A placed model on a tile: its light key, which model (its `mpq://` URL, lowercased) and its
/// world transform.
#[derive(Clone, Debug)]
struct Placed {
    key: u64,
    url: Arc<str>,
    transform: Transform,
}

/// A building on a tile, waiting on its WMO: uniqueId, URL, transform, MODF doodad set.
#[derive(Clone, Debug)]
struct Building {
    unique_id: u32,
    url: Arc<str>,
    transform: Transform,
    set: u16,
}

/// One resident tile: its placements still waiting on a model, its buildings still waiting on
/// their WMO, and the lights resolved so far.
#[derive(Default)]
struct Tile {
    waiting: Vec<Placed>,
    buildings: Vec<Building>,
    lights: Vec<(u64, LocalLight)>,
}

/// The light list, with what it is still waiting on.
#[derive(Resource, Default)]
pub struct LocalLights {
    tiles: HashMap<AssetId<AdtTile>, Tile>,
    /// Every model looked at: its lights, or none.
    models: HashMap<Arc<str>, Arc<ModelLights>>,
    /// Name-matched models being read off the archives.
    reading: HashMap<Arc<str>, Task<ModelLights>>,
    /// The merged list, one light per placement light, rebuilt when a tile changes.
    merged: Vec<LocalLight>,
    /// The merged lights' half-yard cells, for [`Self::covers`].
    cells: HashSet<[i32; 3]>,
    dirty: bool,
}

/// The cell a position falls in, half a yard on a side.
fn cell(at: Vec3) -> [i32; 3] {
    let c = (at * 2.0).floor();
    [c.x as i32, c.y as i32, c.z as i32]
}

impl LocalLights {
    /// Every resident tile's lights.
    pub fn lights(&self) -> &[LocalLight] {
        &self.merged
    }

    /// Whether a listed light stands within half a yard of `at`: the engine's light there is the
    /// same placement's.
    pub fn covers(&self, at: Vec3) -> bool {
        let [x, y, z] = cell(at);
        (-1..=1).any(|dx| {
            (-1..=1).any(|dy| (-1..=1).any(|dz| self.cells.contains(&[x + dx, y + dy, z + dz])))
        })
    }

    /// Placements still waiting on their model, for the probe.
    fn waiting(&self) -> usize {
        self.tiles.values().map(|t| t.waiting.len()).sum()
    }
}

pub(super) struct LocalLightsPlugin;

impl Plugin for LocalLightsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalLights>()
            .add_systems(Update, (track_tiles, resolve, merge, probe).chain());
    }
}

/// A placement's light key: the MDDF uniqueId, or a WMO's uniqueId with its MODD index, so a
/// doodad straddling two tiles is listed once.
fn key(unique_id: u32, doodad: Option<usize>) -> u64 {
    match doodad {
        None => u64::from(unique_id),
        Some(i) => (1 << 63) | (u64::from(unique_id) << 24) | (i as u64 & 0xff_ffff),
    }
}

/// The lights of model `m` placed at `p`.
fn place<'a>(p: &'a Placed, m: &'a ModelLights) -> impl Iterator<Item = (u64, LocalLight)> + 'a {
    m.points.iter().map(move |(at, colour, radius)| {
        (
            p.key,
            LocalLight {
                at: p.transform.transform_point(*at),
                colour: *colour,
                radius: radius * p.transform.scale.x,
                flicker: m.flicker,
            },
        )
    })
}

/// A tile's placements and buildings, as the streamer registers them (`register_doodad`,
/// `register_wmo`): the same transforms, so a listed light lands where the engine's does.
fn tile_of(adt: &AdtTile) -> Tile {
    Tile {
        waiting: adt
            .doodads
            .iter()
            .map(|d| Placed {
                key: key(d.unique_id, None),
                url: m2_url(&d.model).to_ascii_lowercase().into(),
                transform: Transform {
                    translation: wow_to_bevy(d.position),
                    rotation: placement_rotation(d.rotation),
                    scale: Vec3::splat(d.scale),
                },
            })
            .collect(),
        buildings: adt
            .wmos
            .iter()
            .map(|w| Building {
                unique_id: w.unique_id,
                url: wmo_url(&w.model).into(),
                transform: Transform {
                    translation: wow_to_bevy(w.position),
                    rotation: placement_rotation(w.rotation),
                    scale: Vec3::ONE,
                },
                set: w.doodad_set,
            })
            .collect(),
        lights: Vec::new(),
    }
}

/// A loaded building's exterior doodads (set 0 and its own set, as the streamer picks them), as
/// placements. An interior-group doodad's light stays out: interior surfaces never take the
/// firelight, and it would shine through the walls onto the ground outside.
fn building_doodads(b: &Building, wmo: &WmoModel) -> Vec<Placed> {
    let mut ranges: Vec<(u32, u32)> = wmo
        .doodad_sets
        .first()
        .map(|s| (s.start, s.count))
        .into_iter()
        .collect();
    if b.set != 0 {
        if let Some(s) = wmo.doodad_sets.get(usize::from(b.set)) {
            ranges.push((s.start, s.count));
        }
    }
    let mut out = Vec::new();
    for (start, count) in ranges {
        for (di, d) in wmo
            .doodads
            .iter()
            .enumerate()
            .skip(start as usize)
            .take(count as usize)
        {
            let interior = matches!(wmo.doodad_base.get(di), Some(DoodadBase::Interior(_)));
            if d.model.is_empty() || interior {
                continue;
            }
            let local = wmo_doodad_local(d.position, d.orientation, d.scale);
            out.push(Placed {
                key: key(b.unique_id, Some(di)),
                url: m2_url(&d.model).to_ascii_lowercase().into(),
                transform: b.transform.mul_transform(local),
            });
        }
    }
    out
}

/// Tiles in and out: a new tile's placements wait on their models, a dropped tile's lights go.
fn track_tiles(
    mut events: MessageReader<AssetEvent<AdtTile>>,
    tiles: Res<Assets<AdtTile>>,
    mut list: ResMut<LocalLights>,
) {
    if !enabled() {
        events.clear();
        return;
    }
    for event in events.read() {
        match *event {
            AssetEvent::LoadedWithDependencies { id } | AssetEvent::Added { id } => {
                if list.tiles.contains_key(&id) {
                    continue;
                }
                if let Some(adt) = tiles.get(id) {
                    list.tiles.insert(id, tile_of(adt));
                }
            }
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                let gone = list.tiles.remove(&id).is_some();
                list.dirty |= gone;
            }
            _ => {}
        }
    }
}

/// Resolves waiting placements against the models known so far: name-matched models are read off
/// the archives, the rest are taken from the engine's `M2Model` once loaded; buildings expand to
/// their exterior doodads once their WMO has loaded.
fn resolve(
    mut list: ResMut<LocalLights>,
    server: Res<AssetServer>,
    m2s: Res<Assets<M2Model>>,
    wmos: Res<Assets<WmoModel>>,
    world_assets: Option<Res<WorldAssets>>,
    mut m2_events: MessageReader<AssetEvent<M2Model>>,
) {
    let list = &mut *list;
    // Finished archive reads.
    let done: Vec<Arc<str>> = list
        .reading
        .iter()
        .filter(|(_, task)| task.is_finished())
        .map(|(url, _)| url.clone())
        .collect();
    for url in done {
        if let Some(task) = list.reading.remove(&url) {
            let lights = block_on(future::poll_once(task)).unwrap_or_default();
            list.models.insert(url, Arc::new(lights));
        }
    }
    // Models the engine loaded.
    for event in m2_events.read() {
        let AssetEvent::LoadedWithDependencies { id } = *event else {
            continue;
        };
        let Some(path) = server.get_path(id) else {
            continue;
        };
        let url: Arc<str> = path.to_string().to_ascii_lowercase().into();
        if list.models.contains_key(&url) {
            continue;
        }
        if let Some(m) = m2s.get(id) {
            let lights = ModelLights::from_model(&url, m);
            list.models.insert(url.clone(), Arc::new(lights));
            list.reading.remove(&url);
        }
    }

    let mut wanted: HashSet<Arc<str>> = HashSet::new();
    let models = &list.models;
    for tile in list.tiles.values_mut() {
        // Buildings: once the WMO is loaded, its exterior doodads join the waiting list. One a
        // tile a frame keeps the frame flat.
        let loaded = |b: &Building| {
            server
                .get_handle::<WmoModel>(b.url.to_string())
                .and_then(|h| wmos.get(&h))
        };
        if let Some(i) = tile.buildings.iter().position(|b| loaded(b).is_some()) {
            let b = tile.buildings.swap_remove(i);
            if let Some(wmo) = loaded(&b) {
                tile.waiting.extend(building_doodads(&b, wmo));
            }
        }
        if tile.waiting.is_empty() {
            continue;
        }
        let before = tile.lights.len();
        let mut kept = Vec::new();
        for p in std::mem::take(&mut tile.waiting) {
            match models.get(&p.url) {
                Some(m) => tile.lights.extend(place(&p, m)),
                None => {
                    wanted.insert(p.url.clone());
                    kept.push(p);
                }
            }
        }
        tile.waiting = kept;
        if tile.lights.len() != before {
            list.dirty = true;
        }
    }
    // Name-matched models: read them now rather than wait for the engine to decode them.
    let Some(chain) = world_assets.map(|w| w.chain.clone()) else {
        return;
    };
    for url in wanted {
        if list.reading.contains_key(&url)
            || list.models.contains_key(&url)
            || Emits::of_name(&url).is_none()
        {
            continue;
        }
        let file = url.trim_start_matches("mpq://").replace('/', "\\");
        let chain = chain.clone();
        let name = url.clone();
        let task = IoTaskPool::get().spawn(async move {
            let bytes = chain.lock().ok().and_then(|c| c.read(&file).ok());
            bytes.map_or_else(ModelLights::default, |b| ModelLights::from_bytes(&name, &b))
        });
        list.reading.insert(url, task);
    }
}

/// Rebuilds the merged list when a tile changed: one light per placement light.
fn merge(mut list: ResMut<LocalLights>) {
    if !list.dirty {
        return;
    }
    let list = &mut *list;
    list.dirty = false;
    let mut seen: HashSet<(u64, [i32; 3])> = HashSet::new();
    list.merged.clear();
    list.cells.clear();
    for tile in list.tiles.values() {
        for (k, l) in &tile.lights {
            if seen.insert((*k, cell(l.at))) {
                list.merged.push(*l);
                list.cells.insert(cell(l.at));
            }
        }
    }
}

/// `$REDUX_LIGHTS_PROBE=<seconds>`: one line a second until then: the list's size and what it
/// still waits on, then, near the camera, the listed lights and the engine's outdoor unparented
/// (placed) lights within 60 and 250 yd, and how many of the engine's the list covers.
#[allow(clippy::type_complexity)]
fn probe(
    time: Res<Time<Real>>,
    list: Res<LocalLights>,
    camera: Query<&GlobalTransform, With<benilla_world::view::WorldCamera>>,
    engine: Query<
        &GlobalTransform,
        (
            With<benilla_world::lighting::WorldPointLight>,
            Without<benilla_world::lighting::LightRooms>,
            Without<ChildOf>,
        ),
    >,
    mut next: Local<f32>,
) {
    static UNTIL: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    let Some(until) = *UNTIL.get_or_init(|| {
        std::env::var("REDUX_LIGHTS_PROBE")
            .ok()
            .and_then(|v| v.parse().ok())
    }) else {
        return;
    };
    let now = time.elapsed_secs();
    if now > until || now < *next {
        return;
    }
    *next = now + 1.0;
    if std::env::var_os("REDUX_LIGHTS_NAMES").is_some() && now > until - 1.5 {
        let mut named: Vec<_> = list
            .models
            .iter()
            .filter(|(url, _)| Emits::of_name(url).is_some())
            .map(|(url, m)| format!("{}:{}", m.points.len(), url))
            .collect();
        named.sort();
        for n in named {
            info!("LOCAL_LIGHTS_MODEL {n}");
        }
    }
    let eye = camera.iter().next().map_or(Vec3::ZERO, |t| t.translation());
    let within = |at: Vec3, r: f32| at.distance_squared(eye) < r * r;
    if std::env::var_os("REDUX_LIGHTS_NAMES").is_some() && now > until - 1.5 {
        let mut near: Vec<&LocalLight> = list.merged.iter().collect();
        near.sort_by(|a, b| {
            a.at.distance_squared(eye)
                .total_cmp(&b.at.distance_squared(eye))
        });
        let wow = benilla_assets::coords::bevy_to_wow;
        info!("LOCAL_LIGHTS_EYE {:?}", wow(eye));
        for l in near.into_iter().take(12) {
            info!(
                "LOCAL_LIGHTS_NEAR {:?} {:?} {:.0}",
                wow(l.at),
                l.flicker,
                l.at.distance(eye)
            );
        }
    }
    let listed = |r: f32| list.merged.iter().filter(|l| within(l.at, r)).count();
    let placed = |r: f32| engine.iter().filter(|t| within(t.translation(), r)).count();
    let covered = engine
        .iter()
        .filter(|t| within(t.translation(), 250.0) && list.covers(t.translation()))
        .count();
    info!(
        "LOCAL_LIGHTS t={now:.1} lights={} tiles={} waiting={} reading={} models={} listed60={} listed250={} engine60={} engine250={} covered250={covered}",
        list.merged.len(),
        list.tiles.len(),
        list.waiting(),
        list.reading.len(),
        list.models.len(),
        listed(60.0),
        listed(250.0),
        placed(60.0),
        placed(250.0),
    );
}

/// `$REDUX_LOCAL_LIGHTS=0` keeps the list empty for a session, so the firelight runs on the
/// engine's lights alone (an A/B switch).
fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("REDUX_LOCAL_LIGHTS").map_or(true, |v| v.trim() != "0"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_name_says_whether_and_how_it_lights() {
        let of = |p: &str| Emits::of_name(&p.to_ascii_lowercase()).map(|e| e.flicker);
        // Round Goldshire (`REDUX_LIGHTS_NAMES`): none of these authors a casting light.
        for (path, want) in [
            (
                "world/azeroth/elwynn/passivedoodads/lamppost/lamppost.m2",
                Some(Flicker::Lantern),
            ),
            (
                "world/generic/human/passive doodads/braziers/stormwindbrazier01.m2",
                Some(Flicker::Flame),
            ),
            (
                "world/generic/human/passive doodads/lamps/stormwindstreetlamp01.m2",
                Some(Flicker::Lantern),
            ),
            (
                "world/generic/human/passive doodads/hanginglanterns/generalhanginglantern01.m2",
                Some(Flicker::Lantern),
            ),
            (
                "world/generic/human/passive doodads/stonepyres/stonepyre01.m2",
                Some(Flicker::Flame),
            ),
            (
                "world/generic/passivedoodads/lights/generaltorch01.m2",
                Some(Flicker::Flame),
            ),
            (
                "world/generic/passivedoodads/lights/candle01.m2",
                Some(Flicker::Flame),
            ),
            (
                "world/skillactivated/tradeskillenablers/blacksmithforge.m2",
                Some(Flicker::Flame),
            ),
            (
                r"World\Azeroth\Elwynn\PassiveDoodads\Campfire\ElwynnCampfire.m2",
                Some(Flicker::Flame),
            ),
            // Light words that light nothing.
            ("world/generic/passivedoodads/lights/candleoff01.m2", None),
            ("world/generic/passivedoodads/lights/torch_out.m2", None),
            (
                "world/generic/dwarf/passive doodads/banners/ironforgebanner_ornate01.m2",
                None,
            ),
            (
                "world/generic/human/passive doodads/gunshop/gunshopfireworksbig01.m2",
                None,
            ),
            (
                "world/generic/human/passive doodads/firewood/firewoodpile01.m2",
                None,
            ),
            ("world/critter/fireflies/fireflies01.m2", None),
            (
                "world/generic/human/passive doodads/elevatorparts/burningsteppselevator.m2",
                None,
            ),
            // The folder says nothing about the file.
            ("world/kalimdor/torchwood/trees/oak01.m2", None),
        ] {
            assert_eq!(of(path), want, "{path}");
        }
        let candle = Emits::of_name("candle01.m2").unwrap();
        let brazier = Emits::of_name("brazier01.m2").unwrap();
        assert!(candle.colour[0] < brazier.colour[0] && candle.radius < brazier.radius);
    }

    /// A named prop with no authored light takes one at each flame (an emitter cluster's lowest
    /// emitter), or near the top of its bounds when it has no emitter; an unnamed one takes none.
    #[test]
    fn a_named_prop_lights_at_its_flames_or_on_its_bounds() {
        let none = std::iter::empty::<&M2Light>;
        // Two flames 3 yd apart, each with smoke above it.
        let emitters = [
            [0.0, 0.0, 1.0],
            [0.2, 0.0, 2.0],
            [3.0, 0.0, 1.2],
            [3.0, 0.1, 2.5],
        ];
        let m = ModelLights::build("stonepyre01.m2", none(), emitters.into_iter(), None);
        assert_eq!(m.flicker, Flicker::Flame);
        let at: Vec<Vec3> = m.points.iter().map(|p| p.0).collect();
        assert_eq!(
            at,
            vec![wow_to_bevy([0.0, 0.0, 1.0]), wow_to_bevy([3.0, 0.0, 1.2])]
        );
        let bounds = Some(([-0.5, -0.5, 0.0], [0.5, 0.5, 4.0]));
        let m = ModelLights::build("lamppost.m2", none(), std::iter::empty(), bounds);
        assert_eq!(m.points.len(), 1);
        assert_eq!(m.points[0].0, wow_to_bevy([0.0, 0.0, 3.4]));
        assert_eq!(m.flicker, Flicker::Lantern);
        let m = ModelLights::build("generallantern01.m2", none(), std::iter::empty(), bounds);
        assert_eq!(m.points[0].0, wow_to_bevy([0.0, 0.0, 2.0]));
        let tree = ModelLights::build("oak01.m2", none(), emitters.into_iter(), bounds);
        assert!(tree.points.is_empty());
    }

    /// A casting point light keeps the engine's colour (`diffuse × intensity`) and rest position
    /// over any name-made one; a dark or ambient one is left out.
    #[test]
    fn only_casting_point_lights_are_kept() {
        let lit = M2Light {
            light_type: 1,
            bone: -1,
            position: [1.0, 2.0, 3.0],
            ambient_color: [0.0; 3],
            ambient_intensity: 0.0,
            diffuse_color: [0.5, 0.25, 0.0],
            diffuse_intensity: 2.0,
            attenuation_start: 1.0,
            attenuation_end: 6.0,
            bone_z: [0.0, 0.0, 1.0],
            visibility_off: false,
        };
        let dark = M2Light {
            visibility_off: true,
            ..lit
        };
        let ambient = M2Light {
            light_type: 0,
            ..lit
        };
        let m = ModelLights::build(
            "x/torch01.m2",
            [lit, dark, ambient].iter(),
            std::iter::once([9.0; 3]),
            None,
        );
        assert_eq!(m.flicker, Flicker::Flame);
        assert_eq!(
            m.points,
            vec![(wow_to_bevy([1.0, 2.0, 3.0]), [1.0, 0.5, 0.0], 6.0)]
        );
    }

    /// A placed light lands where the engine spawns its own (`transform_point` of the def's
    /// position), so [`LocalLights::covers`] matches it, and a straddling doodad is listed once.
    #[test]
    fn a_placed_light_lands_on_the_engines_and_is_listed_once() {
        let m = ModelLights {
            points: vec![(Vec3::new(0.0, 1.5, 0.0), [1.0, 0.6, 0.2], 8.0)],
            flicker: Flicker::Lantern,
        };
        let p = Placed {
            key: key(42, None),
            url: "mpq://a.m2".into(),
            transform: Transform {
                translation: Vec3::new(-100.0, 50.0, 9000.0),
                rotation: placement_rotation([0.0, 90.0, 0.0]),
                scale: Vec3::splat(2.0),
            },
        };
        let engine_at = p.transform.transform_point(Vec3::new(0.0, 1.5, 0.0));
        let mut list = LocalLights::default();
        for id in [AssetId::<AdtTile>::invalid(), AssetId::default()] {
            let mut tile = Tile::default();
            tile.lights.extend(place(&p, &m));
            list.tiles.insert(id, tile);
        }
        list.dirty = true;
        let mut world = World::new();
        world.insert_resource(list);
        world.run_system_once(merge).unwrap();
        let list = world.resource::<LocalLights>();
        assert_eq!(list.lights().len(), 1);
        let l = list.lights()[0];
        assert_eq!(l.at, engine_at);
        assert_eq!((l.radius, l.flicker), (16.0, Flicker::Lantern));
        assert!(list.covers(engine_at + Vec3::splat(0.2)));
        assert!(!list.covers(engine_at + Vec3::X * 3.0));
    }

    #[test]
    fn keys_keep_mddf_and_modd_apart() {
        assert_ne!(key(7, None), key(7, Some(0)));
        assert_ne!(key(7, Some(1)), key(7, Some(2)));
        assert_ne!(key(7, Some(1)), key(8, Some(1)));
    }

    use bevy::ecs::system::RunSystemOnce;
}
