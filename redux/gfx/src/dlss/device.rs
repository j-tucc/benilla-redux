//! The Vulkan instance and device, created through Streamline's interposer and handed to wgpu.
//!
//! The Streamline programming guide (section 2.5, and the manual hooking guide, 5.2) gives two
//! ways to create a Vulkan device for its features: through the interposer's own
//! `vkCreateInstance`/`vkCreateDevice` proxies, which add every extension, feature and queue the
//! loaded plugins need, or natively plus `slSetVulkanInfo`. This takes the first: `ash` loads the
//! interposer as its Vulkan library (guide 2.1, "instead of vulkan-1.dll dynamically load
//! sl.interposer.dll"), so every function wgpu-hal resolves through that entry, the swap chain and
//! present among them, is Streamline's proxy where Streamline intercepts it and the driver's
//! otherwise. The entry's `vkGetInstanceProcAddr` is the interposer's behind [`super::vk_shim`],
//! which wraps the surface format queries and the present for Frame Generation.
//!
//! The instance is created the way `wgpu_hal::vulkan::Instance::init_with_callback` does it (its
//! extensions from `desired_extensions`, API version 1.3), only from this entry, then wrapped with
//! `Instance::from_raw`. From there wgpu itself picks the adapter and opens the device, with the
//! features and limits Bevy's automatic renderer asks for (`bevy_render::renderer::
//! initialize_renderer` under `WgpuSettingsPriority::Functionality`), so the device is what the
//! game gets without Streamline, plus what Streamline's proxy adds.
use super::sl::{self, Streamline};
use ash::vk;
use bevy::render::renderer::{
    RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue, WgpuWrapper,
};
use bevy::render::settings::{RenderCreation, WgpuSettings, WgpuSettingsPriority};
use std::ffi::{c_void, CString};
use std::path::Path;
use std::sync::Arc;
use wgpu::hal::{api::Vulkan, vulkan};

/// What device creation found about the adapter, for the startup line.
pub struct Opened {
    pub render_creation: RenderCreation,
    pub adapter_name: String,
    pub luid: Option<[u8; 8]>,
    pub physical_device: vk::PhysicalDevice,
}

/// Why the device was not created through the interposer.
pub enum OpenError {
    /// The adapter has neither DLSS nor DLSS-G: nothing for Streamline to do on this machine.
    Unsupported(String),
    /// Anything else.
    Failed(String),
}

impl From<String> for OpenError {
    fn from(reason: String) -> Self {
        OpenError::Failed(reason)
    }
}

impl From<&str> for OpenError {
    fn from(reason: &str) -> Self {
        OpenError::Failed(reason.into())
    }
}

/// Creates the instance and device through the interposer at `interposer`. `slInit` has run.
pub fn open(sl: &Streamline, interposer: &Path) -> Result<Opened, OpenError> {
    let settings = WgpuSettings::default();
    if settings
        .backends
        .is_some_and(|b| !b.contains(wgpu::Backends::VULKAN))
    {
        return Err(OpenError::Unsupported(
            "WGPU_BACKEND excludes Vulkan".into(),
        ));
    }
    // Bevy's default has none; the clamp Bevy applies for them is not repeated here.
    if settings.constrained_limits.is_some() {
        return Err("constrained wgpu limits are not supported".into());
    }
    let entry = interposer_entry(interposer)?;
    let hal_instance = create_instance(entry, settings.instance_flags)?;
    // SAFETY: `hal_instance` is a Vulkan instance wgpu-hal can drive.
    let instance = unsafe { wgpu::Instance::from_hal::<Vulkan>(hal_instance) };
    let adapter = bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: settings.power_preference,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .map_err(|e| format!("no Vulkan adapter: {e}"))?;
    let info = adapter.get_info();
    let (physical_device, luid) = {
        // SAFETY: the adapter is a Vulkan one; nothing else touches it here.
        let hal = unsafe { adapter.as_hal::<Vulkan>() }.ok_or("adapter is not Vulkan")?;
        let physical_device = hal.raw_physical_device();
        (physical_device, luid(&hal, physical_device))
    };
    let pd = vk::Handle::as_raw(physical_device) as *mut c_void;
    let dlss = sl.is_feature_supported(sl::FEATURE_DLSS, pd);
    let dlssg = sl.is_feature_supported(sl::FEATURE_DLSS_G, pd);
    if !dlss.ok() && !dlssg.ok() {
        return Err(OpenError::Unsupported(format!(
            "{} supports neither DLSS ({}) nor DLSS-G ({})",
            info.name,
            dlss.name(),
            dlssg.name()
        )));
    }
    let (device, queue) = bevy::tasks::block_on(adapter.request_device(&device_descriptor(
        &settings,
        &adapter,
        info.device_type,
    )))
    .map_err(|e| format!("request_device through the interposer failed: {e}"))?;
    let adapter_name = info.name.clone();
    let render_creation = RenderCreation::manual(
        RenderDevice::from(device),
        RenderQueue(Arc::new(WgpuWrapper::new(queue))),
        RenderAdapterInfo(WgpuWrapper::new(info)),
        RenderAdapter(Arc::new(WgpuWrapper::new(adapter))),
        RenderInstance(Arc::new(WgpuWrapper::new(instance))),
    );
    Ok(Opened {
        render_creation,
        adapter_name,
        luid,
        physical_device,
    })
}

