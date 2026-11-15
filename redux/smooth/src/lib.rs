//! Smoother character models: one level of PN-triangle refinement over every skinned part of a
//! `Character\` model (players and humanoid NPCs), ported from Elderoth's
//! `benilla-adapter/src/smooth` without its chest rig, which is Elderoth's own.
//!
//! benilla spawns each body part with its skinned mesh (`Mesh3d`), a `RigPart` and a
//! `WorldObject` naming the model path. This crate refines that mesh once per source mesh on the
//! CPU from its main-world copy ([`refine`]), caches it, and swaps the part's `Mesh3d` to it; a
//! redress that points the part at another mesh is refined in turn, and turning the option off
//! puts every stock mesh back. Original vertices never move, so the silhouette, face and texture
//! mapping stay the model's. The picker keeps testing the stock geometry (`PickMesh`).
//!
//! Off by default: `smooth_characters = true` in `benilla-config/redux-gfx.toml` (followed while
//! the client runs), or `REDUX_SMOOTH=1|0` for one session.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use benilla_assets::{ATTRIBUTE_WOW_JOINT_INDEX, ATTRIBUTE_WOW_JOINT_WEIGHT};
use benilla_world::interact::WorldObject;
use benilla_world::rig_palette::RigPart;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;

pub mod refine;

use refine::SkinMesh;

/// The model folder whose skinned parts are smoothed.
const CHARACTER_PREFIX: &str = "character\\";
/// How often the settings file is looked at.
const POLL: Duration = Duration::from_secs(1);

/// Whether character models draw smoothed.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SmoothCharacters(pub bool);

/// A skinned Bevy mesh's streams, or `None` for a layout this pass does not know.
pub fn read_mesh(mesh: &Mesh) -> Option<SkinMesh> {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return None;
    }
    let known = [
        Mesh::ATTRIBUTE_POSITION.id,
        Mesh::ATTRIBUTE_NORMAL.id,
        Mesh::ATTRIBUTE_UV_0.id,
        Mesh::ATTRIBUTE_UV_1.id,
        Mesh::ATTRIBUTE_COLOR.id,
        ATTRIBUTE_WOW_JOINT_INDEX.id,
        ATTRIBUTE_WOW_JOINT_WEIGHT.id,
    ];
    if mesh.attributes().any(|(a, _)| !known.contains(&a.id)) {
        return None;
    }
    let f3 = |id| match mesh.attribute(id)? {
        VertexAttributeValues::Float32x3(v) => Some(v.clone()),
        _ => None,
    };
    let f2 = |id| match mesh.attribute(id)? {
        VertexAttributeValues::Float32x2(v) => Some(v.clone()),
        _ => None,
    };
    let m = SkinMesh {
        positions: f3(Mesh::ATTRIBUTE_POSITION)?,
        normals: f3(Mesh::ATTRIBUTE_NORMAL)?,
        uv0: f2(Mesh::ATTRIBUTE_UV_0)?,
        uv1: match mesh.attribute(Mesh::ATTRIBUTE_UV_1) {
            None => None,
            Some(_) => Some(f2(Mesh::ATTRIBUTE_UV_1)?),
        },
        colors: match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
            None => None,
            Some(VertexAttributeValues::Float32x4(v)) => Some(v.clone()),
            Some(_) => return None,
        },
        joints: match mesh.attribute(ATTRIBUTE_WOW_JOINT_INDEX)? {
            VertexAttributeValues::Uint16x4(v) => v.clone(),
            _ => return None,
        },
        weights: match mesh.attribute(ATTRIBUTE_WOW_JOINT_WEIGHT)? {
            VertexAttributeValues::Float32x4(v) => v.clone(),
            _ => return None,
        },
        indices: match mesh.indices()? {
            Indices::U32(i) => i.clone(),
            Indices::U16(i) => i.iter().map(|&x| x as u32).collect(),
        },
    };
    m.valid().then_some(m)
}

/// The streams back as a skinned mesh with a main-world copy, as benilla builds them
/// (`benilla_assets::submesh_to_skinned_mesh`), so the CPU picker can still read it.
pub fn write_mesh(m: SkinMesh) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, m.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, m.uv0);
    if let Some(uv1) = m.uv1 {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1);
    }
    mesh.insert_indices(Indices::U32(m.indices));
    if let Some(c) = m.colors {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, c);
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals);
    mesh.insert_attribute(
        ATTRIBUTE_WOW_JOINT_INDEX,
        VertexAttributeValues::Uint16x4(m.joints),
    );
    mesh.insert_attribute(
        ATTRIBUTE_WOW_JOINT_WEIGHT,
        VertexAttributeValues::Float32x4(m.weights),
    );
    mesh
}

