//! The Streamline types and calls DLSS Super Resolution uses (`sl_core_types.h`, `sl_consts.h`,
//! `sl_core_api.h`, `sl_dlss.h`, SDK 2.14.16), laid out field for field; the tests pin them to the
//! offsets the SDK's `static_assert`s state (`source/core/sl.api/sl.cpp`) and to the headers' own
//! field order where it asserts none.
use super::{BaseStructure, Feature, SlResult, Streamline, StructType, FEATURE_DLSS};
use std::ffi::{c_char, c_void, CStr};
use std::sync::OnceLock;

/// A struct type's GUID.
pub(super) const fn guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> StructType {
    StructType {
        data1,
        data2,
        data3,
        data4,
    }
}

/// `sl::ViewportHandle`, version 1: the id the host picks for a view.
#[repr(C)]
pub struct ViewportHandle {
    pub base: BaseStructure,
    pub value: u32,
}

impl ViewportHandle {
    pub const fn new(value: u32) -> Self {
        Self {
            base: BaseStructure::new(
                guid(
                    0x171b_6435,
                    0x9b3c,
                    0x4fc8,
                    [0x99, 0x94, 0xfb, 0xe5, 0x25, 0x69, 0xaa, 0xa4],
                ),
                1,
            ),
            value,
        }
    }
}

/// `sl::FrameToken`: opaque, handed out by `slGetNewFrameToken` and passed back by reference.
pub type FrameToken = c_void;

/// `sl::BufferType`: what a tagged resource is.
pub const BUFFER_DEPTH: u32 = 0;
pub const BUFFER_MOTION_VECTORS: u32 = 1;
pub const BUFFER_SCALING_INPUT_COLOR: u32 = 3;
pub const BUFFER_SCALING_OUTPUT_COLOR: u32 = 4;
/// `sl::ResourceLifecycle::eValidUntilEvaluate`: unchanged until `slEvaluateFeature` returns.
pub const LIFECYCLE_VALID_UNTIL_EVALUATE: u32 = 2;
/// `sl::ResourceType::eTex2d`.
pub const RESOURCE_TEX2D: u8 = 0;
/// `sl::Boolean`.
pub const FALSE: u8 = 0;
pub const TRUE: u8 = 1;

/// `sl::Extent`: a sub-rectangle of a tagged resource; all zero is the whole resource.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Extent {
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

/// `sl::Resource`, version 1. On Vulkan: the `VkImage`, its `VkImageView`, its layout as the
/// state, and its size, `VkFormat` and `VkImageUsageFlags`.
#[repr(C)]
pub struct Resource {
    pub base: BaseStructure,
    pub resource_type: u8,
    pub native: *mut c_void,
    pub memory: *mut c_void,
    pub view: *mut c_void,
    pub state: u32,
    pub width: u32,
    pub height: u32,
    pub native_format: u32,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub gpu_virtual_address: u64,
    pub flags: u32,
    pub usage: u32,
    pub internal_flags: u16,
    pub reserved: u16,
}

/// A Vulkan image as Streamline takes it: handles, layout, size, `VkFormat`, `VkImageUsageFlags`.
#[derive(Clone, Copy, Debug)]
pub struct VkImage {
    pub image: u64,
    pub view: u64,
    pub layout: i32,
    pub width: u32,
    pub height: u32,
    pub format: i32,
    pub usage: u32,
}

impl Resource {
    /// A 2D Vulkan image with one mip and one layer.
    pub fn vulkan(image: VkImage) -> Self {
        Self {
            base: BaseStructure::new(
                guid(
                    0x3a9d_70cf,
                    0x2418,
                    0x4b72,
                    [0x83, 0x91, 0x13, 0xf8, 0x72, 0x1c, 0x72, 0x61],
                ),
                1,
            ),
            resource_type: RESOURCE_TEX2D,
            native: image.image as usize as *mut c_void,
            memory: std::ptr::null_mut(),
            view: image.view as usize as *mut c_void,
            state: image.layout as u32,
            width: image.width,
            height: image.height,
            native_format: image.format as u32,
            mip_levels: 1,
            array_layers: 1,
            gpu_virtual_address: 0,
            flags: 0,
            usage: image.usage,
            internal_flags: 0,
            reserved: 0,
        }
    }
}

