//! DLSS Frame Generation (DLSS-G, LOOK-15): 2x, one generated frame after each rendered one (the
//! most an RTX 40 offers), on a camera under a temporal resolve (DLSS Super Resolution or TAA),
//! while [`FrameGen`] is on and not held.
//!
//! DLSS-G runs inside the present (Streamline's swap-chain proxy): it needs no evaluation, only
//! its inputs tagged for the frame and its mode set before the present (DLSS-G guide, sections 5
//! and 6). Each frame [`HudlessNode`], after the last post-processing pass and before the UI
//! passes (bevy_ui's and egui's, both on the world camera), copies the finished image into a
//! back-buffer-sized texture of the back buffer's own format with Bevy's blit pipeline, the one the
//! final upscaling pass draws the back buffer with, so the two match but for the UI. It then tags
//! that texture as the HUD-less colour and the prepass depth and motion vectors over the main
//! pass's rectangle, and the whole back buffer's rectangle (a null back-buffer tag), all valid
//! until present: nothing writes them again this frame, and DLSS-G
//! blocks the queue the next frame's work goes on until it has read them (the default
//! `eBlockPresentingClientQueue`). Under TAA it sets the frame's constants itself; under DLSS the
//! Super Resolution node has.
//!
//! On the presenting thread, just before each present ([`super::vk_shim`]), [`before_present`]
//! turns generation on when it is wanted and this frame's inputs were tagged, and off otherwise:
//! in a pausing menu or a loading screen ([`FrameGen::hold`]), for a few frames after the window's
//! size or present mode changed, on MSAA or Classic (no motion vectors), and when the frame drew
//! no world. Off keeps DLSS-G's resources (`eRetainResourcesWhenOff`, the guide's advice for
//! short pauses) and nulls the tags; turning the choice off frees them after the next present.
//! [`after_present`] counts what DLSS-G presented (`slDLSSGGetState`) for the probes.
use super::frame::SlFrame;
use super::inputs::{constants, image};
use super::sl;
use super::sr::VIEWPORT;
use crate::taa::{motion_drawn, MotionRows, TemporalResolve};
use crate::FrameGen;
use ash::vk;
use bevy::{
    camera::MainPassResolutionOverride,
    core_pipeline::{
        blit::{BlitPipeline, BlitPipelineKey},
        core_3d::graph::{Core3d, Node3d},
        prepass::ViewPrepassTextures,
    },
    ecs::query::QueryItem,
    prelude::*,
    render::{
        camera::{ExtractedCamera, TemporalJitter},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_graph::{
            NodeRunError, RenderGraph, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode,
            ViewNodeRunner,
        },
        render_resource::*,
        renderer::{RenderContext, RenderDevice},
        view::{window::ExtractedWindows, ExtractedView, ViewDepthTexture, ViewTarget},
        Render, RenderApp, RenderStartup, RenderSystems,
    },
};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

/// Presents after the window's size or present mode changed during which generation stays off.
/// Deviation: no source; the guide only says to turn it off around window changes.
const HOLD_AFTER_WINDOW_CHANGE: u32 = 10;
/// Presents in a row with DLSS-G reporting a failing status after which generation is given up
/// for the run. Deviation: no source; a transient status clears, a persistent one stops.
const GIVE_UP_AFTER: u32 = 120;
/// Frames a replaced HUD-less texture is kept alive: DLSS-G may still read it on its own queue.
/// Deviation: no source; more frames than are ever in flight.
const RETIRE_FRAMES: u32 = 8;

/// Whether generation is wanted (chosen, not held, no recent window change) and chosen at all.
static WANT: AtomicBool = AtomicBool::new(false);
static CHOSEN: AtomicBool = AtomicBool::new(false);
/// The frame whose inputs were tagged, and the frame whose constants were set.
static TAGGED: AtomicU32 = AtomicU32::new(0);
static CONSTANTS: AtomicU32 = AtomicU32::new(0);
/// DLSS-G's mode as last set, and whether its resources are kept from having been on.
static ON: AtomicBool = AtomicBool::new(false);
static NEEDS_FREE: AtomicBool = AtomicBool::new(false);
static BROKEN: AtomicBool = AtomicBool::new(false);
static BAD_IN_A_ROW: AtomicU32 = AtomicU32::new(0);
static PRESENTS: AtomicU64 = AtomicU64::new(0);
static PRESENTS_ON: AtomicU64 = AtomicU64::new(0);
static SHOWN: AtomicU64 = AtomicU64::new(0);
static TURNED_ON: AtomicU64 = AtomicU64::new(0);
static ERRORS: AtomicU64 = AtomicU64::new(0);
static LAST_ERROR: AtomicI32 = AtomicI32::new(-1);
static STATUS: AtomicU32 = AtomicU32::new(0);
static MAX_GENERATED: AtomicU32 = AtomicU32::new(0);