/// One built mesh and its triangle counts.
#[derive(Clone)]
struct Built {
    handle: Handle<Mesh>,
    source_triangles: usize,
    triangles: usize,
}

/// The refined meshes by source mesh (`None`: a layout this pass leaves stock), the reverse map
/// for turning the option off, and the build time spent, for the probe.
#[derive(Resource, Default)]
pub struct SmoothMeshes {
    built: HashMap<AssetId<Mesh>, Option<Built>>,
    stock_of: HashMap<AssetId<Mesh>, Handle<Mesh>>,
    pub build_ms: f64,
    pub builds: usize,
    pub swaps: usize,
}

impl SmoothMeshes {
    /// The smooth mesh for `source`, built on first use and then cached; `None` keeps the source.
    fn get(&mut self, meshes: &mut Assets<Mesh>, source: &Handle<Mesh>) -> Option<Built> {
        if let Some(b) = self.built.get(&source.id()) {
            return b.clone();
        }
        let start = Instant::now();
        let built = meshes.get(source).and_then(read_mesh).map(|m| {
            let source_triangles = m.triangles();
            let out = refine::subdivide(&m);
            let triangles = out.triangles();
            Built {
                handle: meshes.add(write_mesh(out)),
                source_triangles,
                triangles,
            }
        });
        self.build_ms += start.elapsed().as_secs_f64() * 1e3;
        self.builds += 1;
        if let Some(b) = &built {
            self.stock_of.insert(b.handle.id(), source.clone());
        }
        self.built.insert(source.id(), built.clone());
        built
    }
}

/// Whether a part's model is a character model.
fn is_character(object: &WorldObject) -> bool {
    object
        .label
        .get(..CHARACTER_PREFIX.len())
        .is_some_and(|p| p.replace('/', "\\").eq_ignore_ascii_case(CHARACTER_PREFIX))
}

/// Keeps every skinned character part on the option's mesh: a newly dressed or redressed part
/// (its `Mesh3d` changed since last run), and every part when the option changes.
fn swap(
    option: Res<SmoothCharacters>,
    mut cache: ResMut<SmoothMeshes>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut parts: Query<(&mut Mesh3d, &WorldObject), With<RigPart>>,
) {
    let all = option.is_changed();
    for (mut mesh, object) in &mut parts {
        if !(all || mesh.is_changed()) || !is_character(object) {
            continue;
        }
        let current = mesh.0.id();
        if option.0 {
            if cache.stock_of.contains_key(&current) {
                continue;
            }
            if let Some(b) = cache.get(&mut meshes, &mesh.0) {
                mesh.0 = b.handle;
                cache.swaps += 1;
            }
        } else if let Some(stock) = cache.stock_of.get(&current).cloned() {
            mesh.0 = stock;
            cache.swaps += 1;
        }
    }
}

/// `REDUX_SMOOTH`, when it names a choice.
fn forced() -> Option<bool> {
    match std::env::var("REDUX_SMOOTH").ok()?.trim() {
        "1" | "on" | "true" => Some(true),
        "0" | "off" | "false" => Some(false),
        _ => None,
    }
}

/// `smooth_characters` in a `redux-gfx.toml` text; absent or unreadable is off.
fn from_text(text: &str) -> bool {
    text.parse::<toml::Table>()
        .ok()
        .and_then(|t| t.get("smooth_characters").and_then(toml::Value::as_bool))
        .unwrap_or(false)
}

/// The settings file and when it last changed.
#[derive(Resource)]
struct Follower {
    path: Option<PathBuf>,
    modified: Option<Option<SystemTime>>,
    last_poll: Option<Instant>,
}

