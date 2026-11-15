//! Ambient occlusion (LOOK-14): depth-only screen-space occlusion of the opaque scene, darkening
//! creases, contacts and the feet of walls and trunks. Three full-screen stages right after the
//! opaque pass ([`ssao.wgsl`]): occlusion at half resolution from the depth alone, a depth-aware
//! blur that also finds foliage (cutout leaves' depth spikes, where it keeps less occlusion), and a
//! joint-bilateral upsample multiplied into the main attachment, so the water, the transparencies
//! and every later pass draw over the darkened scene. Under MSAA it reads sample 0 of the depth
//! and multiplies into every sample, so the resolve stays smooth. The opaque scene is fogged in
//! its own shaders before this, so the multiply reads the fog rows of the engine's shared light
//! buffer and darkens only the share of each pixel the fog leaves.
//!
//! Resolution: everything runs at render resolution. The occlusion and blur cover half the main
//! pass's rectangle, which `MainPassResolutionOverride` shrinks under an upscaler, and place each
//! pixel through it (`main_pass_viewport`); the multiply draws into the same rectangle of the main
//! attachment.
//!
//! Ported from the everwood graphics fork of Benilla (MIT OR Apache-2.0), revision `cdc7130b`,
//! `benilla-app/src/ssao.rs`: its High tier's radius, strength, samples, bias, gain, distance fade
//! and bright-pixel protection, and its stage layout.
use super::Enhanced;
use benilla_world::{
    lighting::{SharedLightBuffer, GFX_DATA_OFFSET},
    view::WorldCamera,
};
use bevy::{
    camera::{MainPassResolutionOverride, Viewport},
    core_pipeline::{core_3d::graph::Core3d, core_3d::graph::Node3d, FullscreenShader},
    ecs::query::QueryItem,
    platform::collections::HashMap,
    prelude::*,
    render::{
        camera::ExtractedCamera,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_graph::{
            NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
        },
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice},
        texture::{CachedTexture, TextureCache},
        view::{ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
        Render, RenderApp, RenderStartup, RenderSystems,
    },
};

/// Radius (yd), darkening strength, samples, cosine bias. Source: everwood's High tier.
const PARAMS: [f32; 4] = [1.5, 0.7, 12.0, 0.05];
/// Distance fade (yd) and the bright-pixel protection ramp. Source: everwood's High tier.
const FADE: [f32; 4] = [60.0, 120.0, 0.7, 0.95];
/// The occlusion gain. Source: everwood's default `GAIN`.
const GAIN: f32 = 6.0;
/// The half-resolution targets.
const AO_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// A world view that takes occlusion.
#[derive(Component, Clone, Copy, PartialEq, ExtractComponent)]
struct AoView;

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct AoLabel;

pub(super) struct SsaoPlugin;

impl Plugin for SsaoPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<AoView>::default())
            .add_systems(Last, update_views);
        if !app.is_plugin_added::<AssetPlugin>() {
            return;
        }
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("ssao.wgsl"),
                "redux_gfx/ssao.wgsl",
            ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(AoShader(shader))
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(
                Render,
                prepare_resources.in_set(RenderSystems::PrepareResources),
            )
            .add_render_graph_node::<ViewNodeRunner<AoNode>>(Core3d, AoLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    Node3d::MainOpaquePass,
                    AoLabel,
                    Node3d::MainTransmissivePass,
                ),
            );
    }
}

fn update_views(
    mut commands: Commands,
    enhanced: Res<Enhanced>,
    mut cameras: Query<(Entity, &Camera, &mut Camera3d, Option<&AoView>), With<WorldCamera>>,
) {
    for (entity, camera, mut camera3d, old) in &mut cameras {
        if !(enhanced.ssao && camera.is_active) {
            if old.is_some() {
                commands.entity(entity).remove::<AoView>();
            }
            continue;
        }
        let binding = TextureUsages::TEXTURE_BINDING.bits();
        if camera3d.depth_texture_usages.0 & binding == 0 {
            camera3d.depth_texture_usages.0 |= binding;
        }
        if old.is_none() {
            commands.entity(entity).insert(AoView);
        }
    }
}

#[derive(Resource)]
struct AoShader(Handle<Shader>);

#[derive(Resource)]
struct AoPipelines {
    /// By `samples > 1`.
    occlude_layouts: [BindGroupLayoutDescriptor; 2],
    blur_layout: BindGroupLayoutDescriptor,
    apply_layouts: [BindGroupLayoutDescriptor; 2],
    occlude: [CachedRenderPipelineId; 2],
    blur: CachedRenderPipelineId,
    /// The multiply, by the main pass's sample count, queued when a view first needs it.
    apply: HashMap<(u32, TextureFormat), CachedRenderPipelineId>,
    shader: Handle<Shader>,
    fullscreen: FullscreenShader,
    /// The occlusion parameters, the same for every view.
    params: Buffer,
}

