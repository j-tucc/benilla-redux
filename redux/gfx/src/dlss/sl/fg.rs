//! The Streamline types and calls Frame Generation (DLSS-G) and Reflex use (`sl_dlss_g.h`,
//! `sl_reflex.h`, `sl_pcl.h`, `sl_core_api.h`, SDK 2.14.16), laid out field for field; the tests
//! pin them to the headers' field order. The core calls (`slSetTagForFrame`, `slFreeResources`)
//! are interposer exports; the rest come from their plugins through `slGetFeatureFunction`.
use super::sr::{guid, FrameToken, ResourceTag, ViewportHandle};
use super::{BaseStructure, SlResult, Streamline, FEATURE_DLSS_G, FEATURE_PCL, FEATURE_REFLEX};
use std::ffi::c_void;
use std::sync::OnceLock;

/// `sl::BufferType` of the tags Frame Generation reads besides depth and motion.
pub const BUFFER_HUDLESS_COLOR: u32 = 2;
/// `sl::kBufferTypeBackbuffer`: tagged with a null resource, it only names the back buffer's
/// rectangle Frame Generation works on (DLSS-G guide, section 5.2).
pub const BUFFER_BACKBUFFER: u32 = 53;
/// `sl::ResourceLifecycle::eValidUntilPresent`: unchanged until the frame is presented.
pub const LIFECYCLE_VALID_UNTIL_PRESENT: u32 = 1;

/// `sl::DLSSGMode`.
pub const DLSSG_MODE_OFF: u32 = 0;
pub const DLSSG_MODE_ON: u32 = 1;
/// `sl::DLSSGFlags::eRetainResourcesWhenOff`: off suspends generation and keeps its memory.
pub const DLSSG_FLAG_RETAIN_RESOURCES_WHEN_OFF: u32 = 1 << 3;

/// `sl::DLSSGOptions`, version 5.
#[repr(C)]
pub struct DlssgOptions {
    pub base: BaseStructure,
    pub mode: u32,
    pub num_frames_to_generate: u32,
    pub flags: u32,
    pub dynamic_res_width: u32,
    pub dynamic_res_height: u32,
    pub num_back_buffers: u32,
    pub mvec_depth_width: u32,
    pub mvec_depth_height: u32,
    pub color_width: u32,
    pub color_height: u32,
    pub color_buffer_format: u32,
    pub mvec_buffer_format: u32,
    pub depth_buffer_format: u32,
    pub hud_less_buffer_format: u32,
    pub ui_buffer_format: u32,
    pub on_error_callback: *const c_void,
    pub b_reserved15: u8,
    pub queue_parallelism_mode: u32,
    pub enable_user_interface_recomposition: u8,
    pub dynamic_target_frame_rate: f32,
}

/// `sl::Boolean::eInvalid`.
const BOOLEAN_INVALID: u8 = 2;

impl Default for DlssgOptions {
    fn default() -> Self {
        Self {
            base: BaseStructure::new(
                guid(
                    0xfac5_f1cb,
                    0x2dfd,
                    0x4f36,
                    [0xa1, 0xe6, 0x3a, 0x9e, 0x86, 0x52, 0x56, 0xc5],
                ),
                5,
            ),
            mode: DLSSG_MODE_OFF,
            num_frames_to_generate: 1,
            flags: 0,
            dynamic_res_width: 0,
            dynamic_res_height: 0,
            num_back_buffers: 0,
            mvec_depth_width: 0,
            mvec_depth_height: 0,
            color_width: 0,
            color_height: 0,
            color_buffer_format: 0,
            mvec_buffer_format: 0,
            depth_buffer_format: 0,
            hud_less_buffer_format: 0,
            ui_buffer_format: 0,
            on_error_callback: std::ptr::null(),
            b_reserved15: BOOLEAN_INVALID,
            queue_parallelism_mode: 0,
            enable_user_interface_recomposition: 0,
            dynamic_target_frame_rate: 0.0,
        }
    }
}