/// `sl::SubresourceRange`, version 1, chained after a [`Resource`]: on Vulkan, the aspect DLSS
/// reads (the depth aspect of a depth image; colour when absent).
#[repr(C)]
pub struct SubresourceRange {
    pub base: BaseStructure,
    pub aspect_mask: u32,
    pub base_mip_level: u32,
    pub level_count: u32,
    pub base_array_layer: u32,
    pub layer_count: u32,
}

impl SubresourceRange {
    pub fn aspect(aspect_mask: u32) -> Self {
        Self {
            base: BaseStructure::new(
                guid(
                    0x8d4c_316c,
                    0xd402,
                    0x4524,
                    [0x89, 0xa7, 0x14, 0xe7, 0x9e, 0x63, 0x8e, 0x3a],
                ),
                1,
            ),
            aspect_mask,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        }
    }
}

/// `sl::ResourceTag`, version 1.
#[repr(C)]
pub struct ResourceTag {
    pub base: BaseStructure,
    pub resource: *mut Resource,
    pub buffer_type: u32,
    pub lifecycle: u32,
    pub extent: Extent,
}

impl ResourceTag {
    pub fn new(resource: *mut Resource, buffer_type: u32, lifecycle: u32) -> Self {
        Self {
            base: BaseStructure::new(
                guid(
                    0x4c6a_5aad,
                    0xb445,
                    0x496c,
                    [0x87, 0xff, 0x1a, 0xf3, 0x84, 0x5b, 0xe6, 0x53],
                ),
                1,
            ),
            resource,
            buffer_type,
            lifecycle,
            extent: Extent::default(),
        }
    }
}

/// `sl::Constants`, version 2: the camera of one frame. Matrices are row-major for row vectors
/// (`v' = v * M`, `sl_matrix_helpers.h`), which is the memory of the column-vector matrix stored
/// column by column: glam's `to_cols_array`.
#[repr(C)]
pub struct Constants {
    pub base: BaseStructure,
    pub camera_view_to_clip: [f32; 16],
    pub clip_to_camera_view: [f32; 16],
    pub clip_to_lens_clip: [f32; 16],
    pub clip_to_prev_clip: [f32; 16],
    pub prev_clip_to_clip: [f32; 16],
    pub jitter_offset: [f32; 2],
    pub mvec_scale: [f32; 2],
    pub camera_pinhole_offset: [f32; 2],
    pub camera_pos: [f32; 3],
    pub camera_up: [f32; 3],
    pub camera_right: [f32; 3],
    pub camera_fwd: [f32; 3],
    pub camera_near: f32,
    pub camera_far: f32,
    pub camera_fov: f32,
    pub camera_aspect_ratio: f32,
    pub motion_vectors_invalid_value: f32,
    pub depth_inverted: u8,
    pub camera_motion_included: u8,
    pub motion_vectors_3d: u8,
    pub reset: u8,
    pub orthographic_projection: u8,
    pub motion_vectors_dilated: u8,
    pub motion_vectors_jittered: u8,
    pub min_relative_linear_depth_object_separation: f32,
}

/// `sl::INVALID_FLOAT`: a value not given.
pub const INVALID_FLOAT: f32 = f32::MAX;

/// The identity, as a matrix field.
pub const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

impl Default for Constants {
    fn default() -> Self {
        Self {
            base: BaseStructure::new(
                guid(
                    0xdcd3_5ad7,
                    0x4e4a,
                    0x4bad,
                    [0xa9, 0x0c, 0xe0, 0xc4, 0x9e, 0xb2, 0x3a, 0xfe],
                ),
                2,
            ),
            camera_view_to_clip: IDENTITY,
            clip_to_camera_view: IDENTITY,
            clip_to_lens_clip: IDENTITY,
            clip_to_prev_clip: IDENTITY,
            prev_clip_to_clip: IDENTITY,
            jitter_offset: [0.0; 2],
            mvec_scale: [1.0; 2],
            camera_pinhole_offset: [0.0; 2],
            camera_pos: [0.0; 3],
            camera_up: [0.0, 1.0, 0.0],
            camera_right: [1.0, 0.0, 0.0],
            camera_fwd: [0.0, 0.0, -1.0],
            camera_near: INVALID_FLOAT,
            camera_far: INVALID_FLOAT,
            camera_fov: INVALID_FLOAT,
            camera_aspect_ratio: INVALID_FLOAT,
            motion_vectors_invalid_value: INVALID_FLOAT,
            depth_inverted: FALSE,
            camera_motion_included: TRUE,
            motion_vectors_3d: FALSE,
            reset: FALSE,
            orthographic_projection: FALSE,
            motion_vectors_dilated: FALSE,
            motion_vectors_jittered: FALSE,
            min_relative_linear_depth_object_separation: 40.0,
        }
    }
}

