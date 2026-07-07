#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LauncherFramePacingInput {
    pub(super) first_visible_copy_done: bool,
    pub(super) frame_start_phase_us: u64,
    pub(super) period_us: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LauncherFramePacingDecision {
    pub(super) wait_before_render: bool,
}

const LATE_FRAME_START_HEADROOM_US: u64 = 6_000;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct LauncherFramePacingPolicy;

impl LauncherFramePacingPolicy {
    #[inline]
    pub(super) fn decide(self, input: LauncherFramePacingInput) -> LauncherFramePacingDecision {
        LauncherFramePacingDecision {
            wait_before_render: input.first_visible_copy_done
                && self.should_wait_before_late_frame_render(
                    input.frame_start_phase_us,
                    input.period_us,
                ),
        }
    }

    #[inline]
    fn should_wait_before_late_frame_render(
        self,
        frame_start_phase_us: u64,
        period_us: u64,
    ) -> bool {
        if period_us <= LATE_FRAME_START_HEADROOM_US {
            return false;
        }
        frame_start_phase_us >= period_us - LATE_FRAME_START_HEADROOM_US
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn should_wait(
        first_visible_copy_done: bool,
        frame_start_phase_us: u64,
        period_us: u64,
    ) -> bool {
        LauncherFramePacingPolicy::default()
            .decide(LauncherFramePacingInput {
                first_visible_copy_done,
                frame_start_phase_us,
                period_us,
            })
            .wait_before_render
    }

    #[test]
    fn first_visible_copy_must_be_done_before_waiting() {
        assert!(!should_wait(false, 10_667, 16_667));
    }

    #[test]
    fn exact_threshold_waits_before_render() {
        assert!(should_wait(true, 10_667, 16_667));
        assert!(!should_wait(true, 10_666, 16_667));
    }

    #[test]
    fn short_period_never_waits_before_render() {
        assert!(!should_wait(true, 6_000, 6_000));
        assert!(!should_wait(true, 6_001, 6_000));
    }

    #[test]
    fn normal_60hz_period_waits_in_final_headroom_window() {
        assert!(should_wait(true, 31_000, 16_667));
        assert!(should_wait(true, 15_000, 16_667));
        assert!(!should_wait(true, 10_000, 16_667));
    }

    #[test]
    fn pal_50hz_period_waits_in_final_headroom_window() {
        assert!(should_wait(true, 14_000, 20_000));
        assert!(!should_wait(true, 13_999, 20_000));
        assert!(!should_wait(true, 5_000, 20_000));
    }
}
