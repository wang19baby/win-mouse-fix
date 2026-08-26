//! Per-axis smooth-scroll engine.
//!
//! Maps discrete raw wheel events into a continuous, inertial scroll stream.
//! Time is injected by the caller (no clock inside) so the logic is fully
//! unit-testable; the injector owns the real clock + `SendInput`.

use std::time::Instant;

use crate::scroll::smoother::DoubleExponentialSmoother;
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
/// Below this velocity (notch/ms) scrolling is considered stopped.
const MIN_VELOCITY: f64 = 1e-4;

/// Smooth-scroll state for a single axis (vertical or horizontal).
pub struct ScrollAxis {
    smoother: DoubleExponentialSmoother,
    subpixel: SubPixelAccumulator,
    /// Signed velocity estimate, in notch/ms.
    velocity: f64,
    last_dir: i32,
    last_event: Option<Instant>,
    last_tick: Option<Instant>,
    gain: f64,
    /// Per-16ms velocity multiplier, used for inertia decay.
    friction: f64,
}

impl ScrollAxis {
    pub fn new(level: f64, trend: f64, gain: f64, friction: f64) -> Self {
        assert!((0.0..=1.0).contains(&friction), "friction must be in [0,1]");
        Self {
            smoother: DoubleExponentialSmoother::new(level, trend),
            subpixel: SubPixelAccumulator::new(1.0),
            velocity: 0.0,
            last_dir: 0,
            last_event: None,
            last_tick: None,
            gain,
            friction,
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
            self.smoother.reset();
            self.last_dir = dir;
            self.last_event = Some(now);
            return;
        }

        let dt = match self.last_event {
            Some(t) => {
                let d = now.duration_since(t).as_secs_f64() * 1000.0;
                if d <= 0.0 {
                    1.0
                } else {
                    d
                }
            }
            None => 16.0,
        };
        let inst = notch / dt; // notch/ms
        let mag = self.smoother.smooth(inst.abs());
        self.velocity = dir as f64 * mag;
        self.last_dir = dir;
        self.last_event = Some(now);
    }

    /// Advance to `now`, returning the integer wheel delta to emit (0 = nothing).
    pub fn tick(&mut self, now: Instant) -> i32 {
        let dt = match self.last_tick {
            Some(t) => {
                let d = now.duration_since(t).as_secs_f64() * 1000.0;
                if d <= 0.0 {
                    0.0
                } else {
                    d
                }
            }
            None => 16.0,
        };
        self.last_tick = Some(now);

        if self.velocity.abs() < MIN_VELOCITY {
            self.velocity = 0.0;
            self.subpixel.reset();
            return self.subpixel.flush();
        }

        let wheel = self.velocity * dt * self.gain * WHEEL_DELTA;
        // Inertial decay (friction applied per 16 ms).
        self.velocity *= self.friction.powf(dt / 16.0);
        self.subpixel.add(wheel)
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
        let mut ax = ScrollAxis::new(0.6, 0.35, 1.0, 0.88);
        let t0 = Instant::now();
        ax.on_wheel(120, at(t0, 0));
        let mut total = 0;
        for i in 1..=5 {
            total += ax.tick(at(t0, 16 * i));
        }
        assert!(total > 0, "expected positive emission, got {total}");
    }

    #[test]
    fn opposite_tick_stops_scroll() {
        let mut ax = ScrollAxis::new(0.6, 0.35, 1.0, 0.88);
        let t0 = Instant::now();
        ax.on_wheel(120, at(t0, 0));
        ax.on_wheel(-120, at(t0, 20)); // opposite direction -> stop
        let e = ax.tick(at(t0, 36));
        assert_eq!(e, 0, "opposite tick should halt emission");
    }

    #[test]
    fn inertia_fully_decays() {
        let mut ax = ScrollAxis::new(0.6, 0.35, 1.0, 0.88);
        let t0 = Instant::now();
        ax.on_wheel(120, at(t0, 0));
        let mut t = t0;
        for _ in 0..300 {
            t = t + Duration::from_millis(16);
            ax.tick(t);
        }
        // After a long idle the next tick must emit nothing.
        assert_eq!(ax.tick(t + Duration::from_millis(16)), 0);
    }
}