/// Whether DLSS-G's mode was last set on (frames are being generated).
pub(super) fn generating() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Whether DLSS-G reported a failing status long enough to be given up for the run.
pub(super) fn broken() -> bool {
    BROKEN.load(Ordering::Relaxed)
}

/// The Super Resolution node set `index`'s constants.
pub(super) fn constants_set(index: u32) {
    CONSTANTS.store(index, Ordering::Relaxed);
}

fn failed(call: &str, result: sl::SlResult) {
    if ERRORS.fetch_add(1, Ordering::Relaxed) == 0 {
        warn!("Frame generation: {call} failed: {}", result.name());
    }
    LAST_ERROR.store(result.0, Ordering::Relaxed);
}

/// What Frame Generation has done this run, for the probes.
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    /// DLSS-G's mode now.
    pub on: bool,
    /// Every present the game made, those made with generation on, and the frames DLSS-G
    /// reported presenting for the latter (rendered plus generated).
    pub presents: u64,
    pub presents_on: u64,
    pub shown: u64,
    /// Times generation was turned on.
    pub turned_on: u64,
    /// DLSS-G's last status bits (`sl::DLSSGStatus`) and its most generated frames per frame.
    pub status: u32,
    pub max_generated: u32,
    pub errors: u64,
    pub last_error: Option<sl::SlResult>,
    pub given_up: bool,
}

/// The counters so far.
pub fn stats() -> Stats {
    let error = LAST_ERROR.load(Ordering::Relaxed);
    Stats {
        on: ON.load(Ordering::Relaxed),
        presents: PRESENTS.load(Ordering::Relaxed),
        presents_on: PRESENTS_ON.load(Ordering::Relaxed),
        shown: SHOWN.load(Ordering::Relaxed),
        turned_on: TURNED_ON.load(Ordering::Relaxed),
        status: STATUS.load(Ordering::Relaxed),
        max_generated: MAX_GENERATED.load(Ordering::Relaxed),
        errors: ERRORS.load(Ordering::Relaxed),
        last_error: (error >= 0).then_some(sl::SlResult(error)),
        given_up: broken(),
    }
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "fg_on={} fg_status={} fg_presents={} fg_presents_on={} fg_shown={} fg_turned_on={} \
             fg_max_generated={} fg_errors={} fg_last_error={} fg_given_up={}",
            self.on,
            sl::dlssg_status_names(self.status),
            self.presents,
            self.presents_on,
            self.shown,
            self.turned_on,
            self.max_generated,
            self.errors,
            self.last_error.map_or("none", |e| e.name()),
            self.given_up
        )
    }
}

/// Before the present of frame `index`, on the presenting thread: DLSS-G's mode for it.
pub(super) fn before_present(sl: &sl::Streamline, index: u32, token: *mut sl::FrameToken) {
    let want = WANT.load(Ordering::Relaxed) && !broken() && TAGGED.load(Ordering::Relaxed) == index;
    if want == ON.load(Ordering::Relaxed) {
        return;
    }
    let options = sl::DlssgOptions {
        mode: if want {
            sl::DLSSG_MODE_ON
        } else {
            sl::DLSSG_MODE_OFF
        },
        num_frames_to_generate: 1,
        flags: sl::DLSSG_FLAG_RETAIN_RESOURCES_WHEN_OFF,
        ..Default::default()
    };
    let viewport = sl::ViewportHandle::new(VIEWPORT);
    let set = sl.dlssg_set_options(&viewport, &options);
    if !set.ok() {
        failed("slDLSSGSetOptions", set);
        return;
    }
    ON.store(want, Ordering::Relaxed);
    if want {
        TURNED_ON.fetch_add(1, Ordering::Relaxed);
        NEEDS_FREE.store(true, Ordering::Relaxed);
        info!("Frame generation: on (2x)");
    } else {
        // Nothing tagged stays referenced while generation is off.
        let tags = [
            sl::BUFFER_DEPTH,
            sl::BUFFER_MOTION_VECTORS,
            sl::BUFFER_HUDLESS_COLOR,
            sl::BUFFER_BACKBUFFER,
        ]
        .map(|b| sl::ResourceTag::new(std::ptr::null_mut(), b, sl::LIFECYCLE_VALID_UNTIL_PRESENT));
        // SAFETY: null tags, for a frame's token.
        let cleared = unsafe { sl.set_tags_for_frame(token, &viewport, &tags) };
        if !cleared.ok() {
            failed("slSetTagForFrame (null)", cleared);
        }
        info!("Frame generation: off");
    }
}

