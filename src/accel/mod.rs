//! Optional additive acceleration of Windows-processed cursor movement.
//!
//! `WH_MOUSE_LL` supplies screen coordinates, not raw device deltas. Native
//! movement always passes through; the worker injects only an extra relative
//! displacement, so queue or injection failures cannot suppress input.

use crate::config::AccelConfig;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::LazyLock;

type Sample = (u64, i32, i32);
static ACTIVE_EPOCH: AtomicU64 = AtomicU64::new(0);
static EPOCH: AtomicU64 = AtomicU64::new(0);
static BASE_VALID: AtomicBool = AtomicBool::new(true);
static ENDPOINT: LazyLock<Mutex<PointerAccel>> =
    LazyLock::new(|| Mutex::new(PointerAccel::default()));
static SENDER: Mutex<Option<SyncSender<Sample>>> = Mutex::new(None);

/// Called from the message-loop thread after hook installation or config changes.
/// A stopped worker exits when its sender drops. Do not join it here: SendInput
/// can wait for this very thread to process the injected low-level hook event.
pub fn configure(cfg: &AccelConfig) {
    ACTIVE_EPOCH.store(0, Ordering::Release);
    EPOCH.fetch_add(1, Ordering::AcqRel);
    *SENDER.lock() = None;
    ENDPOINT.lock().reset();
    BASE_VALID.store(true, Ordering::Release);
    if !cfg.enabled {
        return;
    }
    if !valid_config(cfg) {
        crate::log::write("pointer acceleration disabled: invalid sensitivity/factor range");
        return;
    }
    let epoch = EPOCH.load(Ordering::Acquire);
    let worker_cfg = cfg.clone();
    let (tx, rx) = sync_channel::<Sample>(1);
    match std::thread::Builder::new()
        .name("pointer-accel".into())
        .spawn(move || {
            while let Ok((sample_epoch, dx, dy)) = rx.recv() {
                if sample_epoch != epoch || ACTIVE_EPOCH.load(Ordering::Acquire) != epoch {
                    continue;
                }
                let (bx, by) = bonus_delta(dx, dy, &worker_cfg);
                if (bx != 0 || by != 0) && !crate::scroll::injector::send_mouse_move(bx, by) {
                    if ACTIVE_EPOCH
                        .compare_exchange(epoch, 0, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        crate::log::write("pointer acceleration disabled: SendInput failed");
                    }
                    break;
                }
            }
        }) {
        Ok(_worker) => {
            *SENDER.lock() = Some(tx);
            ACTIVE_EPOCH.store(epoch, Ordering::Release);
        }
        Err(e) => crate::log::write(&format!("pointer acceleration worker unavailable: {e}")),
    }
}

/// O(1), nonblocking hook observation. All original input remains native.
pub fn observe_move(x: i32, y: i32, injected: bool) {
    let epoch = ACTIVE_EPOCH.load(Ordering::Acquire);
    if epoch == 0 {
        return;
    }
    let Some(mut tracker) = ENDPOINT.try_lock() else {
        BASE_VALID.store(false, Ordering::Release);
        return;
    };
    if !BASE_VALID.swap(true, Ordering::AcqRel) {
        tracker.reset();
    }
    if injected {
        tracker.re_sync(x, y);
        return;
    }
    let delta = tracker.on_move(x, y);
    drop(tracker);
    if let Some((dx, dy)) = delta {
        if dx == 0 && dy == 0 {
            return;
        }
        if let Some(sender) = SENDER.try_lock() {
            if let Some(tx) = sender.as_ref() {
                let _ = tx.try_send((epoch, dx, dy));
            }
        }
    }
}

pub fn stop() {
    ACTIVE_EPOCH.store(0, Ordering::Release);
    EPOCH.fetch_add(1, Ordering::AcqRel);
    *SENDER.lock() = None;
    ENDPOINT.lock().reset();
    BASE_VALID.store(true, Ordering::Release);
}

