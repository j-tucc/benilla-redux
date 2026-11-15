//! Headless capture for agents: the client renders its full frame, 3D world and UI alike, into a
//! window that is never shown, never focused and never fullscreen, and writes PNG screenshots on a
//! schedule. Someone working at the same desk sees nothing: no window, no taskbar button, no focus
//! change.
//!
//! Off unless `REDUX_HEADLESS` is set (and not `0`). Switches, all read once at startup:
//!
//! - `REDUX_SHOT_AT="<t>[,<t>…]"`: one shot per entry. `<t>` is wall seconds since launch (`5`,
//!   `12.5`) or seconds after our player first appears in the world (`world+10`).
//! - `REDUX_SHOT_EVERY=<secs>`: a shot every so many wall seconds as well.
//! - `REDUX_SHOT_DIR=<dir>`: where the PNGs go; default `<BENILLA_HOME>/Screenshots`, beside the
//!   stock `Screenshot()`'s own `WoWScrnShot_*.png`.
//! - `REDUX_SHOT_EXIT=1`: exit once every `REDUX_SHOT_AT` shot is on disk (ignored with
//!   `REDUX_SHOT_EVERY`).
//!
//! Files are `shot-<NNN>-<label>.png`, numbered in the order taken: `shot-001-t5.0s.png`,
//! `shot-002-world+10.0s.png`, `shot-003-every30.0s.png`. Each one is logged as
//! `redux-headless: wrote <path>`.
//!
//! How it stays hidden: the primary `Window` entity exists once `run_with`'s plugins have built and
//! winit creates the OS window only when the event loop starts, so [`HeadlessPlugin`] edits the
//! component in between: `visible: false` and unfocused, windowed. On Windows bevy_winit keeps
//! updating an app whose windows are all invisible (its `about_to_wait` drives the frame), wgpu
//! still acquires the hidden window's swapchain image, and bevy's `Screenshot` copies that image
//! before present, so the PNG is the real frame.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::time::Real;
use bevy::window::{PrimaryWindow, WindowMode};

/// Whether this run is headless: `REDUX_HEADLESS` set to anything but `0` or empty.
pub fn enabled() -> bool {
    std::env::var("REDUX_HEADLESS").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// What a scheduled time counts from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// Launch (the app's first frame, by the real clock).
    Launch,
    /// The first frame our own player exists in the world.
    World,
}

/// One `REDUX_SHOT_AT` entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotTime {
    pub anchor: Anchor,
    pub secs: f32,
}

impl ShotTime {
    /// The file-name label: `t5.0s` or `world+10.0s`.
    fn label(&self) -> String {
        match self.anchor {
            Anchor::Launch => format!("t{:.1}s", self.secs),
            Anchor::World => format!("world+{:.1}s", self.secs),
        }
    }
}

/// Parses `REDUX_SHOT_AT`: comma-separated `<secs>` or `world+<secs>`, whitespace ignored.
pub fn parse_shot_times(spec: &str) -> Result<Vec<ShotTime>, String> {
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| {
            let (anchor, num) = match item.strip_prefix("world+") {
                Some(rest) => (Anchor::World, rest),
                None => (Anchor::Launch, item),
            };
            let secs: f32 = num
                .trim()
                .parse()
                .map_err(|_| format!("REDUX_SHOT_AT: `{item}` is not <secs> or world+<secs>"))?;
            if !secs.is_finite() || secs < 0.0 {
                return Err(format!(
                    "REDUX_SHOT_AT: `{item}` must be a non-negative number"
                ));
            }
            Ok(ShotTime { anchor, secs })
        })
        .collect()
}

/// The schedule and its progress.
#[derive(Resource)]
struct Shots {
    dir: PathBuf,
    pending: Vec<ShotTime>,
    every: Option<f32>,
    next_every: f32,
    exit_when_done: bool,
    world_at: Option<f32>,
    taken: u32,
    /// Files asked for and not yet seen on disk.
    in_flight: Vec<PathBuf>,
    exiting: bool,
}

/// Hides the primary window and takes the scheduled screenshots; inert unless [`enabled`].
pub struct HeadlessPlugin;

