//! Opt-in, bounded observation neighborhoods. No allocation or JSON per frame.
use crate::measurement::DroppedFrameRecord;
use serde_json::{Value, json};
use std::{collections::VecDeque, time::Instant};

const PREDECESSORS: usize = 3;
const SUCCESSORS: usize = 2;
// Diagnostic-only reservation: a dense 45-second 60 Hz window already has
// 2,700 produced frames, before superseded attempts. Keep a bounded margin
// so a consistently slow route retains evidence rather than only its start.
const CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EvidenceMode {
    #[default]
    Off,
    Neighbors,
    Phases,
}
impl EvidenceMode {
    pub fn from_request(value: &Value) -> Result<Self, String> {
        match value.as_str() {
            None if value.is_null() => Ok(Self::Off),
            Some("off") => Ok(Self::Off),
            Some("neighbors") => Ok(Self::Neighbors),
            Some("phases") => Ok(Self::Phases),
            _ => Err("frame_evidence must be off, neighbors or phases".into()),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Neighbors => "neighbors",
            Self::Phases => "phases",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HelperEvidence {
    pub renderer_id: u64,
    pub request_generation: u64,
    pub request_timestamp_us: u64,
    pub dispatched_us: u64,
    pub started_us: u64,
    pub finished_us: u64,
    pub received_us: u64,
    pub discarded_generation: Option<u64>,
}
impl HelperEvidence {
    fn json(self) -> Value {
        json!({"renderer_id":self.renderer_id,"request_generation":self.request_generation,
            "request_timestamp_us":self.request_timestamp_us,"dispatched_us":self.dispatched_us,
            "started_us":self.started_us,"finished_us":self.finished_us,"received_us":self.received_us,
            "discarded_generation":self.discarded_generation})
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameEvidence {
    pub phases_enabled: bool,
    pub cpu_us: [Option<u64>; 7],
    pub cpu_brackets_us: [[u64; 2]; 7],
    pub observer_sampling_us: u64,
    pub helper: Option<HelperEvidence>,
    pub input_sequence: Option<u64>,
    pub input_captured_monotonic_us: Option<u64>,
    pub input_dequeued_us: Option<u64>,
    /// Process-relative boundaries: callbacks/tooling, timers, lifecycle, device,
    /// readiness, catalog, media, launch, navigation, qualification, benchmark,
    /// housekeeping. Populated only in phase captures.
    pub pre_input_boundaries_us: [u64; 12],
    pub input_epoch: u64,
    pub bridge_us: u64,
    pub direct_hidden_copy_us: u64,
    pub direct_hidden_copy_bytes: u64,
    pub bridge_model_us: Option<u64>,
    pub bridge_stages_us: Option<[u64; 6]>,
    pub bridge_presenter_us: Option<[u64; 4]>,
    pub bridge_hub_counts_us: Option<[u64; 2]>,
    pub bridge_counters_enabled: bool,
    pub bridge_allocation_us: u64,
    pub bridge_models_replaced: u64,
    pub destination_reveal_us: u64,
    pub destination_stage_us: Option<[u64; 4]>,
    /// Nested list costs: row preparation, then composition into the destination.
    pub destination_list_us: Option<[u64; 2]>,
    /// Native Home raster followed by Slint overlay composition, both nested in render.
    pub home_composition_us: [u64; 2],
    pub card_snapshot_locked: bool,
    pub producer_ready_depth: usize,
    pub producer_ready_age_us: u64,
    pub producer_cancelled: bool,
    pub attempt_id: u64,
    pub produced_frame_id: u64,
    pub previous_observation_attempt_id: Option<u64>,
    pub begin_us: u64,
    pub request_generation: u64,
    pub missing_fresh_pose: u64,
    pub ownership_loss_count: Option<u32>,
    pub raw_presented_count: Option<u32>,
    pub raw_repeat_count: Option<u32>,
    pub logical_time_us: u64,
    pub menu_token: u64,
    pub view: &'static str,
    pub selected: usize,
    pub pose_phase: &'static str,
    pub pose_progress: u64,
    pub content_generation: Option<u64>,
    pub input_generation: u64,
    pub motion: bool,
    pub motion_continues_after_present: Option<bool>,
    pub baseline_reset: bool,
    pub telemetry_valid: bool,
    pub telemetry_before_us: u64,
    pub previous_read_bracket_us: Option<[u64; 2]>,
    pub refresh_counter: Option<u32>,
    pub telemetry_flags: Option<u16>,
    pub slot: u8,
    pub copied_bytes: u64,
    pub full_seed: bool,
    pub outcome: &'static str,
    pub record: DroppedFrameRecord,
    pub finish_us: u64,
}
impl FrameEvidence {
    fn json(self) -> Value {
        let phases = self.phases_enabled.then(|| json!({"cpu_us":self.cpu_us,"cpu_brackets_us":self.cpu_brackets_us,
            "helper":self.helper.map(HelperEvidence::json),"input_sequence":self.input_sequence,
            "input_captured_monotonic_us":self.input_captured_monotonic_us,
            "input_dequeued_us":self.input_dequeued_us,
            "pre_input_boundaries_us":self.pre_input_boundaries_us,
            "direct_hidden_copy_us":self.direct_hidden_copy_us,"direct_hidden_copy_bytes":self.direct_hidden_copy_bytes,"bridge_us":self.bridge_us,"bridge_model_us":self.bridge_model_us,"bridge_stages_us":self.bridge_stages_us,
            "bridge_presenter_us":self.bridge_presenter_us,"bridge_hub_counts_us":self.bridge_hub_counts_us,
            "bridge_counters_enabled":self.bridge_counters_enabled,
            "bridge_allocation_us":self.bridge_allocation_us,"bridge_models_replaced":self.bridge_models_replaced,
            "destination_reveal_us":self.destination_reveal_us,"destination_stage_us":self.destination_stage_us,"destination_list_us":self.destination_list_us,"home_composition_us":self.home_composition_us,"card_snapshot_locked":self.card_snapshot_locked,
            "producer_ready_depth":self.producer_ready_depth,"producer_ready_age_us":self.producer_ready_age_us,
            "producer_cancelled":self.producer_cancelled}));
        json!({"phases":phases,"attempt_id":self.attempt_id,"produced_frame_id":self.produced_frame_id,
            "previous_observation_attempt_id":self.previous_observation_attempt_id,"begin_us":self.begin_us,
            "request_generation":self.request_generation,"missing_fresh_pose":self.missing_fresh_pose,"ownership_loss_count":self.ownership_loss_count,
            "raw_presented_count":self.raw_presented_count,"raw_repeat_count":self.raw_repeat_count,
            "logical_time_us":self.logical_time_us,
            "menu_token":self.menu_token,"view":self.view,"selected":self.selected,"pose_phase":self.pose_phase,
            "pose_progress":self.pose_progress,"content_generation":self.content_generation,
            "input_generation":self.input_generation,"input_epoch":self.input_epoch,"motion":self.motion,
            "motion_continues_after_present":self.motion_continues_after_present,
            "baseline_reset":self.baseline_reset,"telemetry_valid":self.telemetry_valid,
            "telemetry_before_us":self.telemetry_before_us,
            "previous_read_bracket_us":self.previous_read_bracket_us,
            "refresh_counter":self.refresh_counter,"telemetry_flags":self.telemetry_flags,
            "slot":self.slot,"copied_bytes":self.copied_bytes,"full_seed":self.full_seed,
            "outcome":self.outcome,"observation":self.record.json(),"finish_us":self.finish_us})
    }
}

#[derive(Default)]
pub struct FrameEvidenceCapture {
    pub mode: EvidenceMode,
    recent: VecDeque<FrameEvidence>,
    retained: Vec<FrameEvidence>,
    successor_frames: usize,
    overflow: u64,
    gaps: u64,
    observed: u64,
    observer_us: Vec<u64>,
    clocks: Vec<[u64; 3]>,
}
impl FrameEvidenceCapture {
    pub fn reset(&mut self, mode: EvidenceMode) {
        *self = Self::default();
        self.mode = mode;
        if mode != EvidenceMode::Off {
            self.recent.reserve(PREDECESSORS);
            self.retained.reserve(CAPACITY);
            self.observer_us.reserve(3601);
            self.clocks.reserve(16);
        }
    }
    pub fn needs_clock(&self, now_us: u64) -> bool {
        self.mode != EvidenceMode::Off
            && self.clocks.len() < 16
            && self
                .clocks
                .last()
                .is_none_or(|sample| now_us.saturating_sub(sample[2]) >= 5_000_000)
    }
    /// Before/after are app Instant microseconds; middle is Linux CLOCK_MONOTONIC.
    pub fn note_clock(&mut self, bracket: [u64; 3]) {
        self.clocks.push(bracket);
    }
    pub fn observe_timed(&mut self, frame: FrameEvidence, started: Instant) {
        if self.mode == EvidenceMode::Off {
            return;
        }
        self.observe(frame);
        if self.observer_us.len() < 3601 {
            self.observer_us
                .push(frame.observer_sampling_us + started.elapsed().as_micros() as u64);
        }
    }
    fn retain(&mut self, frame: FrameEvidence) {
        if self
            .retained
            .last()
            .is_some_and(|last| last.attempt_id >= frame.attempt_id)
        {
            return;
        }
        if self.retained.len() < CAPACITY {
            self.retained.push(frame);
        } else {
            self.overflow += 1;
        }
    }
    pub fn observe(&mut self, frame: FrameEvidence) {
        if self.mode == EvidenceMode::Off {
            return;
        }
        self.observed += 1;
        if let Some(previous) = self.recent.back() {
            self.gaps += frame.attempt_id.saturating_sub(previous.attempt_id + 1);
        }
        let edge = self.recent.back().is_none_or(|previous| {
            previous.motion != frame.motion
                || previous.input_generation != frame.input_generation
                || previous.menu_token != frame.menu_token
                || previous.view != frame.view
                || previous.input_epoch != frame.input_epoch
                || previous.card_snapshot_locked != frame.card_snapshot_locked
                || previous.producer_cancelled != frame.producer_cancelled
                || (previous.producer_ready_depth != 0) != (frame.producer_ready_depth != 0)
                || previous.outcome != frame.outcome
        });
        let trigger = edge
            || frame.missing_fresh_pose != 0
            || frame.record.dropped_frames != 0
            || frame.motion && !frame.telemetry_valid && frame.outcome != "idle";
        if trigger {
            for i in 0..self.recent.len() {
                self.retain(self.recent[i]);
            }
            self.successor_frames = SUCCESSORS;
            self.retain(frame);
        } else if self.successor_frames != 0 {
            self.retain(frame);
            self.successor_frames -= 1;
        } else if frame.motion && frame.attempt_id.is_multiple_of(32) {
            self.retain(frame);
        }
        if self.recent.len() == PREDECESSORS {
            self.recent.pop_front();
        }
        self.recent.push_back(frame);
    }
    pub fn json(&self) -> Value {
        let mut samples = self.observer_us.clone();
        samples.sort_unstable();
        json!({"schema":"frame-neighborhood-v1","mode":self.mode.label(),
            "predecessors":PREDECESSORS,"successors":SUCCESSORS,"capacity":CAPACITY,
            "observed_frames":self.observed,"retention_overflow":self.overflow,
            "unrecorded_loop_iterations":self.gaps,"clock_resolution_us":1,
            "clock_brackets":{"columns":["app_before_us","clock_monotonic_us","app_after_us"],"samples":self.clocks},
            "deadline":"unknown: FPGA acceptance/cutoff timestamps unavailable",
            "phase_age_scope":"legacy phase fields are host hit ages, not FPGA vblank phase",
            "bridge_stage_labels":["models-and-presenters","pad-and-clock","layout","loading-and-confirmation","preview","setup"],
            "bridge_presenter_labels":["navigation-and-hub","settings","menu-feedback","arcade"],
            "bridge_hub_count_labels":["recent","favourites"],
            "bridge_hub_count_scope":"nested within navigation-and-hub; not additive to presenter stages",
            "bridge_counter_scope":"instrumented presenter operations only; excludes catalog and general allocator work",
            "destination_stage_labels":["preview","list","home","snapshot"],
            "destination_stage_scope":"wall time nested within custom drawing; capture-only work",
            "cpu_phase_points":["loop-entry","render-start","render-end","custom-end","post-return","active-observed","finish"],
            "observer_scope":"CPU sampler brackets and record selection; excludes clock-only reads and metadata construction",
            "observer_us":{"samples":samples.len(),"total":samples.iter().sum::<u64>(),
                "p99":samples.get(samples.len().saturating_sub(1)*99/100),"max":samples.last()},
            "frames":self.retained.iter().copied().map(FrameEvidence::json).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(id: u64, drops: u64) -> FrameEvidence {
        FrameEvidence {
            attempt_id: id,
            telemetry_valid: true,
            record: DroppedFrameRecord {
                dropped_frames: drops,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    #[test]
    fn full_phase_capture_fits_metrics_transport_with_payload_headroom() {
        use crate::measurement::{FramePhaseTimeline, FrameWorkTiming};
        let evidence = FrameEvidence {
            phases_enabled: true,
            helper: Some(HelperEvidence {
                discarded_generation: Some(0),
                ..Default::default()
            }),
            cpu_us: [Some(0); 7],
            input_sequence: Some(0),
            input_captured_monotonic_us: Some(0),
            input_dequeued_us: Some(0),
            bridge_model_us: Some(0),
            bridge_stages_us: Some([0; 6]),
            bridge_presenter_us: Some([0; 4]),
            bridge_hub_counts_us: Some([0; 2]),
            destination_stage_us: Some([0; 4]),
            destination_list_us: Some([0; 2]),
            previous_observation_attempt_id: Some(0),
            ownership_loss_count: Some(0),
            raw_presented_count: Some(0),
            raw_repeat_count: Some(0),
            content_generation: Some(0),
            motion_continues_after_present: Some(false),
            previous_read_bracket_us: Some([0; 2]),
            refresh_counter: Some(0),
            telemetry_flags: Some(0),
            record: DroppedFrameRecord {
                owned_refresh_observed: Some(0),
                active_sequence: Some(0),
                work: Some(FrameWorkTiming {
                    discarded_helper_cpu_us: Some(0),
                    primary_cpu_us: Some(0),
                    secondary_cpu_us: Some(0),
                    primary_run_delay_us: Some(0),
                    secondary_run_delay_us: Some(0),
                    ..Default::default()
                }),
                timeline: Some(FramePhaseTimeline {
                    post_request_start_us: Some(0),
                    post_verified_us: Some(0),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        fn widen(value: &mut Value) {
            match value {
                Value::Number(_) => *value = u64::MAX.into(),
                Value::String(_) => *value = "x".repeat(128).into(),
                Value::Array(values) => values.iter_mut().for_each(widen),
                Value::Object(values) => values.values_mut().for_each(widen),
                _ => {}
            }
        }
        let mut frame = evidence.json();
        widen(&mut frame);
        let mut capture = FrameEvidenceCapture::default();
        capture.reset(EvidenceMode::Phases);
        let mut payload = capture.json();
        payload["frames"] = vec![frame; CAPACITY].into();
        // The negotiated metrics-body-32m-v1 contract leaves 4 MiB for
        // the enclosing window, drop records, thread metrics and clock samples.
        let bytes = serde_json::to_vec(&payload).unwrap().len();
        assert!(
            bytes > 16 * 1024 * 1024,
            "fixture must exercise the old limit"
        );
        assert!(
            bytes + 4 * 1024 * 1024 < 32 * 1024 * 1024,
            "full capture uses {bytes} bytes"
        );
    }

    #[test]
    fn merges_overlapping_drop_neighborhoods_and_keeps_successes() {
        let mut capture = FrameEvidenceCapture::default();
        capture.reset(EvidenceMode::Neighbors);
        for id in 1..=17 {
            capture.observe(frame(id, u64::from([9, 11].contains(&id))));
        }
        let ids = capture
            .retained
            .iter()
            .map(|f| f.attempt_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, [1, 2, 3, 6, 7, 8, 9, 10, 11, 12, 13]);
        assert_eq!(
            capture
                .retained
                .iter()
                .map(|f| f.record.dropped_frames)
                .sum::<u64>(),
            2
        );
    }
    #[test]
    fn sequence_wrap_reset_and_skipped_attempt_are_not_hidden() {
        let mut capture = FrameEvidenceCapture::default();
        capture.reset(EvidenceMode::Neighbors);
        let mut before = frame(5, 0);
        before.record.active_sequence = Some(u16::MAX);
        before.refresh_counter = Some(u32::MAX);
        capture.observe(before);
        let mut after = frame(7, 2);
        after.record.active_sequence = Some(0);
        after.refresh_counter = Some(1);
        after.baseline_reset = true;
        capture.observe(after);
        assert_eq!(capture.gaps, 1);
        assert!(capture.retained[1].baseline_reset);
        assert_eq!(capture.retained[1].record.active_sequence, Some(0));
    }
    #[test]
    fn superseded_post_and_missing_pose_are_retained_without_inventing_refresh_drops() {
        let mut capture = FrameEvidenceCapture::default();
        capture.reset(EvidenceMode::Phases);
        let mut attempted = frame(1, 0);
        attempted.outcome = "superseded-before-confirmation";
        attempted.phases_enabled = true;
        attempted.produced_frame_id = 1;
        attempted.cpu_us[0] = Some(12);
        attempted.cpu_us[6] = Some(22);
        capture.observe(attempted);
        let mut next = frame(2, 0);
        next.missing_fresh_pose = 1;
        capture.observe(next);
        let result = capture.json();
        assert_eq!(result["frames"][0]["observation"]["dropped_frames"], 0);
        assert_eq!(result["frames"][0]["phases"]["cpu_us"][1], Value::Null);
        assert_eq!(result["frames"][1]["missing_fresh_pose"], 1);
    }

    #[test]
    fn isolated_missing_pose_and_view_input_readiness_edges_keep_full_neighborhoods() {
        for change in 0..6 {
            let mut capture = FrameEvidenceCapture::default();
            capture.reset(EvidenceMode::Neighbors);
            for id in 1..=20 {
                let mut current = frame(id, 0);
                current.view = "home";
                current.outcome = "active";
                if id >= 9 {
                    match change {
                        0 if id == 9 => current.missing_fresh_pose = 1,
                        1 => current.view = "system-hub",
                        2 => current.input_epoch = 1,
                        3 => current.card_snapshot_locked = true,
                        4 => current.producer_cancelled = true,
                        5 => current.producer_ready_depth = 1,
                        _ => {}
                    }
                }
                capture.observe(current);
            }
            let ids = capture
                .retained
                .iter()
                .map(|frame| frame.attempt_id)
                .collect::<Vec<_>>();
            for id in 6..=11 {
                assert!(ids.contains(&id), "change={change}, missing frame={id}");
            }
            assert_eq!(
                capture
                    .retained
                    .iter()
                    .map(|f| f.record.dropped_frames)
                    .sum::<u64>(),
                0
            );
        }
    }

    #[test]
    fn dense_long_window_retains_every_drop_without_growing_during_capture() {
        let mut capture = FrameEvidenceCapture::default();
        capture.reset(EvidenceMode::Phases);
        let reserved = capture.retained.capacity();
        for id in 1..=2700 {
            let mut current = frame(id, 1);
            current.phases_enabled = true;
            current.cpu_us = [Some(id * 16_667); 7];
            current.helper = Some(HelperEvidence::default());
            current.record.timeline = Some(Default::default());
            capture.observe(current);
        }
        assert_eq!(capture.retained.len(), 2700);
        assert_eq!(capture.retained.capacity(), reserved);
        assert_eq!(capture.overflow, 0);
        assert_eq!(
            capture
                .retained
                .iter()
                .map(|f| f.record.dropped_frames)
                .sum::<u64>(),
            2700
        );
        capture.reset(EvidenceMode::Off);
        assert_eq!(capture.retained.capacity(), 0);
    }

    #[test]
    fn retention_is_bounded_and_off_allocates_nothing() {
        let mut capture = FrameEvidenceCapture::default();
        capture.observe_timed(frame(1, 1), Instant::now());
        assert_eq!(capture.retained.capacity(), 0);
        assert_eq!(capture.observer_us.capacity(), 0);
        assert_eq!(capture.observed, 0);
        capture.reset(EvidenceMode::Neighbors);
        for id in 1..=CAPACITY as u64 + 32 {
            capture.observe(frame(id, 1));
        }
        assert_eq!(capture.retained.len(), CAPACITY);
        assert!(capture.overflow > 0);
        assert_eq!(capture.retained.capacity(), CAPACITY);
    }
}
