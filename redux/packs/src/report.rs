//! F8: the texture bug reporter, in a dev build (`reporter` feature). It captures a clean frame,
//! lists every texture drawn in view (model parts, terrain layers, the retained static pass),
//! lets the player pick one and add a note, and saves the report in the state folder:
//! `<state>/texture-reports/<UTC stamp>_<slug>/{report.json, screenshot.png}` plus a line in
//! `texture-reports/reports.jsonl`. Ported from Elderoth's `benilla-adapter/src/texture_report`,
//! without its character-skin listing (skins stay stock here). Never writes into the install.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use benilla_assets::materials::{TerrainMaterial, WowModelMaterial};
use benilla_assets::AdtTile;
use benilla_world::interact::WorldObject;
use benilla_world::static_gx::StaticGx;
use benilla_world::view::WorldCamera;
use bevy::camera::primitives::{Aabb, Frustum};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy_egui::{egui, EguiContexts};
use serde::Serialize;

use crate::catalog::{ActivePack, VANILLA};
use crate::switch::PackSwitch;
use crate::TextureBoot;

/// The reporter's key, scene listing and window, and its probe when asked for.
pub(crate) struct TextureReportPlugin;

impl Plugin for TextureReportPlugin {
    fn build(&self, app: &mut App) {
        // benilla's dev build adds egui for its own panels, which also hand egui's keyboard focus
        // to the game's input; add it only when nothing has.
        if !app.is_plugin_added::<bevy_egui::EguiPlugin>() {
            app.add_plugins(bevy_egui::EguiPlugin::default());
        }
        app.init_resource::<Reporter>()
            .add_systems(Update, (reporter_input, collect_on_open).chain())
            .add_systems(bevy_egui::EguiPrimaryContextPass, reporter_window);
        if std::env::var_os("REDUX_REPORT_PROBE").is_some() {
            app.add_systems(PreUpdate, report_probe.after(bevy::input::InputSystems));
        }
    }
}

/// Where a listed texture comes from.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Origin {
    /// `vanilla` or the pack name.
    pub source: String,
    /// The pack file, when a pack supplies it.
    pub pack_file: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// One listed texture.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Row {
    /// The normalized install path, `world\azeroth\...\x.blp`.
    pub texture: String,
    /// What draws it: terrain, model, static.
    pub kinds: BTreeSet<&'static str>,
    /// The models or tiles using it in view.
    pub users: BTreeSet<String>,
    /// Instances in view.
    pub count: usize,
    pub origin: Origin,
}

/// The reporter's window state.
#[derive(Resource, Default)]
pub(crate) struct Reporter {
    pub open: bool,
    /// Frames since F8, for the clean capture before the window draws.
    opening: Option<u32>,
    rows: Vec<Row>,
    selected: Option<usize>,
    filter: String,
    note: String,
    status: Option<String>,
    shot: Option<PathBuf>,
    camera: Option<Transform>,
}

/// The install path key for a texture name: lowercase, backslashes, `.blp`, no sampler marker.
pub(crate) fn texture_key(name: &str) -> String {
    let lower = name.to_ascii_lowercase().replace('/', "\\");
    let stem = match lower.rsplit_once('.') {
        Some((stem, ext)) if ext.len() == 3 => stem.to_string(),
        _ => lower,
    };
    // `texture_url`'s `@rr`/`@cc...` marker before the extension.
    let stem = match stem.rsplit_once('@') {
        Some((base, tag)) if tag.len() <= 3 && tag.chars().all(|c| "rct".contains(c)) => {
            base.to_string()
        }
        _ => stem,
    };
    format!("{stem}.blp")
}

/// The reports folder in the state folder.
fn reports_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("texture-reports")
}

