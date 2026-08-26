//! Pointer acceleration (macOS / "enhance pointer precision" style).
//!
//! The controller is pure state: it tracks the last cursor position reported
//! by the hook and, per move event, returns an *amplified* relative delta to
//! inject. All Win32 (`SendInput`) side effects live in
//! `crate::scroll::injector::send_mouse_move`, so the math stays testable.

use crate::config::AccelConfig;

/// Map a per-event movement speed (pixels) to a multiplier.
///
/// Returns `min_factor` at rest/slow speed and rises toward `max_factor` as
/// speed grows, saturating via a smooth `speed/(speed+sensitivity)` curve.
/// Pure and monotonic; no Win32, fully testable.
pub fn accel_factor(speed: f64, cfg: &AccelConfig) -> f64 {
    if speed <= 0.0 {
        return cfg.min_factor;
    }
    let t = speed / (speed + cfg.sensitivity); // (0, 1)
    let f = cfg.min_factor + (cfg.max_factor - cfg.min_factor) * t;
    f.clamp(cfg.min_factor, cfg.max_factor)
}

/// Tracks cursor position across move events and produces accelerated deltas.
pub struct PointerAccel {
    last: Option<(i32, i32)>,
}

impl PointerAccel {
    pub fn new() -> Self {
        Self { last: None }
    }

    /// Given the current absolute cursor position, return the relative delta to
    /// inject (already accelerated), or `None` for the first sample (no history).
    pub fn on_move(&mut self, x: i32, y: i32, cfg: &AccelConfig) -> Option<(i32, i32)> {
        let prev = match self.last {
            Some(p) => p,
            None => {
                self.last = Some((x, y));
                return None;
            }
        };
        self.last = Some((x, y));
        let dx = x - prev.0;
        let dy = y - prev.1;
        let speed = ((dx * dx + dy * dy) as f64).sqrt();
        let f = accel_factor(speed, cfg);
        Some((((dx as f64) * f).round() as i32, ((dy as f64) * f).round() as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AccelConfig;

    fn cfg() -> AccelConfig {
        AccelConfig {
            enabled: false,
            sensitivity: 1000.0,
            min_factor: 1.0,
            max_factor: 2.0,
        }
    }

    #[test]
    fn factor_rises_and_saturates() {
        let c = cfg();
        assert_eq!(accel_factor(0.0, &c), 1.0);
        // Slow move sits near min_factor.
        let slow = accel_factor(10.0, &c);
        assert!(slow > 1.0 && slow < 1.1, "slow factor was {slow}");
        // Fast move near max_factor.
        let fast = accel_factor(100_000.0, &c);
        assert!(fast > 1.99, "fast factor was {fast}");
        // Monotonic in speed.
        assert!(accel_factor(5.0, &c) < accel_factor(50.0, &c));
        assert!(accel_factor(50.0, &c) < accel_factor(500.0, &c));
    }

    #[test]
    fn controller_scales_deltas() {
        let c = cfg();
        let mut a = PointerAccel::new();
        assert_eq!(a.on_move(0, 0, &c), None); // first sample: no history
        // Slow move (~10px) -> ~1.0x -> unchanged.
        assert_eq!(a.on_move(10, 0, &c), Some((10, 0)));
        // Fast move (1000px) -> ~1.5x -> ~1500.
        let (dx, dy) = a.on_move(1010, 0, &c).unwrap();
        assert!(dx > 1400 && dx < 1600, "fast dx was {dx}");
        assert_eq!(dy, 0);
    }

    #[test]
    fn controller_resets_history() {
        let c = cfg();
        let mut a = PointerAccel::new();
        a.on_move(0, 0, &c);
        a.on_move(10, 0, &c);
        a = PointerAccel::new(); // fresh: history cleared
        assert_eq!(a.on_move(500, 0, &c), None);
    }
}
