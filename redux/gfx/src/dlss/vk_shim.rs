//! The Vulkan entry wgpu-hal gets: the interposer's `vkGetInstanceProcAddr`, with five functions
//! wrapped on their way out. Every other function is the interposer's (or, past it, the driver's)
//! untouched.
//!
//! - `vkGetPhysicalDeviceSurfaceFormatsKHR` (and its `2KHR` form) leave out the sRGB formats once
//!   [`hide_srgb_formats`] is called: Frame Generation refuses sRGB back buffers (the DLSS-G plugin
//!   logs "SRGB format cannot be used for swap-chain back buffers"), and Bevy, offered no sRGB
//!   format, picks `Bgra8Unorm` and draws through a `Bgra8UnormSrgb` view of it
//!   (`bevy_render::view::window::create_surfaces`, wgpu's `VK_KHR_swapchain_mutable_format`),
//!   so the bytes in the back buffer are the same.
//! - `vkQueuePresentKHR` runs [`super::frame`]'s present hooks around the real present: the
//!   Reflex/PCL present markers carry the frame being presented, which DLSS-G matches to its
//!   constants and tags (DLSS-G guide, section 8), and Frame Generation's options are set on the
//!   presenting thread just before the present they apply to (section 6). Bevy presents inside
//!   `render_system`, where nothing else could reach. The present is timed for the performance
//!   probe ([`super::frame::pacing`]).
//! - `vkAcquireNextImageKHR` first lets [`super::in_flight`] hold the CPU to one frame ahead of
//!   the GPU while Frame Generation is on, and `vkGetDeviceQueue` tells it the queue wgpu submits
//!   on.
//! - `vkGetDeviceProcAddr` is wrapped so the device-level `vkQueuePresentKHR` wgpu-hal loads
//!   (`khr::swapchain::Device::new`) and the device-level acquire and queue are the wrapped ones
//!   too.
use ash::vk;
use std::ffi::{c_char, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static GET_INSTANCE_PROC_ADDR: OnceLock<vk::PFN_vkGetInstanceProcAddr> = OnceLock::new();
static GET_DEVICE_PROC_ADDR: OnceLock<vk::PFN_vkGetDeviceProcAddr> = OnceLock::new();
static SURFACE_FORMATS: OnceLock<vk::PFN_vkGetPhysicalDeviceSurfaceFormatsKHR> = OnceLock::new();
static SURFACE_FORMATS2: OnceLock<vk::PFN_vkGetPhysicalDeviceSurfaceFormats2KHR> = OnceLock::new();
static QUEUE_PRESENT: OnceLock<vk::PFN_vkQueuePresentKHR> = OnceLock::new();
static ACQUIRE: OnceLock<vk::PFN_vkAcquireNextImageKHR> = OnceLock::new();
static GET_DEVICE_QUEUE: OnceLock<vk::PFN_vkGetDeviceQueue> = OnceLock::new();
static HIDE_SRGB: AtomicBool = AtomicBool::new(false);

/// An `ash::Entry` over the interposer's `vkGetInstanceProcAddr`, wrapped as above.
///
/// # Safety
/// `real` is the interposer's `vkGetInstanceProcAddr`, loaded for the life of the process.
pub unsafe fn entry(real: vk::PFN_vkGetInstanceProcAddr) -> ash::Entry {
    let _ = GET_INSTANCE_PROC_ADDR.set(real);
    // SAFETY: the shim is a valid `vkGetInstanceProcAddr` over `real`.
    unsafe {
        ash::Entry::from_static_fn(ash::StaticFn {
            get_instance_proc_addr,
        })
    }
}

/// From now on the surface format queries leave out sRGB formats (call before the surface is
/// created: Bevy picks its format once).
pub fn hide_srgb_formats() {
    HIDE_SRGB.store(true, Ordering::Relaxed);
}

/// A function pointer as `vkGet*ProcAddr` returns it.
fn void_fn<F: Copy>(f: F) -> vk::PFN_vkVoidFunction {
    debug_assert_eq!(size_of::<F>(), size_of::<unsafe extern "system" fn()>());
    // SAFETY: `F` is an `extern "system"` function pointer; the caller casts it back to its type.
    Some(unsafe { std::mem::transmute_copy::<F, unsafe extern "system" fn()>(&f) })
}

/// The function `wanted` resolved to, as type `F`, remembered as the real one, and the shim in
/// its place; `None` (nothing to wrap) when the interposer has no such function.
fn wrap<F: Copy>(
    real: vk::PFN_vkVoidFunction,
    slot: &OnceLock<F>,
    shim: F,
) -> vk::PFN_vkVoidFunction {
    let real = real?;
    // SAFETY: `real` was resolved for the name `F` is the type of.
    let typed = unsafe { std::mem::transmute_copy::<unsafe extern "system" fn(), F>(&real) };
    let _ = slot.set(typed);
    void_fn(shim)
}

/// The wrapped `vkGetInstanceProcAddr`.
pub unsafe extern "system" fn get_instance_proc_addr(
    instance: vk::Instance,
    name: *const c_char,
) -> vk::PFN_vkVoidFunction {
    let real = *GET_INSTANCE_PROC_ADDR.get()?;
    // SAFETY: as Vulkan's own `vkGetInstanceProcAddr`; `name` is NUL-terminated.
    let found = unsafe { real(instance, name) };
    // SAFETY: `name` is a NUL-terminated string for the call.
    match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"vkGetInstanceProcAddr" => {
            found.and_then(|_| void_fn(get_instance_proc_addr as vk::PFN_vkGetInstanceProcAddr))
        }
        b"vkGetDeviceProcAddr" => wrap(
            found,
            &GET_DEVICE_PROC_ADDR,
            get_device_proc_addr as vk::PFN_vkGetDeviceProcAddr,
        ),
        b"vkGetPhysicalDeviceSurfaceFormatsKHR" => wrap(
            found,
            &SURFACE_FORMATS,
            surface_formats as vk::PFN_vkGetPhysicalDeviceSurfaceFormatsKHR,
        ),
        b"vkGetPhysicalDeviceSurfaceFormats2KHR" => wrap(
            found,
            &SURFACE_FORMATS2,
            surface_formats2 as vk::PFN_vkGetPhysicalDeviceSurfaceFormats2KHR,
        ),
        b"vkQueuePresentKHR" => wrap(
            found,
            &QUEUE_PRESENT,
            queue_present as vk::PFN_vkQueuePresentKHR,
        ),
        b"vkAcquireNextImageKHR" => wrap(
            found,
            &ACQUIRE,
            acquire_next_image as vk::PFN_vkAcquireNextImageKHR,
        ),
        b"vkGetDeviceQueue" => wrap(
            found,
            &GET_DEVICE_QUEUE,
            get_device_queue as vk::PFN_vkGetDeviceQueue,
        ),
        _ => found,
    }
}

