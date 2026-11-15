//! benilla-redux's Cursor Size: the 1.12 cursor set (`Interface\Cursor\*.blp`, 32×32), every held
//! payload icon and every `SetCursor` picture, drawn at the player's chosen size.
//!
//! benilla hands the window a 32×32 hardware cursor (`benilla-app/src/cursor.rs`). Windows and X11
//! draw a custom cursor at its pixel size whatever the display's DPI, so at 200% it is half the
//! size of the system arrow. [`CursorSizePlugin`] sits after benilla's cursor driver: whenever the
//! window's [`CursorIcon`] is one benilla set, it swaps in a filtered resize of the same picture
//! with its hotspot moved to match ([`scale`]), cached per picture and size. benilla's driver never
//! sees the swap (its own memo of what it last set is unchanged), so every cursor state (point,
//! attack, loot, talk, interact, cast, the `Unable` variants, a held item, a custom cursor) is
//! scaled the same way, and a size change rescales the one showing at once.
//!
//! The choice is [`prefs::CursorSize`]; how it is set and kept is in [`prefs`]. No upstream file
//! is changed. macOS builds its cursors as `NSCursor`s (sized in points, so already DPI-correct)
//! and the plugin does nothing there.

pub mod prefs;
pub mod scale;

use bevy::prelude::*;
use bevy::window::CursorIcon;
use prefs::CursorSize;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// Adds the Cursor Size option. `state_dir` is benilla's state folder (`benilla_app::config_dir`),
/// where `redux-cursor.toml` lives; `None` keeps the choice for the session only.
pub struct CursorSizePlugin {
    pub state_dir: Option<PathBuf>,
}

impl Plugin for CursorSizePlugin {
    fn build(&self, app: &mut App) {
        let path = self.state_dir.as_deref().map(prefs::settings_path);
        let saved = path.as_deref().map(prefs::load).unwrap_or_default();
        let forced = std::env::var("REDUX_CURSOR_SIZE").ok().and_then(|v| {
            let parsed = CursorSize::parse(&v);
            if parsed.is_none() {
                warn!("REDUX_CURSOR_SIZE={v:?}: not a cursor size, using the saved one");
            }
            parsed
        });
        let size = forced.unwrap_or(saved);
        info!("cursor size: {} (saved {})", size.label(), saved.label());
        app.insert_resource(CursorSizeSetting {
            size,
            forced: forced.is_some(),
            mtime: path.as_deref().and_then(prefs::modified),
            path,
            polled: None,
            cvar_seen: None,
        })
        .init_resource::<ScaledCursors>();
        #[cfg(not(target_os = "macos"))]
        app.add_systems(
            PostUpdate,
            (poll_file, cvar_bridge, scale_window_cursor).chain(),
        );
    }
}

/// The size in force, and where it is kept.
#[derive(Resource, Debug)]
pub struct CursorSizeSetting {
    size: CursorSize,
    /// `$REDUX_CURSOR_SIZE` chose it: the file is not re-read (a menu change still applies).
    forced: bool,
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    polled: Option<Instant>,
    /// The `reduxCursorSize` CVar's value last seen in the UI VM.
    cvar_seen: Option<String>,
}

impl CursorSizeSetting {
    /// The choice in force.
    pub fn size(&self) -> CursorSize {
        self.size
    }

    /// Choose `size` and save it (a menu's change). Not saved when there is no state folder.
    pub fn set(&mut self, size: CursorSize) {
        if self.size == size {
            return;
        }
        self.size = size;
        info!("cursor size: {}", size.label());
        if let Some(path) = &self.path {
            match prefs::save(path, size) {
                Ok(()) => self.mtime = prefs::modified(path),
                Err(e) => warn!("cursor size: could not save {}: {e}", path.display()),
            }
        }
    }
}

/// Re-read `redux-cursor.toml` when it changes, twice a second.
fn poll_file(mut setting: ResMut<CursorSizeSetting>) {
    let now = Instant::now();
    if setting.forced
        || setting
            .polled
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(500))
    {
        return;
    }
    setting.polled = Some(now);
    let Some(path) = setting.path.clone() else {
        return;
    };
    let mtime = prefs::modified(&path);
    if mtime == setting.mtime {
        return;
    }
    setting.mtime = mtime;
    let size = prefs::load(&path);
    if size != setting.size {
        info!("cursor size: {} (from {})", size.label(), path.display());
        setting.size = size;
    }
}

/// The options addon's hook: the `reduxCursorSize` CVar. When an addon has registered it, the
/// first sight seeds it with the size in force (so the addon's `GetCVar` shows the choice), and
/// every later change of its value is adopted and saved.
fn cvar_bridge(
    mut setting: ResMut<CursorSizeSetting>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Some(value) = script.cvar(prefs::CVAR) else {
        // Not registered (no options addon, or a fresh VM before it loads): look again later.
        setting.cvar_seen = None;
        return;
    };
    if setting.cvar_seen.as_deref() == Some(value.as_str()) {
        return;
    }
    match (&setting.cvar_seen, CursorSize::parse(&value)) {
        (Some(_), Some(size)) => {
            setting.set(size);
            setting.cvar_seen = Some(value);
        }
        // First sight, or an unreadable value: show the size in force.
        _ => {
            let word = setting.size.word();
            if value != word {
                script.set_cvar_host(prefs::CVAR, &word);
            }
            setting.cvar_seen = Some(word);
        }
    }
}

