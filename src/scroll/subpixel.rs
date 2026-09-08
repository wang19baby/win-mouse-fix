//! Sub-pixel accumulation: accumulates fractional scroll amounts and emits
//! integer wheel deltas, keeping the remainder for the next emit. Prevents
//! jitter and preserves sub-notch motion that would otherwise be lost when
//! rounding.
//!
//! To make scrolling visually smooth, each emit is capped at `max_step` wheel
//! units: a large `wheel` value is subdivided across several ticks instead of
//! being dumped as one big jump. Backlog is bounded so a fast flick can't build
//! a long runaway tail.

#[derive(Debug, Clone)]
pub struct SubPixelAccumulator {
    acc: f64,
    threshold: f64,
    max_step: f64,
}

impl SubPixelAccumulator {
    pub fn new(threshold: f64, max_step: f64) -> Self {
        assert!(threshold > 0.0, "threshold must be > 0");
        assert!(max_step > 0.0, "max_step must be > 0");
        Self {
            acc: 0.0,
            threshold,
            max_step,
        }
    }

    /// Add `delta` (in wheel units) and return the integer amount to emit now.
    /// Emits at most `max_step` per call; the remainder (including any backlog
    /// beyond a few notches) is carried to the next call so motion is
    /// subdivided into small, smooth steps.
    pub fn add(&mut self, delta: f64) -> i32 {
        self.acc += delta;
        // Bound backlog: a sustained fast scroll can't accumulate an unbounded
        // tail that keeps gliding long after the wheel stops.
        let cap = self.max_step * 8.0;
        if self.acc.abs() > cap {
            self.acc = self.acc.signum() * cap;
        }
        if self.acc.abs() >= self.threshold {
            let step = self.acc.trunc().abs().min(self.max_step);
            let out = step * self.acc.signum();
            self.acc -= out;
            out as i32
        } else {
            0
        }
    }

    /// Flush any remaining accumulation >= 1.0 (call when scroll stops).
    pub fn flush(&mut self) -> i32 {
        if self.acc.abs() >= 1.0 {
            let out = self.acc.trunc();
            self.acc -= out;
            out as i32
        } else {
            0
        }
    }

    pub fn reset(&mut self) {
        self.acc = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_below_threshold() {
        let mut a = SubPixelAccumulator::new(1.0, 1e9);
        assert_eq!(a.add(0.4), 0);
        assert_eq!(a.add(0.4), 0);
        assert_eq!(a.add(0.4), 1); // 1.2 >= 1 -> emit 1, keep 0.2
    }

    #[test]
    fn emits_integer_part_and_keeps_fraction() {
        let mut a = SubPixelAccumulator::new(1.0, 1e9);
        assert_eq!(a.add(150.0), 150);
        assert_eq!(a.acc.abs() < 1e-9, true);
        assert_eq!(a.add(0.6), 0);
        assert_eq!(a.add(0.6), 1); // 1.2 -> emit 1
    }

    #[test]
    fn flush_emits_remainder() {
        let mut a = SubPixelAccumulator::new(1.0, 1e9);
        a.add(2.6); // emit 2, keep 0.6
        assert_eq!(a.flush(), 0); // 0.6 < 1, nothing
        assert_eq!(a.add(0.5), 1); // 1.1 -> emit 1, keep 0.1
        assert_eq!(a.flush(), 0);
    }

    #[test]
    fn caps_per_tick_emission() {
        let mut a = SubPixelAccumulator::new(1.0, 30.0);
        // 200 units requested, but only 30 emitted this tick; 170 carried.
        assert_eq!(a.add(200.0), 30);
        assert!((a.acc - 170.0).abs() < 1e-9);
        assert_eq!(a.add(0.0), 30); // drains 30 more
        assert!((a.acc - 140.0).abs() < 1e-9);
    }

    #[test]
    fn caps_negative_emission() {
        let mut a = SubPixelAccumulator::new(1.0, 30.0);
        assert_eq!(a.add(-200.0), -30);
        assert!((a.acc + 170.0).abs() < 1e-9);
    }

    #[test]
    fn bounds_backlog_on_sustained_input() {
        let mut a = SubPixelAccumulator::new(1.0, 30.0);
        // Far more than the 8-step backlog cap (240).
        a.add(10000.0);
        assert!(a.acc <= 240.0 + 1e-9);
    }

    #[test]
    fn test_subpixel_flush_emits_remainder() {
        // Use high add-threshold so add() doesn't drain what flush() should emit.
        let mut a = SubPixelAccumulator::new(10.0, 1e9);
        a.add(3.7); // acc = 3.7, below add threshold (10.0), returns 0
        assert_eq!(a.flush(), 3); // 3.7 >= 1.0 -> emit 3, keep ~0.7
        assert!((a.acc - 0.7).abs() < 1e-9);
        assert_eq!(a.flush(), 0); // 0.7 < 1.0 -> nothing
    }

    #[test]
    fn test_subpixel_multiple_accumulations() {
        let mut a = SubPixelAccumulator::new(1.0, 1e9);
        // Multiple small accumulations below threshold individually
        assert_eq!(a.add(0.3), 0);
        assert_eq!(a.add(0.3), 0);
        assert_eq!(a.add(0.3), 0); // total 0.9, still < 1
        assert_eq!(a.add(0.2), 1); // total 1.1, emit 1
                                   // Now add more small bits
        assert_eq!(a.add(0.4), 0);
        assert_eq!(a.add(0.4), 0);
        assert_eq!(a.add(0.4), 1); // 0.2 + 1.2 = 1.4 -> emit 1, keep 0.4
    }
}
