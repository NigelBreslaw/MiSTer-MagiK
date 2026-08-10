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
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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
    health: InputHealth,
    wake_generation: u64,
}

impl MailboxState {
    fn note_change(&mut self) {
        self.wake_generation = self.wake_generation.wrapping_add(1);
    }

    fn publish(&mut self, pending: crate::input_event::PendingInputEvent) -> bool {
        if self.events.len() >= JOURNAL_CAPACITY {
            self.health.overflow_count = self.health.overflow_count.saturating_add(1);
            self.health.desync_count = self.health.desync_count.saturating_add(1);
            self.health.protocol = InputProtocolHealth::Unhealthy;
            self.events.clear();
            self.health.queue_depth = 0;
            self.held = HeldState::default();
            self.source_epoch.0 = self.source_epoch.0.saturating_add(1);
            self.note_change();
            return false;
        }
        self.next_sequence = self.next_sequence.saturating_add(1).max(1);
        let event = pending.with_sequence(self.next_sequence);
        if self.held.apply_event(&event).is_err() {
            self.health.desync_count = self.health.desync_count.saturating_add(1);
            self.health.protocol = InputProtocolHealth::Unhealthy;
            self.note_change();
            return false;
        }
        self.events.push_back(event);
        self.health.queue_depth = self.events.len();
        self.health.queue_high_water = self.health.queue_high_water.max(self.events.len());
        self.note_change();
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
        self.note_change();
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
        self.note_change();
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InputObservation(u64);

impl InputObservation {
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.0
    }
}

#[derive(Clone)]
pub struct InputObservationProbe {
    mailbox: Arc<InputMailbox>,
}

impl InputObservationProbe {
    #[must_use]
    pub fn observe(&self) -> InputObservation {
        let state = self
            .mailbox
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        InputObservation(state.wake_generation)
    }
}

#[derive(Debug, Default)]
pub struct DrainedInput {
    pub batch: InputBatch,
    pub observation: InputObservation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputWaitOutcome {
    Changed,
    TimedOut,
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
            let mut state = mailbox
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.health.protocol = InputProtocolHealth::Unhealthy;
            state.note_change();
        }
        Self { mailbox, join }
    }

    pub fn drain(&self) -> DrainedInput {
        let mut state = self
            .mailbox
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let events: Vec<_> = state.events.drain(..).collect();
        state.health.queue_depth = 0;
        DrainedInput {
            batch: InputBatch {
                source_epoch: state.source_epoch,
                first_sequence: events.first().map(|event| event.sequence),
                last_sequence: events.last().map(|event| event.sequence),
                events,
                held_after_last: state.held,
                topology: state.topology.clone(),
                health: state.health.clone(),
                ..InputBatch::default()
            },
            observation: InputObservation(state.wake_generation),
        }
    }

    #[must_use]
    pub fn observation_probe(&self) -> InputObservationProbe {
        InputObservationProbe {
            mailbox: Arc::clone(&self.mailbox),
        }
    }

    pub fn wait_for_change(&self, after: InputObservation, timeout: Duration) -> InputWaitOutcome {
        let state = self
            .mailbox
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.wake_generation != after.0 {
            return InputWaitOutcome::Changed;
        }
        let (state, wait) = self
            .mailbox
            .wake
            .wait_timeout_while(state, timeout, |state| state.wake_generation == after.0)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.wake_generation != after.0 {
            InputWaitOutcome::Changed
        } else {
            debug_assert!(wait.timed_out());
            InputWaitOutcome::TimedOut
        }
    }

    fn signal_shutdown(&self) {
        self.mailbox.shutdown.store(true, Ordering::Release);
        self.mailbox
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .note_change();
        self.mailbox.wake.notify_all();
    }
}

impl Drop for InputHub {
    fn drop(&mut self) {
        self.signal_shutdown();
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

pub(crate) fn monotonic_us() -> u64 {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) };
    if result != 0 {
        return 0;
    }
    (value.tv_sec as u64)
        .saturating_mul(1_000_000)
        .saturating_add((value.tv_nsec as u64) / 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::time::Instant;

    fn test_hub() -> InputHub {
        InputHub {
            mailbox: Arc::new(InputMailbox {
                state: Mutex::new(MailboxState::default()),
                wake: Condvar::new(),
                shutdown: AtomicBool::new(false),
            }),
            join: None,
        }
    }

    fn change_mailbox(hub: &InputHub) {
        hub.mailbox
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .note_change();
        hub.mailbox.wake.notify_all();
    }

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

    #[test]
    fn change_between_drain_and_wait_is_observed() {
        let hub = test_hub();
        let drained = hub.drain();
        change_mailbox(&hub);

        assert_eq!(
            hub.wait_for_change(drained.observation, Duration::from_secs(1)),
            InputWaitOutcome::Changed
        );
    }

    #[test]
    fn change_during_wait_wakes_the_waiter() {
        let hub = Arc::new(test_hub());
        let observation = hub.drain().observation;
        let ready = Arc::new(Barrier::new(2));
        let waiter_hub = Arc::clone(&hub);
        let waiter_ready = Arc::clone(&ready);
        let waiter = thread::spawn(move || {
            waiter_ready.wait();
            waiter_hub.wait_for_change(observation, Duration::from_secs(1))
        });
        ready.wait();
        change_mailbox(&hub);

        assert_eq!(waiter.join().unwrap(), InputWaitOutcome::Changed);
    }

    #[test]
    fn unchanged_mailbox_times_out_after_spurious_notification() {
        let hub = Arc::new(test_hub());
        let observation = hub.drain().observation;
        let notifier_hub = Arc::clone(&hub);
        let notifier = thread::spawn(move || {
            thread::yield_now();
            notifier_hub.mailbox.wake.notify_all();
        });
        let started = Instant::now();

        assert_eq!(
            hub.wait_for_change(observation, Duration::from_millis(10)),
            InputWaitOutcome::TimedOut
        );
        assert!(started.elapsed() >= Duration::from_millis(5));
        notifier.join().unwrap();
    }

    #[test]
    fn topology_fault_overflow_and_shutdown_each_change_observation() {
        let hub = test_hub();
        let first = hub.drain().observation;
        hub.mailbox
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mark_proxy_open(1, "/dev/input/event-test");
        let second = hub.drain().observation;
        assert_ne!(first, second);

        hub.mailbox
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mark_proxy_lost();
        let third = hub.drain().observation;
        assert_ne!(second, third);

        {
            let mut state = hub
                .mailbox
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.events.resize(
                JOURNAL_CAPACITY,
                InputEvent {
                    source: InputSourceId {
                        kind: InputSourceKind::Preview,
                        instance: 1,
                    },
                    source_epoch: SourceEpoch(1),
                    sequence: 1,
                    press_id: crate::input_event::PressId(1),
                    captured_at_us: 1,
                    action: LogicalAction::Activate,
                    phase: InputPhase::Pressed,
                },
            );
            state.publish(crate::input_event::PendingInputEvent {
                source: InputSourceId {
                    kind: InputSourceKind::Preview,
                    instance: 1,
                },
                source_epoch: SourceEpoch(1),
                press_id: crate::input_event::PressId(2),
                captured_at_us: 2,
                action: LogicalAction::Activate,
                phase: InputPhase::Pressed,
            });
        }
        let fourth = hub.drain().observation;
        assert_ne!(third, fourth);

        hub.signal_shutdown();
        assert_eq!(
            hub.wait_for_change(fourth, Duration::from_secs(1)),
            InputWaitOutcome::Changed
        );
    }
}