/// `sl::DLSSMode`.
pub const DLSS_MODE_OFF: u32 = 0;
pub const DLSS_MODE_MAX_PERFORMANCE: u32 = 1;
pub const DLSS_MODE_BALANCED: u32 = 2;
pub const DLSS_MODE_MAX_QUALITY: u32 = 3;
pub const DLSS_MODE_ULTRA_PERFORMANCE: u32 = 4;
pub const DLSS_MODE_DLAA: u32 = 6;
/// `sl::DLSSPreset::eDefault`: the SDK's own choice per mode (2.14.16's `sl_dlss.h`: transformer
/// K for DLAA, Quality and Balanced, M for Performance, L for Ultra Performance).
pub const DLSS_PRESET_DEFAULT: u32 = 0;

/// `sl::DLSSOptions`, version 3.
#[repr(C)]
pub struct DlssOptions {
    pub base: BaseStructure,
    pub mode: u32,
    pub output_width: u32,
    pub output_height: u32,
    /// Deprecated in the SDK: no sharpening.
    pub sharpness: f32,
    pub pre_exposure: f32,
    pub exposure_scale: f32,
    pub color_buffers_hdr: u8,
    pub indicator_invert_axis_x: u8,
    pub indicator_invert_axis_y: u8,
    pub dlaa_preset: u32,
    pub quality_preset: u32,
    pub balanced_preset: u32,
    pub performance_preset: u32,
    pub ultra_performance_preset: u32,
    pub ultra_quality_preset: u32,
    pub use_auto_exposure: u8,
    pub alpha_upscaling_enabled: u8,
}

impl Default for DlssOptions {
    fn default() -> Self {
        Self {
            base: BaseStructure::new(
                guid(
                    0x6ac8_26e4,
                    0x4c61,
                    0x4101,
                    [0xa9, 0x2d, 0x63, 0x8d, 0x42, 0x10, 0x57, 0xb8],
                ),
                3,
            ),
            mode: DLSS_MODE_OFF,
            output_width: u32::MAX,
            output_height: u32::MAX,
            sharpness: 0.0,
            pre_exposure: 1.0,
            exposure_scale: 1.0,
            color_buffers_hdr: TRUE,
            indicator_invert_axis_x: FALSE,
            indicator_invert_axis_y: FALSE,
            dlaa_preset: DLSS_PRESET_DEFAULT,
            quality_preset: DLSS_PRESET_DEFAULT,
            balanced_preset: DLSS_PRESET_DEFAULT,
            performance_preset: DLSS_PRESET_DEFAULT,
            ultra_performance_preset: DLSS_PRESET_DEFAULT,
            ultra_quality_preset: DLSS_PRESET_DEFAULT,
            use_auto_exposure: FALSE,
            alpha_upscaling_enabled: FALSE,
        }
    }
}

/// `sl::DLSSOptimalSettings`, version 1.
#[repr(C)]
pub struct DlssOptimalSettings {
    pub base: BaseStructure,
    pub optimal_render_width: u32,
    pub optimal_render_height: u32,
    pub optimal_sharpness: f32,
    pub render_width_min: u32,
    pub render_height_min: u32,
    pub render_width_max: u32,
    pub render_height_max: u32,
}

impl Default for DlssOptimalSettings {
    fn default() -> Self {
        Self {
            base: BaseStructure::new(
                guid(
                    0xef1d_0957,
                    0xfd58,
                    0x4df7,
                    [0xb5, 0x04, 0x8b, 0x69, 0xd8, 0xaa, 0x6b, 0x76],
                ),
                1,
            ),
            optimal_render_width: 0,
            optimal_render_height: 0,
            optimal_sharpness: 0.0,
            render_width_min: 0,
            render_height_min: 0,
            render_width_max: 0,
            render_height_max: 0,
        }
    }
}

type PfnGetNewFrameToken =
    unsafe extern "C" fn(token: *mut *mut FrameToken, frame_index: *const u32) -> i32;
