//! benilla-redux's graphics, ported from Elderoth's `elderoth-gfx` (which ported most of them from
//! the everwood graphics fork of benilla; `redux/THIRD-PARTY.md`): the MSAA and anisotropic
//! filtering defaults ([`session_defaults`]), and [`GfxPlugin`], the optional Enhanced effects,
//! each behind its own switch ([`prefs`]).
//!
//! The engine opens generic hook points (REDUX.md's patch ledger): `benilla::gfx_hooks`, a WGSL
//! module of stock hook bodies its shaders call, at a fixed handle this crate may replace, and
//! `lighting::GfxData`, 64 rows of per-frame data appended to the shared light buffer. This
//! crate writes the rows ([`rows`]), replaces the hook bodies with its own, each effect behind its
//! own shader def (`hooks`), and runs its full-screen passes as render-graph nodes. Classic,
//! every switch off, keeps the engine's own hook module and runs no pass: benilla's image.
//!
//! [`Enhanced`] is the live set of switches: [`GfxSettings`] (`redux-gfx.toml`), or `$REDUX_GFX`
//! for a session (`classic`, `enhanced`, or a comma list of effect names).
use bevy::prelude::*;
use std::path::Path;

mod bloom;
mod defaults;
#[cfg(feature = "dlss")]
pub mod dlss;
mod firelight;
mod fog;
mod glow;
mod grading;
mod haze;
mod hooks;
pub mod locallights;
mod mist;
mod moonlight;
mod post;
pub mod prefs;
mod rays;
pub mod rows;
mod scatter;
mod shadows;
mod shafts;
mod sky;
mod ssao;
mod taa;
mod wetness;
mod wind;

pub use defaults::session_defaults;
use prefs::GraphicsPrefs;

/// The settings file's choice of effects (`redux-gfx.toml`); `$REDUX_GFX` overrides it.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Deref)]
pub struct GfxSettings(pub GraphicsPrefs);

/// The Enhanced effects that are on this frame.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Deref)]
pub struct Enhanced(pub GraphicsPrefs);

/// DLSS's quality mode while the anti-aliasing is DLSS: native resolution (DLAA) or one of the
/// upscaling modes, which draw the world smaller (`slDLSSGetOptimalSettings`) and upscale it to
/// the output. Follows [`GraphicsPrefs::dlss`].
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DlssQuality {
    #[default]
    Dlaa,
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
}

impl DlssQuality {
    /// Every mode, best image first.
    pub const ALL: [DlssQuality; 5] = [
        DlssQuality::Dlaa,
        DlssQuality::Quality,
        DlssQuality::Balanced,
        DlssQuality::Performance,
        DlssQuality::UltraPerformance,
    ];

    /// The mode's word in log lines and `$REDUX_GFX`.
    pub fn word(self) -> &'static str {
        match self {
            DlssQuality::Dlaa => "dlaa",
            DlssQuality::Quality => "dlss_q",
            DlssQuality::Balanced => "dlss_b",
            DlssQuality::Performance => "dlss_p",
            DlssQuality::UltraPerformance => "dlss_up",
        }
    }

    pub(crate) fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.word() == word)
    }

    /// The settings' mode; out of range is the last.
    fn of(prefs: &GraphicsPrefs) -> Self {
        Self::ALL[usize::from(prefs.dlss).min(Self::ALL.len() - 1)]
    }
}

/// Frame Generation: DLSS-G's 2x with Reflex low latency, in a build with the `dlss` feature on a
/// GPU that has it. `on` is the choice ([`GraphicsPrefs::frame_gen`]); `hold` is set each frame
/// while the client shows no world (glue screens, a loading screen), when DLSS-G must be off
/// (DLSS-G guide, section 6.4).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameGen {
    pub on: bool,
    pub hold: bool,
}

/// redux's graphics: every effect's plugin, the hook module and the switches.
pub struct GfxPlugin;

