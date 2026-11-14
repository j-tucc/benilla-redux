//! DLSS through NVIDIA Streamline (LOOK-15), compiled with the `dlss` feature: the Vulkan device
//! is created through Streamline's interposer, so its DLSS Super Resolution, Frame Generation
//! (DLSS-G) and Reflex plugins can work with what wgpu draws and presents.
//!
//! Nothing of Streamline is needed to build. At run time the SDK is the folder `REDUX_STREAMLINE`
//! names, else the newest `overrides/streamline/<tag>/` under one of the roots the launcher
//! passes (beside the binary, and `redux/` in a dev checkout; gitignored by `redux/.gitignore`:
//! the SDK's release zip extracted there), and its `bin/x64` holds the interposer and plugins.
//! `REDUX_STREAMLINE=off` keeps the plain device. A missing SDK, a failed `slInit` or a GPU
//! with neither DLSS nor DLSS-G falls back to Bevy's own device creation, logged, never a crash.
//!
//! [`render_creation`] does the work once at boot, before any Vulkan call, and prints the
//! `STREAMLINE` line; [`status`] keeps what it found. Once the device is up, [`sr`] runs DLSS
//! Super Resolution on a camera `taa.rs` puts under DLSS, [`frame`] carries Streamline's frame
//! tokens and sends the Reflex/PCL markers, [`fg`] runs Frame Generation while
//! [`crate::FrameGen`] is on, and [`in_flight`] holds the CPU to one frame ahead of the GPU while
//! it generates.
use bevy::render::settings::RenderCreation;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

mod device;
mod fg;
mod frame;
mod in_flight;
mod inputs;
mod sl;
mod sr;
mod vk_shim;

pub(crate) use fg::FgPlugin;
pub use fg::{stats as fg_stats, HudlessLabel, Stats as FgStats};
pub(crate) use frame::FramePlugin;
pub use frame::{pacing, Pacing};
pub(crate) use sr::SrPlugin;
pub use sr::{stats as sr_stats, Stats as SrStats};

/// What Streamline initialisation found, for the startup line and the `dlss_init` probe.
#[derive(Clone, Debug)]
pub enum Status {
    /// No SDK folder (or `REDUX_STREAMLINE=off`): the plain device, as without the feature.
    NoSdk(String),
    /// The SDK is there but the GPU has neither DLSS nor DLSS-G: the plain device.
    Unsupported { sdk: String, reason: String },
    /// The SDK is there but Streamline could not create the device: the plain device.
    Failed { sdk: String, reason: String },
    /// The device was created through the interposer.
    Ready {
        sdk: String,
        adapter: String,
        luid: Option<[u8; 8]>,
        dlss: Support,
        dlssg: Support,
        reflex: Support,
        pcl: Support,
        /// DLSS-G's plugin version and its NGX library's.
        dlssg_version: Option<String>,
        /// DLSS's plugin version and its NGX library's.
        dlss_version: Option<String>,
        /// DLSS-G's requirement flags beyond the API (`sl::FeatureRequirementFlags`).
        dlssg_needs: String,
    },
}

/// `slIsFeatureSupported` on the device's adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Support(sl::SlResult);

impl Support {
    pub fn supported(self) -> bool {
        self.0.ok()
    }
}

impl std::fmt::Display for Support {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.supported() {
            f.write_str("true")
        } else {
            write!(f, "false({})", self.0.name())
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::NoSdk(why) => write!(f, "status=no-sdk reason={why:?}"),
            Status::Unsupported { sdk, reason } => {
                write!(f, "status=unsupported sdk={sdk} reason={reason:?}")
            }
            Status::Failed { sdk, reason } => {
                write!(f, "status=failed sdk={sdk} reason={reason:?}")
            }
            Status::Ready {
                sdk,
                adapter,
                luid,
                dlss,
                dlssg,
                reflex,
                pcl,
                dlssg_version,
                dlss_version,
                dlssg_needs,
            } => {
                let luid = luid.map_or("none".into(), |l| {
                    l.iter()
                        .rev()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                });
                write!(
                    f,
                    "status=ready sdk={sdk} adapter={adapter:?} luid={luid} dlss={dlss} \
                     dlssg={dlssg} reflex={reflex} pcl={pcl} dlss_version={} dlssg_version={} \
                     dlssg_needs={dlssg_needs}",
                    dlss_version.as_deref().unwrap_or("none"),
                    dlssg_version.as_deref().unwrap_or("none"),
                )
            }
        }
    }
}

static STATUS: OnceLock<Status> = OnceLock::new();
/// The interposer's exports, once the device was created through it.
static STREAMLINE: OnceLock<&'static sl::Streamline> = OnceLock::new();

fn streamline() -> Option<&'static sl::Streamline> {
    STREAMLINE.get().copied()
}

