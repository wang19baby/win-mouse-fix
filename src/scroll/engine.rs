//! Per-axis smooth-scroll engine.
//!
//! This module provides `ScrollAxis` — a minimal state container for the
//! subpixel accumulator and Mac-matched curve configuration values needed
//! by the injector.  The actual physics (Holt-filter speed + drag-decay curve)
//! lives in the injector and curve.rs as a HybridCurve.

use std::time::Instant;

use crate::scroll::curve::drag_speed;
use crate::scroll::curve::{BezierAccelCurve, ScrollSpeedupCurve};
use crate::scroll::subpixel::SubPixelAccumulator;

/// One notch of a wheel = 120 wheel units (WHEEL_DELTA).
const WHEEL_DELTA: f64 = 120.0;
/// Clamp implied speed (units/sec) so a tiny inter-event gap can't explode it.
const V_MAX: f64 = 40000.0;
/// Clamp inter-event / inter-tick dt (seconds).
const MIN_DT: f64 = 0.001;
const MAX_DT: f64 = 0.2;

/// One raw wheel event captured from the low-level hook.
#[derive(Debug, Clone, Copy)]
pub struct WheelInput {
    /// Raw wheel delta (multiples of `WHEEL_DELTA`).
    pub delta: i32,
    /// True for horizontal wheel (`WM_MOUSEHWHEEL`), false for vertical.
    pub horizontal: bool,
}
pub struct ScrollAxis {
    subpixel: SubPixelAccumulator,
    /// Current signed speed, in wheel units/sec.
    velocity: f64,
    /// Speed at the last curve reset (>= 0). Decay is measured from here.
    v0: f64,
    /// Seconds since the curve was last reset by an input event.
    decay_age: f64,
    last_dir: i32,
    last_event: Option<Instant>,
    last_tick: Option<Instant>,
    /// Scroll-speed multiplier (MMF `speed`).
    gain: f64,
    /// Drag exponent `b` (> 1).
    b: f64,
    /// Drag coefficient `a` (> 0).
    a: f64,
    /// Speed (units/sec) below which scrolling is considered finished.
    stop_speed: f64,
    /// Whether a scroll is currently in progress.
    active: bool,
    /// Acceleration curve: speed (tick/s) → px/tick.
    accel_curve: BezierAccelCurve,
    /// Fast-scroll multiplier curve: swipe count → speed factor.
    speedup_curve: ScrollSpeedupCurve,
    /// Acceleration end interval (seconds) — time below which the curve saturates.
    #[allow(dead_code)]
    accel_end: f64,
    /// Max tick interval (seconds) — time above which a new swipe begins.
    #[allow(dead_code)]
    tick_max: f64,
    /// Base animation duration (ms). -1.0 means use baseMsPerStepCurve lookup.
    /// Mirrors Mac `baseMsPerStep`.
    #[allow(dead_code)]
    base_ms_per_step: f64,
}

impl ScrollAxis {
    /// 10-param constructor matching `wheel_tracker.rs` call site.
    #[allow(dead_code)]
    pub fn new(
        drag_exponent: f64,
        drag_coefficient: f64,
        stop_speed: f64,
        gain: f64,
        step: f64,
        accel_curve: BezierAccelCurve,
        speedup_curve: ScrollSpeedupCurve,
        accel_end: f64,
        tick_max: f64,
        base_ms_per_step: f64,
    ) -> Self {
        assert!(drag_exponent > 1.0 && drag_exponent != 2.0, "drag_exponent must be > 1 and != 2");
        assert!(drag_coefficient > 0.0, "drag_coefficient must be > 0");
        assert!(stop_speed > 0.0, "stop_speed must be > 0");
        Self {
            subpixel: SubPixelAccumulator::new(1.0, step),
            velocity: 0.0,
            v0: 0.0,
            decay_age: 0.0,
            last_dir: 0,
            last_event: None,
            last_tick: None,
            gain,
            b: drag_exponent,
            a: drag_coefficient,
            stop_speed,
            active: false,
            accel_curve,
            speedup_curve,
            accel_end,
            tick_max,
            base_ms_per_step,
        }
    }

