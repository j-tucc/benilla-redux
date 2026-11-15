//! Temporal anti-aliasing for Enhanced (LOOK-15): the world camera drops MSAA and takes Bevy's
//! `TemporalAntiAliasing`, fed with motion vectors this module reprojects from depth.
//!
//! Bevy's TAA wants a depth and a motion-vector prepass. Benilla's world is not drawn in one: the
//! retained static pass (WMOs, doodads, trees) is its own node, terrain places its vertices
//! camera-relative where Bevy's stock prepass shader would not (a prepass depth a hair off the main
//! pass's punches holes through `GreaterEqual`), and the model lane's depth stage writes no motion.
//! So the camera's prepass phases are dropped before anything is queued in them, and this module
//! supplies the view's prepass textures itself: after the main pass, a copy of the finished depth
//! and a full-screen pass that turns each pixel's depth into its motion since the last frame.
//!
//! - Every surface that writes depth (terrain, models, buildings, water, the WDL hills) and the
//!   sky (at infinity) moves exactly with the camera.
//! - Content that moves on its own (units, swaying foliage, doodad animation) is given the
//!   camera's motion; the resolve's colour clamp keeps it from ghosting, at some softness while it
//!   moves. Per-object motion needs the depth stages to write it: see the architecture file.
//!
//! The choice in Graphics names TAA, DLSS or MetalFX. DLSS draws as DLSS Super Resolution
//! (`super::dlss`) at the chosen [`DlssQuality`] when the build has the `dlss` feature and
//! Streamline came up at boot: the camera keeps the jitter, the mip bias and this module's motion
//! pass, and the DLSS node resolves in place of Bevy's TAA. An upscaling mode draws the main pass
//! into a smaller rectangle of the full-size targets (`MainPassResolutionOverride`); the motion
//! pass works in that rectangle, and the prepass textures stay full size with DLSS told the extent.
//! Otherwise DLSS, and MetalFX (not in Bevy), draw as TAA.
//! Classic, and any choice of MSAA, leaves the camera exactly as the engine spawned it.
use super::{DlssQuality, Enhanced};
#[cfg(test)]
use crate::prefs::AA_TAA;
use crate::prefs::{AA_DLSS, AA_METALFX, AA_MSAA};
use benilla_world::view::{MsaaSetting, WorldCamera};
use bevy::camera::{CameraMainTextureUsages, MainPassResolutionOverride};
use bevy::{
    anti_alias::taa::{TemporalAntiAliasPlugin, TemporalAntiAliasing},
    core_pipeline::{
        core_3d::{
            extract_camera_prepass_phase,
            graph::{Core3d, Node3d},
            prepare_prepass_textures, CORE_3D_DEPTH_FORMAT,
        },
        prepass::{
            AlphaMask3dPrepass, DepthPrepass, MotionVectorPrepass, Opaque3dPrepass,
            ViewPrepassTextures, MOTION_VECTOR_PREPASS_FORMAT,
        },
        FullscreenShader,
    },
    ecs::query::QueryItem,
    math::{DMat3, DMat4, DVec3},
    prelude::*,
    render::{
        camera::{ExtractedCamera, MipBias, TemporalJitter},
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_graph::{
            NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
        },
        render_phase::ViewBinnedRenderPhases,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue},
        texture::{ColorAttachment, TextureCache},
        view::{
            Msaa, RetainedViewEntity, ViewDepthTexture, ViewUniform, ViewUniformOffset,
            ViewUniforms,
        },
        Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
    },
};

/// A camera step longer than this in one frame is a cut (a teleport, a load): the history is
/// dropped rather than reprojected across it. Deviation: Bevy leaves cuts to the program; 10 yd
/// is three frames of the fastest mount at 30 fps.
const CUT_STEP: f32 = 10.0;
/// A camera turn wider than this in one frame (radians) is a cut too.
const CUT_TURN: f32 = 1.0;

/// How the world is anti-aliased this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// The engine's MSAA (LOOK-7): the camera as spawned.
    Msaa,
    /// Bevy's TAA, with what was chosen if it was an upscaler this build or machine lacks.
    Taa { asked: u8 },
    /// DLSS Super Resolution through Streamline, at native resolution (DLAA) or upscaling.
    Dlss { quality: DlssQuality },
}

