//! Sub-pixel accumulation: accumulates fractional scroll amounts and emits
//! integer wheel deltas once they cross a threshold, keeping the remainder for
//! the next emit. Prevents jitter and preserves sub-notch motion that would
//! otherwise be lost when rounding.

#[derive(Debug, Clone)]
pub struct SubPixelAccumulator {
    acc: f64,
    threshold: f64,
}

impl SubPixelAccumulator {
    pub fn new(threshold: f64) -> Self {
        assert!(threshold > 0.0, "threshold must be > 0");
        Self {
            acc: 0.0,
            threshold,
        }
    }

    /// Add `delta` (in wheel units) and return the integer amount to emit now.
    pub fn add(&mut self, delta: f64) -> i32 {
        self.acc += delta;
        if self.acc.abs() >= self.threshold {
            let out = self.acc.trunc();
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
        let mut a = SubPixelAccumulator::new(1.0);
        assert_eq!(a.add(0.4), 0);
        assert_eq!(a.add(0.4), 0);
        assert_eq!(a.add(0.4), 1); // 1.2 >= 1 -> emit 1, keep 0.2
    }

    #[test]
    fn emits_integer_part_and_keeps_fraction() {
        let mut a = SubPixelAccumulator::new(1.0);
        assert_eq!(a.add(150.0), 150);
        assert_eq!(a.acc.abs() < 1e-9, true);
        assert_eq!(a.add(0.6), 0);
        assert_eq!(a.add(0.6), 1); // 1.2 -> emit 1
    }

    #[test]
    fn flush_emits_remainder() {
        let mut a = SubPixelAccumulator::new(1.0);
        a.add(2.6); // emit 2, keep 0.6
        assert_eq!(a.flush(), 0); // 0.6 < 1, nothing
        assert_eq!(a.add(0.5), 1); // 1.1 -> emit 1, keep 0.1
        assert_eq!(a.flush(), 0);
    }
}
