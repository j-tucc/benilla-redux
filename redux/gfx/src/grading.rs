//! Colour grading (LOOK-12): one 32³ lookup per pixel on the world view's HDR scene, before the
//! FFX combine clamps it, blending a day and a night cube by the time of day.
//!
//! It runs at output resolution, after `StartMainPassPostProcessing`, where an upscaler's output
//! already stands, on the whole view target.
//!
//! Ported from WarcraftXL (https://github.com/WarcraftXL) by iThorgrim — module wxl-retail-grading, Grading.cpp, shaders/Grading.ps.hlsl.
//! Used with the author's permission; attribution required. Through the everwood graphics fork of
//! Benilla (`benilla-app/src/post/grading.rs`), whose cubes come from a modern client's per-zone
//! BLP strips. Elderoth's cubes are its own, baked at start-up from [`DAY`] and [`NIGHT`]; nothing
//! is read from a modern client. See `THIRD-PARTY.md`.
use super::post::{self, ViewUniform};
use super::Enhanced;
use benilla_world::{ffx_glow::FfxGlowLabel, lighting::GameClock, view::WorldCamera};
use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::{
        core_3d::graph::{Core3d, Node3d},
        FullscreenShader,
    },
    ecs::query::QueryItem,
    prelude::*,
    render::{
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_asset::RenderAssets,
        render_graph::{
            NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
        },
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue},
        texture::GpuImage,
        view::ViewTarget,
        Render, RenderApp, RenderStartup, RenderSystems,
    },
};

/// One authored grade, applied in the gamma space the scene is drawn in: lift, then gain, then a
/// contrast S-curve about mid-grey, then saturation about Rec. 709 luma.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Grade {
    /// Raises the shadows toward this colour (0 = none).
    pub lift: [f32; 3],
    /// Scales every channel (1 = none).
    pub gain: [f32; 3],
    /// 0 = none, 1 = a full smoothstep S-curve.
    pub contrast: f32,
    /// 1 = none.
    pub saturation: f32,
}

/// The daytime grade. Source: authored for Elderoth; the 1.12 client has no grade, and everwood's
/// cubes come from a modern client, which Elderoth does not use. A warm, slightly fuller day:
/// highlights 2 % warmer, shadows a touch cooler, 8 % more saturation, a gentle S-curve.
pub(super) const DAY: Grade = Grade {
    lift: [0.0, 0.0, 0.012],
    gain: [1.02, 1.0, 0.975],
    contrast: 0.12,
    saturation: 1.08,
};

/// The night grade. Source: authored for Elderoth (as [`DAY`]). Cooler and less saturated, as
/// the eye sees under moonlight: blue lifted into the shadows, 15 % less saturation.
pub(super) const NIGHT: Grade = Grade {
    lift: [0.0, 0.006, 0.022],
    gain: [0.96, 0.98, 1.04],
    contrast: 0.08,
    saturation: 0.85,
};

/// How strongly the authored grade applies (1 = fully). Source: everwood's per-zone strength
/// column at its usual full value.
const STRENGTH: f32 = 1.0;

impl Grade {
    fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let mut v = [0.0; 3];
        for i in 0..3 {
            let lifted = c[i] + self.lift[i] * (1.0 - c[i]);
            let gained = (lifted * self.gain[i]).clamp(0.0, 1.0);
            let s = gained * gained * (3.0 - 2.0 * gained);
            v[i] = gained + (s - gained) * self.contrast;
        }
        let luma = 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
        v.map(|x| (luma + (x - luma) * self.saturation).clamp(0.0, 1.0))
    }

    /// The 32³ RGBA8 cube, red fastest, then green, then blue.
    fn cube(&self) -> Vec<u8> {
        let mut cube = Vec::with_capacity(32 * 32 * 32 * 4);
        for b in 0..32 {
            for g in 0..32 {
                for r in 0..32 {
                    let rgb = self.apply([r, g, b].map(|v| v as f32 / 31.0));
                    cube.extend(rgb.map(|v| (v * 255.0).round() as u8));
                    cube.push(255);
                }
            }
        }
        cube
    }
}

fn volume_image(cube: Vec<u8>) -> Image {
    Image::new(
        Extent3d {
            width: 32,
            height: 32,
            depth_or_array_layers: 32,
        },
        TextureDimension::D3,
        cube,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// The night blend by the minute of the day: night until 05:30, day from 07:30 to 18:30, night
/// again from 20:30. Source: everwood's `night_weight`.
fn night_weight(minute: f32) -> f32 {
    let dawn = post::smoothstep(330.0, 450.0, minute);
    let dusk = post::smoothstep(1110.0, 1230.0, minute);
    1.0 - dawn * (1.0 - dusk)
}

#[derive(Resource)]
struct Cubes {
    day: Handle<Image>,
    night: Handle<Image>,
}

/// A world view to grade, with its cubes and `(strength, night)`.
#[derive(Component, Clone, PartialEq, ExtractComponent)]
struct GradeView {
    day: Handle<Image>,
    night: Handle<Image>,
    control: Vec4,
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
pub(super) struct GradeLabel;

pub(super) struct GradingPlugin;

impl Plugin for GradingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<GradeView>::default())
            .add_systems(Last, update_views);
        if !app.is_plugin_added::<AssetPlugin>() {
            return;
        }
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        let cubes = Cubes {
            day: images.add(volume_image(DAY.cube())),
            night: images.add(volume_image(NIGHT.cube())),
        };
        app.insert_resource(cubes);
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("grading.wgsl"),
                "redux_gfx/grading.wgsl",
            ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(GradeShader(shader))
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(
                Render,
                prepare_uniforms.in_set(RenderSystems::PrepareResources),
            )
            .add_render_graph_node::<ViewNodeRunner<GradeNode>>(Core3d, GradeLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    Node3d::StartMainPassPostProcessing,
                    GradeLabel,
                    FfxGlowLabel,
                ),
            );
    }
}