fn mode(aa: u8, quality: DlssQuality, dlss_ready: bool) -> Mode {
    match aa {
        AA_MSAA => Mode::Msaa,
        AA_DLSS if dlss_ready => Mode::Dlss { quality },
        asked => Mode::Taa { asked },
    }
}

/// Whether DLSS Super Resolution can run: Streamline created the device and DLSS has not failed.
#[cfg(feature = "dlss")]
fn dlss_ready() -> bool {
    super::dlss::sr_ready()
}

#[cfg(not(feature = "dlss"))]
fn dlss_ready() -> bool {
    false
}

/// A camera this module put under a temporal resolve: Bevy's TAA (with `TemporalAntiAliasing`)
/// or, with `dlss`, the DLSS node. Both keep the jitter, the mip bias and the motion pass, which
/// `#[require]` brings and the switch back to MSAA takes away.
#[derive(Component, Clone, Copy, Default, PartialEq, Eq, ExtractComponent)]
#[require(TemporalJitter, MipBias, DepthPrepass, MotionVectorPrepass)]
pub(crate) struct TemporalResolve {
    /// The DLSS node resolves this camera, not Bevy's TAA.
    pub(crate) dlss: bool,
    /// DLSS's mode, while `dlss`.
    pub(crate) quality: DlssQuality,
    /// This frame does not follow the last (a cut or a switch): DLSS drops its history.
    pub(crate) reset: bool,
}

/// The last frame's camera, in the main world.
#[derive(Component, Clone, Copy)]
struct LastView {
    rotation: Quat,
    position: Vec3,
    clip_from_view: Mat4,
}

/// The motion pass's rows: this frame's unjittered `clip_from_view`, then the last frame's
/// unjittered clip from this frame's view space, column by column.
#[derive(Component, Clone, Copy, PartialEq, ExtractComponent)]
pub(crate) struct MotionRows([[f32; 4]; 8]);

#[cfg(feature = "dlss")]
impl MotionRows {
    /// This frame's unjittered `clip_from_view`, and the last frame's clip from this frame's view.
    pub(crate) fn matrices(&self) -> (Mat4, Mat4) {
        let now: [[f32; 4]; 4] = self.0[..4].try_into().unwrap();
        let then: [[f32; 4]; 4] = self.0[4..].try_into().unwrap();
        (
            Mat4::from_cols_array_2d(&now),
            Mat4::from_cols_array_2d(&then),
        )
    }

