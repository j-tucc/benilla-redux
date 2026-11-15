//! Loot Window At Cursor, as WoW Forever's Controls option: when on, the loot window opens with
//! its first row under the mouse, kept on the screen; when off (the default), at the stock 1.12
//! place, the left panel slot.
//!
//! 1.12 has no such option (`lootUnderMouse` is a later client's CVar, absent from 1.12's CVar
//! table), so the hook is redux's own Lua ([`loot.lua`](../src/loot.lua)), compiled in and run in
//! each in-game UI once the stock `LootFrame` exists: it wraps the global `LootFrame_OnEvent` the
//! frame's `OnEvent` calls by name. No upstream file changes (REDUX.md, rules 1 and 2).
//!
//! The setting is redux's own `redux-loot.toml` in benilla's state folder (`loot_at_cursor =
//! true`), never a row in upstream's CVar table. [`LootPlugin`] re-reads it every half second
//! and keeps the Lua global `REDUX_LOOT_AT_CURSOR` (`"1"`/`"0"`) current, so the Options page
//! (`redux/options`) or a hand edit applies at the next loot.
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The hook's Lua, run in each in-game UI.
const HOOK: &str = include_str!("loot.lua");
/// The chunk name errors carry.
const CHUNK: &str = "@Interface\\benilla-redux\\Loot.lua";
/// How often the VM and the file are looked at.
const POLL: Duration = Duration::from_millis(500);
/// The settings file's key.
pub const KEY: &str = "loot_at_cursor";

/// The settings file in a state folder.
pub fn settings_path(state_dir: &Path) -> PathBuf {
    state_dir.join("redux-loot.toml")
}

/// Whether the loot window opens at the cursor: the file's `loot_at_cursor`, off when the file
/// or the key is missing or unreadable.
pub fn read(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| t.get(KEY).and_then(toml::Value::as_bool))
        .unwrap_or(false)
}

/// Adds the hook. `state_dir` is benilla's state folder (`benilla_app::config_dir`); `None`
/// adds nothing.
pub struct LootPlugin {
    pub state_dir: Option<PathBuf>,
}

impl Plugin for LootPlugin {
    fn build(&self, app: &mut App) {
        let Some(dir) = self.state_dir.as_deref() else {
            return;
        };
        app.insert_resource(Hook {
            file: settings_path(dir),
            polled: None,
            failed: false,
        })
        .add_systems(Update, hook);
    }
}

#[derive(Resource)]
struct Hook {
    file: PathBuf,
    polled: Option<Instant>,
    /// The hook failed to install in this VM: not retried until a new one.
    failed: bool,
}

fn hook(mut hook: ResMut<Hook>, script: Option<NonSend<UiScript>>) {
    let Some(script) = script else {
        return;
    };
    let now = Instant::now();
    if hook.polled.is_some_and(|t| now.duration_since(t) < POLL) {
        return;
    }
    hook.polled = Some(now);

    // The glue UI has no LootFrame; a new in-game VM (login, `/reload`) has no hook yet.
    let Ok((frame, installed)) =
        script.eval::<(bool, bool)>("return LootFrame_OnEvent ~= nil, REDUX_LOOT ~= nil")
    else {
        return;
    };
    if !frame {
        hook.failed = false;
        return;
    }
    if !installed {
        if hook.failed {
            return;
        }
        if let Err(e) = script.run_chunk_named(HOOK.as_bytes(), CHUNK) {
            error!("redux loot: the hook did not install: {e}");
            hook.failed = true;
            return;
        }
        info!("redux loot: hook installed");
    }
    let on = if read(&hook.file) { "1" } else { "0" };
    if let Err(e) = script.run(&format!("REDUX_LOOT_AT_CURSOR = \"{on}\"")) {
        warn!("redux loot: the setting did not reach the UI: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_reads_off_unless_set() {
        let dir = std::env::temp_dir().join(format!("redux-loot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = settings_path(&dir);
        assert!(!read(&path));
        std::fs::write(&path, "loot_at_cursor = true\n").unwrap();
        assert!(read(&path));
        std::fs::write(&path, "loot_at_cursor = false\n").unwrap();
        assert!(!read(&path));
        std::fs::write(&path, "not toml [").unwrap();
        assert!(!read(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The hook in a bare UI VM, over stand-ins for the stock globals it wraps.
    fn vm() -> UiScript {
        let s = UiScript::new().unwrap();
        s.run(
            "UISpecialFrames = { 'ItemRefTooltip' }
             STOCK = {}
             function LootFrame_OnEvent(event) table.insert(STOCK, event) end
             function GetLeftFrame() return nil end
             function GetCenterFrame() return nil end",
        )
        .unwrap();
        s.run_chunk_named(HOOK.as_bytes(), CHUNK).unwrap();
        s
    }

    #[test]
    fn the_first_row_lands_on_the_cursor_and_the_window_stays_on_screen() {
        let s = vm();
        // A 1024x768 screen, a 186x256 drawn window, the first row 42.5,-98.5 from its top left.
        let place = |cx: f64, cy: f64| -> (f64, f64) {
            s.eval(&format!(
                "return ReduxLoot_Place({cx}, {cy}, 1024, 768, 186, 256, 42.5, -98.5)"
            ))
            .unwrap()
        };
        assert_eq!(place(500.0, 400.0), (457.5, 498.5));
        // Clamped at each edge.
        assert_eq!(place(10.0, 400.0).0, 0.0);
        assert_eq!(place(1020.0, 400.0).0, 1024.0 - 186.0);
        assert_eq!(place(500.0, 760.0).1, 768.0);
        assert_eq!(place(500.0, 20.0).1, 256.0);
    }

    #[test]
    fn off_or_for_other_events_the_stock_handler_runs() {
        let s = vm();
        s.run(
            "this = LootFrame
             REDUX_LOOT_AT_CURSOR = '0'
             LootFrame_OnEvent('LOOT_OPENED')
             REDUX_LOOT_AT_CURSOR = '1'
             LootFrame_OnEvent('LOOT_CLOSED')",
        )
        .unwrap();
        let stock: (String, String, i64) = s
            .eval("return STOCK[1], STOCK[2], table.getn(UISpecialFrames)")
            .unwrap();
        assert_eq!(stock, ("LOOT_OPENED".into(), "LOOT_CLOSED".into(), 2));
    }
}
