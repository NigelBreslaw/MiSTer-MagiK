// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_fb::framebuffer::vsync::{VsyncPace, VsyncPaceSource};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum FrameProductionClass {
    #[default]
    EventDriven,
    SynchronousAnimation,
    Prepared,
}

impl FrameProductionClass {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::EventDriven => "event-driven",
            Self::SynchronousAnimation => "synchronous-animation",
            Self::Prepared => "prepared",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct FrameProductionTrace {
    pub(super) class: FrameProductionClass,
    pub(super) sequence: u64,
    pub(super) ready_depth: usize,
    pub(super) ready_age_us: u64,
    pub(super) render_wall_us: u64,
    pub(super) starvation_count: u64,
    pub(super) cancelled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LauncherFramePacingInput {
    pub(super) first_visible_copy_done: bool,
    pub(super) frame_start_phase_us: u64,
    pub(super) period_us: u64,
    pub(super) late_frame_start_headroom_us: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LauncherFramePacingDecision {
    pub(super) wait_before_render: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LauncherPacingTrace {
    pub(super) vsync_source: Option<VsyncPaceSource>,
    pub(super) vsync_period_us: u64,
    pub(super) vsync_miss_streak: u32,
    pub(super) vsync_stale_hits: u32,
    pub(super) vsync_wait_start_age_us: u64,
    pub(super) vsync_accepted_hit_age_us: u64,
    pub(super) frame_start_phase_us: u64,
    pub(super) present_phase_us: u128,
}

pub(super) const FB0_LATE_FRAME_START_HEADROOM_US: u64 = 6_000;
pub(super) const FPGA_LATCH_LATE_FRAME_START_HEADROOM_US: u64 = 12_000;
const LATCH_PHASE_MIN_HEADROOM_US: u64 = 8_000;
const LATCH_PHASE_MAX_HEADROOM_US: u64 = 14_000;
const LATCH_PHASE_SAFETY_US: u64 = 750;

#[derive(Clone, Copy, Debug)]
pub(super) struct LauncherPhaseAlignment {
    estimated_work_us: u64,
}

impl Default for LauncherPhaseAlignment {
    fn default() -> Self {
        Self {
            estimated_work_us: FPGA_LATCH_LATE_FRAME_START_HEADROOM_US
                .saturating_sub(LATCH_PHASE_SAFETY_US),
        }
    }
}

impl LauncherPhaseAlignment {
    pub(super) fn observe(&mut self, work_us: u64) {
        let bounded = work_us.clamp(
            LATCH_PHASE_MIN_HEADROOM_US.saturating_sub(LATCH_PHASE_SAFETY_US),
            LATCH_PHASE_MAX_HEADROOM_US.saturating_sub(LATCH_PHASE_SAFETY_US),
        );
        self.estimated_work_us = if bounded >= self.estimated_work_us {
            self.estimated_work_us
                .saturating_add((bounded - self.estimated_work_us).div_ceil(8))
        } else {
            self.estimated_work_us
                .saturating_sub((self.estimated_work_us - bounded).div_ceil(8))
        };
    }

    #[must_use]
    pub(super) fn required_headroom_us(self) -> u64 {
        self.estimated_work_us
            .saturating_add(LATCH_PHASE_SAFETY_US)
            .clamp(LATCH_PHASE_MIN_HEADROOM_US, LATCH_PHASE_MAX_HEADROOM_US)
    }
}

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
                    input.late_frame_start_headroom_us,
                ),
        }
    }

    #[inline]
    fn should_wait_before_late_frame_render(
        self,
        frame_start_phase_us: u64,
        period_us: u64,
        late_frame_start_headroom_us: u64,
    ) -> bool {
        if period_us <= late_frame_start_headroom_us {
            return false;
        }
        frame_start_phase_us >= period_us - late_frame_start_headroom_us
    }
}

impl LauncherPacingTrace {
    #[inline]
    pub(super) fn from_pace(
        pace: Option<&VsyncPace>,
        frame_start_phase_us: u64,
        fallback_period_us: u64,
        present_at: Instant,
    ) -> Self {
        Self {
            vsync_source: pace.map(|pace| pace.source),
            vsync_period_us: pace
                .map(|pace| pace.period_us)
                .unwrap_or(fallback_period_us),
            vsync_miss_streak: pace.map(|pace| pace.miss_streak).unwrap_or(0),
            vsync_stale_hits: pace.map(|pace| pace.stale_hits).unwrap_or(0),
            vsync_wait_start_age_us: pace.map(|pace| pace.wait_start_age_us).unwrap_or(0),
            vsync_accepted_hit_age_us: pace.map(|pace| pace.accepted_hit_age_us).unwrap_or(0),
            frame_start_phase_us,
            present_phase_us: pace
                .and_then(|pace| pace.hit_at)
                .map(|hit_at| present_at.saturating_duration_since(hit_at).as_micros())
                .unwrap_or(0),
        }
    }

