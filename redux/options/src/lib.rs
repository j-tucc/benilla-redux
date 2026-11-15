//! benilla-redux's options pages: *Enhanced Graphics*, *Textures* and *General* (Cursor Size), a
//! `benilla-redux` group in benilla's own Options window, so every redux setting can be changed
//! in game instead of in `benilla-config/*.toml`.
//!
//! benilla's Options window keeps its categories and rows in Lua globals
//! (`BENILLA_OPTIONS_CATEGORY_KEYS`, `BENILLA_OPTIONS_PAGE_ROWS`) and builds rows from virtual
//! templates, so [`page.lua`](../src/page.lua) adds the pages from outside, with benilla's own
//! templates and row functions; no upstream file changes (REDUX.md, rules 1 and 2). It is
//! redux's own code, so it is compiled in rather than read from an addon folder, and it runs in
//! each in-game UI (a `/reload` builds a new one) as soon as the window exists.
//!
//! [`OptionsPlugin`] is the host side: it pushes the settings' current values ([`model::values`])
//! into the page, drains the page's changes into the same TOML keys the redux crates follow
//! ([`model::apply`]), and re-reads the files so an edit made outside shows too. Cursor Size goes
//! through redux-cursor's own `reduxCursorSize` CVar, which that plugin keeps.
pub mod model;

use benilla_ui::script::UiScript;
use bevy::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The page's Lua, run in each in-game UI.
const PAGE: &str = include_str!("page.lua");
/// The chunk name errors carry.
const CHUNK: &str = "@Interface\\benilla-redux\\Options.lua";
/// How often the page's change queue is drained.
const DRAIN: Duration = Duration::from_millis(100);
/// How often the files are re-read for outside edits.
const REREAD: Duration = Duration::from_millis(500);

/// Adds the pages. `state_dir` is benilla's state folder (`benilla_app::config_dir`), where the
/// settings files live; `None` adds no pages.
pub struct OptionsPlugin {
    pub state_dir: Option<PathBuf>,
}

impl Plugin for OptionsPlugin {
    fn build(&self, app: &mut App) {
        let Some(dir) = self.state_dir.as_deref() else {
            return;
        };
        app.insert_resource(Bridge {
            files: model::Files::new(dir),
            shown: BTreeMap::new(),
            installed: false,
            drained: None,
            reread: None,
            failed: false,
        })
        .add_systems(Update, bridge);
    }
}

/// The host's half of the page.
#[derive(Resource)]
struct Bridge {
    files: model::Files,
    /// The values last pushed into this VM's page.
    shown: BTreeMap<String, String>,
    /// Whether this VM's page has been built (a cache of `REDUX_OPTIONS ~= nil`).
    installed: bool,
    drained: Option<Instant>,
    reread: Option<Instant>,
    /// The page failed to build in this VM: not retried until a new one.
    failed: bool,
}

/// A Lua string literal for `s`.
fn lua_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Push every value that differs from what the page shows; true if any did.
fn push(script: &UiScript, bridge: &mut Bridge) -> bool {
    let values = model::values(&bridge.files);
    let mut chunk = String::new();
    for (k, v) in &values {
        if bridge.shown.get(k) != Some(v) {
            chunk.push_str(&format!(
                "ReduxOptions_Push({}, {})\n",
                lua_str(k),
                lua_str(v)
            ));
        }
    }
    if chunk.is_empty() {
        return false;
    }
    chunk.push_str("ReduxOptions_Refresh()\n");
    if let Err(e) = script.run(&chunk) {
        warn!("redux options: push failed: {e}");
        return false;
    }
    bridge.shown = values;
    true
}

fn bridge(
    mut bridge: ResMut<Bridge>,
    script: Option<NonSend<UiScript>>,
    catalog: Option<Res<redux_packs::PackCatalog>>,
) {
    let Some(script) = script else {
        return;
    };
    let now = Instant::now();
    if bridge
        .drained
        .is_some_and(|t| now.duration_since(t) < DRAIN)
    {
        return;
    }
    bridge.drained = Some(now);

    // A new VM (login, `/reload`) has no page: build it once the window exists.
    let state: Option<(bool, bool)> = script
        .eval("return BenillaOptionsFrame_SelectCategory ~= nil, REDUX_OPTIONS ~= nil")
        .ok();
    let Some((window, page)) = state else {
        return;
    };
    if !window {
        bridge.installed = false;
        bridge.failed = false;
        return;
    }
    if !page {
        if bridge.installed {
            // The page was there and is gone: a new VM.
            bridge.installed = false;
            bridge.failed = false;
        }
        if bridge.failed {
            return;
        }
        if let Err(e) = script.run_chunk_named(PAGE.as_bytes(), CHUNK) {
            error!("redux options: the page did not build: {e}");
            bridge.failed = true;
            return;
        }
        let mut names = vec![model::VANILLA.to_string()];
        if let Some(catalog) = catalog {
            names.extend(catalog.packs.iter().map(|p| p.name.clone()));
        }
        let list: Vec<String> = names.iter().map(|n| lua_str(n)).collect();
        if let Err(e) = script.run(&format!("ReduxOptions_SetPacks({{{}}})", list.join(", "))) {
            warn!("redux options: the pack list did not load: {e}");
        }
        bridge.installed = true;
        bridge.shown.clear();
        push(&script, &mut bridge);
        bridge.reread = Some(now);
        info!("redux options: pages built");
        return;
    }
    bridge.installed = true;

    let mut wrote = false;
    match script.eval::<Option<String>>("return ReduxOptions_TakeWrites()") {
        Ok(Some(text)) => {
            for change in model::parse_changes(&text) {
                match model::apply(&bridge.files, &change) {
                    Ok(true) => {
                        info!("redux options: {} = {}", change.key, change.value);
                        wrote = true;
                    }
                    Ok(false) => {}
                    Err(e) => warn!("redux options: {} not saved: {e}", change.key),
                }
            }
            // A no-op choice (Custom) still redraws, so the row shows what the files say.
            bridge.shown.clear();
        }
        Ok(None) => {}
        Err(e) => warn!("redux options: reading changes failed: {e}"),
    }
    let due = bridge
        .reread
        .is_none_or(|t| now.duration_since(t) >= REREAD);
    if wrote || due || bridge.shown.is_empty() {
        bridge.reread = Some(now);
        push(&script, &mut bridge);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lua_strings_escape() {
        assert_eq!(super::lua_str(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
