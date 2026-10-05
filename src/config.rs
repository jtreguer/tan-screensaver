//! Settings from the config file (SPEC §4). Flags override the file, which overrides the
//! defaults; an unreadable or invalid file is reported and the defaults are used.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::field::SystemKind;
use crate::palette;
use crate::scene::Choices;

/// Output height at which `line_width` and speeds are taken literally.
pub const REF_HEIGHT_PX: f64 = 1440.0;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Start points in flight per screen.
    pub sparks: u32,
    /// Start points per scene.
    pub trajectories: u32,
    /// Max arc length per direction, world units.
    pub arc_length: f64,
    pub trace_backward: bool,
    /// Target scene duration; the speed is derived from it.
    pub scene_seconds: f64,
    /// px/s at `REF_HEIGHT_PX`.
    pub min_speed: f64,
    pub max_speed: f64,
    pub step_px: f64,
    /// px at `REF_HEIGHT_PX`.
    pub line_width: f64,
    pub exposure: f64,
    pub hold_seconds: f64,
    pub fade_seconds: f64,
    /// Empty means all.
    pub systems: Vec<String>,
    pub palettes: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            sparks: 80,
            trajectories: 1500,
            arc_length: 30.0,
            trace_backward: true,
            scene_seconds: 180.0,
            min_speed: 120.0,
            max_speed: 300.0,
            step_px: 0.5,
            line_width: 1.2,
            exposure: 0.35,
            hold_seconds: 6.0,
            fade_seconds: 2.5,
            systems: Vec::new(),
            palettes: Vec::new(),
        }
    }
}

impl Settings {
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("tan-screensaver").join("config.toml"))
    }

    /// Reads `path`, or the default path when None. A missing default file is not an
    /// error; anything else wrong is returned alongside the defaults.
    pub fn load(path: Option<&Path>) -> (Settings, Option<String>) {
        let (path, explicit) = match path {
            Some(p) => (p.to_path_buf(), true),
            None => match Settings::default_path() {
                Some(p) => (p, false),
                None => return (Settings::default(), None),
            },
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if !explicit && e.kind() == std::io::ErrorKind::NotFound => {
                return (Settings::default(), None)
            }
            Err(e) => {
                return (
                    Settings::default(),
                    Some(format!("{}: {e}", path.display())),
                )
            }
        };
        match Settings::parse(&text) {
            Ok(s) => (s, None),
            Err(e) => (
                Settings::default(),
                Some(format!("{}: {e}", path.display())),
            ),
        }
    }

    pub fn parse(text: &str) -> Result<Settings, String> {
        let s: Settings = toml::from_str(text).map_err(|e| e.message().to_string())?;
        s.validate()?;
        Ok(s)
    }

    pub fn validate(&self) -> Result<(), String> {
        let positive = [
            ("sparks", self.sparks as f64),
            ("trajectories", self.trajectories as f64),
            ("arc_length", self.arc_length),
            ("scene_seconds", self.scene_seconds),
            ("min_speed", self.min_speed),
            ("line_width", self.line_width),
            ("exposure", self.exposure),
        ];
        for (name, v) in positive {
            if !(v > 0.0 && v.is_finite()) {
                return Err(format!("{name} must be positive"));
            }
        }
        if !(self.max_speed >= self.min_speed && self.max_speed.is_finite()) {
            return Err("max_speed must be at least min_speed".into());
        }
        // Below 0.1 px a scene takes millions of steps per trajectory; above 4 px the
        // curves turn into polylines.
        if !(0.1..=4.0).contains(&self.step_px) {
            return Err("step_px must be between 0.1 and 4".into());
        }
        // An hour bounds the timers well inside what Instant arithmetic can hold.
        for (name, v) in [
            ("hold_seconds", self.hold_seconds),
            ("fade_seconds", self.fade_seconds),
            ("scene_seconds", self.scene_seconds),
        ] {
            if !(0.0..=3600.0).contains(&v) {
                return Err(format!("{name} must be between 0 and 3600"));
            }
        }
        Ok(())
    }

    /// Systems and palettes allowed in scenes; unknown names are returned as warnings.
    pub fn choices(&self) -> (Choices, Vec<String>) {
        let mut warnings = Vec::new();
        let systems = self
            .systems
            .iter()
            .filter_map(|n| {
                let k = SystemKind::from_name(n);
                if k.is_none() {
                    warnings.push(format!("unknown system {n:?} ignored"));
                }
                k
            })
            .collect();
        let palettes = self
            .palettes
            .iter()
            .filter_map(|n| {
                let p = palette::find(n);
                if p.is_none() {
                    warnings.push(format!("unknown palette {n:?} ignored"));
                }
                p
            })
            .collect();
        (Choices { systems, palettes }, warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(Settings::parse(""), Ok(Settings::default()));
    }

    #[test]
    fn keys_override_defaults() {
        let s = Settings::parse("sparks = 40\nexposure = 0.5\npalettes = [\"magma\"]").unwrap();
        assert_eq!(s.sparks, 40);
        assert_eq!(s.exposure, 0.5);
        assert_eq!(s.trajectories, 1500);
        assert_eq!(s.choices().0.palettes, vec![0]);
    }

    #[test]
    fn bad_files_are_rejected() {
        assert!(Settings::parse("sparkz = 3").is_err());
        assert!(Settings::parse("sparks = 0").is_err());
        assert!(Settings::parse("sparks = -1").is_err());
        assert!(Settings::parse("step_px = 0.01").is_err());
        assert!(Settings::parse("min_speed = 300\nmax_speed = 200").is_err());
        assert!(Settings::parse("sparks = ").is_err());
        assert!(Settings::parse("hold_seconds = 1e300").is_err());
        assert!(Settings::parse("fade_seconds = -1").is_err());
    }

    #[test]
    fn unknown_names_are_warned_about() {
        let s = Settings::parse("systems = [\"spirals\", \"lorenz\"]").unwrap();
        let (c, w) = s.choices();
        assert_eq!(c.systems, vec![SystemKind::Spirals]);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn missing_explicit_file_is_reported() {
        let (s, warning) = Settings::load(Some(Path::new("/nonexistent/tan.toml")));
        assert_eq!(s, Settings::default());
        assert!(warning.is_some());
    }
}
