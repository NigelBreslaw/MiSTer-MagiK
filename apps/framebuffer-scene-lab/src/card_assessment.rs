// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Scene-neutral bounded frame evidence and physical-refresh accounting.

use serde::Serialize;

pub const FRAME_EVIDENCE_SCHEMA: &str = "mister-magik-scene-lab-frame-v2";
pub const CADENCE_SCHEMA: &str = "mister-magik-scene-lab-cadence-v3";
pub const PRESENTATION_TELEMETRY_SCHEMA: &str = "mister-magik-scene-lab-presentation-telemetry-v1";
pub const VSYNC_EVENT_SCHEMA: &str = "mister-magik-scene-lab-vsync-event-v1";
pub const VSYNC_SUMMARY_SCHEMA: &str = "mister-magik-scene-lab-vsync-summary-v1";

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct VsyncEvent {
    pub schema: &'static str,
    pub ordinal: u64,
    pub status: &'static str,
    pub monotonic_us: u64,
    pub wait_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct VsyncObserverSummary {
    pub schema: &'static str,
    pub events: usize,
    pub hits: usize,
    pub timeouts: usize,
    pub errors: usize,
    pub observed_intervals: usize,
    pub elapsed_us: u64,
    pub interval_min_us: u64,
    pub interval_average_us: u64,
    pub interval_p50_us: u64,
    pub interval_p99_us: u64,
    pub interval_max_us: u64,
    pub observed_hz: f64,
}

#[must_use]
pub fn summarize_vsync_observer(events: &[VsyncEvent]) -> VsyncObserverSummary {
    let hits = events
        .iter()
        .filter(|event| event.status == "hit")
        .collect::<Vec<_>>();
    let mut intervals = hits
        .windows(2)
        .filter_map(|pair| {
            pair[1]
                .monotonic_us
                .checked_sub(pair[0].monotonic_us)
                .filter(|interval| *interval > 0)
        })
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let elapsed_us = hits
        .first()
        .zip(hits.last())
        .and_then(|(first, last)| last.monotonic_us.checked_sub(first.monotonic_us))
        .unwrap_or(0);
    let interval_average_us = intervals
        .iter()
        .copied()
        .sum::<u64>()
        .checked_div(intervals.len() as u64)
        .unwrap_or(0);
    let percentile = |percent: usize| {
        intervals
            .get(
                intervals
                    .len()
                    .saturating_mul(percent)
                    .div_ceil(100)
                    .saturating_sub(1),
            )
            .copied()
            .unwrap_or(0)
    };
    VsyncObserverSummary {
        schema: VSYNC_SUMMARY_SCHEMA,
        events: events.len(),
        hits: hits.len(),
        timeouts: events
            .iter()
            .filter(|event| event.status == "timeout")
            .count(),
        errors: events
            .iter()
            .filter(|event| event.status == "error")
            .count(),
        observed_intervals: intervals.len(),
        elapsed_us,
        interval_min_us: intervals.first().copied().unwrap_or(0),
        interval_average_us,
        interval_p50_us: percentile(50),
        interval_p99_us: percentile(99),
        interval_max_us: intervals.last().copied().unwrap_or(0),
        observed_hz: if elapsed_us == 0 {
            0.0
        } else {
            intervals.len() as f64 * 1_000_000.0 / elapsed_us as f64
        },
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FrameEvidence {
    pub schema: &'static str,
    pub scene: &'static str,
    pub frame: u64,
    pub profiler_enabled: bool,
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
    pub latch_drop_count: u16,
    pub latch_drop_delta: u16,
    pub status_reads: u64,
    pub poll_reads: u64,
    pub source_rect_count: u32,
    pub destination_rect_count: u32,
    pub source_bytes: usize,
    pub destination_bytes: usize,
    pub full_restore: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<CardFrameDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<ScreenshotFrameDetails>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CardFrameDetails {
    pub progress_q16: u16,
    pub face: &'static str,
    pub direction: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScreenshotFrameDetails {
    pub raster_held_cards: usize,
    pub raster_moved_cards: usize,
    pub raster_hold_layer_mask: u8,
    pub raster_visible_layer_mask: u8,
    pub sixteenth_phase_layer_mask: u8,
    pub phase_bank_resident_bytes: usize,
    pub scale_count: u64,
    pub scale_total_us: u64,
    pub scale_max_us: u64,
    pub phase_count: u64,
    pub phase_total_us: u64,
    pub phase_max_us: u64,
    pub preparation_queue_depth: usize,
}

impl FrameEvidence {
    #[must_use]
    pub const fn new(scene: &'static str, frame: u64, profiler_enabled: bool) -> Self {
        Self {
            schema: FRAME_EVIDENCE_SCHEMA,
            scene,
            frame,
            profiler_enabled,
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
            latch_drop_count: 0,
            latch_drop_delta: 0,
            status_reads: 0,
            poll_reads: 0,
            source_rect_count: 0,
            destination_rect_count: 0,
            source_bytes: 0,
            destination_bytes: 0,
            full_restore: false,
            visible_count: None,
            card: None,
            screenshot: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CadenceSummary {
    pub schema: &'static str,
    pub profiler_enabled: bool,
    pub refresh_period_us: u64,
    pub refresh_period_source: &'static str,
    pub confirmed_frames: usize,
    pub elapsed_us: u64,
    pub expected_refresh_intervals: u64,
    pub unique_latch_flips: u64,
    pub dropped_frames: u64,
    pub software_estimated_dropped_frames: u64,
    pub cadence_source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation_telemetry: Option<PresentationTelemetrySummary>,
    pub confirmation_sequence_failures: u64,
    pub latch_drop_delta: u64,
    pub completion_failures: u64,
    pub long_completion_intervals: u64,
    pub max_completion_interval_us: u64,
    pub unique_fps: f64,
    pub cadence_authoritative: bool,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct PresentationTelemetrySnapshot {
    pub owned_vblank_count: u32,
    pub presented_vblank_count: u32,
    pub repeated_vblank_count: u32,
    pub ownership_loss_count: u32,
    pub active_sequence: u16,
    pub flags: u16,
}

impl PresentationTelemetrySnapshot {
    pub const fn magik_ownership(self) -> bool {
        self.flags & (1 << 3) != 0
    }

    pub const fn pending(self) -> bool {
        self.flags & (1 << 2) != 0
    }
}

impl mister_magik_latch_contract::PresentationTelemetryCounters for PresentationTelemetrySnapshot {
    fn owned_vblank_count(self) -> u32 {
        self.owned_vblank_count
    }

    fn presented_vblank_count(self) -> u32 {
        self.presented_vblank_count
    }

    fn repeated_vblank_count(self) -> u32 {
        self.repeated_vblank_count
    }

    fn ownership_loss_count(self) -> u32 {
        self.ownership_loss_count
    }

    fn magik_ownership(self) -> bool {
        self.magik_ownership()
    }

    fn pending(self) -> bool {
        self.pending()
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PresentationTelemetrySummary {
    pub schema: &'static str,
    pub start: PresentationTelemetrySnapshot,
    pub end: PresentationTelemetrySnapshot,
    pub elapsed_us: u64,
    pub owned_vblank_delta: u32,
    pub presented_vblank_delta: u32,
    pub repeated_vblank_delta: u32,
    pub ownership_loss_delta: u32,
    pub maximum_plausible_vblanks: u64,
    pub lifetime_invariant_valid: bool,
    pub delta_invariant_valid: bool,
    pub plausible: bool,
    pub endpoints_owned_and_settled: bool,
}

pub fn summarize_presentation_telemetry(
    start: PresentationTelemetrySnapshot,
    end: PresentationTelemetrySnapshot,
    elapsed_us: u64,
    refresh_period_us: u64,
) -> Result<PresentationTelemetrySummary, String> {
    let delta = mister_magik_latch_contract::validate_presentation_telemetry_window(
        start,
        end,
        elapsed_us,
        refresh_period_us,
    )
    .map_err(|error| error.to_string())?;
    Ok(PresentationTelemetrySummary {
        schema: PRESENTATION_TELEMETRY_SCHEMA,
        start,
        end,
        elapsed_us: delta.elapsed_us,
        owned_vblank_delta: delta.owned_vblank_delta,
        presented_vblank_delta: delta.presented_vblank_delta,
        repeated_vblank_delta: delta.repeated_vblank_delta,
        ownership_loss_delta: delta.ownership_loss_delta,
        maximum_plausible_vblanks: delta.maximum_plausible_vblanks,
        lifetime_invariant_valid: true,
        delta_invariant_valid: true,
        plausible: true,
        endpoints_owned_and_settled: true,
    })
}

pub fn apply_authoritative_presentation_telemetry(
    mut cadence: CadenceSummary,
    telemetry: PresentationTelemetrySummary,
) -> CadenceSummary {
    cadence.software_estimated_dropped_frames = cadence.dropped_frames;
    cadence.expected_refresh_intervals = u64::from(telemetry.owned_vblank_delta);
    cadence.unique_latch_flips = u64::from(telemetry.presented_vblank_delta);
    cadence.dropped_frames = u64::from(telemetry.repeated_vblank_delta);
    cadence.unique_fps =
        cadence.unique_latch_flips as f64 * 1_000_000.0 / telemetry.elapsed_us as f64;
    cadence.cadence_source = "fpga-owned-vblank-telemetry";
    cadence.cadence_authoritative = !cadence.profiler_enabled;
    cadence.presentation_telemetry = Some(telemetry);
    cadence
}

pub fn summarize_cadence(
    frames: &[FrameEvidence],
    refresh_period_us: u64,
    refresh_period_source: &'static str,
) -> Result<CadenceSummary, String> {
    if refresh_period_us == 0 {
        return Err("scene cadence refresh period must be non-zero".into());
    }
    if frames.len() < 2 {
        return Err("scene cadence requires at least two confirmed frames".into());
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
        return Err("scene cadence cannot mix sampled and unsampled frames".into());
    }
    let elapsed_us = last
        .completion_monotonic_us
        .checked_sub(first.completion_monotonic_us)
        .filter(|elapsed| *elapsed > 0)
        .ok_or("scene cadence completion timestamps are not increasing")?;

    let mut expected_refresh_intervals = 0_u64;
    let mut unique_latch_flips = 0_u64;
    let mut dropped_frames = 0_u64;
    let mut confirmation_sequence_failures = 0_u64;
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
                    "scene cadence frame {} has a non-increasing completion timestamp",
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
            dropped_frames = dropped_frames.saturating_add(expected.saturating_sub(flips));
        }
        expected_refresh_intervals = expected_refresh_intervals.saturating_add(expected);
        unique_latch_flips = unique_latch_flips.saturating_add(flips);
        confirmation_sequence_failures = confirmation_sequence_failures.saturating_add(u64::from(
            !confirmation_sequence_is_contiguous(previous.sequence, current.sequence),
        ));
        latch_drop_delta = latch_drop_delta.saturating_add(u64::from(
            current
                .latch_drop_count
                .wrapping_sub(previous.latch_drop_count),
        ));
        long_completion_intervals =
            long_completion_intervals.saturating_add(u64::from(interval_us > long_limit_us));
        max_completion_interval_us = max_completion_interval_us.max(interval_us);
    }

    Ok(CadenceSummary {
        schema: CADENCE_SCHEMA,
        profiler_enabled,
        refresh_period_us,
        refresh_period_source,
        confirmed_frames: frames.len(),
        elapsed_us,
        expected_refresh_intervals,
        unique_latch_flips,
        dropped_frames,
        software_estimated_dropped_frames: dropped_frames,
        cadence_source: "software-completion-estimator",
        presentation_telemetry: None,
        confirmation_sequence_failures,
        latch_drop_delta,
        completion_failures,
        long_completion_intervals,
        max_completion_interval_us,
        unique_fps: unique_latch_flips as f64 * 1_000_000.0 / elapsed_us as f64,
        cadence_authoritative: false,
    })
}

#[must_use]
pub const fn confirmation_sequence_is_contiguous(previous: u16, current: u16) -> bool {
    current == next_sequence(previous)
}

const fn next_sequence(sequence: u16) -> u16 {
    let next = sequence.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vsync(ordinal: u64, monotonic_us: u64) -> VsyncEvent {
        VsyncEvent {
            schema: VSYNC_EVENT_SCHEMA,
            ordinal,
            status: "hit",
            monotonic_us,
            wait_us: 0,
            message: None,
        }
    }

    #[test]
    fn vsync_summary_uses_raw_hit_intervals() {
        let mut events = vec![
            vsync(1, 100_000),
            vsync(2, 116_600),
            vsync(3, 133_300),
            vsync(4, 150_100),
        ];
        events.push(VsyncEvent {
            schema: VSYNC_EVENT_SCHEMA,
            ordinal: 5,
            status: "timeout",
            monotonic_us: 151_000,
            wait_us: 1_000,
            message: None,
        });
        let summary = summarize_vsync_observer(&events);
        assert_eq!(summary.events, 5);
        assert_eq!(summary.hits, 4);
        assert_eq!(summary.timeouts, 1);
        assert_eq!(summary.errors, 0);
        assert_eq!(summary.observed_intervals, 3);
        assert_eq!(summary.elapsed_us, 50_100);
        assert_eq!(summary.interval_min_us, 16_600);
        assert_eq!(summary.interval_p50_us, 16_700);
        assert_eq!(summary.interval_p99_us, 16_800);
        assert_eq!(summary.interval_max_us, 16_800);
    }

    fn frame(id: u64, completion_us: u64, sequence: u16, flips: u16) -> FrameEvidence {
        let mut frame = FrameEvidence::new("test", id, false);
        frame.completion_monotonic_us = completion_us;
        frame.sequence = sequence;
        frame.flip_count = flips;
        frame
    }

    #[test]
    fn exact_fifty_and_sixty_hertz_have_no_dropped_frames() {
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
            assert_eq!(summary.dropped_frames, 0);
            assert_eq!(summary.unique_latch_flips, 9);
        }
    }

    #[test]
    fn detects_dropped_frame_without_latch_drop() {
        let frames = [
            frame(1, 1_000_000, 1, 10),
            frame(2, 1_016_667, 2, 11),
            frame(3, 1_050_001, 3, 12),
        ];
        let summary = summarize_cadence(&frames, 16_667, "test").unwrap();
        assert_eq!(summary.dropped_frames, 1);
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
        assert_eq!(summary.dropped_frames, 0);
        assert_eq!(summary.confirmation_sequence_failures, 0);
    }

    #[test]
    fn confirmation_sequences_accept_wrap_and_reject_gaps() {
        assert!(confirmation_sequence_is_contiguous(u16::MAX, 1));
        assert!(!confirmation_sequence_is_contiguous(41, 41));
        assert!(!confirmation_sequence_is_contiguous(41, 43));
    }

    fn telemetry_snapshot(
        owned: u32,
        presented: u32,
        repeated: u32,
    ) -> PresentationTelemetrySnapshot {
        PresentationTelemetrySnapshot {
            owned_vblank_count: owned,
            presented_vblank_count: presented,
            repeated_vblank_count: repeated,
            ownership_loss_count: 7,
            active_sequence: 42,
            flags: 1 << 3,
        }
    }

    #[test]
    fn hardware_telemetry_replaces_the_rounded_estimator() {
        let frames = [
            frame(1, 1_000_000, 1, 10),
            frame(2, 1_016_667, 2, 11),
            frame(3, 1_033_334, 3, 12),
        ];
        let cadence = summarize_cadence(&frames, 16_667, "test").unwrap();
        assert_eq!(cadence.dropped_frames, 0);
        let telemetry = summarize_presentation_telemetry(
            telemetry_snapshot(u32::MAX - 1, u32::MAX - 1, 0),
            telemetry_snapshot(1, 0, 1),
            50_001,
            16_667,
        )
        .unwrap();
        let cadence = apply_authoritative_presentation_telemetry(cadence, telemetry);
        assert_eq!(cadence.software_estimated_dropped_frames, 0);
        assert_eq!(cadence.dropped_frames, 1);
        assert_eq!(cadence.expected_refresh_intervals, 3);
        assert_eq!(cadence.unique_latch_flips, 2);
        assert!(cadence.cadence_authoritative);
    }

    #[test]
    fn telemetry_rejects_pending_loss_invariant_and_implausible_deltas() {
        let valid = telemetry_snapshot(10, 9, 1);
        let mut pending = valid;
        pending.flags |= 1 << 2;
        assert!(summarize_presentation_telemetry(pending, valid, 16_667, 16_667).is_err());

        let mut loss = telemetry_snapshot(11, 10, 1);
        loss.ownership_loss_count = 8;
        assert!(summarize_presentation_telemetry(valid, loss, 16_667, 16_667).is_err());

        let invalid = telemetry_snapshot(11, 9, 1);
        assert!(summarize_presentation_telemetry(valid, invalid, 16_667, 16_667).is_err());

        let implausible = telemetry_snapshot(1_000, 999, 1);
        assert!(summarize_presentation_telemetry(valid, implausible, 16_667, 16_667).is_err());
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
