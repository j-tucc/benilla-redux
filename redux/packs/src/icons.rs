//! High-resolution icons from a local folder of `<IconName>.tga` files (`$REDUX_HD_ICONS`, else
//! `classic-overrides/ICONS` when the settings file turns them on), served to benilla's UI through
//! its UI sprite source hook (REDUX.md's patch ledger) for every `Interface\Icons\` sprite. The
//! folder is indexed once on a background thread by lower-case stem; an icon is decoded at its
//! first use into the sRGB sprite the stock icon makes, plus mips. Layout keeps the stock size.
//! Unattended runs stay on stock icons unless the variable names a folder. Ported from Elderoth's
//! `benilla-adapter/src/ui/hd_icons.rs`.
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// The UI sprite key prefix of an icon ([`benilla_assets::sprite_key`]).
const ICON_PREFIX: &str = "interface\\icons\\";

/// The HD folder from `$REDUX_HD_ICONS` (`var`), the settings file's switch (`on`), the
/// unattended flag and the found default (`found`): `off`, `vanilla` or empty disable it, any
/// other value names it; unset, the default folder when the settings turn icons on, except in an
/// unattended run.
pub(crate) fn resolve_root(
    var: Option<&str>,
    on: bool,
    unattended: bool,
    found: Option<PathBuf>,
) -> Option<PathBuf> {
    match var.map(str::trim) {
        Some(v)
            if v.is_empty()
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("vanilla") =>
        {
            None
        }
        Some(v) => Some(PathBuf::from(v)),
        None if unattended || !on => None,
        None => found.filter(|p| p.is_dir()),
    }
}

/// Every `*.tga` directly in `dir`, by lower-case file stem. A missing folder is empty.
pub(crate) fn index_dir(dir: &Path) -> HashMap<String, PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return HashMap::default();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let ext = path.extension()?.to_str()?;
            if !ext.eq_ignore_ascii_case("tga") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
            Some((stem, path))
        })
        .collect()
}

/// The HD icon source: the index, filled once by a background scan, and what it served.
pub(crate) struct HdIcons {
    index: Arc<OnceLock<HashMap<String, PathBuf>>>,
    pub served: AtomicU64,
    pub failures: AtomicU64,
}

impl HdIcons {
    /// Starts indexing `dir`.
    pub fn spawn(dir: PathBuf) -> Self {
        let index = Arc::new(OnceLock::new());
        let fill = index.clone();
        std::thread::Builder::new()
            .name("hd-icon-index".into())
            .spawn(move || {
                let found = index_dir(&dir);
                eprintln!("HD_ICONS {} icons from {}", found.len(), dir.display());
                let _ = fill.set(found);
            })
            .expect("spawn the HD icon index thread");
        HdIcons {
            index,
            served: AtomicU64::new(0),
            failures: AtomicU64::new(0),
        }
    }

    /// The file for an icon sprite key, once the index is built: a first ask waits for it, so an
    /// early icon is not cached stock for the session.
    fn file(&self, key: &str) -> Option<&Path> {
        let stem = key.strip_prefix(ICON_PREFIX)?;
        self.index.wait().get(stem).map(PathBuf::as_path)
    }
}

impl benilla_assets::UiSpriteSource for HdIcons {
    fn sprite(&self, key: &str, tiled: bool) -> Option<Image> {
        let file = self.file(key)?;
        match decode(file, tiled) {
            Ok(image) => {
                self.served.fetch_add(1, Ordering::Relaxed);
                Some(image)
            }
            Err(e) => {
                self.failures.fetch_add(1, Ordering::Relaxed);
                warn!("HD_ICONS {e}; the stock icon draws");
                None
            }
        }
    }
}

/// An icon in the stock UI sprite's form (`benilla_assets`' `sprite_image`): sRGB RGBA8, clamped
/// (repeating when `tiled`), linear, so HD and stock icons share brightness and alpha. Unlike the
/// 64 px stock icon it carries a full mip chain: icon slots draw well under 128 px, and minifying
/// 128 px art without mips aliases.
pub(crate) fn icon_image(width: u32, height: u32, rgba: Vec<u8>, tiled: bool) -> Image {
    let mips = mip_chain(width, height, rgba);
    let levels = mips.len() as u32;
    let mut image = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(mips.concat());
    image.texture_descriptor.mip_level_count = levels;
    let address = if tiled {
        ImageAddressMode::Repeat
    } else {
        ImageAddressMode::ClampToEdge
    };
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: address,
        address_mode_v: address,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..Default::default()
    });
    image
}

fn to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(l: f32) -> u8 {
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1. / 2.4) - 0.055
    };
    (c.clamp(0., 1.) * 255.).round() as u8
}

/// Mip 0 and every halving down to 1x1: a 2x2 box in linear light, colour weighted by alpha so
/// transparent texels do not darken the edges (straight alpha in, straight alpha out).
pub(crate) fn mip_chain(width: u32, height: u32, rgba: Vec<u8>) -> Vec<Vec<u8>> {
    let lut: [f32; 256] = std::array::from_fn(|i| to_linear(i as u8));
    let (mut w, mut h) = (width as usize, height as usize);
    let mut mips = vec![rgba];
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let src = mips.last().expect("mip 0");
        let mut out = Vec::with_capacity(nw * nh * 4);
        for y in 0..nh {
            for x in 0..nw {
                let (mut rgb, mut a_sum, mut plain) = ([0f32; 3], 0f32, [0f32; 3]);
                let mut n = 0f32;
                for (sx, sy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (px, py) = ((2 * x + sx).min(w - 1), (2 * y + sy).min(h - 1));
                    let p = &src[(py * w + px) * 4..][..4];
                    let a = f32::from(p[3]) / 255.;
                    for c in 0..3 {
                        let l = lut[p[c] as usize];
                        rgb[c] += l * a;
                        plain[c] += l;
                    }
                    a_sum += a;
                    n += 1.;
                }
                for c in 0..3 {
                    let l = if a_sum > 0. {
                        rgb[c] / a_sum
                    } else {
                        plain[c] / n
                    };
                    out.push(to_srgb(l));
                }
                out.push((a_sum / n * 255.).round() as u8);
            }
        }
        mips.push(out);
        (w, h) = (nw, nh);
    }
    mips
}

