//! The FFT ocean behind Forever water's waves ([`ocean.wgsl`]): Tessendorf's statistical wave
//! field, as WoW Forever's water moves. A fixed initial spectrum (Phillips, one prevailing wind)
//! is built once on the CPU; every frame the GPU advances each wave's phase and inverse-transforms
//! three cascades of [`N`]² into a displacement texture (`Dx, Dy, Dz, Dxz`) and a slope texture
//! (`dDy/dx, dDy/dz, dDx/dx, dDz/dz`), with a mip chain, which `water.wgsl` samples: the long
//! cascades move the surface's vertices, all three shape its normal, and the slopes' Jacobian marks
//! the breaking crests for foam.
//!
//! The wind is fixed per spectrum: changing it would mean a new spectrum, and every wave's phase
//! is a function of the world position, so even a small turn of a live wind would scramble the
//! whole sea. Every wave's angular frequency is a whole multiple of `2π /` [`PERIOD`], so the field
//! loops exactly and the clock can wrap without a seam.
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::{
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        graph::CameraDriverLabel,
        render_asset::RenderAssets,
        render_graph::{self, RenderGraph, RenderLabel},
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue},
        texture::GpuImage,
        Render, RenderApp, RenderStartup, RenderSystems,
    },
};

/// Each cascade's side, texels (`ocean.wgsl`'s `N`).
pub(super) const N: u32 = 256;
/// Cascades, long to short (`ocean.wgsl`'s `CASCADES`).
pub(super) const CASCADES: u32 = 3;
/// Each cascade's patch side, yd: not multiples of each other, so their tiles never line up.
pub(super) const LENGTHS: [f32; 3] = [311.0, 67.0, 17.0];
/// The field's loop, s: every wave's angular frequency is a whole multiple of `2π / PERIOD`.
const PERIOD: f64 = 300.0;
/// Mip levels of the two textures, 256 down to 8.
const MIPS: u32 = 6;
/// The prevailing wind on world XZ (Bevy), unit: the waves run with it.
pub(super) const WIND: [f32; 2] = [0.8, 0.6];
/// The wind speed, yd/s: Phillips' largest wave `U²/g` sets the swell's wavelength.
const WIND_SPEED: f64 = 9.0;
/// Gravity, yd/s².
const GRAVITY: f64 = 10.7;
/// The full field's significant wave height (`4σ` of the height), yd: the open ocean's swell.
/// The water scales it down by lane and shore depth.
pub(super) const SIGNIFICANT_HEIGHT: f64 = 2.0;
/// The displacement and slope textures' format: filterable, and writable from a compute pass.
const FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// The two textures the water samples.
#[derive(Resource, Clone, ExtractResource)]
pub(super) struct OceanImages {
    pub disp: Handle<Image>,
    pub slope: Handle<Image>,
}

/// The wave clock, into [`PERIOD`], and whether anything shows the ocean this frame.
#[derive(Resource, Clone, Copy, Default, ExtractResource)]
pub(super) struct OceanClock {
    pub time: f32,
    pub on: bool,
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct OceanLabel;

pub(super) struct OceanPlugin;

impl Plugin for OceanPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AssetPlugin>() {
            return;
        }
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("ocean.wgsl"),
                "redux_gfx/ocean.wgsl",
            ));
        app.init_resource::<OceanClock>()
            .add_plugins(ExtractResourcePlugin::<OceanImages>::default())
            .add_plugins(ExtractResourcePlugin::<OceanClock>::default())
            .add_systems(Startup, setup);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(OceanShader(shader))
            .add_systems(RenderStartup, init_ocean)
            .add_systems(Render, upload_clock.in_set(RenderSystems::PrepareResources));
        let mut graph = render_app.world_mut().resource_mut::<RenderGraph>();
        graph.add_node(OceanLabel, OceanNode);
        graph.add_node_edge(OceanLabel, CameraDriverLabel);
    }
}

/// One of the two textures: [`CASCADES`] layers of [`N`]², [`MIPS`] levels, written by the
/// compute pass and sampled, repeating, by the water.
fn field_image(label: &'static str) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: N,
            height: N,
            depth_or_array_layers: CASCADES,
        },
        TextureDimension::D2,
        FORMAT,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = MIPS;
    image.texture_descriptor.usage |= TextureUsages::STORAGE_BINDING;
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        label: Some(label),
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        label: Some(label.into()),
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

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(OceanImages {
        disp: images.add(field_image("redux_ocean_disp")),
        slope: images.add(field_image("redux_ocean_slope")),
    });
}

