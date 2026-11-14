//! Sun shadows (LOOK-14): one Bevy `DirectionalLight` with a single 80-yard cascade, aimed at the
//! sun by day and, with moonlight on, at the moon by night. It lights nothing (illuminance 0): it
//! exists for its shadow map, which the real meshes cast into through the engine's depth stages
//! (engine patch 0011: the model lane's skinned depth shader, terrain's prepass and the retained
//! static pass's caster node). The receivers' `gfx_lit` hook samples it ([`shadow.wgsl`]).
//!
//! The rows: the cascade matrix ([`SHADOW_CLIP`], [`SHADOW_TEXEL`]) is written from the render
//! world after the light upload, from the cascade Bevy builds this frame, so map and lookup never
//! drift a frame apart; the weights ([`SHADOW_A`], [`SHADOW_B`]) ride the main-world `GfxData`.
//!
//! Ported from the everwood graphics fork of Benilla (MIT OR Apache-2.0), revision `cdc7130b`,
//! `benilla-app/src/shadow_core.rs`: the single cascade, the biases, the 2048² map, the clamped
//! elevation, the snap that holds the map still between small sun steps, and the one-body
//! hand-over at dusk and dawn. Everwood's CPU casters (`character_shadow.rs`, `world_shadow.rs`)
//! are not ported: every caster here is the drawn mesh itself, GPU-skinned.
use super::rows::{MOON_B, SHADOW_A, SHADOW_B, SHADOW_CLIP, SHADOW_TEXEL};
use super::Enhanced;
use benilla_world::lighting::{GfxData, SharedLightBuffer, WowLighting, GFX_DATA_OFFSET};
use benilla_world::view::WorldCamera;
use bevy::light::{
    CascadeShadowConfigBuilder, Cascades, DirectionalLight, DirectionalLightShadowMap,
    SimulationLightSystems,
};
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::renderer::RenderQueue;
use bevy::render::{Render, RenderApp, RenderSystems};

/// How far the shadows reach (yd), the cascade's far bound. Source: everwood
/// `DEFAULT_SHADOW_DISTANCE`.
pub const SHADOW_DISTANCE: f32 = 80.0;
/// The map's size (texels a side). Source: everwood `DEFAULT_SHADOW_MAP_SIZE`.
pub const SHADOW_MAP_SIZE: usize = 2048;
/// The depth bias along the light (yd). Source: everwood's rig light.
const DEPTH_BIAS: f32 = 0.02;
/// The normal bias, in cascade texels. Source: everwood's rig light.
const NORMAL_BIAS: f32 = 0.8;
/// The share of the direct light a fully shadowed surface keeps. Source: everwood
/// `SHADOW_SUN_FLOOR`.
const SUN_FLOOR: f32 = 0.45;
/// The rig re-aims only past this angle, so small sun steps leave the map still. Source:
/// everwood `SUN_SNAP_RADIANS`.
const SNAP_RADIANS: f32 = 8.0e-4;
/// The shadow direction's elevation clamp (rad): no endless dawn shadows, no vanishing noon ones.
/// Source: everwood `MIN_SHADOW_SUN_ELEVATION`, `MAX_SHADOW_SUN_ELEVATION`.
const MIN_ELEVATION: f32 = 0.314;
const MAX_ELEVATION: f32 = 1.134;
/// The hand-over's fade rate (weight per second) when the rig changes body.
const HANDOVER_RATE: f32 = 1.0;

pub(super) struct ShadowsPlugin;

/// The depth bias along the light, for the shafts' march.
pub(super) const SHAFT_DEPTH_BIAS: f32 = DEPTH_BIAS;

