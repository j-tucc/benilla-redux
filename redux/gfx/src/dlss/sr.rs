//! DLSS Super Resolution (LOOK-15), through Streamline, in place of Bevy's TAA on a camera `taa.rs`
//! put under DLSS (`TemporalResolve::dlss`), at native resolution (DLAA) or upscaling (Quality,
//! Balanced, Performance, Ultra Performance; each mode's preset is the SDK's default).
//!
//! Each frame, before the view's uniforms, [`prepare`] sets DLSS's options for the mode and the
//! output size and takes the render size `slDLSSGetOptimalSettings` gives. Smaller than the output,
//! it becomes the view's `MainPassResolutionOverride`, as Bevy's own DLSS does
//! (`bevy_anti_alias` 0.18 `dlss/prepare.rs`): the main pass and every pass before the resolve
//! draw into that rectangle of the full-size targets, and DLSS reads it as each input's extent
//! (`node.rs`'s `partial_texture_size`). The camera keeps TAA's inputs, set here every frame: the
//! jitter (Halton (2, 3), 8 x (output / render)^2 phases, the DLSS guide's count), the mip bias
//! (the guide's `log2(render / output) - 1`) and the depth and motion `taa.rs` reprojects after the
//! main pass. The node runs after that motion pass and after every pass that still draws into the
//! jittered image (haze, shafts, rays), before `StartMainPassPostProcessing`, so bloom, the grade
//! and the FFX combine run on the resolved, output-size image as they do after TAA.
//!
//! Each frame the node takes a Streamline frame token for the frame number, sets the frame's
//! constants and evaluates DLSS with the four images tagged locally (`slEvaluateFeature`'s inputs):
//! the current main texture as the input colour, the prepass depth and motion vectors, and the
//! next main texture (`post_process_write`) as the output, which DLSS writes as a storage image.
//!
//! wgpu holds back an encoder's passes and copies until `finish`, while `as_hal_mut` records at
//! once, so the evaluation goes in a fresh encoder submitted after the frame's earlier ones. Before
//! it, wgpu puts the four images into known layouts (`transition_resources`): sampled for the
//! inputs, storage for the output. They are tagged with those layouts; Streamline moves each to
//! what DLSS needs and back, so wgpu's tracking holds afterwards.
//!
//! Motion vectors are Bevy's: uv now minus uv then, over the main pass's rectangle, so `mvecScale`
//! is -1 (Streamline multiplies by the input extent for NGX, the `-render_resolution` Bevy's own
//! DLSS node passes). The jitter, in render pixels, goes in negated, as Bevy's DLSS node passes it.
use super::frame::SlFrame;
use super::inputs::{constants, image};
use super::sl;
use crate::taa::{motion_drawn, MotionLabel, MotionRows, TemporalResolve};
use crate::DlssQuality;
use ash::vk::{self, Handle};
use bevy::{
    camera::MainPassResolutionOverride,
    core_pipeline::{
        core_3d::graph::{Core3d, Node3d},
        prepass::ViewPrepassTextures,
    },
    diagnostic::FrameCount,
    ecs::query::QueryItem,
    prelude::*,
    render::{
        camera::{ExtractedCamera, MipBias, TemporalJitter},
        render_graph::{
            NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
        },
        render_resource::{TextureId, TextureUsages},
        renderer::RenderContext,
        view::{ExtractedView, ViewDepthTexture, ViewTarget},
        Render, RenderApp, RenderSystems,
    },
};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use wgpu::hal::api::Vulkan;

/// The Streamline viewport of the world camera, the only view DLSS resolves.
pub(super) const VIEWPORT: u32 = 1;

/// The jitter phases per render pixel of upscaling: the DLSS programming guide's
/// 8 x (output / render)^2 (section 3.7), as `dlss_wgpu`'s `suggested_jitter`.
const PHASES_PER_SCALE: f32 = 8.0;

/// The radical inverse of `index` in `base`: the Halton sequence's term.
fn halton(mut index: u32, base: u32) -> f32 {
    let mut f = 1.0;
    let mut result = 0.0;
    while index > 0 {
        f /= base as f32;
        result += f * (index % base) as f32;
        index /= base;
    }
    result
}