    /// Feed a raw wheel event that occurred at `now`.
    #[allow(dead_code)]
    pub fn on_wheel(&mut self, delta: i32, now: Instant) {
        let notch = delta as f64 / WHEEL_DELTA;
        let dir = if notch > 0.0 {
            1
        } else if notch < 0.0 {
            -1
        } else {
            return;
        };

        // Opposite-tick: hard stop.
        if self.last_dir != 0 && dir != self.last_dir {
            self.velocity = 0.0;
            self.v0 = 0.0;
            self.decay_age = 0.0;
            self.active = false;
            self.last_dir = dir;
            self.last_event = Some(now);
            return;
        }

        let dt = match self.last_event {
            Some(t) => {
                let d = now.duration_since(t).as_secs_f64();
                if d <= 0.0 {
                    MIN_DT
                } else if d > MAX_DT {
                    MAX_DT
                } else {
                    d
                }
            }
            None => 0.016,
        };
        let implied = ((notch.abs() / dt) * WHEEL_DELTA * self.gain).min(V_MAX);
        let new_v = implied.max(self.velocity.abs());
        self.velocity = dir as f64 * new_v;
        self.v0 = new_v;
        self.decay_age = 0.0;
        self.active = true;
        self.last_dir = dir;
        self.last_event = Some(now);
    }

    /// Advance to `now`, returning the integer wheel delta to emit (0 = nothing).
    #[allow(dead_code)]
    pub fn tick(&mut self, now: Instant) -> i32 {
        let dt = match self.last_tick {
            Some(t) => {
                let d = now.duration_since(t).as_secs_f64();
                if d <= 0.0 {
                    0.0
                } else if d > MAX_DT {
                    MAX_DT
                } else {
                    d
                }
            }
            None => 0.016,
        };
        self.last_tick = Some(now);

        if !self.active {
            self.subpixel.reset();
            return 0;
        }

        let v = if self.decay_age <= 0.0 {
            self.v0
        } else {
            drag_speed(self.v0, self.a, self.b, self.decay_age)
        };

        if v < self.stop_speed {
            self.velocity = 0.0;
            self.v0 = 0.0;
            self.decay_age = 0.0;
            self.active = false;
            return self.subpixel.flush();
        }

        self.velocity = if self.velocity < 0.0 { -v } else { v };
        self.decay_age += dt;

        let emit = v * dt;
        let signed = if self.velocity < 0.0 { -emit } else { emit };
        self.subpixel.add(signed)
    }

    /// Current animation speed (magnitude).
    #[allow(dead_code)]
    pub fn current_speed(&self) -> f64 {
        self.velocity.abs()
    }

    /// Signed velocity (positive = forward/down, negative = backward/up).
    #[allow(dead_code)]
    pub fn signed_speed(&self) -> f64 {
        self.velocity
    }

    /// Drag coefficient `a` (> 0).
    pub fn config_drag_coefficient(&self) -> f64 {
        self.a
    }

    /// Drag exponent `b` (> 1).
    pub fn config_drag_exponent(&self) -> f64 {
        self.b
    }

    /// Stop speed (wheel units/sec).
    pub fn config_stop_speed(&self) -> f64 {
        self.stop_speed
    }

    /// Reference to the acceleration curve. Used by the injector to build
    /// the HybridCurve base curve.
    pub fn accel_curve(&self) -> &BezierAccelCurve {
        &self.accel_curve
    }

    /// Reference to the speedup curve. Used by the injector to compute the
    /// fast-scroll multiplier from swipe_count. Mac Scroll.m line 490.
    pub fn speedup_curve(&self) -> &ScrollSpeedupCurve {
        &self.speedup_curve
    }

    /// Flush any remaining accumulation and reset the subpixelator to zero.
    /// Mirrors Mac Scroll.m lines 584-586.
    pub fn subpixel_flush_and_reset(&mut self) {
        self.subpixel.flush();
        self.subpixel.reset();
    }

    /// Base animation duration (ms), or -1 if using the baseMsPerStepCurve lookup.
    #[allow(dead_code)]
    pub fn config_base_ms_per_step(&self) -> f64 {
        self.base_ms_per_step
    }

    /// Add a raw value to the SubPixelAccumulator and return the integer delta.
    #[allow(dead_code)]
    pub fn subpixel_add(&mut self, value: f64) -> i32 {
        self.subpixel.add(value)
    }

