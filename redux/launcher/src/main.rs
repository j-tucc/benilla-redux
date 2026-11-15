//! The `benilla-redux` launcher: benilla's client, started through [`benilla_app::run_with`], with
//! redux's own plugins on top (REDUX.md, rule 1).

use benilla_app::BuildId;

/// The folder a dev build keeps its `WoW` link, `benilla-config/` and `.probe-identity` in. The
/// build stamp names this package's workspace root, `redux/`; the checkout above it is the one
/// benilla's own launcher uses, so both binaries share one state folder and one install link.
/// Empty in a player build, which looks in no source tree.
fn project_dir() -> &'static str {
    let stamped = env!("BENILLA_PROJECT_DIR");
    match std::path::Path::new(stamped).parent() {
        Some(checkout) if !stamped.is_empty() => {
            Box::leak(checkout.display().to_string().into_boxed_str())
        }
        _ => stamped,
    }
}

/// Creates the Vulkan device through NVIDIA Streamline when its SDK is present (`redux-gfx`'s
/// `dlss`), and hands it to benilla's boot; otherwise Bevy creates its own and DLSS draws as TAA.
/// The SDK is looked for under `overrides/streamline/` beside the binary, then in `redux/` of a
/// dev checkout.
#[cfg(feature = "dlss")]
fn streamline_device(project_dir: &str, state_dir: Option<&std::path::Path>) {
    let mut roots = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Into::into))
    {
        roots.push(dir);
    }
    if !project_dir.is_empty() {
        roots.push(std::path::Path::new(project_dir).join("redux"));
    }
    let logs = state_dir.map_or_else(
        || std::env::temp_dir().join("benilla-redux-streamline"),
        |d| d.join("logs/streamline"),
    );
    if let Some(creation) = redux_gfx::dlss::render_creation(&logs, &roots) {
        if let Ok(mut slot) = benilla_world::boot::RENDER_CREATION.lock() {
            *slot = Some(creation);
        }
    }
}

/// Where the shared `classic-overrides/` folder (texture packs, HD icons) is looked for: beside or
/// above the checkout, then the binary's folder.
fn override_starts(project_dir: &str) -> Vec<std::path::PathBuf> {
    let mut starts = Vec::new();
    if !project_dir.is_empty() {
        starts.push(project_dir.into());
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Into::into))
    {
        starts.push(dir);
    }
    starts
}

fn main() -> benilla_app::AppExit {
    let project_dir = project_dir();
    // Resolve the state folder from the redux checkout before the app is built, so redux's
    // session defaults can read the player's saved settings (`run_with` records the same folder).
    benilla_formats::set_project_folder(project_dir);
    let state_dir = benilla_app::config_dir();
    redux_gfx::session_defaults(state_dir.as_deref());
    #[cfg(feature = "dlss")]
    streamline_device(project_dir, state_dir.as_deref());
    let textures = redux_packs::boot(state_dir.clone(), &override_starts(project_dir));
    let smooth = redux_smooth::SmoothPlugin {
        config_dir: state_dir.clone(),
    };
    let cursor = redux_cursor::CursorSizePlugin {
        state_dir: state_dir.clone(),
    };
    redux_gfx::set_state_dir(state_dir);
    let build = BuildId {
        version: env!("CARGO_PKG_VERSION"),
        describe: env!("BENILLA_GIT_DESCRIBE"),
        sha: env!("BENILLA_GIT_SHA"),
        short: env!("BENILLA_GIT_SHORT"),
        date: env!("BENILLA_GIT_DATE"),
        profile: env!("BENILLA_PROFILE"),
        project_dir,
        ..Default::default()
    };
    benilla_app::run_with(build, |app| {
        app.add_plugins(redux_gfx::GfxPlugin)
            .add_plugins(redux_packs::PacksPlugin(textures))
            .add_plugins(smooth)
            .add_plugins(cursor)
            .add_plugins(redux_headless::HeadlessPlugin);
    })
}
