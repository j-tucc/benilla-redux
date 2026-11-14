//! An optional, process-wide source of replacement ground layers for the terrain loader. Unset, the
//! loader is exactly the stock 256² path. Set, a tile whose every MTEX texture the source supplies
//! in one BC7 shape uploads those chains verbatim at that size; any gap keeps the whole tile stock.
//! Deviation: 1.12.1 has no layer replacement; this exists for an embedding game's texture packs
//! and no benilla setting reaches it.

use std::sync::{Arc, OnceLock};

use bevy::render::render_resource::TextureFormat;

use crate::terrain::PackedLayers;

/// One replacement ground texture: a square mip chain to 1x1, level 0 first, sheen mask in alpha
/// (zero for a matte texture), in the non-sRGB gamma-byte space the terrain shader samples.
#[derive(Clone)]
pub struct LayerOverride {
    pub size: u32,
    pub mip_count: u32,
    pub format: TextureFormat,
    pub chain: Arc<[u8]>,
}

/// What the loader did with one tile, for the source's own accounting.
#[derive(Clone, Debug)]
pub struct TileLayerReport {
    /// The tile's asset path.
    pub tile: String,
    /// Layers in the array, the fallback at index 0 included.
    pub layers: u32,
    /// The array's mip-0 edge and format.
    pub size: u32,
    pub format: TextureFormat,
    /// Bytes uploaded for the whole layer array.
    pub bytes: usize,
    /// Whether the replacement chains were used.
    pub replaced: bool,
    /// Why a tile stayed stock, when a source was set.
    pub reason: Option<String>,
}

/// Supplies replacement layers by normalized MTEX key (`tileset\elwynn\elwynngrassbase.blp`).
/// Called from the asset loader's task pool, so it may read files.
pub trait TerrainLayerSource: Send + Sync + 'static {
    fn layer(&self, key: &str) -> Option<LayerOverride>;
    /// Called once per loaded tile whenever a source is set.
    fn packed(&self, _report: &TileLayerReport) {}
}

static LAYER_SOURCE: OnceLock<Arc<dyn TerrainLayerSource>> = OnceLock::new();

/// Install the process's layer source before any tile loads; `false` when one was already set.
pub fn set_terrain_layer_source(source: Arc<dyn TerrainLayerSource>) -> bool {
    LAYER_SOURCE.set(source).is_ok()
}

pub(crate) fn layer_source() -> Option<&'static Arc<dyn TerrainLayerSource>> {
    LAYER_SOURCE.get()
}

/// Bytes of one BC7 level at edge `w`: whole 4x4 blocks of 16 bytes.
fn bc7_level_bytes(w: u32) -> usize {
    let blocks = w.div_ceil(4) as usize;
    blocks * blocks * 16
}

/// The level count of a square chain to 1x1.
fn full_chain(size: u32) -> u32 {
    32 - size.leading_zeros()
}

/// Why `layer` cannot go in a replacement array, if it cannot.
fn invalid(layer: &LayerOverride) -> Option<String> {
    if layer.format != TextureFormat::Bc7RgbaUnorm {
        return Some(format!("format {:?}", layer.format));
    }
    if !layer.size.is_power_of_two() || layer.size < 4 {
        return Some(format!("size {}", layer.size));
    }
    if layer.mip_count != full_chain(layer.size) {
        return Some(format!("{} mips at {}", layer.mip_count, layer.size));
    }
    let want: usize = (0..layer.mip_count)
        .map(|i| bc7_level_bytes(layer.size >> i))
        .sum();
    (layer.chain.len() != want).then(|| format!("{} bytes, want {want}", layer.chain.len()))
}

/// One solid BC7 block, mode 6 with both endpoints `rgba` and every index 0. Mode 6 stores 7 bits
/// per channel plus a shared low bit, here 0, so each channel reads its value with the low bit
/// cleared.
pub(crate) fn solid_bc7_block(rgba: [u8; 4]) -> [u8; 16] {
    let mut bits: u128 = 1 << 6;
    for (channel, value) in rgba.iter().enumerate() {
        let v = u128::from(value >> 1);
        let at = 7 + channel * 14;
        bits |= v << at | v << (at + 7);
    }
    bits.to_le_bytes()
}