    /// Reset all internal state. Called on direction change.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.active = false;
        self.velocity = 0.0;
        self.v0 = 0.0;
        self.decay_age = 0.0;
        self.last_dir = 0;
        self.last_event = None;
        self.last_tick = None;
        self.subpixel.reset();
    }

    /// Whether a scroll is currently in progress.
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// Mac-parity carry-over distance when a new physical tick arrives mid-animation.
///
/// Mirrors `Scroll.m:561-595`: `pxLeftToScroll` is the magnitude of `valueLeftVec`
/// (the remaining distance on the running curve) when the animator is running, and
/// `0` otherwise (`Scroll.m:584-586` reset). `last_frac` is the fraction of
/// `prev_total_dist` already "paid out" by prior ticks, so the remaining distance is
/// `(1 - last_frac) * prev_total_dist`. The injector uses this for
/// `total_dist = pxToScrollForThisTick + pxLeftToScroll`; extracting it makes the
/// previously-untested CRITICAL path (`diff_report.md` #3) regression-safe.
/// Base-phase-aware carry-over distance for mid-animation ticks.
///
/// Mirrors `Scroll.m:575–582`: `pxLeftToScroll` should be the remaining
/// magnitude on the **base phase** of the hybrid curve (via
/// `baseDistanceLeftWithDistanceLeft`), not the full-hybrid remaining. This
/// ensures subpixel carry-over during the base phase is accurate to Mac's
/// behavior. If `curve` is `None` (animation not started) returns `0.0`.
#[allow(dead_code)]
pub fn base_carry_over_distance(
    animating: bool,
    _prev_total_dist: f64,
    curve: Option<&crate::scroll::curve::HybridCurve>,
    t: f64,
) -> f64 {
    if !animating {
        return 0.0;
    }
    match curve {
        Some(c) => c.base_phase_distance_remaining(t),
        None => 0.0,
    }
}

/// Fallback carry-over using the simple fraction-based approach.
/// Used when no HybridCurve is available (e.g. horizontal axis before curve init).
#[allow(dead_code)]
pub fn carry_over_distance(animating: bool, last_frac: f64, prev_total_dist: f64) -> f64 {
    if animating {
        (1.0 - last_frac) * prev_total_dist
    } else {
        0.0
    }
}

