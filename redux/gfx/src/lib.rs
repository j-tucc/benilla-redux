//! benilla-redux's graphics, ported from Elderoth's `elderoth-gfx`: the MSAA and anisotropic
//! filtering defaults ([`session_defaults`]), and [`GfxPlugin`], which carries the optional
//! effects. Classic, every effect off, draws benilla's image.
use bevy::prelude::*;
use std::path::Path;

mod defaults;

pub use defaults::session_defaults;

/// redux's graphics: every effect's plugin and the switches.
pub struct GfxPlugin;

impl Plugin for GfxPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, report_camera);
    }
}

/// One line when a world camera is spawned, naming the anti-aliasing it runs with (after the
/// GPU clamp and the player's saved settings) and the texture filter.
fn report_camera(
    cameras: Query<&Msaa, Added<benilla_world::view::WorldCamera>>,
    setting: Res<benilla_world::view::MsaaSetting>,
) {
    for msaa in &cameras {
        let tex = benilla_assets::tex_filter();
        info!(
            "redux graphics: world camera MSAA {}x (setting {}), anisotropy {}x (filter mode {})",
            msaa.samples(),
            setting.samples,
            tex.anisotropy_clamp(),
            tex.mode()
        );
    }
}

/// redux's own settings file in the state folder, beside benilla's `config.toml`.
pub fn settings_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join("redux-gfx.toml")
}
