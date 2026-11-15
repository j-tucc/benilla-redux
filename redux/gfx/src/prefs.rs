//! The switches: one flag per effect, the world's anti-aliasing and DLSS's choices, ported from
//! Elderoth's `GraphicsPrefs` (its Graphics page's options). Classic (every switch off, MSAA) is
//! the default and draws benilla's image; Enhanced turns every effect on and anti-aliases
//! temporally.
//!
//! They are kept in `redux-gfx.toml` in benilla's state folder ([`crate::settings_path`]), one
//! `gfx_*` key per switch as Elderoth keeps them, plus an optional `preset` (`"classic"` or
//! `"enhanced"`) applied before the keys. The file is re-read when it changes, so a switch can be
//! flipped while the client runs. `$REDUX_GFX` overrides it for a session ([`parse_forced`]).
use bevy::prelude::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Anti-aliasing: benilla's MSAA, as Classic draws.
pub const AA_MSAA: u8 = 0;
/// Anti-aliasing: temporal anti-aliasing.
pub const AA_TAA: u8 = 1;
/// Anti-aliasing: NVIDIA DLSS; drawn as TAA where the build, the SDK or the GPU lacks it.
pub const AA_DLSS: u8 = 2;
/// Anti-aliasing: Apple MetalFX; not built, drawn as TAA.
pub const AA_METALFX: u8 = 3;

/// DLSS's modes under [`AA_DLSS`], best image first.
pub const DLSS_DLAA: u8 = 0;
pub const DLSS_ULTRA_PERFORMANCE: u8 = 4;

/// The enhanced effects, each a flag; all off is Classic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GraphicsPrefs {
    /// Grass and leaves sway in a shared wind and part around bodies.
    pub wind: bool,
    /// The modern distance-fog curve, the analytic haze and the deband dither.
    pub fog: bool,
    /// The smooth sky gradient, sun glow, stars and lit clouds.
    pub sky: bool,
    /// The colour-grading lookup.
    pub grading: bool,
    /// The glow around over-bright light.
    pub bloom: bool,
    /// Real-time sun shadows from every model, building and the ground.
    pub shadows: bool,
    /// A cool light from the moon at night, shadowed when sun shadows are on.
    pub moonlight: bool,
    /// Light shafts through the air: 0 off, 1 Medium (16 samples a ray), 2 High (32).
    pub shafts: u8,
    /// Screen-space ambient occlusion in creases and contacts.
    pub ssao: bool,
    /// Low mist pooling over the ground at dawn, dusk and night.
    pub mist: bool,
    /// Rain darkens and glosses the ground, buildings and trees, and leaves puddles.
    pub wetness: bool,
    /// The air scatters sunlight: the sun glows through the haze, which brightens around it.
    pub scatter: bool,
    /// Sun rays: beams fanning from the sun through gaps in trees and buildings, at any range.
    pub rays: bool,
    /// Fires, braziers and lanterns light the ground and walls round them, with a flicker.
    pub firelight: bool,
    /// The fog glows round the firelight's lights, shimmering, far down a foggy road.
    pub glow: bool,
    /// The world's anti-aliasing: [`AA_MSAA`], [`AA_TAA`], [`AA_DLSS`] or [`AA_METALFX`].
    pub aa: u8,
    /// DLSS's mode while the anti-aliasing is DLSS: 0 DLAA … 4 Ultra Performance.
    pub dlss: u8,
    /// Frame generation (2x) is chosen; it runs only under DLSS, on a GPU with DLSS-G.
    pub fg: bool,
}

impl GraphicsPrefs {
    /// The file's keys, in its order.
    pub const KEYS: [&'static str; 18] = [
        "gfx_wind",
        "gfx_fog",
        "gfx_sky",
        "gfx_grading",
        "gfx_bloom",
        "gfx_shadows",
        "gfx_moonlight",
        "gfx_shafts",
        "gfx_ssao",
        "gfx_mist",
        "gfx_wetness",
        "gfx_aa",
        "gfx_scatter",
        "gfx_rays",
        "gfx_firelight",
        "gfx_dlss",
        "gfx_fg",
        "gfx_glow",
    ];

    /// Every effect on.
    pub const ENHANCED: GraphicsPrefs = GraphicsPrefs {
        wind: true,
        fog: true,
        sky: true,
        grading: true,
        bloom: true,
        shadows: true,
        moonlight: true,
        shafts: 2,
        ssao: true,
        mist: true,
        wetness: true,
        scatter: true,
        rays: true,
        firelight: true,
        glow: true,
        aa: AA_TAA,
        dlss: DLSS_DLAA,
        fg: false,
    };

    fn flag(&mut self, id: &str) -> Option<&mut bool> {
        match id {
            "gfx_wind" => Some(&mut self.wind),
            "gfx_fog" => Some(&mut self.fog),
            "gfx_sky" => Some(&mut self.sky),
            "gfx_grading" => Some(&mut self.grading),
            "gfx_bloom" => Some(&mut self.bloom),
            "gfx_shadows" => Some(&mut self.shadows),
            "gfx_moonlight" => Some(&mut self.moonlight),
            "gfx_ssao" => Some(&mut self.ssao),
            "gfx_mist" => Some(&mut self.mist),
            "gfx_wetness" => Some(&mut self.wetness),
            "gfx_scatter" => Some(&mut self.scatter),
            "gfx_rays" => Some(&mut self.rays),
            "gfx_firelight" => Some(&mut self.firelight),
            "gfx_glow" => Some(&mut self.glow),
            "gfx_fg" => Some(&mut self.fg),
            _ => None,
        }
    }

    /// Frame generation is chosen and its anti-aliasing is DLSS, the only one it runs under.
    pub fn frame_gen(&self) -> bool {
        self.fg && self.aa == AA_DLSS
    }

