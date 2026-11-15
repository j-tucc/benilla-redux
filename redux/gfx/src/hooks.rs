//! The hook bodies (LOOK-12): this crate's `benilla::gfx_hooks` module ([`hooks.wgsl`](hooks.wgsl))
//! replaces the engine's stock one at `materials::GFX_HOOKS_SHADER` (engine patch 0009), with one
//! shader def per effect that is on. Classic puts the engine's own module back, so its pipelines
//! compile exactly the stock code. Every pipeline that imports the module recompiles once on a
//! switch: a settings change, never a per-frame cost.
use super::Enhanced;
use crate::prefs::GraphicsPrefs;
use benilla_assets::materials::{GFX_HOOKS_SHADER, GFX_HOOKS_WGSL};
use bevy::prelude::*;
use bevy::shader::ShaderDefVal;

pub(super) struct HooksPlugin;

impl Plugin for HooksPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AssetPlugin>() {
            return;
        }
        bevy::shader::load_shader_library!(app, "rows.wgsl");
        bevy::shader::load_shader_library!(app, "fog.wgsl");
        bevy::shader::load_shader_library!(app, "wind.wgsl");
        bevy::shader::load_shader_library!(app, "sky.wgsl");
        bevy::shader::load_shader_library!(app, "shadow.wgsl");
        bevy::shader::load_shader_library!(app, "moon.wgsl");
        bevy::shader::load_shader_library!(app, "wet.wgsl");
        bevy::shader::load_shader_library!(app, "fire.wgsl");
        app.add_systems(PostUpdate, install);
    }
}

/// The shader defs for the effects that run inside the engine's shaders; empty under Classic.
fn defs(p: &GraphicsPrefs) -> Vec<ShaderDefVal> {
    [
        ("GFX_FOG", p.fog),
        ("GFX_WIND", p.wind),
        ("GFX_SKY", p.sky),
        ("GFX_SHADOWS", p.shadows),
        ("GFX_MOONLIGHT", p.moonlight),
        ("GFX_WET", p.wetness),
        ("GFX_FIRE", p.firelight),
    ]
    .into_iter()
    .filter(|(_, on)| *on)
    .map(|(name, _)| ShaderDefVal::Bool(name.into(), true))
    .collect()
}

/// The libraries the override imports. It goes in only once they have all loaded: a module
/// installed while one of its imports is missing is parked by Bevy's shader cache, and the
/// pipelines that import it are not queued again when the import arrives.
const LIBRARIES: &[&str] = &[
    "redux::gfx_rows",
    "redux::gfx_fog",
    "redux::gfx_wind",
    "redux::gfx_sky",
    "redux::gfx_shadow",
    "redux::gfx_moon",
    "redux::gfx_wet",
    "redux::gfx_fire",
];

fn libraries_loaded(shaders: &Assets<Shader>) -> bool {
    LIBRARIES.iter().all(|path| {
        shaders
            .iter()
            .any(|(_, s)| s.import_path().module_name().as_str() == *path)
    })
}

fn install(
    enhanced: Res<Enhanced>,
    mut shaders: ResMut<Assets<Shader>>,
    mut installed: Local<Option<Vec<ShaderDefVal>>>,
) {
    let want = defs(&enhanced);
    if installed.as_ref() == Some(&want) {
        return;
    }
    if !want.is_empty() && !libraries_loaded(&shaders) {
        return;
    }
    // At start-up with nothing on, the engine's module is already in place.
    if installed.is_none() && want.is_empty() {
        *installed = Some(want);
        return;
    }
    let shader = if want.is_empty() {
        Shader::from_wgsl(GFX_HOOKS_WGSL, "benilla_assets/shaders/gfx_hooks.wgsl")
    } else {
        let mut s = Shader::from_wgsl(include_str!("hooks.wgsl"), "redux_gfx/hooks.wgsl");
        s.shader_defs = want.clone();
        s
    };
    info!("Enhanced graphics hooks: {want:?}");
    let _ = shaders.insert(GFX_HOOKS_SHADER.id(), shader);
    *installed = Some(want);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_asks_for_no_defs() {
        assert!(defs(&GraphicsPrefs::default()).is_empty());
        assert_eq!(defs(&GraphicsPrefs::ENHANCED).len(), 7);
    }

    /// Bevy queues a pipeline only once every import its modules name is registered. An import
    /// alias that matches a hook's own name (`gfx_fog::` inside `fn gfx_fog`) makes naga_oil
    /// report a phantom `elderoth` import, which parks every world pipeline: the world never draws.
    #[test]
    fn the_override_names_only_real_libraries() {
        let shader = Shader::from_wgsl(include_str!("hooks.wgsl"), "hooks.wgsl");
        let mut imports: Vec<String> = shader
            .imports()
            .map(|i| i.module_name().into_owned())
            .collect();
        imports.sort();
        let mut want: Vec<String> = LIBRARIES.iter().map(|s| s.to_string()).collect();
        want.push("benilla::gfx_data".into());
        want.sort();
        assert_eq!(imports, want);
    }

    #[test]
    fn the_override_keeps_the_engine_signatures() {
        let ours = include_str!("hooks.wgsl");
        for line in GFX_HOOKS_WGSL
            .lines()
            .filter(|l| l.starts_with("fn ") || l.starts_with("const ") || l.starts_with("#define"))
        {
            let head = line.split("//").next().unwrap().trim_end();
            assert!(ours.contains(head), "missing: {head}");
        }
    }
}
