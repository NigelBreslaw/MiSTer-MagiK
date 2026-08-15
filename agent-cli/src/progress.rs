// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::cli::OutputFormat;
use crate::error::AgentError;
use crate::evidence::Evidence;
use serde::{Deserialize, Serialize};
use std::time::Instant;

pub const HEARTBEAT_MS: u64 = 10_000;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailureEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FailureEvidence {
    pub code: String,
    pub phase: String,
    pub retry_policy: String,
    pub recovery_required: bool,
}

impl FailureEvidence {
    fn from_error(error: &AgentError) -> Option<Self> {
        let failure = error.structured_failure()?;
        Some(Self {
            code: failure.code.as_str().to_owned(),
            phase: failure.phase.as_str().to_owned(),
            retry_policy: failure.retry_policy.as_str().to_owned(),
            recovery_required: failure.recovery_required,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProgressGate {
    last_emit_ms: Option<u64>,
    last_phase: Option<String>,
}

impl ProgressGate {
    pub fn should_emit(
        &mut self,
        now_ms: u64,
        kind: EventKind,
        phase: &str,
        _percent: Option<u8>,
    ) -> bool {
        let immediate = kind != EventKind::Progress || self.last_phase.as_deref() != Some(phase);
        let heartbeat = self
            .last_emit_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= HEARTBEAT_MS);
        if immediate || heartbeat {
            self.last_emit_ms = Some(now_ms);
            self.last_phase = Some(phase.to_owned());
            true
        } else {
            false
        }
    }
}

pub struct Reporter<'a> {
    evidence: &'a Evidence,
    run: &'a str,
    started: Instant,
    sequence: u32,
    gate: ProgressGate,
    pending: Vec<ProgressEvent>,
}

impl<'a> Reporter<'a> {
    #[must_use]
    pub fn new(evidence: &'a Evidence, output: OutputFormat, run: &'a str) -> Self {
        Self::new_at(evidence, output, run, Instant::now())
    }

    #[must_use]
    pub fn new_at(
        evidence: &'a Evidence,
        _output: OutputFormat,
        run: &'a str,
        started: Instant,
    ) -> Self {
        Self {
            evidence,
            run,
            started,
            sequence: 0,
            gate: ProgressGate::default(),
            pending: Vec::new(),
        }
    }

    pub fn emit(
        &mut self,
        kind: EventKind,
        phase: &str,
        message: &str,
        percent: Option<u8>,
    ) -> Result<(), String> {
        self.emit_with_failure(kind, phase, message, percent, None)
    }

    pub fn emit_failure(&mut self, phase: &str, error: &AgentError) -> Result<(), String> {
        self.emit_with_failure(
            EventKind::Failed,
            phase,
            &error.to_string(),
            None,
            FailureEvidence::from_error(error),
        )
    }

    fn emit_with_failure(
        &mut self,
        kind: EventKind,
        phase: &str,
        message: &str,
        percent: Option<u8>,
        failure: Option<FailureEvidence>,
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
            failure,
        };
        self.sequence = self.sequence.saturating_add(1);
        self.pending.push(event.clone());
        if kind != EventKind::Progress || self.pending.len() >= 4 {
            self.evidence.record_events(&self.pending)?;
            self.pending.clear();
        }
        if event.kind != EventKind::Started
            && !(event.kind == EventKind::Completed && event.message == "Request complete")
        {
            eprintln!("{}", render_human(&event));
        }
        Ok(())
    }
}

#[must_use]
pub fn render_human(event: &ProgressEvent) -> String {
    let rendered = event.percent.map_or_else(
        || format!("{}: {}", event.phase, event.message),
        |percent| format!("{}: {} ({percent}%)", event.phase, event.message),
    );
    if event.message.starts_with("running") || event.message.starts_with("passed") {
        format!("{rendered} — {} elapsed", elapsed(event.elapsed_ms))
    } else {
        rendered
    }
}

fn elapsed(elapsed_ms: u64) -> String {
    let seconds = elapsed_ms / 1_000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_coalesced_until_heartbeat_or_phase_change() {
        let mut gate = ProgressGate::default();
        assert!(gate.should_emit(0, EventKind::Started, "transfer", Some(0)));
        assert!(!gate.should_emit(1_000, EventKind::Progress, "transfer", Some(9)));
        assert!(!gate.should_emit(2_000, EventKind::Progress, "transfer", Some(10)));
        assert!(!gate.should_emit(2_100, EventKind::Progress, "transfer", Some(19)));
        assert!(!gate.should_emit(2_200, EventKind::Progress, "transfer", Some(20)));
        assert!(gate.should_emit(10_000, EventKind::Progress, "transfer", Some(20)));
        assert!(!gate.should_emit(13_000, EventKind::Progress, "transfer", Some(19)));
        assert!(gate.should_emit(13_001, EventKind::Progress, "verify", None));
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
            failure: None,
        };
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            "{\"v\":1,\"event\":\"progress\",\"run\":\"run-1\",\"seq\":3,\"elapsed_ms\":20000,\"phase\":\"transfer\",\"message\":\"Uploading ARM binary\"}"
        );
    }

    #[test]
    fn failure_projection_is_structured_and_omits_device_detail() {
        let error = AgentError::phase(
            "install",
            AgentError::structured_device(
                "legacy human text",
                mister_magik_agent_protocol::FailureMetadata {
                    code: mister_magik_agent_protocol::FailureCode::ArtifactMismatch,
                    detail: "sensitive device detail".to_string(),
                    phase: mister_magik_agent_protocol::FailurePhase::Artifact,
                    retry_policy: mister_magik_agent_protocol::RetryPolicy::ReconcileThenRetry,
                    recovery_required: false,
                },
            ),
        );
        let failure = FailureEvidence::from_error(&error).unwrap();
        let event = ProgressEvent {
            v: 1,
            kind: EventKind::Failed,
            run: "run-2".into(),
            seq: 4,
            elapsed_ms: 21_000,
            phase: "request".into(),
            message: error.to_string(),
            percent: None,
            failure: Some(failure),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["failure"]["code"], "artifact_mismatch");
        assert_eq!(json["failure"]["phase"], "artifact");
        assert_eq!(json["failure"]["retry_policy"], "reconcile_then_retry");
        assert!(json["failure"].get("detail").is_none());
        assert_eq!(render_human(&event), "request: install: legacy human text");
    }
}
