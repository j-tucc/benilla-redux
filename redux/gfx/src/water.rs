//! Forever water: WoW Forever's new water look on 1.12's liquid surfaces ([`water.wgsl`]). As
//! Blizzard showed it at BlizzCon 2026, its water has waves, depth absorption that runs from clear
//! to murky, reflections of the world, sun and moon that follow the time of day, and foam where
//! water meets land. This module draws the open water (ADT ocean, river and lake) and WMO exterior
//! water that way. Magma, slime and interior pools keep the stock shader.
//!
//! It hooks in from outside the engine. Every stock water surface spawns with the engine's
//! `LiquidMaterial`; while [`GraphicsPrefs::water`] is on, [`swap_materials`] moves the surfaces
//! onto this module's [`WaterMaterial`] (one per ocean/lake and renderer lane) and moves them back
//! when it goes off. The surfaces keep the stock draw slot: the transparent pass, at
//! [`WATER_BIAS`], with no depth write.
//!
//! - **Refraction and depth.** One full-screen pass, [`WaterCopyNode`], copies the scene behind
//!   the water into the water's own texture after the opaque pass and before the transparent one:
//!   the resolved colour, and the planar eye depth read at depth sample 0. Under MSAA the opaque
//!   pass has already resolved its colour, and the water still draws into the multisampled
//!   attachment, so MSAA keeps working. The shader reads this copy for refraction, Beer–Lambert
//!   absorption over the true water column, screen-space reflection, caustics on the bottom, shore
//!   foam, and a soft edge where the surface meets geometry.
//! - **Waves.** [`GraphicsPrefs::waves`]: Gerstner swell on ADT ocean and lake. The vertex stage
//!   displaces by the long waves. The per-pixel normal adds the short waves and three scrolling
//!   layers of a procedural, tileable ripple map ([`ripple_image`]); nothing from the install is
//!   used. The swell follows redux's wind when that effect is on, and fades to nothing at the
//!   shore (the authored depth `V` is 0 there).
//! - **Colour.** The zone's own `Light.dbc` water bands, which also tint the stock water: the deep
//!   band is the water's body, the shallow band tints the refraction. Both are lit by the stock
//!   ambient and diffuse, so night water darkens with the zone.
//! - **From below.** With the camera under the surface, the water shows Snell's window onto the
//!   world above and total internal reflection outside it, under the engine's own underwater fog.
use super::Enhanced;
use benilla_assets::materials::LiquidMaterial;
use benilla_world::{lighting::WowLighting, liquid::SubmergedEye, view::WorldCamera};
use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::{core_3d::graph::Core3d, core_3d::graph::Node3d, FullscreenShader},
    ecs::query::QueryItem,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    pbr::{
        ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
        MaterialPlugin,
    },
    platform::collections::HashMap,
    prelude::*,
    render::{
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_graph::{
            NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
        },
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue},
        texture::GpuImage,
        view::{ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
        Render, RenderApp, RenderStartup, RenderSystems,
    },
    shader::ShaderRef,
};

/// The stock water's sort rung (`benilla_world::sky_order::WATER_BIAS`, crate-private): the
/// reference draws water in a fixed slot between the far and near transparents, and the stock
/// liquid material carries it as a depth bias that sorts but never reaches the rasterizer.
const WATER_BIAS: f32 = -2.0e4;
/// The scene copy's format: colour and the planar eye depth in yd.
const COPY_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
/// The ripple map's side, texels.
const RIPPLE_SIZE: u32 = 256;
/// The ripple map's slope range, `water.wgsl`'s `SLOPE_RANGE`.
const SLOPE_RANGE: f32 = 2.0;

/// The water surfaces' material: the stock liquid's slot and blend, this module's shader.
pub type WaterMaterial = ExtendedMaterial<StandardMaterial, WaterExt>;

/// [`water.wgsl`]'s inputs. The uniforms pack into binding 102, in the WGSL struct's order.
#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct WaterExt {
    #[uniform(102)]
    pub params: WaterParams,
    /// The scene behind the water ([`WaterCopyNode`]).
    #[texture(103)]
    #[sampler(104)]
    pub scene: Handle<Image>,
    /// The procedural ripple map ([`ripple_image`]).
    #[texture(105)]
    #[sampler(106)]
    pub ripple: Handle<Image>,
    /// The engine's shared global light, as the stock liquid reads it.
    #[storage(90, read_only, buffer, visibility(vertex, fragment))]
    pub light_buf: Buffer,
    /// The frame's shared inputs ([`WaterShared`]), rewritten in place every frame.
    #[storage(107, read_only, buffer, visibility(vertex, fragment))]
    pub shared: Buffer,
}

