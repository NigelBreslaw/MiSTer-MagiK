// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Strict protocol-v2 envelopes for progressive sharded catalog events.

use crate::catalog_classify::SystemId;
use crate::sharded_catalog::RunId;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

pub const SHARDED_CATALOG_PROTOCOL_VERSION: u32 = 2;
pub const DEFAULT_MAX_EVENT_BYTES: usize = 64 * 1024;
const MAX_TEXT_BYTES: usize = 512;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShardedCatalogEnvelope {
    pub protocol: u32,
    pub run_id: String,
    pub intended_generation: u64,
    pub sequence: u64,
    #[serde(flatten)]
    pub event: ShardedCatalogEvent,
}

impl ShardedCatalogEnvelope {
    pub fn new(
        run_id: &RunId,
        intended_generation: u64,
        sequence: u64,
        event: ShardedCatalogEvent,
    ) -> Self {
        Self {
            protocol: SHARDED_CATALOG_PROTOCOL_VERSION,
            run_id: run_id.as_str().to_string(),
            intended_generation,
            sequence,
            event,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ShardedCatalogEvent {
    Handshake {
        operation: String,
        current_generation: Option<u64>,
    },
    PlanReady {
        systems: usize,
    },
    SystemQueued {
        system_id: String,
    },
    SystemScanning {
        system_id: String,
    },
    SystemReady {
        system_id: String,
        generation: u64,
        games: u64,
    },
    SystemFailed {
        system_id: String,
        stage: String,
        error: String,
    },
    PausedForUi {
        system_id: String,
    },
    ManifestPublished {
        generation: u64,
        systems: usize,
    },
    Unchanged {
        generation: Option<u64>,
    },
    Failure {
        stage: String,
        error: String,
    },
    Done,
}

impl ShardedCatalogEvent {
    fn terminal(&self) -> bool {
        matches!(self, Self::Failure { .. } | Self::Done)
    }
}

pub fn decode_event(
    bytes: &[u8],
    max_bytes: usize,
) -> Result<ShardedCatalogEnvelope, ProtocolError> {
    if max_bytes == 0 || bytes.is_empty() || bytes.len() > max_bytes {
        return Err(ProtocolError::new(
            "event size is outside configured bounds",
        ));
    }
    let envelope: ShardedCatalogEnvelope = serde_json::from_slice(bytes)
        .map_err(|error| ProtocolError::with("decode event", error))?;
    validate_envelope(&envelope)?;
    Ok(envelope)
}

fn validate_envelope(envelope: &ShardedCatalogEnvelope) -> Result<(), ProtocolError> {
    if envelope.protocol != SHARDED_CATALOG_PROTOCOL_VERSION {
        return Err(ProtocolError::new("unsupported catalog protocol"));
    }
    RunId::new(envelope.run_id.clone()).map_err(|_| ProtocolError::new("invalid run ID"))?;
    match &envelope.event {
        ShardedCatalogEvent::Handshake { operation, .. } => bounded_text(operation, "operation")?,
        ShardedCatalogEvent::SystemQueued { system_id }
        | ShardedCatalogEvent::SystemScanning { system_id }
        | ShardedCatalogEvent::PausedForUi { system_id } => validate_system(system_id)?,
        ShardedCatalogEvent::SystemReady {
            system_id,
            generation,
            ..
        } => {
            validate_system(system_id)?;
            validate_generation(envelope, *generation)?;
        }
        ShardedCatalogEvent::SystemFailed {
            system_id,
            stage,
            error,
        } => {
            validate_system(system_id)?;
            bounded_text(stage, "stage")?;
            bounded_text(error, "error")?;
        }
        ShardedCatalogEvent::ManifestPublished { generation, .. } => {
            validate_generation(envelope, *generation)?;
        }
        ShardedCatalogEvent::Failure { stage, error } => {
            bounded_text(stage, "stage")?;
            bounded_text(error, "error")?;
        }
        ShardedCatalogEvent::Unchanged { generation } => {
            if generation.is_some_and(|value| value != envelope.intended_generation) {
                return Err(ProtocolError::new(
                    "unchanged generation does not match its envelope",
                ));
            }
        }
        ShardedCatalogEvent::PlanReady { .. } | ShardedCatalogEvent::Done => {}
    }
    Ok(())
}

fn validate_system(value: &str) -> Result<(), ProtocolError> {
    match SystemId::parse(value) {
        Ok(system_id) if system_id.as_str() == value => Ok(()),
        Ok(_) | Err(_) => Err(ProtocolError::new("invalid canonical system ID")),
    }
}

fn validate_generation(
    envelope: &ShardedCatalogEnvelope,
    generation: u64,
) -> Result<(), ProtocolError> {
    if generation != envelope.intended_generation {
        return Err(ProtocolError::new(
            "event generation does not match its envelope",
        ));
    }
    Ok(())
}

fn bounded_text(value: &str, label: &str) -> Result<(), ProtocolError> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
        return Err(ProtocolError::new(format!("invalid {label}")));
    }
    Ok(())
}

#[derive(Clone, Default)]
pub struct ProtocolSequence {
    run_id: Option<String>,
    intended_generation: Option<u64>,
    next_sequence: u64,
    outcome_seen: bool,
    terminal: bool,
}

impl ProtocolSequence {
    pub fn accept(&mut self, envelope: &ShardedCatalogEnvelope) -> Result<(), ProtocolError> {
        validate_envelope(envelope)?;
        if self.terminal {
            return Err(ProtocolError::new("event arrived after terminal event"));
        }
        if self.run_id.is_none() {
            if envelope.sequence != 0
                || !matches!(envelope.event, ShardedCatalogEvent::Handshake { .. })
            {
                return Err(ProtocolError::new(
                    "first event is not sequence-zero handshake",
                ));
            }
            self.run_id = Some(envelope.run_id.clone());
            self.intended_generation = Some(envelope.intended_generation);
        } else {
            if matches!(envelope.event, ShardedCatalogEvent::Handshake { .. }) {
                return Err(ProtocolError::new("duplicate handshake"));
            }
            if self.run_id.as_deref() != Some(&envelope.run_id)
                || self.intended_generation != Some(envelope.intended_generation)
            {
                return Err(ProtocolError::new("stale or mismatched event correlation"));
            }
        }
        if envelope.sequence != self.next_sequence {
            return Err(ProtocolError::new("event sequence is not contiguous"));
        }
        if matches!(envelope.event, ShardedCatalogEvent::Done) && !self.outcome_seen {
            return Err(ProtocolError::new("done arrived before a catalog outcome"));
        }
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| ProtocolError::new("event sequence overflow"))?;
        if matches!(
            envelope.event,
            ShardedCatalogEvent::ManifestPublished { .. } | ShardedCatalogEvent::Unchanged { .. }
        ) {
            self.outcome_seen = true;
        }
        self.terminal = envelope.event.terminal();
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolError {
    message: String,
}

impl ProtocolError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn with(label: &str, error: impl fmt::Display) -> Self {
        Self::new(format!("{label}: {error}"))
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_two_round_trips_progressive_system_events() {
        let run_id = RunId::new("fixture-run").unwrap();
        let event = ShardedCatalogEnvelope::new(
            &run_id,
            7,
            2,
            ShardedCatalogEvent::SystemReady {
                system_id: "arcade".to_string(),
                generation: 7,
                games: 42,
            },
        );
        let bytes = serde_json::to_vec(&event).unwrap();
        assert_eq!(
            decode_event(&bytes, DEFAULT_MAX_EVENT_BYTES).unwrap(),
            event
        );
    }

