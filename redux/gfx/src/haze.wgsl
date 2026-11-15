// The distance haze (LOOK-12), ported from the everwood graphics fork of Benilla
// (`benilla-app/src/volumetric_fog.rs`, its `FOG_SHADER`): the analytic haze only. Everwood's
// shadow-mapped sun shafts and lamp halos are not ported here (the shafts are `shafts.wgsl`).
// Gamma space, after the world's transparencies and before the FFX combine decodes the frame.
//
// The ground mist (LOOK-14) rides the same pass: an exponential height fog, densest at its base
// height and thinning upward, whose optical depth along the view ray has a closed form, so it
// costs a few instructions and no march.
#import bevy_render::view::View
#import redux::gfx_fog
#import redux::gfx_fire

@group(0) @binding(0) var scene: texture_2d<f32>;
#ifdef MULTISAMPLED
@group(0) @binding(1) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(1) var depth: texture_depth_2d;
#endif
@group(0) @binding(2) var<uniform> view: View;

struct Haze {
    // rgb = the scene fog colour (gamma), w = extinction per yard.
    color_density: vec4<f32>,
    // The modern fog's sun-fog, end-fog and selector rows (`GfxData` rows `FOG_B`, `FOG_C`, `FOG_D`).
    sun_row: vec4<f32>,
    end_row: vec4<f32>,
    sel_row: vec4<f32>,
    // The ground mist (`GfxData` row `MIST`): density per yard at the base, base height (world
    // Y), falloff per yard upward, onset reach (yd). Density 0 is no mist.
    mist: vec4<f32>,
    // The sunlit air (LOOK-16, `scatter.rs`): xyz = toward the visible sun, w = scattering density
    // per yard (0 = off); rgb = glow colour with its gain, w = the reach (yd); the phase lobes
    // (core g, broad g, core share).
    air_dir: vec4<f32>,
    air_colour: vec4<f32>,
    air_phase: vec4<f32>,
    // Firelight (LOOK-16, `firelight.rs`): its `GfxData` `MISC` row (y = live lights, z = gain,
    // w = flicker clock) and its nearest lights, (world xyz, packed colour), nearest first.
    fire_misc: vec4<f32>,
    fires: array<vec4<f32>, 4>,
}
@group(0) @binding(3) var<uniform> haze: Haze;

// Integral of smoothstep(10, 30, distance): no haze in the first ten yards, continuous density
// and derivative at both ends, bounded at 150 yd.
fn fog_path(distance: f32) -> f32 {
    let d = clamp(distance, 10.0, 150.0);
    let u = clamp((d - 10.0) / 20.0, 0.0, 1.0);
    return 20.0 * (u * u * u - 0.5 * u * u * u * u) + max(d - 30.0, 0.0);
}

// The mist's opacity along the ray from the eye (height `eye_y`) to a point `d` yards away and
// `rise` yards above it: density a·exp(−k·(y − base)) integrated in closed form, faded in over the
// first `reach` yards so the ground at the player's feet stays clear.
fn mist_opacity(eye_y: f32, rise: f32, d: f32) -> f32 {
    let a = haze.mist.x;
    if (a <= 0.0) {
        return 0.0;
    }
    let k = haze.mist.z;
    let at_eye = a * exp(-k * clamp(eye_y - haze.mist.y, -4.0, 200.0));
    let s = k * rise;
    // (1 − e^−s) / s, its limit 1 at s = 0.
    let along = select((1.0 - exp(-s)) / s, 1.0 - 0.5 * s, abs(s) < 1e-3);
    let depth = min(at_eye * d * along, 8.0);
    let onset = smoothstep(0.5 * haze.mist.w, 2.0 * haze.mist.w, d);
    return (1.0 - exp(-depth)) * onset;
}

// Henyey-Greenstein's phase function for anisotropy `g` at `cosine` (per steradian).
fn hg(cosine: f32, g: f32) -> f32 {
    let denom = 1.0 + g * g - 2.0 * g * cosine;
    return 0.07957747 * (1.0 - g * g) / (denom * sqrt(denom));
}

// The sunlit air's in-scatter along unit `ray` over `d` yards: the two-lobe forward phase times
// the scattered share of the light over that path (none in the first five yards), in the glow's
// colour. The sky passes the reach.
fn sunlit_air(ray: vec3<f32>, d: f32) -> vec3<f32> {
    let sigma = haze.air_dir.w;
    if (sigma <= 0.0) {
        return vec3<f32>(0.0);
    }
    let c = dot(ray, haze.air_dir.xyz);
    let phase = mix(hg(c, haze.air_phase.y), hg(c, haze.air_phase.x), haze.air_phase.z);
    let path = max(min(d, haze.air_colour.w) - 5.0, 0.0);
    return haze.air_colour.rgb * (phase * (1.0 - exp(-sigma * path)));
}

// The most of a pixel's headroom the sunlit air may fill: facing a low sun, the lit air would
// otherwise lift every leaf and wall to the same flat white. Deviation: no source value; tuned in
// stills against the golden-forest reference, which keeps the canopy's texture against the glow.
const AIR_SHOULDER: f32 = 0.45;

