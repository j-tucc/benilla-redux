//! forever-art: art exported from the user's own WoW Forever install (by wow.export) made into
//! textures benilla's 1.12 UI loads — each atlas region its own power-of-two TGA — plus the Lua
//! table that lets the benilla-forever addon look regions up by their modern atlas names.
//! It reads and writes only local, gitignored folders; nothing it touches is committed.

pub mod image;
pub mod tables;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use image::{Rgba, MAX_SIDE};
use tables::{Atlas, Member};

/// What to pull: atlas names (exact, or a `prefix*`) and whole texture files.
#[derive(Debug, Default, PartialEq)]
pub struct Manifest {
    pub atlases: Vec<String>,
    pub files: Vec<String>,
}

impl Manifest {
    /// Lines `atlas <name|prefix*>` and `file <path.blp>`; `#` starts a comment.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut m = Manifest::default();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (kind, value) = line
                .split_once(char::is_whitespace)
                .ok_or_else(|| format!("line {}: expected `atlas` or `file` and a value", n + 1))?;
            let value = value.trim().to_lowercase().replace('\\', "/");
            match kind {
                "atlas" => m.atlases.push(value),
                "file" => m.files.push(value),
                _ => return Err(format!("line {}: unknown kind {kind:?}", n + 1)),
            }
        }
        Ok(m)
    }

    fn wants(&self, name: &str) -> bool {
        self.atlases.iter().any(|p| match p.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => name == p,
        })
    }

    /// The members the manifest names, one per name: where a name has several rows, the first
    /// whose sheet is in `have` (if given) wins.
    pub fn select<'m>(
        &self,
        members: &'m [Member],
        atlases: &HashMap<u32, Atlas>,
        have: Option<&HashMap<u32, String>>,
    ) -> Vec<&'m Member> {
        let mut by_name: BTreeMap<&str, &Member> = BTreeMap::new();
        for m in members.iter().filter(|m| self.wants(&m.name)) {
            let Some(atlas) = atlases.get(&m.atlas_id) else {
                continue;
            };
            let usable = have.is_none_or(|h| h.contains_key(&atlas.file_data_id));
            if usable {
                by_name.entry(&m.name).or_insert(m);
            }
        }
        by_name.into_values().collect()
    }
}

