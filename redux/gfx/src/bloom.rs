//! Bloom (LOOK-12): the HDR excess of the world view (lava, lit windows, spells: whatever is drawn
//! brighter than white) extracted at half size, blurred by a separable Gaussian and added back as
//! a halo before the FFX combine clamps the scene. Off, nothing runs and the image is unchanged.
//!
//! It runs at output resolution, after `StartMainPassPostProcessing`, where an upscaler's output
//! already stands: its targets are sized from the camera's physical viewport, not the main pass.
//!
//! From the everwood graphics fork of Benilla (`benilla-app/src/post/bloom.rs`), at its High tier.
use super::grading::GradeLabel;
use super::post;
use super::Enhanced;
use benilla_world::view::WorldCamera;
use bevy::{
    core_pipeline::{
        core_3d::graph::{Core3d, Node3d},
        FullscreenShader,
    },
    ecs::query::QueryItem,
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
        view::ViewTarget,
        Render, RenderApp, RenderStartup, RenderSystems,
    },
};

/// The blur's size divisor and the halo's gain. Source: everwood's High tier (half size, gain
/// 0.78); its Low tier is quarter size at 0.55.
const DIVISOR: u32 = 2;
const GAIN: f32 = 0.78;
/// The shader's tier lane: 2 is High, whose extract taps one source pixel apart.
const TIER: f32 = 2.0;

#[derive(Component, Clone, Copy, ExtractComponent)]
struct BloomView;

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
pub(super) struct BloomLabel;

pub(super) struct BloomPlugin;

impl Plugin for BloomPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<BloomView>::default())
            .add_systems(Last, update_views);
        if !app.is_plugin_added::<AssetPlugin>() {
            return;
        }
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("bloom.wgsl"),
                "redux_gfx/bloom.wgsl",
            ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(BloomShader(shader))
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(
                Render,
                prepare_textures.in_set(RenderSystems::PrepareResources),
            )
            .add_render_graph_node::<ViewNodeRunner<BloomNode>>(Core3d, BloomLabel)
            // Before the grade, which keeps the excess bloom leaves for the clamp.
            .add_render_graph_edges(
                Core3d,
                (Node3d::StartMainPassPostProcessing, BloomLabel, GradeLabel),
            );
    }
}

fn update_views(
    mut commands: Commands,
    enhanced: Res<Enhanced>,
    cameras: Query<(Entity, &Camera, Has<BloomView>), With<WorldCamera>>,
) {
    for (entity, camera, has) in &cameras {
        let want = enhanced.bloom && camera.is_active;
        if want && !has {
            commands.entity(entity).insert(BloomView);
        } else if !want && has {
            commands.entity(entity).remove::<BloomView>();
        }
    }
}

#[derive(Resource)]
struct BloomShader(Handle<Shader>);

#[derive(Resource)]
struct BloomPipelines {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    extract: CachedRenderPipelineId,
    blur_h: CachedRenderPipelineId,
    blur_v: CachedRenderPipelineId,
    combine: CachedRenderPipelineId,
}

fn init_pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
    shader: Res<BloomShader>,
    cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "redux_bloom_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_2d(TextureSampleType::Float { filterable: true }),
                uniform_buffer_sized(false, std::num::NonZeroU64::new(16)),
            ),
        ),
    );
    let pipeline = |label, entry| {
        post::pipeline(
            &cache,
            &fullscreen,
            label,
            &layout,
            &shader.0,
            entry,
            post::SCENE_FORMAT,
        )
    };
    let pipelines = BloomPipelines {
        extract: pipeline("redux_bloom_extract", "fs_extract"),
        blur_h: pipeline("redux_bloom_blur_h", "fs_blur_h"),
        blur_v: pipeline("redux_bloom_blur_v", "fs_blur_v"),
        combine: pipeline("redux_bloom_combine", "fs_combine"),
        sampler: post::clamped_sampler(&device, FilterMode::Linear),
        layout,
    };
    commands.insert_resource(pipelines);
}