impl OceanClock {
    /// Main world: the clock, wrapped into the field's loop; `on` while any water shows it.
    pub(super) fn tick(&mut self, time: &Time, on: bool) {
        self.time = (time.elapsed_secs_f64() % PERIOD) as f32;
        self.on = on;
    }
}

/// The initial spectrum, [`CASCADES`] × [`N`]² of `(h0.re, h0.im, ω, 0)`, centred (index `N/2`
/// is wavenumber 0). Each cascade keeps its own band of wavenumbers, so together they add up to
/// one field; the whole is scaled to [`SIGNIFICANT_HEIGHT`]. Fixed seed: the same sea every run.
pub(super) fn initial_spectrum() -> Vec<[f32; 4]> {
    use std::f64::consts::{PI, TAU};
    let n = N as usize;
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut uniform = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        ((state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    let mut gauss = move || {
        let (u, v) = (uniform(), uniform());
        let r = (-2.0 * u.ln()).sqrt();
        (r * (TAU * v).cos(), r * (TAU * v).sin())
    };
    let wind = (f64::from(WIND[0]), f64::from(WIND[1]));
    let big = WIND_SPEED * WIND_SPEED / GRAVITY;
    let small = 0.04;
    let omega0 = TAU / PERIOD;
    // Band edges: a cascade hands its short waves to the next one six of its texels down.
    let edge = |c: usize| TAU * 6.0 / f64::from(LENGTHS[c]);
    let mut out = vec![[0f32; 4]; CASCADES as usize * n * n];
    let mut variance = 0.0;
    let mut amps = vec![(0f64, 0f64, 0f64); out.len()];
    for (c, &side) in LENGTHS.iter().enumerate() {
        let side = f64::from(side);
        let dk = TAU / side;
        let lo = if c == 0 { 0.0 } else { edge(c) };
        let hi = if c + 1 == CASCADES as usize {
            PI * n as f64 / side
        } else {
            edge(c + 1)
        };
        for y in 0..n {
            for x in 0..n {
                let (g1, g2) = gauss();
                let kx = dk * (x as f64 - n as f64 / 2.0);
                let kz = dk * (y as f64 - n as f64 / 2.0);
                let k = (kx * kx + kz * kz).sqrt();
                let i = (c * n + y) * n + x;
                if k < 1e-9 || k < lo || k >= hi {
                    continue;
                }
                let along = (kx * wind.0 + kz * wind.1) / k;
                // A broad spreading round the wind: Phillips' pure cos² lines the crests up into
                // long parallel ridges; a floor across the wind gives the short-crested, choppy
                // sea Forever shows. Waves against the wind keep a third of their energy, or the
                // two directions stand still where they meet.
                let spread = (0.25 + 0.75 * along * along) * if along < 0.0 { 0.35 } else { 1.0 };
                let phillips = (-1.0 / (k * big).powi(2)).exp() / k.powi(4)
                    * spread
                    * (-(k * small).powi(2)).exp();
                let a = phillips.sqrt() * dk / std::f64::consts::SQRT_2;
                let omega = ((GRAVITY * k).sqrt() / omega0).floor() * omega0;
                amps[i] = (g1 * a, g2 * a, omega);
                variance += 2.0 * (g1 * g1 + g2 * g2) * a * a;
            }
        }
    }
    let scale = (SIGNIFICANT_HEIGHT / 4.0) / variance.sqrt().max(1e-12);
    for (o, (re, im, omega)) in out.iter_mut().zip(amps) {
        *o = [(re * scale) as f32, (im * scale) as f32, omega as f32, 0.0];
    }
    out
}

// ── Render world ────────────────────────────────────────────────────────────────────────────

#[derive(Resource)]
struct OceanShader(Handle<Shader>);

#[derive(Resource)]
struct OceanGpu {
    main_layout: BindGroupLayoutDescriptor,
    mip_layout: BindGroupLayoutDescriptor,
    spectrum: CachedComputePipelineId,
    rows: CachedComputePipelineId,
    cols: CachedComputePipelineId,
    assemble: CachedComputePipelineId,
    downsample: CachedComputePipelineId,
    params: Buffer,
    h0: Buffer,
    fft: Buffer,
}

fn init_ocean(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    shader: Res<OceanShader>,
) {
    let main_layout = BindGroupLayoutDescriptor::new(
        "redux_ocean",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (0, uniform_buffer_sized(false, None)),
                (1, storage_buffer_read_only_sized(false, None)),
                (2, storage_buffer_sized(false, None)),
                (
                    3,
                    texture_storage_2d_array(FORMAT, StorageTextureAccess::WriteOnly),
                ),
                (
                    4,
                    texture_storage_2d_array(FORMAT, StorageTextureAccess::WriteOnly),
                ),
            ),
        ),
    );
    let mip_layout = BindGroupLayoutDescriptor::new(
        "redux_ocean_mip",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (
                    5,
                    texture_2d_array(TextureSampleType::Float { filterable: false }),
                ),
                (
                    6,
                    texture_2d_array(TextureSampleType::Float { filterable: false }),
                ),
                (
                    7,
                    texture_storage_2d_array(FORMAT, StorageTextureAccess::WriteOnly),
                ),
                (
                    8,
                    texture_storage_2d_array(FORMAT, StorageTextureAccess::WriteOnly),
                ),
            ),
        ),
    );
    let queue = |layout: &BindGroupLayoutDescriptor, entry: &'static str| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(format!("redux_ocean_{entry}").into()),
            layout: vec![layout.clone()],
            shader: shader.0.clone(),
            entry_point: Some(entry.into()),
            zero_initialize_workgroup_memory: false,
            ..default()
        })
    };
    let spectrum = queue(&main_layout, "spectrum");
    let rows = queue(&main_layout, "fft_rows");
    let cols = queue(&main_layout, "fft_cols");
    let assemble = queue(&main_layout, "assemble");
    let downsample = queue(&mip_layout, "downsample");
    let h0: Vec<u8> = initial_spectrum()
        .iter()
        .flatten()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let elements = u64::from(CASCADES * 2 * N * N);
    commands.insert_resource(OceanGpu {
        main_layout,
        mip_layout,
        spectrum,
        rows,
        cols,
        assemble,
        downsample,
        params: device.create_buffer(&BufferDescriptor {
            label: Some("redux_ocean_params"),
            size: 32,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        h0: device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("redux_ocean_h0"),
            contents: &h0,
            usage: BufferUsages::STORAGE,
        }),
        fft: device.create_buffer(&BufferDescriptor {
            label: Some("redux_ocean_fft"),
            size: elements * 16,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        }),
    });
}

