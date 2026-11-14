// The world view's motion vectors for temporal anti-aliasing (LOOK-15): every pixel's screen
// motion since the last frame, reprojected from the depth the main pass left (opaque and
// depth-writing transparent surfaces alike) through this frame's and the last frame's camera.
// Static content is exact; content that moved on its own (units, swaying leaves) is given the
// camera's motion and left to the resolve's neighbourhood clamp.
//
// Bevy's convention: uv now minus uv then, in [0, 1] uv units with y down, without the jitter.
#import bevy_render::view::View

@group(0) @binding(0) var depth: texture_depth_2d;
@group(0) @binding(1) var<uniform> view: View;

struct Motion {
    // This frame's unjittered clip_from_view.
    now: mat4x4<f32>,
    // The last frame's unjittered clip from this frame's view space: camera-relative, built in
    // f64 on the CPU from the two rotations and the camera's step, never from absolute positions.
    then: mat4x4<f32>,
}
@group(0) @binding(2) var<uniform> motion: Motion;

@fragment
fn fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec2<f32> {
    let z = textureLoad(depth, vec2<i32>(pixel.xy), 0);
    // The main pass's rectangle: smaller than the target under an upscaler.
    let uv = (pixel.xy - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let ndc = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), z, 1.0);
    // Homogeneous view space through the jittered inverse the depth was drawn with. The sky
    // (reverse-Z depth 0, infinitely far) comes out as a direction (w = 0), which takes only the
    // rotation between the frames.
    let p = view.view_from_clip * ndc;
    let now = motion.now * p;
    let then = motion.then * p;
    // A point behind either camera has no screen motion to give.
    if (now.w <= 1e-6 || then.w <= 1e-6) {
        return vec2<f32>(0.0);
    }
    return (now.xy / now.w - then.xy / then.w) * vec2<f32>(0.5, -0.5);
}
