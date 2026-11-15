//! `benilla-cursor`: the benilla client with [`redux_cursor::CursorSizePlugin`] on top, for running
//! the Cursor Size option on its own (and, with `REDUX_HEADLESS=1`, hidden, as
//! `redux/headless/shoot.py --exe` runs it). A redux launcher adds the plugin in its `run_with` closure the
//! same way (`redux/cursor/README.md`).

use benilla_app::BuildId;

fn main() -> benilla_app::AppExit {
    let build = BuildId {
        version: env!("CARGO_PKG_VERSION"),
        describe: env!("BENILLA_GIT_DESCRIBE"),
        sha: env!("BENILLA_GIT_SHA"),
        short: env!("BENILLA_GIT_SHORT"),
        date: env!("BENILLA_GIT_DATE"),
        profile: env!("BENILLA_PROFILE"),
        project_dir: env!("BENILLA_PROJECT_DIR"),
        ..Default::default()
    };
    benilla_app::run_with(build, |app| {
        app.add_plugins(redux_cursor::CursorSizePlugin {
            state_dir: benilla_app::config_dir(),
        })
        // Inert unless `REDUX_HEADLESS=1`, as `redux/headless/shoot.py --exe` runs it.
        .add_plugins(redux_headless::HeadlessPlugin);
    })
}