    #[cfg(test)]
    pub(crate) fn from_matrices(now: Mat4, then: Mat4) -> Self {
        let mut rows = [[0.0; 4]; 8];
        rows[..4].copy_from_slice(&now.to_cols_array_2d());
        rows[4..].copy_from_slice(&then.to_cols_array_2d());
        MotionRows(rows)
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
pub(crate) struct MotionLabel;

pub(super) struct TaaPlugin;

impl Plugin for TaaPlugin {
    fn build(&self, app: &mut App) {
        // The engine's boot disables Bevy's anti-aliasing plugins; the TAA node is added here,
        // where only a camera this module puts under TAA runs it.
        if !app.is_plugin_added::<TemporalAntiAliasPlugin>() {
            app.add_plugins(TemporalAntiAliasPlugin);
        }
        app.add_plugins(ExtractComponentPlugin::<MotionRows>::default())
            .add_plugins(ExtractComponentPlugin::<TemporalResolve>::default())
            .add_systems(Last, (follow_choice, write_rows).chain());
        if !app.is_plugin_added::<AssetPlugin>() {
            return;
        }
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("motion.wgsl"),
                "redux_gfx/motion.wgsl",
            ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(MotionShader(shader))
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(
                ExtractSchedule,
                drop_prepass_phases.after(extract_camera_prepass_phase),
            )
            .add_systems(
                Render,
                (
                    provide_prepass_textures.after(prepare_prepass_textures),
                    prepare_uniforms,
                )
                    .in_set(RenderSystems::PrepareResources),
            )
            .add_render_graph_node::<ViewNodeRunner<MotionNode>>(Core3d, MotionLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    Node3d::EndMainPass,
                    MotionLabel,
                    Node3d::StartMainPassPostProcessing,
                ),
            )
            // Bloom, the grade and the FFX combine run at output resolution on the resolved image
            // (LOOK-12); the bloom leads the other two.
            .add_render_graph_edges(Core3d, (Node3d::Taa, super::bloom::BloomLabel));
    }
}

/// Puts the world camera under TAA, DLSS or back to its MSAA as the choice changes. The temporal
/// resolve's required components (the jitter, the mip bias, the prepass markers) come and go with
/// it, so Classic's camera is the one the engine spawned.
fn follow_choice(
    mut commands: Commands,
    enhanced: Res<Enhanced>,
    quality: Res<DlssQuality>,
    msaa: Res<MsaaSetting>,
    mut cameras: Query<FollowCamera, With<WorldCamera>>,
    mut shown: Local<Option<Mode>>,
) {
    let want = mode(enhanced.aa, *quality, dlss_ready());
    if *shown != Some(want) {
        match want {
            Mode::Msaa => info!("Anti-aliasing: MSAA {}x", msaa.samples),
            Mode::Dlss { quality } => {
                info!("Anti-aliasing: DLSS Super Resolution ({})", quality.word())
            }
            Mode::Taa { asked: AA_DLSS } => {
                info!("Anti-aliasing: TAA (DLSS is not available in this build or on this GPU)")
            }
            Mode::Taa { asked: AA_METALFX } => {
                info!("Anti-aliasing: TAA (MetalFX is not in this build)")
            }
            Mode::Taa { .. } => info!("Anti-aliasing: TAA"),
        }
        *shown = Some(want);
    }
    for (entity, mut samples, mut camera3d, mut main_usages, resolve) in &mut cameras {
        let storage = TextureUsages::STORAGE_BINDING;
        let quality = match want {
            Mode::Dlss { quality } => quality,
            _ => DlssQuality::default(),
        };
        match (want, resolve.map(|r| (r.dlss, r.quality))) {
            // Between DLSS modes: the render size changes and the history is dropped.
            (Mode::Dlss { quality }, Some((true, was))) if was != quality => {
                commands
                    .entity(entity)
                    .remove::<LastView>()
                    .insert(TemporalResolve {
                        dlss: true,
                        quality,
                        reset: true,
                    });
            }
            (Mode::Taa { .. }, None | Some((true, _)))
            | (Mode::Dlss { .. }, None | Some((false, _))) => {
                let dlss = matches!(want, Mode::Dlss { .. });
                *samples = Msaa::Off;
                // The motion pass reads the depth and copies it into the prepass depth.
                let usage = (TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC).bits();
                if camera3d.depth_texture_usages.0 & usage != usage {
                    camera3d.depth_texture_usages.0 |= usage;
                }
                // DLSS writes its output straight into the next main texture.
                if main_usages.0.contains(storage) != dlss {
                    main_usages.0.set(storage, dlss);
                }
                let mut camera = commands.entity(entity);
                // No last view: the first frame under the new resolve starts its history afresh.
                camera.remove::<LastView>().insert(TemporalResolve {
                    dlss,
                    quality,
                    reset: true,
                });
                if dlss {
                    camera.remove::<TemporalAntiAliasing>();
                } else {
                    camera.insert(TemporalAntiAliasing::default());
                }
            }
            (Mode::Msaa, Some(_)) => {
                *samples = msaa.level();
                if main_usages.0.contains(storage) {
                    main_usages.0.remove(storage);
                }
                commands.entity(entity).remove::<(
                    TemporalResolve,
                    TemporalAntiAliasing,
                    TemporalJitter,
                    MipBias,
                    DepthPrepass,
                    MotionVectorPrepass,
                    LastView,
                    MotionRows,
                )>();
            }
            _ => {}
        }
    }
}

/// The world camera as `follow_choice` switches it.
type FollowCamera<'a> = (
    Entity,
    &'a mut Msaa,
    &'a mut Camera3d,
    &'a mut CameraMainTextureUsages,
    Option<&'a TemporalResolve>,
);

/// The motion rows for a camera at `rotation`/`position` with `clip_from_view`, from the last
/// frame's. The step between the two positions is one f32 subtraction of nearby values, which is
/// exact, so it is the step the renderer drew with; everything else is camera-relative, in f64.
fn motion_rows(
    rotation: Quat,
    position: Vec3,
    clip_from_view: Mat4,
    last: &LastView,
) -> MotionRows {
    let now = DMat3::from_quat(rotation.as_dquat());
    let then = DMat3::from_quat(last.rotation.as_dquat());
    let step = DVec3::from(position - last.position);
    // This frame's view space to the last frame's: rotate to world, add the step, rotate back.
    let back = then.transpose();
    let r = back * now;
    let t = back * step;
    let view_to_last_view = DMat4::from_cols(
        r.x_axis.extend(0.0),
        r.y_axis.extend(0.0),
        r.z_axis.extend(0.0),
        t.extend(1.0),
    );
    let then_clip = (last.clip_from_view.as_dmat4() * view_to_last_view).as_mat4();
    let mut rows = [[0.0; 4]; 8];
    rows[..4].copy_from_slice(&clip_from_view.to_cols_array_2d());
    rows[4..].copy_from_slice(&then_clip.to_cols_array_2d());
    MotionRows(rows)
}