/// A replacement array for a tile whose layers are `keys`, first-seen order, or the reason the
/// tile stays stock. `fallback` becomes index 0 in the replacement shape.
pub(crate) fn replacement_layers(
    source: &dyn TerrainLayerSource,
    keys: &[String],
    fallback: [u8; 4],
    bc_supported: bool,
) -> Result<(u32, PackedLayers), String> {
    if !bc_supported {
        return Err("device has no BC".into());
    }
    if keys.is_empty() {
        return Err("no layers".into());
    }
    let mut layers = Vec::with_capacity(keys.len());
    for key in keys {
        let layer = source.layer(key).ok_or_else(|| format!("missing {key}"))?;
        if let Some(why) = invalid(&layer) {
            return Err(format!("{key}: {why}"));
        }
        layers.push(layer);
    }
    let (size, mip_count, format) = (layers[0].size, layers[0].mip_count, layers[0].format);
    if let Some(odd) = layers.iter().zip(keys).find(|(l, _)| l.size != size) {
        return Err(format!("{}: {} beside {size}", odd.1, odd.0.size));
    }
    let block = solid_bc7_block(fallback);
    let per_layer = layers[0].chain.len();
    let mut data = Vec::with_capacity((layers.len() + 1) * per_layer);
    for _ in 0..per_layer / block.len() {
        data.extend_from_slice(&block);
    }
    for layer in &layers {
        data.extend_from_slice(&layer.chain);
    }
    Ok((
        size,
        PackedLayers {
            data,
            format,
            mip_count,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn chain(size: u32) -> LayerOverride {
        let mips = full_chain(size);
        let bytes: usize = (0..mips).map(|i| bc7_level_bytes(size >> i)).sum();
        LayerOverride {
            size,
            mip_count: mips,
            format: TextureFormat::Bc7RgbaUnorm,
            chain: vec![7u8; bytes].into(),
        }
    }

    struct Map(HashMap<String, LayerOverride>);
    impl TerrainLayerSource for Map {
        fn layer(&self, key: &str) -> Option<LayerOverride> {
            self.0.get(key).cloned()
        }
    }

    fn source(entries: &[(&str, LayerOverride)]) -> Map {
        Map(entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect())
    }

    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_uniform_source_packs_at_its_size_with_the_fallback_first() {
        for size in [1024, 2048] {
            let src = source(&[("a", chain(size)), ("b", chain(size))]);
            let (got, packed) =
                replacement_layers(&src, &keys(&["a", "b"]), [107, 133, 82, 0], true).unwrap();
            assert_eq!(got, size);
            assert_eq!(packed.format, TextureFormat::Bc7RgbaUnorm);
            assert_eq!(packed.mip_count, full_chain(size));
            let per = chain(size).chain.len();
            assert_eq!(packed.data.len(), 3 * per);
            assert_eq!(&packed.data[..16], &solid_bc7_block([107, 133, 82, 0]));
            assert!(packed.data[per..].iter().all(|&b| b == 7));
        }
    }

    #[test]
    fn any_gap_keeps_the_tile_stock() {
        let src = source(&[("a", chain(1024)), ("b", chain(512))]);
        let fb = [0, 0, 0, 0];
        assert!(replacement_layers(&src, &keys(&["a", "c"]), fb, true).is_err());
        assert!(replacement_layers(&src, &keys(&["a", "b"]), fb, true).is_err());
        assert!(replacement_layers(&src, &keys(&["a"]), fb, false).is_err());
        assert!(replacement_layers(&src, &[], fb, true).is_err());
        let mut short = chain(1024);
        short.chain = vec![0u8; 16].into();
        let src = source(&[("a", short)]);
        assert!(replacement_layers(&src, &keys(&["a"]), fb, true).is_err());
        let mut srgb = chain(1024);
        srgb.format = TextureFormat::Bc7RgbaUnormSrgb;
        let src = source(&[("a", srgb)]);
        assert!(replacement_layers(&src, &keys(&["a"]), fb, true).is_err());
    }

    #[test]
    fn the_solid_block_is_mode_six_with_matching_endpoints() {
        let bits = u128::from_le_bytes(solid_bc7_block([107, 133, 82, 0]));
        assert_eq!(bits & 0x7f, 0x40, "mode 6");
        for (channel, value) in [107u8, 133, 82, 0].into_iter().enumerate() {
            let at = 7 + channel * 14;
            let e0 = (bits >> at) & 0x7f;
            let e1 = (bits >> (at + 7)) & 0x7f;
            assert_eq!(e0, u128::from(value >> 1));
            assert_eq!(e0, e1);
        }
        assert_eq!(bits >> 63, 0, "p-bits and indices 0");
    }
}
