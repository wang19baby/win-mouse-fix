//! Momentum model ported from Mac Mouse Fix 3.x (`DragCurve.swift`).
//!
//! MMF models drag (inertial deceleration) with the differential equation
//! `v'(t) = -a * v(t)^b` and solves it in closed form. We use the same
//! functions directly so the scroll coast matches MMF's feel: velocity
//! follows a power-law decay rather than the exponential `friction` decay of
//! the old (MMF 2.x-style) model.
//!
//! Closed forms (MMF `DragCurve.swift`, general case `b != 1, b != 2`):
//! ```text
//! v(t) = ((b - 1) * a * (t - c))^(1 / (1 - b))
//! c    = -v0^(1 - b) / (a * (b - 1))      // so that v(0) = v0
//! ```
//! `a` = drag coefficient, `b` = drag exponent (> 1). `stop_speed` is the
//! speed at which scrolling is considered finished.

/// Speed at time `t` (seconds) given initial speed `v0` (>= 0), coefficient
/// `a > 0` and exponent `b > 1` (`b != 2`).
///
/// Returns `v0` at `t = 0` and decays monotonically toward 0.
pub fn drag_speed(v0: f64, a: f64, b: f64, t: f64) -> f64 {
    assert!(v0 >= 0.0, "v0 must be >= 0");
    assert!(a > 0.0, "a must be > 0");
    assert!(b > 1.0 && b != 2.0, "b must be > 1 and != 2");
    let inv = 1.0 / (1.0 - b); // negative
    let c = -v0.powf(1.0 - b) / (a * (b - 1.0));
    let arg = (b - 1.0) * a * (t - c);
    // arg is positive for t >= 0, so the power is well defined.
    arg.powf(inv)
}

/// Distance scrolled from `0` to `t` (seconds) under [`drag_speed`].
///
/// `d(t) = ((b - 1) * a * (t - c))^(1/(1-b) + 1) / (a * (b - 2)) + k`,
/// with `k` chosen so that `d(0) = 0`.
#[allow(dead_code)] // part of the curve model; exercised by tests
pub fn drag_distance(v0: f64, a: f64, b: f64, t: f64) -> f64 {
    assert!(v0 >= 0.0, "v0 must be >= 0");
    assert!(a > 0.0, "a must be > 0");
    assert!(b > 1.0 && b != 2.0, "b must be > 1 and != 2");
    let c = -v0.powf(1.0 - b) / (a * (b - 1.0));
    let p = 1.0 / (1.0 - b) + 1.0;
    let inner = (b - 1.0) * a * (t - c);
    let term = inner.powf(p) / (a * (b - 2.0));
    let term0 = ((b - 1.0) * a * -c).powf(p) / (a * (b - 2.0));
    term - term0
}

/// Time (seconds) until speed decays from `v0` down to `stop_speed`.
/// Returns `0` when `v0 <= stop_speed`.
#[allow(dead_code)] // part of the curve model; exercised by tests
pub fn drag_stop_time(v0: f64, a: f64, b: f64, stop_speed: f64) -> f64 {
    if v0 <= stop_speed {
        return 0.0;
    }
    // t = (stop^(1-b) - v0^(1-b)) / (a * (b - 1))
    let num = stop_speed.powf(1.0 - b) - v0.powf(1.0 - b);
    let den = a * (b - 1.0);
    num / den
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: f64 = 15.0;
    const B: f64 = 1.2;

    #[test]
    fn speed_starts_at_v0() {
        let v = drag_speed(1000.0, A, B, 0.0);
        assert!((v - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn speed_monotonically_decays() {
        let v0 = 5000.0;
        let mut prev = v0;
        for i in 1..=20 {
            let t = i as f64 * 0.01;
            let v = drag_speed(v0, A, B, t);
            assert!(v <= prev + 1e-9, "speed should not increase");
            assert!(v >= 0.0);
            prev = v;
        }
    }

    #[test]
    fn distance_increases_and_matches_stop_time() {
        let v0 = 8000.0;
        let stop = 200.0;
        let t_stop = drag_stop_time(v0, A, B, stop);
        assert!(t_stop > 0.0);
        // Just before stop, distance is finite and positive.
        let d = drag_distance(v0, A, B, t_stop * 0.999);
        assert!(d > 0.0);
        // Total coast distance should be well bounded (not infinite).
        let d_total = drag_distance(v0, A, B, t_stop);
        assert!(d_total < v0 * t_stop * 2.0, "coast distance sanity");
    }

    #[test]
    fn stop_time_zero_when_already_slow() {
        assert_eq!(drag_stop_time(100.0, A, B, 200.0), 0.0);
    }
}
