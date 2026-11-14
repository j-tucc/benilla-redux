//! Switching the texture pack while the client runs, when `redux-textures.toml`'s `texture_pack`
//! changes. Ported from Elderoth's `benilla-adapter/src/pack_switch.rs`, without its frozen-frame
//! cover and actor rebuild, which are Elderoth's own UI and scene:
//!
//! 1. **Swap**: install the new source into benilla's hooks, then reload every live `mpq` image
//!    in place, so images still held (models, sky) read the new pack.
//! 2. **Rebuild**: once those reloads finished, re-prepare every world material against the new
//!    images (a material's bind group holds the image it was prepared with) and ask benilla for a
//!    same-map [`WorldReload`], which streams the terrain, the static pass and the liquids again.
//! 3. **Settle**: when the world is presentable and no image is loading, the switch is logged.
//!
//! Presentation only: nothing reaches the wire or benilla's settings.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use bevy::asset::{AssetEvent, LoadState};
use bevy::prelude::*;

use benilla_assets::materials::{LiquidMaterial, TerrainMaterial, WdlMaterial, WowModelMaterial};
use benilla_world::terrain_stream::WorldLoadProgress;
use benilla_world::world_map::WorldReload;

use crate::catalog::{self, ActivePack, PackCatalog, VANILLA};
use crate::TextureSettings;

/// Follows the settings file and drives a switch.
pub(crate) struct PackSwitchPlugin;

impl Plugin for PackSwitchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PackSwitch>().add_systems(
            Update,
            (follow_settings, switch_probe, drive_switch).chain(),
        );
    }
}

/// How long the world must stay ready before a switch counts as done.
const SETTLE: Duration = Duration::from_millis(1200);
/// A switch that never becomes ready ends anyway after this long, logged as a timeout.
const TIMEOUT: Duration = Duration::from_secs(120);
/// How often the settings file is looked at.
const POLL: Duration = Duration::from_secs(1);

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    /// A switch was asked for; swap on the next frame.
    Swap,
    /// Live images are reloading from the new source.
    Reloading,
    /// The world is rebuilding.
    Rebuilding,
}

/// The switch state.
#[derive(Resource, Default)]
pub struct PackSwitch {
    phase: Phase,
    target: Option<String>,
    started: Option<Instant>,
    phase_frames: u32,
    ready_since: Option<Instant>,
    pending: HashSet<AssetId<Image>>,
}

impl PackSwitch {
    /// Ask for a switch to `name`; ignored while one is running.
    pub fn request(&mut self, name: &str) {
        if self.phase == Phase::Idle {
            self.target = Some(name.to_string());
            self.started = Some(Instant::now());
            self.enter(Phase::Swap);
        }
    }

    /// A switch is underway.
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        self.phase_frames = 0;
    }
}

/// The settings file and when it last changed.
#[derive(Resource)]
pub(crate) struct SettingsFollower {
    dir: Option<PathBuf>,
    modified: Option<SystemTime>,
    last_poll: Option<Instant>,
}

