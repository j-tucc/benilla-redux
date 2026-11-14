//! The retained pass's shadow casters (an Elderoth engine patch): the cached regions drawn
//! depth-only into each directional shadow view of the world camera, after Bevy's own shadow
//! passes and before the main pass. Stock Benilla spawns no shadow-casting light, so the world
//! camera has no such view, no pipeline is queued and the node returns at once.
//!
//! A region casts when its world bound meets the light view's frustum (no near plane: a caster
//! between the light and the cascade still shades it). Regions a light view sees but the camera
//! has not are assembled here as well, so a building behind the camera shades the ground in front.
//! Every run casts, whatever the camera's per-group admission, both faces, with the colour pass's
//! placement and kill bit and the cutout test on alpha-tested items.

use super::*;
use bevy::camera::primitives::Frustum;
use bevy::pbr::graph::NodePbr;
use bevy::pbr::{ShadowView, ViewLightEntities};
use bevy::render::settings::WgpuFeatures;

/// The two depth pipelines, opaque and cutout, queued at the first shadow view.
#[derive(Resource, Default)]
pub(super) struct GxShadowPipelines(Option<[CachedRenderPipelineId; 2]>);

/// A light view's casting volume: its frustum without the near plane.
fn light_frustum(view: &ExtractedView) -> Frustum {
    let clip_from_world = view
        .clip_from_world
        .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
    Frustum::from_clip_from_world(&clip_from_world)
}

fn casts_into(frusta: &[Frustum], draw: &GxCellDraw) -> bool {
    let at = bevy::math::Affine3A::from_translation(draw.origin);
    frusta
        .iter()
        .any(|f| f.intersects_obb(&draw.aabb, &at, false, true))
}

/// Queue the depth pipelines at the first shadow view, and assemble the regions the light views
/// see that the camera has not.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_shadow_casters(
    gx: Res<GxWorld>,
    mut cache: ResMut<GxGpuCache>,
    mut pool: ResMut<GxTexturePool>,
    pipes: Res<GxPipelines>,
    mut shadow_pipes: ResMut<GxShadowPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    asset_server: Res<AssetServer>,
    images: Res<RenderAssets<GpuImage>>,
    cameras: Query<&ViewLightEntities, With<StaticGxView>>,
    light_views: Query<&ExtractedView, With<ShadowView>>,
) {
    let frusta: Vec<Frustum> = cameras
        .iter()
        .flat_map(|l| l.lights.iter())
        .filter_map(|e| light_views.get(*e).ok())
        .map(light_frustum)
        .collect();
    if frusta.is_empty() {
        return;
    }
    if shadow_pipes.0.is_none() {
        let shader: Handle<Shader> =
            asset_server.load("embedded://benilla_world/shaders/static_gx.wgsl");
        let unclipped = render_device
            .features()
            .contains(WgpuFeatures::DEPTH_CLIP_CONTROL);
        let queue = |cutout: bool| {
            let mut defs = vec![ShaderDefVal::from("GX_DEPTH")];
            if cutout {
                defs.push("GX_CUTOUT".into());
            }
            if !unclipped {
                defs.push("GX_UNCLIPPED_EMULATION".into());
            }
            pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some(format!("static_gx_depth c{}", u8::from(cutout)).into()),
                layout: vec![pipes.view_layout.clone(), pipes.cell_layout.clone()],
                vertex: VertexState {
                    shader: shader.clone(),
                    shader_defs: defs.clone(),
                    entry_point: Some("vertex_depth".into()),
                    buffers: vec![vertex_layout()],
                },
                fragment: (cutout || !unclipped).then(|| FragmentState {
                    shader: shader.clone(),
                    shader_defs: defs,
                    entry_point: Some("fragment_depth".into()),
                    targets: vec![],
                }),
                primitive: PrimitiveState {
                    cull_mode: None,
                    unclipped_depth: unclipped,
                    ..Default::default()
                },
                depth_stencil: Some(DepthStencilState {
                    format: CORE_3D_DEPTH_FORMAT,
                    depth_write_enabled: true,
                    depth_compare: CompareFunction::GreaterEqual,
                    stencil: StencilState::default(),
                    bias: DepthBiasState::default(),
                }),
                multisample: MultisampleState::default(),
                ..default()
            })
        };
        shadow_pipes.0 = Some([queue(false), queue(true)]);
    }
    if gx.cells.is_empty() && gx.wmos.is_empty() && gx.props.is_empty() {
        return;
    }
    let mut assemble = |draw: &GxCellDraw| {
        assemble_region(
            draw,
            &mut pool,
            &pipes,
            &pipeline_cache,
            &render_device,
            &render_queue,
            &images,
        )
    };
    for (key, draw) in &gx.cells {
        if !cache.cells.contains_key(key) && casts_into(&frusta, draw) {
            if let Some(gpu) = assemble(draw) {
                cache.cells.insert(*key, gpu);
            }
        }
    }
    for (key, draw) in &gx.wmos {
        if !cache.wmos.contains_key(key) && casts_into(&frusta, draw) {
            if let Some(gpu) = assemble(draw) {
                cache.wmos.insert(*key, gpu);
            }
        }
    }
    for (key, draw) in &gx.props {
        if !cache.props.contains_key(key) && casts_into(&frusta, draw) {
            if let Some(gpu) = assemble(draw) {
                cache.props.insert(*key, gpu);
            }
        }
    }
    pool.drain_pending(&render_device, &render_queue);
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct StaticGxShadowLabel;