#[derive(Component)]
struct BloomTextures {
    a: CachedTexture,
    b: CachedTexture,
    uniform: Buffer,
    bind_ab: BindGroup,
    bind_ba: BindGroup,
}

fn prepare_textures(
    mut commands: Commands,
    mut textures: ResMut<TextureCache>,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    pipelines: Res<BloomPipelines>,
    views: Query<(Entity, &ExtractedCamera, Option<&BloomTextures>), With<BloomView>>,
) {
    let layout = cache.get_bind_group_layout(&pipelines.layout);
    for (entity, camera, old) in &views {
        let Some(size) = camera.physical_viewport_size else {
            continue;
        };
        let mut texture = |label: &'static str| {
            textures.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size: Extent3d {
                        width: (size.x / DIVISOR).max(8),
                        height: (size.y / DIVISOR).max(8),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: post::SCENE_FORMAT,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let a = texture("redux_bloom_a");
        let b = texture("redux_bloom_b");
        if old.is_some_and(|old| {
            old.a.texture.id() == a.texture.id() && old.b.texture.id() == b.texture.id()
        }) {
            continue;
        }
        let uniform = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("redux_bloom_uniform"),
            contents: &post::bytes(&[[TIER, GAIN, 0.0, 0.0]]),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });
        let bind = |label, view: &TextureView| {
            device.create_bind_group(
                label,
                &layout,
                &BindGroupEntries::sequential((
                    view,
                    &pipelines.sampler,
                    view,
                    uniform.as_entire_binding(),
                )),
            )
        };
        let bind_ab = bind("redux_bloom_a_to_b", &a.default_view);
        let bind_ba = bind("redux_bloom_b_to_a", &b.default_view);
        commands.entity(entity).insert(BloomTextures {
            a,
            b,
            uniform,
            bind_ab,
            bind_ba,
        });
    }
}

#[derive(Default)]
struct BloomNode;

impl ViewNode for BloomNode {
    type ViewQuery = (&'static ViewTarget, &'static BloomTextures);

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        context: &mut RenderContext<'w>,
        (target, textures): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let pipelines = world.resource::<BloomPipelines>();
        let cache = world.resource::<PipelineCache>();
        let (Some(extract), Some(blur_h), Some(blur_v), Some(combine)) = (
            cache.get_render_pipeline(pipelines.extract),
            cache.get_render_pipeline(pipelines.blur_h),
            cache.get_render_pipeline(pipelines.blur_v),
            cache.get_render_pipeline(pipelines.combine),
        ) else {
            return Ok(());
        };
        let device = context.render_device().clone();
        let layout = cache.get_bind_group_layout(&pipelines.layout);
        let bind = |label, source: &TextureView, aux: &TextureView| {
            device.create_bind_group(
                label,
                &layout,
                &BindGroupEntries::sequential((
                    source,
                    &pipelines.sampler,
                    aux,
                    textures.uniform.as_entire_binding(),
                )),
            )
        };
        let extract_bind = bind(
            "redux_bloom_extract",
            target.main_texture_view(),
            &textures.b.default_view,
        );
        let encoder = context.command_encoder();
        let (a, b) = (&textures.a.default_view, &textures.b.default_view);
        post::draw(encoder, "redux_bloom_extract", a, extract, &extract_bind);
        post::draw(encoder, "redux_bloom_blur_h", b, blur_h, &textures.bind_ab);
        post::draw(encoder, "redux_bloom_blur_v", a, blur_v, &textures.bind_ba);
        let out = target.post_process_write();
        let combine_bind = bind("redux_bloom_combine", out.source, a);
        post::draw(
            context.command_encoder(),
            "redux_bloom_combine",
            out.destination,
            combine,
            &combine_bind,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_shader_validates() {
        super::post::validate_wgsl(include_str!("bloom.wgsl"));
    }
}
