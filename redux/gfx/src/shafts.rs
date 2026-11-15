//! Light shafts (LOOK-14): the shadow rig's map (`shadows.rs`, aimed at the sun by day and with
//! moonlight on at the moon by night) ray-marched through the air in front of every pixel, so lit
//! air between trees, buildings and the player in-scatters the light toward the eye.
//!
//! Three full-screen stages after the haze ([`shafts.wgsl`]): a half-resolution march of 16
//! (Medium) or 32 (High) samples a ray with an interleaved-gradient-noise start, a half-resolution
//! depth-aware blur that removes the jitter, and a depth-aware upsample lifted over the scene.
//! No temporal history, so a still frame is stable and a camera cut needs no reset.
//!
//! Resolution: everything runs at render resolution. The march and blur cover half the main
//! pass's rectangle, which `MainPassResolutionOverride` shrinks under an upscaler; the march and
//! the composite place each pixel through that rectangle (`main_pass_viewport`), and the composite
//! draws only into it.
//!
//! Ported from the everwood graphics fork of Benilla (MIT OR Apache-2.0), revision `cdc7130b`,
//! `benilla-app/src/volumetric_light.rs`: the steps per tier, the march start and cap, the
//! anisotropy, the gains, the fog tint and extinction, the height falloff.
use super::rows::{MOON_A, MOON_B, SHADOW_A, SHADOW_B, SHADOW_TEXEL};
use super::shadows::{ShadowCascade, SHADOW_DISTANCE, SHAFT_DEPTH_BIAS};
use super::Enhanced;
use benilla_world::{
    lighting::{GameClock, GfxData, WowLighting},
    view::WorldCamera,
    weather::WeatherState,
    wmo_portal::CameraInteriorClaim,
};
use bevy::{
    camera::{MainPassResolutionOverride, Viewport},
    core_pipeline::{core_3d::graph::Core3d, core_3d::graph::Node3d, FullscreenShader},
    ecs::query::QueryItem,
    pbr::{ShadowSamplers, ViewShadowBindings},
    prelude::*,
    render::{
        camera::ExtractedCamera,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_graph::{
            NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
        },
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue},
        texture::{CachedTexture, TextureCache},
        view::{ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
        Render, RenderApp, RenderStartup, RenderSystems,
    },
};

/// March samples by setting (Off, Medium, High). Source: everwood `STEPS`.
const STEPS: [f32; 3] = [0.0, 16.0, 32.0];
/// The march starts this far from the eye. Source: everwood `MARCH_START`.
const MARCH_START: f32 = 0.35;
/// Henyey-Greenstein anisotropy: forward-peaked. Source: everwood `ANISOTROPY`.
const ANISOTROPY: f32 = 0.6;
/// The sun's shaft gain. Source: everwood `SUN_SHAFT_GAIN`.
const SUN_GAIN: f32 = 9.0;
/// The moon's, on top of its dim moonlight intensity. Source: everwood `MOON_SHAFT_GAIN`.
const MOON_GAIN: f32 = 30.0;
/// Cool moonlight, as `moonlight.rs` lights surfaces.
const MOON_TINT: Vec3 = Vec3::new(0.62, 0.74, 1.0);
/// How far the in-scatter colour leans toward the zone fog's hue. Source: everwood `FOG_TINT`.
const FOG_TINT: f32 = 0.35;
/// A zone authored foggier than this fog end scatters more. Source: everwood
/// `FOG_END_REFERENCE`.
const FOG_END_REFERENCE: f32 = 350.0;
/// The air thins with height above the eye (e-folding yd). Source: everwood `HEIGHT_FALLOFF`.
const HEIGHT_FALLOFF: f32 = 10.0;
/// The least the air scatters per yard, whatever the haze: clear midday air still shows beams
/// between trees (LOOK-16). Extinction keeps the haze's law, so the floor brightens the beams
/// without dimming what lies behind them. Deviation: everwood scatters at its haze density alone
/// (0.004 at noon); 0.009 is tuned in stills against the LOOK-16 references.
const MIN_SCATTER: f32 = 0.009;
/// The march and blur targets: `.r` in-scatter, `.g` the marched distance (yd).
const HALF_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// A world view's shaft rows (direction and steps, colour and extinction, range, bias), present
/// only while shafts are on and a body casts.
#[derive(Component, Clone, Copy, PartialEq, ExtractComponent)]
struct ShaftsView([[f32; 4]; 4]);

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
pub(super) struct ShaftsLabel;

