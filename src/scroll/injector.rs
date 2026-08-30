use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_MOVE, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
};
use crate::config::Config;
use crate::scroll::curve::{BezierAccelCurve, HybridCurve};
use crate::scroll::subpixel::SubPixelAccumulator;
use crate::scroll::wheel_tracker::WheelTracker;
use crate::scroll::engine::WheelInput;
/// Tick cadence (~125 Hz = 8 ms).
const TICK_MS: u64 = 8;
/// Milliseconds after last tick before coast begins.
const COAST_DELAY_MS: f64 = 80.0;
const MAX_ANIMATION_SECS: f64 = 1.5;
struct Coast2D {
    animating: bool,
    anim_start: Option<Instant>,
    total_dist: f64,
    accum: f64,
    curve: Option<HybridCurve>,
    t_norm: f64,
    last_tick: Option<Instant>,
    scroll_dist: f64,
    swipe_count: f64,
    direction: (f64, f64),
    subpix_y: SubPixelAccumulator,
    subpix_x: SubPixelAccumulator,
}

impl Coast2D {
    fn new(step: f64) -> Self {
        Self {
            animating: false,
            anim_start: None,
            total_dist: 0.0,
            accum: 0.0,
            curve: None,
            t_norm: 0.0,
            last_tick: None,
            scroll_dist: 0.0,
            swipe_count: 0.0,
            direction: (0.0, 0.0),
            subpix_y: SubPixelAccumulator::new(1.0, step),
            subpix_x: SubPixelAccumulator::new(1.0, step),
        }
    }

    /// Pure helper: integrate a single `(dy, dx)` tick into the running
    /// direction using an exponential moving average. New ticks contribute
    /// 30% of the new magnitude; existing direction keeps 70%. Returns the
    /// updated (normalised) direction vector.
    ///
    /// Public so PR-C tests can exercise the EMA behaviour without
    /// constructing a full `ScrollInjector`.
    #[allow(dead_code)]
    pub fn update_direction_ema(
        prev: (f64, f64),
        dy: i32,
        dx: i32,
    ) -> (f64, f64) {
        if dy == 0 && dx == 0 {
            return prev;
        }
        let new_y = dy as f64;
        let new_x = dx as f64;
        let mag = (new_y * new_y + new_x * new_x).sqrt();
        if mag < 1e-9 {
            return prev;
        }
        let ny = new_y / mag;
        let nx = new_x / mag;
        const ALPHA: f64 = 0.7;
        let mixed_y = ALPHA * prev.0 + (1.0 - ALPHA) * ny;
        let mixed_x = ALPHA * prev.1 + (1.0 - ALPHA) * nx;
        let m = (mixed_y * mixed_y + mixed_x * mixed_x).sqrt().max(1e-9);
        (mixed_y / m, mixed_x / m)
    }

    /// Integrate a tick into `self.direction` using the EMA helper above.
    #[allow(dead_code)]
    fn update_direction(&mut self, dy: i32, dx: i32) {
        self.direction = Self::update_direction_ema(self.direction, dy, dx);
    }


    fn cancel(&mut self) {
        self.animating = false;
        self.curve = None;
        self.accum = 0.0;
        self.t_norm = 0.0;
        self.anim_start = None;
        self.scroll_dist = 0.0;
    }
}

pub struct ScrollInjector {
    rx: Receiver<WheelInput>,
    vertical: WheelTracker,
    horizontal: WheelTracker,

    // ── Shift nonlinear accelerator (PR-B) ──────────────────────────────────
    /// Optional BezierAccelCurve built from `config.scroll.shift_speedup_curve`.
    /// `None` ⇒ fall back to the scalar `shift_speedup` field below (legacy
    /// behaviour, also used when `shift_speedup_linear = true`).
    shift_curve: Option<BezierAccelCurve>,
    /// Hold duration (seconds) at which the curve reaches its right endpoint (x=1).
    shift_speedup_max_hold: f64,
    /// When true, use `shift_speedup` scalar instead of the curve.
    shift_speedup_linear: bool,
    /// Legacy scalar Shift multiplier, used when `shift_speedup_linear = true`
    /// or `shift_curve` is `None`.
    shift_scalar_speedup: f64,

    coast: Coast2D,
}

