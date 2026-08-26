use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub general: GeneralConfig,
    pub scroll: ScrollConfig,
    pub buttons: ButtonsConfig,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ButtonsConfig {
    pub enabled: bool,
    // Remap table is added in a later iteration.
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
            },
            buttons: ButtonsConfig { enabled: false },
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

[buttons]
enabled = true
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
        assert!(cfg.buttons.enabled);
    }
}
