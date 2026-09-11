use crate::remap::{LegacyRemapEntry, RemapEntry};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;
use toml::{Table, Value};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

/// Caches the last successfully parsed configuration. A transient parse error
/// (e.g. a typo while editing `config.toml`) must NOT silently reset every
/// feature back to defaults — we keep serving the previous valid config until
/// the file parses again. Initialized on first good load.
static LAST_GOOD: OnceLock<Mutex<Config>> = OnceLock::new();

fn store_good(c: &Config) {
    let slot = LAST_GOOD.get_or_init(|| Mutex::new(Config::default()));
    *slot.lock() = c.clone();
}

fn last_good_or_default() -> Config {
    match LAST_GOOD.get() {
        Some(slot) => slot.lock().clone(),
        None => Config::default(),
    }
}

/// Four control points of a 1-D Bezier curve (degree 3).
///
/// Stored as a flat `[x0, y0, x1, y1, x2, y2, x3, y3]` array in TOML so users
/// can edit values without dealing with nested tables. The x-coordinates must
/// be monotonically increasing in [0, 1]; the y-coordinates form the curve's
/// output range. The construction helpers in
/// [`crate::scroll::curve::BezierAccelCurve::from_points`] validate these
/// invariants.
///
/// Used by PR-B for the Shift-hold-to-scroll-speed curve: x = normalised
/// hold time, y = scroll-speed multiplier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BezierControlPoints {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub x3: f64,
    pub y3: f64,
}

impl BezierControlPoints {
    /// Borrow the points as a `[(f64, f64); 4]` slice in (x, y) order, suitable
    /// for `BezierAccelCurve::from_points`.
    pub fn as_point_pairs(&self) -> [(f64, f64); 4] {
        [
            (self.x0, self.y0),
            (self.x1, self.y1),
            (self.x2, self.y2),
            (self.x3, self.y3),
        ]
    }