impl Plugin for HeadlessPlugin {
    fn build(&self, app: &mut App) {
        if !enabled() {
            return;
        }
        hide_primary_window(app.world_mut());

        let pending = match std::env::var("REDUX_SHOT_AT") {
            Ok(spec) => parse_shot_times(&spec).unwrap_or_else(|e| {
                error!("redux-headless: {e}; no REDUX_SHOT_AT shots");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        let every = std::env::var("REDUX_SHOT_EVERY")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|s| s.is_finite() && *s > 0.0);
        let dir = std::env::var_os("REDUX_SHOT_DIR")
            .map(PathBuf::from)
            .or_else(|| benilla_app::config_dir().map(|home| home.join("Screenshots")))
            .unwrap_or_else(|| PathBuf::from("redux-shots"));
        if let Err(e) = std::fs::create_dir_all(&dir) {
            error!("redux-headless: cannot create {}: {e}", dir.display());
        }
        let exit_when_done = std::env::var("REDUX_SHOT_EXIT").is_ok_and(|v| v == "1");
        info!(
            "redux-headless: window hidden; {} scheduled shot(s){} into {}",
            pending.len(),
            every.map_or(String::new(), |s| format!(" + one every {s}s")),
            dir.display()
        );
        app.insert_resource(Shots {
            dir,
            pending,
            every,
            next_every: every.unwrap_or(0.0),
            exit_when_done: exit_when_done && every.is_none(),
            world_at: None,
            taken: 0,
            in_flight: Vec::new(),
            exiting: false,
        })
        .add_systems(Update, (keep_hidden, take_shots));
    }
}

/// Before winit creates the OS window: hidden, unfocused, windowed.
fn hide_primary_window(world: &mut World) {
    let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
    match windows.single_mut(world) {
        Ok(mut window) => {
            window.visible = false;
            window.focused = false;
            window.mode = WindowMode::Windowed;
        }
        Err(e) => error!("redux-headless: no primary window to hide ({e})"),
    }
}

/// Holds the window hidden and windowed whatever else asks: a CVar applying `gxWindow`, a probe.
/// Writes only on change, since touching `Window` re-applies its whole state through winit.
fn keep_hidden(mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    if window.visible {
        warn!("redux-headless: something made the window visible; hiding it again");
        window.visible = false;
    }
    if window.mode != WindowMode::Windowed {
        warn!(
            "redux-headless: window mode {:?} asked; staying windowed",
            window.mode
        );
        window.mode = WindowMode::Windowed;
    }
}

fn take_shots(
    mut commands: Commands,
    mut shots: ResMut<Shots>,
    time: Res<Time<Real>>,
    objects: benilla_app::Objects,
    mut exit: MessageWriter<AppExit>,
) {
    let now = time.elapsed_secs();
    // `world+` counts from the latest entry: a disconnect drops our player, and the reconnect's
    // entry restarts the clock for the shots still pending.
    match (shots.world_at, objects.player().is_some()) {
        (None, true) => {
            shots.world_at = Some(now);
            info!("redux-headless: world entry at {now:.1}s");
        }
        (Some(_), false) => {
            shots.world_at = None;
            info!("redux-headless: left the world at {now:.1}s");
        }
        _ => {}
    }

    let world_at = shots.world_at;
    let due = |t: &ShotTime| match t.anchor {
        Anchor::Launch => now >= t.secs,
        Anchor::World => world_at.is_some_and(|w| now >= w + t.secs),
    };
    let mut labels: Vec<String> = Vec::new();
    shots.pending.retain(|t| {
        if due(t) {
            labels.push(t.label());
            false
        } else {
            true
        }
    });
    if let Some(every) = shots.every {
        if now >= shots.next_every {
            labels.push(format!("every{:.1}s", shots.next_every));
            shots.next_every += every;
        }
    }
    // Two entries due the same frame would capture the same image; one file covers both.
    if let Some(label) = (!labels.is_empty()).then(|| labels.join("_")) {
        shots.taken += 1;
        let path = shots
            .dir
            .join(format!("shot-{:03}-{label}.png", shots.taken));
        info!("redux-headless: capturing {}", path.display());
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()));
        shots.in_flight.push(path);
    }

    shots.in_flight.retain(|p| {
        if p.is_file() {
            info!("redux-headless: wrote {}", p.display());
            false
        } else {
            true
        }
    });

    if shots.exit_when_done
        && !shots.exiting
        && shots.pending.is_empty()
        && shots.in_flight.is_empty()
        && shots.taken > 0
    {
        info!("redux-headless: every scheduled shot is on disk — exiting");
        shots.exiting = true;
        exit.write(AppExit::Success);
        // A teardown hang must not leave a client holding the account.
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(10));
            eprintln!("redux-headless: still alive 10s after AppExit — hard exit");
            std::process::exit(0);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_launch_and_world_times() {
        let got = parse_shot_times(" 5, world+10 ,12.5,").unwrap();
        assert_eq!(
            got,
            vec![
                ShotTime {
                    anchor: Anchor::Launch,
                    secs: 5.0
                },
                ShotTime {
                    anchor: Anchor::World,
                    secs: 10.0
                },
                ShotTime {
                    anchor: Anchor::Launch,
                    secs: 12.5
                },
            ]
        );
        assert_eq!(got[1].label(), "world+10.0s");
        assert_eq!(got[0].label(), "t5.0s");
    }

    #[test]
    fn rejects_garbage_and_negatives() {
        assert!(parse_shot_times("soon").is_err());
        assert!(parse_shot_times("world+x").is_err());
        assert!(parse_shot_times("-1").is_err());
    }
}