/// Frame `frame`'s jitter in render pixels for `render` drawn up to `output`: Halton (2, 3) from
/// its first term, less a half, over the guide's phase count.
fn jitter_at(frame: u32, render: UVec2, output: UVec2) -> Vec2 {
    let scale = output.x as f32 / render.x.max(1) as f32;
    let phases = ((PHASES_PER_SCALE * scale * scale).round() as u32).max(1);
    let i = frame % phases + 1;
    Vec2::new(halton(i, 2), halton(i, 3)) - 0.5
}

/// The texture mip bias for `render` drawn up to `output`: the guide's `log2(render / output) - 1`.
fn mip_bias(render: UVec2, output: UVec2) -> f32 {
    (render.x as f32 / output.x.max(1) as f32).log2() - 1.0
}

/// Streamline's `sl::DLSSMode` for a quality mode.
fn sl_mode(quality: DlssQuality) -> u32 {
    match quality {
        DlssQuality::Dlaa => sl::DLSS_MODE_DLAA,
        DlssQuality::Quality => sl::DLSS_MODE_MAX_QUALITY,
        DlssQuality::Balanced => sl::DLSS_MODE_BALANCED,
        DlssQuality::Performance => sl::DLSS_MODE_MAX_PERFORMANCE,
        DlssQuality::UltraPerformance => sl::DLSS_MODE_ULTRA_PERFORMANCE,
    }
}

/// Failed evaluations in a row after which DLSS is given up for the run and the camera falls back
/// to TAA. Deviation: no source; a few frames ride out a transient error, a persistent one stops.
const GIVE_UP_AFTER: u32 = 8;

/// `sl::Result::eErrorInvalidIntegration` (no Vulkan encoder) and `eErrorInvalidParameter` (an
/// image that cannot be tagged), for failures before Streamline is called.
const ERROR_INVALID_INTEGRATION: sl::SlResult = sl::SlResult(19);
const ERROR_INVALID_PARAMETER: sl::SlResult = sl::SlResult(25);

static EVALUATED: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
/// The smallest output DLSS is asked to fill. A minimized window's swap chain is 1x1, where NGX
/// cannot create its feature, and the failures in a row then dropped DLSS to TAA for the
/// session (seen live behind a fullscreen game); below this the view keeps its last options and
/// is not evaluated. Elderoth had no such floor.
const MIN_OUTPUT: u32 = 64;
static FAILED_IN_A_ROW: AtomicU32 = AtomicU32::new(0);
static LAST_ERROR: AtomicI32 = AtomicI32::new(-1);
static RENDER_SIZE: AtomicU64 = AtomicU64::new(0);
static OUTPUT_SIZE: AtomicU64 = AtomicU64::new(0);
/// The mode last set, as its index in `DlssQuality::ALL`.
static MODE: AtomicU8 = AtomicU8::new(0);
static BROKEN: AtomicBool = AtomicBool::new(false);

/// Whether DLSS failed often enough in a row to be given up for the run.
pub(super) fn broken() -> bool {
    BROKEN.load(Ordering::Relaxed)
}

/// What DLSS Super Resolution has done this run, for the probes.
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    /// The mode last set.
    pub mode: DlssQuality,
    /// Frames `slEvaluateFeature` succeeded on.
    pub evaluated: u64,
    /// Frames a Streamline call failed on.
    pub failed: u64,
    /// The render size `slDLSSGetOptimalSettings` gave, and the output size, as set last.
    pub render: (u32, u32),
    pub output: (u32, u32),
    /// The last failing call's `sl::Result`.
    pub last_error: Option<sl::SlResult>,
    pub given_up: bool,
}

