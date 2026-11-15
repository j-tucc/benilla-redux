//! Streamline's frame tokens and the Reflex/PCL latency markers (the Reflex guide, sections 4-6;
//! the PCL guide, section 3; the Streamline guide, section 2.10).
//!
//! Each main-world frame takes a token for its own index at its start (`First`), sleeps there in
//! `slReflexSleep` (Reflex's pacing; it returns at once with Reflex off) and marks the simulation's
//! start, then marks its end in `Last`. Bevy renders that frame a frame later, on the render
//! thread (pipelined rendering), so the token travels with it through extract as [`SlFrame`]: the
//! render world marks the render submission's start before `render_system`, DLSS sets its
//! constants and evaluates on it, Frame Generation tags its inputs for it, and the present markers
//! carry it, sent by [`super::vk_shim`] around the real `vkQueuePresentKHR`. The markers and the
//! sleep run every frame whether Reflex is on or not, as the guide requires; Reflex's low-latency
//! mode is on while Frame Generation is chosen, which DLSS-G needs (DLSS-G guide, section 8).
use super::{sl, streamline};
use crate::FrameGen;
use bevy::prelude::*;
use bevy::render::{
    extract_resource::{ExtractResource, ExtractResourcePlugin},
    renderer::render_system,
    Render, RenderApp, RenderSystems,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// The Streamline frame the main world is simulating, or, in the render world, the one being
/// rendered: its index and its token (null before the first).
#[derive(Resource, Clone, Copy, Debug, Default, ExtractResource)]
pub(crate) struct SlFrame {
    pub(crate) index: u32,
    token: usize,
}

impl SlFrame {
    /// The token, once there is one.
    pub(crate) fn token(&self) -> Option<*mut sl::FrameToken> {
        (self.token != 0).then_some(self.token as *mut sl::FrameToken)
    }
}

/// The frame the render thread is submitting and about to present, for the present markers.
static PRESENTING: Mutex<Option<(u32, usize)>> = Mutex::new(None);

/// Time spent in `slReflexSleep` and in the real present (DLSS-G's present hook, its queue work
/// and its pacing), in ns, and how many of each, for the perf probe.
static SLEEP_NS: AtomicU64 = AtomicU64::new(0);
static SLEEPS: AtomicU64 = AtomicU64::new(0);
static PRESENT_NS: AtomicU64 = AtomicU64::new(0);
static PRESENT_CALLS: AtomicU64 = AtomicU64::new(0);

/// Where frame time goes outside the game's own work: the totals so far.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pacing {
    /// `slReflexSleep` calls and their summed time (ns).
    pub sleeps: u64,
    pub sleep_ns: u64,
    /// Real presents and their summed time (ns).
    pub presents: u64,
    pub present_ns: u64,
}

/// The pacing totals so far.
pub fn pacing() -> Pacing {
    Pacing {
        sleeps: SLEEPS.load(Ordering::Relaxed),
        sleep_ns: SLEEP_NS.load(Ordering::Relaxed),
        presents: PRESENT_CALLS.load(Ordering::Relaxed),
        present_ns: PRESENT_NS.load(Ordering::Relaxed),
    }
}

/// Times `f` into a total and its count.
pub(super) fn timed<T>(total: &AtomicU64, count: &AtomicU64, f: impl FnOnce() -> T) -> T {
    let at = Instant::now();
    let out = f();
    total.fetch_add(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
    count.fetch_add(1, Ordering::Relaxed);
    out
}

/// The real present, timed.
pub(super) fn present<T>(f: impl FnOnce() -> T) -> T {
    timed(&PRESENT_NS, &PRESENT_CALLS, f)
}

/// Whether the markers can be sent: the device came up through Streamline with PCL loaded.
fn markers_ready() -> bool {
    matches!(super::status(), Some(super::Status::Ready { pcl, .. }) if pcl.supported())
        && streamline().is_some()
}

pub(crate) struct FramePlugin;

impl Plugin for FramePlugin {
    fn build(&self, app: &mut App) {
        if !markers_ready() {
            return;
        }
        app.init_resource::<SlFrame>()
            .add_plugins(ExtractResourcePlugin::<SlFrame>::default())
            .add_systems(First, begin_frame)
            .add_systems(Last, end_frame);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(
                Render,
                submit_start
                    .in_set(RenderSystems::Render)
                    .before(render_system),
            );
        }
    }
}

/// The next frame's token, Reflex's options while they change, its sleep and the simulation's
/// start.
fn begin_frame(
    mut frame: ResMut<SlFrame>,
    frame_gen: Res<FrameGen>,
    mut reflex: Local<Option<i32>>,
    mut warned: Local<bool>,
) {
    let Some(sl) = streamline() else {
        return;
    };
    let index = frame.index.wrapping_add(1).max(1);
    let token = match sl.new_frame_token(index) {
        Ok(token) => token,
        Err(e) => {
            if !std::mem::replace(&mut *warned, true) {
                warn!("Streamline: slGetNewFrameToken failed: {}", e.name());
            }
            *frame = SlFrame::default();
            return;
        }
    };
    let mode = if frame_gen.on && super::fg_ready() {
        sl::REFLEX_MODE_LOW_LATENCY
    } else {
        sl::REFLEX_MODE_OFF
    };
    if *reflex != Some(mode) {
        let set = sl.reflex_set_options(&sl::ReflexOptions::new(mode));
        if set.ok() {
            info!(
                "Reflex: low latency {}",
                if mode == sl::REFLEX_MODE_OFF {
                    "off"
                } else {
                    "on"
                }
            );
        } else {
            warn!("Reflex: slReflexSetOptions failed: {}", set.name());
        }
        *reflex = Some(mode);
    }
    timed(&SLEEP_NS, &SLEEPS, || sl.reflex_sleep(token));
    sl.pcl_marker(sl::PCL_SIMULATION_START, token);
    *frame = SlFrame {
        index,
        token: token as usize,
    };
}

fn end_frame(frame: Res<SlFrame>) {
    if let (Some(sl), Some(token)) = (streamline(), frame.token()) {
        sl.pcl_marker(sl::PCL_SIMULATION_END, token);
    }
}

/// The render submission's start, and the frame the present will carry.
fn submit_start(frame: Option<Res<SlFrame>>) {
    let (Some(sl), Some(frame)) = (streamline(), frame) else {
        return;
    };
    let Some(token) = frame.token() else {
        return;
    };
    *PRESENTING.lock().unwrap() = Some((frame.index, token as usize));
    sl.pcl_marker(sl::PCL_RENDER_SUBMIT_START, token);
}

/// Just before the real present, on the presenting thread: the submission's end, Frame
/// Generation's options for this present, and the present's start.
pub(super) fn before_present() {
    let Some(sl) = streamline() else {
        return;
    };
    let Some((index, token)) = *PRESENTING.lock().unwrap() else {
        return;
    };
    let token = token as *mut sl::FrameToken;
    sl.pcl_marker(sl::PCL_RENDER_SUBMIT_END, token);
    super::fg::before_present(sl, index, token);
    sl.pcl_marker(sl::PCL_PRESENT_START, token);
}

/// Just after the real present: the present's end and Frame Generation's counts.
pub(super) fn after_present() {
    let Some(sl) = streamline() else {
        return;
    };
    let Some((_, token)) = PRESENTING.lock().unwrap().take() else {
        return;
    };
    sl.pcl_marker(sl::PCL_PRESENT_END, token as *mut sl::FrameToken);
    super::fg::after_present(sl);
}