fn update_views(
    mut commands: Commands,
    enhanced: Res<Enhanced>,
    cubes: Option<Res<Cubes>>,
    clock: Option<Res<GameClock>>,
    cameras: Query<(Entity, &Camera, Option<&GradeView>), With<WorldCamera>>,
) {
    let night = night_weight(clock.map_or(720.0, |c| c.minute as f32));
    for (entity, camera, old) in &cameras {
        let next = cubes
            .as_ref()
            .filter(|_| enhanced.grading && camera.is_active)
            .map(|c| GradeView {
                day: c.day.clone(),
                night: c.night.clone(),
                control: Vec4::new(STRENGTH, night, 0.0, 0.0),
            });
        match next {
            Some(next) if old != Some(&next) => {
                commands.entity(entity).insert(next);
            }
            None if old.is_some() => {
                commands.entity(entity).remove::<GradeView>();
            }
            _ => {}
        }
    }
}

#[derive(Resource)]
struct GradeShader(Handle<Shader>);

#[derive(Resource)]
struct GradePipeline {
    layout: BindGroupLayoutDescriptor,
    scene_sampler: Sampler,
    lut_sampler: Sampler,
    pipeline: CachedRenderPipelineId,
}

fn init_pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    shader: Res<GradeShader>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "redux_grading_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_3d(TextureSampleType::Float { filterable: true }),
                texture_3d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer_sized(false, None),
            ),
        ),
    );
    let pipeline = post::pipeline(
        &cache,
        &fullscreen,
        "redux_grading",
        &layout,
        &shader.0,
        "fragment",
        post::SCENE_FORMAT,
    );
    commands.insert_resource(GradePipeline {
        layout,
        scene_sampler: post::clamped_sampler(&device, FilterMode::Nearest),
        lut_sampler: post::clamped_sampler(&device, FilterMode::Linear),
        pipeline,
    });
}

fn prepare_uniforms(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    views: Query<(Entity, &GradeView, Option<&ViewUniform<GradeView>>)>,
) {
    for (entity, grade, buffer) in &views {
        let rows = [grade.control.to_array()];
        if let Some(new) = post::write_uniform(&device, &queue, buffer, "redux_grading", &rows) {
            commands.entity(entity).insert(new);
        }
    }
}

#[derive(Default)]
struct GradeNode;

impl ViewNode for GradeNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static GradeView,
        &'static ViewUniform<GradeView>,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        context: &mut RenderContext<'w>,
        (target, grade, uniform): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let p = world.resource::<GradePipeline>();
        let cache = world.resource::<PipelineCache>();
        let Some(pipeline) = cache.get_render_pipeline(p.pipeline) else {
            return Ok(());
        };
        let images = world.resource::<RenderAssets<GpuImage>>();
        let (Some(day), Some(night)) = (images.get(&grade.day), images.get(&grade.night)) else {
            return Ok(());
        };
        let out = target.post_process_write();
        let bind = context.render_device().create_bind_group(
            "redux_grading",
            &cache.get_bind_group_layout(&p.layout),
            &BindGroupEntries::sequential((
                out.source,
                &p.scene_sampler,
                &day.texture_view,
                &night.texture_view,
                &p.lut_sampler,
                uniform.0.as_entire_binding(),
            )),
        );
        post::draw(
            context.command_encoder(),
            "redux_grading",
            out.destination,
            pipeline,
            &bind,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cubes_keep_black_white_and_grey_in_place() {
        for g in [DAY, NIGHT] {
            let at = |c: [f32; 3]| g.apply(c);
            // Black and white stay near black and white, so the grade never crushes or clips.
            assert!(at([0.0; 3]).iter().all(|v| *v < 0.03), "{g:?}");
            assert!(at([1.0; 3]).iter().all(|v| *v > 0.95), "{g:?}");
            let grey = at([0.5; 3]);
            assert!(grey.iter().all(|v| (v - 0.5).abs() < 0.04), "{g:?}");
        }
        assert_eq!(DAY.cube().len(), 32 * 32 * 32 * 4);
    }

    #[test]
    fn day_and_night_follow_the_clock() {
        assert_eq!(night_weight(0.0), 1.0);
        assert_eq!(night_weight(720.0), 0.0);
        assert_eq!(night_weight(1439.0), 1.0);
        assert!((night_weight(390.0) - 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn the_shader_validates() {
        post::validate_wgsl(include_str!("grading.wgsl"));
    }
}