    /// Default Shift accelerator curve: light press → 1.0×, mid press → 1.8×,
    /// heavy press → 6.0×. Designed so casual Shift-tap behaves normally while
    /// sustained Shift accelerates aggressively.
    pub fn default_shift_speedup() -> Self {
        Self {
            x0: 0.0,
            y0: 1.0,
            x1: 0.25,
            y1: 1.8,
            x2: 0.7,
            y2: 4.0,
            x3: 1.0,
            y3: 6.0,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    pub remote: RemoteConfig,
    #[serde(default)]
    pub touch: TouchConfig,
    #[serde(default)]
    pub profiles: Vec<Profile>,
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
    #[serde(default = "default_shift_speedup")]
    pub shift_speedup: f64,
    #[serde(default)]
    pub shift_horizontal: bool,
    // ── Shift nonlinear accelerator (PR-B) ──────────────────────────────────
    /// Hold-time Bezier curve: 4 control points (x0,y0,x1,y1,x2,y2,x3,y3).
    /// x ∈ [0, 1] is normalized hold time (0 = just pressed, 1 = max hold);
    /// y is the scroll speed multiplier. Ignored when `shift_speedup_linear = true`.
    /// `None` ⇒ use the default curve.
    #[serde(default = "default_shift_speedup_curve")]
    pub shift_speedup_curve: Option<BezierControlPoints>,
    /// Seconds of holding Shift before the curve reaches its right endpoint (x = 1).
    /// Beyond this, the curve extends linearly (flat extrapolation).
    #[serde(default = "default_shift_speedup_max_hold")]
    pub shift_speedup_max_hold: f64,
    /// When true, `shift_speedup` (a scalar) is used as a flat multiplier and the
    /// curve is ignored. Preserves the old single-value behaviour.
    #[serde(default)]
    pub shift_speedup_linear: bool,
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

fn default_scroll_step() -> f64 {
    120.0
}
fn default_drag_exponent() -> f64 {
    1.05
}
fn default_drag_coefficient() -> f64 {
    15.0
}
fn default_stop_speed() -> f64 {
    30.0
}
fn default_shift_speedup_curve() -> Option<BezierControlPoints> {
    Some(BezierControlPoints::default_shift_speedup())
}
fn default_shift_speedup_max_hold() -> f64 {
    0.6
}
fn default_time_smoothing_weight() -> f64 {
    0.5
}
fn default_velocity_a() -> f64 {
    1.0
}
fn default_velocity_y() -> f64 {
    1.0
}
fn default_tick_interval_min() -> f64 {
    0.001
}
fn default_tick_interval_max() -> f64 {
    0.160
}
fn default_tick_interval_accel_end() -> f64 {
    0.015
}
fn default_swipe_max_interval() -> f64 {
    0.375
}
fn default_swipe_min_tick_speed() -> f64 {
    16.0
}
fn default_swipe_threshold() -> u32 {
    2
}
fn default_swipe_max_ticks() -> u32 {
    11
}
fn default_accel_x_min() -> f64 {
    6.25
}
fn default_accel_x_max() -> f64 {
    66.667
}
fn default_accel_y_min() -> f64 {
    30.0
}
fn default_accel_y_max() -> f64 {
    120.0
}
fn default_accel_curvature() -> f64 {
    3.0
}
fn default_fast_scroll_threshold() -> u32 {
    3
}
fn default_fast_scroll_initial() -> f64 {
    1.33
}
fn default_fast_scroll_exponential() -> f64 {
    7.5
}
fn default_base_ms_per_step() -> f64 {
    -1.0
}
fn default_smoothness() -> u8 {
    3
}
fn default_shift_speedup() -> f64 {
    1.0
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ButtonsConfig {
    pub enabled: bool,
    /// Enable the middle-button window-switcher gesture (double-click middle to
    /// open Alt+Tab, wheel to navigate, middle again to confirm).
    #[serde(default)]
    pub window_switcher: bool,
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
        AccelConfig {
            enabled: false,
            sensitivity: 1000.0,
            min_factor: 1.0,
            max_factor: 2.0,
        }
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

fn default_dpi_auto_switch() -> bool {
    false
}
fn default_dpi_base() -> u16 {
    800
}
fn default_dpi_min() -> u16 {
    200
}
fn default_dpi_max() -> u16 {
    4000
}

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

/// Phone-trackpad (Phase 11) LAN bridge. Off by default so the app does not
/// open a network listener / firewall rule unless the user opts in.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoteConfig {
    #[serde(default = "default_remote_enabled")]
    pub enabled: bool,
    #[serde(default = "default_remote_port")]
    pub port: u16,
    #[serde(default = "default_large_screen_split")]
    pub large_screen_split: bool,
    #[serde(default = "default_split_ratio")]
    pub split_ratio: f64,
}

fn default_remote_enabled() -> bool {
    false
}
fn default_remote_port() -> u16 {
    18765
}
fn default_large_screen_split() -> bool {
    true
}
fn default_split_ratio() -> f64 {
    0.4
}

impl Default for RemoteConfig {
    fn default() -> Self {
        RemoteConfig {
            enabled: default_remote_enabled(),
            port: default_remote_port(),
            large_screen_split: default_large_screen_split(),
            split_ratio: default_split_ratio(),
        }
    }
}

/// Touch-trackpad parameters. Sent to the phone via WS status message so the
/// web page can use server-configured values instead of hardcoded constants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TouchConfig {
    #[serde(default = "default_touch_gain")]
    pub gain: f64,
    #[serde(default = "default_touch_accel_ref")]
    pub accel_ref: f64,
    #[serde(default = "default_touch_accel_slope")]
    pub accel_slope: f64,
    #[serde(default = "default_touch_accel_max_mult")]
    pub accel_max_mult: f64,
    #[serde(default = "default_touch_move_ema")]
    pub move_ema: f64,
    #[serde(default = "default_touch_scroll_gain")]
    pub scroll_gain: f64,
    #[serde(default = "default_touch_tap_ms")]
    pub tap_ms: u32,
    #[serde(default = "default_touch_tap_px")]
    pub tap_px: f64,
    #[serde(default = "default_touch_swipe_px")]
    pub swipe_px: f64,
    #[serde(default = "default_touch_decide_px")]
    pub decide_px: f64,
    #[serde(default = "default_touch_pinch_bias")]
    pub pinch_bias: f64,
    #[serde(default = "default_touch_diag_min")]
    pub diag_min: f64,
    #[serde(default = "default_touch_diag_ratio")]
    pub diag_ratio: f64,
    #[serde(default = "default_touch_longpress_ms")]
    pub longpress_ms: u32,
}

fn default_touch_gain() -> f64 {
    6.7
}
fn default_touch_accel_ref() -> f64 {
    10.0
}
fn default_touch_accel_slope() -> f64 {
    10.0
}
fn default_touch_accel_max_mult() -> f64 {
    2.0
}
fn default_touch_move_ema() -> f64 {
    0.35
}
fn default_touch_scroll_gain() -> f64 {
    5.5
}
fn default_touch_tap_ms() -> u32 {
    220
}
fn default_touch_tap_px() -> f64 {
    10.0
}
fn default_touch_swipe_px() -> f64 {
    45.0
}
fn default_touch_decide_px() -> f64 {
    12.0
}
fn default_touch_pinch_bias() -> f64 {
    1.5
}
fn default_touch_diag_min() -> f64 {
    20.0
}
fn default_touch_diag_ratio() -> f64 {
    1.6
}
fn default_touch_longpress_ms() -> u32 {
    400
}

impl Default for TouchConfig {
    fn default() -> Self {
        TouchConfig {
            gain: default_touch_gain(),
            accel_ref: default_touch_accel_ref(),
            accel_slope: default_touch_accel_slope(),
            accel_max_mult: default_touch_accel_max_mult(),
            move_ema: default_touch_move_ema(),
            scroll_gain: default_touch_scroll_gain(),
            tap_ms: default_touch_tap_ms(),
            tap_px: default_touch_tap_px(),
            swipe_px: default_touch_swipe_px(),
            decide_px: default_touch_decide_px(),
            pinch_bias: default_touch_pinch_bias(),
            diag_min: default_touch_diag_min(),
            diag_ratio: default_touch_diag_ratio(),
            longpress_ms: default_touch_longpress_ms(),
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
            general: GeneralConfig {
                start_hidden: true,
                log_path: None,
            },
            scroll: ScrollConfig {
                enabled: false,
                smooth: true,
                speed: 1.0,
                invert: false,
                step: 120.0,
                drag_exponent: 1.05,
                drag_coefficient: 15.0,
                stop_speed: 30.0,
                shift_speedup: 1.0,
                shift_horizontal: false,
                shift_speedup_curve: Some(BezierControlPoints::default_shift_speedup()),
                shift_speedup_max_hold: 0.6,
                shift_speedup_linear: false,
                time_smoothing_weight: 0.5,
                velocity_a: 1.0,
                velocity_y: 1.0,
                tick_interval_min: 0.001,
                tick_interval_max: 0.160,
                tick_interval_accel_end: 0.015,
                swipe_max_interval: 0.375,
                swipe_min_tick_speed: 16.0,
                swipe_threshold: 2,
                swipe_max_ticks: 11,
                accel_x_min: 6.25,
                accel_x_max: 66.667,
                accel_y_min: 30.0,
                accel_y_max: 120.0,
                accel_curvature: 3.0,
                fast_scroll_threshold: 3,
                fast_scroll_initial: 1.33,
                fast_scroll_exponential: 7.5,
                base_ms_per_step: -1.0,
                smoothness: 3,
                precise: false,
            },
            buttons: ButtonsConfig::default(),
            drag: DragConfig::default(),
            accel: AccelConfig::default(),
            dpi: DpiConfig::default(),
            remote: RemoteConfig::default(),
            touch: TouchConfig::default(),
            profiles: Vec::new(),
        }
    }
}

impl Config {
    pub fn load_or_default() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => match toml::from_str::<Config>(&s) {
                Ok(c) => {
                    store_good(&c);
                    c
                }
                Err(e) => {
                    crate::log::write(&format!(
                        "config parse error ({e}); keeping previous valid config"
                    ));
                    last_good_or_default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let c = Config::default();
                if let Ok(s) = toml::to_string_pretty(&c) {
                    let _ = atomic_write(&path, &s);
                }
                store_good(&c);
                c
            }
            // A file exists but couldn't be read/parsed: keep the previous valid
            // config WITHOUT overwriting the user's file (that would destroy it).
            Err(_) => last_good_or_default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let s = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        atomic_write(&config_path(), &s)?;
        store_good(self);
        Ok(())
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

fn staging_path(path: &Path) -> PathBuf {
    let mut staged = path.as_os_str().to_os_string();
    staged.push(".tmp");
    PathBuf::from(staged)
}

/// Replace a config in one filesystem operation so the reload timer can never
/// observe a partially-written TOML document.
pub(crate) fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    let staged = staging_path(path);
    let write_result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&staged)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }

    let staged_wide: Vec<u16> = staged
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let path_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let moved = unsafe {
        MoveFileExW(
            staged_wide.as_ptr(),
            path_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        let error = std::io::Error::last_os_error();
        let _ = std::fs::remove_file(staged);
        return Err(error);
    }
    Ok(())
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
    fn config_equality_detects_every_button_mapping_change() {
        let base = Config::default();

        let mut changed = base.clone();
        changed.buttons.window_switcher = !changed.buttons.window_switcher;
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.buttons.remaps.push(LegacyRemapEntry {
            source: crate::remap::MouseButton::X1,
            action: "disabled".to_string(),
            target: None,
            vk: None,
        });
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.buttons.advanced.push(RemapEntry {
            modifiers: crate::remap::ModifierCondition::default(),
            trigger: crate::remap::Trigger::Scroll,
            effect: crate::remap::Effect::TaskView,
        });
        assert_ne!(base, changed);
    }

    #[test]
    fn keeps_last_good_config_on_parse_failure() {
        // A transient parse error must keep serving the previous valid config
        // instead of silently resetting everything to defaults.
        let mut good = Config::default();
        good.scroll.speed = 9.9;
        store_good(&good);
        let kept = last_good_or_default();
        assert_eq!(kept.scroll.speed, 9.9);
    }

    #[test]
    fn atomic_config_replace_is_complete_and_failure_safe() {
        let unique = format!(
            "win-mouse-fix-config-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::write(&path, "old").unwrap();

        atomic_write(&path, "new").unwrap();
        let replaced = std::fs::read_to_string(&path).unwrap();

        let staged = staging_path(&path);
        std::fs::create_dir(&staged).unwrap();
        let failed = atomic_write(&path, "partial");
        let preserved = std::fs::read_to_string(&path).unwrap();

        std::fs::remove_dir(staged).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(replaced, "new");
        assert!(failed.is_err());
        assert_eq!(preserved, "new");
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
        assert_eq!(
            cfg.buttons.remaps[0].source,
            crate::remap::MouseButton::Middle
        );
        assert_eq!(cfg.buttons.remaps[0].vk, Some(32));
    }

    #[test]
    fn config_changed_detects_creation_and_stability() {
        let p = std::env::temp_dir().join(format!("wmf_cfg_test_{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut last: Option<SystemTime> = None;
        assert!(
            !config_changed(&p, &mut last),
            "missing file must not trigger reload"
        );
        std::fs::write(&p, "[general]\nstart_hidden = true\n").unwrap();
        assert!(
            config_changed(&p, &mut last),
            "new file must trigger reload"
        );
        assert!(
            !config_changed(&p, &mut last),
            "unchanged file must not retrigger"
        );
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
    fn shift_curve_default_roundtrips_through_toml() {
        // Default Config has the shift curve baked in; verify it survives
        // a TOML → Config round-trip without explicit user input.
        let toml = toml::to_string_pretty(&Config::default()).unwrap();
        let parsed: Config = toml::from_str(&toml).unwrap();
        let pts = parsed
            .scroll
            .shift_speedup_curve
            .as_ref()
            .expect("default curve must be Some");
        assert_eq!(pts.x0, 0.0);
        assert_eq!(pts.y0, 1.0);
        assert_eq!(pts.x3, 1.0);
        assert_eq!(pts.y3, 6.0);
        assert!(!parsed.scroll.shift_speedup_linear);
        assert!((parsed.scroll.shift_speedup_max_hold - 0.6).abs() < 1e-9);
    }

    #[test]
    fn shift_curve_accepts_flat_array_form() {
        // The curve must be deserializable from a flat 8-element array
        // (user-facing TOML form), not just a nested table.
        let doc = r#"
[general]
start_hidden = false

[scroll]
enabled = true
smooth = true
speed = 1.0
invert = false
shift_speedup = 1.0
shift_horizontal = false
shift_speedup_curve = [0.0, 1.0, 0.5, 2.0, 0.8, 5.0, 1.0, 8.0]
shift_speedup_max_hold = 0.4
shift_speedup_linear = true

[buttons]
enabled = false
"#;
        let cfg: Config = toml::from_str(doc).unwrap();
        let pts = cfg.scroll.shift_speedup_curve.as_ref().unwrap();
        assert_eq!(pts.x1, 0.5);
        assert_eq!(pts.y1, 2.0);
        assert_eq!(pts.y3, 8.0);
        assert!(cfg.scroll.shift_speedup_linear);
        assert!((cfg.scroll.shift_speedup_max_hold - 0.4).abs() < 1e-9);
    }

    #[test]
    fn shift_curve_missing_uses_default() {
        // A user with an old config.toml that predates PR-B must still parse
        // successfully — new fields fall back to defaults.
        let doc = r#"
[general]
start_hidden = false

[scroll]
enabled = true
smooth = true
speed = 1.0
invert = false
shift_speedup = 2.0
shift_horizontal = false

[buttons]
enabled = false
"#;
        let cfg: Config = toml::from_str(doc).unwrap();
        assert!(
            cfg.scroll.shift_speedup_curve.is_some(),
            "missing curve must default to Some(default_shift_speedup)"
        );
        assert!(
            !cfg.scroll.shift_speedup_linear,
            "missing linear flag must default to false"
        );
        assert!((cfg.scroll.shift_speedup_max_hold - 0.6).abs() < 1e-9);
    }

    #[test]
    fn bezier_control_points_as_point_pairs() {
        // as_point_pairs must return the four (x, y) tuples in declared order
        // so that BezierAccelCurve::from_points receives a valid input.
        let pts = BezierControlPoints {
            x0: 0.0,
            y0: 1.0,
            x1: 0.25,
            y1: 1.8,
            x2: 0.7,
            y2: 4.0,
            x3: 1.0,
            y3: 6.0,
        };
        let pairs = pts.as_point_pairs();
        assert_eq!(pairs.len(), 4);
        assert_eq!(pairs[0], (0.0, 1.0));
        assert_eq!(pairs[1], (0.25, 1.8));
        assert_eq!(pairs[2], (0.7, 4.0));
        assert_eq!(pairs[3], (1.0, 6.0));
    }

    #[test]
    fn bezier_default_shift_speedup_matches_promised_shape() {
        // The default curve must be monotonic in x and reach y=1.0 at x=0
        // (so casual Shift-tap behaves like a normal scroll).
        let pts = BezierControlPoints::default_shift_speedup();
        assert_eq!(pts.x0, 0.0);
        assert_eq!(pts.y0, 1.0);
        assert!(
            pts.x0 < pts.x1 && pts.x1 < pts.x2 && pts.x2 < pts.x3,
            "x coords must be strictly increasing: {:?}",
            pts
        );
        assert!(
            pts.y0 < pts.y1 && pts.y1 < pts.y2 && pts.y2 < pts.y3,
            "y coords must be strictly increasing: {:?}",
            pts
        );
    }

    #[test]
    fn dpi_config_defaults() {
        let d = DpiConfig::default();
        // Off by default: writing hardware DPI to a device must be an explicit,
        // validated opt-in, never a silent startup side effect.
        assert!(!d.auto_switch);
        assert_eq!(d.base_dpi, 800);
        assert_eq!(d.min_dpi, 200);
        assert_eq!(d.max_dpi, 4000);
    }

    #[test]
    fn source_config_is_valid_and_risky_integrations_are_opt_in() {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let path = std::path::Path::new(&manifest).join("config.toml");
        let content =
            std::fs::read_to_string(&path).expect("config.toml must exist at project root");
        let cfg: Config = toml::from_str(&content).expect("config.toml must parse without errors");

        assert!(!cfg.drag.enabled, "drag gestures can conflict with clicks");
        assert!(
            !cfg.accel.enabled,
            "pointer acceleration is not connected to the move hook"
        );
        assert!(
            !cfg.remote.enabled,
            "the LAN listener requires explicit opt-in"
        );
        assert!(
            !cfg.dpi.auto_switch,
            "hardware DPI writes require explicit opt-in"
        );
    }

    #[test]
    fn touch_config_explicit_values() {
        let doc = r#"
gain = 8.0
accel_ref = 15.0
accel_slope = 12.0
accel_max_mult = 3.0
move_ema = 0.4
scroll_gain = 6.0
tap_ms = 250
tap_px = 12.0
swipe_px = 50.0
decide_px = 14.0
pinch_bias = 2.0
diag_min = 25.0
diag_ratio = 1.8
longpress_ms = 450
"#;
        let cfg: TouchConfig = toml::from_str(doc).unwrap();
        assert!((cfg.gain - 8.0).abs() < 1e-9);
        assert!((cfg.accel_ref - 15.0).abs() < 1e-9);
        assert!((cfg.accel_slope - 12.0).abs() < 1e-9);
        assert!((cfg.accel_max_mult - 3.0).abs() < 1e-9);
        assert!((cfg.move_ema - 0.4).abs() < 1e-9);
        assert!((cfg.scroll_gain - 6.0).abs() < 1e-9);
        assert_eq!(cfg.tap_ms, 250);
        assert!((cfg.tap_px - 12.0).abs() < 1e-9);
        assert!((cfg.swipe_px - 50.0).abs() < 1e-9);
        assert!((cfg.decide_px - 14.0).abs() < 1e-9);
        assert!((cfg.pinch_bias - 2.0).abs() < 1e-9);
        assert!((cfg.diag_min - 25.0).abs() < 1e-9);
        assert!((cfg.diag_ratio - 1.8).abs() < 1e-9);
        assert_eq!(cfg.longpress_ms, 450);
    }

    #[test]
    fn touch_config_defaults_when_missing() {
        // An empty document must parse successfully, producing defaults
        // for every field — critical for users upgrading from older configs.
        let doc = "";
        let cfg: TouchConfig = toml::from_str(doc).unwrap();
        let d = TouchConfig::default();
        assert!((cfg.gain - d.gain).abs() < 1e-9);
        assert!((cfg.accel_ref - d.accel_ref).abs() < 1e-9);
        assert!((cfg.accel_slope - d.accel_slope).abs() < 1e-9);
        assert!((cfg.accel_max_mult - d.accel_max_mult).abs() < 1e-9);
        assert!((cfg.move_ema - d.move_ema).abs() < 1e-9);
        assert!((cfg.scroll_gain - d.scroll_gain).abs() < 1e-9);
        assert_eq!(cfg.tap_ms, d.tap_ms);
        assert!((cfg.tap_px - d.tap_px).abs() < 1e-9);
        assert!((cfg.swipe_px - d.swipe_px).abs() < 1e-9);
        assert!((cfg.decide_px - d.decide_px).abs() < 1e-9);
        assert!((cfg.pinch_bias - d.pinch_bias).abs() < 1e-9);
        assert!((cfg.diag_min - d.diag_min).abs() < 1e-9);
        assert!((cfg.diag_ratio - d.diag_ratio).abs() < 1e-9);
        assert_eq!(cfg.longpress_ms, d.longpress_ms);
    }

    #[test]
    fn touch_config_partial_override_with_defaults() {
        // User sets only new fields; existing fields keep their defaults.
        let doc = r#"
decide_px = 20.0
pinch_bias = 3.0
diag_min = 30.0
diag_ratio = 2.0
longpress_ms = 600
"#;
        let cfg: TouchConfig = toml::from_str(doc).unwrap();
        assert!((cfg.decide_px - 20.0).abs() < 1e-9);
        assert!((cfg.pinch_bias - 3.0).abs() < 1e-9);
        assert!((cfg.diag_min - 30.0).abs() < 1e-9);
        assert!((cfg.diag_ratio - 2.0).abs() < 1e-9);
        assert_eq!(cfg.longpress_ms, 600);
        // Untouched fields remain at defaults
        assert!((cfg.gain - default_touch_gain()).abs() < 1e-9);
        assert_eq!(cfg.tap_ms, default_touch_tap_ms());
    }
}
