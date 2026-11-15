// The scene behind the water (`water.rs`): after the opaque pass (and the ambient occlusion that
// darkens it), before the transparent pass the water draws in, one full-screen pass copies the
// resolved scene colour and the planar eye depth into the water's own texture: rgb = colour,
// a = depth in yd, 1e6 where nothing was drawn (the sky draws later, in the transparent pass).
// Under MSAA the opaque pass has resolved its colour into the single-sample main texture already;
// the depth is read at sample 0, as the ambient occlusion reads it.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import bevy_render::view::View

#ifdef MULTISAMPLED
@group(0) @binding(0) var depth_tex: texture_depth_multisampled_2d;
#else
@group(0) @binding(0) var depth_tex: texture_depth_2d;
#endif
@group(0) @binding(1) var color_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> view: View;

@fragment
fn copy(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.position.xy);
    let rgb = textureLoad(color_tex, px, 0).rgb;
#ifdef MULTISAMPLED
    let d = textureLoad(depth_tex, px, 0);
#else
    let d = textureLoad(depth_tex, px, 0);
#endif
    // Reverse-Z with an infinite far plane: planar eye depth = near / ndc_z.
    var z = 1e6;
    if d > 0.0 {
        z = min(view.clip_from_view[3][2] / d, 1e6);
    }
    return vec4<f32>(rgb, z);
}