#[derive(Default)]
struct StaticGxShadowNode;

impl ViewNode for StaticGxShadowNode {
    type ViewQuery = (&'static ViewLightEntities, &'static StaticGxView);

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        (lights, _marker): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        if lights.lights.is_empty() {
            return Ok(());
        }
        let Some(ids) = world.resource::<GxShadowPipelines>().0 else {
            return Ok(());
        };
        let pipeline_cache = world.resource::<PipelineCache>();
        let (Some(opaque), Some(cutout)) = (
            pipeline_cache.get_render_pipeline(ids[0]),
            pipeline_cache.get_render_pipeline(ids[1]),
        ) else {
            return Ok(());
        };
        let Some(view_bind) = world.get_resource::<GxViewBind>() else {
            return Ok(());
        };
        let gx = world.resource::<GxWorld>();
        let cache = world.resource::<GxGpuCache>();
        let meshes = world.resource::<RenderAssets<RenderMesh>>();
        let allocator = world.resource::<MeshAllocator>();
        let cached = cache
            .cells
            .iter()
            .filter_map(|(k, gpu)| gx.cells.get(k).map(|d| (gpu, &**d)))
            .chain(
                cache
                    .wmos
                    .iter()
                    .filter_map(|(k, gpu)| gx.wmos.get(k).map(|d| (gpu, &**d))),
            )
            .chain(
                cache
                    .props
                    .iter()
                    .filter_map(|(k, gpu)| gx.props.get(k).map(|d| (gpu, &**d))),
            );
        let all: Vec<(&GxCellGpu, &GxCellDraw)> = cached.collect();
        for &light in &lights.lights {
            let (Some(shadow_view), Some(view), Some(offset)) = (
                world.get::<ShadowView>(light),
                world.get::<ExtractedView>(light),
                world.get::<ViewUniformOffset>(light),
            ) else {
                continue;
            };
            let frustum = [light_frustum(view)];
            let mut pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
                label: Some("static_gx_shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(
                    shadow_view.depth_attachment.get_attachment(StoreOp::Store),
                ),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &view_bind.0, &[offset.offset]);
            for (gpu, draw) in all.iter().filter(|(_, d)| casts_into(&frustum, d)) {
                let Some(mesh) = meshes.get(draw.mesh.id()) else {
                    continue;
                };
                let (Some(vslice), Some(islice)) = (
                    allocator.mesh_vertex_slice(&draw.mesh.id()),
                    allocator.mesh_index_slice(&draw.mesh.id()),
                ) else {
                    continue;
                };
                let bevy::render::mesh::RenderMeshBufferInfo::Indexed { index_format, .. } =
                    &mesh.buffer_info
                else {
                    continue;
                };
                pass.set_vertex_buffer(0, vslice.buffer.slice(..));
                pass.set_index_buffer(islice.buffer.slice(..), *index_format);
                for run in &gpu.runs {
                    pass.set_render_pipeline(if run.cutout { cutout } else { opaque });
                    pass.set_bind_group(1, &gpu.bind_groups[run.slot].1, &[]);
                    pass.draw_indexed(
                        (islice.range.start + run.index_range.start)
                            ..(islice.range.start + run.index_range.end),
                        i32::try_from(vslice.range.start).unwrap_or(0),
                        0..1,
                    );
                }
            }
        }
        Ok(())
    }
}

/// Wire the casters: after Bevy's shadow passes, which clear each view's depth on first use.
pub(super) fn build(render_app: &mut bevy::app::SubApp) {
    render_app
        .init_resource::<GxShadowPipelines>()
        .add_render_graph_node::<ViewNodeRunner<StaticGxShadowNode>>(Core3d, StaticGxShadowLabel)
        .add_render_graph_edges(
            Core3d,
            (
                NodePbr::LateShadowPass,
                StaticGxShadowLabel,
                Node3d::StartMainPass,
            ),
        );
}