/// Whether DLSS Super Resolution can run: the device was created through Streamline, the GPU
/// supports DLSS, and it has not been given up after failing.
pub fn sr_ready() -> bool {
    matches!(STATUS.get(), Some(Status::Ready { dlss, .. }) if dlss.supported())
        && streamline().is_some()
        && !sr::broken()
}

/// Whether Frame Generation can run: the device was created through Streamline, the GPU supports
/// DLSS-G, Reflex and PCL, and DLSS-G has not been given up after failing.
pub fn fg_ready() -> bool {
    matches!(
        STATUS.get(),
        Some(Status::Ready { dlssg, reflex, pcl, .. })
            if dlssg.supported() && reflex.supported() && pcl.supported()
    ) && streamline().is_some()
        && !fg::broken()
}

/// What [`render_creation`] found, once it has run.
pub fn status() -> Option<&'static Status> {
    STATUS.get()
}

/// The render device created through Streamline, for `RenderPlugin::render_creation`, or `None`
/// to let Bevy create its own. Call once, at boot, before anything touches Vulkan; Streamline
/// writes its logs into `log_dir`. Prints the `STREAMLINE` line either way.
pub fn render_creation(log_dir: &Path, roots: &[PathBuf]) -> Option<RenderCreation> {
    let (status, creation) = match sdk_dir(roots) {
        Err(why) => (Status::NoSdk(why), None),
        Ok(sdk) => {
            let tag = sdk
                .file_name()
                .map_or("?".into(), |n| n.to_string_lossy().into_owned());
            match start(&sdk, &tag, log_dir) {
                Ok((status, creation)) => (status, Some(creation)),
                Err(device::OpenError::Unsupported(reason)) => {
                    (Status::Unsupported { sdk: tag, reason }, None)
                }
                Err(device::OpenError::Failed(reason)) => {
                    (Status::Failed { sdk: tag, reason }, None)
                }
            }
        }
    };
    eprintln!("STREAMLINE {status}");
    let _ = STATUS.set(status);
    creation
}

/// `REDUX_STREAMLINE`, else the newest `<root>/overrides/streamline/*/` holding the interposer.
fn sdk_dir(roots: &[PathBuf]) -> Result<PathBuf, String> {
    match std::env::var_os("REDUX_STREAMLINE") {
        Some(v) if v == "off" => Err("REDUX_STREAMLINE=off".into()),
        Some(v) => Ok(PathBuf::from(v)),
        None => {
            let folders: Vec<PathBuf> = roots
                .iter()
                .map(|r| r.join("overrides/streamline"))
                .collect();
            folders
                .iter()
                .filter_map(|f| std::fs::read_dir(f).ok())
                .flatten()
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| interposer(p).is_file())
                .max_by_key(|p| version_key(p))
                .ok_or_else(|| {
                    let names: Vec<String> =
                        folders.iter().map(|f| f.display().to_string()).collect();
                    format!("no SDK in {}", names.join(" or "))
                })
        }
    }
}

fn interposer(sdk: &Path) -> PathBuf {
    sdk.join("bin/x64/sl.interposer.dll")
}