pub(super) struct ShaftsPlugin;

impl Plugin for ShaftsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<ShaftsView>::default())
            .add_systems(Last, update_views);
        if !app.is_plugin_added::<AssetPlugin>() {
            return;
        }
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("shafts.wgsl"),
                "redux_gfx/shafts.wgsl",
            ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(ShaftsShader(shader))
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(
                Render,
                prepare_resources.in_set(RenderSystems::PrepareResources),
            )
            .add_render_graph_node::<ViewNodeRunner<ShaftsNode>>(Core3d, ShaftsLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    super::haze::HazeLabel,
                    ShaftsLabel,
                    Node3d::StartMainPassPostProcessing,
                ),
            );
    }
}

/// Extinction per yard: the haze's density law, scaled by how short the zone's fog end is.
fn extinction(minute: f32, weather: f32, fog_end: f32) -> f32 {
    let zone = if fog_end > 1.0 {
        (FOG_END_REFERENCE / fog_end).clamp(0.7, 2.5)
    } else {
        1.0
    };
    super::haze::density(minute, weather, false) * zone
}

/// `colour` leaned toward the zone fog's hue, its luminance kept.
fn fog_tinted(colour: Vec3, fog: Vec3) -> Vec3 {
    let l = fog.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    if l <= 1e-4 {
        return colour;
    }
    colour * Vec3::ONE.lerp(fog / l, FOG_TINT)
}

/// The in-scatter colour of the body the rig holds: the sun's diffuse by its weight, or the moon's
/// tint by its light and the hand-over ramp.
fn body_colour(diffuse: [f32; 3], rows: &GfxData) -> Vec3 {
    let sun = Vec3::from_array(diffuse).lerp(Vec3::ONE, 0.5) * (SUN_GAIN * rows.rows[SHADOW_A][3]);
    let moon = MOON_TINT * (MOON_GAIN * rows.rows[MOON_A][3] * rows.rows[MOON_B][3]);
    sun + moon
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
    mut cameras: Query<(Entity, &Camera, &mut Camera3d, Option<&ShaftsView>), With<WorldCamera>>,
) {
    let indoors = interior.is_some_and(|c| c.0.is_some());
    let steps = STEPS[usize::from(enhanced.shafts.min(2))];
    let colour = body_colour(lighting.diffuse, &frame);
    let live = steps > 0.0 && !indoors && colour.max_element() > 0.0;
    let minute = clock.map_or(720.0, |c| c.minute as f32);
    let weather = weather.map_or(0.0, |w| w.sky_density.max(w.effect_density));
    let sigma = extinction(minute, weather, lighting.fog_span().1);
    let scatter = sigma.max(MIN_SCATTER);
    let colour = fog_tinted(colour, Vec3::from_array(lighting.fog_color));
    let b = frame.rows[SHADOW_B];
    let rows = [
        [b[0], b[1], b[2], steps],
        [colour.x, colour.y, colour.z, sigma],
        [SHADOW_DISTANCE, MARCH_START, ANISOTROPY, HEIGHT_FALLOFF],
        [SHAFT_DEPTH_BIAS, scatter, 0.0, 0.0],
    ];
    for (entity, camera, mut camera3d, old) in &mut cameras {
        if !(live && camera.is_active) {
            if old.is_some() {
                commands.entity(entity).remove::<ShaftsView>();
            }
            continue;
        }
        let binding = TextureUsages::TEXTURE_BINDING.bits();
        if camera3d.depth_texture_usages.0 & binding == 0 {
            camera3d.depth_texture_usages.0 |= binding;
        }
        if old != Some(&ShaftsView(rows)) {
            commands.entity(entity).insert(ShaftsView(rows));
        }
    }
}

