//! A minimal hand-written FFI to NVIDIA Streamline's C API (`include/sl*.h` of the SDK), only the
//! calls used: for device creation `slInit`, `slShutdown`, `slIsFeatureSupported`,
//! `slGetFeatureRequirements` and `slGetFeatureVersion`; for DLSS Super Resolution
//! ([`super::sr`]) `slGetNewFrameToken`, `slSetConstants`, `slEvaluateFeature` and, through
//! `slGetFeatureFunction`, `slDLSSSetOptions` and `slDLSSGetOptimalSettings`; for Frame
//! Generation and Reflex ([`super::fg`], [`super::frame`]) `slSetTagForFrame`, `slFreeResources`
//! and, through `slGetFeatureFunction`, `slDLSSGSetOptions`, `slDLSSGGetState`,
//! `slReflexSetOptions`, `slReflexSleep` and `slPCLSetMarker`. The core functions are exported by
//! `sl.interposer.dll`, loaded at run time; no SDK is needed to build.
//!
//! Every struct here is an `sl::BaseStructure` (`sl_struct.h`): a chain pointer, a GUID naming the
//! type and a version, then the type's fields. The layouts follow the SDK's headers field for
//! field; the tests pin them to the offsets the SDK's own `static_assert`s state
//! (`source/core/sl.api/sl.cpp`).
use std::ffi::{c_char, c_void, CStr};
use std::path::Path;

mod fg;
mod sr;
pub use fg::*;
pub use sr::*;

/// `sl::Feature` (`sl_core_types.h`).
pub type Feature = u32;
pub const FEATURE_DLSS: Feature = 0;
pub const FEATURE_REFLEX: Feature = 3;
pub const FEATURE_PCL: Feature = 4;
pub const FEATURE_DLSS_G: Feature = 1000;

/// `sl::StructType`: a GUID.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StructType {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

/// `sl::BaseStructure`.
#[repr(C)]
pub struct BaseStructure {
    pub next: *mut BaseStructure,
    pub struct_type: StructType,
    pub struct_version: usize,
}

impl BaseStructure {
    const fn new(struct_type: StructType, struct_version: usize) -> Self {
        Self {
            next: std::ptr::null_mut(),
            struct_type,
            struct_version,
        }
    }
}

/// `sl::Version`.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub build: u32,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.build)
    }
}

/// `sl::LogLevel`.
pub const LOG_LEVEL_DEFAULT: u32 = 1;
/// `sl::LogType`.
pub const LOG_TYPE_WARN: u32 = 1;
pub const LOG_TYPE_ERROR: u32 = 2;
/// `sl::PreferenceFlags::eDisableCLStateTracking`: the host restores its command buffer's state
/// after an evaluation (the SDK's default).
pub const PREFERENCE_DISABLE_CL_STATE_TRACKING: u64 = 1 << 0;
/// `sl::PreferenceFlags::eUseFrameBasedResourceTagging`: global tags name their frame
/// (`slSetTagForFrame`), as frames overlap between the main and render threads.
pub const PREFERENCE_FRAME_BASED_TAGGING: u64 = 1 << 7;
/// `sl::EngineType::eCustom`.
pub const ENGINE_CUSTOM: u32 = 0;
/// `sl::RenderAPI::eVulkan`.
pub const RENDER_API_VULKAN: u32 = 2;

/// `sl::PFun_LogMessageCallback`.
pub type LogMessageCallback = unsafe extern "C" fn(log_type: u32, msg: *const c_char);

/// `sl::Preferences`, version 1.
#[repr(C)]
pub struct Preferences {
    pub base: BaseStructure,
    pub show_console: bool,
    pub log_level: u32,
    pub paths_to_plugins: *const *const u16,
    pub num_paths_to_plugins: u32,
    pub path_to_logs_and_data: *const u16,
    pub allocate_callback: *const c_void,
    pub release_callback: *const c_void,
    pub log_message_callback: Option<LogMessageCallback>,
    pub flags: u64,
    pub features_to_load: *const Feature,
    pub num_features_to_load: u32,
    pub application_id: u32,
    pub engine: u32,
    pub engine_version: *const c_char,
    pub project_id: *const c_char,
    pub render_api: u32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            base: BaseStructure::new(
                StructType {
                    data1: 0x1ca1_0965,
                    data2: 0xbf8e,
                    data3: 0x432b,
                    data4: [0x8d, 0xa1, 0x67, 0x16, 0xd8, 0x79, 0xfb, 0x14],
                },
                1,
            ),
            show_console: false,
            log_level: LOG_LEVEL_DEFAULT,
            paths_to_plugins: std::ptr::null(),
            num_paths_to_plugins: 0,
            path_to_logs_and_data: std::ptr::null(),
            allocate_callback: std::ptr::null(),
            release_callback: std::ptr::null(),
            log_message_callback: None,
            flags: PREFERENCE_DISABLE_CL_STATE_TRACKING,
            features_to_load: std::ptr::null(),
            num_features_to_load: 0,
            application_id: 0,
            engine: ENGINE_CUSTOM,
            engine_version: std::ptr::null(),
            project_id: std::ptr::null(),
            render_api: RENDER_API_VULKAN,
        }
    }
}