/// F8 toggles; the first frame captures, the second collects and opens. A run that persists
/// nothing (no state folder) has nowhere to save, so F8 does nothing there.
fn reporter_input(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut reporter: ResMut<Reporter>,
    switch: Res<PackSwitch>,
    boot: Res<TextureBoot>,
    camera: Query<&Transform, With<WorldCamera>>,
) {
    if !keys.just_pressed(KeyCode::F8) || switch.busy() {
        return;
    }
    let Some(config_dir) = boot.config_dir.as_deref() else {
        return;
    };
    if reporter.open || reporter.opening.is_some() {
        reporter.open = false;
        reporter.opening = None;
        return;
    }
    // Capture before the window exists, so the picture shows only the world.
    let dir = reports_dir(config_dir);
    let _ = std::fs::create_dir_all(&dir);
    let shot = dir.join(".pending-screenshot.png");
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(shot.clone()));
    reporter.shot = Some(shot);
    reporter.camera = camera.iter().next().copied();
    reporter.opening = Some(0);
    reporter.status = None;
}

/// What the collector reads, grouped to stay under the system parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
struct Scene<'w, 's> {
    models: Query<
        'w,
        's,
        (
            Entity,
            &'static MeshMaterial3d<WowModelMaterial>,
            &'static ViewVisibility,
        ),
    >,
    terrain: Query<
        'w,
        's,
        (
            &'static MeshMaterial3d<TerrainMaterial>,
            &'static ViewVisibility,
            &'static Aabb,
            &'static GlobalTransform,
        ),
    >,
    parents: Query<'w, 's, &'static ChildOf>,
    objects: Query<'w, 's, &'static WorldObject>,
    frustum: Query<'w, 's, &'static Frustum, With<WorldCamera>>,
    model_materials: Res<'w, Assets<WowModelMaterial>>,
    terrain_materials: Res<'w, Assets<TerrainMaterial>>,
    tiles: Res<'w, Assets<AdtTile>>,
    static_gx: Option<Res<'w, StaticGx>>,
    world_assets: Option<Res<'w, benilla_assets::WorldAssets>>,
    asset_server: Res<'w, AssetServer>,
}

/// The second frame after F8: list what is in view and open the window.
fn collect_on_open(mut reporter: ResMut<Reporter>, scene: Scene, active: Res<ActivePack>) {
    let Some(frames) = reporter.opening else {
        return;
    };
    if frames < 2 {
        reporter.opening = Some(frames + 1);
        return;
    }
    reporter.opening = None;
    reporter.rows = collect(&scene, &active);
    reporter.selected = None;
    reporter.note.clear();
    reporter.open = true;
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for row in &reporter.rows {
        for kind in &row.kinds {
            *by_kind.entry(kind).or_default() += 1;
        }
    }
    eprintln!(
        "REPORT_LIST textures={} by_kind={by_kind:?} pack={}",
        reporter.rows.len(),
        active.name
    );
}

