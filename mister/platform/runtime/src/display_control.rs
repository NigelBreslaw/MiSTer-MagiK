// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Typed display transaction adapter over the serialized Main command channel.

use crate::display_resolution;
use crate::main_command::{self, MainCommand};
use mister_magik_core::launcher_effects::{
    DisplayControl, DisplayState, DisplayStateRead, DisplayTransactionPhase, LauncherEffectFailure,
    LauncherEffectFailureKind,
};

const DISPLAY_CONFIRM_SECONDS: u8 = 20;

pub struct MainDisplayControl;

impl MainDisplayControl {
    fn command_failure(detail: impl Into<String>) -> LauncherEffectFailure {
        LauncherEffectFailure::new(LauncherEffectFailureKind::Unavailable, detail)
    }

    fn response_failure(detail: impl Into<String>) -> LauncherEffectFailure {
        LauncherEffectFailure::new(LauncherEffectFailureKind::MalformedResponse, detail)
    }
}

impl DisplayControl for MainDisplayControl {
    fn state(&mut self, read: DisplayStateRead) -> Result<DisplayState, LauncherEffectFailure> {
        let response = match read {
            DisplayStateRead::Wait => main_command::execute(&MainCommand::DisplayState),
            DisplayStateRead::Try => main_command::try_execute(&MainCommand::DisplayState),
        }
        .map_err(|error| Self::command_failure(error.to_string()))?
        .ok_or_else(|| Self::response_failure("MiSTer display command returned no reply"))?;
        parse_state_response(&response).map_err(Self::response_failure)
    }

    fn apply(&mut self, mode: &str) -> Result<(), LauncherEffectFailure> {
        if display_resolution::find(mode).is_none() {
            return Err(Self::response_failure("unsupported display mode"));
        }
        main_command::execute(&MainCommand::DisplayApply {
            mode: mode.to_string(),
        })
        .map(|_| ())
        .map_err(|error| Self::command_failure(error.to_string()))
    }

    fn confirm(&mut self) -> Result<(), LauncherEffectFailure> {
        main_command::execute(&MainCommand::DisplayConfirm)
            .map(|_| ())
            .map_err(|error| Self::command_failure(error.to_string()))
    }

    fn cancel(&mut self) -> Result<(), LauncherEffectFailure> {
        main_command::execute(&MainCommand::DisplayCancel)
            .map(|_| ())
            .map_err(|error| Self::command_failure(error.to_string()))
    }
}

pub fn parse_state_response(response: &str) -> Result<DisplayState, String> {
    let mut active_mode = None;
    let mut pending_mode = None;
    let mut remaining_secs = 0;
    let mut schema = None;
    let mut phase = DisplayTransactionPhase::Idle;
    let mut error = None;
    let mut return_to_settings = false;
    for field in response.split_whitespace() {
        if let Some(value) = field.strip_prefix("schema=") {
            schema = Some(value);
        }
        if let Some(value) = field.strip_prefix("active=") {
            active_mode = Some(value.to_owned());
        }
        if let Some(value) = field.strip_prefix("pending=") {
            pending_mode = (value != "none").then(|| value.to_owned());
        }
        if let Some(value) = field.strip_prefix("remaining=") {
            remaining_secs = value
                .parse::<u8>()
                .unwrap_or(0)
                .min(DISPLAY_CONFIRM_SECONDS);
        }
        if let Some(value) = field.strip_prefix("phase=") {
            phase = match value {
                "idle" => DisplayTransactionPhase::Idle,
                "provisional" => DisplayTransactionPhase::Provisional,
                "persisting" => DisplayTransactionPhase::Persisting,
                "failed" => DisplayTransactionPhase::Failed,
                _ => return Err("display state has unsupported phase".into()),
            };
        }
        if let Some(value) = field.strip_prefix("error=") {
            error = (value != "none").then(|| value.to_owned());
        }
        if let Some(value) = field.strip_prefix("return=") {
            return_to_settings = match value {
                "none" => false,
                "settings" => true,
                _ => return Err("display state has unsupported return screen".into()),
            };
        }
    }
    if schema != Some("1") {
        return Err("display state has unsupported schema".into());
    }
    if pending_mode
        .as_deref()
        .is_some_and(|id| display_resolution::find(id).is_none())
    {
        return Err("display state has unsupported pending mode".into());
    }
    Ok(DisplayState {
        active_mode: active_mode.ok_or("display state missing active mode")?,
        pending_mode,
        remaining_secs,
        phase,
        error,
        return_to_settings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_state_parser_preserves_deadline_and_transaction_fields() {
        let state = parse_state_response(
            "ok DisplayV1 schema=1 active=custom pending=custom remaining=99 phase=provisional error=none return=settings",
        )
        .unwrap();
        assert_eq!(state.active_mode, "custom");
        assert_eq!(state.pending_mode.as_deref(), Some("custom"));
        assert_eq!(state.remaining_secs, DISPLAY_CONFIRM_SECONDS);
        assert_eq!(state.phase, DisplayTransactionPhase::Provisional);
        assert!(state.error.is_none());
        assert!(state.return_to_settings);
    }

    #[test]
    fn display_state_parser_rejects_invalid_contract_values() {
        assert!(
            parse_state_response("ok DisplayV1 active=custom pending=none remaining=0").is_err()
        );
        assert!(
            parse_state_response(
                "ok DisplayV1 schema=1 active=custom pending=invalid remaining=0 phase=idle error=none return=none"
            )
            .is_err()
        );
    }
}