impl Plugin for ShadowsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(DirectionalLightShadowMap {
            size: SHADOW_MAP_SIZE,
        })
        .init_resource::<Rig>()
        .init_resource::<ShadowCascade>()
        .add_plugins(ExtractResourcePlugin::<ShadowCascade>::default())
        .add_systems(
            PostUpdate,
            manage_rig.before(bevy::transform::TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            publish_cascade.after(SimulationLightSystems::UpdateDirectionalLightCascades),
        );
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(
                Render,
                write_cascade.in_set(RenderSystems::PrepareBindGroups),
            );
            render_app.add_systems(
                Render,
                heal_specialization_ticks
                    .in_set(RenderSystems::ManageViews)
                    .before(bevy::pbr::specialize_shadows),
            );
        }
    }
}

/// Which body the rig's map holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Body {
    #[default]
    Sun,
    Moon,
}

/// The rig light's marker.
#[derive(Component)]
struct ShadowRig;

#[derive(Resource, Default)]
struct Rig {
    light: Option<Entity>,
    /// The direction last written to the light.
    written: Option<Vec3>,
    /// The body the map holds, and how far its shadow is faded in (0..1).
    body: Body,
    ramp: f32,
}

/// The cascade the map holds this frame, for the render world: the rows [`SHADOW_CLIP`]..=
/// [`SHADOW_TEXEL`], all zero when there is none.
#[derive(Resource, Clone, Copy, Default, ExtractResource)]
pub(super) struct ShadowCascade(pub(super) [[f32; 4]; 5]);

