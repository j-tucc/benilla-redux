//! An optional, process-wide source of replacement world-art textures for the BLP loader's
//! `WorldArt` lane (M2 and WMO model textures) and `WorldAssets::texture` (ground clutter, skies). Unset, the loader is exactly the stock path. Set,
//! a texture the source supplies as a whole BC7 chain uploads verbatim at its own size, keeping the
//! asset path's sampler mode. Deviation: 1.12.1 has no texture replacement; this exists for an
//! embedding game's texture packs and no benilla setting reaches it.

use std::sync::{Arc, OnceLock};

use bevy::render::render_resource::TextureFormat;

/// One replacement texture: a mip chain to 1x1 (each level `max(1, edge >> i)`), level 0 first,
/// straight (not premultiplied) alpha, in the non-sRGB gamma-byte space world art samples in.
#[derive(Clone)]
pub struct TextureOverride {
    pub width: u32,
    pub height: u32,
    pub mip_count: u32,
    pub format: TextureFormat,
    pub chain: Arc<[u8]>,
}

/// Supplies replacement world art by normalized texture key: lowercase, backslashes, `.blp`, no
/// sampler marker (`world\nodxt\detail\elwynngrass.blp`). Called from the asset task pool.
pub trait WorldTextureSource: Send + Sync + 'static {
    fn texture(&self, key: &str) -> Option<TextureOverride>;
}

static TEXTURE_SOURCE: OnceLock<Arc<dyn WorldTextureSource>> = OnceLock::new();

/// Install the process's world-art source before any model loads; `false` when one was set.
pub fn set_world_texture_source(source: Arc<dyn WorldTextureSource>) -> bool {
    TEXTURE_SOURCE.set(source).is_ok()
}

/// The source's key for a `WorldArt` asset path (`world/x/foo@cc.blp` → `world\x\foo.blp`).
pub(crate) fn texture_key(asset_path: &str) -> String {
    let path = asset_path.to_ascii_lowercase().replace('/', "\\");
    crate::parse_marker(&path).map_or(path.clone(), |m| format!("{}.{}", m.base, m.ext))
}

/// The replacement for `asset_path`, when a source is set, the device takes BC and the chain is
/// a valid BC7 chain of whole 4x4 blocks at mip 0.
pub(crate) fn replacement(asset_path: &str, bc_supported: bool) -> Option<TextureOverride> {
    let source = TEXTURE_SOURCE.get()?;
    if !bc_supported {
        return None;
    }
    let found = source.texture(&texture_key(asset_path))?;
    match invalid(&found) {
        None => Some(found),
        Some(why) => {
            bevy::log::warn!("world texture source: {asset_path}: {why}; using the stock texture");
            None
        }
    }
}

/// Bytes of a BC7 level of `w × h`.
fn bc7_level_bytes(w: u32, h: u32) -> usize {
    (w.div_ceil(4) * h.div_ceil(4)) as usize * 16
}

fn invalid(t: &TextureOverride) -> Option<String> {
    if t.format != TextureFormat::Bc7RgbaUnorm {
        return Some(format!("format {:?}", t.format));
    }
    if t.width == 0 || t.height == 0 || !t.width.is_multiple_of(4) || !t.height.is_multiple_of(4) {
        return Some(format!("{}x{} is not whole blocks", t.width, t.height));
    }
    let full = 32 - t.width.max(t.height).leading_zeros();
    if t.mip_count != full {
        return Some(format!("{} mips, want {full}", t.mip_count));
    }
    let want: usize = (0..full)
        .map(|i| bc7_level_bytes((t.width >> i).max(1), (t.height >> i).max(1)))
        .sum();
    (t.chain.len() != want).then(|| format!("{} bytes, want {want}", t.chain.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(w: u32, h: u32) -> TextureOverride {
        let mips = 32 - w.max(h).leading_zeros();
        let bytes = (0..mips)
            .map(|i| bc7_level_bytes((w >> i).max(1), (h >> i).max(1)))
            .sum::<usize>();
        TextureOverride {
            width: w,
            height: h,
            mip_count: mips,
            format: TextureFormat::Bc7RgbaUnorm,
            chain: vec![0u8; bytes].into(),
        }
    }

    #[test]
    fn keys_drop_the_sampler_marker_and_normalize() {
        assert_eq!(
            texture_key("World/NoDXT/Detail/ElwynnGrass@cc.blp"),
            "world\\nodxt\\detail\\elwynngrass.blp"
        );
        assert_eq!(texture_key("world/a/b.blp"), "world\\a\\b.blp");
    }

    #[test]
    fn square_and_tall_bc7_chains_are_valid() {
        assert!(invalid(&chain(1024, 1024)).is_none());
        assert!(invalid(&chain(256, 1024)).is_none());
        assert!(invalid(&chain(2048, 512)).is_none());
    }

    #[test]
    fn anything_else_is_refused() {
        let mut t = chain(1024, 1024);
        t.format = TextureFormat::Bc7RgbaUnormSrgb;
        assert!(invalid(&t).is_some());
        let mut t = chain(1024, 1024);
        t.mip_count -= 1;
        assert!(invalid(&t).is_some());
        let mut t = chain(1024, 1024);
        t.chain = vec![0u8; 16].into();
        assert!(invalid(&t).is_some());
        assert!(invalid(&chain(1022, 1024)).is_some());
    }

    #[test]
    fn no_source_means_no_replacement() {
        assert!(replacement("world/a/b.blp", true).is_none());
    }
}
