//! The Cursor Size choice: WoW Forever's Options → General list (Default, 32x32 … 128x128), kept in
//! redux's own `redux-cursor.toml` in benilla's state folder, never in upstream's CVar table
//! (REDUX.md, rule 1).
//!
//! - `redux-cursor.toml`: `size = 64` (or `"64x64"`), `size = "default"` / `0` for Default. Re-read
//!   when it changes, so an edit applies while the client runs.
//! - `$REDUX_CURSOR_SIZE`: the same words, for one session; never saved.
//! - The `reduxCursorSize` CVar, the hook an options addon calls: it registers the CVar and sets
//!   it (`RegisterCVar("reduxCursorSize", "0")`, `SetCVar("reduxCursorSize", "64")`); redux adopts
//!   each new value and saves it to the file. See `redux/cursor/README.md`.
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The file in benilla's state folder.
pub const FILE_NAME: &str = "redux-cursor.toml";
/// The key in it.
pub const KEY: &str = "size";
/// The CVar an options addon registers and sets.
pub const CVAR: &str = "reduxCursorSize";
/// The sizes Forever's dropdown offers after Default, in its order.
pub const CHOICES: [u32; 5] = [32, 48, 64, 96, 128];
/// The smallest and largest size taken from the file or the env.
pub const MIN: u32 = 16;
pub const MAX: u32 = 256;

/// The player's choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorSize {
    /// The vanilla 32×32 at the display's scale: 32 px at 100%, 64 at 200%.
    #[default]
    Default,
    /// This many physical pixels on a side.
    Pixels(u32),
}

impl CursorSize {
    /// `default`/`0`/empty, `64`, `64x64`; any case, spaces ignored. `None` when unreadable.
    pub fn parse(word: &str) -> Option<CursorSize> {
        let w = word.trim().to_ascii_lowercase().replace(' ', "");
        if w.is_empty() || w == "default" || w == "0" {
            return Some(CursorSize::Default);
        }
        let side = match w.split_once('x') {
            Some((a, b)) if a == b => a,
            Some(_) => return None,
            None => w.as_str(),
        };
        let n: u32 = side.parse().ok()?;
        Some(CursorSize::Pixels(n.clamp(MIN, MAX)))
    }

    /// The word the CVar and the file carry: `0` for Default, else the side.
    pub fn word(self) -> String {
        match self {
            CursorSize::Default => "0".into(),
            CursorSize::Pixels(n) => n.to_string(),
        }
    }

    /// The dropdown's label: `Default` or `64x64`.
    pub fn label(self) -> String {
        match self {
            CursorSize::Default => "Default".into(),
            CursorSize::Pixels(n) => format!("{n}x{n}"),
        }
    }

    /// The cursor's side in physical pixels on a display at `scale` (the OS's DPI factor).
    pub fn pixels(self, scale: f32) -> u32 {
        match self {
            CursorSize::Default => {
                let s = if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                };
                ((32.0 * s).round() as u32).clamp(MIN, MAX)
            }
            CursorSize::Pixels(n) => n,
        }
    }

    /// Read from a `redux-cursor.toml` table: an integer or a string under [`KEY`]. Default when
    /// absent or unreadable.
    pub fn from_table(table: &toml::Table) -> CursorSize {
        match table.get(KEY) {
            Some(toml::Value::Integer(i)) => {
                CursorSize::parse(&i.max(&0).to_string()).unwrap_or_default()
            }
            Some(toml::Value::String(s)) => CursorSize::parse(s).unwrap_or_default(),
            _ => CursorSize::Default,
        }
    }
}

/// The file's path in a state folder.
pub fn settings_path(state_dir: &Path) -> PathBuf {
    state_dir.join(FILE_NAME)
}

/// The file's table, empty when missing or unreadable.
fn read_table(path: &Path) -> toml::Table {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.parse::<toml::Table>().ok())
        .unwrap_or_default()
}

/// Read the saved choice.
pub fn load(path: &Path) -> CursorSize {
    CursorSize::from_table(&read_table(path))
}

/// Save `size`, keeping any other keys the file holds.
pub fn save(path: &Path, size: CursorSize) -> std::io::Result<()> {
    let mut table = read_table(path);
    let value = match size {
        CursorSize::Default => toml::Value::String("default".into()),
        CursorSize::Pixels(n) => toml::Value::Integer(i64::from(n)),
    };
    table.insert(KEY.into(), value);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = toml::to_string(&table).map_err(std::io::Error::other)?;
    std::fs::write(
        path,
        format!("# benilla-redux cursor size: \"default\", or 32, 48, 64, 96, 128 (px)\n{body}"),
    )
}

/// The file's modified time, `None` when missing.
pub fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_forever_and_the_file_use_parse() {
        assert_eq!(CursorSize::parse("Default"), Some(CursorSize::Default));
        assert_eq!(CursorSize::parse("0"), Some(CursorSize::Default));
        assert_eq!(CursorSize::parse(""), Some(CursorSize::Default));
        assert_eq!(CursorSize::parse("64x64"), Some(CursorSize::Pixels(64)));
        assert_eq!(CursorSize::parse(" 96 X 96 "), Some(CursorSize::Pixels(96)));
        assert_eq!(CursorSize::parse("128"), Some(CursorSize::Pixels(128)));
        assert_eq!(CursorSize::parse("4000"), Some(CursorSize::Pixels(MAX)));
        assert_eq!(CursorSize::parse("32x64"), None);
        assert_eq!(CursorSize::parse("big"), None);
    }

    #[test]
    fn default_follows_the_display_scale() {
        assert_eq!(CursorSize::Default.pixels(1.0), 32);
        assert_eq!(CursorSize::Default.pixels(1.5), 48);
        assert_eq!(CursorSize::Default.pixels(2.0), 64);
        assert_eq!(CursorSize::Default.pixels(f32::NAN), 32);
        assert_eq!(CursorSize::Pixels(96).pixels(2.0), 96);
    }

    #[test]
    fn every_choice_round_trips_through_word_and_label() {
        for n in CHOICES {
            let s = CursorSize::Pixels(n);
            assert_eq!(CursorSize::parse(&s.word()), Some(s));
            assert_eq!(CursorSize::parse(&s.label()), Some(s));
        }
        assert_eq!(
            CursorSize::parse(&CursorSize::Default.label()),
            Some(CursorSize::Default)
        );
    }

    #[test]
    fn save_keeps_other_keys_and_load_reads_it_back() {
        let dir = std::env::temp_dir().join(format!("redux-cursor-test-{}", std::process::id()));
        let path = settings_path(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "other = 1\n").unwrap();
        save(&path, CursorSize::Pixels(64)).unwrap();
        assert_eq!(load(&path), CursorSize::Pixels(64));
        assert!(read_table(&path).contains_key("other"));
        save(&path, CursorSize::Default).unwrap();
        assert_eq!(load(&path), CursorSize::Default);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
