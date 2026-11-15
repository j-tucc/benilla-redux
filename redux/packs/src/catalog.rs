//! Local BC7 texture packs, one folder each, served through benilla's texture source hooks
//! (REDUX.md's patch ledger) and switchable at runtime ([`crate::switch`]). Ported from Elderoth's
//! `benilla-adapter/src/packs.rs`.
//!
//! A pack is `<packs root>/<pack>/textures/<lowercase install path>.dds` (DX10 BC7_UNORM, full
//! mips) with an optional `pack.json` (`name`, `scale`, `description`) and an optional `index.json`
//! which spares the boot scan. One source answers
//! both hooks: terrain layers (`tileset\...`, square power-of-two, the loader keeps a tile stock
//! unless all of its layers match) and every other world-art texture (any whole-block size).
//! Presentation only: nothing here reaches the wire or benilla's settings.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use benilla_assets::{
    LayerOverride, TerrainLayerSource, TextureOverride, TileLayerReport, WorldTextureSource,
};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use serde::{Deserialize, Serialize};

use crate::dds;

/// The selection name of the stock textures.
pub const VANILLA: &str = "vanilla";
const DEFAULT_CACHE_MB: u64 = 512;

/// `pack.json`, every field optional.
#[derive(Debug, Default, Deserialize)]
struct PackManifest {
    name: Option<String>,
    scale: Option<u32>,
    description: Option<String>,
}

/// A discovered pack folder.
#[derive(Clone, Debug)]
pub struct PackInfo {
    pub name: String,
    pub root: PathBuf,
    /// How many times larger than vanilla its textures are (`pack.json` `scale`, default 4).
    pub scale: u32,
    pub description: String,
}

/// Every pack under the packs root, by name.
#[derive(Resource, Clone, Debug, Default)]
pub struct PackCatalog {
    pub root: PathBuf,
    pub packs: Vec<PackInfo>,
}

impl PackCatalog {
    /// Every folder under `root` holding a `textures/` folder.
    pub(crate) fn discover(root: &Path) -> Self {
        let mut packs = Vec::new();
        for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let dir = entry.path();
            if !dir.join("textures").is_dir() {
                continue;
            }
            let manifest: PackManifest = std::fs::read_to_string(dir.join("pack.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let folder = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            packs.push(PackInfo {
                name: manifest.name.unwrap_or(folder),
                root: dir,
                scale: manifest.scale.filter(|s| *s > 0).unwrap_or(4),
                description: manifest.description.unwrap_or_default(),
            });
        }
        packs.sort_by_key(|p| p.name.to_lowercase());
        Self {
            root: root.to_path_buf(),
            packs,
        }
    }

    pub(crate) fn find(&self, name: &str) -> Option<&PackInfo> {
        self.packs
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// The packs root: `$REDUX_TEXTURE_PACKS`, else `classic-overrides/texture-packs` found beside
/// one of `starts` or any folder above it ([`crate::overrides_dir`]).
pub(crate) fn packs_root(starts: &[PathBuf]) -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("REDUX_TEXTURE_PACKS").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(root));
    }
    crate::overrides_dir(starts, "texture-packs")
}

/// The pack file for a normalized texture key (`tileset\elwynn\x.blp` → `tileset/elwynn/x.dds`).
pub(crate) fn pack_relative(key: &str) -> Option<String> {
    let stem = key.to_ascii_lowercase().replace('\\', "/");
    Some(format!("{}.dds", stem.strip_suffix(".blp")?))
}

/// `index.json`: `{"version": 1, "files": {"<rel>": [width, height, mips]}}`.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PackIndex {
    pub version: u32,
    pub files: HashMap<String, [u32; 3]>,
}

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    shape: dds::Bc7Shape,
}

/// A texture the pack served, for the reporter.
#[derive(Clone, Debug)]
pub struct ServedTexture {
    pub file: PathBuf,
    pub width: u32,
    pub height: u32,
}

/// What the loaders did with this pack, for the probe.
#[derive(Default, Debug, Clone)]
pub struct PackStats {
    pub tiles_replaced: u32,
    pub tiles_stock: u32,
    pub layer_bytes: u64,
    pub reasons: Vec<String>,
    pub tiles: Vec<String>,
    pub textures_served: u64,
    pub texture_bytes: u64,
}