/// The interposer as the Vulkan loader (guide 2.1), its `vkGetInstanceProcAddr` behind
/// [`super::vk_shim`]. The library stays loaded for the process, as the interposer itself does.
fn interposer_entry(interposer: &Path) -> Result<ash::Entry, String> {
    // SAFETY: loading the interposer again only takes another reference to the loaded library.
    let library = unsafe { libloading::Library::new(interposer) }
        .map_err(|e| format!("cannot load {}: {e}", interposer.display()))?;
    // SAFETY: the interposer exports the Vulkan loader's entry points (guide 2.1).
    let get_instance_proc_addr = unsafe {
        *library
            .get::<vk::PFN_vkGetInstanceProcAddr>(b"vkGetInstanceProcAddr\0")
            .map_err(|e| format!("cannot load Vulkan through the interposer: {e}"))?
    };
    std::mem::forget(library);
    // SAFETY: the export stays loaded (the library is never unloaded).
    Ok(unsafe { super::vk_shim::entry(get_instance_proc_addr) })
}

/// `wgpu_hal::vulkan::Instance::init_with_callback`, from `entry`: no layers, no debug messenger
/// (the validation layer is a development tool the interposer would sit under, not over).
fn create_instance(
    entry: ash::Entry,
    flags: wgpu::InstanceFlags,
) -> Result<vulkan::Instance, String> {
    // SAFETY: a plain query of the loader.
    let instance_api_version = match unsafe { entry.try_enumerate_instance_version() } {
        Ok(Some(version)) => version,
        Ok(None) => vk::API_VERSION_1_0,
        Err(e) => return Err(format!("vkEnumerateInstanceVersion: {e}")),
    };
    let extensions = vulkan::Instance::desired_extensions(&entry, instance_api_version, flags)
        .map_err(|e| format!("instance extensions: {e}"))?;
    // SAFETY: a plain query of the loader.
    let layers = unsafe { entry.enumerate_instance_layer_properties() }.unwrap_or_default();
    let has_nv_optimus = layers
        .iter()
        .any(|l| l.layer_name_as_c_str() == Ok(c"VK_LAYER_NV_optimus"));
    let app_name = CString::new("benilla-redux").unwrap();
    let app_info = vk::ApplicationInfo::default()
        .application_name(&app_name)
        .application_version(1)
        .engine_name(c"wgpu-hal")
        .engine_version(2)
        // wgpu-hal's highest version; Streamline raises an older one to 1.3 too.
        .api_version(if instance_api_version < vk::API_VERSION_1_1 {
            vk::API_VERSION_1_0
        } else {
            vk::API_VERSION_1_3
        });
    let names: Vec<_> = extensions.iter().map(|e| e.as_ptr()).collect();
    let create_info = vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&names);
    // SAFETY: `create_info` and everything it points at outlive the call.
    let raw = unsafe { entry.create_instance(&create_info, None) }
        .map_err(|e| format!("vkCreateInstance through the interposer: {e}"))?;
    // SAFETY: `raw` was created from `entry` with exactly `extensions`, `instance_api_version`
    // and `flags`; wgpu-hal takes ownership and destroys it.
    unsafe {
        vulkan::Instance::from_raw(
            entry,
            raw,
            instance_api_version,
            0,
            None,
            extensions,
            flags,
            wgpu::MemoryBudgetThresholds::default(),
            has_nv_optimus,
            None,
        )
    }
    .map_err(|e| format!("wgpu-hal instance: {e}"))
}

/// The device Bevy's automatic renderer would request from this adapter
/// (`bevy_render::renderer::initialize_renderer`).
fn device_descriptor<'a>(
    settings: &'a WgpuSettings,
    adapter: &wgpu::Adapter,
    device_type: wgpu::DeviceType,
) -> wgpu::DeviceDescriptor<'a> {
    let mut features = wgpu::Features::empty();
    let mut limits = settings.limits.clone();
    if matches!(settings.priority, WgpuSettingsPriority::Functionality) {
        features = adapter.features();
        if device_type == wgpu::DeviceType::DiscreteGpu {
            features.remove(wgpu::Features::MAPPABLE_PRIMARY_BUFFERS);
        }
        limits = adapter.limits();
    }
    if let Some(disabled) = settings.disabled_features {
        features.remove(disabled);
    }
    features |= settings.features;
    wgpu::DeviceDescriptor {
        label: settings.device_label.as_deref(),
        required_features: features,
        required_limits: limits,
        // SAFETY: as Bevy's own renderer (bevyengine/bevy#22082).
        experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
        memory_hints: settings.memory_hints.clone(),
        trace: wgpu::Trace::Off,
    }
}

/// The adapter's LUID (`VkPhysicalDeviceIDProperties`), as Windows names the GPU.
fn luid(adapter: &vulkan::Adapter, physical_device: vk::PhysicalDevice) -> Option<[u8; 8]> {
    let instance = adapter.shared_instance().raw_instance();
    let mut id = vk::PhysicalDeviceIDProperties::default();
    let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
    // SAFETY: a query on a valid physical device of a 1.1+ instance.
    unsafe { instance.get_physical_device_properties2(physical_device, &mut props) };
    (id.device_luid_valid == vk::TRUE).then_some(id.device_luid)
}
