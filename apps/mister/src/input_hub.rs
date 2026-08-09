// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lossless Main-proxy capture and an atomic UI input mailbox.

use crate::input_event::{
    DeviceInstanceId, HeldState, InputBatch, InputEvent, InputHealth, InputPhase,
    InputProtocolHealth, InputSourceId, InputSourceKind, InputTopology, LogicalAction,
    LogicalEventReducer, SourceEpoch,
};
use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const INPUT_PROXY_CAPABILITY_ENV: &str = "MISTER_MAGIK_INPUT_PROXY";
const INPUT_PROXY_PROTOCOL_ENV: &str = "MISTER_MAGIK_INPUT_PROXY_PROTOCOL";
const INPUT_PROXY_NAME: &str = "MiSTer virtual input";
const INPUT_EVENT_SIZE: usize = if cfg!(target_pointer_width = "64") {
    24
} else {
    16
};
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const SYN_DROPPED: u16 = 3;
const JOURNAL_CAPACITY: usize = 1024;
const DISCOVERY_INTERVAL: Duration = Duration::from_millis(250);

const KEY_ESC: u16 = 1;
const KEY_TAB: u16 = 15;
const KEY_ENTER: u16 = 28;
const KEY_SPACE: u16 = 57;
const KEY_F9: u16 = 67;
const KEY_F10: u16 = 68;
const KEY_UP: u16 = 103;
const KEY_PAGEUP: u16 = 104;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;
const KEY_DOWN: u16 = 108;
const KEY_PAGEDOWN: u16 = 109;
const KEY_MENU: u16 = 139;

struct InputMailbox {
    state: Mutex<MailboxState>,
    wake: Condvar,
    shutdown: AtomicBool,
}

#[derive(Default)]
struct MailboxState {
    source_epoch: SourceEpoch,
    next_sequence: u64,
    events: VecDeque<InputEvent>,
    held: HeldState,
    topology: InputTopology,
    activity_generation: u64,
    health: InputHealth,
    wake_generation: u64,
}

impl MailboxState {
    fn publish(&mut self, pending: crate::input_event::PendingInputEvent) -> bool {
        if self.events.len() >= JOURNAL_CAPACITY {
            self.health.overflow_count = self.health.overflow_count.saturating_add(1);
            self.health.desync_count = self.health.desync_count.saturating_add(1);
            self.health.protocol = InputProtocolHealth::Unhealthy;
            self.events.clear();
            self.health.queue_depth = 0;
            self.held = HeldState::default();
            self.source_epoch.0 = self.source_epoch.0.saturating_add(1);
            self.wake_generation = self.wake_generation.saturating_add(1);
            return false;
        }
        self.next_sequence = self.next_sequence.saturating_add(1).max(1);
        let event = pending.with_sequence(self.next_sequence);
        if self.held.apply_event(&event).is_err() {
            self.health.desync_count = self.health.desync_count.saturating_add(1);
            self.health.protocol = InputProtocolHealth::Unhealthy;
            return false;
        }
        self.events.push_back(event);
        self.health.queue_depth = self.events.len();
        self.health.queue_high_water = self.health.queue_high_water.max(self.events.len());
        self.activity_generation = self.activity_generation.saturating_add(1);
        self.wake_generation = self.wake_generation.saturating_add(1);
        true
    }

    fn mark_proxy_open(&mut self, generation: u64, path: &str) -> SourceEpoch {
        self.source_epoch.0 = self.source_epoch.0.saturating_add(1).max(1);
        self.health.protocol = InputProtocolHealth::ProxyV2;
        self.health.proxy_generation = generation;
        self.topology.devices = vec![DeviceInstanceId {
            plug_id: path.to_string(),
            generation,
        }];
        self.topology.revision = self.topology.revision.saturating_add(1);
        self.wake_generation = self.wake_generation.saturating_add(1);
        self.source_epoch
    }

    fn mark_proxy_lost(&mut self) {
        self.health.protocol = InputProtocolHealth::Unhealthy;
        self.health.desync_count = self.health.desync_count.saturating_add(1);
        self.events.clear();
        self.health.queue_depth = 0;
        self.held = HeldState::default();
        self.topology.devices.clear();
        self.source_epoch.0 = self.source_epoch.0.saturating_add(1).max(1);
        self.topology.revision = self.topology.revision.saturating_add(1);
        self.wake_generation = self.wake_generation.saturating_add(1);
    }
}

pub struct InputHub {
    mailbox: Arc<InputMailbox>,
    join: Option<JoinHandle<()>>,
}