    #[test]
    fn sequence_rejects_stale_runs_gaps_and_events_after_done() {
        let first = RunId::new("first").unwrap();
        let second = RunId::new("second").unwrap();
        let mut sequence = ProtocolSequence::default();
        sequence
            .accept(&ShardedCatalogEnvelope::new(
                &first,
                4,
                0,
                ShardedCatalogEvent::Handshake {
                    operation: "reconcile".to_string(),
                    current_generation: Some(3),
                },
            ))
            .unwrap();
        assert!(
            sequence
                .accept(&ShardedCatalogEnvelope::new(
                    &second,
                    4,
                    1,
                    ShardedCatalogEvent::PlanReady { systems: 2 },
                ))
                .is_err()
        );
        assert!(
            sequence
                .accept(&ShardedCatalogEnvelope::new(
                    &first,
                    4,
                    2,
                    ShardedCatalogEvent::PlanReady { systems: 2 },
                ))
                .is_err()
        );
        sequence
            .accept(&ShardedCatalogEnvelope::new(
                &first,
                4,
                1,
                ShardedCatalogEvent::Unchanged {
                    generation: Some(4),
                },
            ))
            .unwrap();
        sequence
            .accept(&ShardedCatalogEnvelope::new(
                &first,
                4,
                2,
                ShardedCatalogEvent::Done,
            ))
            .unwrap();
        assert!(
            sequence
                .accept(&ShardedCatalogEnvelope::new(
                    &first,
                    4,
                    3,
                    ShardedCatalogEvent::Done,
                ))
                .is_err()
        );
    }

    #[test]
    fn decoder_rejects_wrong_generation_noncanonical_ids_and_oversize_lines() {
        let run_id = RunId::new("fixture").unwrap();
        let wrong_generation = ShardedCatalogEnvelope::new(
            &run_id,
            8,
            1,
            ShardedCatalogEvent::SystemReady {
                system_id: "arcade".to_string(),
                generation: 7,
                games: 1,
            },
        );
        assert!(
            decode_event(
                &serde_json::to_vec(&wrong_generation).unwrap(),
                DEFAULT_MAX_EVENT_BYTES
            )
            .is_err()
        );
        let noncanonical = ShardedCatalogEnvelope::new(
            &run_id,
            8,
            1,
            ShardedCatalogEvent::SystemQueued {
                system_id: "SNES".to_string(),
            },
        );
        assert!(
            decode_event(
                &serde_json::to_vec(&noncanonical).unwrap(),
                DEFAULT_MAX_EVENT_BYTES
            )
            .is_err()
        );
        assert!(decode_event(&[b' '; 9], 8).is_err());
    }
}
