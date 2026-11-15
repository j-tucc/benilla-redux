// The FFT ocean (`ocean.rs`): Tessendorf's statistical wave field, three cascades of N×N, rebuilt on
// the GPU every frame from a fixed initial spectrum. `spectrum` advances each wave's phase and
// writes the four packed complex fields; `fft_rows` and `fft_cols` run the inverse transform in
// workgroup memory, one line per workgroup; `assemble` writes the displacement and slope textures
// the water samples, and `downsample` builds their mip chain.
//
// Packing (two real fields per complex IFFT, since every output is real):
//   plane 0 = (Dx + i·Dy, Dz + i·Dxz), plane 1 = (Dy/dx + i·Dy/dz, Dxx + i·Dzz)
// so disp = (Dx, Dy, Dz, Dxz) and slope = (dDy/dx, dDy/dz, dDx/dx, dDz/dz), in yd.

const N: u32 = 256u;
const LOG_N: u32 = 8u;
const CASCADES: u32 = 3u;
const PI: f32 = 3.14159265;

struct OceanParams {
    // x = time into the spectrum's period (s).
    clock: vec4<f32>,
    // xyz = each cascade's patch side (yd).
    lengths: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: OceanParams;
// The initial spectrum by cascade, centred (index N/2 is wavenumber 0): (h0.re, h0.im, ω, 0).
@group(0) @binding(1) var<storage, read> h0: array<vec4<f32>>;
// The transform's working set: [cascade][plane][y][x], two complex values per element.
@group(0) @binding(2) var<storage, read_write> fft: array<vec4<f32>>;
@group(0) @binding(3) var disp_out: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(4) var slope_out: texture_storage_2d_array<rgba16float, write>;

// The mip pass's own bindings: the level above, read; this level, written.
@group(0) @binding(5) var disp_src: texture_2d_array<f32>;
@group(0) @binding(6) var slope_src: texture_2d_array<f32>;
@group(0) @binding(7) var disp_dst: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(8) var slope_dst: texture_storage_2d_array<rgba16float, write>;

fn cmul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

fn conj(a: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x, -a.y);
}

// i·a.
fn times_i(a: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(-a.y, a.x);
}

fn plane(c: u32, p: u32) -> u32 {
    return (c * 2u + p) * N * N;
}

@compute @workgroup_size(16, 16, 1)
fn spectrum(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x;
    let y = id.y;
    let c = id.z;
    if x >= N || y >= N || c >= CASCADES {
        return;
    }
    let here = (c * N + y) * N + x;
    let mirror = (c * N + (N - y) % N) * N + (N - x) % N;
    let a = h0[here];
    let b = h0[mirror];
    let ph = a.z * params.clock.x;
    let e = vec2<f32>(cos(ph), sin(ph));
    // h(k, t) = h0(k)·e^{iωt} + conj(h0(−k))·e^{−iωt}: real in space.
    let h = cmul(a.xy, e) + cmul(conj(b.xy), conj(e));

    let side = params.lengths[c];
    let kx = 2.0 * PI * (f32(x) - f32(N / 2u)) / side;
    let kz = 2.0 * PI * (f32(y) - f32(N / 2u)) / side;
    let kl = sqrt(kx * kx + kz * kz);
    var p0 = vec4<f32>(0.0);
    var p1 = vec4<f32>(0.0);
    if kl > 1e-6 {
        let ux = kx / kl;
        let uz = kz / kl;
        // −i·h: the choppy (horizontal) displacement's phase.
        let mih = vec2<f32>(h.y, -h.x);
        let dy = h;
        let dx = ux * mih;
        let dz = uz * mih;
        let dydx = kx * times_i(h);
        let dydz = kz * times_i(h);
        let dxx = kx * ux * h;
        let dzz = kz * uz * h;
        let dxz = kx * uz * h;
        p0 = vec4<f32>(dx + times_i(dy), dz + times_i(dxz));
        p1 = vec4<f32>(dydx + times_i(dydz), dxx + times_i(dzz));
    }
    fft[plane(c, 0u) + y * N + x] = p0;
    fft[plane(c, 1u) + y * N + x] = p1;
}

var<workgroup> line_buf: array<vec4<f32>, 256>;

// One inverse radix-2 transform of `line` in workgroup memory, two complex values per element.
fn transform(lid: u32) {
    for (var s = 0u; s < LOG_N; s++) {
        let half = 1u << s;
        let j = lid % half;
        let i0 = (lid / half) * 2u * half + j;
        let i1 = i0 + half;
        let ang = PI * f32(j) / f32(half);
        let w = vec2<f32>(cos(ang), sin(ang));
        let a = line_buf[i0];
        let b = line_buf[i1];
        let bw = vec4<f32>(cmul(b.xy, w), cmul(b.zw, w));
        line_buf[i0] = a + bw;
        line_buf[i1] = a - bw;
        workgroupBarrier();
    }
}

fn fft_pass(wid: vec3<u32>, lid: u32, rows: bool) {
    let base = plane(wid.z, wid.y);
    let line = wid.x;
    for (var e = lid; e < N; e += N / 2u) {
        var at = base + e * N + line;
        if rows {
            at = base + line * N + e;
        }
        line_buf[reverseBits(e) >> (32u - LOG_N)] = fft[at];
    }
    workgroupBarrier();
    transform(lid);
    for (var e = lid; e < N; e += N / 2u) {
        var at = base + e * N + line;
        if rows {
            at = base + line * N + e;
        }
        fft[at] = line_buf[e];
    }
}

@compute @workgroup_size(128, 1, 1)
fn fft_rows(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) lid: u32) {
    fft_pass(wid, lid, true);
}

@compute @workgroup_size(128, 1, 1)
fn fft_cols(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) lid: u32) {
    fft_pass(wid, lid, false);
}

@compute @workgroup_size(16, 16, 1)
fn assemble(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x;
    let y = id.y;
    let c = id.z;
    if x >= N || y >= N || c >= CASCADES {
        return;
    }
    // The centred spectrum shifts every output by (−1)^(x+y).
    let sign = select(1.0, -1.0, ((x + y) & 1u) == 1u);
    let a = fft[plane(c, 0u) + y * N + x] * sign;
    let b = fft[plane(c, 1u) + y * N + x] * sign;
    textureStore(disp_out, vec2<u32>(x, y), c, a);
    textureStore(slope_out, vec2<u32>(x, y), c, b);
}

@compute @workgroup_size(8, 8, 1)
fn downsample(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(disp_dst);
    if id.x >= size.x || id.y >= size.y || id.z >= CASCADES {
        return;
    }
    let s = vec2<i32>(id.xy) * 2;
    let c = i32(id.z);
    let d = textureLoad(disp_src, s, c, 0) + textureLoad(disp_src, s + vec2<i32>(1, 0), c, 0)
        + textureLoad(disp_src, s + vec2<i32>(0, 1), c, 0) + textureLoad(disp_src, s + vec2<i32>(1, 1), c, 0);
    let g = textureLoad(slope_src, s, c, 0) + textureLoad(slope_src, s + vec2<i32>(1, 0), c, 0)
        + textureLoad(slope_src, s + vec2<i32>(0, 1), c, 0) + textureLoad(slope_src, s + vec2<i32>(1, 1), c, 0);
    textureStore(disp_dst, id.xy, c, d * 0.25);
    textureStore(slope_dst, id.xy, c, g * 0.25);
}
