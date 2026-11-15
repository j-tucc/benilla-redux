//! What the pages show and what a change writes: every value is read from, and every change
//! written to, the same TOML keys the redux crates follow (`redux-gfx.toml`, `redux-textures.toml`,
//! `redux-loot.toml`), so their live reload and their defaults keep working unchanged.
//!
//! Values cross into Lua as strings: `"1"`/`"0"` for a switch, a choice's index, a pack's name.
//! The `preset` row is derived, never stored on its own: `"0"` Classic (every effect off, MSAA),
//! `"1"` Enhanced (every effect on, temporal anti-aliasing), `"2"` Custom (a mix).
use redux_gfx::prefs::{GraphicsPrefs, AA_MSAA};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item};

/// The texture pack the game draws with no pack chosen.
pub const VANILLA: &str = redux_packs::VANILLA;

/// The redux settings files in one state folder.
#[derive(Clone, Debug)]
pub struct Files {
    pub gfx: PathBuf,
    pub textures: PathBuf,
    pub loot: PathBuf,
}

impl Files {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            gfx: redux_gfx::settings_path(state_dir),
            textures: redux_packs::settings_path(state_dir),
            loot: redux_loot::settings_path(state_dir),
        }
    }
}

/// A file's text as a TOML document; missing or unreadable is empty.
fn document(path: &Path) -> DocumentMut {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.parse::<DocumentMut>().ok())
        .unwrap_or_default()
}

fn table(path: &Path) -> toml::Table {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .unwrap_or_default()
}

fn flag(on: bool) -> String {
    if on { "1" } else { "0" }.into()
}

/// The effects alone, without the anti-aliasing the preset also decides.
fn effects(p: &GraphicsPrefs) -> GraphicsPrefs {
    GraphicsPrefs {
        aa: 0,
        dlss: 0,
        fg: false,
        ..*p
    }
}

/// The preset the switches add up to: 0 Classic, 1 Enhanced, 2 Custom. DLSS's mode and frame
/// generation never decide it; Enhanced takes any temporal anti-aliasing.
pub fn preset(p: &GraphicsPrefs) -> u8 {
    if effects(p) == GraphicsPrefs::default() && p.aa == AA_MSAA {
        0
    } else if effects(p) == effects(&GraphicsPrefs::ENHANCED) && p.temporal() {
        1
    } else {
        2
    }
}

/// Every value the pages show, by key.
pub fn values(files: &Files) -> BTreeMap<String, String> {
    let gfx = table(&files.gfx);
    let p = GraphicsPrefs::from_table(&gfx);
    let mut v = BTreeMap::new();
    v.insert("preset".into(), preset(&p).to_string());
    for key in GraphicsPrefs::KEYS {
        if let Some(n) = field(&p, key) {
            v.insert(key.into(), n.to_string());
        }
    }
    v.insert(
        "smooth_characters".into(),
        flag(
            gfx.get("smooth_characters")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false),
        ),
    );
    let tex = table(&files.textures);
    v.insert(
        "texture_pack".into(),
        tex.get("texture_pack")
            .and_then(toml::Value::as_str)
            .unwrap_or(VANILLA)
            .into(),
    );
    let tex_flag = |key: &str, default: bool| {
        flag(
            tex.get(key)
                .and_then(toml::Value::as_bool)
                .unwrap_or(default),
        )
    };
    v.insert("hd_icons".into(), tex_flag("hd_icons", false));
    v.insert("pack_ui".into(), tex_flag("pack_ui", true));
    v.insert(redux_loot::KEY.into(), flag(redux_loot::read(&files.loot)));
    v
}

/// A change from the pages: one key and its new value, or `defaults` with a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub key: String,
    pub value: String,
}

/// Parse the Lua queue's text: one `key\tvalue` per line.
pub fn parse_changes(text: &str) -> Vec<Change> {
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(k, v)| Change {
            key: k.to_string(),
            value: v.to_string(),
        })
        .collect()
}

/// One `gfx_*` key's value in `p`: 0/1 for a switch, the index for a choice.
fn field(p: &GraphicsPrefs, key: &str) -> Option<u8> {
    let on = |b: bool| Some(u8::from(b));
    match key {
        "gfx_wind" => on(p.wind),
        "gfx_fog" => on(p.fog),
        "gfx_sky" => on(p.sky),
        "gfx_grading" => on(p.grading),
        "gfx_bloom" => on(p.bloom),
        "gfx_shadows" => on(p.shadows),
        "gfx_moonlight" => on(p.moonlight),
        "gfx_ssao" => on(p.ssao),
        "gfx_mist" => on(p.mist),
        "gfx_wetness" => on(p.wetness),
        "gfx_scatter" => on(p.scatter),
        "gfx_rays" => on(p.rays),
        "gfx_firelight" => on(p.firelight),
        "gfx_glow" => on(p.glow),
        "gfx_water" => on(p.water),
        "gfx_waves" => on(p.waves),
        "gfx_fg" => on(p.fg),
        "gfx_shafts" => Some(p.shafts),
        "gfx_aa" => Some(p.aa),
        "gfx_dlss" => Some(p.dlss),
        _ => None,
    }
}

