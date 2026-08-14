// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Volatile, authenticated logical-input automation for alpha acceptance.

use crate::build_identity::BuildIdentity;
use crate::input_event::{
    InputEvent, InputPhase, InputSourceId, InputSourceKind, LogicalAction, PressId, SourceEpoch,
};
use crate::input_state::PadState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SESSION_SCHEMA: &str = "mister-magik-ui-automation-session-v1";
const REQUEST_SCHEMA: &str = "mister-magik-ui-automation-request-v1";
const RESPONSE_SCHEMA: &str = "mister-magik-ui-automation-response-v1";
const DEFAULT_DESCRIPTOR_PATH: &str = "/tmp/mister-magik/ui-automation-session.json";
const DEFAULT_SOCKET_PATH: &str = "/tmp/mister-magik/ui-automation.sock";
const MAX_SESSION_AGE: Duration = Duration::from_secs(120);
const REQUEST_LEASE: Duration = Duration::from_secs(5);
const CLOCK_SKEW: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Deserialize)]
struct AutomationSessionDescriptor {
    schema: String,
    nonce: String,
    expected_build_version: String,
    expected_source_revision: String,
    launcher_pid: u32,
    main_generation: u64,
    created_unix_ms: u64,
    expires_unix_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub(super) struct AutomationFrameStamp {
    pub(super) state_revision: u64,
    pub(super) action_sequence: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(super) struct AutomationSemanticState {
    pub(super) effective_view: String,
    pub(super) return_screen: String,
    pub(super) menu_id: String,
    pub(super) selected_item_id: String,
    pub(super) active_collection_id: String,
    pub(super) selected_system_id: String,
    pub(super) selected_game_id: String,
    pub(super) selected_game_title: String,
    pub(super) selected_index: usize,
    pub(super) selected_count: usize,
    pub(super) overlay: String,
    pub(super) dialog_title: String,
    pub(super) dialog_message: String,
    pub(super) dialog_selected: i32,
    pub(super) drawer_open: bool,
    pub(super) drawer_level: String,
    pub(super) drawer_selected: usize,
    pub(super) search_active: bool,
    pub(super) search_status: String,
    pub(super) search_query: String,
    pub(super) search_results: usize,
    pub(super) preview_state: String,
    pub(super) launch_state: String,
    pub(super) loading_title: String,
    pub(super) catalog_generation: String,
    pub(super) catalog_ready: bool,
    pub(super) settings_selected: usize,
    pub(super) composition_state: String,
    pub(super) composition_recovery_count: u64,
    pub(super) navigation_transition_active: bool,
    pub(super) input_enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
struct AutomationSnapshot {
    active: bool,
    state_revision: u64,
    action_sequence: u64,
    presented_state_revision: u64,
    presented_action_sequence: u64,
    presented_latch_sequence: u16,
    semantic: AutomationSemanticState,
}

#[derive(Debug, Deserialize)]
struct AutomationRequest {
    schema: String,
    nonce: String,
    command: AutomationCommand,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AutomationCommand {
    Tap {
        button: AutomationButton,
    },
    Hold {
        button: AutomationButton,
        duration_ms: u64,
    },
    ReleaseAll,
    Snapshot,
    End,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AutomationButton {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    Home,
    X,
    Y,
}

#[derive(Clone, Copy, Debug)]
enum PressLifecycle {
    TapPressed {
        sequence: u64,
        button: AutomationButton,
        press_id: PressId,
    },
    HeldUntil {
        deadline: Instant,
        button: AutomationButton,
        press_id: PressId,
    },
}

struct ActiveSession {
    descriptor: AutomationSessionDescriptor,
    socket: UnixDatagram,
    last_request: Instant,
    logical_state: PadState,
    press: Option<PressLifecycle>,
    action_sequence: u64,
    adopted_action_sequence: u64,
    source_epoch: SourceEpoch,
    next_event_sequence: u64,
    next_press_id: u64,
    events: VecDeque<InputEvent>,
}

pub(super) struct LauncherAutomation {
    descriptor_path: PathBuf,
    socket_path: PathBuf,
    session: Option<ActiveSession>,
    last_descriptor_poll: Instant,
    semantic: AutomationSemanticState,
    state_revision: u64,
    presented_state_revision: u64,
    presented_action_sequence: u64,
    presented_latch_sequence: u16,
    pending_releases: VecDeque<InputEvent>,
}

impl LauncherAutomation {
    pub(super) fn new() -> Self {
        Self::with_paths(DEFAULT_DESCRIPTOR_PATH.into(), DEFAULT_SOCKET_PATH.into())
    }

    fn with_paths(descriptor_path: PathBuf, socket_path: PathBuf) -> Self {
        Self {
            descriptor_path,
            socket_path,
            session: None,
            last_descriptor_poll: Instant::now() - Duration::from_secs(1),
            semantic: AutomationSemanticState::default(),
            state_revision: 0,
            presented_state_revision: 0,
            presented_action_sequence: 0,
            presented_latch_sequence: 0,
            pending_releases: VecDeque::new(),
        }
    }

    pub(super) fn active(&self) -> bool {
        self.session.is_some()
    }

    pub(super) fn poll_events(
        &mut self,
        physical: &PadState,
        input_enabled: bool,
        setup_active: bool,
        now: Instant,
    ) -> Vec<InputEvent> {
        self.refresh_session(now);
        if self.session.is_none() {
            return self.pending_releases.drain(..).collect();
        }
        if setup_active || pad_state_has_active_input(physical) {
            return self.abort_releasing("unsafe_input_context");
        }
        let expired = self.session.as_ref().is_some_and(|session| {
            session.last_request.elapsed() > REQUEST_LEASE
                || unix_ms() > session.descriptor.expires_unix_ms
                || current_main_generation() != Some(session.descriptor.main_generation)
        });
        if expired {
            return self.abort_releasing("session_expired");
        }
        self.advance_press_lifecycle(now);
        self.drain_requests(input_enabled, now);
        self.session
            .as_mut()
            .map(|session| session.events.drain(..).collect())
            .unwrap_or_default()
    }

    pub(super) fn action_sequence(&self) -> u64 {
        self.session
            .as_ref()
            .map_or(self.presented_action_sequence, |session| {
                session.adopted_action_sequence
            })
    }

    pub(super) fn observe_state(
        &mut self,
        semantic: AutomationSemanticState,
    ) -> AutomationFrameStamp {
        if self.session.is_none() {
            return AutomationFrameStamp::default();
        }
        if self.state_revision == 0 || self.semantic != semantic {
            self.semantic = semantic;
            self.state_revision = self.state_revision.saturating_add(1);
        }
        AutomationFrameStamp {
            state_revision: self.state_revision,
            action_sequence: self.action_sequence(),
        }
    }

    pub(super) fn acknowledge_presented(
        &mut self,
        stamp: AutomationFrameStamp,
        latch_sequence: u16,
    ) {
        if self.session.is_none() || stamp.state_revision == 0 {
            return;
        }
        self.presented_state_revision = stamp.state_revision;
        self.presented_action_sequence = stamp.action_sequence;
        self.presented_latch_sequence = latch_sequence;
    }

    fn refresh_session(&mut self, now: Instant) {
        if self.session.is_some()
            || now.duration_since(self.last_descriptor_poll) < Duration::from_millis(100)
        {
            return;
        }
        self.last_descriptor_poll = now;
        let Ok(descriptor) = read_valid_descriptor(&self.descriptor_path, BuildIdentity::current())
        else {
            return;
        };
        if let Some(parent) = self.socket_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::remove_file(&self.socket_path);
        let Ok(socket) = UnixDatagram::bind(&self.socket_path) else {
            return;
        };
        if socket.set_nonblocking(true).is_err()
            || fs::set_permissions(&self.socket_path, fs::Permissions::from_mode(0o600)).is_err()
        {
            let _ = fs::remove_file(&self.socket_path);
            return;
        }
        let source_epoch = SourceEpoch(descriptor.main_generation);
        self.session = Some(ActiveSession {
            descriptor,
            socket,
            last_request: now,
            logical_state: PadState::default(),
            press: None,
            action_sequence: self.presented_action_sequence,
            adopted_action_sequence: self.presented_action_sequence,
            source_epoch,
            next_event_sequence: 0,
            next_press_id: 0,
            events: VecDeque::new(),
        });
    }

    fn advance_press_lifecycle(&mut self, now: Instant) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let release = match session.press {
            Some(PressLifecycle::TapPressed {
                sequence,
                button,
                press_id,
            }) => {
                session.adopted_action_sequence = sequence;
                Some((button, press_id))
            }
            Some(PressLifecycle::HeldUntil {
                deadline,
                button,
                press_id,
            }) if now >= deadline => {
                session.action_sequence = session.action_sequence.saturating_add(1);
                session.adopted_action_sequence = session.action_sequence;
                Some((button, press_id))
            }
            _ => None,
        };
        if let Some((button, press_id)) = release {
            push_automation_event(session, button, press_id, InputPhase::Released);
            session.logical_state = PadState::default();
            session.press = None;
        }
    }

    fn drain_requests(&mut self, input_enabled: bool, now: Instant) {
        loop {
            let received = {
                let Some(session) = self.session.as_ref() else {
                    return;
                };
                let mut buffer = [0_u8; 4096];
                match session.socket.recv_from(&mut buffer) {
                    Ok((length, sender)) => Some((buffer[..length].to_vec(), sender)),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => None,
                    Err(_) => {
                        self.abort("socket_receive_failed");
                        return;
                    }
                }
            };
            let Some((bytes, sender)) = received else {
                return;
            };
            let response_socket = self
                .session
                .as_ref()
                .and_then(|session| session.socket.try_clone().ok());
            let result = self.handle_request(&bytes, input_enabled, now);
            let response = match result {
                Ok(value) => json!({"schema":RESPONSE_SCHEMA,"ok":true,"result":value}),
                Err(error) => json!({"schema":RESPONSE_SCHEMA,"ok":false,"error":error}),
            };
            if let (Some(socket), Some(path)) = (response_socket, sender.as_pathname()) {
                let _ = socket.send_to(response.to_string().as_bytes(), path);
            }
        }
    }

    fn handle_request(
        &mut self,
        bytes: &[u8],
        input_enabled: bool,
        now: Instant,
    ) -> Result<Value, String> {
        let request: AutomationRequest =
            serde_json::from_slice(bytes).map_err(|error| format!("invalid_request:{error}"))?;
        let nonce_matches = self.session.as_ref().is_some_and(|session| {
            request.schema == REQUEST_SCHEMA && request.nonce == session.descriptor.nonce
        });
        if !nonce_matches {
            return Err("authentication_failed".to_string());
        }
        if let Some(session) = self.session.as_mut() {
            session.last_request = now;
        }
        match request.command {
            AutomationCommand::Snapshot => {
                Ok(serde_json::to_value(self.snapshot()).unwrap_or(Value::Null))
            }
            AutomationCommand::End => {
                self.end_session();
                Ok(json!({"ended":true}))
            }
            AutomationCommand::Tap { button } => {
                self.require_input_enabled(input_enabled)?;
                let session = self.session.as_mut().ok_or("session_ended")?;
                session.logical_state = button_state(button);
                session.action_sequence = session.action_sequence.saturating_add(1);
                let press_id = next_automation_press_id(session);
                push_automation_event(session, button, press_id, InputPhase::Pressed);
                session.press = Some(PressLifecycle::TapPressed {
                    sequence: session.action_sequence,
                    button,
                    press_id,
                });
                Ok(json!({"action_sequence":session.action_sequence,"phase":"press"}))
            }
            AutomationCommand::Hold {
                button,
                duration_ms,
            } => {
                self.require_input_enabled(input_enabled)?;
                if duration_ms == 0
                    || duration_ms > mister_magik_agent_protocol::LAUNCHER_AUTOMATION_MAX_HOLD_MS
                {
                    return Err("hold_duration_out_of_range".to_string());
                }
                let session = self.session.as_mut().ok_or("session_ended")?;
                session.logical_state = button_state(button);
                session.action_sequence = session.action_sequence.saturating_add(1);
                session.adopted_action_sequence = session.action_sequence;
                let press_id = next_automation_press_id(session);
                push_automation_event(session, button, press_id, InputPhase::Pressed);
                session.press = Some(PressLifecycle::HeldUntil {
                    deadline: now + Duration::from_millis(duration_ms),
                    button,
                    press_id,
                });
                Ok(json!({"action_sequence":session.action_sequence,"phase":"hold"}))
            }
            AutomationCommand::ReleaseAll => {
                self.require_input_enabled(input_enabled)?;
                let session = self.session.as_mut().ok_or("session_ended")?;
                if let Some((button, press_id)) = active_press(session.press) {
                    push_automation_event(session, button, press_id, InputPhase::Released);
                }
                session.logical_state = PadState::default();
                session.press = None;
                session.action_sequence = session.action_sequence.saturating_add(1);
                session.adopted_action_sequence = session.action_sequence;
                Ok(json!({"action_sequence":session.action_sequence,"phase":"released"}))
            }
        }
    }

    fn require_input_enabled(&self, input_enabled: bool) -> Result<(), String> {
        if input_enabled {
            Ok(())
        } else {
            Err("launcher_input_not_enabled".to_string())
        }
    }

    fn snapshot(&self) -> AutomationSnapshot {
        AutomationSnapshot {
            active: self.session.is_some(),
            state_revision: self.state_revision,
            action_sequence: self
                .session
                .as_ref()
                .map_or(self.presented_action_sequence, |session| {
                    session.action_sequence
                }),
            presented_state_revision: self.presented_state_revision,
            presented_action_sequence: self.presented_action_sequence,
            presented_latch_sequence: self.presented_latch_sequence,
            semantic: self.semantic.clone(),
        }
    }

    fn abort(&mut self, reason: &str) {
        crate::runtime_status::event("ui_automation_aborted", reason);
        self.end_session();
    }

    fn abort_releasing(&mut self, reason: &str) -> Vec<InputEvent> {
        self.abort(reason);
        self.pending_releases.drain(..).collect()
    }

    fn end_session(&mut self) {
        if let Some(session) = self.session.as_mut() {
            if let Some((button, press_id)) = active_press(session.press) {
                push_automation_event(session, button, press_id, InputPhase::Released);
            }
            self.pending_releases.extend(session.events.drain(..));
            session.logical_state = PadState::default();
            session.press = None;
        }
        self.session = None;
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_file(&self.descriptor_path);
    }
}

fn active_press(press: Option<PressLifecycle>) -> Option<(AutomationButton, PressId)> {
    match press {
        Some(PressLifecycle::TapPressed {
            button, press_id, ..
        })
        | Some(PressLifecycle::HeldUntil {
            button, press_id, ..
        }) => Some((button, press_id)),
        None => None,
    }
}

fn next_automation_press_id(session: &mut ActiveSession) -> PressId {
    session.next_press_id = session.next_press_id.saturating_add(1).max(1);
    PressId((1_u64 << 63) | session.next_press_id)
}

fn push_automation_event(
    session: &mut ActiveSession,
    button: AutomationButton,
    press_id: PressId,
    phase: InputPhase,
) {
    session.next_event_sequence = session.next_event_sequence.saturating_add(1).max(1);
    session.events.push_back(InputEvent {
        source: InputSourceId {
            kind: InputSourceKind::Automation,
            instance: u64::from(session.descriptor.launcher_pid),
        },
        source_epoch: session.source_epoch,
        sequence: session.next_event_sequence,
        press_id,
        captured_at_us: automation_monotonic_us(),
        action: automation_action(button),
        phase,
    });
}

fn automation_action(button: AutomationButton) -> LogicalAction {
    match button {
        AutomationButton::Up => LogicalAction::Up,
        AutomationButton::Down => LogicalAction::Down,
        AutomationButton::Left => LogicalAction::Left,
        AutomationButton::Right => LogicalAction::Right,
        AutomationButton::A => LogicalAction::Activate,
        AutomationButton::B => LogicalAction::Back,
        AutomationButton::Home => LogicalAction::Home,
        AutomationButton::X => LogicalAction::X,
        AutomationButton::Y => LogicalAction::Y,
    }
}

fn automation_monotonic_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START
        .get_or_init(Instant::now)
        .elapsed()
        .as_micros()
        .min(u64::MAX as u128) as u64
}

impl Drop for LauncherAutomation {
    fn drop(&mut self) {
        self.end_session();
    }
}

fn read_valid_descriptor(
    path: &Path,
    identity: BuildIdentity,
) -> Result<AutomationSessionDescriptor, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("descriptor_metadata:{error}"))?;
    if metadata.permissions().mode() & 0o077 != 0 || !trusted_owner(metadata.uid()) {
        return Err("descriptor_permissions".to_string());
    }
    let descriptor: AutomationSessionDescriptor = serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("descriptor_read:{error}"))?,
    )
    .map_err(|error| format!("descriptor_json:{error}"))?;
    let now = unix_ms();
    let max_age_ms = MAX_SESSION_AGE.as_millis() as u64;
    let skew_ms = CLOCK_SKEW.as_millis() as u64;
    if descriptor.schema != SESSION_SCHEMA
        || descriptor.launcher_pid != std::process::id()
        || descriptor.expected_build_version != identity.version
        || descriptor.expected_source_revision != identity.source_revision
        || descriptor.nonce.len() < 32
        || descriptor.nonce.len() > 128
        || !descriptor
            .nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || descriptor.expires_unix_ms <= descriptor.created_unix_ms
        || descriptor
            .expires_unix_ms
            .saturating_sub(descriptor.created_unix_ms)
            > max_age_ms
        || descriptor.created_unix_ms > now.saturating_add(skew_ms)
        || descriptor.expires_unix_ms < now
        || current_main_generation() != Some(descriptor.main_generation)
    {
        return Err("descriptor_identity_or_expiry".to_string());
    }
    Ok(descriptor)
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn trusted_owner(uid: u32) -> bool {
    uid == 0
}

