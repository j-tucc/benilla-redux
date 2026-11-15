//! forever-art: art exported from the user's own WoW Forever install (by wow.export) made into
//! textures benilla's 1.12 UI loads — each atlas region its own power-of-two TGA — plus the Lua
//! table that lets the benilla-forever addon look regions up by their modern atlas names.
//! It reads and writes only local, gitignored folders; nothing it touches is committed.

pub mod image;
pub mod tables;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use image::{Rgba, MAX_SIDE};
use tables::{Atlas, Member, Slice};

/// The client's atlas tables, as wow.export wrote them.
#[derive(Debug, Default)]
pub struct Tables {
    pub atlases: HashMap<u32, Atlas>,
    pub members: Vec<Member>,
    /// Element id to the name modern code passes to `SetAtlas`.
    pub elements: HashMap<u32, String>,
    pub slices: HashMap<u32, Slice>,
}

/// What to pull: atlas name patterns, whole texture files, and which atlases are status-bar fills.
#[derive(Debug, Default, PartialEq)]
pub struct Manifest {
    pub atlases: Vec<String>,
    pub files: Vec<String>,
    /// Atlases drawn as a StatusBar fill. benilla's bar writes its own texcoords (0 to the fill
    /// fraction, 0 to 1), so their art is resampled to fill its power-of-two texture exactly
    /// instead of being padded.
    pub bars: Vec<String>,
}

/// `*` matches any run of characters, anywhere in the pattern.
pub fn glob(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first) || !name[first.len()..].ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
}

impl Manifest {
    /// Lines `atlas <pattern>`, `bar <pattern>` and `file <path.blp>`; `#` starts a comment.
    /// A `bar` pattern also selects its atlases.
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
                "bar" => m.bars.push(value),
                "file" => m.files.push(value),
                _ => return Err(format!("line {}: unknown kind {kind:?}", n + 1)),
            }
        }
        Ok(m)
    }

    /// Adds another manifest's lines to this one.
    pub fn merge(&mut self, other: Manifest) {
        self.atlases.extend(other.atlases);
        self.files.extend(other.files);
        self.bars.extend(other.bars);
    }

    fn wants(&self, name: &str) -> bool {
        self.atlases.iter().any(|p| glob(p, name)) || self.is_bar(name)
    }

    pub fn is_bar(&self, name: &str) -> bool {
        self.bars.iter().any(|p| glob(p, name))
    }

    /// The members the manifest names, by their own name or their element's, one per name: where
    /// a name has several rows, the first whose sheet is in `have` (if given) wins.
    pub fn select<'t>(
        &self,
        t: &'t Tables,
        have: Option<&HashMap<u32, String>>,
    ) -> Vec<&'t Member> {
        let mut by_name: BTreeMap<&str, &Member> = BTreeMap::new();
        let wanted = |m: &&Member| {
            self.wants(&m.name) || t.elements.get(&m.element_id).is_some_and(|e| self.wants(e))
        };
        for m in t.members.iter().filter(wanted) {
            let Some(atlas) = t.atlases.get(&m.atlas_id) else {
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
    t: &Tables,
    install: &str,
    product: &str,
    out_dir: &str,
    log: &str,
) -> String {
    let fids: std::collections::BTreeSet<u32> = manifest
        .select(t, None)
        .iter()
        .map(|m| t.atlases[&m.atlas_id].file_data_id)
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

/// A file-name-safe form of an atlas name. `!` (vertically tiled art) gets its own spelling so
/// `!x` and `_x`, which are different atlases, never share a file.
pub fn file_stem(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        match c {
            '!' => out.push_str("v_"),
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' => {
                out.push(c.to_ascii_lowercase())
            }
            _ => out.push('_'),
        }
    }
    out
}

/// One texture the addon can draw: its in-game path and the texcoords of the art inside it.
#[derive(Clone, Debug, PartialEq)]
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

/// Nine-slice margins in drawn UI units, and whether the edges tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Margins {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub tiled: bool,
}

/// What a build wrote.
#[derive(Debug, Default)]
pub struct Report {
    pub atlases: BTreeMap<String, Entry>,
    pub textures: BTreeMap<String, Entry>,
    /// Nine-slice margins by element name, in drawn units.
    pub slices: BTreeMap<String, Margins>,
    pub missing_sheets: Vec<u32>,
    pub missing_files: Vec<String>,
}

/// How Forever ranks an element's members, lowest first: the bronze 1x art, plain 1x, bronze
/// 2x, plain 2x. Forever draws c60 art wherever an element has it.
fn member_rank(name: &str) -> u8 {
    let hi = name.ends_with("-2x");
    let c60 = name.trim_end_matches("-2x").ends_with("-c60");
    match (c60, hi) {
        (true, false) => 0,
        (false, false) => 1,
        (true, true) => 2,
        (false, true) => 3,
    }
}