type PfnSetConstants = unsafe extern "C" fn(
    values: *const Constants,
    frame: *const FrameToken,
    viewport: *const ViewportHandle,
) -> i32;
type PfnEvaluateFeature = unsafe extern "C" fn(
    feature: Feature,
    frame: *const FrameToken,
    inputs: *const *const BaseStructure,
    num_inputs: u32,
    command_buffer: *mut c_void,
) -> i32;
type PfnGetFeatureFunction =
    unsafe extern "C" fn(feature: Feature, name: *const c_char, function: *mut *mut c_void) -> i32;
type PfnDlssSetOptions =
    unsafe extern "C" fn(viewport: *const ViewportHandle, options: *const DlssOptions) -> i32;
type PfnDlssGetOptimalSettings =
    unsafe extern "C" fn(options: *const DlssOptions, settings: *mut DlssOptimalSettings) -> i32;

/// `sl::Result::eErrorFeatureMissing`.
const ERROR_FEATURE_MISSING: SlResult = SlResult(31);

/// The interposer's per-frame exports, and DLSS's own functions once asked for.
pub(super) struct SrCalls {
    get_new_frame_token: PfnGetNewFrameToken,
    set_constants: PfnSetConstants,
    evaluate_feature: PfnEvaluateFeature,
    get_feature_function: PfnGetFeatureFunction,
    dlss: OnceLock<Result<DlssCalls, SlResult>>,
}

/// `slDLSSSetOptions` and `slDLSSGetOptimalSettings`, which the DLSS plugin hands out through
/// `slGetFeatureFunction` once the device exists (`sl.h`'s `SL_FEATURE_FUN_IMPORT_STATIC`).
#[derive(Clone, Copy)]
struct DlssCalls {
    set_options: PfnDlssSetOptions,
    get_optimal_settings: PfnDlssGetOptimalSettings,
}

impl SrCalls {
    /// The exports, from the interposer.
    pub(super) fn load(library: &libloading::Library) -> Result<Self, String> {
        // SAFETY: each symbol is the export of that name, with the signature `sl_core_api.h` gives.
        unsafe {
            Ok(Self {
                get_new_frame_token: *library
                    .get::<PfnGetNewFrameToken>(b"slGetNewFrameToken\0")
                    .map_err(|e| e.to_string())?,
                set_constants: *library
                    .get::<PfnSetConstants>(b"slSetConstants\0")
                    .map_err(|e| e.to_string())?,
                evaluate_feature: *library
                    .get::<PfnEvaluateFeature>(b"slEvaluateFeature\0")
                    .map_err(|e| e.to_string())?,
                get_feature_function: *library
                    .get::<PfnGetFeatureFunction>(b"slGetFeatureFunction\0")
                    .map_err(|e| e.to_string())?,
                dlss: OnceLock::new(),
            })
        }
    }

    pub(super) fn feature_function(
        &self,
        feature: Feature,
        name: &CStr,
    ) -> Result<*mut c_void, SlResult> {
        let mut function = std::ptr::null_mut();
        // SAFETY: `name` is NUL-terminated; Streamline writes the function's address.
        let result =
            SlResult(unsafe { (self.get_feature_function)(feature, name.as_ptr(), &mut function) });
        if !result.ok() {
            Err(result)
        } else if function.is_null() {
            Err(ERROR_FEATURE_MISSING)
        } else {
            Ok(function)
        }
    }

    fn dlss(&self) -> Result<DlssCalls, SlResult> {
        *self.dlss.get_or_init(|| {
            let set = self.feature_function(FEATURE_DLSS, c"slDLSSSetOptions")?;
            let optimal = self.feature_function(FEATURE_DLSS, c"slDLSSGetOptimalSettings")?;
            // SAFETY: the DLSS plugin's functions of these names have these signatures
            // (`sl_dlss.h`).
            unsafe {
                Ok(DlssCalls {
                    set_options: std::mem::transmute::<*mut c_void, PfnDlssSetOptions>(set),
                    get_optimal_settings: std::mem::transmute::<
                        *mut c_void,
                        PfnDlssGetOptimalSettings,
                    >(optimal),
                })
            }
        })
    }
}

impl Streamline {
    /// `slGetNewFrameToken` for the host's frame `index`.
    pub fn new_frame_token(&self, index: u32) -> Result<*mut FrameToken, SlResult> {
        let mut token = std::ptr::null_mut();
        // SAFETY: Streamline writes a token valid for the frames in flight.
        let result = SlResult(unsafe { (self.sr.get_new_frame_token)(&mut token, &index) });
        if result.ok() && !token.is_null() {
            Ok(token)
        } else {
            Err(result)
        }
    }

