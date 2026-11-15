// The model lane's depth-only stage (an Elderoth engine patch): `WowModelExt`'s prepass vertex and
// fragment shader, which Bevy runs for the material's shadow views (and any depth prepass). It
// places a vertex exactly as `wow_model.wgsl` does, skinned from the shared palette and
// camera-relative for f32 precision, so a cast shadow sits under the drawn mesh; the stock engine
// never spawns a shadow-casting light, so it runs only for a program that does. A batch that must
// not cast (`WOW_NO_CAST`, from `WowModelExt::specialize`) collapses in the vertex stage; a cutout
// or blended batch discards its transparent texels.

#import benilla::{gfx_data, gfx_hooks}
#import bevy_pbr::{
    mesh_view_bindings::view,
    mesh_functions,
    pbr_bindings,
    pbr_types,
}

// `wow_model.wgsl`'s `ModelParams` and `WowLight`, in the same order; keep the three in sync.
struct ModelParams {
    clutter_fade: vec4<f32>,
    model_flags: vec4<f32>,
    sun_scale: vec4<f32>,
    tint: vec4<f32>,
    sidn: vec4<f32>,
    anim_slots: vec4<f32>,
    stage1: vec4<f32>,
    stage1_uv: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> m: ModelParams;

struct WowLight {
    light_ambient: vec4<f32>,
    light_diffuse: vec4<f32>,
    light_sun: vec4<f32>,
    light_spec: vec4<f32>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
    sh_c10_r: vec4<f32>,
    sh_c10_g: vec4<f32>,
    sh_c10_b: vec4<f32>,
    sh_c13_r: vec4<f32>,
    sh_c13_g: vec4<f32>,
    sh_c13_b: vec4<f32>,
    sh_c16: vec4<f32>,
    _water: array<vec4<f32>, 4>,
    grade: vec4<f32>,
    wmo_fog_color: vec4<f32>,
    wmo_fog_params: vec4<f32>,
    point_count: vec4<f32>,
    points: array<vec4<f32>, 512>,
    gfx: gfx_data::GfxData,
    prop_probes: array<vec4<f32>, 57344>,
    rig_table: array<u32, 2048>,
    rig_tint: array<u32, 2048>,
    rig_origin: array<vec4<f32>, 2048>,
    matanim: array<vec4<f32>, 2048>,
    water_clip: array<vec2<f32>, 2048>,
    palettes: array<vec4<f32>>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(90) var<storage, read> wow_light: WowLight;

// Bevy's prepass locations (position 0, UV_0 1, UV_1 2) plus the palette joints at 10/11, as
// `WowModelExt::specialize` lays them out for a depth pipeline.
struct DepthVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
#ifdef VERTEX_UVS_A
    @location(1) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(2) uv_b: vec2<f32>,
#endif
#ifdef WOW_RIG_SKIN
    @location(10) joint_indices: vec4<u32>,
    @location(11) joint_weights: vec4<f32>,
#endif
}

struct DepthOut {
    @builtin(position) position: vec4<f32>,
#ifdef VERTEX_UVS_A
    @location(0) uv: vec2<f32>,
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    @location(1) unclipped_depth: f32,
#endif
}

#ifdef WOW_RIG_SKIN
fn wow_rig_slot(instance_index: u32) -> u32 {
    return (mesh_functions::get_tag(instance_index) >> 19u) & 0x7ffu;
}

// `wow_model.wgsl`'s `wow_skin_model`, verbatim.
fn wow_skin_model(instance_index: u32, indices: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    let base = wow_light.rig_table[wow_rig_slot(instance_index)];
    let b0 = 3u * (base + indices.x);
    let b1 = 3u * (base + indices.y);
    let b2 = 3u * (base + indices.z);
    let b3 = 3u * (base + indices.w);
    let r0 = weights.x * wow_light.palettes[b0]
        + weights.y * wow_light.palettes[b1]
        + weights.z * wow_light.palettes[b2]
        + weights.w * wow_light.palettes[b3];
    let r1 = weights.x * wow_light.palettes[b0 + 1u]
        + weights.y * wow_light.palettes[b1 + 1u]
        + weights.z * wow_light.palettes[b2 + 1u]
        + weights.w * wow_light.palettes[b3 + 1u];
    let r2 = weights.x * wow_light.palettes[b0 + 2u]
        + weights.y * wow_light.palettes[b1 + 2u]
        + weights.z * wow_light.palettes[b2 + 2u]
        + weights.w * wow_light.palettes[b3 + 2u];
    return mat4x4<f32>(
        vec4<f32>(r0.x, r1.x, r2.x, 0.0),
        vec4<f32>(r0.y, r1.y, r2.y, 0.0),
        vec4<f32>(r0.z, r1.z, r2.z, 0.0),
        vec4<f32>(r0.w, r1.w, r2.w, 1.0),
    );
}
#endif

@vertex
fn vertex(vertex: DepthVertex) -> DepthOut {
    var out: DepthOut;
#ifdef WOW_RIG_SKIN
    let frame_from_local = wow_skin_model(
        vertex.instance_index,
        vertex.joint_indices,
        vertex.joint_weights
    );
    let frame_origin = wow_light.rig_origin[wow_rig_slot(vertex.instance_index)].xyz;
#else
    var frame_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let frame_origin = frame_from_local[3].xyz;
    frame_from_local[3] = vec4<f32>(0.0, 0.0, 0.0, 1.0);
#endif
    var p_cam = (frame_from_local * vec4<f32>(vertex.position, 1.0)).xyz
        + (frame_origin - view.world_position);
#ifdef VERTEX_UVS_B
    if (m.clutter_fade.w > 0.5) {
        p_cam += gfx_hooks::gfx_vertex_offset(
            gfx_hooks::GFX_VERTEX_CLUTTER, p_cam + view.world_position,
            vec4<f32>(vertex.uv_b, 0.0, 0.0), view.world_position, wow_light.gfx,
        );
    }
#endif
    let view_rot = mat3x3<f32>(
        view.view_from_world[0].xyz,
        view.view_from_world[1].xyz,
        view.view_from_world[2].xyz,
    );
    out.position = view.clip_from_view * vec4<f32>(view_rot * p_cam, 1.0);
#ifdef WOW_NO_CAST
    out.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
#endif
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif
    return out;
}

// A cutout batch casts at its own reference, a blended one where it is at least half covered.
fn depth_discard(in: DepthOut) {
#ifdef MAY_DISCARD
#ifdef VERTEX_UVS_A
    let flags = pbr_bindings::material.flags;
    var alpha = pbr_bindings::material.base_color.a;
    if ((flags & pbr_types::STANDARD_MATERIAL_FLAGS_BASE_COLOR_TEXTURE_BIT) != 0u) {
        alpha = alpha * textureSampleBias(
            pbr_bindings::base_color_texture,
            pbr_bindings::base_color_sampler,
            in.uv,
            view.mip_bias,
        ).a;
    }
    let mode = flags & pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_RESERVED_BITS;
    let cutoff = select(0.5, pbr_bindings::material.alpha_cutoff,
        mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_MASK);
    if (alpha < cutoff) {
        discard;
    }
#endif
#endif
}

#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
@fragment
fn fragment(in: DepthOut) -> @builtin(frag_depth) f32 {
    depth_discard(in);
    return in.unclipped_depth;
}
#else
@fragment
fn fragment(in: DepthOut) {
    depth_discard(in);
}
#endif
