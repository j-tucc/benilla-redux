// The cloud dome. Its colour is CPU-side as in the reference: the kernel's `0x6cfb00` port builds
// the image the reference binds as its texture (`0x58ac70`). This stage applies the dome's rim
// fade (`0x6d0530`) and blends the raw gamma texels premultiplied. Depth is the far pin in
// `sky_vertex.wgsl`.

#import bevy_pbr::forward_io::VertexOutput
#import benilla::{gfx_data, gfx_hooks}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var cloud_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var cloud_samp: sampler;
// The shared light buffer, read only for the program's graphics data (`lighting::GfxData`), which
// follows the 21 header rows and the 512-row point table.
struct WowLight {
    _rows: array<vec4<f32>, 21>,
    _points: array<vec4<f32>, 512>,
    gfx: gfx_data::GfxData,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(90) var<storage, read> wow_light: WowLight;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // The program's cloud hook point (the stock hook returns the sample).
    let texel = gfx_hooks::gfx_cloud(
        textureSample(cloud_tex, cloud_samp, in.uv), cloud_tex, cloud_samp, in.uv, wow_light.gfx,
    );
    var a = texel.a;
#ifdef VERTEX_COLORS
    a *= in.color.a; // the dome's rim fade (ring alphas)
#endif
    // Premultiplied gamma blend; the RGB is already the reference's byte math.
    return vec4<f32>(texel.rgb * a, a);
}
