#![allow(non_snake_case)] // L, T, a, y match academic notation

//! Holt–Winters Double Exponential Smoother.
//!
//! Ported from `mac-mouse-fix/Helper/Core/Smoothing/DoubleExponentialSmoother.swift`.
//!
//! Reference: <https://en.wikipedia.org/wiki/Exponential_smoothing#Double_exponential_smoothing>
//!
//! ## Algorithm
//!
//! For each input `Y` (a raw wheel delta or implied velocity):
//!
//! ```text
//! L = a * Y + (1 - a) * (Lprev + Tprev)   // level
//! T = y * (L - Lprev) + (1 - y) * Tprev   // trend
//! output = L
//! ```
//!
//! `a` ∈ (0, 1]: weight for new data — close to 1 = responsive, close to 0 = sluggish.
//! `y` ∈ (0, 1]: weight for trend — close to 1 = trend reacts fast, close to 0 = trend barely changes.
//!
//! ## Predict
//!
//! `predict(h)` (h steps ahead) returns `Lprev + h * Tprev`, used by the
//! injector to pre-fill the momentum queue so the first injected tick already
//! carries the predicted velocity.

/// Holt–Winters Double Exponential Smoother.
#[derive(Debug, Clone)]
pub struct DoubleExponentialSmoother {
    /// Data smoothing factor `a` ∈ (0, 1]. 1 = no smoothing.
    a: f64,
    /// Trend smoothing factor `y` ∈ (0, 1].
    y: f64,
    /// Previous smoothed level.
    Lprev: f64,
    /// Previous smoothed trend.
    Tprev: f64,
    /// How many values have been fed (0-indexed counter).
    usage_count: usize,
    /// Optional seed values to pre-populate Lprev / Tprev before the first real call.
    initial_value1: Option<f64>,
    initial_value2: Option<f64>,
}

impl DoubleExponentialSmoother {
    /// Construct a new smoother.
    ///
    /// `a` — data smoothing weight (1 = no smoothing, lower = smoother/more lagged).
    /// `y` — trend smoothing weight (higher = more responsive to changes in rate).
    ///
    /// # Panics
    /// Panics if `a` or `y` is outside `(0, 1]`.
    pub fn new(a: f64, y: f64) -> Self {
        assert!(a > 0.0 && a <= 1.0, "a must be in (0, 1]");
        assert!(y > 0.0 && y <= 1.0, "y must be in (0, 1]");
        Self {
            a,
            y,
            Lprev: f64::NAN,
            Tprev: f64::NAN,
            usage_count: 0,
            initial_value1: None,
            initial_value2: None,
        }
    }

    /// Construct with seed values.
    ///
    /// `initial1` and `initial2` are fed through `smooth()` before any real input
    /// so that the smoother is "warmed up" and produces sensible output from the
    /// first genuine call. The first two genuine inputs are always returned
    /// unmodified (the algorithm needs two points to establish a real trend).
    pub fn with_seeds(a: f64, y: f64, initial1: f64, initial2: f64) -> Self {
        let mut s = Self::new(a, y);
        s.initial_value1 = Some(initial1);
        s.initial_value2 = Some(initial2);
        // Warm up now.
        s.reset();
        s
    }

    /// Reset to initial state, re-seeding from `initial_value1/2` if provided.
    pub fn reset(&mut self) {
        self.usage_count = 0;
        self.Lprev = f64::NAN;
        self.Tprev = f64::NAN;
        if let Some(v1) = self.initial_value1 {
            let _ = self.smooth(v1);
        }
        if let Some(v2) = self.initial_value2 {
            let _ = self.smooth(v2);
        }
    }

    /// Feed a value, return the smoothed output.
    ///
    /// - 1st call: returns `value` unchanged (no prior state to smooth with).
    /// - 2nd call: derives a fake initial trend `T = Y - Lprev` and returns
    ///   `value` unchanged (still establishing state).
    /// - 3rd+ call: full Holt–Winters update.
    pub fn smooth(&mut self, value: f64) -> f64 {
        let Y = value;
        let L: f64;
        let T: f64;

        match self.usage_count {
            // No prior state — seed L only.
            0 => {
                L = Y;
                T = 0.0;
            }
            // One prior value — derive a fake trend and return Y unchanged.
            // (The trend will be used in the next step.)
            1 => {
                // Tprev doesn't exist yet, so we approximate: T ≈ Y - Lprev
                self.Tprev = Y - self.Lprev;
                L = Y;
                T = self.Tprev;
            }
            // Normal case: Holt–Winters update.
            _ => {
                L = self.a * Y + (1.0 - self.a) * (self.Lprev + self.Tprev);
                T = self.y * (L - self.Lprev) + (1.0 - self.y) * self.Tprev;
            }
        }

        self.usage_count += 1;
        self.Lprev = L;
        self.Tprev = T;

        L
    }

    /// Predict the smoothed value `steps` ticks into the future.
    ///
    /// `predict(h) = Lprev + h * Tprev`
    ///
    /// # Panics
    /// Panics if fewer than 2 values have been fed (insufficient state).
    pub fn predict(&self, steps: usize) -> f64 {
        assert!(
            self.usage_count >= 2,
            "predict() requires at least 2 values to be smoothed"
        );
        self.Lprev + (steps as f64) * self.Tprev
    }

