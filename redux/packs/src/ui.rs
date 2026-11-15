//! The UI sprite source redux installs into benilla's hook: HD icons ([`crate::icons`]) for
//! `Interface\Icons\` when they are on, else the active pack's own `interface/…` file (the pack's
//! 4x BC7 art, uploaded as stored with its full mip chain). Anything neither holds draws stock.
//!
//! benilla caches a UI sprite for the session at its first use, so the pack's UI art follows the
//! pack chosen at startup; a live pack switch reaches only sprites first used after it, and the
//! rest on restart. `pack_ui = false` in `redux-textures.toml` (or `REDUX_PACK_UI=0`) keeps the
//! stock UI under a pack.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use benilla_assets::UiSpriteSource;
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::catalog::PackSource;
use crate::dds::Bc7Shape;
use crate::icons::HdIcons;

/// The UI sprite key prefix a pack can answer ([`benilla_assets::sprite_key`]).
const INTERFACE_PREFIX: &str = "interface\\";

/// HD icons and the active pack, as one UI sprite source.
pub(crate) struct UiSprites {
    icons: Option<Arc<HdIcons>>,
    pack_ui: bool,
    pack: RwLock<Option<Arc<PackSource>>>,
    pub pack_served: AtomicU64,
}

static UI: OnceLock<Arc<UiSprites>> = OnceLock::new();

/// Install the UI source into benilla's hook, once, before the app is built.
pub(crate) fn install(icons: Option<Arc<HdIcons>>, pack_ui: bool) -> Arc<UiSprites> {
    let ui = UI
        .get_or_init(|| {
            Arc::new(UiSprites {
                icons,
                pack_ui,
                pack: RwLock::new(None),
                pack_served: AtomicU64::new(0),
            })
        })
        .clone();
    benilla_assets::set_ui_sprite_source(Some(ui.clone() as Arc<dyn UiSpriteSource>));
    ui
}

/// Point the UI source at the active pack (or none), for sprites first used from now on.
pub(crate) fn set_pack(source: Option<&Arc<PackSource>>) {
    if let Some(ui) = UI.get() {
        *ui.pack.write().unwrap_or_else(|p| p.into_inner()) = source.cloned();
    }
}

impl UiSprites {
    /// Whether HD icons are on.
    pub fn icons_on(&self) -> bool {
        self.icons.is_some()
    }

    /// The HD icon source, for the probe.
    pub fn icons(&self) -> Option<&HdIcons> {
        self.icons.as_deref()
    }
}

impl UiSpriteSource for UiSprites {
    fn sprite(&self, key: &str, tiled: bool) -> Option<Image> {
        if let Some(icons) = &self.icons {
            if let Some(image) = icons.sprite(key, tiled) {
                return Some(image);
            }
        }
        if !self.pack_ui || !key.starts_with(INTERFACE_PREFIX) || !benilla_assets::bc_supported() {
            return None;
        }
        let pack = self
            .pack
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()?;
        let (shape, chain) = pack.ui_texture(key)?;
        self.pack_served.fetch_add(1, Ordering::Relaxed);
        Some(bc7_sprite(shape, &chain, tiled))
    }
}

/// A pack chain as a UI sprite: BC7 read as sRGB, as the stock sprite's gamma bytes upload as
/// `Rgba8UnormSrgb` (`benilla_assets`' `sprite_image`), clamped (repeating when `tiled`), linear
/// with mips, so the 4x art minifies cleanly to the stock size it is laid out at.
pub(crate) fn bc7_sprite(shape: Bc7Shape, chain: &[u8], tiled: bool) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: shape.width,
            height: shape.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Bc7RgbaUnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(chain.to_vec());
    image.texture_descriptor.mip_level_count = shape.mips;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pack_sprite_is_a_full_srgb_bc7_chain() {
        let shape = Bc7Shape {
            width: 256,
            height: 64,
            mips: crate::dds::full_mips(256, 64),
        };
        let chain = vec![0u8; crate::dds::bc7_chain_bytes(256, 64)];
        let image = bc7_sprite(shape, &chain, true);
        assert_eq!(
            image.texture_descriptor.format,
            TextureFormat::Bc7RgbaUnormSrgb
        );
        assert_eq!(image.texture_descriptor.mip_level_count, 9);
        assert_eq!(image.data.as_ref().unwrap().len(), chain.len());
        let ImageSampler::Descriptor(s) = &image.sampler else {
            panic!("explicit sampler");
        };
        assert_eq!(s.address_mode_u, ImageAddressMode::Repeat);
    }
}