impl InputHub {
    pub fn start() -> Self {
        let mailbox = Arc::new(InputMailbox {
            state: Mutex::new(MailboxState::default()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let thread_mailbox = Arc::clone(&mailbox);
        let join = thread::Builder::new()
            .name("input-reader".to_string())
            .spawn(move || capture_loop(thread_mailbox))
            .ok();
        if join.is_none() {
            mailbox
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .health
                .protocol = InputProtocolHealth::Unhealthy;
        }
        Self { mailbox, join }
    }

    pub fn drain(&self) -> InputBatch {
        let mut state = self
            .mailbox
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let events: Vec<_> = state.events.drain(..).collect();
        state.health.queue_depth = 0;
        InputBatch {
            source_epoch: state.source_epoch,
            first_sequence: events.first().map(|event| event.sequence),
            last_sequence: events.last().map(|event| event.sequence),
            events,
            held_after_last: state.held,
            topology: state.topology.clone(),
            activity_generation: state.activity_generation,
            health: state.health.clone(),
            ..InputBatch::default()
        }
    }

    pub fn wait_for_input(&self, timeout: Duration) {
        let state = self
            .mailbox
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let generation = state.wake_generation;
        let _ = self
            .mailbox
            .wake
            .wait_timeout_while(state, timeout, |state| state.wake_generation == generation);
    }
}

impl Drop for InputHub {
    fn drop(&mut self) {
        self.mailbox.shutdown.store(true, Ordering::Release);
        self.mailbox.wake.notify_all();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct ProxyReader {
    file: File,
    path: String,
    source: InputSourceId,
    epoch: SourceEpoch,
    reducer: LogicalEventReducer,
}

fn capture_loop(mailbox: Arc<InputMailbox>) {
    apply_runtime_thread_policy(RuntimeThreadRole::InputReader);
    let protocol_enabled = std::env::var(INPUT_PROXY_CAPABILITY_ENV).as_deref() == Ok("1")
        && std::env::var(INPUT_PROXY_PROTOCOL_ENV).as_deref() == Ok("2");
    if !protocol_enabled {
        mailbox.wake.notify_all();
        while !mailbox.shutdown.load(Ordering::Acquire) {
            thread::sleep(DISCOVERY_INTERVAL);
        }
        return;
    }

    let mut reader: Option<ProxyReader> = None;
    let mut generation = 0_u64;
    while !mailbox.shutdown.load(Ordering::Acquire) {
        if reader.is_none()
            && let Some(path) = discover_main_proxy()
        {
            match open_proxy(&path) {
                Ok(file) => {
                    generation = generation.saturating_add(1).max(1);
                    let epoch = {
                        let mut state = mailbox
                            .state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        state.mark_proxy_open(generation, &path)
                    };
                    mailbox.wake.notify_all();
                    reader = Some(ProxyReader {
                        file,
                        path,
                        source: InputSourceId {
                            kind: InputSourceKind::MainProxy,
                            instance: generation,
                        },
                        epoch,
                        reducer: LogicalEventReducer::default(),
                    });
                }
                Err(_) => thread::sleep(DISCOVERY_INTERVAL),
            }
        }

        let Some(active) = reader.as_mut() else {
            thread::sleep(DISCOVERY_INTERVAL);
            continue;
        };
        let mut pollfd = libc::pollfd {
            fd: active.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe {
            libc::poll(
                &mut pollfd,
                1,
                DISCOVERY_INTERVAL.as_millis().min(i32::MAX as u128) as i32,
            )
        };
        if ready < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if ready < 0 || pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            lose_proxy(&mailbox);
            reader = None;
            continue;
        }
        if ready > 0 && pollfd.revents & libc::POLLIN != 0 {
            match drain_proxy(active, &mailbox) {
                Ok(()) => {}
                Err(_) => {
                    lose_proxy(&mailbox);
                    reader = None;
                }
            }
        } else if !Path::new(&active.path).exists() {
            lose_proxy(&mailbox);
            reader = None;
        }
    }
}

fn lose_proxy(mailbox: &InputMailbox) {
    mailbox
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .mark_proxy_lost();
    mailbox.wake.notify_all();
}

fn drain_proxy(reader: &mut ProxyReader, mailbox: &InputMailbox) -> io::Result<()> {
    let mut bytes = [0_u8; INPUT_EVENT_SIZE];
    loop {
        match reader.file.read_exact(&mut bytes) {
            Ok(()) => {
                let (event_type, code, value) = parse_input_event(&bytes);
                if event_type == EV_SYN && code == SYN_DROPPED {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "input desync"));
                }
                if event_type != EV_KEY {
                    continue;
                }
                {
                    let mut state = mailbox
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.activity_generation = state.activity_generation.saturating_add(1);
                    state.wake_generation = state.wake_generation.saturating_add(1);
                }
                mailbox.wake.notify_all();
                if value == 2 {
                    continue;
                }
                let Some(action) = logical_action_for_key(code) else {
                    continue;
                };
                let phase = if value == 0 {
                    InputPhase::Released
                } else if value == 1 {
                    InputPhase::Pressed
                } else {
                    continue;
                };
                match reader.reducer.transition(
                    reader.source,
                    reader.epoch,
                    action,
                    phase,
                    monotonic_us(),
                ) {
                    Ok(Some(pending)) => {
                        let published = mailbox
                            .state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .publish(pending);
                        mailbox.wake.notify_all();
                        if !published {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "input journal overflow",
                            ));
                        }
                    }
                    Ok(None) => {}
                    Err(crate::input_event::InputReductionError::UnmatchedRelease { .. }) => {}
                    Err(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid input phase",
                        ));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

fn logical_action_for_key(code: u16) -> Option<LogicalAction> {
    match code {
        KEY_UP => Some(LogicalAction::Up),
        KEY_DOWN => Some(LogicalAction::Down),
        KEY_LEFT => Some(LogicalAction::Left),
        KEY_RIGHT => Some(LogicalAction::Right),
        KEY_ENTER => Some(LogicalAction::Activate),
        KEY_ESC => Some(LogicalAction::Back),
        KEY_MENU => Some(LogicalAction::Home),
        KEY_TAB => Some(LogicalAction::X),
        KEY_SPACE => Some(LogicalAction::Y),
        KEY_PAGEUP => Some(LogicalAction::L),
        KEY_PAGEDOWN => Some(LogicalAction::R),
        KEY_F10 => Some(LogicalAction::Select),
        KEY_F9 => Some(LogicalAction::Start),
        _ => None,
    }
}

fn open_proxy(path: &str) -> io::Result<File> {
    let file = OpenOptions::new().read(true).open(path)?;
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

fn discover_main_proxy() -> Option<String> {
    let entries = std::fs::read_dir("/sys/class/input").ok()?;
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("event") || !name[5..].chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let Ok(device_name) = std::fs::read_to_string(entry.path().join("device/name")) else {
            continue;
        };
        if device_name.trim() == INPUT_PROXY_NAME {
            paths.push(format!("/dev/input/{name}"));
        }
    }
    paths.sort();
    paths.into_iter().next()
}

fn parse_input_event(bytes: &[u8; INPUT_EVENT_SIZE]) -> (u16, u16, i32) {
    let offset = INPUT_EVENT_SIZE - 8;
    (
        u16::from_ne_bytes([bytes[offset], bytes[offset + 1]]),
        u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]]),
        i32::from_ne_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]),
    )
}

