//! Sun rays (LOOK-16): long beams fanning from the sun through gaps in the canopy and between
//! buildings, at any range. The light shafts (`shafts.rs`) march the sun-shadow map, which ends at
//! 80 yd; these work in screen space from the sun disc, so a gap in a far tree line throws rays too.
//!
//! Four full-screen stages ([`rays.wgsl`]), after the shafts and before TAA resolves the frame: a
//! quarter-resolution mask of the sky seen near the sun, two quarter-resolution radial blurs toward
//! the sun's screen position (16 coarse taps over the span, then 16 fine taps over one coarse
//! step: 256 evenly spaced taps, no noise and no history, so a still frame is stable and a camera
//! cut needs no reset), and a full-resolution composite lifted over the scene. The method is
//! Mitchell's "Volumetric Light Scattering as a Post-Process" (GPU Gems 3, ch. 13).
//!
//! The rays fade out as the sun leaves the screen, sets, or hides behind storm cloud, and are off
//! indoors. Their colour is the sun's light leaned toward the zone fog's hue, as the sunlit air's
//! (`scatter.rs`).
//!
//! Resolution: everything runs at render resolution. The mask and blurs cover a quarter of the
//! main pass's rectangle a side (the depth target's texels, or fewer under an upscaler's
//! `MainPassResolutionOverride`) and treat that rectangle as the whole target; the composite draws
//! only into it.
use super::Enhanced;
use benilla_world::{lighting::WowLighting, view::WorldCamera, wmo_portal::CameraInteriorClaim};
use bevy::{
    camera::{MainPassResolutionOverride, Viewport},
    core_pipeline::{core_3d::graph::Core3d, core_3d::graph::Node3d, FullscreenShader},
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
        texture::{CachedTexture, TextureCache},
        view::{ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
        Render, RenderApp, RenderStartup, RenderSystems,
    },
};

/// The rays' gain over the sun's light. Deviation: no source value; tuned in stills against the
/// LOOK-16 references (the golden and blue forests) so beams read without blowing out the sky.
const GAIN: f32 = 1.8;
/// The mask's radius round the sun, in viewport heights: how far from the disc the sky still
/// throws rays. Deviation: tuned in stills.
const RADIUS: f32 = 0.45;
/// The share of the way to the sun the blur spans. Source: Mitchell's `Density` (GPU Gems 3,
/// ch. 13), whose example uses 0.84; Deviation: 0.8, tuned in stills.
const SPAN: f32 = 0.8;
/// The weight lost per coarse tap. Source: Mitchell's `Decay` (0.9–1.0 in his example);
/// Deviation: per coarse tap, each of which stands for 16 of his, 0.93.
const DECAY: f32 = 0.93;
/// How far outside the viewport (in uv) the sun may stand before the rays are gone.
const OFF_SCREEN: f32 = 0.35;
/// The quarter-resolution targets: `.r` the mask or the blurred rays.
const FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Where unit `to_sun` (world axes) lands in a viewport seen through `clip_from_view` by a camera
/// turned by `rotation`: uv, y down, `None` behind the camera. Directions only, so the camera's
/// absolute position never enters.
pub(super) fn sun_screen(rotation: Quat, clip_from_view: Mat4, to_sun: Vec3) -> Option<Vec2> {
    let v = rotation.inverse() * to_sun;
    let clip = clip_from_view * v.extend(0.0);
    if clip.w <= 1e-4 {
        return None;
    }
    let ndc = clip.truncate().truncate() / clip.w;
    Some(Vec2::new(0.5 + 0.5 * ndc.x, 0.5 - 0.5 * ndc.y))
}

/// The rays' fade for the sun at `uv` (or behind the camera), its height (`sin(elevation)`) and
/// the storm blend: gone `OFF_SCREEN` outside the viewport, as it sets, and under storm cloud.
pub(super) fn fade(uv: Option<Vec2>, sun_y: f32, bcc: f32) -> f32 {
    let Some(uv) = uv else {
        return 0.0;
    };
    let outside = (Vec2::ZERO - uv)
        .max(uv - Vec2::ONE)
        .max(Vec2::ZERO)
        .length();
    let screen = 1.0 - super::post::smoothstep(0.0, OFF_SCREEN, outside);
    let horizon = super::post::smoothstep(-0.03, 0.06, sun_y);
    screen * horizon * (1.0 - 0.85 * bcc.clamp(0.0, 1.0))
}