fn layout(label: &'static str, ms: bool, apply: bool) -> BindGroupLayoutDescriptor {
    let mut entries = BindGroupLayoutEntries::with_indices(
        ShaderStages::FRAGMENT,
        (
            (
                0,
                if ms {
                    texture_depth_2d_multisampled()
                } else {
                    texture_depth_2d()
                },
            ),
            (
                1,
                texture_2d(TextureSampleType::Float { filterable: false }),
            ),
            (2, uniform_buffer::<ViewUniform>(true)),
            (3, uniform_buffer_sized(false, None)),
        ),
    )
    .to_vec();
    if apply {
        // The shared light buffer, read for its fog rows.
        entries.push(storage_buffer_read_only_sized(false, None).build(4, ShaderStages::FRAGMENT));
    }
    BindGroupLayoutDescriptor::new(label, &entries)
}

fn descriptor(
    p: (&Handle<Shader>, &FullscreenShader),
    layout: &BindGroupLayoutDescriptor,
    stage: &'static str,
    entry: &'static str,
    multisampled_depth: bool,
    target: (TextureFormat, u32, Option<BlendState>),
) -> RenderPipelineDescriptor {
    // `GFX_BASE`: the shared light buffer's row where `GfxData` starts.
    let mut defs: Vec<bevy::shader::ShaderDefVal> = vec![
        stage.into(),
        bevy::shader::ShaderDefVal::UInt("GFX_BASE".into(), (GFX_DATA_OFFSET / 16) as u32),
    ];
    if multisampled_depth {
        defs.push("MULTISAMPLED".into());
    }
    RenderPipelineDescriptor {
        label: Some(entry.into()),
        layout: vec![layout.clone()],
        vertex: p.1.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: p.0.clone(),
            shader_defs: defs,
            entry_point: Some(entry.into()),
            targets: vec![Some(ColorTargetState {
                format: target.0,
                blend: target.2,
                write_mask: ColorWrites::ALL,
            })],
        }),
        multisample: MultisampleState {
            count: target.1,
            ..default()
        },
        ..default()
    }
}

fn init_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    shader: Res<AoShader>,
    fullscreen: Res<FullscreenShader>,
) {
    let occlude_layouts = [
        layout("redux_ao_occlude", false, false),
        layout("redux_ao_occlude", true, false),
    ];
    let apply_layouts = [
        layout("redux_ao_apply", false, true),
        layout("redux_ao_apply", true, true),
    ];
    let blur_layout = BindGroupLayoutDescriptor::new(
        "redux_ao_blur",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (
                    0,
                    texture_2d(TextureSampleType::Float { filterable: false }),
                ),
                (3, uniform_buffer_sized(false, None)),
            ),
        ),
    );
    let p = (&shader.0, &*fullscreen);
    let occlude = [false, true].map(|ms| {
        cache.queue_render_pipeline(descriptor(
            p,
            &occlude_layouts[ms as usize],
            "AO_OCCLUDE",
            "occlude",
            ms,
            (AO_FORMAT, 1, None),
        ))
    });
    let blur = cache.queue_render_pipeline(descriptor(
        p,
        &blur_layout,
        "AO_BLUR",
        "blur",
        false,
        (AO_FORMAT, 1, None),
    ));
    let rows = [PARAMS, FADE, [0.0, GAIN, 0.0, 0.0]];
    let params = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("redux_ao"),
        contents: &super::post::bytes(&rows),
        usage: BufferUsages::UNIFORM,
    });
    commands.insert_resource(AoPipelines {
        occlude_layouts,
        blur_layout,
        apply_layouts,
        occlude,
        blur,
        apply: HashMap::default(),
        shader: shader.0.clone(),
        fullscreen: fullscreen.clone(),
        params,
    });
}

/// A view's half-resolution targets and the multiply it draws with.
#[derive(Component)]
struct AoTextures {
    raw: CachedTexture,
    blurred: CachedTexture,
    apply: CachedRenderPipelineId,
}

/// The multiply: the scene times the blend source, alpha kept.
const MULTIPLY: BlendState = BlendState {
    color: BlendComponent {
        src_factor: BlendFactor::Dst,
        dst_factor: BlendFactor::Zero,
        operation: BlendOperation::Add,
    },
    alpha: BlendComponent {
        src_factor: BlendFactor::Zero,
        dst_factor: BlendFactor::One,
        operation: BlendOperation::Add,
    },
};