/// `sl::DLSSGState`, version 4.
#[repr(C)]
pub struct DlssgState {
    pub base: BaseStructure,
    pub estimated_vram_usage_in_bytes: u64,
    /// `sl::DLSSGStatus` bits; 0 is working.
    pub status: u32,
    pub min_width_or_height: u32,
    /// Frames presented since the last `slDLSSGGetState`.
    pub num_frames_actually_presented: u32,
    pub num_frames_to_generate_max: u32,
    pub b_reserved4: u8,
    pub b_is_vsync_support_available: u8,
    pub inputs_processing_completion_fence: *mut c_void,
    pub last_present_inputs_processing_completion_fence_value: u64,
    pub b_is_dynamic_mfg_supported: u8,
}

impl Default for DlssgState {
    fn default() -> Self {
        Self {
            base: BaseStructure::new(
                guid(
                    0xcc8a_c8e1,
                    0xa179,
                    0x44f5,
                    [0x97, 0xfa, 0xe7, 0x41, 0x12, 0xf9, 0xbc, 0x61],
                ),
                4,
            ),
            estimated_vram_usage_in_bytes: 0,
            status: 0,
            min_width_or_height: 0,
            num_frames_actually_presented: 0,
            num_frames_to_generate_max: 0,
            b_reserved4: 0,
            b_is_vsync_support_available: 0,
            inputs_processing_completion_fence: std::ptr::null_mut(),
            last_present_inputs_processing_completion_fence_value: 0,
            b_is_dynamic_mfg_supported: 0,
        }
    }
}

/// `sl::DLSSGStatus` bits, by the SDK's names.
pub fn dlssg_status_names(status: u32) -> String {
    const NAMES: [&str; 6] = [
        "ResolutionTooLow",
        "ReflexNotDetectedAtRuntime",
        "HDRFormatNotSupported",
        "CommonConstantsInvalid",
        "GetCurrentBackBufferIndexNotCalled",
        "Reserved5",
    ];
    if status == 0 {
        return "ok".into();
    }
    NAMES
        .iter()
        .enumerate()
        .filter(|(i, _)| status & (1 << i) != 0)
        .map(|(_, n)| *n)
        .collect::<Vec<_>>()
        .join("+")
}

/// `sl::ReflexMode`.
pub const REFLEX_MODE_OFF: i32 = 0;
pub const REFLEX_MODE_LOW_LATENCY: i32 = 1;

/// `sl::ReflexOptions`, version 1.
#[repr(C)]
pub struct ReflexOptions {
    pub base: BaseStructure,
    pub mode: i32,
    pub frame_limit_us: u32,
    pub use_markers_to_optimize: bool,
    pub virtual_key: u16,
    pub id_thread: u32,
}

impl ReflexOptions {
    pub fn new(mode: i32) -> Self {
        Self {
            base: BaseStructure::new(
                guid(
                    0xf03a_f81a,
                    0x6d0b,
                    0x4902,
                    [0xa6, 0x51, 0xc4, 0x96, 0x5e, 0x21, 0x54, 0x34],
                ),
                1,
            ),
            mode,
            frame_limit_us: 0,
            use_markers_to_optimize: false,
            virtual_key: 0,
            id_thread: 0,
        }
    }
}

/// `sl::PCLMarker`.
pub type PclMarker = u32;
pub const PCL_SIMULATION_START: PclMarker = 0;
pub const PCL_SIMULATION_END: PclMarker = 1;
pub const PCL_RENDER_SUBMIT_START: PclMarker = 2;
pub const PCL_RENDER_SUBMIT_END: PclMarker = 3;
pub const PCL_PRESENT_START: PclMarker = 4;
pub const PCL_PRESENT_END: PclMarker = 5;

type PfnSetTagForFrame = unsafe extern "C" fn(
    frame: *const FrameToken,
    viewport: *const ViewportHandle,
    tags: *const ResourceTag,
    num_tags: u32,
    command_buffer: *mut c_void,
) -> i32;
type PfnFreeResources = unsafe extern "C" fn(feature: u32, viewport: *const ViewportHandle) -> i32;
type PfnDlssgSetOptions =
    unsafe extern "C" fn(viewport: *const ViewportHandle, options: *const DlssgOptions) -> i32;