/// After a present: what DLSS-G presented for it, and its resources freed once it is no longer
/// chosen.
pub(super) fn after_present(sl: &sl::Streamline) {
    PRESENTS.fetch_add(1, Ordering::Relaxed);
    let viewport = sl::ViewportHandle::new(VIEWPORT);
    if !ON.load(Ordering::Relaxed) {
        if !CHOSEN.load(Ordering::Relaxed) && NEEDS_FREE.swap(false, Ordering::Relaxed) {
            let freed = sl.dlssg_free_resources(&viewport);
            if !freed.ok() {
                failed("slFreeResources", freed);
            }
        }
        return;
    }
    PRESENTS_ON.fetch_add(1, Ordering::Relaxed);
    match sl.dlssg_state(&viewport) {
        Ok(state) => {
            SHOWN.fetch_add(
                u64::from(state.num_frames_actually_presented),
                Ordering::Relaxed,
            );
            STATUS.store(state.status, Ordering::Relaxed);
            MAX_GENERATED.store(state.num_frames_to_generate_max, Ordering::Relaxed);
            if state.status == 0 {
                BAD_IN_A_ROW.store(0, Ordering::Relaxed);
            } else {
                let bad = BAD_IN_A_ROW.fetch_add(1, Ordering::Relaxed) + 1;
                if bad == 1 {
                    warn!(
                        "Frame generation: DLSS-G status {}",
                        sl::dlssg_status_names(state.status)
                    );
                }
                if bad >= GIVE_UP_AFTER && !BROKEN.swap(true, Ordering::Relaxed) {
                    error!("Frame generation: DLSS-G status failing {bad} presents; giving up");
                }
            }
        }
        Err(e) => failed("slDLSSGGetState", e),
    }
}

/// [`FrameGen`] in the render world: whether generation is chosen, and wanted this frame.
#[derive(Resource, Clone, Copy, Default)]
struct Wanted {
    chosen: bool,
    now: bool,
}

impl ExtractResource for Wanted {
    type Source = FrameGen;

    fn extract_resource(source: &FrameGen) -> Self {
        let chosen = source.on && super::fg_ready();
        Wanted {
            chosen,
            now: chosen && !source.hold,
        }
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
pub struct HudlessLabel;

pub(crate) struct FgPlugin;

impl Plugin for FgPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractResourcePlugin::<Wanted>::default());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<Wanted>()
            .init_resource::<HudlessTextures>()
            .add_systems(RenderStartup, order_before_ui)
            .add_systems(Render, follow.in_set(RenderSystems::ManageViews))
            .add_systems(
                Render,
                prepare_hudless.in_set(RenderSystems::PrepareResources),
            )
            .add_render_graph_node::<ViewNodeRunner<HudlessNode>>(Core3d, HudlessLabel);
    }
}

/// Puts the HUD-less copy after every pass that leads into `EndMainPassPostProcessing` and before
/// it, so after all post-processing and before the UI passes, which follow it.
fn order_before_ui(mut graph: ResMut<RenderGraph>) {
    let Some(core3d) = graph.get_sub_graph_mut(Core3d) else {
        return;
    };
    let end = Node3d::EndMainPassPostProcessing.intern();
    let label = HudlessLabel.intern();
    let leading: Vec<_> = core3d
        .get_node_state(end)
        .map(|n| {
            n.edges
                .input_edges()
                .iter()
                // An edge's output node is the one it leaves.
                .map(|e| e.get_output_node())
                .filter(|&n| n != label)
                .collect()
        })
        .unwrap_or_default();
    // Already there when the render world starts again (a second window, a device recreated).
    for node in leading {
        let _ = core3d.try_add_node_edge(node, label);
    }
    let _ = core3d.try_add_node_edge(label, end);
}

