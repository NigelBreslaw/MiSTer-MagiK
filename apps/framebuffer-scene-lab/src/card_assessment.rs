// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded card-flip frame evidence and physical-refresh accounting.

use serde::Serialize;

pub const CARD_FLIP_EVIDENCE_SCHEMA: &str = "mister-magik-card-flip-frame-v1";
pub const CARD_FLIP_CADENCE_SCHEMA: &str = "mister-magik-card-flip-cadence-v1";

#[derive(Clone, Debug, Serialize)]
pub struct CardFlipFrameEvidence {
    pub schema: &'static str,
    pub frame: u64,
    pub profiler_enabled: bool,
    pub progress_q16: u16,
    pub face: &'static str,
    pub direction: &'static str,
    pub render_wall_us: u64,
    pub render_cpu_us: u64,
    pub transfer_wall_us: u64,
    pub transfer_cpu_us: u64,
    pub post_wall_us: u64,
    pub post_cpu_us: u64,
    pub settle_wall_us: u64,
    pub settle_cpu_us: u64,
    pub post_to_confirm_wall_us: u64,
    pub frame_to_confirm_wall_us: u64,
    pub process_cpu_us: u64,
    pub completion_monotonic_us: u64,
    pub completion_interval_us: u64,
    pub slot_index: u8,
    pub sequence: u16,
    pub sequence_delta: u16,
    pub flip_count: u16,
    pub flip_delta: u16,
    pub post_count: u16,
    pub post_delta: u16,
    pub drop_count: u16,
    pub drop_delta: u16,
    pub status_reads: u64,
    pub poll_reads: u64,
    pub source_rect_count: u32,
    pub destination_rect_count: u32,
    pub source_bytes: usize,
    pub destination_bytes: usize,
    pub full_restore: bool,
}

