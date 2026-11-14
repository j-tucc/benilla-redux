//! The atlas tables wow.export writes as `;`-separated CSV, and its CLI log of exported files.

use std::collections::HashMap;

/// One `UiTextureAtlas` row: a texture sheet and the pixel size its members' coordinates use.
#[derive(Clone, Debug, PartialEq)]
pub struct Atlas {
    pub file_data_id: u32,
    pub width: u32,
    pub height: u32,
}

/// One `UiTextureAtlasMember` row: a named region of a sheet, `left..right` × `top..bottom` in the
/// sheet's pixels, drawn at `width` × `height` UI units.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub name: String,
    pub atlas_id: u32,
    pub width: u32,
    pub height: u32,
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

/// A header plus rows, each row a map from column name to cell.
fn rows(text: &str) -> Result<Vec<HashMap<&str, &str>>, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<&str> = lines
        .next()
        .ok_or("empty table")?
        .trim_start_matches('\u{feff}')
        .split(';')
        .map(str::trim)
        .collect();
    Ok(lines
        .map(|line| {
            header
                .iter()
                .copied()
                .zip(line.split(';').map(str::trim))
                .collect()
        })
        .collect())
}

fn num(row: &HashMap<&str, &str>, column: &str) -> Result<u32, String> {
    let cell = row
        .get(column)
        .ok_or_else(|| format!("no column {column}"))?;
    cell.parse()
        .map_err(|_| format!("{column}: not a number: {cell:?}"))
}

/// `UiTextureAtlas.csv` by atlas id.
pub fn parse_atlases(text: &str) -> Result<HashMap<u32, Atlas>, String> {
    rows(text)?
        .iter()
        .map(|r| {
            Ok((
                num(r, "ID")?,
                Atlas {
                    file_data_id: num(r, "FileDataID")?,
                    width: num(r, "AtlasWidth")?,
                    height: num(r, "AtlasHeight")?,
                },
            ))
        })
        .collect()
}

/// `UiTextureAtlasMember.csv`, names lower-cased (atlas lookups ignore case). A non-zero
/// `OverrideWidth`/`OverrideHeight` replaces the drawn size, as the modern client does.
pub fn parse_members(text: &str) -> Result<Vec<Member>, String> {
    rows(text)?
        .iter()
        .map(|r| {
            let over = |c: &str, base: u32| num(r, c).map(|v| if v == 0 { base } else { v });
            let width = num(r, "Width")?;
            let height = num(r, "Height")?;
            Ok(Member {
                name: r
                    .get("CommittedName")
                    .ok_or("no column CommittedName")?
                    .to_lowercase(),
                atlas_id: num(r, "UiTextureAtlasID")?,
                width: over("OverrideWidth", width)?,
                height: over("OverrideHeight", height)?,
                left: num(r, "CommittedLeft")?,
                right: num(r, "CommittedRight")?,
                top: num(r, "CommittedTop")?,
                bottom: num(r, "CommittedBottom")?,
            })
        })
        .collect()
}

/// The wow.export CLI log's `ok <fid> <path>` lines, FileDataID to exported path. Name-only
/// entries (`ok  <path>`) carry no id and are skipped.
pub fn parse_export_log(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split(' ');
            (words.next()? == "ok").then_some(())?;
            let fid = words.next()?.parse().ok()?;
            Some((fid, words.next()?.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_parse_and_override_size() {
        let csv = "ID;CommittedName;UiTextureAtlasID;Width;Height;CommittedLeft;CommittedRight;CommittedTop;CommittedBottom;UiTextureAtlasElementID;OverrideWidth;OverrideHeight;CommittedFlags;UiCanvasID\n\
                   16110;UI-HUD-UnitFrame-Player-PortraitOn;2060;198;71;1;199;87;158;19918;0;0;0;0\n\
                   2;X;5;10;10;0;10;0;10;1;20;0;0;0\n";
        let m = parse_members(csv).unwrap();
        assert_eq!(m[0].name, "ui-hud-unitframe-player-portraiton");
        assert_eq!(
            (m[0].left, m[0].right, m[0].top, m[0].bottom),
            (1, 199, 87, 158)
        );
        assert_eq!((m[1].width, m[1].height), (20, 10));
    }

    #[test]
    fn atlases_parse() {
        let a = parse_atlases("ID;FileDataID;UiTextureAtlasSetID;AtlasWidth;AtlasHeight;UiCanvasID\n2060;4631591;0;1024;512;1\n").unwrap();
        assert_eq!(
            a[&2060],
            Atlas {
                file_data_id: 4631591,
                width: 1024,
                height: 512
            }
        );
    }

    #[test]
    fn export_log_keeps_ids_only() {
        let log = "loading wowf\nok 4631591 interface/hud/uiunitframe.blp\nok  interface/x.blp\nfail 5 y\n";
        let map = parse_export_log(log);
        assert_eq!(map.len(), 1);
        assert_eq!(map[&4631591], "interface/hud/uiunitframe.blp");
    }
}