fn upload_clock(
    queue: Res<RenderQueue>,
    gpu: Option<Res<OceanGpu>>,
    clock: Option<Res<OceanClock>>,
) {
    let (Some(gpu), Some(clock)) = (gpu, clock) else {
        return;
    };
    if !clock.on {
        return;
    }
    let rows = [
        [clock.time, 0.0, 0.0, 0.0],
        [LENGTHS[0], LENGTHS[1], LENGTHS[2], 0.0],
    ];
    queue.write_buffer(&gpu.params, 0, &super::post::bytes(&rows));
}

/// Rebuilds the wave field once a frame, before any camera draws.
struct OceanNode;

impl render_graph::Node for OceanNode {
    fn run<'w>(
        &self,
        _graph: &mut render_graph::RenderGraphContext,
        context: &mut RenderContext<'w>,
        world: &'w World,
    ) -> Result<(), render_graph::NodeRunError> {
        let (Some(gpu), Some(images), Some(clock), Some(gpu_images)) = (
            world.get_resource::<OceanGpu>(),
            world.get_resource::<OceanImages>(),
            world.get_resource::<OceanClock>(),
            world.get_resource::<RenderAssets<GpuImage>>(),
        ) else {
            return Ok(());
        };
        if !clock.on {
            return Ok(());
        }
        let (Some(disp), Some(slope)) =
            (gpu_images.get(&images.disp), gpu_images.get(&images.slope))
        else {
            return Ok(());
        };
        let cache = world.resource::<PipelineCache>();
        let ids = [
            gpu.spectrum,
            gpu.rows,
            gpu.cols,
            gpu.assemble,
            gpu.downsample,
        ];
        let Some(pipes) = ids
            .iter()
            .map(|id| cache.get_compute_pipeline(*id))
            .collect::<Option<Vec<_>>>()
        else {
            return Ok(());
        };
        let level = |texture: &Texture, mip: u32| {
            texture.create_view(&TextureViewDescriptor {
                label: Some("redux_ocean_level"),
                dimension: Some(TextureViewDimension::D2Array),
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..default()
            })
        };
        let disp_levels: Vec<_> = (0..MIPS).map(|m| level(&disp.texture, m)).collect();
        let slope_levels: Vec<_> = (0..MIPS).map(|m| level(&slope.texture, m)).collect();
        let device = context.render_device();
        let main = device.create_bind_group(
            "redux_ocean",
            &cache.get_bind_group_layout(&gpu.main_layout),
            &BindGroupEntries::with_indices((
                (0, gpu.params.as_entire_binding()),
                (1, gpu.h0.as_entire_binding()),
                (2, gpu.fft.as_entire_binding()),
                (3, &disp_levels[0]),
                (4, &slope_levels[0]),
            )),
        );
        let mip_layout = cache.get_bind_group_layout(&gpu.mip_layout);
        let mips: Vec<_> = (1..MIPS as usize)
            .map(|m| {
                device.create_bind_group(
                    "redux_ocean_mip",
                    &mip_layout,
                    &BindGroupEntries::with_indices((
                        (5, &disp_levels[m - 1]),
                        (6, &slope_levels[m - 1]),
                        (7, &disp_levels[m]),
                        (8, &slope_levels[m]),
                    )),
                )
            })
            .collect();
        let mut pass = context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("redux_ocean"),
                timestamp_writes: None,
            });
        pass.set_bind_group(0, &main, &[]);
        let tiles = N / 16;
        pass.set_pipeline(pipes[0]);
        pass.dispatch_workgroups(tiles, tiles, CASCADES);
        pass.set_pipeline(pipes[1]);
        pass.dispatch_workgroups(N, 2, CASCADES);
        pass.set_pipeline(pipes[2]);
        pass.dispatch_workgroups(N, 2, CASCADES);
        pass.set_pipeline(pipes[3]);
        pass.dispatch_workgroups(tiles, tiles, CASCADES);
        pass.set_pipeline(pipes[4]);
        for (m, bind) in mips.iter().enumerate() {
            let side = (N >> (m + 1)).max(1);
            pass.set_bind_group(0, bind, &[]);
            pass.dispatch_workgroups(side.div_ceil(8), side.div_ceil(8), CASCADES);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spectrum_is_scaled_banded_and_loops() {
        let h0 = initial_spectrum();
        let n = N as usize;
        assert_eq!(h0.len(), CASCADES as usize * n * n);
        // σ² = Σ 2|h0|² ⇒ Hs = 4σ.
        let var: f64 = h0
            .iter()
            .map(|v| 2.0 * (f64::from(v[0]).powi(2) + f64::from(v[1]).powi(2)))
            .sum();
        assert!(
            (4.0 * var.sqrt() - SIGNIFICANT_HEIGHT).abs() < 1e-3,
            "Hs {}",
            4.0 * var.sqrt()
        );
        // The DC term is empty, and every ω is a whole multiple of 2π / PERIOD.
        assert_eq!(h0[n / 2 * n + n / 2], [0.0; 4]);
        let omega0 = std::f64::consts::TAU / PERIOD;
        for v in h0.iter().filter(|v| v[2] > 0.0) {
            let m = f64::from(v[2]) / omega0;
            assert!((m - m.round()).abs() < 1e-3, "ω {}", v[2]);
        }
        // Each cascade carries energy (no empty band).
        for c in 0..CASCADES as usize {
            let e: f32 = h0[c * n * n..(c + 1) * n * n]
                .iter()
                .map(|v| v[0] * v[0] + v[1] * v[1])
                .sum();
            assert!(e > 0.0, "cascade {c}");
        }
    }

    #[test]
    fn the_shader_validates_and_matches() {
        let src = include_str!("ocean.wgsl");
        super::super::post::validate_wgsl(src);
        assert!(src.contains(&format!("const N: u32 = {N}u;")));
        assert!(src.contains(&format!("const LOG_N: u32 = {}u;", N.trailing_zeros())));
        assert!(src.contains(&format!("const CASCADES: u32 = {CASCADES}u;")));
    }
}