// How bright a fire's halo is, per unit of its light. Deviation: no source value (everwood's lamp
// halos are not ported); tuned in stills against the campfire reference, so a flame glows into
// the bloom without lighting the whole night air.
const HALO_GAIN: f32 = 0.05;
// How far round a fire its halo spreads (yd), a Gaussian's width.
const HALO_SPREAD: f32 = 2.5;

// The air's glow round the nearest fires along unit `ray` over `d` yards: the in-scatter of a
// point light along the segment, the integral of 1 / (h^2 + (t - t0)^2) in closed form, so what
// stands before a flame hides its glow and the glow behind it. Additive and unshouldered, so a
// flame's core goes past white into the bloom.
fn fire_halos(ray: vec3<f32>, d: f32) -> vec3<f32> {
    let count = min(u32(haze.fire_misc.y), 4u);
    var sum = vec3<f32>(0.0);
    for (var i = 0u; i < count; i = i + 1u) {
        let fire = haze.fires[i];
        // Camera-relative from two nearby absolute positions: exact to the f32 step.
        let c = fire.xyz - view.world_position;
        let t0 = dot(c, ray);
        let h2 = max(dot(c, c) - t0 * t0, 0.01);
        // Past three widths the Gaussian leaves under 0.02 % of the glow.
        if (h2 > 9.0 * HALO_SPREAD * HALO_SPREAD) {
            continue;
        }
        let h = sqrt(h2);
        let seen = (atan((min(d, 1.0e4) - t0) / h) + atan(t0 / h)) / h;
        let spread = exp(-h2 / (HALO_SPREAD * HALO_SPREAD));
        let rgb = gfx_fire::fire_rgb(fire.w);
        sum += rgb * (seen * spread * gfx_fire::fire_flicker_of(haze.fire_misc.w, fire.xyz, fire.w, rgb));
    }
    return sum * (HALO_GAIN * haze.fire_misc.z);
}

// `light` lifted over `rgb` on the soft screen curve the shafts use, with a shoulder: bright
// pixels saturate gently instead of clipping, and the lift fills at most `AIR_SHOULDER` of the
// headroom, so what the air glows over keeps its contrast.
fn lift(rgb: vec3<f32>, light: vec3<f32>) -> vec3<f32> {
    let fill = AIR_SHOULDER * (vec3<f32>(1.0) - exp(-light / AIR_SHOULDER));
    return rgb + max(vec3<f32>(0.0), vec3<f32>(1.0) - rgb) * fill;
}

@fragment
fn fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(pixel.xy);
    let source = textureLoad(scene, xy, 0);
    let z = textureLoad(depth, xy, 0);
    // View space, then rotated to world: the camera-relative offset, free of the absolute
    // position's f32 step far from the origin.
    // The main pass's rectangle: smaller than the target under an upscaler.
    let uv = (pixel.xy - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let ndc = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), max(z, 1e-6), 1.0);
    let v = view.view_from_clip * ndc;
    let view_pos = v.xyz / v.w;
    let delta = mat3x3<f32>(
        view.world_from_view[0].xyz,
        view.world_from_view[1].xyz,
        view.world_from_view[2].xyz,
    ) * view_pos;
    // The sky (reverse-Z depth 0) carries its own horizon fog and takes no haze, only the sunlit
    // air's whole path.
    if (z <= 0.0) {
        let sky_ray = normalize(delta);
        var sky = source.rgb;
        if (haze.air_dir.w > 0.0) {
            sky = lift(sky, sunlit_air(sky_ray, 1.0e9));
        }
        return vec4<f32>(sky + fire_halos(sky_ray, 1.0e9), source.a);
    }
    let d = length(delta);
    if (d <= 1e-3) {
        return source;
    }
    let ray = delta / d;
    let opacity = 1.0 - exp(-haze.color_density.w * fog_path(d));
    var haze_rgb = haze.color_density.rgb;
    if (gfx_fog::fog_is_modern(haze.sel_row)) {
        haze_rgb = gfx_fog::fog_modern_colour(
            haze.color_density.rgb, ray, d, haze.sun_row, haze.end_row, haze.sel_row,
        );
    }
    let mist = mist_opacity(view.world_position.y, delta.y, d);
    let air = sunlit_air(ray, d);
    let halos = fire_halos(ray, d);
    if (opacity <= 0.0 && mist <= 0.0 && haze.air_dir.w <= 0.0) {
        return vec4<f32>(source.rgb + halos, source.a);
    }
    var rgb = mix(source.rgb, haze_rgb, opacity);
    // The mist is the fog colour lifted toward white: low cloud, not smoke.
    rgb = mix(rgb, mix(haze.color_density.rgb, vec3<f32>(1.0), 0.35), mist);
    // A darker fog colour may change the hue, never the pixel's luminance: add neutral headroom
    // in proportion, so saturated channels cannot clip dark.
    let luma = vec3<f32>(0.2126, 0.7152, 0.0722);
    let missing = max(0.0, dot(source.rgb - rgb, luma));
    let headroom = max(vec3<f32>(0.0), vec3<f32>(1.0) - rgb);
    rgb += headroom * clamp(missing / max(dot(headroom, luma), 0.00001), 0.0, 1.0);
    return vec4<f32>(lift(rgb, air) + halos, source.a);
}