#[derive(Default)]
struct Cache {
    chains: HashMap<String, Arc<[u8]>>,
    order: VecDeque<String>,
    bytes: u64,
}

/// An opened pack: its index, a bounded chain cache, its stats and what it served.
pub struct PackSource {
    pub info: PackInfo,
    files: HashMap<String, Entry>,
    cache: Mutex<Cache>,
    cache_limit: u64,
    pub stats: Mutex<PackStats>,
    served: Mutex<HashMap<String, ServedTexture>>,
    pub reads: AtomicU64,
    pub hits: AtomicU64,
    pub failures: AtomicU64,
    pub read_micros: AtomicU64,
}

impl PackSource {
    /// Open `info`'s index (`index.json`, else a header scan of every file).
    pub(crate) fn open(info: &PackInfo, cache_limit: u64) -> Result<Self, String> {
        let textures = info.root.join("textures");
        let mut files = HashMap::new();
        let index_path = info.root.join("index.json");
        let mut skipped = 0usize;
        if let Some(index) = std::fs::read_to_string(&index_path)
            .ok()
            .and_then(|s| serde_json::from_str::<PackIndex>(&s).ok())
            .filter(|i| i.version == 1)
        {
            for (rel, [width, height, mips]) in index.files {
                let path = textures.join(&rel);
                let shape = dds::Bc7Shape {
                    width,
                    height,
                    mips,
                };
                files.insert(rel.to_ascii_lowercase(), Entry { path, shape });
            }
        } else {
            eprintln!(
                "TEXTURE_PACK {}: no index.json, scanning headers",
                info.name
            );
            for (rel, path) in dds::files_under(&textures)? {
                match dds::read_head(&path).and_then(|(h, n)| dds::parse(&h, n)) {
                    Ok(shape) => {
                        files.insert(rel, Entry { path, shape });
                    }
                    Err(_) => skipped += 1,
                }
            }
        }
        if skipped > 0 {
            eprintln!(
                "TEXTURE_PACK {}: {skipped} files are not valid BC7 chains and stay vanilla",
                info.name
            );
        }
        Ok(Self {
            info: info.clone(),
            files,
            cache: Mutex::default(),
            cache_limit,
            stats: Mutex::default(),
            served: Mutex::default(),
            reads: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            read_micros: AtomicU64::new(0),
        })
    }

    pub fn texture_count(&self) -> usize {
        self.files.len()
    }

    /// The pack's file and size for a texture key it holds, served or not.
    pub fn file_for(&self, key: &str) -> Option<ServedTexture> {
        let entry = self.files.get(&pack_relative(key)?)?;
        Some(ServedTexture {
            file: entry.path.clone(),
            width: entry.shape.width,
            height: entry.shape.height,
        })
    }

