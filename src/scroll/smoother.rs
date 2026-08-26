//! Double exponential (Holt's linear trend) smoothing.
//!
//! Port of mac-mouse-fix's `DoubleExponentialSmoother`. Used to smooth the
//! scroll *velocity* and expose a momentum (trend) term.
//!
//! Level:  L_t = a·y_t + (1-a)·(L_{t-1} + T_{t-1})
//! Trend:  T_t = y·(L_t - L_{t-1}) + (1-y)·T_{t-1}
//! Forecast (h steps): ŷ_{t+h} = L_t + h·T_t
//!
//! The first input only establishes the initial level and is returned
//! unaltered; the second input establishes the initial trend and is also
//! returned unaltered. Smoothing begins on the third input.

/// Holt's linear trend smoothing.
#[derive(Debug, Clone)]
pub struct DoubleExponentialSmoother {
    /// Level (data) smoothing factor. 1 = no smoothing, 0 = frozen.
    a: f64,
    /// Trend smoothing factor. 1 = trend isn't smoothed.
    y: f64,
    l_prev: f64,
    t_prev: f64,
    init1: Option<f64>,
    initialized: bool,
}

impl DoubleExponentialSmoother {
    pub fn new(a: f64, y: f64) -> Self {
        assert!((0.0..=1.0).contains(&a), "a must be in [0,1]");
        assert!((0.0..=1.0).contains(&y), "y must be in [0,1]");
        Self {
            a,
            y,
            l_prev: 0.0,
            t_prev: 0.0,
            init1: None,
            initialized: false,
        }
    }

    /// Feed a value; returns the smoothed value (raw value for first two inputs).
    pub fn smooth(&mut self, value: f64) -> f64 {
        match self.init1 {
            None => {
                self.init1 = Some(value);
                value
            }
            Some(first) if !self.initialized => {
                self.l_prev = first;
                self.t_prev = value - first;
                self.initialized = true;
                value
            }
            Some(_) => {
                let l = self.a * value + (1.0 - self.a) * (self.l_prev + self.t_prev);
                let t = self.y * (l - self.l_prev) + (1.0 - self.y) * self.t_prev;
                self.l_prev = l;
                self.t_prev = t;
                l
            }
        }
    }

    #[allow(dead_code)]
    /// Forecast the value `steps` steps into the future using the current trend.
    pub fn predict(&self, steps: usize) -> f64 {
        self.l_prev + (steps as f64) * self.t_prev
    }

    #[allow(dead_code)]
    /// Current level (smoothed value at the last input).
    pub fn level(&self) -> f64 {
        self.l_prev
    }

    #[allow(dead_code)]
    /// Current trend (velocity estimate).
    pub fn trend(&self) -> f64 {
        self.t_prev
    }

    pub fn reset(&mut self) {
        self.init1 = None;
        self.initialized = false;
        self.l_prev = 0.0;
        self.t_prev = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_two_inputs_returned_unaltered() {
        let mut s = DoubleExponentialSmoother::new(0.5, 0.5);
        assert_eq!(s.smooth(10.0), 10.0);
        assert_eq!(s.smooth(14.0), 14.0);
    }

    #[test]
    fn constant_input_stabilizes_and_trend_is_zero() {
        let mut s = DoubleExponentialSmoother::new(0.5, 0.5);
        for _ in 0..30 {
            let v = s.smooth(7.0);
            if s.initialized {
                assert!((v - 7.0).abs() < 1e-6, "v={v}");
            }
        }
        assert!(s.trend().abs() < 1e-6);
    }

    #[test]
    fn sustained_ramp_keeps_trend() {
        let mut s = DoubleExponentialSmoother::new(0.5, 0.5);
        s.smooth(0.0);
        s.smooth(2.0); // trend = 2
        let mut v = 2.0;
        for _ in 0..40 {
            v += 2.0;
            s.smooth(v);
        }
        assert!((s.trend() - 2.0).abs() < 0.2, "trend={}", s.trend());
    }

    #[test]
    fn predict_uses_trend() {
        let mut s = DoubleExponentialSmoother::new(0.5, 0.5);
        s.smooth(0.0);
        s.smooth(1.0);
        s.smooth(3.0); // trend now ~2
        let p = s.predict(2);
        assert!((p - (s.level() + 2.0 * s.trend())).abs() < 1e-9);
    }

    #[test]
    fn reset_clears_state() {
        let mut s = DoubleExponentialSmoother::new(0.5, 0.5);
        s.smooth(5.0);
        s.smooth(9.0);
        s.smooth(13.0);
        s.reset();
        assert_eq!(s.smooth(1.0), 1.0);
        assert_eq!(s.smooth(1.0), 1.0);
    }

    #[test]
    #[should_panic]
    fn rejects_out_of_range_params() {
        let _ = DoubleExponentialSmoother::new(1.5, 0.5);
    }
}