/// A world view's ray rows, present only while rays are on and the sun can throw them.
#[derive(Component, Clone, Copy, PartialEq, ExtractComponent)]
struct RaysView([[f32; 4]; 2]);

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
pub(crate) struct RaysLabel;

pub(super) struct RaysPlugin;

impl Plugin for RaysPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<RaysView>::default())
            .add_systems(Last, update_views);
        if !app.is_plugin_added::<AssetPlugin>() {
            return;
        }
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("rays.wgsl"),
                "redux_gfx/rays.wgsl",
            ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(RaysShader(shader))
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(
                Render,
                prepare_resources.in_set(RenderSystems::PrepareResources),
            )
            .add_render_graph_node::<ViewNodeRunner<RaysNode>>(Core3d, RaysLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    super::shafts::ShaftsLabel,
                    RaysLabel,
                    Node3d::StartMainPassPostProcessing,
                ),
            );
    }
}

/// The world camera's parts the rays read and mark.
type RaysCamera = (
    Entity,
    &'static Camera,
    &'static GlobalTransform,
    &'static mut Camera3d,
    Option<&'static RaysView>,
);

fn update_views(
    mut commands: Commands,
    enhanced: Res<Enhanced>,
    lighting: Res<WowLighting>,
    interior: Option<Res<CameraInteriorClaim>>,
    mut cameras: Query<RaysCamera, With<WorldCamera>>,
) {
    let indoors = interior.is_some_and(|c| c.0.is_some());
    let to_sun = lighting.celestial_dir().normalize_or(Vec3::Y);
    let colour = super::scatter::colour(
        Vec3::from_array(lighting.diffuse),
        Vec3::from_array(lighting.fog_color),
    );
    for (entity, camera, transform, mut camera3d, old) in &mut cameras {
        let uv = sun_screen(transform.rotation(), camera.clip_from_view(), to_sun);
        let f = fade(uv, to_sun.y, lighting.storm_bcc());
        let live = enhanced.rays && !indoors && camera.is_active && f > 0.0;
        if !live {
            if old.is_some() {
                commands.entity(entity).remove::<RaysView>();
            }
            continue;
        }
        let binding = TextureUsages::TEXTURE_BINDING.bits();
        if camera3d.depth_texture_usages.0 & binding == 0 {
            camera3d.depth_texture_usages.0 |= binding;
        }
        let uv = uv.unwrap_or_default();
        let c = colour * (GAIN * f);
        let rows = RaysView([[uv.x, uv.y, RADIUS, SPAN], [c.x, c.y, c.z, DECAY]]);
        if old != Some(&rows) {
            commands.entity(entity).insert(rows);
        }
    }
}

#[derive(Resource)]
struct RaysShader(Handle<Shader>);

#[derive(Resource)]
struct RaysPipelines {
    /// By `samples > 1`.
    mask_layouts: [BindGroupLayoutDescriptor; 2],
    blur_layout: BindGroupLayoutDescriptor,
    composite_layout: BindGroupLayoutDescriptor,
    mask: [CachedRenderPipelineId; 2],
    coarse: CachedRenderPipelineId,
    fine: CachedRenderPipelineId,
    composite: CachedRenderPipelineId,
    sampler: Sampler,
}