/// Every texture drawn in view, one row per texture.
fn collect(scene: &Scene, active: &ActivePack) -> Vec<Row> {
    let mut rows: HashMap<String, (BTreeSet<&'static str>, BTreeSet<String>, usize)> =
        HashMap::new();
    let mut add = |name: &str, kind: &'static str, user: String| {
        let entry = rows.entry(texture_key(name)).or_default();
        entry.0.insert(kind);
        if entry.1.len() < 12 {
            entry.1.insert(user);
        }
        entry.2 += 1;
    };

    // Images made without a path (clutter, sky) by their WorldAssets key.
    let mut unnamed: HashMap<AssetId<Image>, String> = HashMap::new();
    if let Some(world) = &scene.world_assets {
        for ((key, _, _), handle) in world.textures.iter() {
            unnamed.insert(handle.id(), key.clone());
        }
    }
    let name_of = |id: AssetId<Image>| -> Option<String> {
        if let Some(path) = scene.asset_server.get_path(id) {
            return Some(path.path().to_string_lossy().into_owned());
        }
        unnamed.get(&id).cloned()
    };

    // Model entities: doodads, WMO groups, clutter, units. A composited character skin has no
    // name and is left out.
    for (entity, material, visible) in &scene.models {
        if !visible.get() {
            continue;
        }
        let Some(material) = scene.model_materials.get(&material.0) else {
            continue;
        };
        let user = owner(scene, entity);
        let handles = [
            material.base.base_color_texture.as_ref(),
            material.extension.stage1_texture.as_ref(),
        ];
        for handle in handles.into_iter().flatten() {
            if let Some(name) = name_of(handle.id()) {
                add(&name, "model", user.clone());
            }
        }
    }

    // Terrain: the layers of the chunks inside each visible cell.
    let mut tile_of: HashMap<AssetId<Image>, AssetId<AdtTile>> = HashMap::new();
    for (id, tile) in scene.tiles.iter() {
        tile_of.insert(tile.layer_array.id(), id);
    }
    for (material, visible, aabb, transform) in &scene.terrain {
        if !visible.get() {
            continue;
        }
        let Some(material) = scene.terrain_materials.get(&material.0) else {
            continue;
        };
        let Some(tile) = tile_of
            .get(&material.extension.layer_array.id())
            .and_then(|id| scene.tiles.get(*id))
        else {
            continue;
        };
        let center = transform.transform_point(Vec3::from(aabb.center));
        let half = Vec3::from(aabb.half_extents) * transform.compute_transform().scale;
        for chunk in &tile.chunks {
            let Some(first) = chunk.positions.first() else {
                continue;
            };
            let at = benilla_assets::coords::wow_to_bevy(*first);
            if (at.x - center.x).abs() > half.x + 34.0 || (at.z - center.z).abs() > half.z + 34.0 {
                continue;
            }
            for name in chunk.layer_textures.iter().take(4) {
                add(name, "terrain", "terrain".into());
            }
        }
    }

    // The retained static pass: batches whose world box is in the frustum.
    if let (Some(gx), Some(frustum)) = (&scene.static_gx, scene.frustum.iter().next()) {
        for item in gx.items() {
            let (Some(name), Some((min, max))) = (item.texture, item.bounds) else {
                continue;
            };
            let aabb = Aabb::from_min_max(min, max);
            if frustum.intersects_obb(&aabb, &bevy::math::Affine3A::IDENTITY, true, true) {
                add(name, "static", item.label.to_string());
            }
        }
    }

    let served = active
        .source
        .as_ref()
        .map(|s| s.served())
        .unwrap_or_default();
    let mut out: Vec<Row> = rows
        .into_iter()
        .map(|(texture, (kinds, users, count))| {
            let pack = served
                .get(&texture)
                .cloned()
                .or_else(|| active.source.as_ref().and_then(|s| s.file_for(&texture)));
            let origin = match pack {
                Some(p) => Origin {
                    source: active.name.clone(),
                    pack_file: Some(p.file.to_string_lossy().into_owned()),
                    width: Some(p.width),
                    height: Some(p.height),
                },
                None => Origin {
                    source: VANILLA.into(),
                    pack_file: None,
                    width: None,
                    height: None,
                },
            };
            Row {
                texture,
                kinds,
                users,
                count,
                origin,
            }
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then(a.texture.cmp(&b.texture)));
    out
}

/// The nearest model label at or above `entity`.
fn owner(scene: &Scene, entity: Entity) -> String {
    let mut at = entity;
    for _ in 0..16 {
        if let Ok(object) = scene.objects.get(at) {
            if !object.label.is_empty() {
                return object.label.clone();
            }
        }
        match scene.parents.get(at) {
            Ok(parent) => at = parent.parent(),
            Err(_) => break,
        }
    }
    "model".into()
}

/// The reporter window.
fn reporter_window(
    mut contexts: EguiContexts,
    mut reporter: ResMut<Reporter>,
    active: Res<ActivePack>,
    boot: Res<TextureBoot>,
) -> bevy::ecs::error::Result {
    if !reporter.open {
        return Ok(());
    }
    let mut open = true;
    let mut save = false;
    let rep = &mut *reporter;
    egui::Window::new("Report a texture")
        .open(&mut open)
        .default_size([760.0, 520.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(contexts.ctx_mut()?, |ui| {
            ui.label(format!(
                "{} textures in view - pack: {}",
                rep.rows.len(),
                active.name
            ));
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.text_edit_singleline(&mut rep.filter);
            });
            ui.separator();
            let filter = rep.filter.to_ascii_lowercase();
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (i, row) in rep.rows.iter().enumerate() {
                        let users = row.users.iter().cloned().collect::<Vec<_>>().join(", ");
                        if !filter.is_empty()
                            && !row.texture.contains(&filter)
                            && !users.to_ascii_lowercase().contains(&filter)
                        {
                            continue;
                        }
                        let size = match (row.origin.width, row.origin.height) {
                            (Some(w), Some(h)) => format!("{w}x{h}"),
                            _ => "stock".into(),
                        };
                        let text = format!(
                            "{:>3}x  {}  [{} | {}]",
                            row.count, row.texture, row.origin.source, size
                        );
                        let selected = rep.selected == Some(i);
                        if ui
                            .selectable_label(selected, text)
                            .on_hover_text(format!(
                                "{}\nUsed by: {users}",
                                row.kinds.iter().copied().collect::<Vec<_>>().join(", ")
                            ))
                            .clicked()
                        {
                            rep.selected = Some(i);
                        }
                    }
                });
            ui.separator();
            match rep.selected.and_then(|i| rep.rows.get(i)) {
                Some(row) => {
                    ui.strong(&row.texture);
                    ui.label(format!(
                        "From: {}{}",
                        row.origin.source,
                        row.origin
                            .pack_file
                            .as_ref()
                            .map(|f| format!(" ({f})"))
                            .unwrap_or_default()
                    ));
                    ui.label(format!(
                        "Used by: {}",
                        row.users.iter().cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
                None => {
                    ui.weak("Pick the texture that looks wrong.");
                }
            }
            ui.label("Note:");
            ui.add(
                egui::TextEdit::multiline(&mut rep.note)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(rep.selected.is_some(), egui::Button::new("Save report"))
                    .clicked()
                {
                    save = true;
                }
                if let Some(status) = &rep.status {
                    ui.label(status);
                }
            });
        });
    if save {
        let status = match boot.config_dir.as_deref() {
            Some(dir) => match save_report(rep, &active, dir) {
                Ok(dir) => format!("Saved to {}", dir.display()),
                Err(e) => format!("Save failed: {e}"),
            },
            None => "Save failed: no state folder".into(),
        };
        eprintln!("REPORT_SAVED {status}");
        rep.status = Some(status);
        rep.note.clear();
    }
    if !open {
        rep.open = false;
    }
    Ok(())
}

/// A saved report, as written to `report.json` and `reports.jsonl`.
#[derive(Serialize)]
struct Report<'a> {
    created_utc: String,
    texture: &'a str,
    note: &'a str,
    pack: &'a str,
    pack_generation: u32,
    origin: &'a Origin,
    kinds: Vec<&'a str>,
    users: Vec<&'a str>,
    count: usize,
    camera_translation: Option<[f32; 3]>,
    camera_rotation: Option<[f32; 4]>,
    screenshot: Option<String>,
    pack_file_sha256: Option<String>,
}

