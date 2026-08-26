//! Per-axis smooth-scroll engine — MMF 3.x style curve/momentum model.
//!
//! Instead of the old MMF-2.x Holt filter + exponential `friction` decay, this
//! ports MMF 3.x's drag physics (`curve.rs`): each wheel notch injects speed,
//! and the emitted scroll velocity follows a power-law deceleration
//! (`v'(t) = -a·v(t)^b`). While you keep flicking, each event refreshes the
//! start of the curve so speed is sustained; when you stop, the curve's tail
//! provides the natural coast. Time is injected by the caller (no clock
//! inside) so the logic stays unit-testable; the injector owns the clock.

use std::time::Instant;

use crate::scroll::curve::drag_speed;
use crate::scroll::subpixel::SubPixelAccumulator;

/// One raw wheel event captured from the low-level hook.
#[derive(Debug, Clone, Copy)]
pub struct WheelInput {
    /// Raw wheel delta (multiples of `WHEEL_DELTA`).
    pub delta: i32,
    /// True for horizontal wheel (`WM_MOUSEHWHEEL`), false for vertical.
    pub horizontal: bool,
}

/// One notch of a wheel = 120 wheel units (WHEEL_DELTA).
const WHEEL_DELTA: f64 = 120.0;
/// Clamp implied speed (units/sec) so a tiny inter-event gap can't explode it.
const V_MAX: f64 = 40000.0;
/// Clamp inter-event / inter-tick dt (seconds).
const MIN_DT: f64 = 0.001;
const MAX_DT: f64 = 0.2;

/// Smooth-scroll state for a single axis (vertical or horizontal).
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
}

impl ScrollAxis {
    pub fn new(
        drag_exponent: f64,
        drag_coefficient: f64,
        stop_speed: f64,
        gain: f64,
        step: f64,
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
        }
    }

    /// Feed a raw wheel event that occurred at `now`.
    pub fn on_wheel(&mut self, delta: i32, now: Instant) {
        let notch = delta as f64 / WHEEL_DELTA;
        let dir = if notch > 0.0 {
            1
        } else if notch < 0.0 {
            -1
        } else {
            return;
        };

        // Opposite-tick: hard stop (mac-mouse-fix "opposite-tick" feature).
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
        // Implied speed from how fast notches are arriving, scaled by gain.
        let implied = ((notch.abs() / dt) * WHEEL_DELTA * self.gain).min(V_MAX);
        let new_v = implied.max(self.velocity.abs());
        self.velocity = dir as f64 * new_v;
        self.v0 = new_v;
        // Refresh the curve start so sustained scrolling keeps full speed.
        self.decay_age = 0.0;
        self.active = true;
        self.last_dir = dir;
        self.last_event = Some(now);
    }

    /// Advance to `now`, returning the integer wheel delta to emit (0 = nothing).
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

        // Current speed: full at the curve start, then drag-physics decay.
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

        let emit = v * dt; // wheel units this tick
        let signed = if self.velocity < 0.0 { -emit } else { emit };
        self.subpixel.add(signed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn emits_positive_when_scrolling_down() {
        let mut ax = ScrollAxis::new(1.2, 15.0, 200.0, 1.0, 120.0);
        let t0 = Instant::now();
        ax.on_wheel(120, at(t0, 0));
        let mut total = 0;
        for i in 1..=20 {
            total += ax.tick(at(t0, i * 8));
        }
        assert!(total > 0, "should emit scroll for a downward wheel");
    }

    #[test]
    fn opposite_tick_stops_scroll() {
        let mut ax = ScrollAxis::new(1.2, 15.0, 200.0, 1.0, 120.0);
        let t0 = Instant::now();
        ax.on_wheel(120, at(t0, 0));
        ax.on_wheel(-120, at(t0, 20)); // opposite direction -> stop
        let after = ax.tick(at(t0, 28));
        assert_eq!(after, 0, "opposite tick must hard-stop");
    }

    #[test]
    fn inertia_fully_decays() {
        let mut ax = ScrollAxis::new(1.2, 15.0, 200.0, 1.0, 120.0);
        let t0 = Instant::now();
        ax.on_wheel(120, at(t0, 0));
        // Long pause with no further input: coast must end and stop.
        let stop = crate::scroll::curve::drag_stop_time(7500.0, 15.0, 1.2, 200.0) * 1000.0;
        let mut last_nonzero = 0;
        for i in 1..=200 {
            let out = ax.tick(at(t0, 50 + i * 8));
            if out != 0 {
                last_nonzero = i;
            }
        }
        assert!(last_nonzero > 0, "should coast then stop");
        assert!(
            (50.0 + (last_nonzero as f64) * 8.0) < 50.0 + stop + 200.0,
            "coast should finish around the predicted stop time"
        );
        // After the coast, ticks emit nothing.
        assert_eq!(ax.tick(at(t0, 5000)), 0);
    }
}
