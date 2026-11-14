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

fn main() -> benilla_app::AppExit {
    let project_dir = project_dir();
    // Resolve the state folder from the redux checkout before the app is built, so redux's
    // session defaults can read the player's saved settings (`run_with` records the same folder).
    benilla_formats::set_project_folder(project_dir);
    redux_gfx::session_defaults(benilla_app::config_dir().as_deref());
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
        app.add_plugins(redux_gfx::GfxPlugin);
    })
}