/// This frame's want, held off for a few presents after the window changed.
fn follow(wanted: Res<Wanted>, windows: Res<ExtractedWindows>, mut hold: Local<u32>) {
    if windows
        .windows
        .values()
        .any(|w| w.size_changed || w.present_mode_changed)
    {
        *hold = HOLD_AFTER_WINDOW_CHANGE;
    } else {
        *hold = hold.saturating_sub(1);
    }
    CHOSEN.store(wanted.chosen, Ordering::Relaxed);
    WANT.store(wanted.now && *hold == 0, Ordering::Relaxed);
}

/// The HUD-less texture: one, back-buffer sized, and the ones it replaced, still alive a while.
#[derive(Resource, Default)]
struct HudlessTextures {
    current: Option<Hudless>,
    retired: Vec<(u32, Texture)>,
}

/// The HUD-less copy of a view: the texture, in the back buffer's format, the sRGB view the blit
/// draws through, the plain view Streamline is given, and the blit pipeline.
#[derive(Component, Clone)]
struct Hudless {
    texture: Texture,
    draw: TextureView,
    plain: TextureView,
    size: UVec2,
    pipeline: CachedRenderPipelineId,
}

fn prepare_hudless(
    mut commands: Commands,
    wanted: Res<Wanted>,
    device: Res<RenderDevice>,
    mut textures: ResMut<HudlessTextures>,
    pipeline_cache: Res<PipelineCache>,
    blit: Res<BlitPipeline>,
    mut pipelines: ResMut<SpecializedRenderPipelines<BlitPipeline>>,
    views: Query<(Entity, &ViewTarget, &ExtractedCamera), With<TemporalResolve>>,
) {
    textures.retired.retain_mut(|(frames, _)| {
        *frames = frames.saturating_sub(1);
        *frames > 0
    });
    if !wanted.chosen {
        return;
    }
    for (entity, target, camera) in &views {
        let Some(size) = camera.physical_target_size else {
            continue;
        };
        let draw_format = target.out_texture_view_format();
        let format = draw_format.remove_srgb_suffix();
        let fits = |h: &Hudless| h.size == size && h.texture.format() == format;
        if !textures.current.as_ref().is_some_and(fits) {
            let texture = device.create_texture(&TextureDescriptor {
                label: Some("redux_dlssg_hudless"),
                size: Extent3d {
                    width: size.x,
                    height: size.y,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[draw_format],
            });
            let draw = texture.create_view(&TextureViewDescriptor {
                format: Some(draw_format),
                ..default()
            });
            let plain = texture.create_view(&TextureViewDescriptor::default());
            let pipeline = pipelines.specialize(
                &pipeline_cache,
                &blit,
                BlitPipelineKey {
                    texture_format: draw_format,
                    blend_state: None,
                    samples: 1,
                },
            );
            let new = Hudless {
                texture,
                draw,
                plain,
                size,
                pipeline,
            };
            if let Some(old) = textures.current.replace(new) {
                textures.retired.push((RETIRE_FRAMES, old.texture));
            }
        }
        if let Some(hudless) = &textures.current {
            commands.entity(entity).insert(hudless.clone());
        }
    }
}

#[derive(Default)]
struct HudlessNode {
    bind_group: Mutex<Option<(TextureViewId, BindGroup)>>,
}

impl ViewNode for HudlessNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static ViewDepthTexture,
        &'static ViewPrepassTextures,
        &'static ExtractedView,
        &'static TemporalJitter,
        &'static TemporalResolve,
        &'static MotionRows,
        Option<&'static MainPassResolutionOverride>,
        &'static Hudless,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        context: &mut RenderContext<'w>,
        (target, view_depth, prepass, view, jitter, resolve, rows, resolution, hudless): QueryItem<
            'w,
            '_,
            Self::ViewQuery,
        >,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        if !world.resource::<Wanted>().now {
            return Ok(());
        }
        let Some(sl) = super::streamline() else {
            return Ok(());
        };
        let Some(frame) = world.get_resource::<SlFrame>().copied() else {
            return Ok(());
        };
        let Some(token) = frame.token() else {
            return Ok(());
        };
        if !motion_drawn(view_depth, prepass, world) {
            return Ok(());
        }
        let (Some(depth), Some(motion)) = (&prepass.depth, &prepass.motion_vectors) else {
            return Ok(());
        };
        let size = target.main_texture().size();
        let output = UVec2::new(size.width, size.height);
        if hudless.size != output
            || depth.texture.texture.size() != size
            || motion.texture.texture.size() != size
        {
            return Ok(());
        }
        let pipeline_cache = world.resource::<PipelineCache>();
        let Some(pipeline) = pipeline_cache.get_render_pipeline(hudless.pipeline) else {
            return Ok(());
        };
        let viewport = sl::ViewportHandle::new(VIEWPORT);
        if CONSTANTS.load(Ordering::Relaxed) != frame.index {
            let set = sl.set_constants(
                &constants(&view.world_from_view, jitter.offset, rows, resolve.reset),
                token,
                &viewport,
            );
            if !set.ok() {
                failed("slSetConstants", set);
                return Ok(());
            }
            constants_set(frame.index);
        }
        let source = target.main_texture_view();
        let mut cached = self.bind_group.lock().unwrap();
        let bind_group = match &mut *cached {
            Some((id, group)) if *id == source.id() => group.clone(),
            slot => {
                let group = world.resource::<BlitPipeline>().create_bind_group(
                    context.render_device(),
                    source,
                    pipeline_cache,
                );
                *slot = Some((source.id(), group.clone()));
                group
            }
        };
        let encoder = context.command_encoder();
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("redux_dlssg_hudless"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &hudless.draw,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Default::default()),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        // The layouts the tags give, kept to the present: nothing uses these again this frame.
        let transition = |texture, state| wgpu::TextureTransition {
            texture,
            selector: None,
            state,
        };
        encoder.transition_resources(
            std::iter::empty(),
            [
                transition(&*depth.texture.texture, wgpu::TextureUses::RESOURCE),
                transition(&motion.texture.texture, wgpu::TextureUses::RESOURCE),
                transition(&hudless.texture, wgpu::TextureUses::RESOURCE),
            ]
            .into_iter(),
        );
        let images = (|| {
            use wgpu::TextureUses as U;
            Some([
                image(
                    &depth.texture.texture,
                    &depth.texture.default_view,
                    U::RESOURCE,
                )?,
                image(
                    &motion.texture.texture,
                    &motion.texture.default_view,
                    U::RESOURCE,
                )?,
                image(&hudless.texture, &hudless.plain, U::RESOURCE)?,
            ])
        })();
        let Some(images) = images else {
            failed("tagging", sl::SlResult(25));
            return Ok(());
        };
        let mut resources = images.map(sl::Resource::vulkan);
        let mut depth_aspect = sl::SubresourceRange::aspect(vk::ImageAspectFlags::DEPTH.as_raw());
        resources[0].base.next = &mut depth_aspect.base;
        let drawn = resolution.map_or(output, |r| r.0);
        let rect = sl::Extent {
            top: 0,
            left: 0,
            width: drawn.x,
            height: drawn.y,
        };
        let [d, m, h] = resources.each_mut();
        let lifecycle = sl::LIFECYCLE_VALID_UNTIL_PRESENT;
        let mut tags = [
            sl::ResourceTag::new(d, sl::BUFFER_DEPTH, lifecycle),
            sl::ResourceTag::new(m, sl::BUFFER_MOTION_VECTORS, lifecycle),
            sl::ResourceTag::new(h, sl::BUFFER_HUDLESS_COLOR, lifecycle),
            // The whole back buffer, by its rectangle alone: untagged, DLSS-G warns and assumes it.
            sl::ResourceTag::new(std::ptr::null_mut(), sl::BUFFER_BACKBUFFER, lifecycle),
        ];
        tags[0].extent = rect;
        tags[1].extent = rect;
        tags[3].extent = sl::Extent {
            top: 0,
            left: 0,
            width: output.x,
            height: output.y,
        };
        // SAFETY: the images live past the present (Bevy's texture cache and `HudlessTextures`
        // keep them), are in the layouts tagged from the transitions above to the present, and
        // their content is written by work submitted before it.
        let set = unsafe { sl.set_tags_for_frame(token, &viewport, &tags) };
        if set.ok() {
            TAGGED.store(frame.index, Ordering::Relaxed);
        } else {
            failed("slSetTagForFrame", set);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_print_the_probe_words() {
        let line = stats().to_string();
        assert!(line.starts_with("fg_on=false fg_status=ok fg_presents="));
        assert!(line.contains("fg_errors=0") && line.contains("fg_given_up=false"));
    }
}