/// Reads and decodes one HD icon file.
pub(crate) fn decode(path: &Path, tiled: bool) -> std::io::Result<Image> {
    let bytes = std::fs::read(path)?;
    let (w, h, rgba) = benilla_formats::tga_to_rgba(&bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: {e}", path.display()),
        )
    })?;
    Ok(icon_image(w, h, rgba, tiled))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("redux-hd-icons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A 2x2 top-left-origin 32-bit TGA of one BGRA pixel.
    fn tga(bgra: [u8; 4]) -> Vec<u8> {
        let mut b = vec![0u8; 18];
        b[2] = 2;
        b[12..14].copy_from_slice(&2u16.to_le_bytes());
        b[14..16].copy_from_slice(&2u16.to_le_bytes());
        b[16] = 32;
        b[17] = 8 | 0x20;
        for _ in 0..4 {
            b.extend_from_slice(&bgra);
        }
        b
    }

    #[test]
    fn the_index_matches_stems_without_case_and_only_tga() {
        let dir = scratch("index");
        std::fs::write(dir.join("INV_Sword_100.tga"), b"x").unwrap();
        std::fs::write(dir.join("Spell_Holy_Heal.TGA"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.join("INV_Misc_Bag_07.blp"), b"x").unwrap();
        let index = index_dir(&dir);
        assert_eq!(index.len(), 2);
        assert!(index["inv_sword_100"].ends_with("INV_Sword_100.tga"));
        assert!(index.contains_key("spell_holy_heal"));
        assert!(!index.contains_key("inv_misc_bag_07"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mips_run_to_one_texel_without_dark_transparent_fringes() {
        assert_eq!(mip_chain(128, 128, vec![0; 128 * 128 * 4]).len(), 8);
        let sizes: Vec<usize> = mip_chain(4, 2, vec![0; 32]).iter().map(Vec::len).collect();
        assert_eq!(sizes, [32, 8, 4]);
        // Three opaque white texels and one transparent black: white, three-quarter alpha.
        let mut px = [255u8, 255, 255, 255].repeat(3);
        px.extend([0, 0, 0, 0]);
        assert_eq!(mip_chain(2, 2, px)[1], [255, 255, 255, 191]);
        // Black and white average in linear light, not in the stored gamma bytes.
        let px = [[0u8, 0, 0, 255], [255, 255, 255, 255]].concat().repeat(2);
        assert_eq!(mip_chain(2, 2, px)[1], [188, 188, 188, 255]);
    }

    #[test]
    fn a_missing_folder_is_no_hd_icons() {
        assert!(index_dir(Path::new("Z:/no/such/redux/icons")).is_empty());
        let hd = HdIcons::spawn(PathBuf::from("Z:/no/such/redux/icons"));
        assert!(hd.file("interface\\icons\\inv_sword_100").is_none());
    }

    #[test]
    fn only_icon_sprites_are_looked_up() {
        let dir = scratch("lookup");
        std::fs::write(dir.join("INV_Sword_100.tga"), b"x").unwrap();
        let hd = HdIcons::spawn(dir.clone());
        assert!(hd.file("interface\\icons\\inv_sword_100").is_some());
        assert!(hd.file("interface\\buttons\\inv_sword_100").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_root_honours_off_explicit_the_setting_and_unattended() {
        let found = scratch("root");
        for off in ["off", "OFF", "vanilla", "", "  "] {
            let got = resolve_root(Some(off), true, false, Some(found.clone()));
            assert_eq!(got, None, "{off:?}");
        }
        assert_eq!(
            resolve_root(Some("D:/icons"), false, true, None),
            Some(PathBuf::from("D:/icons"))
        );
        assert_eq!(resolve_root(None, true, true, Some(found.clone())), None);
        assert_eq!(resolve_root(None, false, false, Some(found.clone())), None);
        assert_eq!(
            resolve_root(None, true, false, Some(found.clone())),
            Some(found.clone())
        );
        assert_eq!(
            resolve_root(None, true, false, Some(found.join("missing"))),
            None
        );
        let _ = std::fs::remove_dir_all(&found);
    }

    #[test]
    fn a_decoded_icon_is_an_srgb_sprite_of_the_stored_bytes() {
        let dir = scratch("decode");
        let file = dir.join("INV_Test.tga");
        std::fs::write(&file, tga([10, 20, 30, 200])).unwrap();
        let image = decode(&file, false).unwrap();
        assert_eq!(
            image.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!((image.width(), image.height()), (2, 2));
        assert_eq!(image.texture_descriptor.mip_level_count, 2);
        // A uniform image keeps its texel through every mip.
        assert_eq!(
            image.data.as_ref().unwrap().as_slice(),
            [30, 20, 10, 200].repeat(5).as_slice()
        );
        assert!(decode(&dir.join("missing.tga"), false).is_err());
        std::fs::write(dir.join("bad.tga"), b"nope").unwrap();
        assert!(decode(&dir.join("bad.tga"), false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
