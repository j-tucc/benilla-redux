//! What the full-screen passes share: a fullscreen-triangle pipeline on the HDR scene, clamped
//! samplers, a per-view uniform rewritten every frame, and one pass's encoding.
use bevy::{
    camera::MainPassResolutionOverride,
    core_pipeline::FullscreenShader,
    prelude::*,
    render::{
        render_resource::*,
        renderer::{RenderDevice, RenderQueue},
        view::ViewTarget,
    },
};

/// The fullscreen-triangle pipeline for `entry` of `shader`, writing `format` with no blend.
pub(super) fn pipeline(
    cache: &PipelineCache,
    fullscreen: &FullscreenShader,
    label: &'static str,
    layout: &BindGroupLayoutDescriptor,
    shader: &Handle<Shader>,
    entry: &'static str,
    format: TextureFormat,
) -> CachedRenderPipelineId {
    cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some(label.into()),
        layout: vec![layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: shader.clone(),
            shader_defs: vec![],
            entry_point: Some(entry.into()),
            targets: vec![Some(ColorTargetState {
                format,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
        }),
        ..default()
    })
}

/// The world views' scene format: the camera is HDR, so the passes before the FFX combine's clamp
/// read and write this.
pub(super) const SCENE_FORMAT: TextureFormat = ViewTarget::TEXTURE_FORMAT_HDR;

/// A clamp-to-edge sampler with `filter` in every direction.
pub(super) fn clamped_sampler(device: &RenderDevice, filter: FilterMode) -> Sampler {
    device.create_sampler(&SamplerDescriptor {
        min_filter: filter,
        mag_filter: filter,
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        address_mode_w: AddressMode::ClampToEdge,
        ..default()
    })
}

/// The native-endian bytes of `rows`, as a uniform buffer holds them.
pub(super) fn bytes(rows: &[[f32; 4]]) -> Vec<u8> {
    rows.iter()
        .flatten()
        .flat_map(|v| v.to_ne_bytes())
        .collect()
}

/// A pass's per-view uniform, created once and rewritten in place each frame. `P` is the pass:
/// passes on the same view each keep their own buffer, since their prepare systems run in no set
/// order and one shared buffer would carry whichever wrote last.
#[derive(Component)]
pub(super) struct ViewUniform<P: Send + Sync + 'static>(pub Buffer, std::marker::PhantomData<P>);

/// Writes `rows` into `buffer` if there is one, else returns a new buffer holding them.
pub(super) fn write_uniform<P: Send + Sync + 'static>(
    device: &RenderDevice,
    queue: &RenderQueue,
    buffer: Option<&ViewUniform<P>>,
    label: &'static str,
    rows: &[[f32; 4]],
) -> Option<ViewUniform<P>> {
    let data = bytes(rows);
    match buffer {
        Some(b) => {
            queue.write_buffer(&b.0, 0, &data);
            None
        }
        None => Some(ViewUniform(
            device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some(label),
                contents: &data,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            }),
            std::marker::PhantomData,
        )),
    }
}

/// One fullscreen draw of `pipeline` with `bind` into `target`.
pub(super) fn draw(
    encoder: &mut CommandEncoder,
    label: &'static str,
    target: &TextureView,
    pipeline: &RenderPipeline,
    bind: &BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: Operations::default(),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind, &[]);
    pass.draw(0..3, 0..1);
}

/// A view's depth target and the main-pass override an upscaler may set on it: what a pass sizes
/// its intermediate targets from ([`main_pass_size`]).
pub(super) type DepthAndOverride<'a> = (
    &'a bevy::render::view::ViewDepthTexture,
    Option<&'a MainPassResolutionOverride>,
);

/// The size of the main pass's rectangle in a target of `full`: the override an upscaler sets
/// (`MainPassResolutionOverride`, the render size), else the whole target.
pub(super) fn main_pass_size(
    full: Extent3d,
    resolution: Option<&MainPassResolutionOverride>,
) -> Extent3d {
    match resolution {
        Some(r) => Extent3d {
            width: r.0.x.clamp(1, full.width.max(1)),
            height: r.0.y.clamp(1, full.height.max(1)),
            depth_or_array_layers: full.depth_or_array_layers,
        },
        None => full,
    }
}

/// Smoothstep from `a` to `b`.
pub(super) fn smoothstep(a: f32, b: f32, v: f32) -> f32 {
    let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Validates a pass's WGSL with the naga Bevy compiles it with, for the passes' tests.
#[cfg(test)]
pub(super) fn validate_wgsl(src: &str) {
    let module = naga::front::wgsl::parse_str(src).expect("the WGSL parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("the WGSL validates");
}
