//! One frame in flight while Frame Generation is on (LOOK-15).
//!
//! With DLSS-G generating, a run of camera cuts every quarter second (the `dlss_fg_cuts` probe)
//! stalled the frame for seconds about half the runs: DLSS-G's present timed out waiting for a
//! semaphore (`waitCPUFence ... timed out`, its 500 ms `kMaxSemaphoreWaitUs`, Streamline 2.14.16
//! `vulkan.cpp`), then wgpu-hal's wait for the previous submission saw a stalled GPU for 1 to 10
//! seconds (its 1 s retry loop). Neither the acquire's fence, the reset flag nor the back-buffer
//! tag changed it. Holding the CPU to one frame ahead of the GPU did: before each acquire, wait
//! for the frame before last to finish on the GPU. With DLSS-G paced by Reflex (`slReflexSleep`,
//! which DLSS-G needs, DLSS-G guide section 8) the CPU waits there anyway, so the limit costs no
//! frame rate (121 rendered fps at 4K with it and without it).
//!
//! The wait is a ring of two fences on the queue wgpu submits on: each acquire waits on the fence
//! the previous acquire submitted (signalled once everything submitted before it, the frame
//! before last, is done), then resubmits its own slot's fence with an empty `vkQueueSubmit`.
//! Only the render thread acquires and submits (Bevy's `prepare_windows` and `render_system`, one
//! after the other), so the queue is never submitted to from two threads at once.
use ash::vk;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;

/// How long an acquire waits for the frame before last before going on without it.
/// Deviation: no source; past DLSS-G's own 500 ms timeout, a wait this long is a stall the
/// limit cannot clear, and wgpu's own waits report it.
const WAIT_NS: u64 = 1_000_000_000;

/// The queue wgpu submits on, as a raw handle (0 before `vkGetDeviceQueue`).
static QUEUE: AtomicU64 = AtomicU64::new(0);
/// The ring's two fences, created on first use.
static RING: OnceLock<Ring> = OnceLock::new();
/// Acquires so far: the slot this acquire submits is `count % 2`.
static COUNT: AtomicUsize = AtomicUsize::new(0);

struct Ring {
    fences: [vk::Fence; 2],
    wait: vk::PFN_vkWaitForFences,
    reset: vk::PFN_vkResetFences,
    submit: vk::PFN_vkQueueSubmit,
}

/// Remember the queue wgpu-hal got (it takes one, its first family's first).
pub(super) fn remember_queue(queue: vk::Queue) {
    QUEUE.store(vk::Handle::as_raw(queue), Ordering::Relaxed);
}

/// The device function `name`, as type `F`.
///
/// # Safety
/// `F` is the function pointer type of `name`.
unsafe fn load<F: Copy>(
    device: vk::Device,
    device_fn: vk::PFN_vkGetDeviceProcAddr,
    name: &std::ffi::CStr,
) -> Option<F> {
    // SAFETY: a live device and a NUL-terminated name.
    let f = unsafe { device_fn(device, name.as_ptr()) }?;
    // SAFETY: the caller names `F` as `name`'s type.
    Some(unsafe { std::mem::transmute_copy::<unsafe extern "system" fn(), F>(&f) })
}

/// The ring for `device`: two fences created signalled, and the calls it makes.
///
/// # Safety
/// `device` is the live device and `device_fn` its `vkGetDeviceProcAddr`.
unsafe fn ring(device: vk::Device, device_fn: vk::PFN_vkGetDeviceProcAddr) -> Option<Ring> {
    // SAFETY: each type is its name's.
    let create: vk::PFN_vkCreateFence = unsafe { load(device, device_fn, c"vkCreateFence") }?;
    let mut fences = [vk::Fence::null(); 2];
    let info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
    for fence in &mut fences {
        // SAFETY: a valid create info, no allocator, one fence written.
        let result = unsafe { create(device, &info, std::ptr::null(), fence) };
        if result != vk::Result::SUCCESS {
            return None;
        }
    }
    Some(Ring {
        fences,
        // SAFETY: as above.
        wait: unsafe { load(device, device_fn, c"vkWaitForFences") }?,
        reset: unsafe { load(device, device_fn, c"vkResetFences") }?,
        submit: unsafe { load(device, device_fn, c"vkQueueSubmit") }?,
    })
}

/// Before an acquire: while Frame Generation is generating, wait for the frame before last to
/// finish on the GPU and mark where this one starts. Does nothing otherwise, or before the queue
/// is known.
///
/// # Safety
/// `device` is the live device the acquire is for and `device_fn` its `vkGetDeviceProcAddr`;
/// called on the thread that submits to wgpu's queue.
pub(super) unsafe fn before_acquire(device: vk::Device, device_fn: vk::PFN_vkGetDeviceProcAddr) {
    let queue = QUEUE.load(Ordering::Relaxed);
    if queue == 0 || !super::fg::generating() {
        return;
    }
    let Some(ring) = RING.get().or_else(|| {
        // SAFETY: as this function's own contract.
        let made = unsafe { ring(device, device_fn) }?;
        Some(RING.get_or_init(|| made))
    }) else {
        return;
    };
    let count = COUNT.fetch_add(1, Ordering::Relaxed);
    let (previous, own) = (ring.fences[(count + 1) % 2], ring.fences[count % 2]);
    // SAFETY: fences of this device. A timeout goes on: the limit is pacing, not correctness.
    // `own` was submitted before `previous`, on the same queue, so it is signalled too unless
    // the wait timed out, and then waiting on it bounds the reset below.
    unsafe {
        let _ = (ring.wait)(device, 1, &previous, vk::TRUE, WAIT_NS);
        if (ring.wait)(device, 1, &own, vk::TRUE, WAIT_NS) != vk::Result::SUCCESS {
            return;
        }
        let _ = (ring.reset)(device, 1, &own);
        let _ = (ring.submit)(vk::Handle::from_raw(queue), 0, std::ptr::null(), own);
    }
}
