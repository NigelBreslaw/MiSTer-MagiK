// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::cli::OutputFormat;
use crate::evidence::Evidence;
use serde::Serialize;
use std::time::Instant;

pub const HEARTBEAT_MS: u64 = 10_000;
pub const PERCENT_STEP: u8 = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Started,
    Progress,
    Warning,
    Completed,
    Failed,
}

impl EventKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Progress => "progress",
            Self::Warning => "warning",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProgressEvent {
    pub v: u8,
    #[serde(rename = "event")]
    pub kind: EventKind,
    pub run: String,
    pub seq: u32,
    pub elapsed_ms: u64,
    pub phase: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProgressGate {
    last_emit_ms: Option<u64>,
    last_phase: Option<String>,
    last_percent_bucket: Option<u8>,
}

impl ProgressGate {
    pub fn should_emit(
        &mut self,
        now_ms: u64,
        kind: EventKind,
        phase: &str,
        percent: Option<u8>,
    ) -> bool {
        let bucket = percent.map(|value| value.min(100) / PERCENT_STEP);
        let immediate = kind != EventKind::Progress
            || self.last_phase.as_deref() != Some(phase)
            || bucket > self.last_percent_bucket;
        let heartbeat = self
            .last_emit_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= HEARTBEAT_MS);
        if immediate || heartbeat {
            self.last_emit_ms = Some(now_ms);
            self.last_phase = Some(phase.to_owned());
            if bucket.is_some() {
                self.last_percent_bucket = bucket;
            }
            true
        } else {
            false
        }
    }
}

pub struct Reporter<'a> {
    evidence: &'a Evidence,
    output: OutputFormat,
    run: &'a str,
    started: Instant,
    sequence: u32,
    gate: ProgressGate,
}

impl<'a> Reporter<'a> {
    #[must_use]
    pub fn new(evidence: &'a Evidence, output: OutputFormat, run: &'a str) -> Self {
        Self {
            evidence,
            output,
            run,
            started: Instant::now(),
            sequence: 0,
            gate: ProgressGate::default(),
        }
    }

    pub fn emit(
        &mut self,
        kind: EventKind,
        phase: &str,
        message: &str,
        percent: Option<u8>,
    ) -> Result<(), String> {
        let elapsed_ms = self
            .started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        if !self.gate.should_emit(elapsed_ms, kind, phase, percent) {
            return Ok(());
        }
        let event = ProgressEvent {
            v: 1,
            kind,
            run: self.run.to_owned(),
            seq: self.sequence,
            elapsed_ms,
            phase: phase.to_owned(),
            message: message.to_owned(),
            percent: percent.map(|value| value.min(100)),
        };
        self.sequence = self.sequence.saturating_add(1);
        self.evidence.record_event(&event)?;
        match self.output {
            OutputFormat::Ndjson => println!("{}", serde_json::to_string(&event).unwrap()),
            OutputFormat::Human => println!("{}", render_human(&event)),
        }
        Ok(())
    }
}

#[must_use]
pub fn render_human(event: &ProgressEvent) -> String {
    event.percent.map_or_else(
        || format!("{}: {}", event.phase, event.message),
        |percent| format!("{}: {} ({percent}%)", event.phase, event.message),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_coalesced_until_heartbeat_or_ten_percent_boundary() {
        let mut gate = ProgressGate::default();
        assert!(gate.should_emit(0, EventKind::Started, "transfer", Some(0)));
        assert!(!gate.should_emit(1_000, EventKind::Progress, "transfer", Some(9)));
        assert!(gate.should_emit(2_000, EventKind::Progress, "transfer", Some(10)));
        assert!(!gate.should_emit(11_999, EventKind::Progress, "transfer", Some(19)));
        assert!(gate.should_emit(12_000, EventKind::Progress, "transfer", Some(19)));
        assert!(gate.should_emit(12_001, EventKind::Progress, "verify", None));
    }

    #[test]
    fn warnings_and_completion_are_immediate() {
        let mut gate = ProgressGate::default();
        assert!(gate.should_emit(0, EventKind::Progress, "build", None));
        assert!(gate.should_emit(1, EventKind::Warning, "build", None));
        assert!(gate.should_emit(2, EventKind::Completed, "done", Some(100)));
    }

    #[test]
    fn ndjson_contract_is_compact_and_omits_absent_percent() {
        let event = ProgressEvent {
            v: 1,
            kind: EventKind::Progress,
            run: "run-1".into(),
            seq: 3,
            elapsed_ms: 20_000,
            phase: "transfer".into(),
            message: "Uploading ARM binary".into(),
            percent: None,
        };
        assert_eq!(serde_json::to_string(&event).unwrap(), "{\"v\":1,\"event\":\"progress\",\"run\":\"run-1\",\"seq\":3,\"elapsed_ms\":20000,\"phase\":\"transfer\",\"message\":\"Uploading ARM binary\"}");
    }
}