/// Whether `key` holds a number (a choice) rather than a switch.
fn is_choice(key: &str) -> bool {
    matches!(key, "gfx_shafts" | "gfx_aa" | "gfx_dlss")
}

fn write(path: &Path, doc: &DocumentMut) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, doc.to_string())
}

/// Apply one change to the files. Returns whether a file was written.
pub fn apply(files: &Files, change: &Change) -> std::io::Result<bool> {
    let key = change.key.as_str();
    let value = change.value.as_str();
    let on = value != "0" && !value.is_empty();
    match key {
        "preset" => {
            let word = match value {
                "0" => "classic",
                "1" => "enhanced",
                // Custom is what a mix reads as; choosing it keeps the mix.
                _ => return Ok(false),
            };
            let mut doc = document(&files.gfx);
            doc["preset"] = toml_edit::value(word);
            // The preset decides the effects and the anti-aliasing; DLSS's mode and frame
            // generation are kept for when DLSS is chosen again.
            for k in GraphicsPrefs::KEYS {
                if k != "gfx_dlss" && k != "gfx_fg" {
                    doc.remove(k);
                }
            }
            write(&files.gfx, &doc)?;
        }
        k if GraphicsPrefs::KEYS.contains(&k) => {
            let mut doc = document(&files.gfx);
            let n: u8 = if is_choice(k) {
                value.parse().unwrap_or(0)
            } else {
                u8::from(on)
            };
            let enhanced = doc
                .get("preset")
                .and_then(Item::as_str)
                .is_some_and(|s| s.eq_ignore_ascii_case("enhanced"));
            let base = if enhanced {
                GraphicsPrefs::ENHANCED
            } else {
                GraphicsPrefs::default()
            };
            // A value the preset already gives drops its key, so the file stays the preset's.
            if field(&base, k) == Some(n) {
                doc.remove(k);
            } else if is_choice(k) {
                doc[k] = toml_edit::value(i64::from(n));
            } else {
                doc[k] = toml_edit::value(n != 0);
            }
            write(&files.gfx, &doc)?;
        }
        "smooth_characters" => {
            let mut doc = document(&files.gfx);
            if on {
                doc[key] = toml_edit::value(true);
            } else {
                doc.remove(key);
            }
            write(&files.gfx, &doc)?;
        }
        "texture_pack" => {
            let mut doc = document(&files.textures);
            doc[key] = toml_edit::value(value);
            write(&files.textures, &doc)?;
        }
        "hd_icons" | "pack_ui" => {
            let mut doc = document(&files.textures);
            doc[key] = toml_edit::value(on);
            write(&files.textures, &doc)?;
        }
        redux_loot::KEY => {
            // Off is the absence of the key, so the file stays as small as a hand-written one.
            let mut doc = document(&files.loot);
            if on {
                doc[key] = toml_edit::value(true);
            } else {
                doc.remove(key);
            }
            write(&files.loot, &doc)?;
        }
        "defaults" => match value {
            "ReduxGraphics" => {
                let mut doc = document(&files.gfx);
                doc.remove("preset");
                doc.remove("smooth_characters");
                for k in GraphicsPrefs::KEYS {
                    doc.remove(k);
                }
                write(&files.gfx, &doc)?;
            }
            "ReduxTextures" => {
                let mut doc = document(&files.textures);
                for k in ["texture_pack", "hd_icons", "pack_ui"] {
                    doc.remove(k);
                }
                write(&files.textures, &doc)?;
            }
            _ => return Ok(false),
        },
        _ => return Ok(false),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (PathBuf, Files) {
        let dir = std::env::temp_dir().join(format!("redux-options-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let files = Files::new(&dir);
        (dir, files)
    }

    fn set(files: &Files, key: &str, value: &str) {
        apply(
            files,
            &Change {
                key: key.into(),
                value: value.into(),
            },
        )
        .unwrap();
    }

    #[test]
    fn no_files_read_as_the_code_defaults() {
        let (dir, files) = scratch("defaults");
        let v = values(&files);
        assert_eq!(v["preset"], "0");
        assert_eq!(v["gfx_bloom"], "0");
        assert_eq!(v["gfx_aa"], "0");
        assert_eq!(v["texture_pack"], VANILLA);
        assert_eq!(v["hd_icons"], "0");
        assert_eq!(v["pack_ui"], "1");
        assert_eq!(v["smooth_characters"], "0");
        assert_eq!(v["loot_at_cursor"], "0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_preset_writes_its_word_and_a_switch_makes_it_custom() {
        let (dir, files) = scratch("preset");
        std::fs::write(
            &files.gfx,
            "# mine\naa_defaults_seeded = true\ngfx_bloom = true\ngfx_dlss = 1\n",
        )
        .unwrap();
        assert_eq!(values(&files)["preset"], "2");
        set(&files, "preset", "1");
        let text = std::fs::read_to_string(&files.gfx).unwrap();
        assert!(text.contains("# mine") && text.contains("aa_defaults_seeded = true"));
        assert!(text.contains("preset = \"enhanced\"") && !text.contains("gfx_bloom"));
        assert!(text.contains("gfx_dlss = 1"));
        let v = values(&files);
        assert_eq!((v["preset"].as_str(), v["gfx_aa"].as_str()), ("1", "1"));
        assert_eq!(v["gfx_shafts"], "2");
        // One effect off under Enhanced is Custom, and only that key is written.
        set(&files, "gfx_bloom", "0");
        let text = std::fs::read_to_string(&files.gfx).unwrap();
        assert!(text.contains("gfx_bloom = false"));
        assert_eq!(values(&files)["preset"], "2");
        // Back on drops the key: the file is the preset's again.
        set(&files, "gfx_bloom", "1");
        assert!(!std::fs::read_to_string(&files.gfx)
            .unwrap()
            .contains("gfx_bloom"));
        assert_eq!(values(&files)["preset"], "1");
        // DLSS under Enhanced stays Enhanced; MSAA makes it Custom.
        set(&files, "gfx_aa", "2");
        assert_eq!(values(&files)["preset"], "1");
        set(&files, "gfx_aa", "0");
        assert_eq!(values(&files)["preset"], "2");
        set(&files, "preset", "0");
        let v = values(&files);
        assert_eq!((v["preset"].as_str(), v["gfx_wind"].as_str()), ("0", "0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn choices_and_texture_keys_round_trip() {
        let (dir, files) = scratch("choices");
        set(&files, "gfx_shafts", "1");
        set(&files, "gfx_dlss", "3");
        set(&files, "smooth_characters", "1");
        set(&files, "texture_pack", "realesrgan-x4plus-4x");
        set(&files, "hd_icons", "1");
        set(&files, "pack_ui", "0");
        let v = values(&files);
        assert_eq!(v["gfx_shafts"], "1");
        assert_eq!(v["gfx_dlss"], "3");
        assert_eq!(v["smooth_characters"], "1");
        assert_eq!(v["texture_pack"], "realesrgan-x4plus-4x");
        assert_eq!((v["hd_icons"].as_str(), v["pack_ui"].as_str()), ("1", "0"));
        // The packs crate reads the same file.
        let s = redux_packs::TextureSettings::read(Some(&dir));
        assert_eq!(s.texture_pack.as_deref(), Some("realesrgan-x4plus-4x"));
        assert_eq!((s.hd_icons, s.pack_ui), (Some(true), Some(false)));
        set(&files, "defaults", "ReduxTextures");
        set(&files, "defaults", "ReduxGraphics");
        let v = values(&files);
        assert_eq!(v["texture_pack"], VANILLA);
        assert_eq!((v["gfx_shafts"].as_str(), v["preset"].as_str()), ("0", "0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loot_at_cursor_round_trips_through_redux_loot_toml() {
        let (dir, files) = scratch("loot");
        set(&files, "loot_at_cursor", "1");
        assert_eq!(values(&files)["loot_at_cursor"], "1");
        // The loot crate reads the same file.
        assert!(redux_loot::read(&files.loot));
        set(&files, "loot_at_cursor", "0");
        assert_eq!(values(&files)["loot_at_cursor"], "0");
        assert!(!std::fs::read_to_string(&files.loot)
            .unwrap()
            .contains("loot_at_cursor"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_queue_text_parses() {
        assert_eq!(
            parse_changes("gfx_bloom\t1\ntexture_pack\tmy pack\n"),
            vec![
                Change {
                    key: "gfx_bloom".into(),
                    value: "1".into()
                },
                Change {
                    key: "texture_pack".into(),
                    value: "my pack".into()
                },
            ]
        );
    }
}