/// `sl::AdapterInfo`, version 1: the adapter by LUID or, on Vulkan, by `VkPhysicalDevice`.
#[repr(C)]
pub struct AdapterInfo {
    pub base: BaseStructure,
    pub device_luid: *mut u8,
    pub device_luid_size_in_bytes: u32,
    pub vk_physical_device: *mut c_void,
}

impl AdapterInfo {
    pub fn vulkan(physical_device: *mut c_void) -> Self {
        Self {
            base: BaseStructure::new(
                StructType {
                    data1: 0x0677_315f,
                    data2: 0xa746,
                    data3: 0x4492,
                    data4: [0x9f, 0x42, 0xcb, 0x61, 0x42, 0xc9, 0xc3, 0xd4],
                },
                1,
            ),
            device_luid: std::ptr::null_mut(),
            device_luid_size_in_bytes: 0,
            vk_physical_device: physical_device,
        }
    }
}

/// `sl::FeatureRequirementFlags`.
pub const REQUIREMENT_VULKAN_SUPPORTED: u32 = 1 << 2;
pub const REQUIREMENT_VSYNC_OFF_REQUIRED: u32 = 1 << 3;
pub const REQUIREMENT_HARDWARE_SCHEDULING_REQUIRED: u32 = 1 << 4;

/// `sl::FeatureRequirements`, version 2.
#[repr(C)]
pub struct FeatureRequirements {
    pub base: BaseStructure,
    pub flags: u32,
    pub max_num_cpu_threads: u32,
    pub max_num_viewports: u32,
    pub num_required_tags: u32,
    pub required_tags: *const u32,
    pub os_version_detected: Version,
    pub os_version_required: Version,
    pub driver_version_detected: Version,
    pub driver_version_required: Version,
    pub vk_num_compute_queues_required: u32,
    pub vk_num_graphics_queues_required: u32,
    pub vk_num_device_extensions: u32,
    pub vk_device_extensions: *const *const c_char,
    pub vk_num_instance_extensions: u32,
    pub vk_instance_extensions: *const *const c_char,
    pub vk_num_features12: u32,
    pub vk_features12: *const *const c_char,
    pub vk_num_features13: u32,
    pub vk_features13: *const *const c_char,
    pub vk_num_optical_flow_queues_required: u32,
}

impl Default for FeatureRequirements {
    fn default() -> Self {
        Self {
            base: BaseStructure::new(
                StructType {
                    data1: 0x6671_4097,
                    data2: 0xac6d,
                    data3: 0x4bc6,
                    data4: [0x89, 0x15, 0x1e, 0x0f, 0x55, 0xa6, 0xb6, 0x1f],
                },
                2,
            ),
            flags: 0,
            max_num_cpu_threads: 0,
            max_num_viewports: 0,
            num_required_tags: 0,
            required_tags: std::ptr::null(),
            os_version_detected: Version::default(),
            os_version_required: Version::default(),
            driver_version_detected: Version::default(),
            driver_version_required: Version::default(),
            vk_num_compute_queues_required: 0,
            vk_num_graphics_queues_required: 0,
            vk_num_device_extensions: 0,
            vk_device_extensions: std::ptr::null(),
            vk_num_instance_extensions: 0,
            vk_instance_extensions: std::ptr::null(),
            vk_num_features12: 0,
            vk_features12: std::ptr::null(),
            vk_num_features13: 0,
            vk_features13: std::ptr::null(),
            vk_num_optical_flow_queues_required: 0,
        }
    }
}

impl FeatureRequirements {
    /// The device extensions the feature needs, as names.
    pub fn device_extensions(&self) -> Vec<String> {
        // SAFETY: Streamline fills `count` valid C strings, alive while it is initialised.
        unsafe { names(self.vk_device_extensions, self.vk_num_device_extensions) }
    }