#[derive(Resource)]
struct ShaftsShader(Handle<Shader>);

#[derive(Resource)]
struct ShaftsPipelines {
    /// By `samples > 1`.
    march_layouts: [BindGroupLayoutDescriptor; 2],
    blur_layout: BindGroupLayoutDescriptor,
    composite_layouts: [BindGroupLayoutDescriptor; 2],
    march: [CachedRenderPipelineId; 2],
    blur: CachedRenderPipelineId,
    composite: [CachedRenderPipelineId; 2],
}

fn init_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    shader: Res<ShaftsShader>,
    fullscreen: Res<FullscreenShader>,
) {
    let depth = |ms: bool| {
        if ms {
            texture_depth_2d_multisampled()
        } else {
            texture_depth_2d()
        }
    };
    let march_layout = |ms: bool| {
        BindGroupLayoutDescriptor::new(
            "redux_shafts_march",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    depth(ms),
                    uniform_buffer::<ViewUniform>(true),
                    texture_2d_array(TextureSampleType::Depth),
                    sampler(SamplerBindingType::Comparison),
                    uniform_buffer_sized(false, None),
                ),
            ),
        )
    };
    let composite_layout = |ms: bool| {
        BindGroupLayoutDescriptor::new(
            "redux_shafts_composite",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: false }),
                    depth(ms),
                    texture_2d(TextureSampleType::Float { filterable: false }),
                    uniform_buffer::<ViewUniform>(true),
                    uniform_buffer_sized(false, None),
                ),
            ),
        )
    };
    let blur_layout = BindGroupLayoutDescriptor::new(
        "redux_shafts_blur",
        &BindGroupLayoutEntries::single(
            ShaderStages::FRAGMENT,
            texture_2d(TextureSampleType::Float { filterable: false }),
        ),
    );
    let march_layouts = [march_layout(false), march_layout(true)];
    let composite_layouts = [composite_layout(false), composite_layout(true)];
    let queue = |label: &'static str,
                 layout: &BindGroupLayoutDescriptor,
                 stage: &'static str,
                 entry: &'static str,
                 ms: bool,
                 format: TextureFormat| {
        let mut defs: Vec<bevy::shader::ShaderDefVal> = vec![stage.into()];
        if ms {
            defs.push("MULTISAMPLED".into());
        }
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.0.clone(),
                shader_defs: defs,
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            ..default()
        })
    };
    let march = [false, true].map(|ms| {
        queue(
            "redux_shafts_march",
            &march_layouts[ms as usize],
            "SHAFTS_MARCH",
            "march",
            ms,
            HALF_FORMAT,
        )
    });
    let blur = queue(
        "redux_shafts_blur",
        &blur_layout,
        "SHAFTS_BLUR",
        "blur",
        false,
        HALF_FORMAT,
    );
    let composite = [false, true].map(|ms| {
        queue(
            "redux_shafts_composite",
            &composite_layouts[ms as usize],
            "SHAFTS_COMPOSITE",
            "composite",
            ms,
            super::post::SCENE_FORMAT,
        )
    });
    commands.insert_resource(ShaftsPipelines {
        march_layouts,
        blur_layout,
        composite_layouts,
        march,
        blur,
        composite,
    });
}

/// A view's half-resolution targets, while it has a live map to march.
#[derive(Component)]
struct ShaftsTextures {
    marched: CachedTexture,
    blurred: CachedTexture,
}

