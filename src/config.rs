use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use toml::{Table, Value};
use crate::remap::{LegacyRemapEntry, RemapEntry};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: GeneralConfig,
    pub scroll: ScrollConfig,
    pub buttons: ButtonsConfig,
    #[serde(default)]
    pub drag: DragConfig,
    #[serde(default)]
    pub accel: AccelConfig,
    #[serde(default)]
    pub dpi: DpiConfig,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

impl PartialEq for Config {
    fn eq(&self, other: &Self) -> bool {
        self.general == other.general
            && self.scroll == other.scroll
            && self.buttons.enabled == other.buttons.enabled
            && self.drag == other.drag
                    && self.accel == other.accel
        && self.dpi == other.dpi
        && self.profiles == other.profiles
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeneralConfig {
    pub start_hidden: bool,
    pub log_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScrollConfig {
    pub enabled: bool,
    pub smooth: bool,
    pub speed: f64,
    pub invert: bool,
    #[serde(default = "default_scroll_step")]
    pub step: f64,
    #[serde(default = "default_drag_exponent")]
    pub drag_exponent: f64,
    #[serde(default = "default_drag_coefficient")]
    pub drag_coefficient: f64,
    #[serde(default = "default_stop_speed")]
    pub stop_speed: f64,
    pub shift_speedup: f64,
    pub shift_horizontal: bool,
    // ── Smoothing params ──────────────────────────────────────────────────────
    #[serde(default = "default_time_smoothing_weight")]
    pub time_smoothing_weight: f64,
    #[serde(default = "default_velocity_a")]
    pub velocity_a: f64,
    #[serde(default = "default_velocity_y")]
    pub velocity_y: f64,
    // ── Tick-interval params ─────────────────────────────────────────────────
    #[serde(default = "default_tick_interval_min")]
    pub tick_interval_min: f64,
    #[serde(default = "default_tick_interval_max")]
    pub tick_interval_max: f64,
    #[serde(default = "default_tick_interval_accel_end")]
    pub tick_interval_accel_end: f64,
    #[serde(default = "default_swipe_max_interval")]
    pub swipe_max_interval: f64,
    #[serde(default = "default_swipe_min_tick_speed")]
    pub swipe_min_tick_speed: f64,
    #[serde(default = "default_swipe_threshold")]
    pub swipe_threshold: u32,
    #[serde(default = "default_swipe_max_ticks")]
    pub swipe_max_ticks: u32,
    // ── Acceleration curve params ────────────────────────────────────────────
    #[serde(default = "default_accel_x_min")]
    pub accel_x_min: f64,
    #[serde(default = "default_accel_x_max")]
    pub accel_x_max: f64,
    #[serde(default = "default_accel_y_min")]
    pub accel_y_min: f64,
    #[serde(default = "default_accel_y_max")]
    pub accel_y_max: f64,
    #[serde(default = "default_accel_curvature")]
    pub accel_curvature: f64,
    // ── Fast-scroll params ───────────────────────────────────────────────────
    #[serde(default = "default_fast_scroll_threshold")]
    pub fast_scroll_threshold: u32,
    #[serde(default = "default_fast_scroll_initial")]
    pub fast_scroll_initial: f64,
    #[serde(default = "default_fast_scroll_exponential")]
    pub fast_scroll_exponential: f64,
    // ── Animation params ─────────────────────────────────────────────────────
    #[serde(default = "default_base_ms_per_step")]
    pub base_ms_per_step: f64,
    // ── ScrollAnalyzer wiring ───────────────────────────────────────────────
    #[serde(default = "default_smoothness")]
    pub smoothness: u8,
    #[serde(default)]
    pub precise: bool,
}

fn default_scroll_step() -> f64 { 120.0 }
fn default_drag_exponent() -> f64 { 1.05 }
fn default_drag_coefficient() -> f64 { 15.0 }
fn default_stop_speed() -> f64 { 30.0 }
fn default_time_smoothing_weight() -> f64 { 0.5 }
fn default_velocity_a() -> f64 { 1.0 }
fn default_velocity_y() -> f64 { 1.0 }
fn default_tick_interval_min() -> f64 { 0.001 }
fn default_tick_interval_max() -> f64 { 0.160 }
fn default_tick_interval_accel_end() -> f64 { 0.015 }
fn default_swipe_max_interval() -> f64 { 0.375 }
fn default_swipe_min_tick_speed() -> f64 { 16.0 }
fn default_swipe_threshold() -> u32 { 2 }
fn default_swipe_max_ticks() -> u32 { 11 }
fn default_accel_x_min() -> f64 { 6.25 }
fn default_accel_x_max() -> f64 { 66.667 }
fn default_accel_y_min() -> f64 { 30.0 }
fn default_accel_y_max() -> f64 { 120.0 }
fn default_accel_curvature() -> f64 { 3.0 }
fn default_fast_scroll_threshold() -> u32 { 3 }
fn default_fast_scroll_initial() -> f64 { 1.33 }
fn default_fast_scroll_exponential() -> f64 { 7.5 }
fn default_base_ms_per_step() -> f64 { -1.0 }
fn default_smoothness() -> u8 { 3 }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ButtonsConfig {
    pub enabled: bool,
    /// Legacy remap entries (source → target/action). Used for backward compat.
    #[serde(default)]
    pub remaps: Vec<LegacyRemapEntry>,
    /// Advanced remap entries with trigger/effect and modifier support.
    ///
    /// ```toml
    /// [[buttons.advanced]]
    /// trigger = { type = "button", button = "right", level = 1, duration = "click" }
    /// effect = { type = "symbolic_hotkey", keycode = 0x23, flags = 0x800 }  # Alt+Tab
    ///
    /// [[buttons.advanced]]
    /// modifiers = { keyboard = 0x200 }  # Shift held
    /// trigger = { type = "button", button = "right", level = 2, duration = "hold" }
    /// effect = { type = "navigation_swipe", direction = "forward" }
    ///
    /// [[buttons.advanced]]
    /// modifiers = { keyboard = 0x100 }  # Ctrl held
    /// trigger = { type = "scroll" }
    /// effect = { type = "modified_scroll", modification = "precision" }
    ///
    /// [[buttons.advanced]]
    /// trigger = { type = "drag" }
    /// effect = { type = "modified_drag", drag_type = "two_finger_swipe" }
    /// ```
    #[serde(default)]
    pub advanced: Vec<RemapEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DragConfig {
    pub enabled: bool,
    /// Button that initiates the drag gesture.
    pub button: String,
    /// What a button-drag produces. `move` = drag the window under the cursor
    /// (default, Windows-style); `scroll` = feed pointer delta into the
    /// smooth-scroll injector (mac drag-to-scroll); `navigate` = horizontal
    /// swipe fires browser back/forward. Mirrors mac `Core/Drag/` output modes.
    #[serde(default = "default_drag_mode")]
    pub mode: String,
}

fn default_drag_mode() -> String {
    "move".to_string()
}

impl Default for DragConfig {
    fn default() -> Self {
        DragConfig {
            enabled: false,
            button: "left".to_string(),
            mode: "move".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccelConfig {
    pub enabled: bool,
    pub sensitivity: f64,
    pub min_factor: f64,
    pub max_factor: f64,
}

impl Default for AccelConfig {
    fn default() -> Self {
        AccelConfig { enabled: false, sensitivity: 1000.0, min_factor: 1.0, max_factor: 2.0 }
    }
}

/// DPI cross-screen auto-switch configuration (Phase 10).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DpiConfig {
    /// Auto-adjust hardware DPI when the cursor enters a differently-scaled monitor.
    #[serde(default = "default_dpi_auto_switch")]
    pub auto_switch: bool,
    /// Hardware DPI at the reference (anchor) monitor; target scales from this.
    #[serde(default = "default_dpi_base")]
    pub base_dpi: u16,
    /// Hard limits for the hardware DPI the device can accept.
    #[serde(default = "default_dpi_min")]
    pub min_dpi: u16,
    #[serde(default = "default_dpi_max")]
    pub max_dpi: u16,
}

fn default_dpi_auto_switch() -> bool { true }
fn default_dpi_base() -> u16 { 800 }
fn default_dpi_min() -> u16 { 200 }
fn default_dpi_max() -> u16 { 4000 }

impl Default for DpiConfig {
    fn default() -> Self {
        DpiConfig {
            auto_switch: default_dpi_auto_switch(),
            base_dpi: default_dpi_base(),
            min_dpi: default_dpi_min(),
            max_dpi: default_dpi_max(),
        }
    }
}

/// A per-app (or, in a future phase, per-device) configuration override.
///
/// When the foreground window's exe name contains `match_exe` (case-insensitive
/// substring), `config` is deep-merged over the base config. `match_type` is
/// currently only `"exe"`; `"device"` is parsed but ignored until the device
/// layer (Phase 8) lands.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    #[serde(default = "default_match_type")]
    pub match_type: String,
    #[serde(default)]
    pub match_exe: Option<String>,
    #[serde(default)]
    pub match_device: Option<String>,
    /// Partial `Config` override, expressed as a TOML table so only the keys
    /// the user sets are merged.
    #[serde(default)]
    pub config: Table,
}

fn default_match_type() -> String {
    "exe".to_string()
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            match_type: "exe".to_string(),
            match_exe: None,
            match_device: None,
            config: Table::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            general: GeneralConfig { start_hidden: true, log_path: None },
            scroll: ScrollConfig {
                enabled: false, smooth: true, speed: 1.0, invert: false,
                step: 120.0, drag_exponent: 1.05, drag_coefficient: 15.0,
                stop_speed: 30.0, shift_speedup: 1.0, shift_horizontal: false,
                time_smoothing_weight: 0.5, velocity_a: 1.0, velocity_y: 1.0,
                tick_interval_min: 0.001, tick_interval_max: 0.160,
                tick_interval_accel_end: 0.015, swipe_max_interval: 0.375,
                swipe_min_tick_speed: 16.0, swipe_threshold: 2, swipe_max_ticks: 11,
                accel_x_min: 6.25, accel_x_max: 66.667,
                accel_y_min: 30.0, accel_y_max: 120.0, accel_curvature: 3.0,
                fast_scroll_threshold: 3, fast_scroll_initial: 1.33,
                fast_scroll_exponential: 7.5, base_ms_per_step: -1.0,
                smoothness: 3, precise: false,
            },
            buttons: ButtonsConfig::default(),
            drag: DragConfig::default(),
            accel: AccelConfig::default(),
            dpi: DpiConfig::default(),
            profiles: Vec::new(),
        }
    }
}

impl Config {
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

    pub fn save(&self) -> std::io::Result<()> {
        let s = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(config_path(), s)
    }

    /// Merge the first `exe`-type profile whose `match_exe` is a substring of
    /// `exe` (case-insensitive) over `self`. `exe` is the foreground exe basename
    /// (or `None` for base-only). Pure and testable.
    pub fn resolve_for_exe(&self, exe: Option<&str>) -> Config {
        let key = exe.map(|e| e.to_ascii_lowercase());
        if let Some(p) = self.profiles.iter().find(|p| profile_matches_exe(p, &key)) {
            merge_config(self, &p.config)
        } else {
            self.clone()
        }
    }
}

/// Returns true if `p` (an `exe`-type profile) matches the foreground exe `key`.
fn profile_matches_exe(p: &Profile, key: &Option<String>) -> bool {
    if p.match_type != "exe" {
        return false;
    }
    let m = match &p.match_exe {
        Some(m) => m.to_ascii_lowercase(),
        None => return false,
    };
    match key {
        Some(k) => k.contains(&m),
        None => false,
    }
}

/// Deep-merge `src` into `dst` (recursing into nested tables).
fn deep_merge(dst: &mut Table, src: &Table) {
    for (k, v) in src {
        match (dst.get_mut(k), v) {
            (Some(Value::Table(d)), Value::Table(s)) => deep_merge(d, s),
            _ => {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Serialize `base` to TOML, deep-merge `over`, deserialize back to `Config`.
/// Falls back to a clone of `base` on any serialization error.
fn merge_config(base: &Config, over: &Table) -> Config {
    let base_str = match toml::to_string(base) {
        Ok(s) => s,
        Err(_) => return base.clone(),
    };
    let mut base_table: Table = match toml::from_str(&base_str) {
        Ok(t) => t,
        Err(_) => return base.clone(),
    };
    deep_merge(&mut base_table, over);
    let merged_str = match toml::to_string(&base_table) {
        Ok(s) => s,
        Err(_) => return base.clone(),
    };
    toml::from_str(&merged_str).unwrap_or_else(|_| base.clone())
}

pub fn config_path() -> PathBuf {
    let mut p = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    p.push("config.toml");
    p
}

/// Returns true if `path`'s mtime changed since `last` was recorded, updating `last`.
/// Missing file -> false (avoids reload loops). Pure and testable.
pub fn config_changed(path: &Path, last: &mut Option<SystemTime>) -> bool {
    match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => {
            if *last != Some(t) {
                *last = Some(t);
                true
            } else {
                false
            }
        }
        Err(_) => false,
    }
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
        assert_eq!(cfg.scroll.shift_speedup, 1.0);
        assert!(!cfg.scroll.shift_horizontal);
        assert!(cfg.buttons.enabled);
        assert_eq!(cfg.buttons.remaps.len(), 2);
        assert_eq!(cfg.buttons.remaps[0].source, crate::remap::MouseButton::Middle);
        assert_eq!(cfg.buttons.remaps[0].vk, Some(32));
    }

    #[test]
    fn config_changed_detects_creation_and_stability() {
        let p = std::env::temp_dir().join(format!("wmf_cfg_test_{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut last: Option<SystemTime> = None;
        assert!(!config_changed(&p, &mut last), "missing file must not trigger reload");
        std::fs::write(&p, "[general]\nstart_hidden = true\n").unwrap();
        assert!(config_changed(&p, &mut last), "new file must trigger reload");
        assert!(!config_changed(&p, &mut last), "unchanged file must not retrigger");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn resolve_for_exe_merges_matching_profile_and_keeps_base_otherwise() {
        let mut base = Config::default();
        base.scroll.enabled = true;
        let mut scroll = Table::new();
        scroll.insert("enabled".into(), Value::Boolean(false));
        let mut over = Table::new();
        over.insert("scroll".into(), Value::Table(scroll));
        base.profiles.push(Profile {
            match_type: "exe".to_string(),
            match_exe: Some("explorer".to_string()),
            match_device: None,
            config: over,
        });

        // Matching exe (case-insensitive) → override applied.
        let m = base.resolve_for_exe(Some("EXPLORER.EXE"));
        assert!(!m.scroll.enabled, "profile override should disable scroll");
        // Non-matching exe → base retained.
        let n = base.resolve_for_exe(Some("notepad.exe"));
        assert!(n.scroll.enabled, "no matching profile → base retained");
        // None → base retained.
        let b = base.resolve_for_exe(None);
        assert!(b.scroll.enabled);
        // Merge is deep: sibling config is untouched.
        assert_eq!(b.drag, base.drag);
        assert_eq!(b.buttons.enabled, base.buttons.enabled);
    }

    #[test]
    fn parses_dpi_section_with_explicit_values() {
        let doc = r#"
auto_switch = false
base_dpi = 1000
min_dpi = 400
max_dpi = 3200
"#;
        let cfg: DpiConfig = toml::from_str(doc).unwrap();
        assert!(!cfg.auto_switch);
        assert_eq!(cfg.base_dpi, 1000);
        assert_eq!(cfg.min_dpi, 400);
        assert_eq!(cfg.max_dpi, 3200);
    }

    #[test]
    fn dpi_config_defaults() {
        let d = DpiConfig::default();
        assert!(d.auto_switch);
        assert_eq!(d.base_dpi, 800);
        assert_eq!(d.min_dpi, 200);
        assert_eq!(d.max_dpi, 4000);
    }
}