impl Report {
    /// Draw each 1x region from its `-2x` sibling's art at the 1x size, as the modern client does
    /// on a high-resolution screen (benilla draws a UI unit over about two pixels there). A bar
    /// fill swaps only for a sibling also built as a bar: their textures crop differently.
    fn prefer_hi_res(&mut self, manifest: &Manifest) {
        let swaps: Vec<(String, Entry)> = self
            .atlases
            .iter()
            .filter(|(name, _)| !name.ends_with("-2x"))
            .filter_map(|(name, _)| {
                let hi = format!("{name}-2x");
                let e = self.atlases.get(&hi)?;
                (manifest.is_bar(name) == manifest.is_bar(&hi)).then(|| (name.clone(), e.clone()))
            })
            .collect();
        for (name, hi) in swaps {
            let e = self.atlases.get_mut(&name).expect("listed above");
            e.texture = hi.texture;
            e.right = hi.right;
            e.bottom = hi.bottom;
        }
    }

    /// Modern code asks for an element by name (`SetAtlas("UI-HUD-MicroMenu-Questlog-Up")`) and
    /// the client picks one of its members; some elements have only `-2x` members. Each element
    /// whose members were built gets an entry under its own name, from its best member, at its
    /// drawn size (a 2x member without an override draws at half its pixels), plus its
    /// nine-slice margins when it has them.
    fn add_element_aliases(&mut self, built: &[&Member], t: &Tables) {
        let mut best: BTreeMap<u32, &Member> = BTreeMap::new();
        for m in built.iter().copied() {
            if !self.atlases.contains_key(&m.name) {
                continue;
            }
            best.entry(m.element_id)
                .and_modify(|b| {
                    if member_rank(&m.name) < member_rank(&b.name) {
                        *b = m;
                    }
                })
                .or_insert(m);
        }
        for (element, m) in best {
            let Some(name) = t.elements.get(&element) else {
                continue;
            };
            // The member the element's entry comes from: one of its own name, else the best.
            let source = built.iter().copied().find(|b| &b.name == name).unwrap_or(m);
            if !self.atlases.contains_key(name) {
                let mut e = self.atlases[&m.name].clone();
                if m.name.ends_with("-2x") && !m.overridden {
                    e.width = e.width.div_ceil(2);
                    e.height = e.height.div_ceil(2);
                }
                self.atlases.insert(name.clone(), e);
            }
            if let Some(s) = t.slices.get(&element) {
                // Slice margins are in the member's pixels; the addon wants drawn units.
                let e = &self.atlases[name];
                let sx = e.width as f64 / source.right.saturating_sub(source.left).max(1) as f64;
                let sy = e.height as f64 / source.bottom.saturating_sub(source.top).max(1) as f64;
                self.slices.insert(
                    name.clone(),
                    Margins {
                        left: s.left as f64 * sx,
                        top: s.top as f64 * sy,
                        right: s.right as f64 * sx,
                        bottom: s.bottom as f64 * sy,
                        tiled: s.mode == 1,
                    },
                );
            }
        }
    }
}