/// A temporal camera as `write_rows` reads it.
type RowCamera<'a> = (
    Entity,
    &'a Camera,
    &'a GlobalTransform,
    &'a mut TemporalResolve,
    Option<&'a mut TemporalAntiAliasing>,
    Option<&'a mut LastView>,
    Option<&'a mut MotionRows>,
);

/// A camera this module put under a temporal resolve, once it has its motion rows.
type UnderTaa = (With<TemporalResolve>, With<MotionRows>);

/// Each temporal camera's motion rows, from its final transform this frame and the last; a cut
/// drops the history (Bevy's TAA clears its flag itself; DLSS's is set per frame). Written in place
/// after the first frame.
fn write_rows(mut commands: Commands, mut cameras: Query<RowCamera, With<WorldCamera>>) {
    for (entity, camera, transform, mut resolve, mut taa, mut last_view, old) in &mut cameras {
        let (_, rotation, position) = transform.to_scale_rotation_translation();
        let here = LastView {
            rotation,
            position,
            clip_from_view: camera.clip_from_view(),
        };
        let follows = last_view.as_deref().filter(|l| {
            (position - l.position).length() <= CUT_STEP
                && rotation.angle_between(l.rotation) <= CUT_TURN
        });
        let last = follows.copied().unwrap_or(here);
        let cut = follows.is_none();
        if let Some(taa) = taa.as_deref_mut().filter(|t| cut && !t.reset) {
            taa.reset = true;
        }
        if resolve.reset != cut {
            resolve.reset = cut;
        }
        let rows = motion_rows(rotation, position, here.clip_from_view, &last);
        match (last_view.as_deref_mut(), old) {
            (Some(l), Some(mut old)) => {
                *l = here;
                if *old != rows {
                    *old = rows;
                }
            }
            _ => {
                commands.entity(entity).insert((here, rows));
            }
        }
    }
}

/// Removes the TAA camera's prepass phases right after Bevy makes them, so no material is
/// specialized, queued or drawn into a prepass of the world view; `provide_prepass_textures`
/// stands in for what the prepass would have written.
fn drop_prepass_phases(
    cameras: Extract<Query<(Entity, &Camera), UnderTaa>>,
    mut opaque: ResMut<ViewBinnedRenderPhases<Opaque3dPrepass>>,
    mut mask: ResMut<ViewBinnedRenderPhases<AlphaMask3dPrepass>>,
) {
    for (entity, camera) in &cameras {
        if camera.is_active {
            let view = RetainedViewEntity::new(entity.into(), None, 0);
            opaque.remove(&view);
            mask.remove(&view);
        }
    }
}

/// The TAA view's prepass textures, which Bevy removes for a view without prepass phases: a depth
/// the motion pass copies the finished depth into, and the motion vectors it draws.
fn provide_prepass_textures(
    mut commands: Commands,
    mut cache: ResMut<TextureCache>,
    device: Res<RenderDevice>,
    views: Query<(Entity, &ExtractedCamera), UnderTaa>,
) {
    for (entity, camera) in &views {
        let Some(size) = camera.physical_target_size else {
            continue;
        };
        let size = Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        };
        let mut texture = |label, format, usage| {
            cache.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                },
            )
        };
        let depth = texture(
            "redux_taa_depth",
            CORE_3D_DEPTH_FORMAT,
            TextureUsages::COPY_DST | TextureUsages::TEXTURE_BINDING,
        );
        let motion = texture(
            "redux_taa_motion",
            MOTION_VECTOR_PREPASS_FORMAT,
            TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        );
        commands.entity(entity).insert(ViewPrepassTextures {
            depth: Some(ColorAttachment::new(depth, None, None, None)),
            normal: None,
            motion_vectors: Some(ColorAttachment::new(motion, None, None, None)),
            deferred: None,
            deferred_lighting_pass_id: None,
            size,
        });
    }
}