fn init_pipelines(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    shader: Res<RaysShader>,
    fullscreen: Res<FullscreenShader>,
) {
    let mask_layout = |ms: bool| {
        BindGroupLayoutDescriptor::new(
            "redux_rays_mask",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: false }),
                    if ms {
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
    let blur_layout = BindGroupLayoutDescriptor::new(
        "redux_rays_blur",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer_sized(false, None),
            ),
        ),
    );
    let composite_layout = BindGroupLayoutDescriptor::new(
        "redux_rays_composite",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer_sized(false, None),
            ),
        ),
    );
    let mask_layouts = [mask_layout(false), mask_layout(true)];
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
    let mask = [false, true].map(|ms| {
        queue(
            "redux_rays_mask",
            &mask_layouts[ms as usize],
            "RAYS_MASK",
            "mask",
            ms,
            FORMAT,
        )
    });
    let coarse = queue(
        "redux_rays_coarse",
        &blur_layout,
        "RAYS_BLUR",
        "blur_coarse",
        false,
        FORMAT,
    );
    let fine = queue(
        "redux_rays_fine",
        &blur_layout,
        "RAYS_BLUR",
        "blur_fine",
        false,
        FORMAT,
    );
    let composite = queue(
        "redux_rays_composite",
        &composite_layout,
        "RAYS_COMPOSITE",
        "composite",
        false,
        super::post::SCENE_FORMAT,
    );
    commands.insert_resource(RaysPipelines {
        mask_layouts,
        blur_layout,
        composite_layout,
        mask,
        coarse,
        fine,
        composite,
        sampler: super::post::clamped_sampler(&device, FilterMode::Linear),
    });
}

/// A view's two quarter-resolution targets, ping-ponged: mask, coarse blur, fine blur.
#[derive(Component)]
struct RaysTextures([CachedTexture; 2]);

fn prepare_resources(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut textures: ResMut<TextureCache>,
    views: Query<(
        Entity,
        &RaysView,
        super::post::DepthAndOverride,
        Option<&super::post::ViewUniform<RaysView>>,
    )>,
) {
    for (entity, rays, (depth, resolution), buffer) in &views {
        if let Some(new) =
            super::post::write_uniform(&device, &queue, buffer, "redux_rays", &rays.0)
        {
            commands.entity(entity).insert(new);
        }
        // The main pass's rectangle: smaller than the depth target under an upscaler.
        let full = super::post::main_pass_size(depth.texture.size(), resolution);
        let size = Extent3d {
            width: full.width.div_ceil(4).max(1),
            height: full.height.div_ceil(4).max(1),
            depth_or_array_layers: 1,
        };
        let mut quarter = |label: &'static str| {
            textures.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: FORMAT,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let a = quarter("redux_rays_a");
        let b = quarter("redux_rays_b");
        commands.entity(entity).insert(RaysTextures([a, b]));
    }
}

#[derive(Default)]
struct RaysNode;

