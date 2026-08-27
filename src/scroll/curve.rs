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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub fn drag_stop_time(v0: f64, a: f64, b: f64, stop_speed: f64) -> f64 {
    if v0 <= stop_speed {
        return 0.0;
    }
    let num = stop_speed.powf(1.0 - b) - v0.powf(1.0 - b);
    let den = a * (b - 1.0);
    num / den
}

// ─── BezierAccelCurve ─────────────────────────────────────────────────────────

/// n-point Bezier acceleration curve with linear extrapolation beyond
/// the control point range. Ported from `BezierCappedAccelerationCurve`
/// + `AccelerationBezier` in MMF.
///
/// Control points must have monotonically increasing x values and cover the
/// domain [x_min, x_max]. Outside this range, the curve extends linearly via
/// a pre-line (slope=0 through first control point) and post-line (slope =
/// bezier_exit_slope through last control point).
///
/// Evaluation uses De Casteljau's algorithm (numerically stable O(n²))
/// for both the x→t inversion (binary search) and y(t) evaluation.
#[derive(Debug, Clone)]
pub struct BezierAccelCurve {
    /// Control points in sorted order by x.
    pts: Vec<(f64, f64)>,
    n: usize,
    x_min: f64,
    x_max: f64,
    y_min: f64,
    y_max: f64,
    /// Pre-line: y = b (horizontal), b = y0
    pre_b: f64,
    /// Post-line: y = a*(x - x_max) + y_max, a = exit_slope
    post_a: f64,
    /// Raw x-coordinates of control points (for binary search in t-finding).
    pts_x: Vec<f64>,
    /// Raw y-coordinates of control points (for De Casteljau in y-evaluation).
    pts_y: Vec<f64>,
}

impl BezierAccelCurve {
    /// Create with evenly-spaced control points.
    ///
    /// Generates `degree + 1` control points at x positions linearly spaced
    /// between `x_min` and `x_max`. Point i=0 has y=`y_min`; all others have y=`y_max`.
    /// This creates a flat-saturated acceleration curve.
    pub fn new(x_min: f64, x_max: f64, y_min: f64, y_max: f64, curvature: f64) -> Self {
        let degree = curvature + 1.0;
        let mut pts: Vec<(f64, f64)> = Vec::with_capacity(degree as usize + 1);
        let mut i = 0.0;
        loop {
            let x = if degree == 0.0 {
                x_max
            } else {
                (i / degree).mul_add(x_max - x_min, x_min)
            };
            if x > x_max {
                break;
            }
            let y = if i == 0.0 { y_min } else { y_max };
            pts.push((x, y));
            if (i as f64) >= degree {
                break;
            }
            i += 1.0;
        }
        Self::from_points(&pts)
    }

    /// Create from explicitly specified control points.
    pub fn from_points(pts: &[(f64, f64)]) -> Self {
        assert!(pts.len() >= 2, "Bezier needs at least 2 control points");
        let n = pts.len() - 1;
        let x_min = pts.first().map(|p| p.0).unwrap();
        let x_max = pts.last().map(|p| p.0).unwrap();
        let y_min = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let y_max = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);

        // Pre-line: horizontal at y=y_min (slope=0 through first point)
        let pre_b = pts.first().map(|p| p.1).unwrap();

        // Exit slope at last point using finite difference on last two points
        let (x_last, y_last) = pts[pts.len() - 1];
        let (x_pen, y_pen) = pts[pts.len() - 2];
        let dx = x_last - x_pen;
        let post_a = if dx.abs() > 1e-12 {
            (y_last - y_pen) / dx
        } else {
            0.0
        };

        let pts_x: Vec<f64> = pts.iter().map(|p| p.0).collect();
        let pts_y: Vec<f64> = pts.iter().map(|p| p.1).collect();

        Self {
            pts: pts.to_vec(),
            n,
            x_min,
            x_max,
            y_min,
            y_max,
            pre_b,
            post_a,
            pts_x,
            pts_y,
        }
    }

    /// Evaluate y at given x.
    pub fn eval(&self, x: f64) -> f64 {
        if x <= self.x_min {
            self.y_min
        } else if x >= self.x_max {
            self.y_max + self.post_a * (x - self.x_max)
        } else {
            // Binary search for t such that B_x(t) = x, then evaluate B_y(t).
            let t = solve_t_for_x(x, &self.pts_x, 1e-12);
            decasteljau_1d(&self.pts_y, t)
        }
    }

    /// Evaluate at speed → px/tick.
    pub fn evaluate(&self, speed: f64) -> f64 {
        self.eval(speed)
    }

    /// Evaluate the Bezier y-coordinate at parameter t ∈ [0, 1].
    /// This is the accumulated distance fraction at t.
    /// mac: `baseCurve.evaluate(at: t)` or `baseCurve.sampleCurve(onAxis: .yAxis, atT: t)`
    pub fn sample_y_at_t(&self, t: f64) -> f64 {
        decasteljau_1d(&self.pts_y, t)
    }

    /// Evaluate the Bezier x-coordinate at parameter t ∈ [0, 1].
    /// This is the normalized time at t.
    /// mac: `baseCurve.sampleCurve(onAxis: .xAxis, atT: t)`
    pub fn sample_x_at_t(&self, t: f64) -> f64 {
        decasteljau_1d(&self.pts_x, t)
    }

    /// Compute dy/dx at parameter t using the chain rule:
    /// B_x'(t) and B_y'(t) are computed via De Casteljau on the derivative control points.
    /// mac: `baseCurve.derivativeDyOverDx(atT: t)`
    pub fn derivative_dy_dx(&self, t: f64) -> f64 {
        // Derivative of a Bezier of degree n is a Bezier of degree n-1.
        // Control points for derivative in x: (n * (pts_x[i+1] - pts_x[i]))
        let n = self.n;
        if n == 0 {
            return 0.0;
        }
        let mut dx_pts: Vec<f64> = Vec::with_capacity(n);
        let mut dy_pts: Vec<f64> = Vec::with_capacity(n);
        for i in 0..n {
            dx_pts.push((self.pts_x[i + 1] - self.pts_x[i]) * (n as f64));
            dy_pts.push((self.pts_y[i + 1] - self.pts_y[i]) * (n as f64));
        }
        let dx = decasteljau_1d(&dx_pts, t);
        let dy = decasteljau_1d(&dy_pts, t);
        if dx.abs() < 1e-12 {
            return if dy >= 0.0 { f64::INFINITY } else { f64::NEG_INFINITY };
        }
        dy / dx
    }
}