    /// The textures this pack actually served since it was opened.
    pub fn served(&self) -> HashMap<String, ServedTexture> {
        self.served
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// `rel`'s chain, from the cache or the file, checked against the index shape.
    fn chain(&self, rel: &str, entry: &Entry) -> Option<Arc<[u8]>> {
        if let Some(chain) = self
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .chains
            .get(rel)
        {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Some(chain.clone());
        }
        let started = Instant::now();
        let read = std::fs::read(&entry.path).ok().filter(|bytes| {
            dds::parse(bytes, bytes.len().saturating_sub(dds::HEADER)).ok() == Some(entry.shape)
        });
        let Some(bytes) = read else {
            self.failures.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let chain: Arc<[u8]> = bytes[dds::HEADER..].into();
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.read_micros
            .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if !cache.chains.contains_key(rel) {
            cache.bytes += chain.len() as u64;
            cache.chains.insert(rel.to_string(), chain.clone());
            cache.order.push_back(rel.to_string());
            while cache.bytes > self.cache_limit {
                let Some(old) = cache.order.pop_front() else {
                    break;
                };
                if let Some(gone) = cache.chains.remove(&old) {
                    cache.bytes -= gone.len() as u64;
                }
            }
        }
        Some(chain)
    }

    /// A UI sprite's chain, for a sprite key (`interface\buttons\ui-panel-button-up`, no
    /// extension), when the pack holds that `interface/…` file.
    pub(crate) fn ui_texture(&self, sprite_key: &str) -> Option<(dds::Bc7Shape, Arc<[u8]>)> {
        if !sprite_key.starts_with("interface\\") {
            return None;
        }
        self.serve(&format!("{sprite_key}.blp"), false)
    }

    fn serve(&self, key: &str, square: bool) -> Option<(dds::Bc7Shape, Arc<[u8]>)> {
        let rel = pack_relative(key)?;
        let entry = self.files.get(&rel)?;
        let shape = entry.shape;
        if square && (shape.width != shape.height || !shape.width.is_power_of_two()) {
            return None;
        }
        let chain = self.chain(&rel, entry)?;
        let mut served = self.served.lock().unwrap_or_else(|p| p.into_inner());
        if !served.contains_key(key) {
            let mut stats = self.stats.lock().unwrap_or_else(|p| p.into_inner());
            stats.textures_served += 1;
            stats.texture_bytes += chain.len() as u64;
            served.insert(
                key.to_string(),
                ServedTexture {
                    file: entry.path.clone(),
                    width: shape.width,
                    height: shape.height,
                },
            );
        }
        Some((shape, chain))
    }
}

impl TerrainLayerSource for PackSource {
    fn layer(&self, key: &str) -> Option<LayerOverride> {
        let (shape, chain) = self.serve(key, true)?;
        Some(LayerOverride {
            size: shape.width,
            mip_count: shape.mips,
            format: TextureFormat::Bc7RgbaUnorm,
            chain,
        })
    }

    fn packed(&self, report: &TileLayerReport) {
        let mut stats = self.stats.lock().unwrap_or_else(|p| p.into_inner());
        if report.replaced {
            stats.tiles_replaced += 1;
        } else {
            stats.tiles_stock += 1;
            if let Some(why) = &report.reason {
                if stats.reasons.len() < 16 {
                    stats.reasons.push(format!("{}: {why}", report.tile));
                }
            }
        }
        stats.layer_bytes += report.bytes as u64;
        if stats.tiles.len() < 64 {
            stats.tiles.push(format!(
                "{} {}x{}² {:?} {}KiB",
                report.tile,
                report.layers,
                report.size,
                report.format,
                report.bytes >> 10
            ));
        }
    }
}

impl WorldTextureSource for PackSource {
    fn texture(&self, key: &str) -> Option<TextureOverride> {
        let (shape, chain) = self.serve(key, false)?;
        Some(TextureOverride {
            width: shape.width,
            height: shape.height,
            mip_count: shape.mips,
            format: TextureFormat::Bc7RgbaUnorm,
            chain,
        })
    }
}

/// The selection the world is drawn with now, and how many times it has changed.
#[derive(Resource, Clone)]
pub struct ActivePack {
    /// [`VANILLA`] or a pack name.
    pub name: String,
    pub source: Option<Arc<PackSource>>,
    pub generation: u32,
}

impl ActivePack {
    pub(crate) fn vanilla() -> Self {
        Self {
            name: VANILLA.into(),
            source: None,
            generation: 0,
        }
    }
}

/// The chain cache budget, `$REDUX_TEXTURE_PACK_CACHE_MB` (default 512).
fn cache_limit() -> u64 {
    std::env::var("REDUX_TEXTURE_PACK_CACHE_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CACHE_MB)
        << 20
}

/// Open the named pack, or `Ok(None)` for vanilla. `WOW_NO_BC=1` forces vanilla.
pub(crate) fn open(catalog: &PackCatalog, name: &str) -> Result<Option<Arc<PackSource>>, String> {
    if name.eq_ignore_ascii_case(VANILLA) {
        return Ok(None);
    }
    if std::env::var("WOW_NO_BC").as_deref() == Ok("1") {
        return Err("WOW_NO_BC=1 forces vanilla textures".into());
    }
    let info = catalog
        .find(name)
        .ok_or_else(|| format!("no pack named {name} under {}", catalog.root.display()))?;
    let started = Instant::now();
    let source = PackSource::open(info, cache_limit())?;
    eprintln!(
        "TEXTURE_PACK on: {} ({} textures, scale {}) from {} in {} ms",
        info.name,
        source.texture_count(),
        info.scale,
        info.root.display(),
        started.elapsed().as_millis()
    );
    Ok(Some(Arc::new(source)))
}

/// Point both engine hooks at `source` (or back to vanilla). Already-loaded textures keep their
/// images until the world reloads ([`crate::switch`]).
pub(crate) fn install(source: Option<&Arc<PackSource>>) {
    benilla_assets::set_terrain_layer_source(
        source.map(|s| s.clone() as Arc<dyn TerrainLayerSource>),
    );
    benilla_assets::set_world_texture_source(
        source.map(|s| s.clone() as Arc<dyn WorldTextureSource>),
    );
    crate::ui::set_pack(source);
}

/// The pack to boot with: `$REDUX_TEXTURE_PACK`, else the settings file's `texture_pack` (never
/// for an unattended run), else vanilla.
pub(crate) fn startup_selection(saved: Option<&str>) -> String {
    if let Ok(name) = std::env::var("REDUX_TEXTURE_PACK") {
        if !name.is_empty() {
            return name;
        }
    }
    if crate::unattended() {
        return VANILLA.into();
    }
    saved.map_or_else(|| VANILLA.into(), str::to_string)
}

/// Discover the packs, open the startup selection and install it, before any loader runs.
pub(crate) fn boot(root: Option<PathBuf>, saved: Option<&str>) -> (PackCatalog, ActivePack) {
    let catalog = root
        .as_deref()
        .map(PackCatalog::discover)
        .unwrap_or_default();
    eprintln!(
        "TEXTURE_PACKS {} pack(s) under {}: {}",
        catalog.packs.len(),
        root.as_deref()
            .map_or_else(|| "(no packs folder)".into(), |r| r.display().to_string()),
        catalog
            .packs
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let wanted = startup_selection(saved);
    let mut active = ActivePack::vanilla();
    match open(&catalog, &wanted) {
        Ok(source) => {
            install(source.as_ref());
            if let Some(source) = source {
                active.name = source.info.name.clone();
                active.source = Some(source);
            }
        }
        Err(why) => eprintln!("TEXTURE_PACK off: {why}; vanilla textures"),
    }
    (catalog, active)
}

/// `$REDUX_TEXTURE_PROBE=<seconds>`: what the loaders took from the active pack, that long after
/// start.
pub(crate) fn texture_probe(
    time: Res<Time<Real>>,
    active: Res<ActivePack>,
    progress: Option<Res<benilla_world::terrain_stream::WorldLoadProgress>>,
    mut done: Local<bool>,
) {
    let Some(at) = std::env::var("REDUX_TEXTURE_PROBE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    else {
        return;
    };
    if *done || time.elapsed_secs() < at {
        return;
    }
    *done = true;
    report(&active, progress.as_deref());
}

/// Print the `TERRAIN_*`/`MODEL_*` summary for the active pack.
pub(crate) fn report(
    active: &ActivePack,
    progress: Option<&benilla_world::terrain_stream::WorldLoadProgress>,
) {
    if let Some(progress) = progress {
        eprintln!(
            "TERRAIN_LOAD ready={}/{} focus_resident={} scene_ready={}",
            progress.ready, progress.total, progress.focus_resident, progress.scene_ready
        );
    }
    let Some(pack) = &active.source else {
        eprintln!(
            "TEXTURE_PROBE pack=vanilla generation={}",
            active.generation
        );
        eprintln!("TERRAIN_CHECK stock");
        eprintln!("MODEL_CHECK stock");
        return;
    };
    let stats = pack.stats.lock().unwrap_or_else(|e| e.into_inner()).clone();
    eprintln!(
        "TEXTURE_PROBE pack={} generation={} tiles_bc7={} tiles_stock={} layer_array_mib={:.1} textures_served={} texture_mib={:.1} reads={} hits={} failures={} read_ms_total={:.1}",
        pack.info.name,
        active.generation,
        stats.tiles_replaced,
        stats.tiles_stock,
        stats.layer_bytes as f64 / (1u64 << 20) as f64,
        stats.textures_served,
        stats.texture_bytes as f64 / (1u64 << 20) as f64,
        pack.reads.load(Ordering::Relaxed),
        pack.hits.load(Ordering::Relaxed),
        pack.failures.load(Ordering::Relaxed),
        pack.read_micros.load(Ordering::Relaxed) as f64 / 1000.0,
    );
    for why in &stats.reasons {
        eprintln!("TERRAIN_STOCK {why}");
    }
    let terrain = stats.tiles_replaced > 0 && stats.tiles_stock == 0;
    eprintln!("TERRAIN_CHECK {}", if terrain { "pass" } else { "fail" });
    let models = stats.textures_served > 0 && pack.failures.load(Ordering::Relaxed) == 0;
    eprintln!("MODEL_CHECK {}", if models { "pass" } else { "fail" });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_keys_map_to_lowercase_pack_paths() {
        assert_eq!(
            pack_relative("tileset\\elwynn\\ElwynnGrassBase.blp").as_deref(),
            Some("tileset/elwynn/elwynngrassbase.dds")
        );
        assert_eq!(pack_relative("world\\x.tga"), None);
    }

    fn write_dds(path: &Path, w: u32, h: u32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mips = dds::full_mips(w, h);
        let mut bytes = dds::test_header(w, h, mips, 98);
        bytes.resize(dds::HEADER + dds::bc7_chain_bytes(w, h), 0);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_pack_folder_serves_terrain_and_world_art_and_remembers_it() {
        let root = std::env::temp_dir().join(format!("redux-pack-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack = root.join("upscaled");
        write_dds(&pack.join("textures/tileset/elwynn/grass.dds"), 1024, 1024);
        write_dds(&pack.join("textures/world/tree/leaves.dds"), 256, 1024);
        std::fs::write(pack.join("pack.json"), r#"{"name":"Up","scale":4}"#).unwrap();
        std::fs::create_dir_all(root.join("not-a-pack")).unwrap();

        let catalog = PackCatalog::discover(&root);
        assert_eq!(catalog.packs.len(), 1);
        let info = catalog.find("up").unwrap();
        assert_eq!(info.scale, 4);
        let source = PackSource::open(info, 64 << 20).unwrap();
        let layer = source.layer("tileset\\elwynn\\grass.blp").unwrap();
        assert_eq!(layer.size, 1024);
        // A non-square sheet is world art, never a terrain layer.
        assert!(source.layer("world\\tree\\leaves.blp").is_none());
        let art = source.texture("world\\tree\\leaves.blp").unwrap();
        assert_eq!((art.width, art.height), (256, 1024));
        assert!(source.texture("world\\tree\\missing.blp").is_none());
        let served = source.served();
        assert_eq!(served.len(), 2);
        // A UI sprite key names its `interface/` file; anything else is not UI.
        write_dds(
            &pack.join("textures/interface/buttons/ui-quickslot.dds"),
            256,
            256,
        );
        let ui_pack = PackSource::open(info, 64 << 20).unwrap();
        let (shape, _) = ui_pack
            .ui_texture("interface\\buttons\\ui-quickslot")
            .unwrap();
        assert_eq!((shape.width, shape.height), (256, 256));
        assert!(ui_pack.ui_texture("world\\tree\\leaves").is_none());
        assert!(ui_pack.ui_texture("interface\\buttons\\missing").is_none());
        std::fs::remove_file(pack.join("textures/interface/buttons/ui-quickslot.dds")).unwrap();
        assert_eq!(served["world\\tree\\leaves.blp"].height, 1024);

        // The index spares the scan and gives the same answers.
        let index = PackIndex {
            version: 1,
            files: [
                ("tileset/elwynn/grass.dds".to_string(), [1024, 1024, 11]),
                ("world/tree/leaves.dds".to_string(), [256, 1024, 11]),
            ]
            .into(),
        };
        std::fs::write(
            pack.join("index.json"),
            serde_json::to_string(&index).unwrap(),
        )
        .unwrap();
        let indexed = PackSource::open(info, 64 << 20).unwrap();
        assert_eq!(indexed.texture_count(), 2);
        assert!(indexed.texture("world\\tree\\leaves.blp").is_some());

        if std::env::var_os("REDUX_TEXTURE_PACK").is_none() && !crate::unattended() {
            assert_eq!(startup_selection(Some("Up")), "Up");
            assert_eq!(startup_selection(None), VANILLA);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
