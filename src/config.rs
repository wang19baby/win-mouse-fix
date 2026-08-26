use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::remap::RemapEntry;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub general: GeneralConfig,
    pub scroll: ScrollConfig,
    pub buttons: ButtonsConfig,
    #[serde(default)]
    pub drag: DragConfig,
    #[serde(default)]
    pub accel: AccelConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeneralConfig {
    /// Start without showing any window (tray-only). Reserved for future UI.
    pub start_hidden: bool,
    /// Optional path for a rolling log file. When None, only stderr is used.
    pub log_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScrollConfig {
    pub enabled: bool,
    /// Smooth (inertial) scrolling instead of raw wheel deltas.
    pub smooth: bool,
    /// Scroll speed multiplier.
    pub speed: f64,
    /// Invert vertical scroll direction.
    pub invert: bool,
    /// Double-exponential smoothing: level (data) factor in [0,1]. 1 = no smoothing.
    pub smooth_level: f64,
    /// Double-exponential smoothing: trend factor in [0,1]. Controls momentum smoothness.
    pub smooth_trend: f64,
    /// Inertia decay per ~16ms tick in [0,1]. Lower = shorter coast.
    pub friction: f64,
    /// Scroll speed multiplier while Shift is held (MMF-style Shift-to-accelerate).
    pub shift_speedup: f64,
    /// While Shift is held, swap the scroll axis (vertical wheel -> horizontal).
    pub shift_horizontal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ButtonsConfig {
    pub enabled: bool,
    /// Per-button remappings. See [`crate::remap`].
    #[serde(default)]
    pub remaps: Vec<RemapEntry>,
}

/// Window-drag gesture settings (MMF-style Space-drag, or a chosen button).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DragConfig {
    /// Enable window-drag gestures.
    pub enabled: bool,
    /// Button that initiates a drag: "left" (Space-armed, MMF-style),
    /// "middle", "x1", or "x2" (three-finger-style, no Space needed).
    pub button: String,
}

impl Default for DragConfig {
    fn default() -> Self {
        DragConfig {
            enabled: false,
            button: "left".to_string(),
        }
    }
}

/// Pointer-acceleration settings (macOS / mouse-precision style).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccelConfig {
    /// Enable pointer acceleration.
    pub enabled: bool,
    /// Curve knee: higher = acceleration engages at higher speeds.
    pub sensitivity: f64,
    /// Multiplier at low speed (<=1 for finer slow control).
    pub min_factor: f64,
    /// Multiplier at high speed.
    pub max_factor: f64,
}

impl Default for AccelConfig {
    fn default() -> Self {
        AccelConfig {
            enabled: false,
            sensitivity: 1000.0,
            min_factor: 1.0,
            max_factor: 2.0,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            general: GeneralConfig {
                start_hidden: true,
                log_path: None,
            },
            scroll: ScrollConfig {
                enabled: false,
                smooth: true,
                speed: 1.0,
                invert: false,
                smooth_level: 0.6,
                smooth_trend: 0.35,
                friction: 0.88,
                shift_speedup: 1.0,
                shift_horizontal: false,
            },
            buttons: ButtonsConfig {
                enabled: false,
                remaps: Vec::new(),
            },
            drag: DragConfig {
                enabled: false,
                button: "left".to_string(),
            },
            accel: AccelConfig::default(),
        }
    }
}

impl Config {
    /// Load `config.toml` from the executable's directory, or write defaults if absent.
    pub fn load_or_default() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => match toml::from_str::<Config>(&s) {
                Ok(c) => c,
                Err(e) => {
                    crate::log::write(&format!("config parse error ({e}); using defaults"));
                    Config::default()
                }
            },
            Err(_) => {
                let c = Config::default();
                if let Ok(s) = toml::to_string_pretty(&c) {
                    let _ = std::fs::write(&path, s);
                }
                c
            }
        }
    }

    /// Persist this config to the executable-directory `config.toml`.
    pub fn save(&self) -> std::io::Result<()> {
        let s = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(config_path(), s)
    }
}

pub fn config_path() -> PathBuf {
    let mut p = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    p.push("config.toml");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrips_through_toml() {
        let toml = toml::to_string_pretty(&Config::default()).unwrap();
        let parsed: Config = toml::from_str(&toml).unwrap();
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn parses_custom_config() {
        let doc = r#"
[general]
start_hidden = false
log_path = "run.log"

[scroll]
enabled = true
smooth = true
speed = 2.5
invert = true
smooth_level = 0.6
smooth_trend = 0.35
friction = 0.88
shift_speedup = 1.0
shift_horizontal = false

[buttons]
enabled = true

[[buttons.remaps]]
source = "middle"
action = "key"
vk = 32

[[buttons.remaps]]
source = "x1"
action = "disabled"
"#;
        let cfg: Config = toml::from_str(doc).unwrap();
        assert!(!cfg.general.start_hidden);
        assert_eq!(cfg.general.log_path.as_deref(), Some("run.log"));
        assert!(cfg.scroll.enabled);
        assert_eq!(cfg.scroll.speed, 2.5);
        assert!(cfg.scroll.invert);
        assert_eq!(cfg.scroll.smooth_level, 0.6);
        assert_eq!(cfg.scroll.smooth_trend, 0.35);
        assert_eq!(cfg.scroll.friction, 0.88);
        assert_eq!(cfg.scroll.shift_speedup, 1.0);
        assert!(!cfg.scroll.shift_horizontal);
        assert!(cfg.buttons.enabled);
        assert_eq!(cfg.buttons.remaps.len(), 2);
        assert_eq!(cfg.buttons.remaps[0].source, crate::remap::MouseButton::Middle);
        assert_eq!(cfg.buttons.remaps[0].vk, Some(32));
    }
}
