// Ported from WarcraftXL (https://github.com/WarcraftXL) by iThorgrim — module wxl-retail-grading, Grading.cpp, shaders/Grading.ps.hlsl.
// Used with the author's permission; attribution required. See THIRD-PARTY.md.
// Through the everwood graphics fork of Benilla (`benilla-app/src/post/grading.wgsl`): real 32³
// textures make the original 1024x32 strip's two bilinear taps plus lerp(fract(b*31)) one
// hardware-trilinear lookup, for both the day and night cubes. Elderoth keeps one world pair (no
// zone rows) and its LUTs are its own (`grading.rs`).

struct FullscreenVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var day_lut: texture_3d<f32>;
@group(0) @binding(3) var night_lut: texture_3d<f32>;
@group(0) @binding(4) var lut_sampler: sampler;

struct Grade {
    // x = strength, y = night blend.
    control: vec4<f32>,
};
@group(0) @binding(5) var<uniform> grade: Grade;

fn grade_one(lut: texture_3d<f32>, color: vec3<f32>) -> vec3<f32> {
    // Half a texel in from each face, with 31 intervals across the 32-texel cube.
    let uvw = color * (31.0 / 32.0) + vec3(0.5 / 32.0);
    return textureSampleLevel(lut, lut_sampler, uvw, 0.0).rgb;
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let source = textureSampleLevel(scene, scene_sampler, in.uv, 0.0);
    let base = clamp(source.rgb, vec3(0.0), vec3(1.0));
    let authored = mix(grade_one(day_lut, base), grade_one(night_lut, base), grade.control.y);
    let graded = mix(base, authored, grade.control.x);
    // Bloom already took the HDR excess; keep what is left of it for the FFX clamp.
    return vec4(graded + max(source.rgb - vec3(1.0), vec3(0.0)), source.a);
}