impl CardFlipFrameEvidence {
    #[must_use]
    pub const fn new(frame: u64, profiler_enabled: bool) -> Self {
        Self {
            schema: CARD_FLIP_EVIDENCE_SCHEMA,
            frame,
            profiler_enabled,
            progress_q16: 0,
            face: "front",
            direction: "forward",
            render_wall_us: 0,
            render_cpu_us: 0,
            transfer_wall_us: 0,
            transfer_cpu_us: 0,
            post_wall_us: 0,
            post_cpu_us: 0,
            settle_wall_us: 0,
            settle_cpu_us: 0,
            post_to_confirm_wall_us: 0,
            frame_to_confirm_wall_us: 0,
            process_cpu_us: 0,
            completion_monotonic_us: 0,
            completion_interval_us: 0,
            slot_index: 0,
            sequence: 0,
            sequence_delta: 0,
            flip_count: 0,
            flip_delta: 0,
            post_count: 0,
            post_delta: 0,
            drop_count: 0,
            drop_delta: 0,
            status_reads: 0,
            poll_reads: 0,
            source_rect_count: 0,
            destination_rect_count: 0,
            source_bytes: 0,
            destination_bytes: 0,
            full_restore: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CardFlipCadenceSummary {
    pub schema: &'static str,
    pub profiler_enabled: bool,
    pub refresh_period_us: u64,
    pub refresh_period_source: &'static str,
    pub confirmed_frames: usize,
    pub elapsed_us: u64,
    pub expected_refresh_intervals: u64,
    pub unique_latch_flips: u64,
    pub repeated_refreshes: u64,
    pub sequence_failures: u64,
    pub latch_drop_delta: u64,
    pub completion_failures: u64,
    pub long_completion_intervals: u64,
    pub max_completion_interval_us: u64,
    pub unique_fps: f64,
    pub cadence_authoritative: bool,
}

pub fn summarize_cadence(
    frames: &[CardFlipFrameEvidence],
    refresh_period_us: u64,
    refresh_period_source: &'static str,
) -> Result<CardFlipCadenceSummary, String> {
    if refresh_period_us == 0 {
        return Err("card cadence refresh period must be non-zero".into());
    }
    if frames.len() < 2 {
        return Err("card cadence requires at least two confirmed frames".into());
    }
    let first = frames
        .first()
        .expect("frame count checked before first access");
    let last = frames
        .last()
        .expect("frame count checked before last access");
    let profiler_enabled = first.profiler_enabled;
    if frames
        .iter()
        .any(|frame| frame.profiler_enabled != profiler_enabled)
    {
        return Err("card cadence cannot mix sampled and unsampled frames".into());
    }
    let elapsed_us = last
        .completion_monotonic_us
        .checked_sub(first.completion_monotonic_us)
        .filter(|elapsed| *elapsed > 0)
        .ok_or("card cadence completion timestamps are not increasing")?;

    let mut expected_refresh_intervals = 0_u64;
    let mut unique_latch_flips = 0_u64;
    let mut repeated_refreshes = 0_u64;
    let mut sequence_failures = 0_u64;
    let mut latch_drop_delta = 0_u64;
    let mut completion_failures = 0_u64;
    let mut long_completion_intervals = 0_u64;
    let mut max_completion_interval_us = 0_u64;
    let long_limit_us = refresh_period_us.saturating_mul(3) / 2;

    for pair in frames.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        let interval_us = current
            .completion_monotonic_us
            .checked_sub(previous.completion_monotonic_us)
            .filter(|interval| *interval > 0)
            .ok_or_else(|| {
                format!(
                    "card cadence frame {} has a non-increasing completion timestamp",
                    current.frame
                )
            })?;
        let expected = interval_us
            .saturating_add(refresh_period_us / 2)
            .checked_div(refresh_period_us)
            .unwrap_or(1)
            .max(1);
        let flips = u64::from(current.flip_count.wrapping_sub(previous.flip_count));
        if flips > expected {
            completion_failures = completion_failures.saturating_add(1);
        } else {
            repeated_refreshes = repeated_refreshes.saturating_add(expected.saturating_sub(flips));
        }
        expected_refresh_intervals = expected_refresh_intervals.saturating_add(expected);
        unique_latch_flips = unique_latch_flips.saturating_add(flips);
        sequence_failures = sequence_failures.saturating_add(u64::from(
            current.sequence != next_sequence(previous.sequence),
        ));
        latch_drop_delta = latch_drop_delta.saturating_add(u64::from(
            current.drop_count.wrapping_sub(previous.drop_count),
        ));
        long_completion_intervals =
            long_completion_intervals.saturating_add(u64::from(interval_us > long_limit_us));
        max_completion_interval_us = max_completion_interval_us.max(interval_us);
    }

    Ok(CardFlipCadenceSummary {
        schema: CARD_FLIP_CADENCE_SCHEMA,
        profiler_enabled,
        refresh_period_us,
        refresh_period_source,
        confirmed_frames: frames.len(),
        elapsed_us,
        expected_refresh_intervals,
        unique_latch_flips,
        repeated_refreshes,
        sequence_failures,
        latch_drop_delta,
        completion_failures,
        long_completion_intervals,
        max_completion_interval_us,
        unique_fps: unique_latch_flips as f64 * 1_000_000.0 / elapsed_us as f64,
        cadence_authoritative: !profiler_enabled,
    })
}

const fn next_sequence(sequence: u16) -> u16 {
    let next = sequence.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: u64, completion_us: u64, sequence: u16, flips: u16) -> CardFlipFrameEvidence {
        let mut frame = CardFlipFrameEvidence::new(id, false);
        frame.completion_monotonic_us = completion_us;
        frame.sequence = sequence;
        frame.flip_count = flips;
        frame
    }

    #[test]
    fn exact_fifty_and_sixty_hertz_have_no_repeats() {
        for period_us in [16_667, 20_000] {
            let frames = (0..10)
                .map(|index| {
                    frame(
                        index,
                        1_000_000 + index * period_us,
                        index as u16 + 1,
                        index as u16,
                    )
                })
                .collect::<Vec<_>>();
            let summary = summarize_cadence(&frames, period_us, "test").unwrap();
            assert_eq!(summary.repeated_refreshes, 0);
            assert_eq!(summary.unique_latch_flips, 9);
        }
    }

    #[test]
    fn detects_repeated_refresh_without_latch_drop() {
        let frames = [
            frame(1, 1_000_000, 1, 10),
            frame(2, 1_016_667, 2, 11),
            frame(3, 1_050_001, 3, 12),
        ];
        let summary = summarize_cadence(&frames, 16_667, "test").unwrap();
        assert_eq!(summary.repeated_refreshes, 1);
        assert_eq!(summary.latch_drop_delta, 0);
        assert_eq!(summary.long_completion_intervals, 1);
    }

    #[test]
    fn accepts_wrapped_counters_and_two_flips_across_two_intervals() {
        let frames = [
            frame(1, 1_000_000, u16::MAX, u16::MAX),
            frame(2, 1_033_334, 1, 1),
        ];
        let summary = summarize_cadence(&frames, 16_667, "test").unwrap();
        assert_eq!(summary.expected_refresh_intervals, 2);
        assert_eq!(summary.unique_latch_flips, 2);
        assert_eq!(summary.repeated_refreshes, 0);
        assert_eq!(summary.sequence_failures, 0);
    }

    #[test]
    fn sampled_frames_are_never_authoritative() {
        let mut frames = [frame(1, 1_000_000, 1, 1), frame(2, 1_016_667, 2, 2)];
        for frame in &mut frames {
            frame.profiler_enabled = true;
        }
        assert!(
            !summarize_cadence(&frames, 16_667, "test")
                .unwrap()
                .cadence_authoritative
        );
    }

    #[test]
    fn rejects_missing_nonmonotonic_and_mixed_evidence() {
        assert!(summarize_cadence(&[frame(1, 1, 1, 1)], 16_667, "test").is_err());
        assert!(
            summarize_cadence(&[frame(1, 2, 1, 1), frame(2, 1, 2, 2)], 16_667, "test").is_err()
        );
        let mut mixed = [frame(1, 1, 1, 1), frame(2, 2, 2, 2)];
        mixed[1].profiler_enabled = true;
        assert!(summarize_cadence(&mixed, 16_667, "test").is_err());
    }
}