/// `water.wgsl`'s `WaterParams`: the lane's own row, which changes only with the switches.
#[derive(ShaderType, Clone, Copy, Debug, Default, PartialEq)]
pub struct WaterParams {
    /// x = ocean; y = renderer (0 ADT, 1 WMO exterior); z = wave scale; w = debug view.
    pub lane: Vec4,
}

/// `water.wgsl`'s `WaterShared`, row for row: the sky stops (zenith, middle, horizon), the sun
/// (toward it, glint strength), its colour, the moon (toward it, glint strength) and the state
/// (copy valid, camera under water, refraction on, caustics on). Written into one storage buffer
/// from the render world each frame, as the engine writes its shared light, so no material
/// changes while the sky moves.
#[derive(Resource, Clone, ExtractResource)]
struct WaterShared {
    buffer: Buffer,
    rows: [[f32; 4]; SHARED_ROWS],
}

/// [`WaterShared`]'s rows.
const SHARED_ROWS: usize = 7;

impl MaterialExtension for WaterExt {
    fn vertex_shader() -> ShaderRef {
        "embedded://redux_gfx/water.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "embedded://redux_gfx/water.wgsl".into()
    }
    fn enable_shadows() -> bool {
        false
    }
    /// The sort rung stays out of the rasterizer, as the stock liquid keeps it. Unlike the stock
    /// liquid, the surface writes depth: redux's haze and mist passes read the depth after the
    /// main pass, and through depth-less water they fog the sea floor (or the empty sky behind
    /// deep water) instead of the surface. The near transparents that draw after the water then
    /// stop at its surface, which the refraction shows them through anyway.
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.bias.constant = 0;
            ds.depth_write_enabled = true;
        }
        Ok(())
    }
}

/// A surface moved off its stock material, which it gets back when the water goes off.
#[derive(Component)]
struct Swapped(Handle<LiquidMaterial>);

/// The lane a water material serves: ocean or not, ADT or WMO exterior.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Lane {
    ocean: bool,
    wmo: bool,
}

/// The water materials by lane, and the ripple map.
#[derive(Resource)]
struct WaterAssets {
    lanes: HashMap<Lane, Handle<WaterMaterial>>,
    ripple: Handle<Image>,
}

/// The scene copy's image, and whether the copy runs this frame.
#[derive(Resource, Clone, ExtractResource)]
struct WaterScene {
    image: Handle<Image>,
    size: UVec2,
    /// Frames the image has kept its current size: the copy is valid from the second.
    settled: u32,
}

/// A world view whose scene is copied for the water.
#[derive(Component, Clone, Copy, PartialEq, ExtractComponent)]
struct WaterView;

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct WaterCopyLabel;