/// The rows and the cascade, and the half-resolution targets; neither while the map is not live.
fn prepare_resources(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut textures: ResMut<TextureCache>,
    cascade: Res<ShadowCascade>,
    views: Query<(
        Entity,
        &ShaftsView,
        super::post::DepthAndOverride,
        Option<&super::post::ViewUniform<ShaftsView>>,
    )>,
) {
    let c = cascade.0;
    let live = c[SHADOW_TEXEL - super::rows::SHADOW_CLIP][1] >= 0.5;
    for (entity, shafts, (depth, resolution), buffer) in &views {
        if !live {
            commands.entity(entity).remove::<ShaftsTextures>();
            continue;
        }
        let mut rows = shafts.0.to_vec();
        rows.extend_from_slice(&c[..4]);
        if let Some(new) =
            super::post::write_uniform(&device, &queue, buffer, "redux_shafts", &rows)
        {
            commands.entity(entity).insert(new);
        }
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
                    format: HALF_FORMAT,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let marched = half("redux_shafts_marched");
        let blurred = half("redux_shafts_blurred");
        commands
            .entity(entity)
            .insert(ShaftsTextures { marched, blurred });
    }
}

#[derive(Default)]
struct ShaftsNode;

impl ViewNode for ShaftsNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static ExtractedCamera,
        Option<&'static MainPassResolutionOverride>,
        &'static ViewDepthTexture,
        &'static ViewUniformOffset,
        &'static ViewShadowBindings,
        &'static ShaftsTextures,
        &'static super::post::ViewUniform<ShaftsView>,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        context: &mut RenderContext<'w>,
        (target, camera, resolution, depth, view_offset, shadow, textures, uniform): QueryItem<
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
        let p = world.resource::<ShaftsPipelines>();
        let cache = world.resource::<PipelineCache>();
        let ms = (depth.texture.sample_count() > 1) as usize;
        let (Some(march), Some(blur), Some(composite)) = (
            cache.get_render_pipeline(p.march[ms]),
            cache.get_render_pipeline(p.blur),
            cache.get_render_pipeline(p.composite[ms]),
        ) else {
            return Ok(());
        };
        let Some(view) = world.resource::<ViewUniforms>().uniforms.binding() else {
            return Ok(());
        };
        let samplers = world.resource::<ShadowSamplers>();
        let device = context.render_device().clone();
        let march_bind = device.create_bind_group(
            "redux_shafts_march",
            &cache.get_bind_group_layout(&p.march_layouts[ms]),
            &BindGroupEntries::sequential((
                depth.view(),
                view.clone(),
                &shadow.directional_light_depth_texture_view,
                &samplers.directional_light_comparison_sampler,
                uniform.0.as_entire_binding(),
            )),
        );
        let blur_bind = device.create_bind_group(
            "redux_shafts_blur",
            &cache.get_bind_group_layout(&p.blur_layout),
            &BindGroupEntries::single(&textures.marched.default_view),
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
        half(
            context.command_encoder(),
            "redux_shafts_march",
            &textures.marched.default_view,
            march,
            &march_bind,
            &[view_offset.offset],
        );
        half(
            context.command_encoder(),
            "redux_shafts_blur",
            &textures.blurred.default_view,
            blur,
            &blur_bind,
            &[],
        );
        let out = target.post_process_write();
        let composite_bind = device.create_bind_group(
            "redux_shafts_composite",
            &cache.get_bind_group_layout(&p.composite_layouts[ms]),
            &BindGroupEntries::sequential((
                out.source,
                depth.view(),
                &textures.blurred.default_view,
                view,
                uniform.0.as_entire_binding(),
            )),
        );
        let mut pass = context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("redux_shafts_composite"),
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
        pass.set_pipeline(composite);
        pass.set_bind_group(0, &composite_bind, &[view_offset.offset]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_air_scatters_more_in_foggier_zones_and_never_without_a_body() {
        let clear = extinction(720.0, 0.0, 350.0);
        assert!(extinction(720.0, 0.0, 100.0) > clear * 2.0);
        assert!(extinction(720.0, 0.0, 2000.0) < clear);
        let none = GfxData::default();
        assert_eq!(body_colour([1.0; 3], &none), Vec3::ZERO);
    }
}