/// The wow.export job that exports every sheet the manifest's atlases live on, plus its files.
pub fn job_json(
    manifest: &Manifest,
    members: &[Member],
    atlases: &HashMap<u32, Atlas>,
    install: &str,
    product: &str,
    out_dir: &str,
    log: &str,
) -> String {
    let fids: std::collections::BTreeSet<u32> = manifest
        .select(members, atlases, None)
        .iter()
        .map(|m| atlases[&m.atlas_id].file_data_id)
        .collect();
    let mut files: Vec<String> = fids.iter().map(|f| format!("{{\"fid\":{f}}}")).collect();
    files.extend(
        manifest
            .files
            .iter()
            .map(|n| format!("{{\"name\":{}}}", json_str(n))),
    );
    format!(
        "{{\"install\":{},\"product\":{},\"outDir\":{},\"log\":{},\"format\":\"png\",\"files\":[{}]}}\n",
        json_str(install),
        json_str(product),
        json_str(out_dir),
        json_str(log),
        files.join(",")
    )
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A file-name-safe form of an atlas name.
pub fn file_stem(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// One texture the addon can draw: its in-game path and the texcoords of the art inside it.
#[derive(Debug, PartialEq)]
pub struct Entry {
    pub texture: String,
    pub width: u32,
    pub height: u32,
    pub right: f64,
    pub bottom: f64,
}

/// Crop `region`'s art (already shrunk to fit) onto a power-of-two canvas, write it, and say
/// where in the canvas it sits.
fn emit(art: &Rgba, dir: &Path, stem: &str, game_dir: &str) -> std::io::Result<(String, f64, f64)> {
    let padded = art.pad_pot();
    padded.write_tga(&dir.join(format!("{stem}.tga")))?;
    Ok((
        format!("{game_dir}\\{stem}"),
        art.width as f64 / padded.width as f64,
        art.height as f64 / padded.height as f64,
    ))
}

/// Where a build reads from and writes to.
pub struct Build<'a> {
    /// wow.export's output folder (`<outDir>/interface/...png`).
    pub extract: &'a Path,
    /// The addon folder; art goes in `Art/`, the table in `Atlas.lua`.
    pub addon: &'a Path,
    /// The addon's folder name as the game sees it, e.g. `benilla-forever`.
    pub addon_name: &'a str,
}

/// What a build wrote.
#[derive(Debug, Default)]
pub struct Report {
    pub atlases: BTreeMap<String, Entry>,
    pub textures: BTreeMap<String, Entry>,
    pub missing_sheets: Vec<u32>,
    pub missing_files: Vec<String>,
}

pub fn build(
    b: &Build,
    manifest: &Manifest,
    members: &[Member],
    atlases: &HashMap<u32, Atlas>,
    exported: &HashMap<u32, String>,
) -> Result<Report, String> {
    let art_dir = b.addon.join("Art");
    std::fs::create_dir_all(&art_dir).map_err(|e| e.to_string())?;
    let game_dir = format!("Interface\\AddOns\\{}\\Art", b.addon_name);
    let mut report = Report::default();

    let wanted = manifest.select(members, atlases, None);
    let missing: HashSet<u32> = wanted
        .iter()
        .map(|m| atlases[&m.atlas_id].file_data_id)
        .filter(|f| !exported.contains_key(f))
        .collect();
    report.missing_sheets = missing.into_iter().collect();
    report.missing_sheets.sort();

    let mut sheets: HashMap<u32, Rgba> = HashMap::new();
    for m in manifest.select(members, atlases, Some(exported)) {
        let atlas = &atlases[&m.atlas_id];
        let fid = atlas.file_data_id;
        if let std::collections::hash_map::Entry::Vacant(slot) = sheets.entry(fid) {
            let png = png_path(b.extract, &exported[&fid]);
            let bytes = std::fs::read(&png).map_err(|e| format!("{}: {e}", png.display()))?;
            slot.insert(Rgba::from_png(&bytes).map_err(|e| format!("{}: {e}", png.display()))?);
        }
        let sheet = &sheets[&fid];
        // Member coordinates are in the atlas's declared size; the exported sheet may differ.
        let sx = sheet.width as f64 / atlas.width as f64;
        let sy = sheet.height as f64 / atlas.height as f64;
        let px = |v: u32, s: f64| (v as f64 * s).round() as u32;
        let region = sheet
            .crop(
                px(m.left, sx),
                px(m.top, sy),
                px(m.right, sx),
                px(m.bottom, sy),
            )
            .fit(MAX_SIDE);
        if region.width == 0 || region.height == 0 {
            continue;
        }
        let (texture, right, bottom) =
            emit(&region, &art_dir, &file_stem(&m.name), &game_dir).map_err(|e| e.to_string())?;
        report.atlases.insert(
            m.name.clone(),
            Entry {
                texture,
                width: m.width,
                height: m.height,
                right,
                bottom,
            },
        );
    }

    for file in &manifest.files {
        let png = png_path(b.extract, file);
        let Ok(bytes) = std::fs::read(&png) else {
            report.missing_files.push(file.clone());
            continue;
        };
        let art = Rgba::from_png(&bytes).map_err(|e| format!("{}: {e}", png.display()))?;
        let (w, h) = (art.width, art.height);
        let base = file
            .rsplit('/')
            .next()
            .unwrap_or(file)
            .trim_end_matches(".blp");
        let (texture, right, bottom) = emit(
            &art.fit(MAX_SIDE),
            &art_dir,
            &format!("file-{}", file_stem(base)),
            &game_dir,
        )
        .map_err(|e| e.to_string())?;
        let key = file.trim_end_matches(".blp").replace('/', "\\");
        report.textures.insert(
            key,
            Entry {
                texture,
                width: w,
                height: h,
                right,
                bottom,
            },
        );
    }

    std::fs::write(b.addon.join("Atlas.lua"), lua(&report)).map_err(|e| e.to_string())?;
    Ok(report)
}

fn png_path(extract: &Path, exported: &str) -> PathBuf {
    let rel = exported.trim_end_matches(".blp");
    extract.join(format!("{rel}.png"))
}

fn lua_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn num(v: f64) -> String {
    let s = format!("{v:.7}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// `Atlas.lua`: `BF_ATLAS[name]` and `BF_TEXTURES[path]` as
/// `{ texture, width, height, left, right, top, bottom }`. Lua 5.0, no `#`.
pub fn lua(report: &Report) -> String {
    let mut out = String::from(
        "-- Generated by forever-art from the WoW Forever client's atlas tables. Do not edit.\n\
         -- { texture, width, height, left, right, top, bottom }\n",
    );
    for (table, entries) in [
        ("BF_ATLAS", &report.atlases),
        ("BF_TEXTURES", &report.textures),
    ] {
        out.push_str(&format!("{table} = {{\n"));
        for (key, e) in entries {
            out.push_str(&format!(
                "[{}] = {{ {}, {}, {}, 0, {}, 0, {} }},\n",
                lua_str(key),
                lua_str(&e.texture),
                e.width,
                e.height,
                num(e.right),
                num(e.bottom)
            ));
        }
        out.push_str("}\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, atlas_id: u32) -> Member {
        Member {
            name: name.into(),
            atlas_id,
            width: 10,
            height: 4,
            left: 2,
            right: 12,
            top: 1,
            bottom: 5,
        }
    }

    #[test]
    fn manifest_parses_and_matches() {
        let m = Manifest::parse("# c\natlas UI-HUD-UnitFrame-*\natlas circlemask # x\nfile Interface\\TargetingFrame\\UI-StatusBar.blp\n").unwrap();
        assert_eq!(m.atlases, ["ui-hud-unitframe-*", "circlemask"]);
        assert_eq!(m.files, ["interface/targetingframe/ui-statusbar.blp"]);
        assert!(m.wants("ui-hud-unitframe-player-portraiton"));
        assert!(m.wants("circlemask"));
        assert!(!m.wants("circlemask2"));
        assert!(Manifest::parse("bogus x").is_err());
    }

    #[test]
    fn select_dedupes_preferring_exported_sheets() {
        let atlases = HashMap::from([
            (
                1,
                Atlas {
                    file_data_id: 100,
                    width: 64,
                    height: 64,
                },
            ),
            (
                2,
                Atlas {
                    file_data_id: 200,
                    width: 64,
                    height: 64,
                },
            ),
        ]);
        let members = vec![member("a", 1), member("a", 2), member("b", 2)];
        let m = Manifest::parse("atlas *").unwrap();
        assert_eq!(m.select(&members, &atlases, None).len(), 2);
        let have = HashMap::from([(200, "x".to_string())]);
        let picked = m.select(&members, &atlases, Some(&have));
        assert!(picked.iter().all(|p| p.atlas_id == 2));
    }

    #[test]
    fn job_lists_sheets_once_and_files() {
        let atlases = HashMap::from([(
            1,
            Atlas {
                file_data_id: 100,
                width: 64,
                height: 64,
            },
        )]);
        let members = vec![member("a", 1), member("b", 1)];
        let m = Manifest::parse("atlas *\nfile interface/x.blp").unwrap();
        let j = job_json(
            &m, &members, &atlases, "C:\\WoW", "wowf", "C:\\out", "C:\\log",
        );
        assert!(j.contains("\"install\":\"C:\\\\WoW\""));
        assert!(j.contains("\"files\":[{\"fid\":100},{\"name\":\"interface/x.blp\"}]"));
    }

    #[test]
    fn lua_table_shape() {
        let mut r = Report::default();
        r.atlases.insert(
            "a\"b".into(),
            Entry {
                texture: "Interface\\AddOns\\x\\Art\\a".into(),
                width: 198,
                height: 71,
                right: 198.0 / 256.0,
                bottom: 71.0 / 128.0,
            },
        );
        let text = lua(&r);
        assert!(text.contains("[\"a\\\"b\"] = { \"Interface\\\\AddOns\\\\x\\\\Art\\\\a\", 198, 71, 0, 0.7734375, 0, 0.5546875 },"));
        assert!(text.contains("BF_TEXTURES = {\n}"));
    }

    #[test]
    fn stems_are_file_safe() {
        assert_eq!(
            file_stem("!RaidFrame-Absorb Edge"),
            "_raidframe-absorb_edge"
        );
    }
}