pub(super) struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<WaterView>::default());
        if !app.is_plugin_added::<AssetPlugin>() {
            return;
        }
        bevy::asset::embedded_asset!(app, "water.wgsl");
        let copy_shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("water_copy.wgsl"),
                "redux_gfx/water_copy.wgsl",
            ));
        app.add_plugins(MaterialPlugin::<WaterMaterial>::default())
            .add_plugins(ExtractResourcePlugin::<WaterScene>::default())
            .add_plugins(ExtractResourcePlugin::<WaterShared>::default())
            .add_systems(Startup, setup)
            .add_systems(
                PostUpdate,
                (update_views, size_scene, swap_materials, update_params).chain(),
            );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(CopyShader(copy_shader))
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(
                Render,
                upload_shared.in_set(RenderSystems::PrepareResources),
            )
            .add_render_graph_node::<ViewNodeRunner<WaterCopyNode>>(Core3d, WaterCopyLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    Node3d::MainTransmissivePass,
                    WaterCopyLabel,
                    Node3d::MainTransparentPass,
                ),
            );
    }
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    device: Option<Res<RenderDevice>>,
) {
    let Some(device) = device else {
        return; // no renderer: no water
    };
    commands.insert_resource(WaterShared {
        buffer: device.create_buffer(&BufferDescriptor {
            label: Some("redux_water_shared"),
            size: (SHARED_ROWS * 16) as u64,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        rows: [[0.0; 4]; SHARED_ROWS],
    });
    let mut scene = Image::new_target_texture(1, 1, COPY_FORMAT, None);
    scene.sampler = ImageSampler::linear();
    // Bevy copies the old texture into the new one on a resize.
    scene.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    scene.asset_usage = RenderAssetUsages::RENDER_WORLD;
    commands.insert_resource(WaterScene {
        image: images.add(scene),
        size: UVec2::ONE,
        settled: 0,
    });
    commands.insert_resource(WaterAssets {
        lanes: HashMap::default(),
        ripple: images.add(ripple_image()),
    });
}

/// Render world: writes the frame's shared rows before any draw reads them.
fn upload_shared(queue: Res<RenderQueue>, shared: Option<Res<WaterShared>>) {
    if let Some(shared) = shared {
        queue.write_buffer(&shared.buffer, 0, &super::post::bytes(&shared.rows));
    }
}

/// Marks the active world camera for the copy, with a depth it can sample.
fn update_views(
    mut commands: Commands,
    enhanced: Res<Enhanced>,
    mut cameras: Query<(Entity, &Camera, &mut Camera3d, Option<&WaterView>), With<WorldCamera>>,
) {
    for (entity, camera, mut camera3d, old) in &mut cameras {
        if !(enhanced.water && camera.is_active) {
            if old.is_some() {
                commands.entity(entity).remove::<WaterView>();
            }
            continue;
        }
        let binding = TextureUsages::TEXTURE_BINDING.bits();
        if camera3d.depth_texture_usages.0 & binding == 0 {
            camera3d.depth_texture_usages.0 |= binding;
        }
        if old.is_none() {
            commands.entity(entity).insert(WaterView);
        }
    }
}

/// Keeps the scene copy the size of the world camera's target, which its main texture is.
fn size_scene(
    scene: Option<ResMut<WaterScene>>,
    mut images: ResMut<Assets<Image>>,
    cameras: Query<&Camera, (With<WorldCamera>, With<WaterView>)>,
) {
    let Some(mut scene) = scene else {
        return;
    };
    let Some(size) = cameras
        .iter()
        .filter(|c| c.is_active)
        .find_map(Camera::physical_target_size)
    else {
        return;
    };
    if size == scene.size {
        scene.settled = scene.settled.saturating_add(1);
        return;
    }
    if let Some(image) = images.get_mut(&scene.image) {
        image.resize(Extent3d {
            width: size.x.max(1),
            height: size.y.max(1),
            depth_or_array_layers: 1,
        });
    }
    scene.size = size;
    scene.settled = 0;
}

/// Stock liquid surfaces spawned this frame and not yet swapped.
type NewStock = (Without<Swapped>, Added<MeshMaterial3d<LiquidMaterial>>);

/// Moves the water surfaces onto [`WaterMaterial`] while the water is on, and back off.
#[allow(clippy::too_many_arguments)]
fn swap_materials(
    mut commands: Commands,
    enhanced: Res<Enhanced>,
    mut was_on: Local<bool>,
    all_stock: Query<(Entity, &MeshMaterial3d<LiquidMaterial>), Without<Swapped>>,
    new_stock: Query<(Entity, &MeshMaterial3d<LiquidMaterial>), NewStock>,
    swapped: Query<(Entity, &Swapped)>,
    stock: Res<Assets<LiquidMaterial>>,
    mut ours: ResMut<Assets<WaterMaterial>>,
    water: Option<ResMut<WaterAssets>>,
    scene: Option<Res<WaterScene>>,
    shared: Option<Res<WaterShared>>,
) {
    let (Some(mut water), Some(scene), Some(shared)) = (water, scene, shared) else {
        return;
    };
    let on = enhanced.water;
    let turned = on != *was_on;
    *was_on = on;
    if !on {
        if turned {
            for (entity, back) in &swapped {
                commands
                    .entity(entity)
                    .insert(MeshMaterial3d(back.0.clone()))
                    .remove::<(MeshMaterial3d<WaterMaterial>, Swapped)>();
            }
        }
        return;
    }
    let candidates: Vec<(Entity, Handle<LiquidMaterial>)> = if turned {
        all_stock.iter().map(|(e, m)| (e, m.0.clone())).collect()
    } else {
        new_stock.iter().map(|(e, m)| (e, m.0.clone())).collect()
    };
    for (entity, handle) in candidates {
        let Some(m) = stock.get(&handle) else {
            continue;
        };
        let ext = &m.extension;
        // Magma and slime (fullbright) and true interior pools keep the stock shader.
        if ext.kind.x > 0.5 || ext.path.x > 1.5 {
            continue;
        }
        let lane = Lane {
            ocean: ext.kind.y > 0.5,
            wmo: ext.path.x > 0.5,
        };
        let ripple = water.ripple.clone();
        let material = water
            .lanes
            .entry(lane)
            .or_insert_with(|| {
                ours.add(ExtendedMaterial {
                    base: StandardMaterial {
                        unlit: true,
                        alpha_mode: AlphaMode::Blend,
                        cull_mode: None,
                        double_sided: true,
                        depth_bias: WATER_BIAS,
                        ..default()
                    },
                    extension: WaterExt {
                        params: lane_params(lane, &enhanced),
                        scene: scene.image.clone(),
                        ripple,
                        light_buf: ext.light_buf.clone(),
                        shared: shared.buffer.clone(),
                    },
                })
            })
            .clone();
        debug!("redux water: surface {entity} on lane {lane:?}");
        commands
            .entity(entity)
            .insert((MeshMaterial3d(material), Swapped(handle)))
            .remove::<MeshMaterial3d<LiquidMaterial>>();
    }
}

/// A lane's own row.
fn lane_params(lane: Lane, enhanced: &Enhanced) -> WaterParams {
    let waves = enhanced.waves && !lane.wmo;
    WaterParams {
        lane: Vec4::new(
            if lane.ocean { 1.0 } else { 0.0 },
            if lane.wmo { 1.0 } else { 0.0 },
            if waves { 1.0 } else { 0.0 },
            debug_view(),
        ),
    }
}

/// The frame's sky, sun, moon and state into [`WaterShared`]; the lanes' materials change only
/// when their bindings must be made again (a resized scene copy) or the waves switch flips.
#[allow(clippy::too_many_arguments)]
fn update_params(
    enhanced: Res<Enhanced>,
    lighting: Option<Res<WowLighting>>,
    eye: Option<Res<SubmergedEye>>,
    scene: Option<Res<WaterScene>>,
    water: Option<Res<WaterAssets>>,
    shared: Option<ResMut<WaterShared>>,
    mut ours: ResMut<Assets<WaterMaterial>>,
    mut last: Local<(UVec2, bool, bool)>,
) {
    let (Some(scene), Some(water), Some(mut shared)) = (scene, water, shared) else {
        return;
    };
    if !enhanced.water {
        return;
    }
    let l = lighting.as_deref().copied().unwrap_or_default();
    let sky = l.sky();
    let row3 = |c: [f32; 3]| [c[0], c[1], c[2], 1.0];
    let sun_dir = l.celestial_dir().normalize_or(Vec3::Y);
    let storm = l.storm_bcc();
    let sun_w = smoothstep(-0.03, 0.12, sun_dir.y) * (1.0 - 0.85 * storm);
    let moon_dir = l.moon_dir().normalize_or(Vec3::Y);
    let moon_w = smoothstep(0.0, 0.15, moon_dir.y) * (1.0 - sun_w) * (1.0 - 0.85 * storm) * 0.7;
    let valid = scene.settled >= 1;
    let flag = |b: bool| if b { 1.0 } else { 0.0 };
    let rows = [
        row3(sky[0]),
        row3(sky[2]),
        row3(sky[4]),
        sun_dir.extend(sun_w).to_array(),
        row3(l.celestial_tint()),
        moon_dir.extend(moon_w).to_array(),
        [
            flag(valid),
            flag(eye.is_some_and(|e| e.depth > 0.0)),
            1.0,
            1.0,
        ],
    ];
    if shared.rows != rows {
        shared.rows = rows;
    }
    // A resized copy is a new texture: the lanes' bind groups must be made again.
    let state = (scene.size, valid, enhanced.waves);
    if *last == state {
        return;
    }
    *last = state;
    debug!("redux water: scene copy {:?}, valid {valid}", scene.size);
    for (lane, handle) in &water.lanes {
        if let Some(m) = ours.get_mut(handle) {
            m.extension.params = lane_params(*lane, &enhanced);
        }
    }
}

/// `REDUX_WATER_DEBUG=<n>`: paint a water input instead of the water (1 = the copied depth,
/// 2 = the column thickness, 3 = the copied colour). Read once.
fn debug_view() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("REDUX_WATER_DEBUG")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    })
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The ripple map: a tileable height field of 72 waves on whole-cycle wave vectors (Phillips-like
/// falloff, fixed seed), stored as slope x/z (rg, ±[`SLOPE_RANGE`] over 0..1), height (b) and the
/// caustic focus of light refracted through it (a, from the field's curvature), with a full mip
/// chain. Procedural: nothing from the install.
fn ripple_image() -> Image {
    let n = RIPPLE_SIZE as usize;
    let mut rng = 0x9e37_79b9_u32;
    let mut rand = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        rng as f32 / u32::MAX as f32
    };
    // (kx, ky, amplitude, phase), whole cycles per tile so the field wraps.
    let mut waves = Vec::new();
    while waves.len() < 72 {
        let kx = (rand() * 40.0 - 20.0).round();
        let ky = (rand() * 40.0 - 20.0).round();
        let k = (kx * kx + ky * ky).sqrt();
        if !(2.0..=20.0).contains(&k) {
            continue;
        }
        waves.push((kx, ky, k.powf(-1.6), rand() * std::f32::consts::TAU));
    }
    let tau = std::f32::consts::TAU;
    let mut h = vec![0f32; n * n];
    let mut sx = vec![0f32; n * n];
    let mut sz = vec![0f32; n * n];
    let mut lap = vec![0f32; n * n];
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
            let i = y * n + x;
            for &(kx, ky, a, ph) in &waves {
                let arg = tau * (kx * u + ky * v) + ph;
                let (s, c) = arg.sin_cos();
                h[i] += a * c;
                sx[i] -= a * tau * kx * s;
                sz[i] -= a * tau * ky * s;
                lap[i] -= a * tau * tau * (kx * kx + ky * ky) * c;
            }
        }
    }
    let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
    let slope_rms = rms(&sx).max(rms(&sz)).max(1e-6);
    let lap_rms = rms(&lap).max(1e-6);
    let (hmin, hmax) = h
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    let unorm = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut base = Vec::with_capacity(n * n * 4);
    for i in 0..n * n {
        // A slope RMS of 0.5 before the shader's per-layer weights.
        let s = 0.5 / slope_rms;
        let focus = 1.0 / (1.0 + 0.6 * lap[i] / lap_rms).max(0.12);
        base.extend_from_slice(&[
            unorm(sx[i] * s / SLOPE_RANGE * 0.5 + 0.5),
            unorm(sz[i] * s / SLOPE_RANGE * 0.5 + 0.5),
            unorm((h[i] - hmin) / (hmax - hmin).max(1e-6)),
            unorm((focus - 0.8) / 3.0),
        ]);
    }
    let mut data = base.clone();
    let mut level = base;
    let mut side = n;
    let mut mips = 1;
    while side > 1 {
        let half = side / 2;
        let mut next = vec![0u8; half * half * 4];
        for y in 0..half {
            for x in 0..half {
                for c in 0..4 {
                    let at = |xx: usize, yy: usize| u32::from(level[(yy * side + xx) * 4 + c]);
                    let sum = at(2 * x, 2 * y)
                        + at(2 * x + 1, 2 * y)
                        + at(2 * x, 2 * y + 1)
                        + at(2 * x + 1, 2 * y + 1);
                    next[(y * half + x) * 4 + c] = sum.div_ceil(4) as u8;
                }
            }
        }
        data.extend_from_slice(&next);
        level = next;
        side = half;
        mips += 1;
    }
    let mut image = Image::new(
        Extent3d {
            width: RIPPLE_SIZE,
            height: RIPPLE_SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0; n * n * 4],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mips;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        label: Some("redux_water_ripple".into()),
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..default()
    });
    image
}