impl ViewNode for RaysNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static ExtractedCamera,
        Option<&'static MainPassResolutionOverride>,
        &'static ViewDepthTexture,
        &'static ViewUniformOffset,
        &'static RaysTextures,
        &'static super::post::ViewUniform<RaysView>,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        context: &mut RenderContext<'w>,
        (target, camera, resolution, depth, view_offset, textures, uniform): QueryItem<
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
        let p = world.resource::<RaysPipelines>();
        let cache = world.resource::<PipelineCache>();
        let ms = (depth.texture.sample_count() > 1) as usize;
        let (Some(mask), Some(coarse), Some(fine), Some(composite)) = (
            cache.get_render_pipeline(p.mask[ms]),
            cache.get_render_pipeline(p.coarse),
            cache.get_render_pipeline(p.fine),
            cache.get_render_pipeline(p.composite),
        ) else {
            return Ok(());
        };
        let Some(view) = world.resource::<ViewUniforms>().uniforms.binding() else {
            return Ok(());
        };
        let device = context.render_device().clone();
        let [a, b] = &textures.0;
        let mask_bind = device.create_bind_group(
            "redux_rays_mask",
            &cache.get_bind_group_layout(&p.mask_layouts[ms]),
            &BindGroupEntries::sequential((
                target.main_texture_view(),
                depth.view(),
                view.clone(),
                uniform.0.as_entire_binding(),
            )),
        );
        let blur_bind = |from: &CachedTexture| {
            device.create_bind_group(
                "redux_rays_blur",
                &cache.get_bind_group_layout(&p.blur_layout),
                &BindGroupEntries::sequential((
                    &from.default_view,
                    &p.sampler,
                    uniform.0.as_entire_binding(),
                )),
            )
        };
        let quarter = |encoder: &mut CommandEncoder,
                       label: &'static str,
                       out: &CachedTexture,
                       pipeline: &RenderPipeline,
                       bind: &BindGroup,
                       offsets: &[u32]| {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &out.default_view,
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
        quarter(
            encoder,
            "redux_rays_mask",
            a,
            mask,
            &mask_bind,
            &[view_offset.offset],
        );
        quarter(encoder, "redux_rays_coarse", b, coarse, &blur_bind(a), &[]);
        quarter(encoder, "redux_rays_fine", a, fine, &blur_bind(b), &[]);
        let out = target.post_process_write();
        let composite_bind = device.create_bind_group(
            "redux_rays_composite",
            &cache.get_bind_group_layout(&p.composite_layout),
            &BindGroupEntries::sequential((
                out.source,
                &a.default_view,
                &p.sampler,
                view,
                uniform.0.as_entire_binding(),
            )),
        );
        let mut pass = context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("redux_rays_composite"),
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

    fn projection() -> Mat4 {
        Mat4::perspective_infinite_reverse_rh(1.0, 16.0 / 9.0, 0.1)
    }

    #[test]
    fn the_sun_ahead_lands_mid_screen_and_behind_lands_nowhere() {
        // Bevy cameras look down -Z.
        let ahead = sun_screen(Quat::IDENTITY, projection(), Vec3::NEG_Z).unwrap();
        assert!((ahead - Vec2::splat(0.5)).length() < 1e-5, "{ahead}");
        assert_eq!(sun_screen(Quat::IDENTITY, projection(), Vec3::Z), None);
        // Up is up the screen (uv y down), right is right.
        let up = sun_screen(
            Quat::IDENTITY,
            projection(),
            Vec3::new(0.0, 0.2, -1.0).normalize(),
        );
        assert!(up.unwrap().y < 0.5);
        let right = sun_screen(
            Quat::IDENTITY,
            projection(),
            Vec3::new(0.2, 0.0, -1.0).normalize(),
        );
        assert!(right.unwrap().x > 0.5);
        // Turning the camera left a quarter turn brings a sun on the left to the middle.
        let turned = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let left = sun_screen(turned, projection(), Vec3::NEG_X).unwrap();
        assert!((left - Vec2::splat(0.5)).length() < 1e-5, "{left}");
    }

    #[test]
    fn the_rays_fade_off_screen_below_the_horizon_and_in_storms() {
        let mid = Some(Vec2::splat(0.5));
        assert_eq!(fade(mid, 0.3, 0.0), 1.0);
        assert_eq!(fade(None, 0.3, 0.0), 0.0);
        assert_eq!(fade(Some(Vec2::new(1.5, 0.5)), 0.3, 0.0), 0.0);
        let edge = fade(Some(Vec2::new(1.1, 0.5)), 0.3, 0.0);
        assert!(edge > 0.0 && edge < 1.0);
        assert_eq!(fade(mid, -0.1, 0.0), 0.0);
        assert!(fade(mid, 0.3, 1.0) < 0.2);
    }

    /// The shader with one stage's defs applied (`#ifdef`, `#else`, `#endif`), `View` stood in.
    fn stage(defs: &[&str]) -> String {
        let mut out = String::new();
        let mut keep = vec![true];
        for line in include_str!("rays.wgsl").lines() {
            let t = line.trim();
            if let Some(def) = t.strip_prefix("#ifdef ") {
                let on = *keep.last().unwrap() && defs.contains(&def.trim());
                keep.push(on);
            } else if t == "#else" {
                let inner = keep.pop().unwrap();
                let outer = *keep.last().unwrap();
                keep.push(outer && !inner);
            } else if t == "#endif" {
                keep.pop();
            } else if t.starts_with("#import") {
                out.push_str("struct View { main_pass_viewport: vec4<f32> }\n");
            } else if *keep.last().unwrap() {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    #[test]
    fn every_stage_of_the_shader_validates() {
        for defs in [
            &["RAYS_MASK"][..],
            &["RAYS_MASK", "MULTISAMPLED"],
            &["RAYS_BLUR"],
            &["RAYS_COMPOSITE"],
        ] {
            super::super::post::validate_wgsl(&stage(defs));
        }
    }
}