/// Map observed cursor displacement to an additional multiplier. This is not
/// an OS-independent physical mouse velocity curve.
pub fn valid_config(cfg: &AccelConfig) -> bool {
    cfg.sensitivity.is_finite()
        && cfg.sensitivity > 0.0
        && cfg.min_factor.is_finite()
        && cfg.max_factor.is_finite()
        && cfg.min_factor >= 1.0
        && cfg.max_factor >= cfg.min_factor
}

/// Return the optional bonus displacement, bounded to 128 pixels per event.
/// Keeping the cap explicit prevents unusual monitor jumps from launching a
/// cursor across the entire desktop.
pub fn bonus_delta(dx: i32, dy: i32, cfg: &AccelConfig) -> (i32, i32) {
    let speed = (f64::from(dx).hypot(f64::from(dy))).min(1_000_000.0);
    let factor = accel_factor(speed, cfg) - 1.0;
    let extra_x = (f64::from(dx) * factor).round().clamp(-128.0, 128.0) as i32;
    let extra_y = (f64::from(dy) * factor).round().clamp(-128.0, 128.0) as i32;
    (extra_x, extra_y)
}

/// Smooth speed/(speed+sensitivity) curve. Only used with valid configuration.
pub fn accel_factor(speed: f64, cfg: &AccelConfig) -> f64 {
    if speed <= 0.0 {
        return cfg.min_factor;
    }
    let t = speed / (speed + cfg.sensitivity); // (0, 1)
    let f = cfg.min_factor + (cfg.max_factor - cfg.min_factor) * t;
    f.clamp(cfg.min_factor, cfg.max_factor)
}

/// Tracks chronological cursor endpoints on the hook-owning thread.
/// Every injected event reseeds the baseline; a marked bonus at a screen edge
/// therefore cannot create overshoot on the next real move.
#[derive(Default)]
pub struct PointerAccel {
    last: Option<(i32, i32)>,
}

impl PointerAccel {
    pub fn on_move(&mut self, x: i32, y: i32) -> Option<(i32, i32)> {
        let prev = self.last.replace((x, y))?;
        let dx = x.checked_sub(prev.0)?;
        let dy = y.checked_sub(prev.1)?;
        Some((dx, dy))
    }

    pub fn re_sync(&mut self, x: i32, y: i32) {
        self.last = Some((x, y));
    }

    pub fn reset(&mut self) {
        self.last = None;
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
    fn observed_bonus_reseeds_after_injected_move() {
        let c = cfg();
        let mut tracker = PointerAccel::default();
        assert_eq!(tracker.on_move(0, 0), None);
        assert_eq!(tracker.on_move(10, 0), Some((10, 0)));
        assert_eq!(bonus_delta(10, 0, &c), (0, 0));
        tracker.re_sync(15, 0);
        assert_eq!(tracker.on_move(25, 0), Some((10, 0)));
        assert_eq!(bonus_delta(1000, 0, &c), (128, 0));
        tracker.reset();
        assert_eq!(tracker.on_move(500, 0), None);
    }

    #[test]
    fn invalid_parameters_and_coordinate_overflow_fail_safe() {
        let mut c = cfg();
        assert!(valid_config(&c));
        c.sensitivity = f64::NAN;
        assert!(!valid_config(&c));
        c.sensitivity = 1.0;
        c.min_factor = 0.9;
        assert!(!valid_config(&c));
        c.min_factor = 1.0;
        c.max_factor = f64::INFINITY;
        assert!(!valid_config(&c));
        let mut tracker = PointerAccel::default();
        assert_eq!(tracker.on_move(i32::MIN, i32::MIN), None);
        assert_eq!(tracker.on_move(i32::MAX, i32::MAX), None);
        assert_eq!(tracker.on_move(i32::MAX - 10, i32::MAX), Some((-10, 0)));
    }
}