/// Writes the selected texture's report (and the screenshot) under the reports folder.
fn save_report(rep: &Reporter, active: &ActivePack, config_dir: &Path) -> Result<PathBuf, String> {
    use std::io::Write;
    let err = |e: std::io::Error| e.to_string();
    let row = rep
        .selected
        .and_then(|i| rep.rows.get(i))
        .ok_or("no texture selected")?;
    let stamp = utc_stamp(std::time::SystemTime::now());
    let slug: String = row
        .texture
        .rsplit('\\')
        .next()
        .unwrap_or("texture")
        .trim_end_matches(".blp")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let root = reports_dir(config_dir);
    let dir = root.join(format!("{stamp}_{slug}"));
    std::fs::create_dir_all(&dir).map_err(err)?;
    let mut screenshot = None;
    if let Some(shot) = &rep.shot {
        if shot.is_file() {
            std::fs::copy(shot, dir.join("screenshot.png")).map_err(err)?;
            screenshot = Some("screenshot.png".to_string());
        }
    }
    let sha = row
        .origin
        .pack_file
        .as_ref()
        .and_then(|f| std::fs::read(f).ok())
        .map(|bytes| sha256_hex(&bytes));
    let report = Report {
        created_utc: stamp,
        texture: &row.texture,
        note: rep.note.trim(),
        pack: &active.name,
        pack_generation: active.generation,
        origin: &row.origin,
        kinds: row.kinds.iter().copied().collect(),
        users: row.users.iter().map(String::as_str).collect(),
        count: row.count,
        camera_translation: rep.camera.map(|c| c.translation.to_array()),
        camera_rotation: rep.camera.map(|c| c.rotation.to_array()),
        screenshot,
        pack_file_sha256: sha,
    };
    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("report.json"), json).map_err(err)?;
    let mut index = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("reports.jsonl"))
        .map_err(err)?;
    let mut line = serde_json::to_value(&report).map_err(|e| e.to_string())?;
    line["dir"] = serde_json::Value::String(dir.to_string_lossy().into_owned());
    writeln!(index, "{line}").map_err(err)?;
    Ok(dir)
}

