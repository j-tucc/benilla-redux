//! Ground mist (LOOK-14): low mist pooling over the ground, densest at dawn, thinner at dusk and
//! through the night, scarce at midday, thicker in weather, none indoors. An exponential height
//! fog whose base follows the terrain under the camera, so it lies in the valley the player stands
//! in; the haze pass (`haze.wgsl`) draws it with the closed-form optical depth along each view ray,
//! no march. This module fills the [`MIST`] row.
//!
//! The technique (an analytic exponential height fog over a distance haze) is the textbook one;
//! no code is taken from any other project.
use super::post::smoothstep;
use super::rows::MIST;
use super::Enhanced;
use benilla_assets::AdtTile;
use benilla_world::{
    lighting::{GameClock, GfxData},
    terrain_stream::{terrain_height_under, TerrainStreamer},
    view::WorldCamera,
    weather::WeatherState,
    wmo_portal::CameraInteriorClaim,
};
use bevy::prelude::*;

/// The density per yard at the mist's base at its thickest (dawn, clear sky).
const PEAK_DENSITY: f32 = 0.02;
/// The base sits this far above the ground, so the grass stands in the mist.
const BASE_LIFT: f32 = 0.5;
/// How fast the mist thins upward (per yd): an e-fold every 2.5 yd.
const FALLOFF: f32 = 0.4;
/// Its onset (yd): clear at the player's feet, full from about twice this.
const REACH: f32 = 6.0;
/// Where the base sits under the camera when no terrain is streamed there.
const CAMERA_HEIGHT: f32 = 3.0;

pub(super) struct MistPlugin;

impl Plugin for MistPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, update_mist);
    }
}

/// The mist's share of its peak at `minute` of the day (0..1440) in `weather` (0..1): the dawn
/// peak, a lesser dusk one, a night floor and a little at midday.
pub(super) fn mist_strength(minute: f32, weather: f32) -> f32 {
    let bump = |centre: f32, half: f32| {
        let t = (1.0 - ((minute - centre) / half).abs()).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let night = smoothstep(1110.0, 1260.0, minute).max(1.0 - smoothstep(300.0, 420.0, minute));
    let time = bump(390.0, 120.0)
        .max(0.6 * bump(1170.0, 120.0))
        .max(0.35 * night)
        .max(0.08);
    (time * (1.0 + weather.clamp(0.0, 1.0))).min(1.0)
}

#[allow(clippy::too_many_arguments)]
fn update_mist(
    enhanced: Res<Enhanced>,
    clock: Option<Res<GameClock>>,
    weather: Option<Res<WeatherState>>,
    interior: Option<Res<CameraInteriorClaim>>,
    streamer: Option<Res<TerrainStreamer>>,
    tiles: Option<Res<Assets<AdtTile>>>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    mut frame: ResMut<GfxData>,
) {
    let indoors = interior.is_some_and(|c| c.0.is_some());
    let row = match camera.iter().next() {
        Some(at) if enhanced.mist && !indoors => {
            let eye = at.translation();
            let ground = streamer
                .zip(tiles)
                .and_then(|(s, t)| terrain_height_under(&s, &t, eye))
                .unwrap_or(eye.y - CAMERA_HEIGHT);
            let minute = clock.map_or(720.0, |c| c.minute as f32);
            let weather = weather.map_or(0.0, |w| w.sky_density.max(w.effect_density));
            [
                PEAK_DENSITY * mist_strength(minute, weather),
                ground + BASE_LIFT,
                FALLOFF,
                REACH,
            ]
        }
        _ => [0.0; 4],
    };
    if frame.rows[MIST] != row {
        frame.rows[MIST] = row;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dawn_is_mistiest_midday_least_and_weather_thickens_it() {
        let dawn = mist_strength(390.0, 0.0);
        let noon = mist_strength(720.0, 0.0);
        let dusk = mist_strength(1170.0, 0.0);
        let midnight = mist_strength(0.0, 0.0);
        assert_eq!(dawn, 1.0);
        assert!(noon < 0.1 && noon > 0.0);
        assert!(dusk < dawn && dusk > midnight);
        assert!(midnight > noon);
        assert!(mist_strength(720.0, 1.0) > noon * 1.9);
    }
}