impl ScrollInjector {
    /// Compute the current Shift speed multiplier.
    ///
    /// In linear mode (or when the curve is unavailable) returns the scalar
    /// `shift_scalar_speedup`. Otherwise looks up the configured Bezier
    /// curve at `modifiers::shift_hold_secs()`.
    ///
    /// Always returns ≥ 1.0 — Shift never slows the scroll down.
    fn current_shift_factor(&self) -> f64 {
        if !crate::modifiers::shift_held() {
            return 1.0;
        }
        if self.shift_speedup_linear || self.shift_curve.is_none() {
            return self.shift_scalar_speedup.max(1.0);
        }
        let curve = self.shift_curve.as_ref().expect("checked is_some above");
        let hold = crate::modifiers::shift_hold_secs();
        crate::scroll::curve::evaluate_shift_speedup(
            curve,
            hold,
            self.shift_speedup_max_hold,
        )
        .max(1.0)
    }

    /// Main loop: receive raw wheel events, drive trackers, emit smoothed deltas.
    fn run(mut self) {
        loop {
            let t0 = Instant::now();
            while let Ok(ev) = self.rx.try_recv() {
                crate::log::write(&format!(
                    "[wheel] rx_event delta={} horiz={}",
                    ev.delta, ev.horizontal
                ));
                let (dy, dx) = if ev.horizontal { (0, ev.delta) } else { (ev.delta, 0) };
                self.handle_physical_tick(dy, dx, t0);
            }
            let now = Instant::now();
            let (dy, dx) = self.advance_coast(now);
            if dy != 0 {
                unsafe { send_wheel(dy, false) };
            }
            if dx != 0 {
                unsafe { send_wheel(dx, true) };
            }
            thread::sleep(Duration::from_millis(TICK_MS));
        }
    }

    fn handle_physical_tick(&mut self, dy: i32, dx: i32, now: Instant) {
        let va = self.vertical.on_tick(dy, now);
        let ha = self.horizontal.on_tick(dx, now);
        if self.coast.animating {
            self.coast.cancel();
        }
        if va.is_new_swipe || ha.is_new_swipe {
            self.vertical.axis_mut().subpixel_flush_and_reset();
            self.horizontal.axis_mut().subpixel_flush_and_reset();
            self.coast.scroll_dist = 0.0;
            self.coast.direction = (0.0, 0.0);
        }
        let mult_v = (self.vertical.axis().speedup_curve().evaluate(va.swipe_count)).min(5.0);
        let mult_h = (self.horizontal.axis().speedup_curve().evaluate(ha.swipe_count)).min(5.0);
        let multiplier = ((mult_v + mult_h) * 0.5).max(mult_v.min(mult_h));
        let shift = self.current_shift_factor();
        let tick_dist = ((dy.abs() + dx.abs()) as f64) * multiplier * shift;
        self.coast.scroll_dist += tick_dist;
        self.update_direction(dy, dx);
        if dy != 0 {
            unsafe { send_wheel(dy, false) };
        }
        if dx != 0 {
            unsafe { send_wheel(dx, true) };
        }
        self.coast.last_tick = Some(now);
        self.coast.swipe_count = va.swipe_count.max(ha.swipe_count);
    }

    fn update_direction(&mut self, dy: i32, dx: i32) {
        if dy == 0 && dx == 0 {
            return;
        }
        let new_y = dy as f64;
        let new_x = dx as f64;
        let mag = (new_y * new_y + new_x * new_x).sqrt();
        if mag < 1e-9 {
            return;
        }
        let ny = new_y / mag;
        let nx = new_x / mag;
        const ALPHA: f64 = 0.7;
        let (oy, ox) = self.coast.direction;
        let mixed_y = ALPHA * oy + (1.0 - ALPHA) * ny;
        let mixed_x = ALPHA * ox + (1.0 - ALPHA) * nx;
        let m = (mixed_y * mixed_y + mixed_x * mixed_x).sqrt().max(1e-9);
        self.coast.direction = (mixed_y / m, mixed_x / m);
    }

    fn maybe_start_coast(&mut self, now: Instant) {
        if self.coast.animating {
            return;
        }
        if let Some(last) = self.coast.last_tick {
            let elapsed_ms = now.duration_since(last).as_secs_f64() * 1000.0;
            let enough_for_coast = self.coast.scroll_dist >= 240.0
                && self.coast.swipe_count >= 2.0;
            if elapsed_ms >= COAST_DELAY_MS && enough_for_coast {
                self.start_coast(now);
            }
        }
        self.vertical.axis_mut().subpixel_flush_and_reset();
        self.horizontal.axis_mut().subpixel_flush_and_reset();
    }