impl SettingsFollower {
    pub(crate) fn new(dir: Option<&Path>) -> Self {
        let modified = dir.and_then(|d| modified(&crate::settings_path(d)));
        Self {
            dir: dir.map(Path::to_path_buf),
            modified,
            last_poll: None,
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Once a second: when the settings file changed and names another pack than the one drawn,
/// switch to it (a missing `texture_pack` is vanilla). `$REDUX_TEXTURE_PACK` pins the pack for the
/// session, so the file is not followed then.
fn follow_settings(
    mut follower: ResMut<SettingsFollower>,
    active: Res<ActivePack>,
    mut switch: ResMut<PackSwitch>,
) {
    let Some(dir) = follower.dir.clone() else {
        return;
    };
    if follower.last_poll.is_some_and(|t| t.elapsed() < POLL) {
        return;
    }
    follower.last_poll = Some(Instant::now());
    let now = modified(&crate::settings_path(&dir));
    if now == follower.modified {
        return;
    }
    follower.modified = now;
    if std::env::var("REDUX_TEXTURE_PACK").is_ok_and(|v| !v.is_empty()) {
        return;
    }
    let want = TextureSettings::read(Some(&dir))
        .texture_pack
        .unwrap_or_else(|| VANILLA.into());
    if !want.eq_ignore_ascii_case(&active.name) {
        info!("PACK_SWITCH settings ask for {want}");
        switch.request(&want);
    }
}

/// Every world material type whose bind groups hold pack images.
type WorldMaterials<'w> = (
    ResMut<'w, Assets<WowModelMaterial>>,
    ResMut<'w, Assets<TerrainMaterial>>,
    ResMut<'w, Assets<LiquidMaterial>>,
    ResMut<'w, Assets<WdlMaterial>>,
);

/// The switch state machine: swap, reload, rebuild, settle.
#[allow(clippy::too_many_arguments)]
fn drive_switch(
    mut switch: ResMut<PackSwitch>,
    catalog: Res<PackCatalog>,
    mut active: ResMut<ActivePack>,
    images: Res<Assets<Image>>,
    mut image_events: MessageReader<AssetEvent<Image>>,
    asset_server: Res<AssetServer>,
    mut reload: MessageWriter<WorldReload>,
    progress: Option<Res<WorldLoadProgress>>,
    mut materials: WorldMaterials,
) {
    switch.phase_frames += 1;
    let elapsed = switch.started.map_or(Duration::ZERO, |s| s.elapsed());
    match switch.phase {
        Phase::Idle => {
            image_events.clear();
        }
        Phase::Swap => {
            let target = switch.target.clone().unwrap_or_else(|| VANILLA.into());
            match catalog::open(&catalog, &target) {
                Ok(source) => {
                    catalog::install(source.as_ref());
                    active.name = source
                        .as_ref()
                        .map_or_else(|| VANILLA.to_string(), |s| s.info.name.clone());
                    active.source = source;
                    active.generation += 1;
                }
                Err(why) => {
                    warn!("PACK_SWITCH {why}");
                    finish(&mut switch, &active, false);
                    return;
                }
            }
            // Every image still alive reads again through the new source, in place.
            let mut pending = HashSet::new();
            for id in images.ids() {
                let Some(path) = asset_server.get_path(id) else {
                    continue;
                };
                let source = path.source().as_str();
                if source != Some(benilla_assets::MPQ_SOURCE)
                    && source != Some(benilla_assets::MPQ_LOOSE_SOURCE)
                {
                    continue;
                }
                asset_server.reload(path.into_owned());
                pending.insert(id);
            }
            info!(
                "PACK_SWITCH {} reloading {} live textures",
                active.name,
                pending.len()
            );
            image_events.clear();
            switch.pending = pending;
            switch.enter(Phase::Reloading);
        }
        Phase::Reloading => {
            for event in image_events.read() {
                if let AssetEvent::Modified { id } | AssetEvent::LoadedWithDependencies { id } =
                    event
                {
                    switch.pending.remove(id);
                }
            }
            let mut pending = std::mem::take(&mut switch.pending);
            pending.retain(|id| {
                images.contains(*id)
                    && !matches!(asset_server.load_state(*id), LoadState::Failed(_))
            });
            switch.pending = pending;
            if !switch.pending.is_empty() && elapsed < TIMEOUT {
                return;
            }
            // Touching every material re-prepares its bind group against the reloaded images.
            let (models, terrain, liquid, wdl) = &mut materials;
            let touched = models.iter_mut().count()
                + terrain.iter_mut().count()
                + liquid.iter_mut().count()
                + wdl.iter_mut().count();
            info!("PACK_SWITCH re-preparing {touched} materials, reloading the world");
            reload.write(WorldReload);
            switch.ready_since = None;
            switch.enter(Phase::Rebuilding);
        }
        Phase::Rebuilding => {
            image_events.clear();
            // Past the engine's reload hold before the gate means anything.
            if switch.phase_frames < 8 {
                return;
            }
            let world = progress
                .as_ref()
                .is_none_or(|p| p.presentable() && p.focus_resident);
            let loading = images.ids().any(|id| {
                matches!(
                    asset_server.load_state(id),
                    LoadState::Loading | LoadState::NotLoaded
                ) && asset_server.get_path(id).is_some()
            });
            let ready = world && !loading;
            if !ready {
                switch.ready_since = None;
                if elapsed < TIMEOUT {
                    return;
                }
                warn!("PACK_SWITCH timed out: world={world} loading={loading}");
            } else {
                let since = *switch.ready_since.get_or_insert_with(Instant::now);
                if since.elapsed() < SETTLE {
                    return;
                }
            }
            finish(&mut switch, &active, ready);
        }
    }
}

/// End the switch with one line.
fn finish(switch: &mut PackSwitch, active: &ActivePack, ready: bool) {
    let ms = switch.started.map_or(0, |s| s.elapsed().as_millis());
    eprintln!(
        "PACK_SWITCH to={} generation={} ms={ms} ready={ready}",
        active.name, active.generation
    );
    switch.target = None;
    switch.started = None;
    switch.pending.clear();
    switch.enter(Phase::Idle);
}

/// `$REDUX_SWITCH_PROBE=a,b,...`: switch to each named selection in turn, starting at
/// `$REDUX_SWITCH_AT` seconds (default 20) and 10 s after each switch ends, reporting the pack's
/// numbers after each.
fn switch_probe(
    time: Res<Time<Real>>,
    mut switch: ResMut<PackSwitch>,
    active: Res<ActivePack>,
    progress: Option<Res<WorldLoadProgress>>,
    mut state: Local<SwitchProbe>,
) {
    if state.plan.is_none() {
        let plan = std::env::var("REDUX_SWITCH_PROBE").unwrap_or_default();
        state.plan = Some(
            plan.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        );
        state.next_at = std::env::var("REDUX_SWITCH_AT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20.0);
    }
    let now = time.elapsed_secs();
    if switch.busy() {
        state.was_busy = true;
        return;
    }
    if state.was_busy {
        state.was_busy = false;
        state.next_at = now + 10.0;
        catalog::report(&active, progress.as_deref());
    }
    if now < state.next_at {
        return;
    }
    let Some(plan) = state.plan.as_mut() else {
        return;
    };
    if plan.is_empty() {
        return;
    }
    let next = plan.remove(0);
    eprintln!("SWITCH_PROBE requesting {next} at {now:.1}s");
    switch.request(&next);
}

#[derive(Default)]
struct SwitchProbe {
    plan: Option<Vec<String>>,
    next_at: f32,
    was_busy: bool,
}