#[derive(Resource)]
struct MotionShader(Handle<Shader>);

#[derive(Resource)]
struct MotionPipeline {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedRenderPipelineId,
}

fn init_pipeline(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    shader: Res<MotionShader>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "redux_taa_motion_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_depth_2d(),
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer_sized(false, None),
            ),
        ),
    );
    let pipeline = super::post::pipeline(
        &cache,
        &fullscreen,
        "redux_taa_motion",
        &layout,
        &shader.0,
        "fragment",
        MOTION_VECTOR_PREPASS_FORMAT,
    );
    commands.insert_resource(MotionPipeline { layout, pipeline });
}

fn prepare_uniforms(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    views: Query<(
        Entity,
        &MotionRows,
        Option<&super::post::ViewUniform<MotionRows>>,
    )>,
) {
    for (entity, rows, buffer) in &views {
        if let Some(new) =
            super::post::write_uniform(&device, &queue, buffer, "redux_taa_motion", &rows.0)
        {
            commands.entity(entity).insert(new);
        }
    }
}

/// Whether the motion pass draws the view's prepass depth and motion this frame. The usage change
/// reaches the depth texture a frame after a switch, and the view's MSAA a frame after that; the
/// pipeline compiles in the first frames. The DLSS node reads the prepass textures only then.
pub(crate) fn motion_drawn(
    depth: &ViewDepthTexture,
    prepass: &ViewPrepassTextures,
    world: &World,
) -> bool {
    let usage = depth.texture.usage();
    let (Some(prepass_depth), Some(_)) = (&prepass.depth, &prepass.motion_vectors) else {
        return false;
    };
    usage.contains(TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC)
        && depth.texture.sample_count() == 1
        && prepass_depth.texture.texture.size() == depth.texture.size()
        && world
            .resource::<PipelineCache>()
            .get_render_pipeline(world.resource::<MotionPipeline>().pipeline)
            .is_some()
        && world
            .resource::<ViewUniforms>()
            .uniforms
            .binding()
            .is_some()
}

#[derive(Default)]
struct MotionNode;