    fn start_coast(&mut self, now: Instant) {
        let total_dist = self.coast.scroll_dist;
        let v_coeff = self.vertical.config_drag_coefficient();
        let h_coeff = self.horizontal.config_drag_coefficient();
        let coeff = (v_coeff + h_coeff) * 0.5;
        let v_exp = self.vertical.config_drag_exponent();
        let h_exp = self.horizontal.config_drag_exponent();
        let exp = (v_exp + h_exp) * 0.5;
        let stop = (self.vertical.config_stop_speed()
            + self.horizontal.config_stop_speed()) * 0.5;
        let base_ms = 250.0;
        let curve = HybridCurve::new(
            self.vertical.axis().accel_curve(),
            base_ms,
            total_dist,
            coeff,
            exp,
            stop,
            0.2,
        );
        crate::log::write(&format!(
            "[wheel] START_COAST total_dist={:.0} direction=({:.2}, {:.2})",
            total_dist, self.coast.direction.0, self.coast.direction.1
        ));
        self.coast.animating = true;
        self.coast.anim_start = Some(now);
        self.coast.accum = 0.0;
        self.coast.curve = Some(curve);
        self.coast.total_dist = total_dist;
        self.coast.t_norm = 0.0;
        self.coast.scroll_dist = 0.0;
    }

    fn advance_coast(&mut self, now: Instant) -> (i32, i32) {
        if !self.coast.animating {
            self.maybe_start_coast(now);
            return (0, 0);
        }
        let dt = self.coast.anim_start
            .map(|s| now.duration_since(s).as_secs_f64())
            .unwrap_or(0.0)
            .min(MAX_ANIMATION_SECS);
        let total_dur = self.coast.curve.as_ref().map(|c| c.total_duration()).unwrap_or(0.5);
        let t_norm = (dt / total_dur).min(1.0);
        let (dy, dx) = if let Some(curve) = self.coast.curve.as_ref() {
            let new_frac = curve.evaluate(t_norm);
            let new_accum = new_frac * self.coast.total_dist;
            let remaining = (new_accum - self.coast.accum).abs();
            self.coast.accum = new_accum;
            self.coast.t_norm = t_norm;
            if t_norm >= 1.0 || remaining < 0.5 {
                self.coast.cancel();
                return (0, 0);
            }
            let dir_y = self.coast.direction.0;
            let dir_x = self.coast.direction.1;
            (
                self.coast.subpix_y.add(remaining * dir_y),
                self.coast.subpix_x.add(remaining * dir_x),
            )
        } else {
            (0, 0)
        };
        (dy, dx)
    }
}

/// Spawn the injector thread and return a `Sender` for the hook layer to feed.
///
/// Returns `None` if smooth scrolling is disabled in `cfg`.
pub fn start(cfg: &Config) -> Option<Sender<WheelInput>> {
    eprintln!("[DEBUG] start() called: scroll.enabled={}, scroll.smooth={}", cfg.scroll.enabled, cfg.scroll.smooth);
    if !cfg.scroll.enabled || !cfg.scroll.smooth {
        eprintln!("[DEBUG] start() early return: enabled={}, smooth={}", cfg.scroll.enabled, cfg.scroll.smooth);
        return None;
    }
    let s = &cfg.scroll;
    let (tx, rx) = mpsc::channel();
    eprintln!("[DEBUG] start(): channel created, about to construct injector");
    let injector = ScrollInjector {
        rx,
        vertical: WheelTracker::new(
            s.drag_exponent, s.drag_coefficient, s.stop_speed, s.speed, s.step,
            s.time_smoothing_weight, s.velocity_a, s.velocity_y,
            s.swipe_threshold, s.swipe_max_interval, s.swipe_min_tick_speed,
            s.smoothness, s.precise, s.tick_interval_accel_end, s.tick_interval_max,
            s.base_ms_per_step,
            s.accel_x_min, s.accel_x_max, s.accel_y_min, s.accel_y_max,
            s.accel_curvature, s.fast_scroll_threshold, s.fast_scroll_initial,
            s.fast_scroll_exponential,
        ),
        horizontal: WheelTracker::new(
            s.drag_exponent, s.drag_coefficient, s.stop_speed, s.speed, s.step,
            s.time_smoothing_weight, s.velocity_a, s.velocity_y,
            s.swipe_threshold, s.swipe_max_interval, s.swipe_min_tick_speed,
            s.smoothness, s.precise, s.tick_interval_accel_end, s.tick_interval_max,
            s.base_ms_per_step,
            s.accel_x_min, s.accel_x_max, s.accel_y_min, s.accel_y_max,
            s.accel_curvature, s.fast_scroll_threshold, s.fast_scroll_initial,
            s.fast_scroll_exponential,
        ),
        shift_curve: match s.shift_speedup_curve.as_ref() {
            Some(pts) => Some(BezierAccelCurve::from_points(&pts.as_point_pairs())),
            None => None,
        },
        shift_speedup_max_hold: s.shift_speedup_max_hold,
        shift_speedup_linear: s.shift_speedup_linear,
        shift_scalar_speedup: s.shift_speedup,
        coast: Coast2D::new(s.step),
    };
    eprintln!("[DEBUG] start(): injector constructed, spawning thread");
    std::thread::spawn(move || { eprintln!("[DEBUG] injector thread started"); injector.run(); });
    eprintln!("[DEBUG] start(): thread spawned, returning tx");
    Some(tx)
}