    /// `slSetConstants` for `frame` on `viewport`; Streamline keeps a copy.
    pub fn set_constants(
        &self,
        values: &Constants,
        frame: *mut FrameToken,
        viewport: &ViewportHandle,
    ) -> SlResult {
        // SAFETY: `frame` came from `new_frame_token`; the rest are valid for the call.
        SlResult(unsafe { (self.sr.set_constants)(values, frame, viewport) })
    }

    /// `slEvaluateFeature`, recorded into `command_buffer`.
    ///
    /// # Safety
    /// `inputs` point at live Streamline structures, `frame` came from `new_frame_token`, and
    /// `command_buffer` is a `VkCommandBuffer` of this device, recording, outside a render pass.
    pub unsafe fn evaluate(
        &self,
        feature: Feature,
        frame: *mut FrameToken,
        inputs: &[*const BaseStructure],
        command_buffer: *mut c_void,
    ) -> SlResult {
        // SAFETY: as the caller promises.
        SlResult(unsafe {
            (self.sr.evaluate_feature)(
                feature,
                frame,
                inputs.as_ptr(),
                inputs.len() as u32,
                command_buffer,
            )
        })
    }

    /// `slDLSSSetOptions` on `viewport`.
    pub fn dlss_set_options(&self, viewport: &ViewportHandle, options: &DlssOptions) -> SlResult {
        match self.sr.dlss() {
            // SAFETY: both point at valid structures for the call; Streamline copies them.
            Ok(calls) => SlResult(unsafe { (calls.set_options)(viewport, options) }),
            Err(e) => e,
        }
    }

    /// `slDLSSGetOptimalSettings`: the render size and its range for `options`' mode and output.
    pub fn dlss_optimal_settings(
        &self,
        options: &DlssOptions,
    ) -> Result<DlssOptimalSettings, SlResult> {
        let calls = self.sr.dlss()?;
        let mut settings = DlssOptimalSettings::default();
        // SAFETY: `options` is valid and `settings` writable for the call.
        let result = SlResult(unsafe { (calls.get_optimal_settings)(options, &mut settings) });
        if result.ok() {
            Ok(settings)
        } else {
            Err(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// `sl.cpp`'s `static_assert`s (`ResourceTag::extent`, `Resource::internalFlags`/`reserved`,
    /// `Constants::motionVectorsJittered`), and the headers' field order for the rest.
    #[test]
    fn layouts_match_the_sdk() {
        assert_eq!(offset_of!(ResourceTag, extent), 48);
        assert_eq!(size_of::<ResourceTag>(), 64);
        assert_eq!(offset_of!(Resource, native), 40);
        assert_eq!(offset_of!(Resource, state), 64);
        assert_eq!(offset_of!(Resource, gpu_virtual_address), 88);
        assert_eq!(offset_of!(Resource, internal_flags), 104);
        assert_eq!(offset_of!(Resource, reserved), 106);
        assert_eq!(size_of::<Resource>(), 112);
        assert_eq!(size_of::<SubresourceRange>(), 56);
        assert_eq!(size_of::<ViewportHandle>(), 40);
        assert_eq!(offset_of!(Constants, jitter_offset), 352);
        assert_eq!(offset_of!(Constants, camera_pos), 376);
        assert_eq!(offset_of!(Constants, camera_near), 424);
        assert_eq!(offset_of!(Constants, depth_inverted), 444);
        assert_eq!(offset_of!(Constants, reset), 447);
        assert_eq!(offset_of!(Constants, motion_vectors_jittered), 450);
        assert_eq!(size_of::<Constants>(), 456);
        assert_eq!(offset_of!(DlssOptions, color_buffers_hdr), 56);
        assert_eq!(offset_of!(DlssOptions, dlaa_preset), 60);
        assert_eq!(offset_of!(DlssOptions, use_auto_exposure), 84);
        assert_eq!(size_of::<DlssOptions>(), 88);
        assert_eq!(offset_of!(DlssOptimalSettings, render_height_max), 56);
    }

    #[test]
    fn structs_carry_their_versions() {
        assert_eq!(Constants::default().base.struct_version, 2);
        assert_eq!(DlssOptions::default().base.struct_version, 3);
        assert_eq!(ViewportHandle::new(7).value, 7);
    }
}
