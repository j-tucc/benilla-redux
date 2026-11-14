//! benilla-redux's HD textures, ported from Elderoth's `benilla-adapter`: local BC7 texture packs
//! ([`catalog`]) served through benilla's terrain-layer and world-texture source hooks, switchable
//! while the client runs ([`switch`]), and HD icons ([`icons`]) through its UI sprite source hook
//! (REDUX.md's patch ledger). Nothing here is Blizzard's or committed: packs and icons are read
//! from a local folder, `classic-overrides/` beside or above the checkout (or the binary) unless
//! `$REDUX_TEXTURE_PACKS` / `$REDUX_HD_ICONS` name another.
//!
//! The choice lives in `redux-textures.toml` in benilla's state folder:
//!
//! ```toml
//! texture_pack = "realesrgan-x4plus-4x"   # or "vanilla" (the default)
//! hd_icons = true                         # default false
//! ```
//!
//! [`boot`] installs the startup choice before the app is built, so the first tile already reads
//! it; [`PacksPlugin`] follows the file, switching the pack live. HD icons follow it on restart.
//! Character skins stay stock: they are composited on the CPU from the archives.
use bevy::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod catalog;
mod dds;
mod icons;
pub mod switch;

pub use catalog::{ActivePack, PackCatalog, VANILLA};

/// The shared overrides folder's name, looked for beside and above the start folders.
const OVERRIDES: &str = "classic-overrides";

/// `redux-textures.toml`, every field optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct TextureSettings {
    pub texture_pack: Option<String>,
    pub hd_icons: Option<bool>,
}

impl TextureSettings {
    /// The settings file in `config_dir`; a missing or unreadable file is the defaults.
    pub fn read(config_dir: Option<&Path>) -> Self {
        config_dir
            .and_then(|d| std::fs::read_to_string(settings_path(d)).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }
}

/// redux's texture settings file in the state folder, beside benilla's `config.toml`.
pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("redux-textures.toml")
}

/// An unattended run (a probe): it never boots the saved pack or HD icons.
pub(crate) fn unattended() -> bool {
    std::env::var("WOW_UNATTENDED").as_deref() == Ok("1")
}

/// `classic-overrides/<name>` in one of `starts` or the nearest folder above it that has one.
pub(crate) fn overrides_dir(starts: &[PathBuf], name: &str) -> Option<PathBuf> {
    starts.iter().find_map(|start| {
        start
            .ancestors()
            .map(|dir| dir.join(OVERRIDES).join(name))
            .find(|dir| dir.is_dir())
    })
}

/// What [`boot`] chose, for the plugin.
#[derive(Resource, Clone)]
pub struct TextureBoot {
    pub catalog: PackCatalog,
    pub active: ActivePack,
    pub config_dir: Option<PathBuf>,
    icons: Option<Arc<icons::HdIcons>>,
}

/// Discovers the packs and icons from `starts` (the checkout, the binary's folder), installs the
/// startup choices into benilla's hooks, and returns them for [`PacksPlugin`]. Call once, before
/// the app is built.
pub fn boot(config_dir: Option<PathBuf>, starts: &[PathBuf]) -> TextureBoot {
    let saved = TextureSettings::read(config_dir.as_deref());
    let (catalog, active) =
        catalog::boot(catalog::packs_root(starts), saved.texture_pack.as_deref());
    let root = icons::resolve_root(
        std::env::var("REDUX_HD_ICONS").ok().as_deref(),
        saved.hd_icons.unwrap_or(false),
        unattended(),
        overrides_dir(starts, "ICONS"),
    );
    let icons = root.map(|dir| Arc::new(icons::HdIcons::spawn(dir)));
    benilla_assets::set_ui_sprite_source(
        icons
            .clone()
            .map(|i| i as Arc<dyn benilla_assets::UiSpriteSource>),
    );
    eprintln!("HD_ICONS {}", if icons.is_some() { "on" } else { "off" });
    TextureBoot {
        catalog,
        active,
        config_dir,
        icons,
    }
}

/// The pack switch, the settings file's follower and the probes.
pub struct PacksPlugin(pub TextureBoot);

impl Plugin for PacksPlugin {
    fn build(&self, app: &mut App) {
        let boot = self.0.clone();
        app.insert_resource(boot.catalog.clone())
            .insert_resource(boot.active.clone())
            .insert_resource(switch::SettingsFollower::new(boot.config_dir.as_deref()))
            .insert_resource(boot)
            .add_plugins(switch::PackSwitchPlugin)
            .add_systems(Update, (catalog::texture_probe, icon_probe));
    }
}

/// `$REDUX_TEXTURE_PROBE=<seconds>`: how many HD icons were served, at that time.
fn icon_probe(time: Res<Time<Real>>, boot: Res<TextureBoot>, mut done: Local<bool>) {
    let Some(at) = std::env::var("REDUX_TEXTURE_PROBE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    else {
        return;
    };
    if *done || time.elapsed_secs() < at {
        return;
    }
    *done = true;
    match &boot.icons {
        Some(icons) => eprintln!(
            "HD_ICONS_PROBE served={} failures={}",
            icons.served.load(std::sync::atomic::Ordering::Relaxed),
            icons.failures.load(std::sync::atomic::Ordering::Relaxed)
        ),
        None => eprintln!("HD_ICONS_PROBE off"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overrides_folder_is_found_above_a_start() {
        let root = std::env::temp_dir().join(format!("redux-overrides-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let deep = root.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(root.join("classic-overrides/ICONS")).unwrap();
        assert_eq!(
            overrides_dir(std::slice::from_ref(&deep), "ICONS"),
            Some(root.join("classic-overrides/ICONS"))
        );
        assert_eq!(overrides_dir(&[deep], "texture-packs"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_settings_file_reads_and_defaults() {
        let dir = std::env::temp_dir().join(format!("redux-tex-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            TextureSettings::read(Some(&dir)),
            TextureSettings::default()
        );
        std::fs::write(
            settings_path(&dir),
            "texture_pack = \"Up\"\nhd_icons = true\n",
        )
        .unwrap();
        let s = TextureSettings::read(Some(&dir));
        assert_eq!(s.texture_pack.as_deref(), Some("Up"));
        assert_eq!(s.hd_icons, Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