    /// Whether the world anti-aliases temporally (any choice but MSAA).
    pub fn temporal(&self) -> bool {
        self.aa != AA_MSAA
    }

    /// Sets an option by its key; false if `id` is not one of these.
    pub fn set(&mut self, id: &str, v: usize) -> bool {
        match id {
            "gfx_shafts" => self.shafts = v.min(2) as u8,
            "gfx_aa" => self.aa = v.min(usize::from(AA_METALFX)) as u8,
            "gfx_dlss" => self.dlss = v.min(usize::from(DLSS_ULTRA_PERFORMANCE)) as u8,
            _ => return self.flag(id).map(|f| *f = v != 0).is_some(),
        }
        true
    }

    /// The switches a `redux-gfx.toml` holds: its `preset`, then each `gfx_*` key (a bool or a
    /// number). Unknown keys are ignored, so the file can carry redux's other entries.
    pub fn from_table(table: &toml::Table) -> GraphicsPrefs {
        let mut p = match table.get("preset").and_then(toml::Value::as_str) {
            Some(s) if s.eq_ignore_ascii_case("enhanced") => GraphicsPrefs::ENHANCED,
            _ => GraphicsPrefs::default(),
        };
        for key in Self::KEYS {
            let v = match table.get(key) {
                Some(toml::Value::Boolean(b)) => usize::from(*b),
                Some(toml::Value::Integer(i)) => (*i).max(0) as usize,
                _ => continue,
            };
            p.set(key, v);
        }
        p
    }
}

/// `$REDUX_GFX`: `classic`, `enhanced`, or a comma list of effect names (`wind`, `fog`, `sky`,
/// `grading`, `bloom`, `shadows`, `moonlight`, `shafts` (Medium) or `shafts_high`, `ssao`, `mist`,
/// `wetness`, `scatter`, `rays`, `firelight`, `glow`), one anti-aliasing (`msaa`, `taa`, `dlss`, a DLSS
/// mode `dlaa`/`dlss_q`/`dlss_b`/`dlss_p`/`dlss_up`, `metalfx`) and `fg`. `None` when unreadable.
pub fn parse_forced(v: &str) -> Option<GraphicsPrefs> {
    let mut p = GraphicsPrefs::default();
    for word in v.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        match word.to_ascii_lowercase().as_str() {
            "classic" => {}
            "enhanced" => p = GraphicsPrefs::ENHANCED,
            "shafts_high" => p.shafts = 2,
            "msaa" => p.aa = AA_MSAA,
            "taa" => p.aa = AA_TAA,
            "dlss" => p.aa = AA_DLSS,
            "metalfx" => p.aa = AA_METALFX,
            mode if crate::DlssQuality::from_word(mode).is_some() => {
                p.aa = AA_DLSS;
                p.dlss = crate::DlssQuality::from_word(mode).unwrap() as u8;
            }
            effect => {
                if !p.set(&format!("gfx_{effect}"), 1) {
                    warn!("REDUX_GFX: unknown effect {effect:?}");
                    return None;
                }
            }
        }
    }
    Some(p)
}

/// The settings file, polled for changes.
#[derive(Resource)]
pub(crate) struct SettingsFile {
    path: Option<PathBuf>,
    stamp: Option<SystemTime>,
    next_poll: Duration,
}

impl SettingsFile {
    pub(crate) fn new(config_dir: Option<&Path>) -> Self {
        Self {
            path: config_dir.map(crate::settings_path),
            stamp: None,
            next_poll: Duration::ZERO,
        }
    }
}

/// How often the file's change time is checked.
const POLL: Duration = Duration::from_secs(1);

/// Copies `redux-gfx.toml` into [`crate::GfxSettings`] at boot and whenever it changes.
pub(crate) fn follow_file(
    time: Res<Time<Real>>,
    mut file: ResMut<SettingsFile>,
    mut settings: ResMut<crate::GfxSettings>,
) {
    let now = time.elapsed();
    if now < file.next_poll {
        return;
    }
    file.next_poll = now + POLL;
    let Some(path) = file.path.clone() else {
        return;
    };
    let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if stamp == file.stamp {
        return;
    }
    file.stamp = stamp;
    let table = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .unwrap_or_default();
    let prefs = GraphicsPrefs::from_table(&table);
    if settings.0 != prefs {
        info!("redux graphics: {} -> {prefs:?}", path.display());
        settings.0 = prefs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_reads_a_preset_then_its_keys() {
        let t: toml::Table =
            "preset = \"enhanced\"\ngfx_bloom = false\ngfx_aa = 2\naa_defaults_seeded = true\n"
                .parse()
                .unwrap();
        let p = GraphicsPrefs::from_table(&t);
        assert!(p.wind && p.shadows && !p.bloom);
        assert_eq!(p.aa, AA_DLSS);
        assert_eq!(
            GraphicsPrefs::from_table(&toml::Table::new()),
            GraphicsPrefs::default()
        );
    }

    #[test]
    fn the_session_switch_reads_presets_and_effect_lists() {
        assert_eq!(parse_forced("classic"), Some(GraphicsPrefs::default()));
        assert_eq!(parse_forced("Enhanced"), Some(GraphicsPrefs::ENHANCED));
        let p = parse_forced("bloom, grading").unwrap();
        assert!(p.bloom && p.grading && !p.wind);
        assert_eq!(parse_forced("bloom,lasers"), None);
        let p = parse_forced("enhanced,msaa").unwrap();
        assert!(p.wind && p.shadows && p.aa == AA_MSAA);
        let p = parse_forced("enhanced,dlss_q,fg").unwrap();
        assert_eq!((p.aa, p.dlss), (AA_DLSS, 1));
        assert!(p.frame_gen());
    }
}