/// Synthesize one wheel event via `SendInput`.
unsafe fn send_wheel(delta: i32, horizontal: bool) {
    let mut input = INPUT {
        r#type: INPUT_MOUSE,
        ..std::mem::zeroed()
    };
    input.Anonymous.mi = MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: delta as u32,
        dwFlags: if horizontal {
            MOUSEEVENTF_HWHEEL
        } else {
            MOUSEEVENTF_WHEEL
        },
        time: 0,
        dwExtraInfo: 0xFA57_0000,
    };
    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
}

/// Synthesize a relative mouse move via `SendInput` (used by pointer accel).
pub fn send_mouse_move(dx: i32, dy: i32) {
    unsafe {
        let mut input = INPUT {
            r#type: INPUT_MOUSE,
            ..std::mem::zeroed()
        };
        input.Anonymous.mi = MOUSEINPUT {
            dx,
            dy,
            mouseData: 0,
            dwFlags: MOUSEEVENTF_MOVE,
            time: 0,
            dwExtraInfo: crate::win::hooks::OUR_MARKER,
        };
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_ema_pure_diagonal_stays_diagonal() {
        let mut dir = (0.0, 0.0);
        for _ in 0..6 {
            dir = Coast2D::update_direction_ema(dir, 120, 80);
        }
        let ratio = dir.0 / dir.1;
        assert!((ratio - 1.5).abs() < 0.05);
        let mag = (dir.0 * dir.0 + dir.1 * dir.1).sqrt();
        assert!((mag - 1.0).abs() < 1e-9);
    }

    #[test]
    fn direction_ema_pure_vertical_keeps_x_zero() {
        let mut dir = (0.0, 0.0);
        for _ in 0..5 {
            dir = Coast2D::update_direction_ema(dir, 120, 0);
        }
        assert!(dir.1.abs() < 1e-9);
        assert!(dir.0 > 0.99);
    }

    #[test]
    fn direction_ema_pure_horizontal_keeps_y_zero() {
        let mut dir = (0.0, 0.0);
        for _ in 0..5 {
            dir = Coast2D::update_direction_ema(dir, 0, 120);
        }
        assert!(dir.0.abs() < 1e-9);
        assert!(dir.1 > 0.99);
    }

    #[test]
    fn direction_ema_blended_swipe_preserves_majority() {
        let mut dir = (0.0, 0.0);
        for _ in 0..3 {
            dir = Coast2D::update_direction_ema(dir, 100, 100);
        }
        let before = dir;
        dir = Coast2D::update_direction_ema(dir, 100, 0);
        assert!(dir.1 > 0.2, "single vertical tick should not erase prior dx");
        assert!(dir.0 > dir.1);
    }

    #[test]
    fn direction_ema_zero_tick_is_noop() {
        let dir = (0.5, 0.5);
        let after = Coast2D::update_direction_ema(dir, 0, 0);
        assert_eq!(after, dir);
    }

    #[test]
    fn direction_ema_negative_sign_preserved() {
        let mut dir = (0.0, 0.0);
        for _ in 0..5 {
            dir = Coast2D::update_direction_ema(dir, -120, -80);
        }
        assert!(dir.0 < 0.0);
        assert!(dir.1 < 0.0);
        let ratio = dir.0 / dir.1;
        assert!((ratio - 1.5).abs() < 0.05);
    }

    #[test]
    fn coast_new_starts_idle_with_zero_direction() {
        let c = Coast2D::new(120.0);
        assert!(!c.animating);
        assert_eq!(c.direction, (0.0, 0.0));
        assert_eq!(c.scroll_dist, 0.0);
        assert_eq!(c.total_dist, 0.0);
        assert_eq!(c.swipe_count, 0.0);
    }

    #[test]
    fn coast_cancel_resets_animating_and_curve() {
        let mut c = Coast2D::new(120.0);
        c.animating = true;
        c.t_norm = 0.5;
        c.accum = 100.0;
        c.cancel();
        assert!(!c.animating);
        assert!(c.curve.is_none());
        assert_eq!(c.accum, 0.0);
        assert_eq!(c.t_norm, 0.0);
        assert_eq!(c.scroll_dist, 0.0);
    }
}