    /// The instance extensions the feature needs, as names.
    pub fn instance_extensions(&self) -> Vec<String> {
        // SAFETY: as `device_extensions`.
        unsafe { names(self.vk_instance_extensions, self.vk_num_instance_extensions) }
    }
}

unsafe fn names(list: *const *const c_char, count: u32) -> Vec<String> {
    if list.is_null() {
        return Vec::new();
    }
    (0..count as usize)
        .filter_map(|i| {
            let name = unsafe { *list.add(i) };
            (!name.is_null()).then(|| unsafe { CStr::from_ptr(name) }.to_string_lossy().into())
        })
        .collect()
}

/// `sl::FeatureVersion`, version 1.
#[repr(C)]
pub struct FeatureVersion {
    pub base: BaseStructure,
    pub version_sl: Version,
    pub version_ngx: Version,
}

impl Default for FeatureVersion {
    fn default() -> Self {
        Self {
            base: BaseStructure::new(
                StructType {
                    data1: 0x6d5b_51f0,
                    data2: 0x076b,
                    data3: 0x486d,
                    data4: [0x99, 0x95, 0x5a, 0x56, 0x10, 0x43, 0xf5, 0xc1],
                },
                1,
            ),
            version_sl: Version::default(),
            version_ngx: Version::default(),
        }
    }
}

/// `sl::Result` (`sl_result.h`): 0 is success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlResult(pub i32);

impl SlResult {
    pub fn ok(self) -> bool {
        self.0 == 0
    }

    /// The SDK's name for the code.
    pub fn name(self) -> &'static str {
        const NAMES: [&str; 42] = [
            "eOk",
            "eErrorIO",
            "eErrorDriverOutOfDate",
            "eErrorOSOutOfDate",
            "eErrorOSDisabledHWS",
            "eErrorDeviceNotCreated",
            "eErrorNoSupportedAdapterFound",
            "eErrorAdapterNotSupported",
            "eErrorNoPlugins",
            "eErrorVulkanAPI",
            "eErrorDXGIAPI",
            "eErrorD3DAPI",
            "eErrorNRDAPI",
            "eErrorNVAPI",
            "eErrorReflexAPI",
            "eErrorNGXFailed",
            "eErrorJSONParsing",
            "eErrorMissingProxy",
            "eErrorMissingResourceState",
            "eErrorInvalidIntegration",
            "eErrorMissingInputParameter",
            "eErrorNotInitialized",
            "eErrorComputeFailed",
            "eErrorInitNotCalled",
            "eErrorExceptionHandler",
            "eErrorInvalidParameter",
            "eErrorMissingConstants",
            "eErrorDuplicatedConstants",
            "eErrorMissingOrInvalidAPI",
            "eErrorCommonConstantsMissing",
            "eErrorUnsupportedInterface",
            "eErrorFeatureMissing",
            "eErrorFeatureNotSupported",
            "eErrorFeatureMissingHooks",
            "eErrorFeatureFailedToLoad",
            "eErrorFeatureWrongPriority",
            "eErrorFeatureMissingDependency",
            "eErrorFeatureManagerInvalidState",
            "eErrorInvalidState",
            "eWarnOutOfVRAM",
            "?",
            "?",
        ];
        usize::try_from(self.0)
            .ok()
            .and_then(|i| NAMES.get(i))
            .copied()
            .unwrap_or("?")
    }
}

/// `sl::kSDKVersion` (`sl_version.h`) for the SDK these layouts follow, 2.14.16: major, minor
/// and patch in the top three 16-bit words over the magic `0xfedc`.
pub const SDK_VERSION: u64 = (2u64 << 48) | (14u64 << 32) | (16u64 << 16) | 0xfedc;

type PfnInit = unsafe extern "C" fn(pref: *const Preferences, sdk_version: u64) -> i32;
type PfnShutdown = unsafe extern "C" fn() -> i32;
type PfnIsFeatureSupported =
    unsafe extern "C" fn(feature: Feature, info: *const AdapterInfo) -> i32;
type PfnGetFeatureRequirements =
    unsafe extern "C" fn(feature: Feature, req: *mut FeatureRequirements) -> i32;
type PfnGetFeatureVersion = unsafe extern "C" fn(feature: Feature, v: *mut FeatureVersion) -> i32;

/// The interposer's exports this crate calls. The library stays loaded for the process: Vulkan
/// calls go through it for as long as the device lives.
pub struct Streamline {
    _library: libloading::Library,
    init: PfnInit,
    shutdown: PfnShutdown,
    is_feature_supported: PfnIsFeatureSupported,
    get_feature_requirements: PfnGetFeatureRequirements,
    get_feature_version: PfnGetFeatureVersion,
    sr: sr::SrCalls,
    fg: fg::FgCalls,
}