// ── The scene copy ──────────────────────────────────────────────────────────────────────────

#[derive(Resource)]
struct CopyShader(Handle<Shader>);

#[derive(Resource)]
struct CopyPipelines {
    /// By `samples > 1`.
    layouts: [BindGroupLayoutDescriptor; 2],
    pipelines: [CachedRenderPipelineId; 2],
}

fn init_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    shader: Res<CopyShader>,
    fullscreen: Res<FullscreenShader>,
) {
    let layouts = [false, true].map(|ms| {
        BindGroupLayoutDescriptor::new(
            "redux_water_copy",
            &BindGroupLayoutEntries::with_indices(
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
                ),
            ),
        )
    });
    let pipelines = [false, true].map(|ms| {
        let mut defs: Vec<bevy::shader::ShaderDefVal> = Vec::new();
        if ms {
            defs.push("MULTISAMPLED".into());
        }
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("redux_water_copy".into()),
            layout: vec![layouts[ms as usize].clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.0.clone(),
                shader_defs: defs,
                entry_point: Some("copy".into()),
                targets: vec![Some(ColorTargetState {
                    format: COPY_FORMAT,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            ..default()
        })
    });
    commands.insert_resource(CopyPipelines { layouts, pipelines });
}

/// Copies the opaque scene's colour and depth into [`WaterScene`]'s image.
#[derive(Default)]
struct WaterCopyNode;

impl ViewNode for WaterCopyNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static ViewDepthTexture,
        &'static ViewUniformOffset,
        &'static WaterView,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        context: &mut RenderContext<'w>,
        (target, depth, view_offset, _): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        if !depth
            .texture
            .usage()
            .contains(TextureUsages::TEXTURE_BINDING)
        {
            return Ok(());
        }
        let (Some(scene), Some(p), Some(images)) = (
            world.get_resource::<WaterScene>(),
            world.get_resource::<CopyPipelines>(),
            world.get_resource::<RenderAssets<GpuImage>>(),
        ) else {
            return Ok(());
        };
        let Some(out) = images.get(&scene.image) else {
            return Ok(());
        };
        // A frame where the copy and the main texture disagree in size (a resize) draws no copy.
        let main = target.main_texture().size();
        static SEEN: std::sync::Once = std::sync::Once::new();
        SEEN.call_once(|| {
            debug!(
                "redux water: scene copy {}x{}, main {}x{}, depth samples {}",
                out.size.width,
                out.size.height,
                main.width,
                main.height,
                depth.texture.sample_count()
            );
        });
        if out.size.width != main.width || out.size.height != main.height {
            return Ok(());
        }
        let ms = (depth.texture.sample_count() > 1) as usize;
        let cache = world.resource::<PipelineCache>();
        let Some(pipeline) = cache.get_render_pipeline(p.pipelines[ms]) else {
            return Ok(());
        };
        let Some(view) = world.resource::<ViewUniforms>().uniforms.binding() else {
            return Ok(());
        };
        let bind = context.render_device().create_bind_group(
            "redux_water_copy",
            &cache.get_bind_group_layout(&p.layouts[ms]),
            &BindGroupEntries::with_indices((
                (0, depth.view()),
                (1, target.main_texture_view()),
                (2, view),
            )),
        );
        let mut pass = context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("redux_water_copy"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &out.texture_view,
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
        pass.set_bind_group(0, &bind, &[view_offset.offset]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ripple_map_tiles_and_carries_a_full_mip_chain() {
        let image = ripple_image();
        let data = image.data.as_ref().unwrap();
        let n = RIPPLE_SIZE as usize;
        assert_eq!(image.texture_descriptor.mip_level_count, 9);
        assert_eq!(
            data.len(),
            (0..9).map(|m| (n >> m) * (n >> m) * 4).sum::<usize>()
        );
        // Whole-cycle wave vectors: the first column continues the last one.
        let at = |x: usize, y: usize, c: usize| i32::from(data[(y * n + x) * 4 + c]);
        for y in [0, 77, 200] {
            let step = (0..n - 1)
                .map(|x| (at(x + 1, y, 2) - at(x, y, 2)).abs())
                .max()
                .unwrap();
            assert!((at(0, y, 2) - at(n - 1, y, 2)).abs() <= step + 2, "row {y}");
        }
        // Slopes centre on 0.5 (no tilt).
        let mean: f32 = (0..n * n).map(|i| f32::from(data[i * 4])).sum::<f32>() / (n * n) as f32;
        assert!((mean - 127.5).abs() < 6.0, "mean slope {mean}");
    }

    /// The `#ifdef` lines of a pass shader, resolved for `defs`.
    fn preprocess(src: &str, defs: &[&str]) -> String {
        let mut live = vec![true];
        let mut out = String::new();
        for line in src.lines() {
            let t = line.trim();
            if let Some(def) = t.strip_prefix("#ifdef ") {
                let on = *live.last().unwrap() && defs.contains(&def.trim());
                live.push(on);
            } else if t == "#else" {
                let on = !live.pop().unwrap() && *live.last().unwrap();
                live.push(on);
            } else if t == "#endif" {
                live.pop();
            } else if *live.last().unwrap() {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    #[test]
    fn the_shaders_validate() {
        // Bevy's and the engine's imports, by stand-ins with the members the shaders read.
        let copy = include_str!("water_copy.wgsl")
            .replace(
                "#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput",
                "struct FullscreenVertexOutput { @builtin(position) position: vec4<f32>, \
                 @location(0) uv: vec2<f32> }",
            )
            .replace(
                "#import bevy_render::view::View",
                "struct View { clip_from_view: mat4x4<f32> }",
            );
        for defs in [&[][..], &["MULTISAMPLED"][..]] {
            super::super::post::validate_wgsl(&preprocess(&copy, defs));
        }
        let src = include_str!("water.wgsl");
        let start = src.find("#import bevy_pbr::{").unwrap();
        let end = src.find("#import benilla::{gfx_data, gfx_hooks}").unwrap();
        let body = format!("{}{}", &src[..start], &src[end..])
            .replace("#import benilla::{gfx_data, gfx_hooks}", STANDINS)
            .replace("#{MATERIAL_BIND_GROUP}", "2")
            .replace("mesh_functions::", "mesh_functions_")
            .replace("gfx_data::", "gfx_data_")
            .replace("gfx_hooks::", "gfx_hooks_");
        super::super::post::validate_wgsl(&body);
    }

    const STANDINS: &str = "
struct View { clip_from_world: mat4x4<f32>, view_from_world: mat4x4<f32>,
    world_position: vec3<f32>, viewport: vec4<f32>, mip_bias: f32 }
@group(0) @binding(0) var<uniform> view: View;
struct Globals { time: f32 }
@group(0) @binding(1) var<uniform> globals: Globals;
struct Vertex { @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) uv_b: vec2<f32> }
fn mesh_functions_get_world_from_local(i: u32) -> mat4x4<f32> { return mat4x4<f32>(); }
fn mesh_functions_mesh_position_local_to_world(m: mat4x4<f32>, p: vec4<f32>) -> vec4<f32> {
    return m * p;
}
fn position_world_to_clip(p: vec3<f32>) -> vec4<f32> {
    return view.clip_from_world * vec4<f32>(p, 1.0);
}
struct gfx_data_GfxData { rows: array<vec4<f32>, 64> }
fn gfx_hooks_gfx_fog(c: vec3<f32>, span: vec2<f32>, z: f32, p: vec3<f32>, cam: vec3<f32>,
    scene: bool, g: gfx_data_GfxData) -> vec4<f32> {
    return vec4<f32>(c, 1.0);
}
";

    #[test]
    fn the_params_match_the_shader_struct() {
        let src = include_str!("water.wgsl");
        for field in [
            "lane", "sky_hi", "sky_mid", "sky_lo", "sun", "sun_col", "moon", "state",
        ] {
            assert!(src.contains(&format!("    {field}: vec4<f32>,")), "{field}");
        }
        assert_eq!(WaterParams::min_size().get(), 16);
        let shared = &src[src.find("struct WaterShared").unwrap()..];
        let shared = &shared[..shared.find("};").unwrap()];
        assert_eq!(shared.matches(": vec4<f32>,").count(), SHARED_ROWS);
    }
}