pub fn build(
    b: &Build,
    manifest: &Manifest,
    t: &Tables,
    exported: &HashMap<u32, String>,
) -> Result<Report, String> {
    let atlases = &t.atlases;
    let art_dir = b.addon.join("Art");
    std::fs::create_dir_all(&art_dir).map_err(|e| e.to_string())?;
    let game_dir = format!("Interface\\AddOns\\{}\\Art", b.addon_name);
    let mut report = Report::default();

    let wanted = manifest.select(t, None);
    let missing: HashSet<u32> = wanted
        .iter()
        .map(|m| atlases[&m.atlas_id].file_data_id)
        .filter(|f| !exported.contains_key(f))
        .collect();
    report.missing_sheets = missing.into_iter().collect();
    report.missing_sheets.sort();

    let mut sheets: HashMap<u32, Rgba> = HashMap::new();
    let built = manifest.select(t, Some(exported));
    for m in built.iter().copied() {
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
        let stem = file_stem(&m.name);
        let (texture, right, bottom) = if manifest.is_bar(&m.name) {
            // A bar fill spans its whole texture: the StatusBar owns the texcoords.
            let w = image::pot(region.width).min(MAX_SIDE);
            let h = image::pot(region.height).min(MAX_SIDE);
            region
                .resize(w, h)
                .write_tga(&art_dir.join(format!("{stem}.tga")))
                .map_err(|e| e.to_string())?;
            (format!("{game_dir}\\{stem}"), 1.0, 1.0)
        } else {
            emit(&region, &art_dir, &stem, &game_dir).map_err(|e| e.to_string())?
        };
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

    report.prefer_hi_res(manifest);
    report.add_element_aliases(&built, t);

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
    out.push_str("-- Nine-slice margins in drawn units: { left, top, right, bottom, tiled }\nBF_SLICES = {\n");
    for (key, s) in &report.slices {
        out.push_str(&format!(
            "[{}] = {{ {}, {}, {}, {}, {} }},\n",
            lua_str(key),
            num(s.left),
            num(s.top),
            num(s.right),
            num(s.bottom),
            u8::from(s.tiled)
        ));
    }
    out.push_str("}\n");
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
            element_id: 0,
            overridden: false,
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
    fn globs_and_bars() {
        assert!(glob(
            "ui-hud-*-bar-health*",
            "ui-hud-unitframe-player-portraiton-bar-health-c60"
        ));
        assert!(!glob(
            "ui-hud-*-bar-health*",
            "ui-hud-unitframe-player-portraiton-bar-mana"
        ));
        assert!(glob("*mask", "circlemask"));
        assert!(glob("exact", "exact") && !glob("exact", "exactly"));
        assert!(!glob("a*b*c", "acb"));
        let m = Manifest::parse("bar *-bar-mana*\n").unwrap();
        assert!(m.is_bar("x-bar-mana-c60") && m.wants("x-bar-mana-c60"));
        assert!(!m.is_bar("x-bar-health") && !m.wants("x-bar-health"));
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
        let t = Tables {
            atlases,
            members: vec![member("a", 1), member("a", 2), member("b", 2)],
            ..Tables::default()
        };
        let m = Manifest::parse("atlas *").unwrap();
        assert_eq!(m.select(&t, None).len(), 2);
        let have = HashMap::from([(200, "x".to_string())]);
        let picked = m.select(&t, Some(&have));
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
        let t = Tables {
            atlases,
            members: vec![member("a", 1), member("b", 1)],
            ..Tables::default()
        };
        let m = Manifest::parse("atlas *\nfile interface/x.blp").unwrap();
        let j = job_json(&m, &t, "C:\\WoW", "wowf", "C:\\out", "C:\\log");
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

    fn entry(width: u32, height: u32) -> Entry {
        Entry {
            texture: "t".into(),
            width,
            height,
            right: 1.0,
            bottom: 1.0,
        }
    }

    #[test]
    fn elements_alias_their_best_member() {
        let el = |name: &str, element_id, overridden, w, h| Member {
            element_id,
            overridden,
            width: w,
            height: h,
            left: 0,
            right: w,
            top: 0,
            bottom: h,
            ..member(name, 1)
        };
        let members = vec![
            el("frame-2x", 7, false, 110, 110),
            el("frame-c60-2x", 7, false, 110, 110),
            el("slot", 8, true, 45, 45),
            el("slot-c60", 8, false, 64, 62),
            el("own", 9, false, 10, 10),
        ];
        let t = Tables {
            elements: HashMap::from([
                (7, "frame".to_string()),
                (8, "slot-el".to_string()),
                (9, "own".to_string()),
            ]),
            slices: HashMap::from([(
                7,
                Slice {
                    left: 16,
                    top: 20,
                    right: 16,
                    bottom: 20,
                    mode: 1,
                },
            )]),
            ..Tables::default()
        };
        let mut r = Report::default();
        for m in &members {
            r.atlases.insert(m.name.clone(), entry(m.width, m.height));
        }
        let built: Vec<&Member> = members.iter().collect();
        r.add_element_aliases(&built, &t);
        // The bronze 2x member wins and draws at half its pixels.
        assert_eq!(
            (r.atlases["frame"].width, r.atlases["frame"].height),
            (55, 55)
        );
        // The bronze 1x member beats the plain one, whatever the plain one's override.
        assert_eq!(r.atlases["slot-el"].width, 64);
        // A member already named like its element keeps its own entry.
        assert_eq!(r.atlases["own"].width, 10);
        // Margins come out in drawn units: half the 2x member's pixels.
        assert_eq!(r.slices["frame"].top, 10.0);
        assert!(lua(&r).contains("BF_SLICES = {\n[\"frame\"] = { 8, 10, 8, 10, 1 },\n}"));
    }

    #[test]
    fn regions_draw_from_their_2x_art_at_1x_size() {
        let mut r = Report::default();
        let mut hi = entry(92, 90);
        hi.texture = "hi".into();
        hi.right = 0.72;
        r.atlases.insert("frame-c60".into(), entry(46, 45));
        r.atlases.insert("frame-c60-2x".into(), hi);
        r.atlases.insert("fill".into(), entry(124, 20));
        r.atlases.insert("fill-2x".into(), {
            let mut e = entry(248, 40);
            e.texture = "padded".into();
            e
        });
        r.atlases.insert("lone".into(), entry(10, 10));
        let m = Manifest {
            bars: vec!["fill".into()],
            ..Manifest::default()
        };
        r.prefer_hi_res(&m);
        let e = &r.atlases["frame-c60"];
        assert_eq!(
            (e.texture.as_str(), e.width, e.height, e.right),
            ("hi", 46, 45, 0.72)
        );
        // A bar fill keeps its own art when its 2x sibling is not built as a bar.
        assert_ne!(r.atlases["fill"].texture, "padded");
        assert_eq!(r.atlases["lone"].width, 10);
    }

    #[test]
    fn stems_are_file_safe() {
        assert_eq!(
            file_stem("!RaidFrame-Absorb Edge"),
            "v_raidframe-absorb_edge"
        );
        assert_ne!(file_stem("!a-center"), file_stem("_a-center"));
    }
}