    #[inline]
    pub(super) fn from_pace_with_present_phase(
        pace: Option<&VsyncPace>,
        frame_start_phase_us: u64,
        fallback_period_us: u64,
        present_phase_us: u128,
    ) -> Self {
        let mut trace = Self::from_pace(
            pace,
            frame_start_phase_us,
            fallback_period_us,
            Instant::now(),
        );
        trace.present_phase_us = present_phase_us;
        trace
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn should_wait(
        first_visible_copy_done: bool,
        frame_start_phase_us: u64,
        period_us: u64,
        late_frame_start_headroom_us: u64,
    ) -> bool {
        LauncherFramePacingPolicy::default()
            .decide(LauncherFramePacingInput {
                first_visible_copy_done,
                frame_start_phase_us,
                period_us,
                late_frame_start_headroom_us,
            })
            .wait_before_render
    }

    #[test]
    fn first_visible_copy_must_be_done_before_waiting() {
        assert!(!should_wait(false, 10_667, 16_667, 6_000));
    }

    #[test]
    fn exact_threshold_waits_before_render() {
        assert!(should_wait(true, 10_667, 16_667, 6_000));
        assert!(!should_wait(true, 10_666, 16_667, 6_000));
    }

    #[test]
    fn short_period_never_waits_before_render() {
        assert!(!should_wait(true, 6_000, 6_000, 6_000));
        assert!(!should_wait(true, 6_001, 6_000, 6_000));
    }

    #[test]
    fn normal_60hz_period_waits_in_final_headroom_window() {
        assert!(should_wait(true, 31_000, 16_667, 6_000));
        assert!(should_wait(true, 15_000, 16_667, 6_000));
        assert!(!should_wait(true, 10_000, 16_667, 6_000));
    }

    #[test]
    fn pal_50hz_period_waits_in_final_headroom_window() {
        assert!(should_wait(true, 14_000, 20_000, 6_000));
        assert!(!should_wait(true, 13_999, 20_000, 6_000));
        assert!(!should_wait(true, 5_000, 20_000, 6_000));
    }

    #[test]
    fn latch_headroom_waits_before_render_earlier_than_fb0() {
        assert!(should_wait(
            true,
            5_000,
            16_667,
            FPGA_LATCH_LATE_FRAME_START_HEADROOM_US
        ));
        assert!(!should_wait(
            true,
            5_000,
            16_667,
            FB0_LATE_FRAME_START_HEADROOM_US
        ));
    }

    #[test]
    fn latch_phase_alignment_tracks_work_with_bounded_headroom() {
        let mut alignment = LauncherPhaseAlignment::default();
        assert_eq!(
            alignment.required_headroom_us(),
            FPGA_LATCH_LATE_FRAME_START_HEADROOM_US
        );
        for _ in 0..32 {
            alignment.observe(9_000);
        }
        assert!(alignment.required_headroom_us() < FPGA_LATCH_LATE_FRAME_START_HEADROOM_US);
        assert!(alignment.required_headroom_us() >= LATCH_PHASE_MIN_HEADROOM_US);
        for _ in 0..128 {
            alignment.observe(30_000);
        }
        assert_eq!(
            alignment.required_headroom_us(),
            LATCH_PHASE_MAX_HEADROOM_US
        );
    }

    fn test_pace(source: VsyncPaceSource, hit_at: Option<Instant>) -> VsyncPace {
        VsyncPace {
            source,
            wait_us: 12_000,
            period_us: 16_700,
            miss_streak: 2,
            hit_at,
            wait_start_age_us: 3_000,
            accepted_hit_age_us: 400,
            stale_hits: 1,
            message: None,
        }
    }

    #[test]
    fn no_pace_uses_default_values() {
        let present_at = Instant::now();

        let trace = LauncherPacingTrace::from_pace(None, 7_000, 20_000, present_at);

        assert_eq!(
            trace,
            LauncherPacingTrace {
                vsync_source: None,
                vsync_period_us: 20_000,
                vsync_miss_streak: 0,
                vsync_stale_hits: 0,
                vsync_wait_start_age_us: 0,
                vsync_accepted_hit_age_us: 0,
                frame_start_phase_us: 7_000,
                present_phase_us: 0,
            }
        );
    }

    #[test]
    fn direct_vsync_pace_values_are_preserved() {
        let present_at = Instant::now();
        let hit_at = present_at - std::time::Duration::from_micros(900);
        let pace = test_pace(VsyncPaceSource::Vsync, Some(hit_at));

        let trace = LauncherPacingTrace::from_pace(Some(&pace), 8_000, 20_000, present_at);

        assert_eq!(trace.vsync_source, Some(VsyncPaceSource::Vsync));
        assert_eq!(trace.vsync_period_us, 16_700);
        assert_eq!(trace.vsync_miss_streak, 2);
        assert_eq!(trace.vsync_stale_hits, 1);
        assert_eq!(trace.vsync_wait_start_age_us, 3_000);
        assert_eq!(trace.vsync_accepted_hit_age_us, 400);
        assert_eq!(trace.frame_start_phase_us, 8_000);
    }

    #[test]
    fn present_phase_uses_present_time_since_hit() {
        let present_at = Instant::now();
        let hit_at = present_at - std::time::Duration::from_micros(1_234);
        let pace = test_pace(VsyncPaceSource::Vsync, Some(hit_at));

        let trace = LauncherPacingTrace::from_pace(Some(&pace), 0, 16_667, present_at);

        assert_eq!(trace.present_phase_us, 1_234);
    }

    #[test]
    fn fallback_period_behavior_matches_existing_trace_defaults() {
        let present_at = Instant::now();
        let pace = test_pace(VsyncPaceSource::Fallback, None);

        let trace = LauncherPacingTrace::from_pace(Some(&pace), 9_000, 20_000, present_at);

        assert_eq!(trace.vsync_source, Some(VsyncPaceSource::Fallback));
        assert_eq!(trace.vsync_period_us, 16_700);
        assert_eq!(trace.present_phase_us, 0);
    }
}