type PfnDlssgGetState = unsafe extern "C" fn(
    viewport: *const ViewportHandle,
    state: *mut DlssgState,
    options: *const DlssgOptions,
) -> i32;
type PfnReflexSleep = unsafe extern "C" fn(frame: *const FrameToken) -> i32;
type PfnReflexSetOptions = unsafe extern "C" fn(options: *const ReflexOptions) -> i32;
type PfnPclSetMarker = unsafe extern "C" fn(marker: PclMarker, frame: *const FrameToken) -> i32;

/// The plugins' functions, asked for once each.
#[derive(Clone, Copy)]
struct PluginCalls {
    dlssg_set_options: PfnDlssgSetOptions,
    dlssg_get_state: PfnDlssgGetState,
    reflex_sleep: PfnReflexSleep,
    reflex_set_options: PfnReflexSetOptions,
    pcl_set_marker: PfnPclSetMarker,
}

/// The interposer's tagging exports, and the plugins' functions once asked for.
pub(super) struct FgCalls {
    set_tag_for_frame: PfnSetTagForFrame,
    free_resources: PfnFreeResources,
    plugins: OnceLock<Result<PluginCalls, SlResult>>,
}

impl FgCalls {
    pub(super) fn load(library: &libloading::Library) -> Result<Self, String> {
        // SAFETY: each symbol is the export of that name, with the signature `sl_core_api.h` gives.
        unsafe {
            Ok(Self {
                set_tag_for_frame: *library
                    .get::<PfnSetTagForFrame>(b"slSetTagForFrame\0")
                    .map_err(|e| e.to_string())?,
                free_resources: *library
                    .get::<PfnFreeResources>(b"slFreeResources\0")
                    .map_err(|e| e.to_string())?,
                plugins: OnceLock::new(),
            })
        }
    }
}

impl Streamline {
    fn fg_plugins(&self) -> Result<PluginCalls, SlResult> {
        *self.fg.plugins.get_or_init(|| {
            let f = |feature, name| self.sr.feature_function(feature, name);
            let dlssg_set = f(FEATURE_DLSS_G, c"slDLSSGSetOptions")?;
            let dlssg_state = f(FEATURE_DLSS_G, c"slDLSSGGetState")?;
            let sleep = f(FEATURE_REFLEX, c"slReflexSleep")?;
            let reflex_set = f(FEATURE_REFLEX, c"slReflexSetOptions")?;
            let marker = f(FEATURE_PCL, c"slPCLSetMarker")?;
            // SAFETY: the plugins' functions of these names have these signatures (`sl_dlss_g.h`,
            // `sl_reflex.h`, `sl_pcl.h`).
            unsafe {
                use std::mem::transmute as t;
                Ok(PluginCalls {
                    dlssg_set_options: t::<*mut c_void, PfnDlssgSetOptions>(dlssg_set),
                    dlssg_get_state: t::<*mut c_void, PfnDlssgGetState>(dlssg_state),
                    reflex_sleep: t::<*mut c_void, PfnReflexSleep>(sleep),
                    reflex_set_options: t::<*mut c_void, PfnReflexSetOptions>(reflex_set),
                    pcl_set_marker: t::<*mut c_void, PfnPclSetMarker>(marker),
                })
            }
        })
    }

    /// `slSetTagForFrame` with no command buffer: every tag is null or valid until present.
    ///
    /// # Safety
    /// `frame` came from `new_frame_token`; each tag's resource is null or a live image in the
    /// state tagged until the frame is presented.
    pub unsafe fn set_tags_for_frame(
        &self,
        frame: *mut FrameToken,
        viewport: &ViewportHandle,
        tags: &[ResourceTag],
    ) -> SlResult {
        // SAFETY: as the caller promises; Streamline copies the tags.
        SlResult(unsafe {
            (self.fg.set_tag_for_frame)(
                frame,
                viewport,
                tags.as_ptr(),
                tags.len() as u32,
                std::ptr::null_mut(),
            )
        })
    }

