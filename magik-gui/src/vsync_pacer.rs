//! Host-testable vsync pacing policy.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VsyncPaceSource {
    Vsync,
    Fallback,
    Timeout,
    Error,
}

impl VsyncPaceSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vsync => "vsync",
            Self::Fallback => "fallback",
            Self::Timeout => "timeout",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug)]
pub struct VsyncPacerModel {
    period_us: u64,
    last_hit_us: Option<u64>,
    miss_streak: u32,
    degraded_threshold: u32,
    observed_max_miss_streak: u32,
    hits: u64,
    timeouts: u64,
    errors: u64,
    fallback_frames: u64,
}

impl VsyncPacerModel {
    pub fn new(period_us: u64, degraded_threshold: u32) -> Self {
        Self {
            period_us,
            last_hit_us: None,
            miss_streak: 0,
            degraded_threshold,
            observed_max_miss_streak: 0,
            hits: 0,
            timeouts: 0,
            errors: 0,
            fallback_frames: 0,
        }
    }

    pub fn period_us(&self) -> u64 {
        self.period_us
    }

    pub fn miss_streak(&self) -> u32 {
        self.miss_streak
    }

    pub fn max_miss_streak(&self) -> u32 {
        self.observed_max_miss_streak
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn fallback_frames(&self) -> u64 {
        self.fallback_frames
    }

    pub fn record_hit_at_us(&mut self, at_us: u64) {
        self.hits += 1;
        self.miss_streak = 0;
        if let Some(prev) = self.last_hit_us {
            let observed = at_us.saturating_sub(prev);
            if (8_000..=25_000).contains(&observed) {
                self.period_us = ((self.period_us * 7) + observed) / 8;
            }
        }
        self.last_hit_us = Some(at_us);
    }

    pub fn record_miss(&mut self, source: VsyncPaceSource) -> bool {
        match source {
            VsyncPaceSource::Timeout => self.timeouts += 1,
            VsyncPaceSource::Error => self.errors += 1,
            VsyncPaceSource::Fallback | VsyncPaceSource::Vsync => {}
        }
        self.miss_streak += 1;
        self.observed_max_miss_streak = self.observed_max_miss_streak.max(self.miss_streak);
        self.fallback_frames += 1;
        self.miss_streak >= self.degraded_threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FALLBACK_60_US: u64 = 16_667;

    #[test]
    fn learns_perfect_60hz() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 3);
        for i in 0..120 {
            model.record_hit_at_us(i * 16_667);
        }

        let inferred_hz = 1_000_000.0 / model.period_us() as f64;
        assert!(
            (59.5..=60.5).contains(&inferred_hz),
            "inferred_hz={inferred_hz}"
        );
    }

    #[test]
    fn learns_perfect_pal_50hz_without_snapping_to_60hz() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 3);
        for i in 0..120 {
            model.record_hit_at_us(i * 20_000);
        }

        let inferred_hz = 1_000_000.0 / model.period_us() as f64;
        assert!(
            (49.5..=50.5).contains(&inferred_hz),
            "inferred_hz={inferred_hz}"
        );
    }

    #[test]
    fn ten_isolated_misses_over_two_minutes_do_not_degrade() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 3);
        let mut now_us = 0;
        model.record_hit_at_us(now_us);

        for _ in 0..10 {
            now_us += 12_000_000;
            let degraded = model.record_miss(VsyncPaceSource::Timeout);
            assert!(!degraded);
            assert_eq!(model.miss_streak(), 1);
            now_us += FALLBACK_60_US;
            model.record_hit_at_us(now_us);
        }

        assert_eq!(model.fallback_frames(), 10);
        assert_eq!(model.max_miss_streak(), 1);
        assert_eq!(model.miss_streak(), 0);
    }

    #[test]
    fn three_consecutive_misses_enter_degraded_state() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 3);
        assert!(!model.record_miss(VsyncPaceSource::Timeout));
        assert!(!model.record_miss(VsyncPaceSource::Timeout));
        assert!(model.record_miss(VsyncPaceSource::Timeout));

        assert_eq!(model.fallback_frames(), 3);
        assert_eq!(model.max_miss_streak(), 3);
    }

    #[test]
    fn successful_hit_recovers_after_degraded_state() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 3);
        model.record_miss(VsyncPaceSource::Timeout);
        model.record_miss(VsyncPaceSource::Timeout);
        model.record_miss(VsyncPaceSource::Timeout);
        assert_eq!(model.miss_streak(), 3);

        model.record_hit_at_us(50_000);

        assert_eq!(model.miss_streak(), 0);
        assert_eq!(model.hits(), 1);
        assert_eq!(model.max_miss_streak(), 3);
    }

    #[test]
    fn source_labels_match_runtime_status_values() {
        assert_eq!(VsyncPaceSource::Vsync.label(), "vsync");
        assert_eq!(VsyncPaceSource::Fallback.label(), "fallback");
        assert_eq!(VsyncPaceSource::Timeout.label(), "timeout");
        assert_eq!(VsyncPaceSource::Error.label(), "error");
    }

    #[test]
    fn error_misses_count_as_fallback_frames() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 2);

        assert!(!model.record_miss(VsyncPaceSource::Error));
        assert!(model.record_miss(VsyncPaceSource::Fallback));

        assert_eq!(model.fallback_frames(), 2);
        assert_eq!(model.miss_streak(), 2);
        assert_eq!(model.max_miss_streak(), 2);
    }
}
