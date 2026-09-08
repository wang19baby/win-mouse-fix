//! Global cache of the most recent battery reading.
//!
//! Part of the Phase 8.2 cache design, but introduced now because Phase 9
//! (tray battery display) needs a single shared, lock-free-to-read value that
//! the tray poll refreshes. Held in a `LazyLock<RwLock<…>>` mirroring `CONFIG`.
use parking_lot::RwLock;
use std::sync::LazyLock;

/// Last-known battery state of the primary Logitech device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryInfo {
    pub percent: u8,
    pub charging: bool,
    /// `true` when `percent <= 20` — drives low-battery cues.
    pub low: bool,
}

impl BatteryInfo {
    pub fn from_level(level: u8, charging: bool) -> BatteryInfo {
        BatteryInfo {
            percent: level,
            charging,
            low: level <= 20,
        }
    }
}

pub static BATTERY: LazyLock<RwLock<Option<BatteryInfo>>> = LazyLock::new(|| RwLock::new(None));

/// Last-known DPI state for the screen under the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DpiState {
    /// Effective (logical) DPI of the active monitor.
    pub monitor_dpi: i32,
    /// Hardware DPI the device should be set to for this monitor.
    pub target_hw_dpi: u16,
}

pub static DPI: LazyLock<RwLock<Option<DpiState>>> = LazyLock::new(|| RwLock::new(None));

/// Compute and cache the DPI target for the monitor under the cursor.
/// Returns the fresh value (or `None` if no monitor was found).
pub fn update_dpi(base_dpi: u16, ref_dpi: i32, min_dpi: u16, max_dpi: u16) -> Option<DpiState> {
    let state = crate::device::dpi::cursor_monitor().map(|m| {
        let target = crate::device::dpi::target_dpi(base_dpi, ref_dpi, m.dpi_x, min_dpi, max_dpi);
        DpiState {
            monitor_dpi: m.dpi_x,
            target_hw_dpi: target,
        }
    });
    *DPI.write() = state;
    state
}

/// Re-read the battery from the device and refresh the cache. Returns the
/// fresh value (or `None` if no device responded).
#[allow(dead_code)]
pub fn update() -> Option<BatteryInfo> {
    let info = crate::device::battery::read_first_battery();
    *BATTERY.write() = info;
    info
}