unsafe extern "system" fn get_device_proc_addr(
    device: vk::Device,
    name: *const c_char,
) -> vk::PFN_vkVoidFunction {
    let real = *GET_DEVICE_PROC_ADDR.get()?;
    // SAFETY: as Vulkan's own `vkGetDeviceProcAddr`; `name` is NUL-terminated.
    let found = unsafe { real(device, name) };
    // SAFETY: as above.
    match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"vkGetDeviceProcAddr" => {
            found.and_then(|_| void_fn(get_device_proc_addr as vk::PFN_vkGetDeviceProcAddr))
        }
        b"vkQueuePresentKHR" => wrap(
            found,
            &QUEUE_PRESENT,
            queue_present as vk::PFN_vkQueuePresentKHR,
        ),
        b"vkAcquireNextImageKHR" => wrap(
            found,
            &ACQUIRE,
            acquire_next_image as vk::PFN_vkAcquireNextImageKHR,
        ),
        b"vkGetDeviceQueue" => wrap(
            found,
            &GET_DEVICE_QUEUE,
            get_device_queue as vk::PFN_vkGetDeviceQueue,
        ),
        _ => found,
    }
}

fn is_srgb(format: vk::Format) -> bool {
    matches!(
        format,
        vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB | vk::Format::A8B8G8R8_SRGB_PACK32
    )
}

/// The formats to report: all of them, or, while sRGB is hidden, those that are not sRGB (all of
/// them if nothing else is left).
fn shown<T: Copy>(all: Vec<T>, format: impl Fn(&T) -> vk::Format) -> Vec<T> {
    if !HIDE_SRGB.load(Ordering::Relaxed) {
        return all;
    }
    let kept: Vec<T> = all
        .iter()
        .copied()
        .filter(|f| !is_srgb(format(f)))
        .collect();
    if kept.is_empty() {
        all
    } else {
        kept
    }
}

/// Vulkan's two-call enumeration over `items`: the count when `out` is null, else as many as fit.
///
/// # Safety
/// `count` is valid; `out`, when not null, holds `*count` writable elements.
unsafe fn enumerate<T: Copy>(items: &[T], count: *mut u32, out: *mut T) -> vk::Result {
    // SAFETY: as the caller promises.
    unsafe {
        if out.is_null() {
            *count = items.len() as u32;
            return vk::Result::SUCCESS;
        }
        let n = (*count as usize).min(items.len());
        std::ptr::copy_nonoverlapping(items.as_ptr(), out, n);
        *count = n as u32;
        if n < items.len() {
            vk::Result::INCOMPLETE
        } else {
            vk::Result::SUCCESS
        }
    }
}