/// The counters so far.
pub fn stats() -> Stats {
    let size = |packed: u64| ((packed >> 32) as u32, packed as u32);
    let error = LAST_ERROR.load(Ordering::Relaxed);
    let mode = DlssQuality::ALL[usize::from(MODE.load(Ordering::Relaxed)) % DlssQuality::ALL.len()];
    Stats {
        mode,
        evaluated: EVALUATED.load(Ordering::Relaxed),
        failed: FAILED.load(Ordering::Relaxed),
        render: size(RENDER_SIZE.load(Ordering::Relaxed)),
        output: size(OUTPUT_SIZE.load(Ordering::Relaxed)),
        last_error: (error >= 0).then_some(sl::SlResult(error)),
        given_up: broken(),
    }
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "sr_mode={} sr_render={}x{} sr_output={}x{} sr_evaluated={} sr_errors={} \
             sr_last_error={} sr_given_up={}",
            self.mode.word(),
            self.render.0,
            self.render.1,
            self.output.0,
            self.output.1,
            self.evaluated,
            self.failed,
            self.last_error.map_or("none", |e| e.name()),
            self.given_up
        )
    }
}

fn pack(width: u32, height: u32) -> u64 {
    (u64::from(width) << 32) | u64::from(height)
}

fn failed(call: &str, result: sl::SlResult) {
    FAILED.fetch_add(1, Ordering::Relaxed);
    LAST_ERROR.store(result.0, Ordering::Relaxed);
    let in_a_row = FAILED_IN_A_ROW.fetch_add(1, Ordering::Relaxed) + 1;
    if in_a_row == 1 {
        warn!("DLSS: {call} failed: {}", result.name());
    }
    if in_a_row >= GIVE_UP_AFTER && !BROKEN.swap(true, Ordering::Relaxed) {
        error!("DLSS: {call} failed {in_a_row} frames in a row; falling back to TAA");
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct SrLabel;

pub(crate) struct SrPlugin;

impl Plugin for SrPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<SrState>()
            .add_systems(Render, prepare.in_set(RenderSystems::ManageViews))
            .add_render_graph_node::<ViewNodeRunner<SrNode>>(Core3d, SrLabel)
            .add_render_graph_edges(
                Core3d,
                (MotionLabel, SrLabel, Node3d::StartMainPassPostProcessing),
            )
            .add_render_graph_edges(Core3d, (crate::rays::RaysLabel, SrLabel));
    }
}

/// A view `prepare` may put under DLSS, in the render world.
type PrepareView<'a> = (
    Entity,
    &'a ExtractedCamera,
    &'a ExtractedView,
    Option<&'a TemporalResolve>,
    Option<&'a mut TemporalJitter>,
    Option<&'a mut MipBias>,
    Option<&'a MainPassResolutionOverride>,
);

/// Each DLSS view's options, render size, jitter and mip bias this frame, before its uniforms are
/// written. A view no longer under DLSS (or DLSS given up) loses its render-size override.
fn prepare(
    mut commands: Commands,
    frames: Res<FrameCount>,
    state: Res<SrState>,
    mut views: Query<PrepareView>,
) {
    let mut memory = state.0.lock().unwrap();
    for (entity, camera, view, resolve, jitter, bias, current) in &mut views {
        let wanted = resolve
            .filter(|r| r.dlss && !broken())
            .and_then(|r| Some((r.quality, super::streamline()?)));
        let (Some((quality, sl)), Some(output), Some(mut jitter), Some(mut bias)) =
            (wanted, camera.physical_viewport_size, jitter, bias)
        else {
            if current.is_some() {
                commands
                    .entity(entity)
                    .remove::<MainPassResolutionOverride>();
            }
            continue;
        };
        // A minimized window (1x1): leave DLSS's options as they are and evaluate nothing until
        // the window is back, so the frames it cannot fill never count as failures.
        if output.min_element() < MIN_OUTPUT {
            continue;
        }
        let key = (quality, output, view.hdr);
        if memory.options.map(|(k, _)| k) != Some(key) {
            match set_options(sl, quality, output, view.hdr) {
                Ok(render) => {
                    memory.options = Some((key, render));
                    memory.reset_next = true;
                }
                Err((call, e)) => {
                    failed(call, e);
                    memory.options = None;
                    if current.is_some() {
                        commands
                            .entity(entity)
                            .remove::<MainPassResolutionOverride>();
                    }
                    continue;
                }
            }
        }
        let Some((_, render)) = memory.options else {
            continue;
        };
        if render != output {
            if current.map(|c| c.0) != Some(render) {
                commands
                    .entity(entity)
                    .insert(MainPassResolutionOverride(render));
            }
        } else if current.is_some() {
            commands
                .entity(entity)
                .remove::<MainPassResolutionOverride>();
        }
        jitter.offset = jitter_at(frames.0, render, output);
        bias.0 = mip_bias(render, output);
    }
}