impl Plugin for GfxPlugin {
    fn build(&self, app: &mut App) {
        let config_dir = benilla_app_config_dir();
        app.init_resource::<GfxSettings>()
            .init_resource::<Enhanced>()
            .init_resource::<DlssQuality>()
            .init_resource::<FrameGen>()
            .insert_resource(prefs::SettingsFile::new(config_dir.as_deref()))
            .add_systems(PreUpdate, (prefs::follow_file, follow_settings).chain())
            .add_systems(Update, report_camera)
            .add_plugins(hooks::HooksPlugin)
            .add_plugins(grading::GradingPlugin)
            .add_plugins(bloom::BloomPlugin)
            .add_plugins(haze::HazePlugin)
            .add_plugins(fog::FogPlugin)
            .add_plugins(mist::MistPlugin)
            .add_plugins(sky::SkyPlugin)
            .add_plugins(wind::WindPlugin)
            .add_plugins(shadows::ShadowsPlugin)
            .add_plugins(moonlight::MoonlightPlugin)
            .add_plugins(shafts::ShaftsPlugin)
            .add_plugins(rays::RaysPlugin)
            .add_plugins(ssao::SsaoPlugin)
            .add_plugins(wetness::WetnessPlugin)
            .add_plugins(locallights::LocalLightsPlugin)
            .add_plugins(firelight::FirelightPlugin)
            .add_plugins(glow::GlowPlugin)
            .add_plugins(taa::TaaPlugin);
        #[cfg(feature = "dlss")]
        app.add_plugins((dlss::FramePlugin, dlss::SrPlugin, dlss::FgPlugin))
            .add_systems(PreUpdate, frame_gen_bridge);
    }
}

/// benilla's state folder, where `redux-gfx.toml` lives; `None` when the run persists nothing.
fn benilla_app_config_dir() -> Option<std::path::PathBuf> {
    STATE_DIR.get().cloned().flatten()
}

/// The state folder the launcher resolved ([`set_state_dir`]).
static STATE_DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

/// Records benilla's state folder (`benilla_app::config_dir()`), for `redux-gfx.toml`. The
/// launcher calls it once, before the app is built.
pub fn set_state_dir(dir: Option<std::path::PathBuf>) {
    let _ = STATE_DIR.set(dir);
}

/// `$REDUX_GFX`, read once.
fn forced() -> Option<GraphicsPrefs> {
    prefs::parse_forced(&std::env::var("REDUX_GFX").ok()?)
}

/// Mirrors the settings (or `$REDUX_GFX`) into [`Enhanced`], [`DlssQuality`] and [`FrameGen`].
fn follow_settings(
    settings: Res<GfxSettings>,
    mut enhanced: ResMut<Enhanced>,
    mut quality: ResMut<DlssQuality>,
    mut frame_gen: ResMut<FrameGen>,
    mut env: Local<Option<Option<GraphicsPrefs>>>,
) {
    let forced = *env.get_or_insert_with(|| {
        let f = forced();
        if let Some(f) = f {
            info!("REDUX_GFX: graphics forced to {f:?}");
        }
        f
    });
    let want = forced.unwrap_or(settings.0);
    if enhanced.0 != want {
        info!("redux graphics: {want:?}");
        enhanced.0 = want;
    }
    let q = DlssQuality::of(&want);
    if *quality != q {
        *quality = q;
    }
    if frame_gen.on != want.frame_gen() {
        frame_gen.on = want.frame_gen();
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

/// Frames Frame Generation must have been off before the window goes back to its vsync present
/// mode: DLSS-G is off for the swap chain's recreation. A few presents (Elderoth's value).
#[cfg(feature = "dlss")]
const VSYNC_BACK_AFTER: u32 = 10;

/// Holds Frame Generation off while the client shows no world: no active world camera (the glue
/// screens), or a world still streaming in (DLSS-G guide, section 6.4). While it runs the window
/// presents without vsync, which DLSS-G requires (Reflex paces the frames), and the vsync mode it
/// had comes back once it is off. Ported from Elderoth's `gfx_bridge::frame_gen`.
#[cfg(feature = "dlss")]
fn frame_gen_bridge(
    mut frame_gen: ResMut<FrameGen>,
    cameras: Query<&Camera, With<benilla_world::view::WorldCamera>>,
    progress: Option<Res<benilla_world::terrain_stream::WorldLoadProgress>>,
    mut window: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    mut vsync: Local<Option<bevy::window::PresentMode>>,
    mut off_for: Local<u32>,
) {
    use bevy::window::PresentMode;
    let world = cameras.iter().any(|c| c.is_active) && progress.is_some_and(|p| p.presentable());
    let hold = !world;
    if frame_gen.hold != hold {
        frame_gen.hold = hold;
    }
    let Ok(mut window) = window.single_mut() else {
        return;
    };
    if frame_gen.on && dlss::fg_ready() {
        *off_for = 0;
        if matches!(
            window.present_mode,
            PresentMode::Fifo | PresentMode::FifoRelaxed | PresentMode::AutoVsync
        ) {
            *vsync = Some(window.present_mode);
            window.present_mode = PresentMode::AutoNoVsync;
        }
    } else if vsync.is_some() {
        *off_for += 1;
        if *off_for >= VSYNC_BACK_AFTER {
            window.present_mode = vsync.take().unwrap_or_default();
        }
    }
}
