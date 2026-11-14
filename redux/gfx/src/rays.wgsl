// Sun rays (LOOK-16): screen-space light scattering from the sun disc, four stages chosen by entry
// point (see `rays.rs`):
//   mask         quarter resolution: the sky seen near the sun, weighted by its brightness and its
//                closeness to the sun; anything that writes depth (leaves, trunks, roofs) is dark.
//   blur_coarse  quarter resolution: 16 taps along the line toward the sun across the whole span,
//                each further one weaker (the decay).
//   blur_fine    quarter resolution: 16 taps over one coarse step, filling between them, so the
//                two passes sum 256 evenly spaced taps with no noise and no history.
//   composite    full resolution: the rays, coloured, lifted over the scene.
//
// The method is Mitchell's "Volumetric Light Scattering as a Post-Process" (GPU Gems 3, ch. 13):
// a radial blur of an occlusion mask toward the light's screen position, here split into two
// passes of 16 taps.

#import bevy_render::view::View

struct Rays {
    // xy = the sun's position in the viewport (uv, y down), z = the mask's radius (in viewport
    // heights), w = the share of the way to the sun the coarse blur spans.
    sun: vec4<f32>,
    // rgb = the rays' colour with the gain and the fades folded in, w = the decay per coarse tap.
    colour: vec4<f32>,
}

const TAPS: i32 = 16;
// How much of the rays the open sky gives up.
const SKY_DIM: f32 = 0.85;
// The most of a pixel's headroom the rays may fill, so a canopy before the sun keeps its leaves'
// texture and silhouettes. Deviation: no source value; tuned in stills against the golden-forest
// reference.
const SHOULDER: f32 = 0.35;

// The main pass's aspect, width over height. Every stage works in the main pass's rectangle,
// which an upscaler shrinks inside the full-size targets (`main_pass_viewport`).
fn aspect(view: View) -> f32 {
    return view.main_pass_viewport.z / max(view.main_pass_viewport.w, 1.0);
}

#ifdef RAYS_MASK
@group(0) @binding(0) var scene: texture_2d<f32>;
#ifdef MULTISAMPLED
@group(0) @binding(1) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(1) var depth: texture_depth_2d;
#endif
@group(0) @binding(2) var<uniform> view: View;
@group(0) @binding(3) var<uniform> rays: Rays;

@fragment
fn mask(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = min(vec2<i32>(textureDimensions(depth)), vec2<i32>(view.main_pass_viewport.zw));
    // Four of the sixteen full-resolution pixels under this texel: the sky's share of them, and
    // its mean brightness.
    let base = vec2<i32>(frag.xy) * 4;
    var sky = 0.0;
    var bright = 0.0;
    for (var j = 0; j < 2; j += 1) {
        for (var i = 0; i < 2; i += 1) {
            let p = min(base + vec2<i32>(1 + 2 * i, 1 + 2 * j), dims - vec2<i32>(1));
            if (textureLoad(depth, p, 0) <= 0.0) {
                let c = textureLoad(scene, p, 0).rgb;
                sky += 0.25;
                bright += 0.25 * dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
            }
        }
    }
    if (sky <= 0.0) {
        return vec4<f32>(0.0);
    }
    let uv = (frag.xy * 4.0 - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let offset = (uv - rays.sun.xy) * vec2<f32>(aspect(view), 1.0);
    let near = exp(-dot(offset, offset) / (rays.sun.z * rays.sun.z));
    // Cloud or a dark sky before the sun gives weaker rays; a bright sky the full measure.
    let lit = 0.35 + 0.65 * smoothstep(0.15, 0.75, bright / sky);
    return vec4<f32>(sky * lit * near, sky, 0.0, 1.0);
}
#endif

#ifdef RAYS_BLUR
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear: sampler;
@group(0) @binding(2) var<uniform> rays: Rays;

// `TAPS` taps from `uv` toward the sun over `span` of the way, weighted by `decay` per tap.
fn radial(uv: vec2<f32>, span: f32, decay: f32) -> f32 {
    let step = (rays.sun.xy - uv) * (span / f32(TAPS));
    var sum = 0.0;
    var weight = 1.0;
    var total = 0.0;
    for (var i = 0; i < TAPS; i += 1) {
        sum += textureSampleLevel(source, linear, uv + step * f32(i), 0.0).r * weight;
        total += weight;
        weight *= decay;
    }
    return sum / total;
}

// Both blurs carry the texel's own sky share in `.g` for the composite.
@fragment
fn blur_coarse(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag.xy / vec2<f32>(textureDimensions(source));
    let sky = textureSampleLevel(source, linear, uv, 0.0).g;
    return vec4<f32>(radial(uv, rays.sun.w, rays.colour.w), sky, 0.0, 1.0);
}

@fragment
fn blur_fine(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag.xy / vec2<f32>(textureDimensions(source));
    let sky = textureSampleLevel(source, linear, uv, 0.0).g;
    return vec4<f32>(radial(uv, rays.sun.w / f32(TAPS), 1.0), sky, 0.0, 1.0);
}
#endif

#ifdef RAYS_COMPOSITE
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var blurred: texture_2d<f32>;
@group(0) @binding(2) var linear: sampler;
@group(0) @binding(3) var<uniform> view: View;
@group(0) @binding(4) var<uniform> rays: Rays;

@fragment
fn composite(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let source = textureLoad(scene, vec2<i32>(frag.xy), 0);
    let uv = (frag.xy - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let r = textureSampleLevel(blurred, linear, uv, 0.0);
    // Rays read against what they cross; over the open sky they would only whiten the sun's own
    // glow, so the sky keeps a little of them.
    let light = rays.colour.rgb * (r.r * (1.0 - SKY_DIM * r.g));
    // The soft screen curve the haze and shafts use (bright pixels saturate gently), shouldered.
    let fill = SHOULDER * (vec3<f32>(1.0) - exp(-light / SHOULDER));
    let lifted = source.rgb + max(vec3<f32>(0.0), vec3<f32>(1.0) - source.rgb) * fill;
    return vec4<f32>(lifted, source.a);
}
#endif