/// Once a second, the settings file's `smooth_characters`, unless `REDUX_SMOOTH` pins it.
fn follow(mut follower: ResMut<Follower>, mut option: ResMut<SmoothCharacters>) {
    if follower.last_poll.is_some_and(|t| t.elapsed() < POLL) {
        return;
    }
    follower.last_poll = Some(Instant::now());
    let want = match forced() {
        Some(f) => f,
        None => {
            let Some(path) = follower.path.clone() else {
                return;
            };
            let now = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            if follower.modified == Some(now) {
                return;
            }
            follower.modified = Some(now);
            std::fs::read_to_string(&path).is_ok_and(|t| from_text(&t))
        }
    };
    if option.0 != want {
        info!("redux smooth characters: {want}");
        option.0 = want;
    }
}

/// `REDUX_SMOOTH_PROBE=<seconds>`: how many parts were smoothed and what it cost, at that time.
fn probe(
    time: Res<Time<Real>>,
    cache: Res<SmoothMeshes>,
    option: Res<SmoothCharacters>,
    parts: Query<(&Mesh3d, &WorldObject), With<RigPart>>,
    mut done: Local<bool>,
) {
    let Some(at) = std::env::var("REDUX_SMOOTH_PROBE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    else {
        return;
    };
    if *done || time.elapsed_secs() < at {
        return;
    }
    *done = true;
    let (mut characters, mut smooth) = (0, 0);
    for (mesh, object) in &parts {
        if is_character(object) {
            characters += 1;
            smooth += usize::from(cache.stock_of.contains_key(&mesh.0.id()));
        }
    }
    let (stock_tris, smooth_tris) = cache.built.values().flatten().fold((0, 0), |(a, b), x| {
        (a + x.source_triangles, b + x.triangles)
    });
    eprintln!(
        "SMOOTH_PROBE on={} character_parts={characters} smoothed={smooth} meshes_built={} \
         unknown_layout={} triangles {stock_tris}->{smooth_tris} build_ms={:.1} swaps={}",
        option.0,
        cache.built.values().filter(|b| b.is_some()).count(),
        cache.built.values().filter(|b| b.is_none()).count(),
        cache.build_ms,
        cache.swaps
    );
}

/// Smoothed character models, following `redux-gfx.toml` in `config_dir`.
pub struct SmoothPlugin {
    pub config_dir: Option<PathBuf>,
}

impl Plugin for SmoothPlugin {
    fn build(&self, app: &mut App) {
        let path = self.config_dir.as_deref().map(settings_path);
        app.init_resource::<SmoothCharacters>()
            .init_resource::<SmoothMeshes>()
            .insert_resource(Follower {
                path,
                modified: None,
                last_poll: None,
            })
            .add_systems(PreUpdate, follow)
            .add_systems(PostUpdate, (swap, probe).chain());
    }
}

/// The file the switch lives in, shared with `redux-gfx`.
pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("redux-gfx.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> SkinMesh {
        SkinMesh {
            positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]],
            normals: vec![[0., 0., 1.]; 4],
            uv0: vec![[0., 0.], [1., 0.], [0., 1.], [1., 1.]],
            uv1: None,
            colors: None,
            joints: vec![[1, 0, 0, 0]; 4],
            weights: vec![[1., 0., 0., 0.]; 4],
            indices: vec![0, 1, 2, 2, 1, 3],
        }
    }

    #[test]
    fn a_mesh_round_trips_through_bevy() {
        let m = refine::subdivide(&quad());
        assert_eq!(m.triangles(), 8);
        let back = read_mesh(&write_mesh(m.clone())).expect("known layout");
        assert_eq!(back, m);
    }

    #[test]
    fn an_unknown_layout_stays_stock() {
        let mut mesh = write_mesh(quad());
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, vec![[0f32, 0., 0., 1.]; 4]);
        assert!(read_mesh(&mesh).is_none());
    }

    #[test]
    fn only_character_models_smooth() {
        let object = |label: &str| WorldObject {
            kind: benilla_world::model_render::ModelKind::Creature,
            label: label.into(),
            id: 0,
            detail: String::new(),
        };
        assert!(is_character(&object(
            "Character\\Human\\Male\\HumanMale.mdx"
        )));
        assert!(is_character(&object("character/orc/female/orcfemale.m2")));
        assert!(!is_character(&object("Creature\\Kobold\\Kobold.mdx")));
        assert!(!is_character(&object("")));
    }

    #[test]
    fn the_switch_reads_from_the_shared_file() {
        assert!(from_text("smooth_characters = true\ngfx_wind = true\n"));
        assert!(!from_text("preset = \"enhanced\"\n"));
        assert!(!from_text("not toml ="));
    }
}