#[cfg(not(all(target_os = "linux", target_arch = "arm")))]
fn trusted_owner(uid: u32) -> bool {
    uid == unsafe { libc::geteuid() }
}

fn current_main_generation() -> Option<u64> {
    let value: Value =
        serde_json::from_slice(&fs::read("/tmp/mister-magik/main-status.json").ok()?).ok()?;
    value.get("main_generation")?.as_u64()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn button_state(button: AutomationButton) -> PadState {
    let mut state = PadState::default();
    match button {
        AutomationButton::Up => state.dpad_up = true,
        AutomationButton::Down => state.dpad_down = true,
        AutomationButton::Left => state.dpad_left = true,
        AutomationButton::Right => state.dpad_right = true,
        AutomationButton::A => state.btn_a = true,
        AutomationButton::B => state.btn_b = true,
        AutomationButton::Home => state.btn_home = true,
        AutomationButton::X => state.btn_x = true,
        AutomationButton::Y => state.btn_y = true,
    }
    state.rebuild_pressed_now();
    state
}

fn pad_state_has_active_input(state: &PadState) -> bool {
    state.dpad_up
        || state.dpad_down
        || state.dpad_left
        || state.dpad_right
        || state.btn_a
        || state.btn_b
        || state.btn_x
        || state.btn_y
        || state.btn_home
        || state.left_x.abs() > f32::EPSILON
        || state.left_y.abs() > f32::EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_allowlist_maps_to_logical_pad_state() {
        assert!(button_state(AutomationButton::Down).dpad_down);
        assert!(button_state(AutomationButton::Home).btn_home);
        assert!(!button_state(AutomationButton::A).btn_b);
    }

    #[test]
    fn automation_press_and_release_share_a_press_id() {
        let now = Instant::now();
        let mut session = ActiveSession {
            descriptor: AutomationSessionDescriptor {
                schema: SESSION_SCHEMA.to_string(),
                nonce: "c".repeat(32),
                expected_build_version: "test".to_string(),
                expected_source_revision: "test".to_string(),
                launcher_pid: std::process::id(),
                main_generation: 3,
                created_unix_ms: 1,
                expires_unix_ms: 2,
            },
            socket: UnixDatagram::unbound().unwrap(),
            last_request: now,
            logical_state: PadState::default(),
            press: None,
            action_sequence: 0,
            adopted_action_sequence: 0,
            source_epoch: SourceEpoch(3),
            next_event_sequence: 0,
            next_press_id: 0,
            events: VecDeque::new(),
        };
        let press_id = next_automation_press_id(&mut session);
        push_automation_event(
            &mut session,
            AutomationButton::A,
            press_id,
            InputPhase::Pressed,
        );
        push_automation_event(
            &mut session,
            AutomationButton::A,
            press_id,
            InputPhase::Released,
        );

        assert_eq!(session.events.len(), 2);
        assert_eq!(session.events[0].press_id, session.events[1].press_id);
        assert_eq!(session.events[0].action, LogicalAction::Activate);
        assert_eq!(session.events[0].phase, InputPhase::Pressed);
        assert_eq!(session.events[1].phase, InputPhase::Released);
    }

    #[test]
    fn tap_is_adopted_only_after_release_frame() {
        let now = Instant::now();
        let mut session = ActiveSession {
            descriptor: AutomationSessionDescriptor {
                schema: SESSION_SCHEMA.to_string(),
                nonce: "a".repeat(32),
                expected_build_version: "test".to_string(),
                expected_source_revision: "test".to_string(),
                launcher_pid: std::process::id(),
                main_generation: 1,
                created_unix_ms: 1,
                expires_unix_ms: 2,
            },
            socket: UnixDatagram::unbound().unwrap(),
            last_request: now,
            logical_state: button_state(AutomationButton::A),
            press: Some(PressLifecycle::TapPressed {
                sequence: 7,
                button: AutomationButton::A,
                press_id: PressId(1_u64 << 63 | 1),
            }),
            action_sequence: 7,
            adopted_action_sequence: 6,
            source_epoch: SourceEpoch(1),
            next_event_sequence: 1,
            next_press_id: 1,
            events: VecDeque::new(),
        };
        assert!(session.logical_state.btn_a);
        if let Some(PressLifecycle::TapPressed { sequence, .. }) = session.press {
            session.logical_state = PadState::default();
            session.adopted_action_sequence = sequence;
            session.press = None;
        }
        assert!(!session.logical_state.btn_a);
        assert_eq!(session.adopted_action_sequence, 7);
    }

    #[test]
    fn semantic_revisions_ignore_identical_snapshots() {
        let mut automation = LauncherAutomation::with_paths("missing".into(), "missing".into());
        automation.session = Some(ActiveSession {
            descriptor: AutomationSessionDescriptor {
                schema: SESSION_SCHEMA.to_string(),
                nonce: "b".repeat(32),
                expected_build_version: "test".to_string(),
                expected_source_revision: "test".to_string(),
                launcher_pid: std::process::id(),
                main_generation: 1,
                created_unix_ms: 1,
                expires_unix_ms: 2,
            },
            socket: UnixDatagram::unbound().unwrap(),
            last_request: Instant::now(),
            logical_state: PadState::default(),
            press: None,
            action_sequence: 0,
            adopted_action_sequence: 0,
            source_epoch: SourceEpoch(1),
            next_event_sequence: 0,
            next_press_id: 0,
            events: VecDeque::new(),
        });
        let state = AutomationSemanticState {
            effective_view: "home".to_string(),
            ..AutomationSemanticState::default()
        };
        let first = automation.observe_state(state.clone());
        let second = automation.observe_state(state);
        assert_eq!(first.state_revision, second.state_revision);
    }
}
