//! The distance haze (LOOK-12), part of the modern fog: a full-screen pass over the world view's
//! depth, after the main pass and before the post passes, that thickens the air past ten yards
//! toward the fog colour, thicker at dawn, in weather, and thinner indoors.
//!
//! Ported from the everwood graphics fork of Benilla (`benilla-app/src/volumetric_fog.rs`):
//! its `density` law, `fog_path` integral and luminance-keeping blend. Not ported: the
//! shadow-mapped sun shafts (a ray march per pixel), the lamp halos and the sea-level plane.
//!
//! The ground mist (LOOK-14, `mist.rs`) and the sunlit air (LOOK-16, `scatter.rs`) are drawn by
//! the same pass, which runs when any of the three is on.
//!
//! It runs at render resolution, inside the main pass's span: it draws only into the rectangle
//! the main pass drew, which `MainPassResolutionOverride` shrinks when an upscaler renders below
//! the output size, and reads the scene and depth by pixel, never by a UV over the whole target.
use super::Enhanced;
use benilla_world::{
    lighting::{GameClock, GfxData, WowLighting},
    view::WorldCamera,
    weather::WeatherState,
    wmo_portal::CameraInteriorClaim,
};
use bevy::{
    camera::{MainPassResolutionOverride, Viewport},
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
        renderer::{RenderContext, RenderDevice, RenderQueue},
        view::{ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
        Render, RenderApp, RenderStartup, RenderSystems,
    },
};

/// The haze's extinction per yard: 0.004 in clear day air, up to 2.5× at dawn and 3× more in
/// weather, 0.18× indoors. Source: everwood `volumetric_fog::density`.
pub(super) fn density(minute: f32, weather: f32, indoors: bool) -> f32 {
    let dawn = (1.0 - ((minute - 390.0) / 90.0).abs()).clamp(0.0, 1.0);
    let dawn = dawn * dawn * (3.0 - 2.0 * dawn);
    0.004 * (1.0 + dawn * 1.5 + weather.clamp(0.0, 1.0) * 2.0) * if indoors { 0.18 } else { 1.0 }
}

/// A world view's haze rows, present only while the haze is on.
#[derive(Component, Clone, Copy, PartialEq, ExtractComponent)]
struct HazeView([[f32; 4]; ROWS]);

/// How many of the firelight's lights (nearest first) glow in the air.
const HALOS: usize = 4;
/// The haze uniform's rows: eight for the haze, mist and sunlit air, then the firelight's `MISC`
/// row and its [`HALOS`] nearest lights.
const ROWS: usize = 9 + HALOS;

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
pub(super) struct HazeLabel;

pub(super) struct HazePlugin;

impl Plugin for HazePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<HazeView>::default())
            .add_systems(Last, update_views);
        if !app.is_plugin_added::<AssetPlugin>() {
            return;
        }
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("haze.wgsl"),
                "redux_gfx/haze.wgsl",
            ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(HazeShader(shader))
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(
                Render,
                prepare_uniforms.in_set(RenderSystems::PrepareResources),
            )
            .add_render_graph_node::<ViewNodeRunner<HazeNode>>(Core3d, HazeLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    Node3d::EndMainPass,
                    HazeLabel,
                    Node3d::StartMainPassPostProcessing,
                ),
            );
    }
}

#[allow(clippy::too_many_arguments)]
fn update_views(
    mut commands: Commands,
    enhanced: Res<Enhanced>,
    lighting: Res<WowLighting>,
    frame: Res<GfxData>,
    clock: Option<Res<GameClock>>,
    weather: Option<Res<WeatherState>>,
    interior: Option<Res<CameraInteriorClaim>>,
    mut cameras: Query<(Entity, &Camera, &mut Camera3d, Option<&HazeView>), With<WorldCamera>>,
) {
    let minute = clock.map_or(720.0, |c| c.minute as f32);
    let weather = weather.map_or(0.0, |w| w.sky_density.max(w.effect_density));
    let indoors = interior.is_some_and(|c| c.0.is_some());
    let fog = lighting.fog_color;
    let haze = if enhanced.fog {
        density(minute, weather, indoors)
    } else {
        0.0
    };
    let air = super::scatter::rows(
        enhanced.scatter,
        lighting.celestial_dir(),
        Vec3::from_array(lighting.diffuse),
        Vec3::from_array(fog),
        lighting.storm_bcc(),
        density(minute, weather, indoors),
        indoors,
    );
    let mut rows = [[0.0; 4]; ROWS];
    rows[..9].copy_from_slice(&[
        [fog[0], fog[1], fog[2], haze],
        frame.rows[super::rows::FOG_B],
        frame.rows[super::rows::FOG_C],
        frame.rows[super::rows::FOG_D],
        frame.rows[super::rows::MIST],
        air[0],
        air[1],
        super::scatter::PHASE,
        frame.rows[super::rows::MISC],
    ]);
    // The firelight's nearest lights glow in the air round them (zero count when it is off).
    let fires = &frame.rows[super::rows::FIRES..super::rows::FIRES + HALOS];
    rows[9..].copy_from_slice(fires);
    let on = enhanced.fog || enhanced.mist || enhanced.scatter || enhanced.firelight;
    for (entity, camera, mut camera3d, old) in &mut cameras {
        if !(on && camera.is_active) {
            if old.is_some() {
                commands.entity(entity).remove::<HazeView>();
            }
            continue;
        }
        // The pass reads the depth buffer; Classic leaves its usage as the engine set it.
        let binding = TextureUsages::TEXTURE_BINDING.bits();
        if camera3d.depth_texture_usages.0 & binding == 0 {
            camera3d.depth_texture_usages.0 |= binding;
        }
        if old != Some(&HazeView(rows)) {
            commands.entity(entity).insert(HazeView(rows));
        }
    }
}