/// The sun's shadow strength from its height (`sin(elevation)`): none below the horizon, full
/// from about 12° up. Source: everwood `sun_shadow_strength`.
pub(super) fn body_strength(height: f32) -> f32 {
    let t = (height / 0.208).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The body the clock asks for and its strength: the sun while it is up at all, else the moon
/// when moonlight is on and it is up, else none.
pub(super) fn wanted_body(sun_y: f32, moon_y: f32, moon_on: bool) -> (Body, f32) {
    let sun = body_strength(sun_y);
    if sun > 0.0 {
        (Body::Sun, sun)
    } else if moon_on {
        (Body::Moon, body_strength(moon_y))
    } else {
        (Body::Sun, 0.0)
    }
}

/// The direction the light travels for a body at `to_body`, its elevation clamped. Source:
/// everwood `shadow_sun_travel`.
pub(super) fn shadow_travel(to_body: Vec3) -> Vec3 {
    let s = to_body.normalize_or_zero();
    if s == Vec3::ZERO {
        return Vec3::NEG_Y;
    }
    let flat = Vec3::new(s.x, 0.0, s.z);
    let bearing = if flat.length() > 1e-4 {
        flat.normalize()
    } else {
        Vec3::NEG_Z
    };
    let elevation =
        s.y.clamp(-1.0, 1.0)
            .asin()
            .clamp(MIN_ELEVATION, MAX_ELEVATION);
    -(bearing * elevation.cos() + Vec3::Y * elevation.sin())
}

#[allow(clippy::too_many_arguments)]
fn manage_rig(
    enhanced: Res<Enhanced>,
    lighting: Res<WowLighting>,
    time: Res<Time>,
    mut rig: ResMut<Rig>,
    mut frame: ResMut<GfxData>,
    mut commands: Commands,
    mut lights: Query<(&mut Transform, &mut DirectionalLight), With<ShadowRig>>,
    camera: Query<(), With<WorldCamera>>,
) {
    // Light shafts march the same map, so they keep the rig too.
    let on = (enhanced.shadows || enhanced.shafts > 0) && !camera.is_empty();
    let mut next = *frame;
    if !on {
        if let Some(light) = rig.light.take() {
            commands.entity(light).despawn();
        }
        *rig = Rig::default();
        next.rows[SHADOW_A] = [0.0; 4];
        next.rows[SHADOW_B] = [0.0; 4];
        next.rows[MOON_B][3] = 0.0;
        if *frame != next {
            *frame = next;
        }
        return;
    }
    let light = *rig.light.get_or_insert_with(|| {
        commands
            .spawn((
                DirectionalLight {
                    illuminance: 0.0,
                    shadows_enabled: true,
                    shadow_depth_bias: DEPTH_BIAS,
                    shadow_normal_bias: NORMAL_BIAS,
                    ..default()
                },
                CascadeShadowConfigBuilder {
                    num_cascades: 1,
                    minimum_distance: 0.1,
                    maximum_distance: SHADOW_DISTANCE,
                    ..default()
                }
                .build(),
                Transform::IDENTITY,
                ShadowRig,
            ))
            .id()
    });
    let sun_y = lighting.celestial_dir().y;
    let moon_y = lighting.moon_dir().y;
    let (want, strength) = wanted_body(sun_y, moon_y, enhanced.moonlight);
    // The hand-over: fade the held body out, re-aim while it is invisible, fade the new one in.
    let dt = time.delta_secs();
    if want != rig.body {
        rig.ramp = (rig.ramp - HANDOVER_RATE * dt).max(0.0);
        if rig.ramp <= 0.0 {
            rig.body = want;
            rig.written = None;
        }
    } else {
        rig.ramp = (rig.ramp + HANDOVER_RATE * dt).min(1.0);
    }
    let held = match rig.body {
        Body::Sun => lighting.celestial_dir(),
        Body::Moon => lighting.moon_dir(),
    };
    let held_strength = if rig.body == want {
        strength
    } else {
        match rig.body {
            Body::Sun => body_strength(sun_y),
            Body::Moon => body_strength(moon_y),
        }
    };
    let weight = held_strength * rig.ramp;
    let travel = shadow_travel(held);
    if let Ok((mut transform, mut dir_light)) = lights.get_mut(light) {
        let due = rig
            .written
            .is_none_or(|w| w.angle_between(travel) > SNAP_RADIANS);
        if due {
            *transform = Transform::IDENTITY.looking_to(travel, Vec3::Y);
            rig.written = Some(travel);
        }
        // No map while no body casts: the shadow views and their passes go away.
        let cast = weight > 0.0;
        if dir_light.shadows_enabled != cast {
            dir_light.shadows_enabled = cast;
        }
    }
    let to_light = -rig.written.unwrap_or(travel);
    let sun_weight = if rig.body == Body::Sun { weight } else { 0.0 };
    next.rows[SHADOW_A] = [weight, SHADOW_DISTANCE, SUN_FLOOR, sun_weight];
    next.rows[SHADOW_B] = [to_light.x, to_light.y, to_light.z, DEPTH_BIAS];
    next.rows[MOON_B][3] = if rig.body == Body::Moon {
        rig.ramp
    } else {
        0.0
    };
    if *frame != next {
        *frame = next;
    }
}

/// The shadow-flicker probe's switches (`benilla-adapter/src/gfx_probe/flicker.rs`), read once:
/// `REDUX_GFX_DAPPLE=0` turns the dapples off, `REDUX_GFX_SHADOW_VIEW=1` draws every receiver
/// as its bare visibility (white lit, black shadowed), so a measure sees the shadow alone, and
/// `REDUX_GFX_BAKE=0` steps WoW's baked shadow wholly aside inside the map. The last two ride
/// one value as bits (1 the view, 2 the bake aside).
struct ProbeKnobs {
    dapple: f32,
    view: f32,
}

impl Default for ProbeKnobs {
    fn default() -> Self {
        let env = |k: &str| std::env::var(k).ok();
        Self {
            dapple: if env("REDUX_GFX_DAPPLE").as_deref() == Some("0") {
                0.0
            } else {
                1.0
            },
            view: f32::from(u8::from(
                env("REDUX_GFX_SHADOW_VIEW").as_deref() == Some("1"),
            )) + 2.0 * f32::from(u8::from(env("REDUX_GFX_BAKE").as_deref() == Some("0"))),
        }
    }
}

/// The world camera's cascade of the rig light, for the render world.
fn publish_cascade(
    rig: Res<Rig>,
    lights: Query<(&Cascades, &DirectionalLight), With<ShadowRig>>,
    camera: Query<Entity, With<WorldCamera>>,
    mut out: ResMut<ShadowCascade>,
    knobs: Local<ProbeKnobs>,
) {
    let cascade = rig
        .light
        .and_then(|l| lights.get(l).ok())
        .filter(|(_, light)| light.shadows_enabled)
        .zip(camera.iter().next())
        .and_then(|((cascades, _), cam)| cascades.cascades.get(&cam))
        .and_then(|c| c.first());
    let rows = match cascade {
        Some(c) => {
            let cols = c.clip_from_world.to_cols_array_2d();
            [
                cols[0],
                cols[1],
                cols[2],
                cols[3],
                [NORMAL_BIAS * c.texel_size, 1.0, knobs.dapple, knobs.view],
            ]
        }
        None => [[0.0; 4]; 5],
    };
    if out.0 != rows {
        out.0 = rows;
    }
}

/// Render world, after the light upload (`PrepareResources`): the cascade's rows over the
/// uploaded block, so the lookup matches the map Bevy draws this frame.
fn write_cascade(
    queue: Res<RenderQueue>,
    buffer: Option<Res<SharedLightBuffer>>,
    cascade: Res<ShadowCascade>,
) {
    let Some(buffer) = buffer else {
        return;
    };
    if cascade.0[SHADOW_TEXEL - SHADOW_CLIP][1] < 0.5 {
        return;
    }
    let offset = GFX_DATA_OFFSET + (SHADOW_CLIP * 16) as u64;
    let bytes: Vec<u8> = cascade
        .0
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    queue.write_buffer(&buffer.0, offset, &bytes);
}

/// Bevy 0.18's `specialize_shadows` unwraps a specialization tick for every mesh a light sees,
/// but a mesh whose material just changed or left can keep its material instance a frame after
/// its tick is swept (only the camera's own pass, which never sees such a mesh, is safe). A light
/// sees meshes the camera does not, so the rig would panic the render world: give each such
/// instance a tick first, which at worst specializes one extra shadow pipeline for it.
fn heal_specialization_ticks(
    instances: Res<bevy::pbr::RenderMaterialInstances>,
    mut ticks: ResMut<bevy::pbr::EntitySpecializationTicks>,
    rig: Res<ShadowCascade>,
    this_run: bevy::ecs::system::SystemChangeTick,
) {
    if rig.0[SHADOW_TEXEL - SHADOW_CLIP][1] < 0.5 {
        return;
    }
    for (entity, _) in instances.instances.iter() {
        if !ticks.contains_key(entity) {
            ticks.insert(
                *entity,
                bevy::pbr::EntitySpecializationTickPair {
                    system_tick: this_run.this_run(),
                    material_instances_tick: instances.current_change_tick,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rig_hands_the_night_to_the_moon_only_with_moonlight() {
        assert_eq!(wanted_body(0.5, -0.5, true), (Body::Sun, 1.0));
        assert_eq!(wanted_body(-0.3, 0.6, false).1, 0.0);
        assert_eq!(wanted_body(-0.3, 0.6, true), (Body::Moon, 1.0));
        assert!(wanted_body(0.05, 0.6, true).1 < 1.0);
    }

    #[test]
    fn the_shadow_direction_keeps_its_elevation_in_band() {
        for to_sun in [
            Vec3::Y,
            Vec3::new(1.0, 0.01, 0.0),
            Vec3::new(0.3, 0.5, -0.8),
        ] {
            let t = shadow_travel(to_sun);
            let elev = (-t.y).asin();
            assert!((MIN_ELEVATION - 1e-4..=MAX_ELEVATION + 1e-4).contains(&elev));
        }
    }

    #[test]
    fn the_cascade_rows_sit_before_the_weights() {
        assert_eq!(SHADOW_CLIP + 4, SHADOW_TEXEL);
        const { assert!(SHADOW_TEXEL < SHADOW_A) };
    }
}
