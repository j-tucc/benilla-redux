// Forever water (`water.rs`): the open water and WMO exterior water surfaces, drawn in the stock
// liquid's place in the transparent pass. Rolling Gerstner waves (vertex displacement on ocean and
// lake, normals everywhere), three scrolling layers of a procedural ripple map, depth absorption
// and in-scatter coloured by the zone's own `Light.dbc` water bands, refraction of the scene
// copied before the water (`water_copy.wgsl`), screen-space reflection with a sky fallback, a
// fresnel blend, sun and moon glints, caustics on the refracted bottom, shore foam and a soft
// edge where the water meets geometry. Seen from below, the surface shows Snell's window.
//
// Colour math stays in benilla's gamma space, like the stock liquid shader and the scene copy.

#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{view, globals},
}
#import benilla::{gfx_data, gfx_hooks}

struct WaterParams {
    // x = ocean (the ocean bands, the larger swell); y = renderer: 0 ADT, 1 WMO exterior;
    // z = wave scale (0 = flat: WMO water, waves off); w = debug view (`REDUX_WATER_DEBUG`).
    lane: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var<uniform> w: WaterParams;

// The frame's shared inputs (`water.rs`, written every frame from the render world).
struct WaterShared {
    // The sky dome's stops, gamma: zenith, middle, horizon.
    sky_hi: vec4<f32>,
    sky_mid: vec4<f32>,
    sky_lo: vec4<f32>,
    // xyz = toward the visible sun; w = its glint strength (0 below the horizon).
    sun: vec4<f32>,
    // rgb = the sun's colour (the celestial tint); w = unused.
    sun_col: vec4<f32>,
    // xyz = toward the moon; w = its glint strength (0 by day).
    moon: vec4<f32>,
    // x = the scene copy is valid; y = the camera is under water; z = refraction and reflection
    // read the copy; w = caustics.
    state: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var<storage, read> sh: WaterShared;

// The scene behind the water (`water_copy.wgsl`): rgb = the opaque scene, a = planar eye depth
// (yd, 1e6 for the empty sky).
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var scene: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var scene_samp: sampler;
// The procedural ripple map (`water.rs`, tileable): rg = slope x/z in [-1, 1] as 0..1,
// b = height, a = caustic focus.
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var ripple: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var ripple_samp: sampler;

// The prefix of the shared global light (`lighting::global_light`), as `liquid.wgsl` declares it.
struct WowLight {
    light_ambient: vec4<f32>,
    light_diffuse: vec4<f32>,
    light_sun: vec4<f32>,
    light_spec: vec4<f32>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
    _sh: array<vec4<f32>, 6>,
    _sh_c16: vec4<f32>,
    water_river: array<vec4<f32>, 2>,
    water_ocean: array<vec4<f32>, 2>,
    _grade: vec4<f32>,
    wmo_fog_color: vec4<f32>,
    wmo_fog_params: vec4<f32>,
    _point_count: vec4<f32>,
    _points: array<vec4<f32>, 512>,
    gfx: gfx_data::GfxData,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(90) var<storage, read> wow_light: WowLight;

// redux's wind row (`rows.rs` WIND_A): xy = the wind's direction on world XZ, zero when off.
const WIND_A: u32 = 4u;
// The ripple map's slope range: rg = 0..1 covers [-SLOPE_RANGE, SLOPE_RANGE] (`water.rs`).
const SLOPE_RANGE: f32 = 2.0;
// Gravity in yd/s², for the deep-water dispersion ω = sqrt(g·k).
const GRAVITY: f32 = 10.7;
const PI: f32 = 3.14159265;

struct WaterVsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    // The undisplaced surface point; the waves' normals are evaluated there.
    @location(1) rest: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // x = the authored depth coordinate V; y = this vertex's wave amplitude scale.
    @location(3) depth: vec2<f32>,
}

fn time() -> f32 {
    return globals.time;
}

fn wind_dir() -> vec2<f32> {
    let d = wow_light.gfx.rows[WIND_A].xy;
    if dot(d, d) < 1e-4 {
        return vec2<f32>(0.8, 0.6);
    }
    return normalize(d);
}

fn rotate(d: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(c * d.x - s * d.y, s * d.x + c * d.y);
}

// One Gerstner wave: wavelength (yd), amplitude (yd), direction angle off the wind (rad).
struct Wave {
    len: f32,
    amp: f32,
    angle: f32,
}

// The swell: the first three displace vertices (an MCLQ vertex is 4.2 yd apart, so only these
// long waves sample cleanly), all five shape the per-pixel normal.
const WAVES: array<Wave, 5> = array<Wave, 5>(
    Wave(34.0, 0.38, 0.0),
    Wave(21.0, 0.24, 0.62),
    Wave(14.0, 0.13, -0.48),
    Wave(8.5, 0.06, 1.10),
    Wave(5.3, 0.03, -1.25),
);
const DISPLACED: u32 = 3u;
// Gerstner steepness: how much the crests sharpen (horizontal pull).
const STEEP: f32 = 0.55;

// The displacement of the rest point `p` (world XZ) at amplitude scale `s`.
fn swell_offset(p: vec2<f32>, s: f32) -> vec3<f32> {
    let wind = wind_dir();
    var off = vec3<f32>(0.0);
    for (var i = 0u; i < DISPLACED; i++) {
        let wv = WAVES[i];
        let k = 2.0 * PI / wv.len;
        let d = rotate(wind, wv.angle);
        let phase = k * dot(d, p) - sqrt(GRAVITY * k) * time();
        let q = STEEP / (k * f32(DISPLACED)) * s;
        off += vec3<f32>(d.x * q * cos(phase), wv.amp * s * sin(phase), d.y * q * cos(phase));
    }
    return off;
}

// The swell's surface slope (dy/dx, dy/dz) at `p`, every wave, scale `s`; `far` fades the short
// waves out with distance.
fn swell_slope(p: vec2<f32>, s: f32, far: f32) -> vec2<f32> {
    let wind = wind_dir();
    var g = vec2<f32>(0.0);
    for (var i = 0u; i < 5u; i++) {
        let wv = WAVES[i];
        let k = 2.0 * PI / wv.len;
        let d = rotate(wind, wv.angle);
        let phase = k * dot(d, p) - sqrt(GRAVITY * k) * time();
        let fade = select(1.0, far, i >= DISPLACED);
        g += d * (wv.amp * s * k * cos(phase) * fade);
    }
    return g;
}

@vertex
fn vertex(in: Vertex) -> WaterVsOut {
    var out: WaterVsOut;
    let world_from_local = mesh_functions::get_world_from_local(in.instance_index);
    let rest =
        mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(in.position, 1.0));
    let v = clamp(in.uv_b.x, 0.0, 1.0);
    // Amplitude: none at the shore (V = 0), full in open water. Ocean V is byte/255 and reaches
    // deep water late; river/lake V is byte/42, saturating near 5 yd.
    var s = 0.0;
    if w.lane.z > 0.0 {
        if w.lane.x > 0.5 {
            s = smoothstep(0.02, 0.35, v);
        } else {
            s = smoothstep(0.08, 0.9, v) * 0.7;
        }
        s *= w.lane.z;
    }
    out.rest = rest.xyz;
    out.world_position = vec4<f32>(rest.xyz + swell_offset(rest.xz, s), 1.0);
    out.clip_position = position_world_to_clip(out.world_position.xyz);
    out.uv = in.uv;
    out.depth = vec2<f32>(v, s);
    return out;
}

// The ripple map's slope at `uv` (tiles), weighted.
fn ripple_slope(uv: vec2<f32>, weight: f32) -> vec2<f32> {
    let t = textureSampleBias(ripple, ripple_samp, uv, view.mip_bias);
    return (t.rg * 2.0 - 1.0) * SLOPE_RANGE * weight;
}

// The sky dome's colour along unit `dir`, from the zone's stops; below the horizon, the horizon.
fn sky_along(dir: vec3<f32>) -> vec3<f32> {
    let e = clamp(dir.y, 0.0, 1.0);
    let a = asin(e) / (0.5 * PI);
    let lo = mix(sh.sky_lo.rgb, sh.sky_mid.rgb, smoothstep(0.0, 0.3, a));
    return mix(lo, sh.sky_hi.rgb, smoothstep(0.3, 1.0, a));
}

// GGX normal distribution, roughness `r`.
fn ggx(ndoth: f32, r: f32) -> f32 {
    let a2 = r * r * r * r;
    let d = ndoth * ndoth * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn fresnel(cos_t: f32) -> f32 {
    // Schlick from water's F0 = 0.02 (n = 1.33), lifted to 0.06 at normal incidence so the sky
    // still reads on water seen from above, as Forever's does.
    return 0.06 + 0.94 * pow(1.0 - clamp(cos_t, 0.0, 1.0), 5.0);
}

// The scene copy's texel coordinate of a world point, or a negative uv off screen.
fn scene_uv(p: vec3<f32>) -> vec3<f32> {
    let clip = view.clip_from_world * vec4<f32>(p, 1.0);
    if clip.w <= 0.0 {
        return vec3<f32>(-1.0);
    }
    let ndc = clip.xy / clip.w;
    let vp = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    let dims = vec2<f32>(textureDimensions(scene));
    let uv = (view.viewport.xy + vp * view.viewport.zw) / dims;
    let z = -(view.view_from_world * vec4<f32>(p, 1.0)).z;
    return vec3<f32>(uv, z);
}

fn in_viewport(uv: vec2<f32>) -> bool {
    let dims = vec2<f32>(textureDimensions(scene));
    let lo = view.viewport.xy / dims;
    let hi = (view.viewport.xy + view.viewport.zw) / dims;
    return all(uv > lo) && all(uv < hi);
}

// Screen-space reflection off the scene copy: rgb = the hit's colour, a = its weight (0 = none).
fn ssr(origin: vec3<f32>, dir: vec3<f32>, eye_dist: f32) -> vec4<f32> {
    var step = clamp(eye_dist * 0.02, 0.4, 4.0);
    var t = step;
    var prev_t = 0.0;
    for (var i = 0; i < 28; i++) {
        let p = origin + dir * t;
        let s = scene_uv(p);
        if s.x < 0.0 || !in_viewport(s.xy) {
            return vec4<f32>(0.0);
        }
        let scene_z = textureSampleLevel(scene, scene_samp, s.xy, 0.0).a;
        let behind = s.z - scene_z;
        if behind > 0.05 {
            // Refine between the last two steps.
            var lo = prev_t;
            var hi = t;
            for (var j = 0; j < 5; j++) {
                let mid = 0.5 * (lo + hi);
                let m = scene_uv(origin + dir * mid);
                let mz = textureSampleLevel(scene, scene_samp, m.xy, 0.0).a;
                if m.z > mz {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let h = scene_uv(origin + dir * hi);
            let hz = textureSampleLevel(scene, scene_samp, h.xy, 0.0).a;
            // A ray that passed behind a thin object rather than into it is no hit.
            if h.z - hz > max(1.5, step * 1.5) {
                return vec4<f32>(0.0);
            }
            let rgb = textureSampleLevel(scene, scene_samp, h.xy, 0.0).rgb;
            // Fade at the screen's edges and with the march's length.
            let dims = vec2<f32>(textureDimensions(scene));
            let lo_uv = view.viewport.xy / dims;
            let hi_uv = (view.viewport.xy + view.viewport.zw) / dims;
            let edge = min(min(h.x - lo_uv.x, hi_uv.x - h.x), min(h.y - lo_uv.y, hi_uv.y - h.y))
                / max(hi_uv.x - lo_uv.x, 1e-3);
            let fade = smoothstep(0.0, 0.08, edge) * (1.0 - smoothstep(18.0, 28.0, f32(i)));
            return vec4<f32>(rgb, fade);
        }
        prev_t = t;
        step *= 1.18;
        t += step;
    }
    return vec4<f32>(0.0);
}

// The scene fog at a surface point: rgb = fog colour, w = survival (1 = no fog).
fn fog_at(world_pos: vec3<f32>) -> vec4<f32> {
    let fog_color = wow_light.fog_color;
    if fog_color.w <= 0.5 {
        return vec4<f32>(fog_color.rgb, 1.0);
    }
    let eye_z = -(view.view_from_world * vec4<f32>(world_pos, 1.0)).z;
    return gfx_hooks::gfx_fog(
        fog_color.xyz, wow_light.fog_params.xy, eye_z, world_pos, view.world_position, true,
        wow_light.gfx,
    );
}

@fragment
fn fragment(in: WaterVsOut) -> @location(0) vec4<f32> {
    // The far-clip wall, as the stock liquid.
    let water_z = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
    if wow_light.fog_params.w > 0.0 && water_z > wow_light.fog_params.w {
        discard;
    }

    let cam = view.world_position;
    let to_eye_v = cam - in.world_position.xyz;
    let eye_dist = length(to_eye_v);
    let v = to_eye_v / max(eye_dist, 1e-4);
    let ocean = w.lane.x > 0.5;

    // ---- The normal: the swell's slope plus three ripple layers, scrolling across each other.
    let wind = wind_dir();
    let t = time();
    let p = in.rest.xz;
    let far = 1.0 - smoothstep(60.0, 220.0, eye_dist);
    var slope = swell_slope(p, in.depth.y, far);
    let calm = select(0.75, 1.0, ocean);
    slope += ripple_slope(p / 23.0 + wind * (t * 0.020), 0.26 * calm);
    slope += ripple_slope(rotate(p, 0.9) / 8.3 + rotate(wind, 0.7) * (t * 0.045), 0.10 * calm);
    slope += ripple_slope(rotate(p, -0.6) / 2.9 + rotate(wind, -0.9) * (t * 0.11), 0.05 * far);
    // Distance smooths the surface toward a mirror, as the ripples fall below a pixel.
    slope *= mix(0.45, 1.0, far);
    var n = normalize(vec3<f32>(-slope.x, 1.0, -slope.y));
    // Sampled here, in uniform control flow (implicit derivatives), for the shore foam below.
    let foam_noise = textureSampleBias(ripple, ripple_samp, p / 4.3 + wind * (t * 0.03), view.mip_bias).b;

    // From below, the surface faces down. Decided by the eye's height alone: the engine calls
    // the eye submerged (`sh.state.y`) as soon as the near plane's lowest corner dips, while the
    // eye itself can still be above a surface that fills the screen.
    let below = cam.y < in.world_position.y;
    if below {
        n = -n;
    }

    // ---- Lighting: the stock lit vertex, `clamp(ambient + diffuse·N·L)` on the up normal.
    let to_light = -normalize(wow_light.light_sun.xyz);
    let lit = clamp(
        wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb * max(to_light.y, 0.0),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    var bands = wow_light.water_river;
    if ocean {
        bands = wow_light.water_ocean;
    }
    // The water's body: the zone's deep band, lit; the shallow band tints what shows through.
    // Forever's lakes and seas read teal: the band is pulled part way toward it.
    let body = mix(bands[1].rgb, vec3<f32>(0.04, 0.16, 0.19), 0.45) * lit;
    let shallow_tint = mix(vec3<f32>(1.0), bands[0].rgb * 1.6 + 0.25, 0.6);

    // ---- What lies behind the surface: the scene copy, offset by the normal.
    let dims = vec2<f32>(textureDimensions(scene));
    let frag_uv = in.clip_position.xy / dims;
    let copy_ok = sh.state.x > 0.5 && sh.state.z > 0.5;
    var scene_z = 1e6;
    var refr = body;
    var thickness = mix(1.0, 30.0, in.depth.x);
    if copy_ok {
        let here = textureSampleLevel(scene, scene_samp, frag_uv, 0.0);
        let raw_thick = max(here.a - water_z, 0.0) * eye_dist / max(water_z, 1e-3);
        // Refraction: the bend grows with the water's depth and shrinks with distance.
        let bend = n.xz * (0.035 * clamp(raw_thick / 3.0, 0.0, 1.0)) / (1.0 + eye_dist * 0.015);
        var uv = frag_uv + bend * vec2<f32>(1.0, dims.x / dims.y) * 0.5;
        if !in_viewport(uv) {
            uv = frag_uv;
        }
        var s = textureSampleLevel(scene, scene_samp, uv, 0.0);
        // Something in front of the water (a body above the surface) is not refracted.
        if s.a < water_z {
            s = here;
        }
        scene_z = s.a;
        refr = s.rgb;
        thickness = max(scene_z - water_z, 0.0) * eye_dist / max(water_z, 1e-3);
    }
    // The water column's vertical depth, roughly (a flat bottom).
    let vertical = thickness * abs(v.y);
    if w.lane.w > 0.5 {
        if w.lane.w < 1.5 {
            return vec4<f32>(fract(scene_z / 10.0), fract(water_z / 10.0), select(0.0, 1.0, copy_ok), 1.0);
        }
        if w.lane.w < 2.5 {
            return vec4<f32>(fract(thickness), thickness / 10.0, 0.0, 1.0);
        }
        return vec4<f32>(refr, 1.0);
    }

    // ---- Seen from below: Snell's window onto the world above, total reflection outside it.
    if below {
        // Snell's window: k < 0 is total internal reflection; a soft band round the critical
        // angle keeps the ripples' window edges from cutting hard.
        let c = max(dot(v, n), 0.0);
        let k = 1.0 - 1.33 * 1.33 * (1.0 - c * c);
        let r_in = refract(-v, n, 1.33);
        var a = sky_along(normalize(select(vec3<f32>(0.0, 1.0, 0.0), r_in, k > 0.0)));
        if copy_ok && scene_z < 1e5 {
            a = refr;
        }
        let inside = body * 0.6;
        let above = mix(inside, mix(a, inside, fresnel(c)), smoothstep(0.0, 0.3, k));
        // The underwater fog the engine already applies to the scene, at the surface's distance.
        let uf = fog_at(in.world_position.xyz);
        return vec4<f32>(mix(uf.rgb, above, uf.w), 1.0);
    }

    // ---- Absorption and in-scatter along the column (Beer–Lambert).
    let murk = select(1.0, 0.75, ocean);
    let sigma = vec3<f32>(0.55, 0.20, 0.14) * murk;
    let trans = exp(-sigma * thickness);
    let scatter = 1.0 - exp(-0.60 * murk * thickness);

    // Caustics on the refracted bottom, fading with its depth below the surface.
    var caustic = 0.0;
    if copy_ok && sh.state.w > 0.5 && scene_z < 1e5 {
        let bottom = cam - v * (scene_z * eye_dist / max(water_z, 1e-3));
        let below_surface = in.world_position.y - bottom.y;
        let shift = to_light.xz * (below_surface / max(to_light.y, 0.2));
        let cp = bottom.xz + shift;
        let c1 = textureSampleLevel(ripple, ripple_samp, cp / 6.5 + wind * (t * 0.05), 0.0).a;
        let c2 = textureSampleLevel(ripple, ripple_samp, rotate(cp, 1.3) / 5.1 - wind * (t * 0.04), 0.0).a;
        let sun_up = clamp(to_light.y * 3.0, 0.0, 1.0) * sh.sun.w;
        caustic = min(c1, c2) * 1.6 * sun_up * smoothstep(0.0, 0.6, below_surface)
            * (1.0 - smoothstep(4.0, 14.0, below_surface)) * far;
    }
    let refr_lit = refr * (1.0 + caustic * wow_light.light_diffuse.rgb) * shallow_tint;

    // ---- Reflection: the scene where SSR finds it, else the sky; fresnel-weighted.
    let r = reflect(-v, n);
    let r_up = normalize(vec3<f32>(r.x, max(r.y, 0.02), r.z));
    var refl = sky_along(r_up);
    if copy_ok {
        // The march follows a calmer normal: the full ripple scatters a reflected bank into
        // blotches, where a mirror broken by the swell reads as water.
        let n_ssr = normalize(mix(n, vec3<f32>(0.0, 1.0, 0.0), 0.65));
        let r_s = reflect(-v, n_ssr);
        let hit = ssr(in.world_position.xyz, normalize(vec3<f32>(r_s.x, max(r_s.y, 0.02), r_s.z)), eye_dist);
        refl = mix(refl, hit.rgb, hit.a);
    }
    let ndotv = max(dot(n, v), 0.0);
    // A softened fresnel: real water's grazing mirror reads too hard against 1.12's sky.
    let f = fresnel(ndotv);

    // Sun and moon glints: a tight GGX lobe and a broad sheen.
    var glint = vec3<f32>(0.0);
    if sh.sun.w > 0.0 {
        let l = normalize(sh.sun.xyz);
        let h = normalize(l + v);
        let nh = max(dot(n, h), 0.0);
        // A tight lobe peaking near 9·F (bloom catches it) and a broad sheen near 0.6·F.
        let spec = ggx(nh, 0.12) * 0.0015 + ggx(nh, 0.30) * 0.004;
        glint += sh.sun_col.rgb * spec * fresnel(max(dot(v, h), 0.0)) * 4.0 * sh.sun.w;
    }
    if sh.moon.w > 0.0 {
        let l = normalize(sh.moon.xyz);
        let h = normalize(l + v);
        let nh = max(dot(n, h), 0.0);
        let spec = ggx(nh, 0.10) * 0.0008;
        glint += vec3<f32>(0.75, 0.85, 1.0) * spec * fresnel(max(dot(v, h), 0.0)) * 4.0 * sh.moon.w;
    }

    // ---- Shore foam: thin water over a bottom close below, broken by the ripple map.
    let foam = (1.0 - smoothstep(0.0, 0.7, vertical)) * smoothstep(0.42, 0.7, foam_noise)
        * select(0.0, 0.55, copy_ok);

    // ---- The blend: emitted (reflection, in-scatter, glint, foam) and transmitted (refraction).
    let emitted = f * refl + (1.0 - f) * scatter * body + glint + foam * lit;
    let kept = (1.0 - f) * (1.0 - scatter) * (1.0 - foam);
    let fog = fog_at(in.world_position.xyz);
    var rgb: vec3<f32>;
    var alpha: f32;
    if copy_ok {
        // The refraction is fogged already, at the bottom's distance.
        let k = kept * trans;
        rgb = fog.w * emitted + (1.0 - fog.w) * fog.rgb * (1.0 - k) + k * refr_lit;
        // A soft edge where the water meets geometry: no hard line round a wader or a bank.
        alpha = smoothstep(0.0, 0.18, vertical);
    } else {
        // No copy yet (the first frames): an opaque-enough sheet over the stock depth ramp.
        let k = kept * dot(trans, vec3<f32>(1.0 / 3.0));
        rgb = mix(fog.rgb, emitted / max(1.0 - k, 0.05), fog.w);
        alpha = clamp(1.0 - k, bands[0].w, 1.0);
    }
    return vec4<f32>(rgb, alpha);
}