#[derive(Resource)]
struct HazeShader(Handle<Shader>);

#[derive(Resource)]
struct HazePipeline {
    /// Single-sampled and multisampled depth, by `samples > 1`.
    layouts: [BindGroupLayoutDescriptor; 2],
    pipelines: [CachedRenderPipelineId; 2],
}

fn init_pipeline(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    shader: Res<HazeShader>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout = |multisampled: bool| {
        BindGroupLayoutDescriptor::new(
            "redux_haze_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: false }),
                    if multisampled {
                        texture_depth_2d_multisampled()
                    } else {
                        texture_depth_2d()
                    },
                    uniform_buffer::<ViewUniform>(true),
                    uniform_buffer_sized(false, None),
                ),
            ),
        )
    };
    let layouts = [layout(false), layout(true)];
    let pipelines = [false, true].map(|multisampled| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("redux_haze".into()),
            layout: vec![layouts[multisampled as usize].clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.0.clone(),
                shader_defs: if multisampled {
                    vec!["MULTISAMPLED".into()]
                } else {
                    vec![]
                },
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format: super::post::SCENE_FORMAT,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            ..default()
        })
    });
    commands.insert_resource(HazePipeline { layouts, pipelines });
}

fn prepare_uniforms(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    views: Query<(
        Entity,
        &HazeView,
        Option<&super::post::ViewUniform<HazeView>>,
    )>,
) {
    for (entity, haze, buffer) in &views {
        if let Some(new) =
            super::post::write_uniform(&device, &queue, buffer, "redux_haze", &haze.0)
        {
            commands.entity(entity).insert(new);
        }
    }
}

#[derive(Default)]
struct HazeNode;

impl ViewNode for HazeNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static ExtractedCamera,
        Option<&'static MainPassResolutionOverride>,
        &'static ViewDepthTexture,
        &'static ViewUniformOffset,
        &'static HazeView,
        &'static super::post::ViewUniform<HazeView>,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        context: &mut RenderContext<'w>,
        (target, camera, resolution, depth, view_offset, _, uniform): QueryItem<
            'w,
            '_,
            Self::ViewQuery,
        >,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        // The usage change reaches the depth texture a frame after the switch.
        if !depth
            .texture
            .usage()
            .contains(TextureUsages::TEXTURE_BINDING)
        {
            return Ok(());
        }
        let p = world.resource::<HazePipeline>();
        let cache = world.resource::<PipelineCache>();
        let ms = (depth.texture.sample_count() > 1) as usize;
        let Some(pipeline) = cache.get_render_pipeline(p.pipelines[ms]) else {
            return Ok(());
        };
        let Some(view) = world.resource::<ViewUniforms>().uniforms.binding() else {
            return Ok(());
        };
        let out = target.post_process_write();
        let bind = context.render_device().create_bind_group(
            "redux_haze",
            &cache.get_bind_group_layout(&p.layouts[ms]),
            &BindGroupEntries::sequential((
                out.source,
                depth.view(),
                view,
                uniform.0.as_entire_binding(),
            )),
        );
        let mut pass = context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("redux_haze"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: out.destination,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations::default(),
                })],
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[view_offset.offset]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dawn_weather_and_rooms_set_the_density() {
        let noon = density(720.0, 0.0, false);
        assert!((noon - 0.004).abs() < 1e-6);
        assert!(density(390.0, 0.0, false) > noon * 2.4);
        assert!(density(720.0, 1.0, false) > noon * 2.9);
        assert!(density(720.0, 0.0, true) < noon * 0.2);
    }
}