/// `YYYYMMDDTHHMMSSZ` for a system time, from the civil calendar (no date crate).
fn utc_stamp(at: std::time::SystemTime) -> String {
    let secs = at
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// SHA-256 of `bytes` as lowercase hex (FIPS 180-4), so a report pins the exact pack file.
fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = bytes.to_vec();
    let bits = (bytes.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (a, b) in h.iter_mut().zip(v) {
            *a = a.wrapping_add(b);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// `REDUX_REPORT_PROBE=<seconds>`: open the reporter then, list what is in view, and save a report
/// on the most used texture with a probe note.
fn report_probe(
    time: Res<Time<Real>>,
    mut reporter: ResMut<Reporter>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    active: Res<ActivePack>,
    boot: Res<TextureBoot>,
    mut step: Local<u8>,
) {
    let at = std::env::var("REDUX_REPORT_PROBE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(30.0);
    match *step {
        0 if time.elapsed_secs() >= at => {
            keys.press(KeyCode::F8);
            *step = 1;
        }
        1 => {
            keys.release(KeyCode::F8);
            if reporter.open {
                *step = 2;
            }
        }
        2 if !reporter.rows.is_empty() => {
            reporter.selected = Some(0);
            reporter.note = "probe: automated report".into();
            let status = match boot.config_dir.as_deref() {
                Some(dir) => match save_report(&reporter, &active, dir) {
                    Ok(dir) => format!("pass {}", dir.display()),
                    Err(e) => format!("fail {e}"),
                },
                None => "fail no state folder".into(),
            };
            eprintln!("REPORT_CHECK {status}");
            reporter.open = false;
            *step = 3;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::pack_relative;

    #[test]
    fn texture_names_normalize_to_install_keys() {
        assert_eq!(
            texture_key("World/Azeroth/Elwynn/Tree@cc.blp"),
            "world\\azeroth\\elwynn\\tree.blp"
        );
        assert_eq!(
            texture_key("Tileset\\Elwynn\\Grass.blp"),
            "tileset\\elwynn\\grass.blp"
        );
        assert_eq!(
            texture_key("Character\\Human\\Skin"),
            "character\\human\\skin.blp"
        );
        assert_eq!(texture_key("x\\a.tga"), "x\\a.blp");
        assert_eq!(texture_key("x\\a@cct.blp"), "x\\a.blp");
        assert_eq!(
            pack_relative(&texture_key("x\\a@rc.blp")).as_deref(),
            Some("x/a.dds")
        );
    }

    #[test]
    fn stamps_are_utc_civil_dates() {
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_789_000_000);
        assert_eq!(utc_stamp(at), "20260910T002640Z");
        assert_eq!(utc_stamp(std::time::UNIX_EPOCH), "19700101T000000Z");
    }

    #[test]
    fn sha256_matches_known_digests() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_report_saves_with_its_origin() {
        let dir = std::env::temp_dir().join(format!("redux-report-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rep = Reporter {
            rows: vec![Row {
                texture: "world\\x.blp".into(),
                kinds: ["model"].into(),
                users: ["tree.m2".to_string()].into(),
                count: 3,
                origin: Origin {
                    source: "Up".into(),
                    pack_file: None,
                    width: Some(1024),
                    height: Some(1024),
                },
            }],
            selected: Some(0),
            note: "seam ".into(),
            ..Default::default()
        };
        let active = ActivePack::vanilla();
        let saved = save_report(&rep, &active, &dir).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(saved.join("report.json")).unwrap())
                .unwrap();
        assert_eq!(json["origin"]["width"], 1024);
        assert_eq!(json["texture"], "world\\x.blp");
        assert_eq!(json["note"], "seam");
        let index = std::fs::read_to_string(dir.join("texture-reports/reports.jsonl")).unwrap();
        assert_eq!(index.lines().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