    /// The most recent smoothed output.
    ///
    /// # Panics
    /// Panics if no values have been fed.
    pub fn last_smoothed(&self) -> f64 {
        assert!(self.usage_count >= 1, "last_smoothed() requires at least 1 value");
        self.Lprev
    }

    /// Whether `predict()` can be called (i.e. at least 2 values have been fed).
    pub fn can_predict(&self) -> bool {
        self.usage_count >= 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Convergence ────────────────────────────────────────────────────────────

    #[test]
    fn smooth_stable_input_converges() {
        // With constant input, the smoothed value should settle toward that value.
        let mut s = DoubleExponentialSmoother::new(0.3, 0.2);
        for i in 0..1000 {
            let out = s.smooth(42.0);
            if i > 0 {
                assert!(
                    out.is_finite(),
                    "output went inf/nan at iteration {i}"
                );
            }
        }
        // After 1000 constant inputs the smoothed value should be very close to 42.
        let final_out = s.smooth(42.0);
        assert!(
            (final_out - 42.0).abs() < 1e-6,
            "final output {final_out} should converge to 42"
        );
    }

    // ─── Trend direction ───────────────────────────────────────────────────────

    #[test]
    fn smooth_trend_follows_rising_input() {
        let mut s = DoubleExponentialSmoother::new(0.5, 0.5);
        let _ = s.smooth(0.0);
        let mut prev = s.smooth(0.0);
        for val in [1.0, 2.0, 3.0, 4.0, 5.0] {
            let out = s.smooth(val);
            assert!(
                out > prev,
                "rising input {val} should produce output {out} > prev {prev}"
            );
            prev = out;
        }
    }

    #[test]
    fn smooth_trend_follows_falling_input() {
        let mut s = DoubleExponentialSmoother::new(0.5, 0.5);
        let _ = s.smooth(10.0);
        let mut prev = s.smooth(10.0);
        for val in [8.0, 6.0, 4.0, 2.0, 0.0] {
            let out = s.smooth(val);
            assert!(
                out < prev,
                "falling input {val} should produce output {out} < prev {prev}"
            );
            prev = out;
        }
    }

    // ─── First two calls pass-through ─────────────────────────────────────────

    #[test]
    fn first_two_calls_return_input() {
        let mut s = DoubleExponentialSmoother::new(0.1, 0.1);
        assert_eq!(s.smooth(3.14), 3.14);
        assert_eq!(s.smooth(2.71), 2.71);
    }

    // ─── Predict ─────────────────────────────────────────────────────────────

    #[test]
    fn predict_is_linear_extrapolation() {
        let mut s = DoubleExponentialSmoother::new(0.5, 0.5);
        // Warm up.
        s.smooth(0.0);
        s.smooth(0.0);
        // Feed a constant slope so T stabilises.
        for _ in 0..50 {
            s.smooth(10.0);
        }
        assert!(
            s.can_predict(),
            "smoother should be able to predict after warm-up"
        );
        let pred1 = s.predict(1);
        let last = s.last_smoothed();
        let trend = s.Tprev;
        assert!(
            (pred1 - (last + trend)).abs() < 1e-9,
            "predict(1) should be Lprev + Tprev"
        );
        assert!(
            (s.predict(5) - (last + 5.0 * trend)).abs() < 1e-9,
            "predict(5) should be Lprev + 5*Tprev"
        );
    }

    #[test]
    #[should_panic(expected = "requires at least 2 values")]
    fn predict_panics_without_enough_data() {
        let mut s = DoubleExponentialSmoother::new(0.5, 0.5);
        s.smooth(1.0);
        let _ = s.predict(1);
    }

    // ─── Reset ───────────────────────────────────────────────────────────────

    #[test]
    fn reset_restores_initial_state() {
        let mut s = DoubleExponentialSmoother::with_seeds(0.3, 0.2, 1.0, 2.0);
        s.smooth(10.0);
        s.smooth(20.0);
        assert_ne!(s.usage_count, 0);
        s.reset();
        assert!(s.usage_count >= 2);
        let out = s.smooth(5.0);
        assert!(out.is_finite());
    }

    // ─── Edge cases ───────────────────────────────────────────────────────────

    #[test]
    fn zero_trend_input_produces_stable_output() {
        let mut s = DoubleExponentialSmoother::new(0.5, 0.5);
        s.smooth(0.0);
        s.smooth(0.0);
        for _ in 0..100 {
            let out = s.smooth(0.0);
            assert!(
                out.abs() < 1e-6,
                "zero input should produce near-zero output, got {out}"
            );
        }
    }

    #[test]
    fn negative_input_handled() {
        let mut s = DoubleExponentialSmoother::new(0.5, 0.5);
        s.smooth(-100.0);
        s.smooth(-100.0);
        let out = s.smooth(-50.0);
        assert!(out.is_finite());
    }

    #[test]
    fn with_seeds_skips_first_two_smoothing_passes() {
        // Seeds consumed in reset() → usage_count is 2.
        let mut s = DoubleExponentialSmoother::with_seeds(0.5, 0.5, 1.0, 2.0);
        assert_eq!(s.usage_count, 2);
        let out = s.smooth(3.0);
        assert!(out.is_finite());
        assert_eq!(s.usage_count, 3);
    }
}