/// `slDLSSSetOptions` for `quality` at `output`, then the render size `slDLSSGetOptimalSettings`
/// gives for it, clamped into the output.
fn set_options(
    sl: &sl::Streamline,
    quality: DlssQuality,
    output: UVec2,
    hdr: bool,
) -> Result<UVec2, (&'static str, sl::SlResult)> {
    let options = sl::DlssOptions {
        mode: sl_mode(quality),
        output_width: output.x,
        output_height: output.y,
        color_buffers_hdr: if hdr { sl::TRUE } else { sl::FALSE },
        ..Default::default()
    };
    let set = sl.dlss_set_options(&sl::ViewportHandle::new(VIEWPORT), &options);
    if !set.ok() {
        return Err(("slDLSSSetOptions", set));
    }
    let o = sl
        .dlss_optimal_settings(&options)
        .map_err(|e| ("slDLSSGetOptimalSettings", e))?;
    let render = UVec2::new(o.optimal_render_width, o.optimal_render_height)
        .clamp(UVec2::ONE, output.max(UVec2::ONE));
    RENDER_SIZE.store(pack(render.x, render.y), Ordering::Relaxed);
    OUTPUT_SIZE.store(pack(output.x, output.y), Ordering::Relaxed);
    let index = DlssQuality::ALL.iter().position(|q| *q == quality);
    MODE.store(index.unwrap_or(0) as u8, Ordering::Relaxed);
    info!(
        "DLSS: {} at {}x{}, render size {}x{} (SDK range {}x{} to {}x{})",
        quality.word(),
        output.x,
        output.y,
        render.x,
        render.y,
        o.render_width_min,
        o.render_height_min,
        o.render_width_max,
        o.render_height_max
    );
    Ok(render)
}

/// What the systems keep between frames: the options last set (mode, output size, HDR) with the
/// render size they gave, whether the next evaluation drops the history, and the main textures
/// the node has cleared once so wgpu counts them written (wgpu cannot see DLSS's write).
#[derive(Resource, Default)]
struct SrState(Mutex<SrMemory>);

/// The options' key: the mode, the output size and whether the colour is HDR.
type OptionsKey = (DlssQuality, UVec2, bool);

#[derive(Default)]
struct SrMemory {
    options: Option<(OptionsKey, UVec2)>,
    reset_next: bool,
    cleared: Vec<TextureId>,
}

/// The four images of one evaluation, and the rectangle of the inputs this frame drew.
struct Images {
    color_in: sl::VkImage,
    color_out: sl::VkImage,
    depth: sl::VkImage,
    motion: sl::VkImage,
    render: UVec2,
}

/// `slEvaluateFeature` for DLSS on the world viewport, the images tagged locally.
///
/// # Safety
/// `command_buffer` is recording, outside a render pass, and is submitted after the work that
/// wrote the inputs and put the images into the layouts tagged.
unsafe fn evaluate(
    sl: &sl::Streamline,
    frame: *mut sl::FrameToken,
    images: &Images,
    command_buffer: vk::CommandBuffer,
) -> sl::SlResult {
    let viewport = sl::ViewportHandle::new(VIEWPORT);
    let mut resources = [
        images.color_in,
        images.color_out,
        images.depth,
        images.motion,
    ]
    .map(sl::Resource::vulkan);
    // The depth image's view is its depth aspect (NGX takes colour when no range is chained).
    let mut depth_aspect = sl::SubresourceRange::aspect(vk::ImageAspectFlags::DEPTH.as_raw());
    resources[2].base.next = &mut depth_aspect.base;
    let [color_in, color_out, depth, motion] = resources.each_mut();
    let lifecycle = sl::LIFECYCLE_VALID_UNTIL_EVALUATE;
    // The inputs hold the frame in the main pass's rectangle at the origin; the output is whole.
    let input = sl::Extent {
        top: 0,
        left: 0,
        width: images.render.x,
        height: images.render.y,
    };
    let tag = |resource, buffer, extent| {
        let mut tag = sl::ResourceTag::new(resource, buffer, lifecycle);
        tag.extent = extent;
        tag
    };
    let tags = [
        tag(color_in, sl::BUFFER_SCALING_INPUT_COLOR, input),
        tag(
            color_out,
            sl::BUFFER_SCALING_OUTPUT_COLOR,
            sl::Extent::default(),
        ),
        tag(depth, sl::BUFFER_DEPTH, input),
        tag(motion, sl::BUFFER_MOTION_VECTORS, input),
    ];
    let inputs = [
        &viewport.base as *const sl::BaseStructure,
        &tags[0].base,
        &tags[1].base,
        &tags[2].base,
        &tags[3].base,
    ];
    // SAFETY: everything `inputs` points at lives to the end of this call; as the caller promises.
    unsafe {
        sl.evaluate(
            sl::FEATURE_DLSS,
            frame,
            &inputs,
            command_buffer.as_raw() as usize as *mut c_void,
        )
    }
}