impl ViewNode for MotionNode {
    type ViewQuery = (
        &'static ViewDepthTexture,
        &'static ViewPrepassTextures,
        &'static ViewUniformOffset,
        &'static super::post::ViewUniform<MotionRows>,
        Option<&'static MainPassResolutionOverride>,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        context: &mut RenderContext<'w>,
        (depth, prepass, view_offset, uniform, resolution): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        if !motion_drawn(depth, prepass, world) {
            return Ok(());
        }
        let (Some(prepass_depth), Some(motion)) = (&prepass.depth, &prepass.motion_vectors) else {
            return Ok(());
        };
        let p = world.resource::<MotionPipeline>();
        let cache = world.resource::<PipelineCache>();
        let Some(pipeline) = cache.get_render_pipeline(p.pipeline) else {
            return Ok(());
        };
        let Some(view) = world.resource::<ViewUniforms>().uniforms.binding() else {
            return Ok(());
        };
        let bind = context.render_device().create_bind_group(
            "redux_taa_motion",
            &cache.get_bind_group_layout(&p.layout),
            &BindGroupEntries::sequential((depth.view(), view, uniform.0.as_entire_binding())),
        );
        // Under an upscaler only the main pass's rectangle (at the origin: the world camera has
        // no viewport) holds this frame; DLSS reads that extent of both textures. A depth copy
        // must be whole (wgpu), so the depth is copied whole and the motion drawn in the rectangle.
        let rect = super::post::main_pass_size(depth.texture.size(), resolution);
        let encoder = context.command_encoder();
        encoder.copy_texture_to_texture(
            depth.texture.as_image_copy(),
            prepass_depth.texture.texture.as_image_copy(),
            depth.texture.size(),
        );
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("redux_taa_motion"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &motion.texture.default_view,
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
        if resolution.is_some() {
            pass.set_viewport(0.0, 0.0, rect.width as f32, rect.height as f32, 0.0, 1.0);
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[view_offset.offset]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows' reprojection of a view-space point `p` (w = 1, or 0 for a direction), as the
    /// shader does it: uv now minus uv then.
    fn motion_of(rows: &MotionRows, p: Vec4) -> Vec2 {
        let now = Mat4::from_cols_array_2d(&rows.0[..4].try_into().unwrap()) * p;
        let then = Mat4::from_cols_array_2d(&rows.0[4..].try_into().unwrap()) * p;
        (now.truncate().truncate() / now.w - then.truncate().truncate() / then.w)
            * Vec2::new(0.5, -0.5)
    }

    fn projection() -> Mat4 {
        Mat4::perspective_infinite_reverse_rh(1.0, 16.0 / 9.0, 0.1)
    }

    /// At Northshire (x ≈ −8900), where an f32 steps by about a millimetre.
    const NORTHSHIRE: Vec3 = Vec3::new(-8900.0, 80.0, 130.0);

    #[test]
    fn a_still_camera_gives_no_motion() {
        let last = LastView {
            rotation: Quat::from_rotation_y(0.3),
            position: NORTHSHIRE,
            clip_from_view: projection(),
        };
        let rows = motion_rows(last.rotation, last.position, last.clip_from_view, &last);
        for p in [
            Vec4::new(0.5, -0.2, -7.0, 1.0),
            Vec4::new(0.1, 0.1, -1.0, 0.0),
        ] {
            assert!(motion_of(&rows, p).length() < 1e-6);
        }
    }

    #[test]
    fn a_sideways_step_moves_near_points_and_leaves_the_sky() {
        let rotation = Quat::from_rotation_y(0.3);
        let last = LastView {
            rotation,
            position: NORTHSHIRE,
            clip_from_view: projection(),
        };
        // One frame's step of 0.1 yd to the camera's right, at Northshire's coordinates.
        let position = NORTHSHIRE + rotation * Vec3::new(0.1, 0.0, 0.0);
        let rows = motion_rows(rotation, position, projection(), &last);
        // A point 5 yd ahead: it was about 0.1 yd further right a frame ago, so it moved left.
        // The expected motion takes the step the f32 positions actually hold (a millimetre off
        // 0.1 out here), which is the step the renderer drew.
        let near = motion_of(&rows, Vec4::new(0.0, 0.0, -5.0, 1.0));
        let then = Vec3::new(0.0, 0.0, -5.0) + rotation.inverse() * (position - NORTHSHIRE);
        let expected = 0.5 * projection().x_axis.x * then.x / -then.z;
        assert!(expected > 0.0019);
        assert!(
            (near.x + expected).abs() < 1e-4 * expected,
            "{near} vs {expected}"
        );
        assert!(near.y.abs() < 1e-6);
        // The sky at infinity does not move under a step.
        assert!(motion_of(&rows, Vec4::new(0.0, 0.0, -1.0, 0.0)).length() < 1e-6);
    }

    #[test]
    fn a_turn_moves_the_sky() {
        let last = LastView {
            rotation: Quat::IDENTITY,
            position: NORTHSHIRE,
            clip_from_view: projection(),
        };
        // Turning left by 0.01 rad: what is ahead now was a little to the left, so it moved right.
        let rows = motion_rows(Quat::from_rotation_y(0.01), NORTHSHIRE, projection(), &last);
        let sky = motion_of(&rows, Vec4::new(0.0, 0.0, -1.0, 0.0));
        let expected = 0.5 * 0.01_f32.tan() * projection().x_axis.x;
        assert!((sky.x - expected).abs() < 1e-3 * expected, "{sky}");
    }

    #[test]
    fn msaa_is_the_classic_choice_dlss_needs_streamline_and_the_rest_are_taa() {
        let q = DlssQuality::Performance;
        assert_eq!(mode(AA_MSAA, q, true), Mode::Msaa);
        assert_eq!(mode(AA_DLSS, q, false), Mode::Taa { asked: AA_DLSS });
        assert_eq!(mode(AA_DLSS, q, true), Mode::Dlss { quality: q });
        assert_eq!(mode(AA_METALFX, q, true), Mode::Taa { asked: AA_METALFX });
        assert_eq!(mode(AA_TAA, q, true), Mode::Taa { asked: AA_TAA });
    }

    #[test]
    fn the_motion_shader_validates() {
        // `View` comes from Bevy's import; validate the body against a stand-in.
        let src = include_str!("motion.wgsl").replace(
            "#import bevy_render::view::View",
            "struct View { view_from_clip: mat4x4<f32>, main_pass_viewport: vec4<f32> }",
        );
        super::super::post::validate_wgsl(&src);
    }
}