fn monotonic_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START
        .get_or_init(Instant::now)
        .elapsed()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_keys_map_to_logical_actions() {
        assert_eq!(logical_action_for_key(KEY_DOWN), Some(LogicalAction::Down));
        assert_eq!(
            logical_action_for_key(KEY_ENTER),
            Some(LogicalAction::Activate)
        );
        assert_eq!(logical_action_for_key(KEY_MENU), Some(LogicalAction::Home));
    }

    #[test]
    fn mailbox_keeps_press_and_release_in_one_batch() {
        let source = InputSourceId {
            kind: InputSourceKind::MainProxy,
            instance: 1,
        };
        let epoch = SourceEpoch(1);
        let mut reducer = LogicalEventReducer::default();
        let mut state = MailboxState {
            source_epoch: epoch,
            ..MailboxState::default()
        };
        for phase in [InputPhase::Pressed, InputPhase::Released] {
            state.publish(
                reducer
                    .transition(source, epoch, LogicalAction::Down, phase, 1)
                    .unwrap()
                    .unwrap(),
            );
        }
        assert_eq!(state.events.len(), 2);
        assert!(!state.held.is_held(LogicalAction::Down));
    }

    #[test]
    fn critical_journal_overflow_becomes_unhealthy() {
        let source = InputSourceId {
            kind: InputSourceKind::Preview,
            instance: 1,
        };
        let epoch = SourceEpoch(1);
        let mut state = MailboxState {
            source_epoch: epoch,
            ..MailboxState::default()
        };
        for sequence in 0..JOURNAL_CAPACITY {
            let action = if sequence & 1 == 0 {
                LogicalAction::Down
            } else {
                LogicalAction::Up
            };
            state.events.push_back(InputEvent {
                source,
                source_epoch: epoch,
                sequence: sequence as u64 + 1,
                press_id: crate::input_event::PressId(sequence as u64 + 1),
                captured_at_us: sequence as u64,
                action,
                phase: InputPhase::Pressed,
            });
        }
        state.publish(crate::input_event::PendingInputEvent {
            source,
            source_epoch: epoch,
            press_id: crate::input_event::PressId(2000),
            captured_at_us: 2000,
            action: LogicalAction::Activate,
            phase: InputPhase::Pressed,
        });
        assert_eq!(state.health.protocol, InputProtocolHealth::Unhealthy);
        assert_eq!(state.health.overflow_count, 1);
        assert!(state.events.is_empty());
    }
}