#[derive(Default)]
struct SrNode;

impl ViewNode for SrNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static ViewDepthTexture,
        &'static ViewPrepassTextures,
        &'static ExtractedView,
        &'static TemporalJitter,
        &'static TemporalResolve,
        &'static MotionRows,
        Option<&'static MainPassResolutionOverride>,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        context: &mut RenderContext<'w>,
        (target, view_depth, prepass, view, jitter, resolve, rows, resolution): QueryItem<
            'w,
            '_,
            Self::ViewQuery,
        >,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let Some(sl) = super::streamline() else {
            return Ok(());
        };
        if !resolve.dlss || broken() || !motion_drawn(view_depth, prepass, world) {
            return Ok(());
        }
        let (Some(depth), Some(motion)) = (&prepass.depth, &prepass.motion_vectors) else {
            return Ok(());
        };
        let main = target.main_texture();
        let size = main.size();
        // The storage usage reaches the main textures a frame after the switch. The prepass
        // textures are full size; the main pass drew into the override's rectangle of them.
        if !main.usage().contains(TextureUsages::STORAGE_BINDING)
            || depth.texture.texture.size() != size
            || motion.texture.texture.size() != size
        {
            return Ok(());
        }
        let mut memory = world.resource::<SrState>().0.lock().unwrap();
        // DLSS takes the rectangle its options were set for: `prepare` set both this frame.
        let Some(((quality, output, _), render)) = memory.options else {
            return Ok(());
        };
        let drawn = resolution.map_or(output, |r| r.0);
        if output != UVec2::new(size.width, size.height)
            || drawn != render
            || resolve.quality != quality
        {
            return Ok(());
        }
        let viewport = sl::ViewportHandle::new(VIEWPORT);
        let reset = resolve.reset || memory.reset_next;
        // The frame's token from the main world's frame start (`super::frame`), else one for the
        // frame number when Streamline's markers are not running.
        let carried = world
            .get_resource::<SlFrame>()
            .and_then(|f| Some((f.index, f.token()?)));
        let (index, frame) = match carried {
            Some(carried) => carried,
            None => {
                let index = world.resource::<FrameCount>().0;
                match sl.new_frame_token(index) {
                    Ok(frame) => (index, frame),
                    Err(e) => {
                        failed("slGetNewFrameToken", e);
                        return Ok(());
                    }
                }
            }
        };
        let set = sl.set_constants(
            &constants(&view.world_from_view, jitter.offset, rows, reset),
            frame,
            &viewport,
        );
        if !set.ok() {
            failed("slSetConstants", set);
            return Ok(());
        }
        super::fg::constants_set(index);
        let post = target.post_process_write();
        let images = (|| {
            use wgpu::TextureUses as U;
            Some(Images {
                color_in: image(post.source_texture, post.source, U::RESOURCE)?,
                color_out: image(
                    post.destination_texture,
                    post.destination,
                    U::STORAGE_READ_WRITE,
                )?,
                depth: image(
                    &depth.texture.texture,
                    &depth.texture.default_view,
                    U::RESOURCE,
                )?,
                motion: image(
                    &motion.texture.texture,
                    &motion.texture.default_view,
                    U::RESOURCE,
                )?,
                render,
            })
        })();
        let Some(images) = images else {
            // Not a Vulkan texture, or a format DLSS is not given: the source stays the image.
            target.post_process_write();
            failed("tagging", ERROR_INVALID_PARAMETER);
            return Ok(());
        };
        let device = context.render_device().clone();
        let encoder = context.command_encoder();
        let destination = post.destination_texture.id();
        if !memory.cleared.contains(&destination) {
            // wgpu would zero a main texture it never saw written when it is next read.
            if device.features().contains(wgpu::Features::CLEAR_TEXTURE) {
                encoder.clear_texture(post.destination_texture, &Default::default());
            }
            if memory.cleared.len() >= 8 {
                memory.cleared.remove(0);
            }
            memory.cleared.push(destination);
        }
        let transition = |texture, state| wgpu::TextureTransition {
            texture,
            selector: None,
            state,
        };
        encoder.transition_resources(
            std::iter::empty(),
            [
                transition(&**post.source_texture, wgpu::TextureUses::RESOURCE),
                transition(&depth.texture.texture, wgpu::TextureUses::RESOURCE),
                transition(&motion.texture.texture, wgpu::TextureUses::RESOURCE),
                transition(
                    &**post.destination_texture,
                    wgpu::TextureUses::STORAGE_READ_WRITE,
                ),
            ]
            .into_iter(),
        );
        let mut raw =
            device
                .wgpu_device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("redux_dlss_sr"),
                });
        // SAFETY: the fresh encoder records nothing else; it is submitted after the encoder above,
        // which wrote nothing the evaluation reads after the transitions.
        let result = unsafe {
            raw.as_hal_mut::<Vulkan, _, _>(|encoder| {
                encoder.map(|encoder| evaluate(sl, frame, &images, encoder.raw_handle()))
            })
        };
        context.add_command_buffer(raw.finish());
        match result {
            Some(r) if r.ok() => {
                EVALUATED.fetch_add(1, Ordering::Relaxed);
                FAILED_IN_A_ROW.store(0, Ordering::Relaxed);
                memory.reset_next = false;
            }
            other => {
                // Nothing was written: the source stays the main texture.
                target.post_process_write();
                failed(
                    "slEvaluateFeature",
                    other.unwrap_or(ERROR_INVALID_INTEGRATION),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_jitter_has_eight_phases_and_upscaling_more() {
        let native = UVec2::new(3840, 2160);
        // Halton (2, 3) from its first term, less a half.
        let first = jitter_at(0, native, native);
        assert!(
            (first - Vec2::new(0.0, -1.0 / 6.0)).length() < 1e-6,
            "{first}"
        );
        assert_eq!(jitter_at(8, native, native), jitter_at(0, native, native));
        // Performance at 4K renders 1920x1080: 32 phases.
        let half = UVec2::new(1920, 1080);
        assert_ne!(jitter_at(8, half, native), jitter_at(0, half, native));
        assert_eq!(jitter_at(32, half, native), jitter_at(0, half, native));
        for f in 0..64 {
            let j = jitter_at(f, half, native);
            assert!(j.x.abs() <= 0.5 && j.y.abs() <= 0.5);
        }
    }

    #[test]
    fn the_mip_bias_follows_the_guide() {
        let out = UVec2::new(3840, 2160);
        assert!((mip_bias(out, out) + 1.0).abs() < 1e-6);
        assert!((mip_bias(UVec2::new(1920, 1080), out) + 2.0).abs() < 1e-6);
    }

    #[test]
    fn modes_map_to_streamlines() {
        assert_eq!(sl_mode(DlssQuality::Dlaa), 6);
        assert_eq!(sl_mode(DlssQuality::Quality), 3);
        assert_eq!(sl_mode(DlssQuality::Balanced), 2);
        assert_eq!(sl_mode(DlssQuality::Performance), 1);
        assert_eq!(sl_mode(DlssQuality::UltraPerformance), 4);
    }

    #[test]
    fn stats_print_the_probe_words() {
        let line = stats().to_string();
        assert!(line.starts_with("sr_mode=dlaa sr_render="));
        assert!(line.contains("sr_errors=") && !line.contains("fail"));
    }
}