    /// `slFreeResources` for DLSS-G on `viewport`.
    pub fn dlssg_free_resources(&self, viewport: &ViewportHandle) -> SlResult {
        // SAFETY: a valid viewport for the call.
        SlResult(unsafe { (self.fg.free_resources)(FEATURE_DLSS_G, viewport) })
    }

    /// `slDLSSGSetOptions`: takes effect at the next present. Call on the presenting thread.
    pub fn dlssg_set_options(&self, viewport: &ViewportHandle, options: &DlssgOptions) -> SlResult {
        match self.fg_plugins() {
            // SAFETY: both point at valid structures; Streamline copies them.
            Ok(calls) => SlResult(unsafe { (calls.dlssg_set_options)(viewport, options) }),
            Err(e) => e,
        }
    }

    /// `slDLSSGGetState` without options: the status and the frames presented since last asked.
    pub fn dlssg_state(&self, viewport: &ViewportHandle) -> Result<DlssgState, SlResult> {
        let calls = self.fg_plugins()?;
        let mut state = DlssgState::default();
        // SAFETY: `state` is writable; no options, so no VRAM estimate is made.
        let result =
            SlResult(unsafe { (calls.dlssg_get_state)(viewport, &mut state, std::ptr::null()) });
        if result.ok() {
            Ok(state)
        } else {
            Err(result)
        }
    }

    /// `slReflexSleep` for `frame`: Reflex's pacing wait (returns at once with Reflex off).
    pub fn reflex_sleep(&self, frame: *mut FrameToken) -> SlResult {
        match self.fg_plugins() {
            // SAFETY: `frame` came from `new_frame_token`.
            Ok(calls) => SlResult(unsafe { (calls.reflex_sleep)(frame) }),
            Err(e) => e,
        }
    }

    /// `slReflexSetOptions`.
    pub fn reflex_set_options(&self, options: &ReflexOptions) -> SlResult {
        match self.fg_plugins() {
            // SAFETY: a valid structure; Streamline copies it.
            Ok(calls) => SlResult(unsafe { (calls.reflex_set_options)(options) }),
            Err(e) => e,
        }
    }

    /// `slPCLSetMarker` for `frame`.
    pub fn pcl_marker(&self, marker: PclMarker, frame: *mut FrameToken) -> SlResult {
        match self.fg_plugins() {
            // SAFETY: `frame` came from `new_frame_token`.
            Ok(calls) => SlResult(unsafe { (calls.pcl_set_marker)(marker, frame) }),
            Err(e) => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// The headers' field order with C's alignment (no `static_assert`s in `sl.cpp` for these).
    #[test]
    fn layouts_match_the_sdk() {
        assert_eq!(offset_of!(DlssgOptions, mode), 32);
        assert_eq!(offset_of!(DlssgOptions, ui_buffer_format), 88);
        assert_eq!(offset_of!(DlssgOptions, on_error_callback), 96);
        assert_eq!(offset_of!(DlssgOptions, b_reserved15), 104);
        assert_eq!(offset_of!(DlssgOptions, queue_parallelism_mode), 108);
        assert_eq!(offset_of!(DlssgOptions, dynamic_target_frame_rate), 116);
        assert_eq!(size_of::<DlssgOptions>(), 120);
        assert_eq!(offset_of!(DlssgState, status), 40);
        assert_eq!(offset_of!(DlssgState, num_frames_actually_presented), 48);
        assert_eq!(offset_of!(DlssgState, b_is_vsync_support_available), 57);
        assert_eq!(
            offset_of!(DlssgState, inputs_processing_completion_fence),
            64
        );
        assert_eq!(offset_of!(DlssgState, b_is_dynamic_mfg_supported), 80);
        assert_eq!(size_of::<DlssgState>(), 88);
        assert_eq!(offset_of!(ReflexOptions, frame_limit_us), 36);
        assert_eq!(offset_of!(ReflexOptions, virtual_key), 42);
        assert_eq!(size_of::<ReflexOptions>(), 48);
    }

    #[test]
    fn statuses_are_named() {
        assert_eq!(dlssg_status_names(0), "ok");
        assert_eq!(
            dlssg_status_names(0b1010),
            "ReflexNotDetectedAtRuntime+CommonConstantsInvalid"
        );
    }
}