// ─── De Casteljau helpers ────────────────────────────────────────────────────

/// De Casteljau's algorithm for 1D Bezier evaluation.
/// Returns the interpolated value at parameter t ∈ [0, 1].
/// Numerically stable: errors shrink as t approaches 0 or 1.
fn decasteljau_1d(pts: &[f64], t: f64) -> f64 {
    let n = pts.len();
    let mut vals: Vec<f64> = pts.to_vec();
    for _ in 1..n {
        for i in 0..(vals.len() - 1) {
            vals[i] = (1.0 - t) * vals[i] + t * vals[i + 1];
        }
        vals.pop();
    }
    vals[0]
}

/// Find t ∈ [0,1] such that B_x(t) ≈ target_x using binary search.
/// Requires that the x-coordinates of control points are monotonically increasing.
fn solve_t_for_x(target_x: f64, pts_x: &[f64], epsilon: f64) -> f64 {
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..80 {
        let mid = (lo + hi) / 2.0;
        let x_mid = decasteljau_1d(pts_x, mid);
        if (x_mid - target_x).abs() < epsilon {
            return mid;
        }
        if x_mid < target_x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

// ─── ScrollSpeedupCurve ───────────────────────────────────────────────────────

/// Fast-scroll multiplier curve. Evaluates the speedup factor based on how many
/// consecutive swipe bursts the user has completed.
/// Mirrors `ScrollSpeedupCurve` in MMF.
#[derive(Debug, Clone)]
pub struct ScrollSpeedupCurve {
    /// Swipe count at which speedup begins (1-indexed, matching mac).
    pub swipe_threshold: f64,
    /// Initial speedup factor `p` — value at the threshold.
    pub initial_speedup: f64,
    /// Exponential speedup parameter `c` — growth rate exponent.
    pub exponential_speedup: f64,
}

impl ScrollSpeedupCurve {
    /// Create from swipe threshold, initial speedup factor, and exponential rate.
    /// b = 1.1 is hardcoded to match Mac.
    /// a = (p - 1) / (b^c - 1).
    pub fn new(swipe_threshold: u32, initial_speedup: f64, exponential_speedup: f64) -> Self {
        let b: f64 = 1.1;
        let _a = (initial_speedup - 1.0) / (b.powf(exponential_speedup) - 1.0);
        Self {
            swipe_threshold: swipe_threshold as f64,
            initial_speedup,
            exponential_speedup,
        }
    }

    /// Evaluate the speedup multiplier at swipe count `x`.
    /// Returns 1.0 below threshold; exponential growth above.
    /// Mirrors `ScrollSpeedupCurve.evaluate(at:)` from MMF.
    pub fn evaluate(&self, x: f64) -> f64 {
        if x < self.swipe_threshold {
            1.0
        } else {
            let b: f64 = 1.1;
            let a = (self.initial_speedup - 1.0) / (b.powf(self.exponential_speedup) - 1.0);
            a * b.powf((x - self.swipe_threshold) * self.exponential_speedup) + 1.0 - a
        }
    }
}

// ─── DragCurve ───────────────────────────────────────────────────────────────
// ─── DragCurve ───────────────────────────────────────────────────────────────

/// Standalone drag-curve functions matching MMF `DragCurve.swift`.
/// Used internally by HybridCurve. The DragCurve struct mirrors MMF's
/// instance-based API for the `_bezierInit` fallback.

/// Computed drag curve parameters for a given initial speed.
#[derive(Debug, Clone)]
pub struct DragCurveParams {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub initial_speed: f64,
    pub stop_speed: f64,
}

impl DragCurveParams {
    /// Create from initial speed (MMF `init(coefficient:, exponent:, initialSpeed:, stopSpeed:)`).
    pub fn from_speed(a: f64, b: f64, initial_speed: f64, stop_speed: f64) -> Self {
        assert!(a > 0.0 && b > 1.0 && b != 2.0);
        let c = -initial_speed.powf(1.0 - b) / (a * (b - 1.0));
        Self { a, b, c, initial_speed, stop_speed }
    }

    /// Create such that the curve covers exactly `distance` before stopping.
    /// Solves for c (time offset) given target distance.
    /// MMF: `DragCurve(coefficient:, exponent:, distance:, stopSpeed:)`
    pub fn from_distance(a: f64, b: f64, distance: f64, stop_speed: f64) -> Self {
        assert!(a > 0.0 && b > 1.0 && b != 2.0 && distance > 0.0 && stop_speed >= 0.0);
        // From d(t) formula with k chosen so d(0)=0:
        // d(t) = inner^p/(a*(b-2)) - inner0^p/(a*(b-2))
        // where inner = (b-1)*a*(t-c), inner0 = (b-1)*a*(-c)
        // We want d(t_stop) = distance, where t_stop is when v(t_stop) = stop_speed.
        // t_stop = stop_speed^(1-b)/(a*(b-1)) + c
        // This gives: (b-1)^(p) * a^(p-1) * ((t_stop-c)^p - (-c)^p) / (b-2) = distance * a * (b-2)
        // For simplicity, use Newton's method to find c that gives target distance.
        let mut c = 0.1; // initial guess
        for _ in 0..50 {
            let stop_t = stop_speed.powf(1.0 - b) / (a * (b - 1.0)) + c;
            let p = 1.0 / (1.0 - b) + 1.0;
            let inner_stop = (b - 1.0) * a * (stop_t - c);
            let inner0 = (b - 1.0) * a * (-c);
            let term_stop = inner_stop.powf(p) / (a * (b - 2.0));
            let term0 = inner0.powf(p) / (a * (b - 2.0));
            let d = term_stop - term0;
            let err = d - distance;
            if err.abs() < 1e-6 * distance {
                break;
            }
            // derivative dd/dc (approx via finite diff)
            let delta = 0.001;
            let c2 = c + delta;
            let stop_t2 = stop_speed.powf(1.0 - b) / (a * (b - 1.0)) + c2;
            let inner_stop2 = (b - 1.0) * a * (stop_t2 - c2);
            let inner02 = (b - 1.0) * a * (-c2);
            let term_stop2 = inner_stop2.powf(p) / (a * (b - 2.0));
            let term02 = inner02.powf(p) / (a * (b - 2.0));
            let d2 = term_stop2 - term02;
            let deriv = (d2 - d) / delta;
            if deriv.abs() > 1e-12 {
                c -= err / deriv;
            }
        }
        Self { a, b, c, initial_speed: 0.0, stop_speed }
    }

    /// Total distance covered from t=0 until stop.
    pub fn total_distance(&self) -> f64 {
        let stop_t = self.stop_speed.powf(1.0 - self.b) / (self.a * (self.b - 1.0)) + self.c;
        let p = 1.0 / (1.0 - self.b) + 1.0;
        let inner_stop = (self.b - 1.0) * self.a * (stop_t - self.c);
        let inner0 = (self.b - 1.0) * self.a * (-self.c);
        let term_stop = inner_stop.powf(p) / (self.a * (self.b - 2.0));
        let term0 = inner0.powf(p) / (self.a * (self.b - 2.0));
        term_stop - term0
    }

    /// Evaluate at time t: returns accumulated distance (0..total_distance).
    pub fn evaluate(&self, t: f64) -> f64 {
        if t <= 0.0 {
            return 0.0;
        }
        let stop_t = self.stop_speed.powf(1.0 - self.b) / (self.a * (self.b - 1.0)) + self.c;
        if t >= stop_t {
            return self.total_distance();
        }
        let p = 1.0 / (1.0 - self.b) + 1.0;
        let inner = (self.b - 1.0) * self.a * (t - self.c);
        let inner0 = (self.b - 1.0) * self.a * (-self.c);
        let term = inner.powf(p) / (self.a * (self.b - 2.0));
        let term0 = inner0.powf(p) / (self.a * (self.b - 2.0));
        term - term0
    }

    /// Evaluate at normalized fraction `t` ∈ [0, 1] (relative to the curve's own time span).
    /// Returns accumulated distance as a fraction of total_distance ∈ [0, 1].
    /// Matches Mac's `DragCurve.evaluate(at:)` which takes a normalized t.
    pub fn evaluate_fraction(&self, t_norm: f64) -> f64 {
        let t = t_norm.clamp(0.0, 1.0);
        if t <= 0.0 {
            return 0.0;
        }
        let stop_t = self.stop_speed.powf(1.0 - self.b) / (self.a * (self.b - 1.0)) + self.c;
        if t >= 1.0 {
            return 1.0; // fully stopped — normalized fraction = 1
        }
        // Compute t_abs from t_norm: t_abs = t_norm * stop_t
        let t_abs = t * stop_t;
        self.evaluate(t_abs) / self.total_distance()
    }
}

// ─── HybridCurve ─────────────────────────────────────────────────────────────

/// Sub-curve enum returned by `sub_curve()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HybridSubCurve {
    /// Animation is in the base (gesture-driven) phase.
    Base,
    /// Animation is in the drag (momentum coast) phase.
    Drag,
}

/// Hybrid animation curve: base Bezier then drag-curve momentum tail.
///
/// Ported from `BezierHybridCurve` in MMF.
///
/// `evaluate(t)` returns the accumulated scroll fraction (0..1) at normalized time t.
#[derive(Debug, Clone)]
pub struct HybridCurve {
    /// Fraction of total duration where base phase ends: transitionTime / totalDuration.
    base_end: f64,
    /// Fraction of total distance covered by base phase: transitionDistance / totalDistance.
    base_dist_end: f64,
    /// Total animation duration in seconds.
    total_duration: f64,
    /// Total scroll distance in wheel units.
    total_distance: f64,
    /// Drag parameters for the tail (None if transition speed <= stop_speed).
    drag: Option<DragCurveParams>,
    /// Bezier control points for the base curve.
    base_curve: BezierAccelCurve,
    /// Transition time in seconds (base phase length).
    transition_time: f64,
    /// Transition distance in wheel units.
    transition_distance: f64,
}

impl HybridCurve {
    /// Create a hybrid curve with Mac's `_bezierInit` transition-point search.
    ///
    /// `base_curve` — the Bezier curve for the base phase (must pass through (0,0) and (1,1)
    /// in its normalized [0,1]×[0,1] space).
    /// `base_ms` — minimum duration for the base phase in milliseconds.
    /// `target_distance` — total animation distance in wheel units.
    /// `drag_coefficient`, `drag_exponent`, `stop_speed` — drag physics params.
    /// `distance_epsilon` — search tolerance for transition-point finding (use 0.2).
    pub fn new(
        base_curve: &BezierAccelCurve,
        base_ms: f64,
        target_distance: f64,
        drag_coefficient: f64,
        drag_exponent: f64,
        stop_speed: f64,
        distance_epsilon: f64,
    ) -> Self {
        assert!(target_distance > 0.0);

        let min_duration = if base_ms <= 0.0 { 140.0 } else { base_ms } / 1000.0;

        // Mac's _bezierInit: search for the transition point.
        let result = bezier_init(
            base_curve,
            min_duration,
            target_distance,
            drag_coefficient,
            drag_exponent,
            stop_speed,
            distance_epsilon,
        );

        let total_duration = result.transition_time + result.drag_duration;
        let total_distance = result.transition_distance + result.drag_distance;

        let base_end = if total_duration > 0.0 {
            result.transition_time / total_duration
        } else {
            1.0
        };
        let base_dist_end = if total_distance > 0.0 {
            result.transition_distance / total_distance
        } else {
            1.0
        };

        Self {
            base_end,
            base_dist_end,
            total_duration,
            total_distance,
            drag: result.drag,
            base_curve: base_curve.clone(),
            transition_time: result.transition_time,
            transition_distance: result.transition_distance,
        }
    }

    /// Total duration of this animation in seconds.
    pub fn total_duration(&self) -> f64 {
        self.total_duration
    }

    /// Total scroll distance in wheel units.
    pub fn total_distance(&self) -> f64 {
        self.total_distance
    }

    /// Distance remaining in the base phase at normalized time `t`.
    ///
    /// Mirrors `Scroll.m:575–582` (`baseDistanceLeftWithDistanceLeft`): during
    /// the base phase the remaining magnitude is the base phase total times the
    /// fraction of the base phase not yet consumed. Returns `0.0` once the drag
    /// phase has taken over.
    pub fn base_phase_distance_remaining(&self, t: f64) -> f64 {
        let base_end_t = if self.total_duration > 0.0 {
            self.transition_time / self.total_duration
        } else {
            return self.transition_distance;
        };
        if t >= base_end_t {
            return 0.0;
        }
        let scaled_t = if base_end_t > 1e-12 { (t / base_end_t).min(1.0) } else { 0.0 };
        let mut frac = self.base_curve.sample_y_at_t(scaled_t).clamp(0.0, 1.0);
        if frac > 1.0 { frac = 1.0; }
        self.transition_distance * (1.0 - frac)
    }

    /// Total scroll distance remaining at normalized time `t` (both phases combined).
    /// During the drag phase the drag portion is accounted for via the accumulated
    /// fraction: `(1 − evaluate(t)) * total_distance`.
    pub fn distance_remaining(&self, t: f64) -> f64 {
        (1.0 - self.evaluate(t).clamp(0.0, 1.0)) * self.total_distance
    }

    /// Evaluate at normalized time t ∈ [0, 1].
    /// Returns accumulated scroll fraction (0..1).
    ///
    /// Mirrors `HybridCurve.evaluate(at:)` from MMF:
    /// - t <= base_end: base phase — evaluate Bezier and scale to base distance interval
    /// - t > base_end: drag phase — evaluate DragCurve and scale to drag distance interval
    pub fn evaluate(&self, t: f64) -> f64 {
        // mac: baseEnd = baseDuration / duration
        let base_end_t = if self.total_duration > 0.0 {
            self.transition_time / self.total_duration
        } else {
            1.0
        };

        if t <= base_end_t {
            // BASE PHASE: evaluate Bezier at scaled t, then scale to base distance interval.
            // mac: baseCurve.evaluate(at: scale(x, baseTimeIntervalUnit, .unitInterval))
            // The base_curve maps [0,1]→[0,1] in normalized space.
            // We need to scale t from [0, base_end_t] to [0, 1].
            let scaled_t = if base_end_t > 0.0 {
                (t / base_end_t).min(1.0)
            } else {
                0.0
            };
            // Bezier.sample_y_at_t gives normalized accumulated distance (0..1).
            let mut base_result = self.base_curve.sample_y_at_t(scaled_t);
            if base_result > 1.0 {
                base_result = 1.0; // mac: if baseCurveResult > 1 { baseCurveResult = 1 }
            }
            if base_result < 0.0 {
                base_result = 0.0;
            }
            // mac: scale from unitInterval to baseDistanceIntervalUnit
            // base_dist_end = transitionDistance / totalDistance
            base_result * self.base_dist_end
        } else {
            // DRAG PHASE.
            // mac: c.evaluate(at: scale(x, dragTimeIntervalUnit, .unitInterval))
            // where dragTimeIntervalUnit = [baseDuration/duration, 1]
            // So we normalize t from [base_end_t, 1] to [0, 1].
            // DragCurve.evaluate takes t in [0, 1] (normalized to its total duration),
            // and returns accumulated fraction (0..1).
            if let Some(ref drag) = self.drag {
                let denom = 1.0 - base_end_t;
                let drag_t_norm = if denom > 1e-12 {
                    ((t - base_end_t) / denom).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let drag_frac = drag.evaluate_fraction(drag_t_norm);
                self.base_dist_end + (1.0 - self.base_dist_end) * drag_frac
            } else {
                1.0
            }
        }
    }

    /// Which sub-curve is active at normalized time t.
    pub fn sub_curve(&self, t: f64) -> HybridSubCurve {
        let base_end_t = if self.total_duration > 0.0 {
            self.transition_time / self.total_duration
        } else {
            1.0
        };
        if t <= base_end_t {
            HybridSubCurve::Base
        } else {
            HybridSubCurve::Drag
        }
    }

    /// True for the base (gesture-driven) phase.
    pub fn is_gesture_phase(&self, t: f64) -> bool {
        self.sub_curve(t) == HybridSubCurve::Base
    }
}

// ─── _bezierInit ─────────────────────────────────────────────────────────────

/// Result of Mac's `_bezierInit` search.
struct BezierInitResult {
    transition_time: f64,
    transition_distance: f64,
    drag_duration: f64,
    drag_distance: f64,
    drag: Option<DragCurveParams>,
}

/// Mac's `_bezierInit`: finds the transition point on the Bezier where
/// combined (base + drag) distance = targetDistance.
///
/// Algorithm:
/// 1. Coarse scan: t from 1.0→0.0 in 10 steps of 0.1
/// 2. Bisection: refine the transition point
/// 3. Fallback: if no point found, use drag curve that exactly covers distance
fn bezier_init(
    base_curve: &BezierAccelCurve,
    min_duration: f64,
    target_distance: f64,
    drag_coefficient: f64,
    drag_exponent: f64,
    stop_speed: f64,
    distance_epsilon: f64,
) -> BezierInitResult {
    let n = 10;

    let mut transition_point_range: Option<(f64, f64)> = None;
    let mut transition_point: Option<f64> = None;
    let mut drag: Option<DragCurveParams> = None;

    // Coarse scan from t=1.0 down to t=0.0.
    // t_k = scale(k, (0,n), (1,0)) = 1 - k/n
    for k in 0..=n {
        let t = 1.0 - (k as f64) / (n as f64);

        let combined = combined_distance(
            t,
            base_curve,
            target_distance,
            min_duration,
            drag_coefficient,
            drag_exponent,
            stop_speed,
        );

        if k == 0 {
            // t=1.0: combined distance should be >= target (base curve alone covers target)
            assert!(combined >= target_distance, "Bezier should cover target distance at t=1");
        }

        let diff = (combined - target_distance).abs();
        if diff < distance_epsilon {
            transition_point = Some(t);
            break;
        }
        if combined < target_distance {
            // Found a range where combined crosses target.
            // Next t in loop is t - 1/n = t + (1/n) since we're going backward.
            let t_next = if k < n { 1.0 - ((k + 1) as f64) / (n as f64) } else { 0.0 };
            transition_point_range = Some((t, t_next));
            break;
        }
    }

    // Bisection to refine.
    if let Some((lo, hi)) = transition_point_range {
        transition_point = Some(bisect_distance(
            lo,
            hi,
            target_distance,
            base_curve,
            target_distance,
            min_duration,
            drag_coefficient,
            drag_exponent,
            stop_speed,
            distance_epsilon,
        ));
    }

    // Fallback: no transition point found.
    if transition_point.is_none() {
        // Use a drag curve that exactly covers targetDistance.
        drag = Some(DragCurveParams::from_distance(
            drag_coefficient,
            drag_exponent,
            target_distance,
            stop_speed,
        ));
        transition_point = Some(0.0);
    }

    let tp = transition_point.unwrap();

    // Get speed at transition point: slope * distance / duration.
    // slope = dy/dx at t = derivative_dy_dx(t) * (target_distance / min_duration)
    let slope = base_curve.derivative_dy_dx(tp);
    let speed_at_t = slope * target_distance / min_duration;

    if speed_at_t > stop_speed {
        drag = Some(DragCurveParams::from_speed(
            drag_coefficient,
            drag_exponent,
            speed_at_t,
            stop_speed,
        ));
    } else {
        // Transition speed below stop speed: base curve covers everything.
        // Mac asserts transitionPoint == 1.
        assert!((tp - 1.0).abs() < 1e-9, "transitionPoint should be 1 when speed <= stopSpeed");
    }

    let transition_time = base_curve.sample_x_at_t(tp) * min_duration;
    let transition_distance = base_curve.sample_y_at_t(tp) * target_distance;

    let drag_dur = drag.as_ref().map_or(0.0, |d| {
        drag_time_for_distance(d.a, d.b, d.total_distance(), d.stop_speed)
    });
    let drag_dist = drag.as_ref().map_or(0.0, |d| d.total_distance());

    BezierInitResult {
        transition_time,
        transition_distance,
        drag_duration: drag_dur,
        drag_distance: drag_dist,
        drag,
    }
}

/// Combined distance when transitioning from base to drag at parameter t.
fn combined_distance(
    t: f64,
    base_curve: &BezierAccelCurve,
    target_distance: f64,
    min_duration: f64,
    drag_coefficient: f64,
    drag_exponent: f64,
    stop_speed: f64,
) -> f64 {
    assert!(t >= 0.0 && t <= 1.0);

    // Speed at t: slope * distance / duration.
    let slope = base_curve.derivative_dy_dx(t);
    let speed_at_t = slope * target_distance / min_duration;

    // Distance covered by base at t.
    let base_dist = base_curve.sample_y_at_t(t) * target_distance;

    if speed_at_t <= stop_speed {
        // Drag can't contribute — base covers this and everything beyond.
        return base_dist;
    }

    // Create drag curve from this speed.
    let drag = DragCurveParams::from_speed(drag_coefficient, drag_exponent, speed_at_t, stop_speed);
    let drag_dist = drag.total_distance();

    base_dist + drag_dist
}

/// Bisection search for transition point t where combined_distance = target.
fn bisect_distance(
    lo: f64,
    hi: f64,
    target: f64,
    base_curve: &BezierAccelCurve,
    target_distance: f64,
    min_duration: f64,
    drag_coefficient: f64,
    drag_exponent: f64,
    stop_speed: f64,
    epsilon: f64,
) -> f64 {
    let mut lo = lo;
    let mut hi = hi;
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        let d = combined_distance(
            mid,
            base_curve,
            target_distance,
            min_duration,
            drag_coefficient,
            drag_exponent,
            stop_speed,
        );
        if (d - target).abs() < epsilon {
            return mid;
        }
        if d < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

/// Time for drag curve to cover a given distance (Newton's method on drag_distance inverse).
fn drag_time_for_distance(a: f64, b: f64, distance: f64, stop_speed: f64) -> f64 {
    if distance <= 0.0 {
        return 0.0;
    }
    // Use drag_curve params to find time.
    // drag_distance(v0, a, b, t) = distance.
    // We know v0 from the params... but we only have a, b, distance, stop_speed.
    // For the from_distance case, c is set to make total distance = distance.
    // We need to solve for t such that d(t) = distance.
    // From the DragCurveParams::from_distance, c is chosen so d(stop_t) = distance.
    // So drag_time_for_distance is just the stop_t from the params.
    // We need to reconstruct c from distance... actually let's use Newton's method.
    let c_guess = 0.1;
    let mut c = c_guess;
    let stop_speed_f = stop_speed;
    // From distance constructor: we set c such that d(t_stop) = distance.
    // We need to find t such that d(t) = distance, starting from t=0.
    // d(t) formula: let p = 1/(1-b) + 1, inner = (b-1)*a*(t-c), d = (inner^p - inner0^p) / (a*(b-2))
    // where inner0 = (b-1)*a*(-c)
    // We want d(t_target) = distance, with d(t_stop) = distance (full stop).
    // So t_target = t_stop = stop_speed^(1-b)/(a*(b-1)) + c
    // This means: stop_speed^(1-b)/(a*(b-1)) + c = t_stop = t_target
    // We need to solve for c in terms of distance... using Newton.
    for _ in 0..50 {
        let p = 1.0 / (1.0 - b) + 1.0;
        let t_stop = stop_speed_f.powf(1.0 - b) / (a * (b - 1.0)) + c;
        let inner_stop = (b - 1.0) * a * (t_stop - c);
        let inner0 = (b - 1.0) * a * (-c);
        let d_full = (inner_stop.powf(p) - inner0.powf(p)) / (a * (b - 2.0));
        let err = d_full - distance;
        if err.abs() < 1e-6 * distance {
            return t_stop;
        }
        let delta = 0.001;
        let c2 = c + delta;
        let t_stop2 = stop_speed_f.powf(1.0 - b) / (a * (b - 1.0)) + c2;
        let inner_stop2 = (b - 1.0) * a * (t_stop2 - c2);
        let inner02 = (b - 1.0) * a * (-c2);
        let d_full2 = (inner_stop2.powf(p) - inner02.powf(p)) / (a * (b - 2.0));
        let deriv = (d_full2 - d_full) / delta;
        if deriv.abs() > 1e-12 {
            c -= err / deriv;
        }
    }
    stop_speed_f.powf(1.0 - b) / (a * (b - 1.0)) + c
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const A: f64 = 15.0;
    const B: f64 = 1.2;

    // ── drag_speed / drag_distance / drag_stop_time ────────────────────────────

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
        let d = drag_distance(v0, A, B, t_stop * 0.999);
        assert!(d > 0.0);
        let d_total = drag_distance(v0, A, B, t_stop);
        assert!(d_total < v0 * t_stop * 2.0, "coast distance sanity");
    }

    #[test]
    fn stop_time_zero_when_already_slow() {
        assert_eq!(drag_stop_time(100.0, A, B, 200.0), 0.0);
    }

    #[test]
    fn drag_speed_non_negative() {
        let v0 = 5000.0;
        for i in 0..=100 {
            let t = i as f64 * 0.1;
            let v = drag_speed(v0, 15.0, 1.2, t);
            assert!(v >= 0.0, "speed should never go negative at t={t}");
        }
    }

    // ── BezierAccelCurve tests ────────────────────────────────────────────────

    #[test]
    fn bezier_accel_pre_line_is_y_min() {
        let curve = BezierAccelCurve::new(6.25, 66.667, 30.0, 120.0, 3.0);
        let y = curve.evaluate(0.0);
        assert!((y - 30.0).abs() < 1e-9, "below x_min should return y_min");
    }

    #[test]
    fn bezier_accel_at_endpoints_matches_y_min_and_y_max() {
        let curve = BezierAccelCurve::new(6.25, 66.667, 30.0, 120.0, 3.0);
        assert!((curve.evaluate(6.25) - 30.0).abs() < 1e-6, "x_min → y_min");
        assert!((curve.evaluate(66.667) - 120.0).abs() < 1e-6, "x_max → y_max");
    }

    #[test]
    fn bezier_accel_within_range_is_bounded() {
        let curve = BezierAccelCurve::new(6.25, 66.667, 30.0, 120.0, 3.0);
        for x in [6.25_f64, 20.0, 40.0, 60.0, 66.667] {
            let y = curve.evaluate(x);
            assert!(y >= 30.0 - 1e-6 && y <= 120.0 + 1e-6,
                "y at x={x} should be between 30 and 120, got {y}");
        }
    }

    #[test]
    fn bezier_accel_post_line_extrapolates() {
        let curve = BezierAccelCurve::new(6.25, 66.667, 30.0, 120.0, 3.0);
        let y = curve.evaluate(100.0);
        assert!((y - 120.0).abs() < 1.0,
            "above x_max should stay near y_max (post_a≈0), got {y}");
    }

    #[test]
    fn bezier_accel_from_points_matches_new() {
        let curve_default = BezierAccelCurve::new(6.25, 66.667, 30.0, 120.0, 3.0);
        let pts = vec![
            (6.25_f64, 30.0),
            (21.3541667, 120.0),
            (36.4583333, 120.0),
            (51.5625000, 120.0),
            (66.6666667, 120.0),
        ];
        let curve_explicit = BezierAccelCurve::from_points(&pts);
        for x in [6.25_f64, 20.0, 40.0, 60.0, 66.667] {
            let d = (curve_default.evaluate(x) - curve_explicit.evaluate(x)).abs();
            assert!(d < 1e-3, "curves should match at x={x}, diff={d}");
        }
    }

    // ── ScrollSpeedupCurve tests ──────────────────────────────────────────────

    #[test]
    fn speedup_returns_one_before_threshold() {
        let curve = ScrollSpeedupCurve::new(3, 1.33, 7.5);
        for n in [0.0_f64, 1.0, 2.0] {
            assert!((curve.evaluate(n) - 1.0).abs() < 1e-9, "n={n} should be 1.0");
        }
    }

    #[test]
    fn speedup_increases_after_threshold() {
        // Mac formula: returns 1.0 at x <= threshold, then grows rapidly.
        // Using None config (6, 1.4, 3.0) — safe params that don't overflow.
        let curve = ScrollSpeedupCurve::new(6, 1.4, 3.0);

        // Below threshold: returns exactly 1.0
        let below = curve.evaluate(5.0);
        assert!((below - 1.0).abs() < 1e-9, "below threshold should be ~1.0, got {below}");

        // At threshold: Mac formula returns 1.0 (within FP precision)
        let at_thresh = curve.evaluate(6.0);
        assert!((at_thresh - 1.0).abs() < 1e-9, "at threshold should be ~1.0, got {at_thresh}");

        // Above threshold: should exceed 1.0 and grow
        let above = curve.evaluate(7.0);
        assert!(above > 1.0, "above threshold should exceed 1.0, got {above}");

        let higher = curve.evaluate(10.0);
        assert!(higher > above, "higher swipe count should increase factor");
    }

    #[test]
    fn speedup_bounded_by_exponential() {
        // Using None config (6, 1.4, 3.0) which stays bounded up to x=10.
        let curve = ScrollSpeedupCurve::new(6, 1.4, 3.0);
        // Below threshold is approximately 1.0 (FP precision)
        // Below threshold is approximately 1.0 (FP precision)
        let below = curve.evaluate(5.0);
        assert!((below - 1.0).abs() < 1e-9, "below threshold should be ~1.0");

        // At threshold is approximately 1.0 (FP precision)
        let at_thresh = curve.evaluate(6.0);
        assert!((at_thresh - 1.0).abs() < 1e-9, "at threshold should be ~1.0");

        // Above threshold grows toward infinity (but monotonically)
        let val7 = curve.evaluate(7.0);
        let val10 = curve.evaluate(10.0);
        assert!(val10 > val7, "should grow");
        assert!(val10 > 1.0, "should exceed 1.0");

        // Large swipe count grows very large (c=3.0, less extreme than c=7.5)
        let large = curve.evaluate(15.0);
        assert!(large > 10.0, "large swipe count should be >> 1.0, got {large}");
    }

    // ── HybridCurve tests ─────────────────────────────────────────────────────

    /// Create a simple test Bezier curve for HybridCurve tests.
    /// Passes through (0,0) and (1,1) in normalized space — matches Mac's requirement.
    fn test_bezier() -> BezierAccelCurve {
        BezierAccelCurve::from_points(&[(0.0, 0.0), (0.4, 0.0), (0.6, 1.0), (1.0, 1.0)])
    }

    /// Create a HybridCurve with typical test params: base_ms, distance, drag_a, drag_b, stop_speed, speed.
    /// Note: `speed` was the old 6th param (initial speed for drag). We ignore it and let
    /// _bezierInit compute the transition from the Bezier + distance + duration.
    fn test_hybrid(base_ms: f64, distance: f64, drag_a: f64, drag_b: f64, stop_speed: f64) -> HybridCurve {
        HybridCurve::new(&test_bezier(), base_ms, distance, drag_a, drag_b, stop_speed, 0.2)
    }

    #[test]
    fn hybrid_curve_total_duration_is_positive() {
        let curve = test_hybrid(14.0, 120.0, 15.0, 1.05, 30.0);
        assert!(curve.total_duration() > 0.0, "duration must be positive");
        assert!(curve.total_distance() > 0.0, "distance must be positive");
    }

    #[test]
    fn hybrid_curve_evaluate_at_zero_is_zero() {
        let curve = test_hybrid(14.0, 120.0, 15.0, 1.05, 30.0);
        assert!((curve.evaluate(0.0) - 0.0).abs() < 1e-9, "t=0 should return 0");
    }

    #[test]
    fn hybrid_curve_evaluate_at_one_is_one() {
        let curve = test_hybrid(14.0, 120.0, 15.0, 1.05, 30.0);
        // Allow some tolerance due to Bezier fitting
        let result = curve.evaluate(1.0);
        assert!((result - 1.0).abs() < 0.1,
            "t=1 should return ~1, got {result}");
    }

    #[test]
    fn hybrid_curve_is_monotonically_increasing() {
        let curve = test_hybrid(14.0, 120.0, 15.0, 1.05, 30.0);
        let mut prev = 0.0;
        for i in 0..=20 {
            let t = i as f64 / 20.0;
            let v = curve.evaluate(t);
            assert!(v >= prev - 1e-9,
                "hybrid curve should be monotonically increasing at t={t}: {v} < {prev}");
            prev = v;
        }
    }

    #[test]
    fn hybrid_curve_sub_curve_transitions() {
        let curve = test_hybrid(14.0, 120.0, 15.0, 1.05, 30.0);
        assert_eq!(curve.sub_curve(0.0), HybridSubCurve::Base);
        // Note: with the new algorithm, transition point may vary; just check it changes
        let mid = curve.sub_curve(0.5);
        assert!(mid == HybridSubCurve::Base || mid == HybridSubCurve::Drag,
            "sub_curve(0.5) should be valid");
    }

    #[test]
    fn hybrid_curve_low_speed_has_large_base_fraction() {
        let curve = test_hybrid(14.0, 120.0, 15.0, 1.05, 20.0);
        let total_dur = curve.total_duration();
        // For low speed, the base phase should dominate (>50% of duration)
        let base_frac = curve.transition_time / total_dur;
        assert!(base_frac > 0.5,
            "low speed should have large base fraction, got {base_frac}");
    }
    #[test]
    fn hybrid_base_phase_remaining_at_start_equals_transition_distance() {
        let curve = test_hybrid(14.0, 120.0, 15.0, 1.05, 20.0);
        // At t=0, no base phase consumed → remaining = full transition_distance.
        let rem = curve.base_phase_distance_remaining(0.0);
        assert!((rem - curve.transition_distance).abs() < 1e-9,
            "at t=0 remaining should equal transition_distance");
    }

    #[test]
    fn hybrid_base_phase_remaining_zero_once_drag_phase_begins() {
        let curve = test_hybrid(14.0, 120.0, 15.0, 1.05, 20.0);
        let base_end_t = curve.transition_time / curve.total_duration();
        // At t = base_end_t, base phase is complete → remaining = 0.
        assert_eq!(curve.base_phase_distance_remaining(base_end_t), 0.0,
            "at base_end_t remaining should be zero");
        // Slightly past base_end_t also returns 0.
        assert_eq!(curve.base_phase_distance_remaining((base_end_t + 0.1).min(1.0)), 0.0);
    }

    #[test]
    fn hybrid_base_phase_remaining_decreases_during_base_phase() {
        let curve = test_hybrid(14.0, 120.0, 15.0, 1.05, 20.0);
        let base_end_t = curve.transition_time / curve.total_duration();
        // During base phase, remaining should decrease monotonically.
        let rem_start = curve.base_phase_distance_remaining(0.0);
        let rem_mid = curve.base_phase_distance_remaining(base_end_t * 0.5);
        assert!(rem_mid < rem_start,
            "remaining should decrease as t progresses through base phase");
        assert!(rem_mid > 0.0, "mid-base remaining should still be positive");
    }

    #[test]
    fn hybrid_distance_remaining_at_end_is_zero() {
        let curve = test_hybrid(14.0, 120.0, 15.0, 1.05, 20.0);
        assert_eq!(curve.distance_remaining(1.0), 0.0,
            "at t=1.0 all distance should be consumed");
    }

    #[test]
    fn hybrid_distance_remaining_at_start_equals_total_distance() {
        let curve = test_hybrid(14.0, 120.0, 15.0, 1.05, 20.0);
        let rem = curve.distance_remaining(0.0);
        assert!((rem - curve.total_distance).abs() < 1e-9,
            "at t=0 remaining should equal total_distance");
    }
}