unsafe extern "system" fn surface_formats(
    physical_device: vk::PhysicalDevice,
    surface: vk::SurfaceKHR,
    count: *mut u32,
    formats: *mut vk::SurfaceFormatKHR,
) -> vk::Result {
    let Some(&real) = SURFACE_FORMATS.get() else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let mut n = 0;
    // SAFETY: a count query on the caller's handles.
    let r = unsafe { real(physical_device, surface, &mut n, std::ptr::null_mut()) };
    if r != vk::Result::SUCCESS {
        return r;
    }
    let mut all = vec![vk::SurfaceFormatKHR::default(); n as usize];
    // SAFETY: `all` holds `n` elements.
    let r = unsafe { real(physical_device, surface, &mut n, all.as_mut_ptr()) };
    if r.as_raw() < 0 {
        return r;
    }
    all.truncate(n as usize);
    let shown = shown(all, |f| f.format);
    // SAFETY: the caller's `count` and `formats`, as Vulkan defines them.
    unsafe { enumerate(&shown, count, formats) }
}

unsafe extern "system" fn surface_formats2(
    physical_device: vk::PhysicalDevice,
    info: *const vk::PhysicalDeviceSurfaceInfo2KHR<'_>,
    count: *mut u32,
    formats: *mut vk::SurfaceFormat2KHR<'_>,
) -> vk::Result {
    let Some(&real) = SURFACE_FORMATS2.get() else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let mut n = 0;
    // SAFETY: a count query on the caller's handles.
    let r = unsafe { real(physical_device, info, &mut n, std::ptr::null_mut()) };
    if r != vk::Result::SUCCESS {
        return r;
    }
    let mut all = vec![vk::SurfaceFormat2KHR::default(); n as usize];
    // SAFETY: `all` holds `n` elements, with no chained structures.
    let r = unsafe { real(physical_device, info, &mut n, all.as_mut_ptr()) };
    if r.as_raw() < 0 {
        return r;
    }
    all.truncate(n as usize);
    let shown: Vec<vk::SurfaceFormatKHR> = shown(
        all.iter().map(|f| f.surface_format).collect(),
        |f: &vk::SurfaceFormatKHR| f.format,
    );
    // SAFETY: the caller's `count` and `formats`, as Vulkan defines them; only each element's
    // `surface_format` is written, leaving the caller's `pNext` chain as it was.
    unsafe {
        if formats.is_null() {
            *count = shown.len() as u32;
            return vk::Result::SUCCESS;
        }
        let n = (*count as usize).min(shown.len());
        for (i, f) in shown.iter().take(n).enumerate() {
            (*formats.add(i)).surface_format = *f;
        }
        *count = n as u32;
        if n < shown.len() {
            vk::Result::INCOMPLETE
        } else {
            vk::Result::SUCCESS
        }
    }
}

unsafe extern "system" fn get_device_queue(
    device: vk::Device,
    family: u32,
    index: u32,
    queue: *mut vk::Queue,
) {
    let Some(&real) = GET_DEVICE_QUEUE.get() else {
        return;
    };
    // SAFETY: the caller's call, passed on unchanged; `queue` is then written.
    unsafe { real(device, family, index, queue) };
    // SAFETY: as above.
    super::in_flight::remember_queue(unsafe { *queue });
}

unsafe extern "system" fn acquire_next_image(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    timeout: u64,
    semaphore: vk::Semaphore,
    fence: vk::Fence,
    index: *mut u32,
) -> vk::Result {
    let Some(&real) = ACQUIRE.get() else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    if let Some(&device_fn) = GET_DEVICE_PROC_ADDR.get() {
        // SAFETY: `device` is the live device the acquire is for, `device_fn` its loader.
        unsafe { super::in_flight::before_acquire(device, device_fn) };
    }
    // SAFETY: the caller's acquire, passed on unchanged.
    unsafe { real(device, swapchain, timeout, semaphore, fence, index) }
}

unsafe extern "system" fn queue_present(
    queue: vk::Queue,
    info: *const vk::PresentInfoKHR<'_>,
) -> vk::Result {
    let Some(&real) = QUEUE_PRESENT.get() else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    super::frame::before_present();
    // SAFETY: the caller's present, passed on unchanged.
    let result = super::frame::present(|| unsafe { real(queue, info) });
    super::frame::after_present();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumeration_counts_then_fills() {
        let items = [1u32, 2, 3];
        let mut count = 0;
        // SAFETY: a count query.
        assert_eq!(
            unsafe { enumerate(&items, &mut count, std::ptr::null_mut()) },
            vk::Result::SUCCESS
        );
        assert_eq!(count, 3);
        let mut out = [0u32; 2];
        count = 2;
        // SAFETY: `out` holds `count` elements.
        assert_eq!(
            unsafe { enumerate(&items, &mut count, out.as_mut_ptr()) },
            vk::Result::INCOMPLETE
        );
        assert_eq!(out, [1, 2]);
    }

    #[test]
    fn srgb_formats_are_recognised() {
        assert!(is_srgb(vk::Format::B8G8R8A8_SRGB));
        assert!(!is_srgb(vk::Format::B8G8R8A8_UNORM));
        assert!(!is_srgb(vk::Format::A2B10G10R10_UNORM_PACK32));
    }
}
