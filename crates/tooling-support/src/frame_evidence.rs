//! Opt-in, bounded observation neighborhoods. No allocation or JSON per frame.
use crate::measurement::DroppedFrameRecord;
use serde_json::{Value, json};
use std::collections::VecDeque;

const PREDECESSORS: usize = 3;
const SUCCESSORS: usize = 2;
const CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EvidenceMode {
    #[default]
    Off,
    Neighbors,
}
impl EvidenceMode {
    pub fn from_request(value: &Value) -> Result<Self, String> {
        match value.as_str() {
            None if value.is_null() => Ok(Self::Off),
            Some("off") => Ok(Self::Off),
            Some("neighbors") => Ok(Self::Neighbors),
            _ => Err("frame_evidence must be off or neighbors".into()),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Neighbors => "neighbors",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameEvidence {
    pub attempt_id: u64,
    pub begin_us: u64,
    pub request_generation: u64,
    pub missing_fresh_pose: u64,
    pub ownership_loss_count: Option<u32>,
    pub raw_presented_count: Option<u32>,
    pub raw_repeat_count: Option<u32>,
    pub logical_time_us: u64,
    pub menu_token: u64,
    pub selected: usize,
    pub pose_phase: &'static str,
    pub pose_progress: u64,
    pub content_generation: Option<u64>,
    pub input_generation: u64,
    pub motion: bool,
    pub baseline_reset: bool,
    pub telemetry_valid: bool,
    pub telemetry_before_us: u64,
    pub previous_read_bracket_us: [u64; 2],
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
        json!({"attempt_id":self.attempt_id,"begin_us":self.begin_us,
            "request_generation":self.request_generation,"missing_fresh_pose":self.missing_fresh_pose,"ownership_loss_count":self.ownership_loss_count,
            "raw_presented_count":self.raw_presented_count,"raw_repeat_count":self.raw_repeat_count,
            "logical_time_us":self.logical_time_us,
            "menu_token":self.menu_token,"selected":self.selected,"pose_phase":self.pose_phase,
            "pose_progress":self.pose_progress,"content_generation":self.content_generation,
            "input_generation":self.input_generation,"motion":self.motion,
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
    last: Option<FrameEvidence>,
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
    pub fn note_observer_us(&mut self, us: u64) {
        if self.observer_us.len() < 3601 {
            self.observer_us.push(us);
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
        if let Some(previous) = self.last {
            self.gaps += frame.attempt_id.saturating_sub(previous.attempt_id + 1);
        }
        let edge = self.last.is_none_or(|previous| {
            previous.motion != frame.motion
                || previous.input_generation != frame.input_generation
                || previous.menu_token != frame.menu_token
                || previous.outcome != frame.outcome
        });
        let trigger = edge
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
        self.last = Some(frame);
    }
    pub fn json(&self) -> Value {
        let mut samples = self.observer_us.clone();
        samples.sort_unstable();
        json!({"schema":"frame-neighborhood-v1","mode":self.mode.label(),
            "predecessors":PREDECESSORS,"successors":SUCCESSORS,"capacity":CAPACITY,
            "observed_frames":self.observed,"retention_overflow":self.overflow,
            "unobserved_attempts":self.gaps,"clock_resolution_us":1,
            "clock_brackets":{"columns":["app_before_us","clock_monotonic_us","app_after_us"],"samples":self.clocks},
            "deadline":"unknown: FPGA acceptance/cutoff timestamps unavailable",
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
    fn retention_is_bounded_and_off_allocates_nothing() {
        let mut capture = FrameEvidenceCapture::default();
        capture.observe(frame(1, 1));
        assert_eq!(capture.retained.capacity(), 0);
        assert_eq!(capture.observed, 0);
        capture.reset(EvidenceMode::Neighbors);
        for id in 1..=1000 {
            capture.observe(frame(id, 1));
        }
        assert_eq!(capture.retained.len(), CAPACITY);
        assert!(capture.overflow > 0);
        assert_eq!(capture.retained.capacity(), CAPACITY);
    }
}