/// A cursor picture and its hotspot.
type Picture = (Handle<Image>, (u16, u16));

/// Every resize made, and where each came from.
#[derive(Resource, Default)]
struct ScaledCursors {
    /// `(source image, side)` → the resized image and its hotspot.
    made: bevy::platform::collections::HashMap<(AssetId<Image>, u32), Picture>,
    /// A resized image → its source and the source's hotspot.
    origin: bevy::platform::collections::HashMap<AssetId<Image>, Picture>,
}

/// Swap the window's custom cursor for its resize at the size in force.
fn scale_window_cursor(
    setting: Res<CursorSizeSetting>,
    mut window: Query<(&Window, &mut CursorIcon), With<bevy::window::PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut scaled: ResMut<ScaledCursors>,
) {
    use bevy::window::{CustomCursor, CustomCursorImage};
    let Ok((win, mut icon)) = window.single_mut() else {
        return;
    };
    let CursorIcon::Custom(CustomCursor::Image(shown)) = &*icon else {
        return;
    };
    // A sub-rect or atlas cursor is not one of benilla's; leave it alone.
    if shown.rect.is_some() || shown.texture_atlas.is_some() {
        return;
    }
    let (source, hotspot) = scaled
        .origin
        .get(&shown.handle.id())
        .cloned()
        .unwrap_or_else(|| (shown.handle.clone(), shown.hotspot));
    let side = setting.size.pixels(win.resolution.base_scale_factor());
    let key = (source.id(), side);
    let made = match scaled.made.get(&key) {
        Some(m) => Some(m.clone()),
        None => resize(&images, &source, hotspot, side).map(|(image, hot)| {
            let handle = images.add(image);
            scaled.made.insert(key, (handle.clone(), hot));
            scaled.origin.insert(handle.id(), (source.clone(), hotspot));
            dump(&images, &source, &handle, side, hot);
            (handle, hot)
        }),
    };
    // No pixels to read (not loaded): show the source as it is.
    let (handle, hot) = made.unwrap_or_else(|| (source.clone(), hotspot));
    if shown.handle == handle && shown.hotspot == hot {
        return;
    }
    if trace() {
        let (w, h) = images
            .get(&handle)
            .map_or((0, 0), |i| (i.width(), i.height()));
        info!(
            "cursor size: showing {:?} at {w}x{h}, hotspot {hot:?} (source {:?}, hotspot {hotspot:?})",
            handle.id(),
            source.id()
        );
    }
    let mut next: CustomCursorImage = shown.clone();
    next.handle = handle;
    next.hotspot = hot;
    *icon = CursorIcon::Custom(CustomCursor::Image(next));
}

/// `$REDUX_CURSOR_LOG`: log every cursor the window is handed.
fn trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("REDUX_CURSOR_LOG").is_some())
}

/// `$REDUX_CURSOR_DUMP=<dir>`: write each source and its resize as PNGs, to look at the filter
/// and the hotspot (a red texel) without a screen capture, which leaves the hardware cursor out.
fn dump(
    images: &Assets<Image>,
    source: &Handle<Image>,
    made: &Handle<Image>,
    side: u32,
    hot: (u16, u16),
) {
    let Some(dir) = std::env::var_os("REDUX_CURSOR_DUMP").map(PathBuf::from) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let stem = format!("{:?}", source.id()).replace(|c: char| !c.is_ascii_alphanumeric(), "");
    for (handle, name, mark) in [
        (source, format!("{stem}-src.png"), None),
        (made, format!("{stem}-{side}.png"), Some(hot)),
    ] {
        let Some(img) = images.get(handle) else {
            continue;
        };
        let Some(mut data) = img.data.clone() else {
            continue;
        };
        let (w, h) = (img.width(), img.height());
        if let Some((x, y)) = mark {
            let i = ((u32::from(y) * w + u32::from(x)) * 4) as usize;
            if let Some(px) = data.get_mut(i..i + 4) {
                px.copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        if let Err(e) = image::save_buffer(
            dir.join(&name),
            &data,
            w,
            h,
            image::ExtendedColorType::Rgba8,
        ) {
            warn!("cursor size: dump {name}: {e}");
        }
    }
}

/// The source image resized so a 32-texel side becomes `side` pixels, with its hotspot moved.
/// `None` when it is already that size or its pixels are unreadable: the source is shown as it is.
fn resize(
    images: &Assets<Image>,
    source: &Handle<Image>,
    hotspot: (u16, u16),
    side: u32,
) -> Option<(Image, (u16, u16))> {
    let image = images.get(source)?;
    let (w, h) = (image.width(), image.height());
    let data = image.data.as_ref()?;
    if w == 0 || h == 0 || data.len() != (w * h * 4) as usize {
        return None;
    }
    let dw = (w * side).div_ceil(32).max(1);
    let dh = (h * side).div_ceil(32).max(1);
    if (dw, dh) == (w, h) {
        return None; // already that size: the source is shown as it is
    }
    let rgba = scale::resize_rgba(w, h, data, dw, dh);
    let mut out = Image::new(
        bevy::render::render_resource::Extent3d {
            width: dw,
            height: dh,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        rgba,
        image.texture_descriptor.format,
        bevy::asset::RenderAssetUsages::MAIN_WORLD,
    );
    out.sampler = image.sampler.clone();
    Some((out, scale::scale_hotspot(hotspot, (w, h), (dw, dh))))
}