#[cfg(test)]
pub(crate) fn make_axis() -> ScrollAxis {
    let accel = BezierAccelCurve::new(6.25, 66.667, 30.0, 120.0, 3.0);
    let speedup = ScrollSpeedupCurve::new(3, 1.33, 7.5);
    ScrollAxis::new(
        1.05,   // drag_exponent
        15.0,   // drag_coefficient
        30.0,   // stop_speed
        1.0,    // gain
        120.0,  // step
        accel,
        speedup,
        0.015,  // accel_end
        0.160,  // tick_max
        -1.0,   // base_ms_per_step
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scroll::curve::HybridCurve;

    fn make_axis_for_tests() -> ScrollAxis {
        make_axis()
    }

    #[test]
    fn carry_over_distance_matches_mac_px_left_to_scroll() {
        // Not animating -> no carry-over (Scroll.m:584-586 reset to 0).
        assert_eq!(carry_over_distance(false, 0.5, 100.0), 0.0);
        // Animating -> remaining = (1 - last_frac) * prev_total
        // (Scroll.m:595 delta = pxToScrollForThisTick + pxLeftToScroll).
        assert!((carry_over_distance(true, 0.3, 100.0) - 70.0).abs() < 1e-9);
        // At animation end (last_frac -> 1) carry-over -> 0.
        assert!(carry_over_distance(true, 1.0, 100.0).abs() < 1e-9);
    }

    #[test]
    fn tick_emits_positive_delta_after_wheel() {
        let mut axis = make_axis_for_tests();
        let t0 = Instant::now();
        axis.on_wheel(120, t0);
        let delta = axis.tick(t0);
        assert!(delta > 0, "delta should be positive after wheel down");
    }

    #[test]
    fn inertia_fully_decays() {
        let mut axis = make_axis_for_tests();
        let t0 = Instant::now();
        axis.on_wheel(120, t0);
        let mut prev_speed = axis.current_speed();
        assert!(prev_speed > 0.0, "should have speed after wheel event");
        for i in 1..=100 {
            let tn = t0 + std::time::Duration::from_secs_f64(0.008 * i as f64);
            axis.tick(tn);
            let speed = axis.current_speed();
            assert!(speed <= prev_speed + 1e-9, "speed should not increase at tick {i}");
            prev_speed = speed;
        }
    }

    #[test]
    fn opposite_tick_resets_coast_but_allows_new_scroll() {
        let mut axis = make_axis_for_tests();
        let t0 = Instant::now();
        axis.on_wheel(120, t0);
        let t1 = t0 + std::time::Duration::from_millis(50);
        axis.on_wheel(-120, t1);
        assert!(!axis.is_active(), "axis should not be active after opposite tick");
    }
    #[test]
    fn base_carry_over_distance_not_animating_returns_zero() {
        // When not animating, carry-over is always 0.
        let axis = make_axis_for_tests();
        let curve = HybridCurve::new(
            axis.accel_curve(),
            50.0,
            100.0,
            15.0,
            1.05,
            30.0,
            0.2,
        );
        assert_eq!(base_carry_over_distance(false, 100.0, None, 0.0), 0.0);
        assert_eq!(base_carry_over_distance(false, 100.0, Some(&curve), 0.5), 0.0);
    }

    #[test]
    fn base_carry_over_distance_animating_with_curve_delegates_to_base_phase_remaining() {
        let axis = make_axis_for_tests();
        let curve = HybridCurve::new(
            axis.accel_curve(), 50.0, 100.0, 15.0, 1.05, 30.0, 0.2,
        );
        // At t=0, no base phase consumed → base_phase_distance_remaining(0) = transition_distance.
        let expected = curve.base_phase_distance_remaining(0.0);
        assert_eq!(base_carry_over_distance(true, 100.0, Some(&curve), 0.0), expected);
    }

    #[test]
    fn base_carry_over_distance_animating_no_curve_returns_zero() {
        // Edge case: animating but no curve yet (horizontal before first curve init).
        assert_eq!(base_carry_over_distance(true, 100.0, None, 0.0), 0.0);
    }

    #[test]
    fn hybrid_curve_evaluate_at_t_zero_is_near_zero() {
        let axis = make_axis_for_tests();
        let curve = HybridCurve::new(
            axis.accel_curve(), 50.0, 100.0, 15.0, 1.05, 30.0, 0.2,
        );
        // At t=0 (animation just starting), accumulated fraction should be ~0
        let frac = curve.evaluate(0.0);
        assert!(frac < 0.1, "evaluate(0) should be near 0, got {}", frac);
    }
}

// ── Performance benchmarks ────────────────────────────────────────────────
// Run: cargo test bench --release -- --nocapture
#[cfg(test)]
mod bench {
    use super::*;
    use crate::scroll::smoother::DoubleExponentialSmoother;

    #[test]
    fn bench_wheel_processing() {
        let mut axis = make_axis();
        let t0 = Instant::now();
        let mut now = t0;
        for _ in 0..1000 {
            axis.on_wheel(120, now);
            let _ = axis.tick(now);
            now += std::time::Duration::from_millis(16);
        }
        let elapsed = t0.elapsed();
        eprintln!("bench_wheel_processing: 1000 ticks in {elapsed:?}");
    }

    #[test]
    fn bench_smoother() {
        let mut smoother = DoubleExponentialSmoother::new(0.5, 0.5);
        let t0 = Instant::now();
        for i in 0..1000 {
            smoother.smooth(i as f64);
        }
        let elapsed = t0.elapsed();
        eprintln!("bench_smoother: 1000 smooth passes in {elapsed:?}");
    }

    #[test]
    fn bench_coast_simulation() {
        let mut axis = make_axis();
        let t0 = Instant::now();
        let start = t0;
        // Prime the engine with a wheel event so coasting begins.
        axis.on_wheel(120, start);
        let mut now = start + std::time::Duration::from_millis(16);
        for _ in 0..5000 {
            let _ = axis.tick(now);
            now += std::time::Duration::from_millis(8);
        }
        let elapsed = t0.elapsed();
        eprintln!("bench_coast_simulation: 5000 frames in {elapsed:?}");
    }
}