fn prepare_resources(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<AoPipelines>,
    mut textures: ResMut<TextureCache>,
    views: Query<
        (
            Entity,
            &ViewTarget,
            &ViewDepthTexture,
            Option<&MainPassResolutionOverride>,
        ),
        With<AoView>,
    >,
) {
    for (entity, target, depth, resolution) in &views {
        // The apply pass draws into the main attachment, so its pipeline takes that attachment's
        // samples; the node skips a frame whose depth does not match them yet.
        let samples = attachment_samples(target);
        let format = target.main_texture_format();
        let p = &mut *pipelines;
        let apply = *p.apply.entry((samples, format)).or_insert_with(|| {
            cache.queue_render_pipeline(descriptor(
                (&p.shader, &p.fullscreen),
                &p.apply_layouts[(samples > 1) as usize],
                "AO_APPLY",
                "apply",
                samples > 1,
                (format, samples, Some(MULTIPLY)),
            ))
        });
        // The main pass's rectangle: smaller than the depth target under an upscaler.
        let full = super::post::main_pass_size(depth.texture.size(), resolution);
        let size = Extent3d {
            width: full.width.div_ceil(2).max(1),
            height: full.height.div_ceil(2).max(1),
            depth_or_array_layers: 1,
        };
        let mut half = |label: &'static str| {
            textures.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: AO_FORMAT,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let raw = half("redux_ao_raw");
        let blurred = half("redux_ao_blurred");
        commands.entity(entity).insert(AoTextures {
            raw,
            blurred,
            apply,
        });
    }
}

/// The samples of the main attachment a pass on `target` draws into: the multisampled texture
/// when there is one.
fn attachment_samples(target: &ViewTarget) -> u32 {
    target
        .sampled_main_texture()
        .map_or(1, |t| t.sample_count())
}

#[derive(Default)]
struct AoNode;

impl ViewNode for AoNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static ViewDepthTexture,
        &'static ViewUniformOffset,
        &'static AoTextures,
        &'static ExtractedCamera,
        Option<&'static MainPassResolutionOverride>,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        context: &mut RenderContext<'w>,
        (target, depth, view_offset, textures, camera, resolution): QueryItem<
            'w,
            '_,
            Self::ViewQuery,
        >,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        if !depth
            .texture
            .usage()
            .contains(TextureUsages::TEXTURE_BINDING)
        {
            return Ok(());
        }
        // On a switch between MSAA and TAA the depth and the main attachment change their
        // samples a frame apart; that frame draws without AO.
        if depth.texture.sample_count() != attachment_samples(target) {
            return Ok(());
        }
        let p = world.resource::<AoPipelines>();
        let cache = world.resource::<PipelineCache>();
        let ms = (depth.texture.sample_count() > 1) as usize;
        let (Some(occlude), Some(blur), Some(apply)) = (
            cache.get_render_pipeline(p.occlude[ms]),
            cache.get_render_pipeline(p.blur),
            cache.get_render_pipeline(textures.apply),
        ) else {
            return Ok(());
        };
        let Some(view) = world.resource::<ViewUniforms>().uniforms.binding() else {
            return Ok(());
        };
        let Some(light) = world.get_resource::<SharedLightBuffer>() else {
            return Ok(());
        };
        let device = context.render_device().clone();
        let params = p.params.as_entire_binding();
        // `main_texture_view` is the single-sample texture the opaque pass resolved into.
        let occlude_bind = device.create_bind_group(
            "redux_ao_occlude",
            &cache.get_bind_group_layout(&p.occlude_layouts[ms]),
            &BindGroupEntries::with_indices((
                (0, depth.view()),
                (1, target.main_texture_view()),
                (2, view.clone()),
                (3, params.clone()),
            )),
        );
        let blur_bind = device.create_bind_group(
            "redux_ao_blur",
            &cache.get_bind_group_layout(&p.blur_layout),
            &BindGroupEntries::with_indices(((0, &textures.raw.default_view), (3, params.clone()))),
        );
        let apply_bind = device.create_bind_group(
            "redux_ao_apply",
            &cache.get_bind_group_layout(&p.apply_layouts[ms]),
            &BindGroupEntries::with_indices((
                (0, depth.view()),
                (1, &textures.blurred.default_view),
                (2, view),
                (3, params),
                (4, light.0.as_entire_binding()),
            )),
        );
        let half = |encoder: &mut CommandEncoder,
                    label: &'static str,
                    out: &TextureView,
                    pipeline: &RenderPipeline,
                    bind: &BindGroup,
                    offsets: &[u32]| {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: out,
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
            pass.set_bind_group(0, bind, offsets);
            pass.draw(0..3, 0..1);
        };
        let encoder = context.command_encoder();
        half(
            encoder,
            "redux_ao_occlude",
            &textures.raw.default_view,
            occlude,
            &occlude_bind,
            &[view_offset.offset],
        );
        half(
            encoder,
            "redux_ao_blur",
            &textures.blurred.default_view,
            blur,
            &blur_bind,
            &[],
        );
        // The main attachment (MSAA with its resolve when multisampled): later passes load it
        // darkened.
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("redux_ao_apply"),
            color_attachments: &[Some(target.get_color_attachment())],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        if let Some(v) = Viewport::from_viewport_and_override(camera.viewport.as_ref(), resolution)
        {
            pass.set_viewport(
                v.physical_position.x as f32,
                v.physical_position.y as f32,
                v.physical_size.x as f32,
                v.physical_size.y as f32,
                v.depth.start,
                v.depth.end,
            );
        }
        pass.set_pipeline(apply);
        pass.set_bind_group(0, &apply_bind, &[view_offset.offset]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}