/// A folder name's numbers, in order (`v2.14.16` sorts after `v2.9.0`).
fn version_key(path: &Path) -> Vec<u64> {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// The features loaded at `slInit`: DLSS Super Resolution, Frame Generation, Reflex and its
/// latency stats (PCL), which Frame Generation requires (DLSS-G guide, section 8).
const FEATURES: [sl::Feature; 4] = [
    sl::FEATURE_DLSS,
    sl::FEATURE_DLSS_G,
    sl::FEATURE_REFLEX,
    sl::FEATURE_PCL,
];

fn start(
    sdk: &Path,
    tag: &str,
    log_dir: &Path,
) -> Result<(Status, RenderCreation), device::OpenError> {
    if !cfg!(windows) {
        return Err(device::OpenError::Unsupported(
            "Streamline's Vulkan interposer is Windows-only".into(),
        ));
    }
    let dll = interposer(sdk);
    if !dll.is_file() {
        return Err(format!("no {}", dll.display()).into());
    }
    // The interposer and its plugins stay loaded for the process (`Streamline` is leaked): the
    // device's Vulkan calls go through them until exit.
    let sl: &'static sl::Streamline = Box::leak(Box::new(sl::Streamline::load(&dll)?));
    let plugins = wide(&sdk.join("bin/x64"));
    let plugin_paths = [plugins.as_ptr()];
    let _ = std::fs::create_dir_all(log_dir);
    let logs = wide(log_dir);
    let engine_version = c"benilla-redux";
    // A fixed id for this project, as the guide asks when there is no NVIDIA application id.
    let project_id = c"6f1d3c7e-5b0a-4f6e-9d1b-8e3a2c4f7b19";
    let pref = sl::Preferences {
        paths_to_plugins: plugin_paths.as_ptr(),
        num_paths_to_plugins: 1,
        path_to_logs_and_data: logs.as_ptr(),
        log_message_callback: Some(log_message),
        // No over-the-air updates: the plugins are the SDK's in `overrides/`, nothing downloaded.
        // Global tags name their frame: the render thread tags a frame behind the main thread.
        flags: sl::PREFERENCE_DISABLE_CL_STATE_TRACKING | sl::PREFERENCE_FRAME_BASED_TAGGING,
        features_to_load: FEATURES.as_ptr(),
        num_features_to_load: FEATURES.len() as u32,
        engine_version: engine_version.as_ptr(),
        project_id: project_id.as_ptr(),
        ..Default::default()
    };
    let init = sl.init(&pref);
    if !init.ok() {
        return Err(format!("slInit: {}", init.name()).into());
    }
    let opened = match device::open(sl, &dll) {
        Ok(opened) => opened,
        Err(reason) => {
            // Nothing of Streamline's is in use: shut it down before Bevy creates its own device.
            sl.shutdown();
            return Err(reason);
        }
    };
    let pd = ash::vk::Handle::as_raw(opened.physical_device) as *mut std::ffi::c_void;
    let support = |f| Support(sl.is_feature_supported(f, pd));
    let version = |f| {
        sl.feature_version(f)
            .map(|(sl, ngx)| format!("{sl}/ngx{ngx}"))
    };
    let dlssg_needs = sl
        .feature_requirements(sl::FEATURE_DLSS_G)
        .map_or_else(|e| format!("unknown({})", e.name()), requirements);
    let status = Status::Ready {
        sdk: tag.into(),
        adapter: opened.adapter_name,
        luid: opened.luid,
        dlss: support(sl::FEATURE_DLSS),
        dlssg: support(sl::FEATURE_DLSS_G),
        reflex: support(sl::FEATURE_REFLEX),
        pcl: support(sl::FEATURE_PCL),
        dlss_version: version(sl::FEATURE_DLSS),
        dlssg_version: version(sl::FEATURE_DLSS_G),
        dlssg_needs,
    };
    if let Status::Ready { dlssg, .. } = &status {
        if dlssg.supported() {
            // DLSS-G takes no sRGB back buffer: Bevy picks a plain one and draws through an sRGB
            // view of it.
            vk_shim::hide_srgb_formats();
        }
    }
    let _ = STREAMLINE.set(sl);
    Ok((status, opened.render_creation))
}

/// A feature's requirements in a word list: `vsync-off`, `hws`, and the driver it wants.
fn requirements(req: sl::FeatureRequirements) -> String {
    let mut needs = Vec::new();
    if req.flags & sl::REQUIREMENT_VULKAN_SUPPORTED == 0 {
        needs.push("no-vulkan".to_string());
    }
    if req.flags & sl::REQUIREMENT_VSYNC_OFF_REQUIRED != 0 {
        needs.push("vsync-off".into());
    }
    if req.flags & sl::REQUIREMENT_HARDWARE_SCHEDULING_REQUIRED != 0 {
        needs.push("hws".into());
    }
    needs.push(format!(
        "driver{}>={}",
        req.driver_version_detected, req.driver_version_required
    ));
    let device = req.device_extensions();
    if !device.is_empty() {
        needs.push(format!("vk_device[{}]", device.join("+")));
    }
    let instance = req.instance_extensions();
    if !instance.is_empty() {
        needs.push(format!("vk_instance[{}]", instance.join("+")));
    }
    needs.join(",")
}

/// A path as the NUL-terminated UTF-16 Streamline's preferences take.
#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain([0]).collect()
}

#[cfg(not(windows))]
fn wide(_: &Path) -> Vec<u16> {
    vec![0]
}

/// Streamline's warnings and errors, on stderr beside the game's own log.
unsafe extern "C" fn log_message(log_type: u32, msg: *const std::ffi::c_char) {
    if msg.is_null() || !(log_type == sl::LOG_TYPE_WARN || log_type == sl::LOG_TYPE_ERROR) {
        return;
    }
    // SAFETY: Streamline passes a NUL-terminated message valid for the call.
    let text = unsafe { std::ffi::CStr::from_ptr(msg) }.to_string_lossy();
    let kind = if log_type == sl::LOG_TYPE_ERROR {
        "error"
    } else {
        "warn"
    };
    eprintln!("STREAMLINE_LOG {kind} {}", text.trim_end());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_sdk_sorts_by_version() {
        let mut tags = ["v2.9.0", "v2.14.16", "v2.14.1"].map(PathBuf::from);
        tags.sort_by_key(|p| version_key(p));
        assert_eq!(tags[2], PathBuf::from("v2.14.16"));
    }
}