impl Streamline {
    /// Loads `sl.interposer.dll` and its exports.
    pub fn load(interposer: &Path) -> Result<Self, String> {
        // SAFETY: loading NVIDIA's interposer runs its DllMain, which only sets up its own state.
        let library = unsafe { libloading::Library::new(interposer) }
            .map_err(|e| format!("cannot load {}: {e}", interposer.display()))?;
        // SAFETY: each symbol is the export of that name, with the signature `sl_core_api.h` gives.
        unsafe {
            let init = *library
                .get::<PfnInit>(b"slInit\0")
                .map_err(|e| e.to_string())?;
            let shutdown = *library
                .get::<PfnShutdown>(b"slShutdown\0")
                .map_err(|e| e.to_string())?;
            let is_feature_supported = *library
                .get::<PfnIsFeatureSupported>(b"slIsFeatureSupported\0")
                .map_err(|e| e.to_string())?;
            let get_feature_requirements = *library
                .get::<PfnGetFeatureRequirements>(b"slGetFeatureRequirements\0")
                .map_err(|e| e.to_string())?;
            let get_feature_version = *library
                .get::<PfnGetFeatureVersion>(b"slGetFeatureVersion\0")
                .map_err(|e| e.to_string())?;
            let sr = sr::SrCalls::load(&library)?;
            let fg = fg::FgCalls::load(&library)?;
            Ok(Self {
                _library: library,
                init,
                shutdown,
                is_feature_supported,
                get_feature_requirements,
                get_feature_version,
                sr,
                fg,
            })
        }
    }

    /// `slInit`. Must come before any Vulkan call through the interposer.
    pub fn init(&self, pref: &Preferences) -> SlResult {
        // SAFETY: `pref` is a valid `Preferences`; Streamline copies it.
        SlResult(unsafe { (self.init)(pref, SDK_VERSION) })
    }

    /// `slShutdown`.
    pub fn shutdown(&self) -> SlResult {
        // SAFETY: no arguments; valid after `slInit`.
        SlResult(unsafe { (self.shutdown)() })
    }

    /// `slIsFeatureSupported` on a Vulkan physical device.
    pub fn is_feature_supported(&self, feature: Feature, physical_device: *mut c_void) -> SlResult {
        let info = AdapterInfo::vulkan(physical_device);
        // SAFETY: `info` is a valid `AdapterInfo` for the call.
        SlResult(unsafe { (self.is_feature_supported)(feature, &info) })
    }

    /// `slGetFeatureRequirements`.
    pub fn feature_requirements(&self, feature: Feature) -> Result<FeatureRequirements, SlResult> {
        let mut req = FeatureRequirements::default();
        // SAFETY: `req` is a valid, writable `FeatureRequirements`.
        let result = SlResult(unsafe { (self.get_feature_requirements)(feature, &mut req) });
        if result.ok() {
            Ok(req)
        } else {
            Err(result)
        }
    }

    /// `slGetFeatureVersion`: the plugin's and, if it has one, its NGX library's version.
    pub fn feature_version(&self, feature: Feature) -> Option<(Version, Version)> {
        let mut v = FeatureVersion::default();
        // SAFETY: `v` is a valid, writable `FeatureVersion`.
        let result = SlResult(unsafe { (self.get_feature_version)(feature, &mut v) });
        result.ok().then_some((v.version_sl, v.version_ngx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// The offsets the SDK's own `static_assert`s pin (`source/core/sl.api/sl.cpp`, 2.14.16).
    #[test]
    fn layouts_match_the_sdk() {
        assert_eq!(size_of::<BaseStructure>(), 32);
        assert_eq!(offset_of!(Preferences, render_api), 136);
        assert_eq!(offset_of!(AdapterInfo, vk_physical_device), 48);
        assert_eq!(offset_of!(FeatureVersion, version_ngx), 44);
        assert_eq!(
            offset_of!(FeatureRequirements, vk_num_optical_flow_queues_required),
            176
        );
    }

    #[test]
    fn sdk_version_encodes_2_14_16() {
        assert_eq!(SDK_VERSION, 0x0002_000e_0010_fedc);
    }

    #[test]
    fn results_are_named() {
        assert_eq!(SlResult(0).name(), "eOk");
        assert_eq!(SlResult(4).name(), "eErrorOSDisabledHWS");
        assert_eq!(SlResult(32).name(), "eErrorFeatureNotSupported");
        assert_eq!(SlResult(-1).name(), "?");
    }
}
