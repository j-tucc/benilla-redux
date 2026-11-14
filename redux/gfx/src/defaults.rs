//! redux's anti-aliasing and filtering defaults, Elderoth's: 8x MSAA and 16x anisotropic
//! filtering, where benilla boots at 1.12's stock (no multisampling, anisotropy off).
//!
//! They are benilla's own CVars (`gxMultisample`, `anisotropic`), so the player's Video options
//! still show and change them. On the first launch of a state folder redux writes the two values
//! into `config.toml` once, and records that it did in `redux-gfx.toml`; from then on the file is
//! the player's, and a choice of no multisampling sticks. A run with no state folder (a capture)
//! takes them for the session through `$WOW_MSAA`/`$WOW_ANISO` instead, which never persist.
//!
//! Anisotropy reaches the samplers only through `$WOW_ANISO`: benilla freezes its texture-filter
//! policy (`benilla_assets::tex_filter`) the first time a sampler asks, and the static pass's
//! samplers ask at `RenderStartup`, before the CVar load publishes `config.toml`'s value, so the
//! saved `anisotropic` never applies (an upstream ordering bug, seen live: the CVar log says mode
//! 5, the samplers run mode 4). redux exports the file's value as `$WOW_ANISO` for the session;
//! a change in the options still applies only after a restart, as the reference's UI says, but is
//! not saved while the variable is set, so edit `config.toml` (or set `$WOW_ANISO`) to change it.
//!
//! Fallbacks are benilla's: `MsaaSetting` is clamped to the GPU's supported sample counts at
//! `finish`, and an anisotropy clamp applies only where wgpu allows it (every filter linear). An
//! explicit `$WOW_MSAA`/`$WOW_ANISO` always wins.
use std::collections::BTreeMap;
use std::path::Path;

/// The defaults: (`config.toml` CVar, session variable, value).
const DEFAULTS: [(&str, &str, &str); 2] = [
    ("gxMultisample", "WOW_MSAA", "8"),
    ("anisotropic", "WOW_ANISO", "16"),
];

/// `redux-gfx.toml`'s key recording that the defaults were written once.
const SEEDED: &str = "aa_defaults_seeded";

/// Applies the defaults before the app is built (the engine reads both settings while it builds).
/// `config_dir` is benilla's state folder, `None` when the run persists nothing.
pub fn session_defaults(config_dir: Option<&Path>) {
    let Some(dir) = config_dir else {
        for (_, var, value) in DEFAULTS {
            if std::env::var_os(var).is_none() {
                std::env::set_var(var, value);
            }
        }
        return;
    };
    if let Err(e) = seed_once(dir) {
        eprintln!("redux graphics: could not write the MSAA/anisotropy defaults: {e}");
    }
    if std::env::var_os("WOW_ANISO").is_none() {
        let text = std::fs::read_to_string(dir.join("config.toml")).unwrap_or_default();
        if let Some(aniso) = saved_cvar(&text, "anisotropic") {
            std::env::set_var("WOW_ANISO", aniso);
        }
    }
}

/// A CVar's saved value in `config.toml`'s text, by case-insensitive name.
fn saved_cvar(text: &str, name: &str) -> Option<String> {
    let file: toml::Table = text.parse().ok()?;
    let cvars = file.get("cvars")?.as_table()?;
    cvars
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.as_str().map(str::to_owned))
}

/// Writes the defaults into `dir/config.toml` unless `dir/redux-gfx.toml` says it was done, then
/// records it there.
fn seed_once(dir: &Path) -> std::io::Result<()> {
    let marker_path = crate::settings_path(dir);
    let mut marker = read_table(&marker_path);
    if marker.get(SEEDED).and_then(toml::Value::as_bool) == Some(true) {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let config_path = dir.join("config.toml");
    let text = std::fs::read_to_string(&config_path).unwrap_or_default();
    if let Some(seeded) = seed(&text) {
        std::fs::write(&config_path, seeded)?;
    }
    marker.insert(SEEDED.into(), toml::Value::Boolean(true));
    std::fs::write(
        &marker_path,
        toml::to_string(&marker).map_err(std::io::Error::other)?,
    )
}

fn read_table(path: &Path) -> toml::Table {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .unwrap_or_default()
}

/// `config.toml`'s text with every default the player has not set added to its `[cvars]` table,
/// or `None` when nothing changes or the file does not parse (it is then left alone).
fn seed(text: &str) -> Option<String> {
    let mut file: toml::Table = text.parse().ok()?;
    let cvars = file
        .entry("cvars")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()?;
    let mut changed = false;
    for (name, _, value) in DEFAULTS {
        // CVar names are case-insensitive, as every lookup the client makes.
        if !cvars.keys().any(|k| k.eq_ignore_ascii_case(name)) {
            cvars.insert(name.into(), toml::Value::String(value.into()));
            changed = true;
        }
    }
    // benilla saves `[cvars]` sorted; keep that order.
    let sorted: BTreeMap<_, _> = std::mem::take(cvars).into_iter().collect();
    cvars.extend(sorted);
    changed.then(|| toml::to_string(&file).ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_takes_both_defaults() {
        let out: toml::Table = seed("").unwrap().parse().unwrap();
        let cvars = out["cvars"].as_table().unwrap();
        assert_eq!(cvars["gxMultisample"].as_str(), Some("8"));
        assert_eq!(cvars["anisotropic"].as_str(), Some("16"));
    }

    #[test]
    fn a_players_choice_is_kept() {
        let out = seed("[cvars]\ngxmultisample = \"1\"\nMasterVolume = \"0.5\"\n").unwrap();
        let out: toml::Table = out.parse().unwrap();
        let cvars = out["cvars"].as_table().unwrap();
        assert_eq!(cvars["gxmultisample"].as_str(), Some("1"));
        assert!(!cvars.contains_key("gxMultisample"));
        assert_eq!(cvars["anisotropic"].as_str(), Some("16"));
        assert_eq!(cvars["MasterVolume"].as_str(), Some("0.5"));
    }

    #[test]
    fn nothing_to_add_or_a_broken_file_changes_nothing() {
        assert_eq!(
            seed("[cvars]\ngxMultisample = \"4\"\nanisotropic = \"2\"\n"),
            None
        );
        assert_eq!(seed("not = [toml"), None);
    }

    #[test]
    fn a_saved_cvar_is_found_by_any_spelling() {
        let text = "[cvars]
Anisotropic = \"8\"
";
        assert_eq!(saved_cvar(text, "anisotropic").as_deref(), Some("8"));
        assert_eq!(saved_cvar(text, "gxMultisample"), None);
    }

    #[test]
    fn the_defaults_are_written_once() {
        let dir = std::env::temp_dir().join(format!("redux-gfx-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        seed_once(&dir).unwrap();
        // The player turns multisampling off; benilla's diff save drops the default row.
        std::fs::write(dir.join("config.toml"), "[cvars]\nanisotropic = \"16\"\n").unwrap();
        seed_once(&dir).unwrap();
        let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(!text.contains("gxMultisample"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
