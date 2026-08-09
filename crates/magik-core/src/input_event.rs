// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ordered, host-neutral input events shared by capture and UI routing.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum LogicalAction {
    Up,
    Down,
    Left,
    Right,
    Activate,
    Back,
    Home,
    X,
    Y,
    L,
    R,
    Select,
    Start,
}

impl LogicalAction {
    pub const ALL: [Self; 13] = [
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
        Self::Activate,
        Self::Back,
        Self::Home,
        Self::X,
        Self::Y,
        Self::L,
        Self::R,
        Self::Select,
        Self::Start,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputPhase {
    Pressed,
    Released,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PressId(pub u64);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SourceEpoch(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InputSourceKind {
    MainProxy,
    RawDevice,
    SetupKeyboard,
    Automation,
    Preview,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InputSourceId {
    pub kind: InputSourceKind,
    pub instance: u64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DeviceInstanceId {
    pub plug_id: String,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RawControl {
    Button(u16),
    Axis(u16),
    Key(u16),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawControlEvent {
    pub device: DeviceInstanceId,
    pub captured_at_us: u64,
    pub control: RawControl,
    pub value: i32,
    pub phase: InputPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputEvent {
    pub source: InputSourceId,
    pub source_epoch: SourceEpoch,
    pub sequence: u64,
    pub press_id: PressId,
    pub captured_at_us: u64,
    pub action: LogicalAction,
    pub phase: InputPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingInputEvent {
    pub source: InputSourceId,
    pub source_epoch: SourceEpoch,
    pub press_id: PressId,
    pub captured_at_us: u64,
    pub action: LogicalAction,
    pub phase: InputPhase,
}

impl PendingInputEvent {
    #[must_use]
    pub const fn with_sequence(self, sequence: u64) -> InputEvent {
        InputEvent {
            source: self.source,
            source_epoch: self.source_epoch,
            sequence,
            press_id: self.press_id,
            captured_at_us: self.captured_at_us,
            action: self.action,
            phase: self.phase,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HeldState {
    actions: [bool; LogicalAction::ALL.len()],
}

impl HeldState {
    #[must_use]
    pub const fn is_held(self, action: LogicalAction) -> bool {
        self.actions[action.index()]
    }

    pub fn apply_event(&mut self, event: &InputEvent) -> Result<(), InputReductionError> {
        let held = &mut self.actions[event.action.index()];
        match event.phase {
            InputPhase::Pressed if *held => Err(InputReductionError::DuplicatePress {
                source: event.source,
                action: event.action,
            }),
            InputPhase::Released if !*held => Err(InputReductionError::UnmatchedRelease {
                source: event.source,
                action: event.action,
            }),
            InputPhase::Pressed => {
                *held = true;
                Ok(())
            }
            InputPhase::Released => {
                *held = false;
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputProtocolHealth {
    ProxyV2,
    Unhealthy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputHealth {
    pub protocol: InputProtocolHealth,
    pub queue_depth: usize,
    pub queue_high_water: usize,
    pub overflow_count: u64,
    pub desync_count: u64,
    pub proxy_generation: u64,
}

impl Default for InputHealth {
    fn default() -> Self {
        Self {
            protocol: InputProtocolHealth::Unhealthy,
            queue_depth: 0,
            queue_high_water: 0,
            overflow_count: 0,
            desync_count: 0,
            proxy_generation: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InputTopology {
    pub devices: Vec<DeviceInstanceId>,
    pub revision: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputBatch {
    pub source_epoch: SourceEpoch,
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub events: Vec<InputEvent>,
    pub raw_events: Vec<RawControlEvent>,
    pub held_after_last: HeldState,
    pub topology: InputTopology,
    pub activity_generation: u64,
    pub health: InputHealth,
}

impl InputBatch {
    pub fn validate_from(&self, mut held: HeldState) -> Result<HeldState, InputReductionError> {
        let expected = self.events.first().map(|event| event.sequence);
        if expected != self.first_sequence
            || self.events.last().map(|event| event.sequence) != self.last_sequence
        {
            return Err(InputReductionError::InvalidWatermark);
        }
        for (offset, event) in self.events.iter().enumerate() {
            let expected_sequence = self
                .first_sequence
                .unwrap_or(0)
                .saturating_add(offset as u64);
            if event.sequence != expected_sequence {
                return Err(InputReductionError::SequenceGap {
                    expected: expected_sequence,
                    actual: event.sequence,
                });
            }
            held.apply_event(event)?;
        }
        if held != self.held_after_last {
            return Err(InputReductionError::HeldStateMismatch);
        }
        Ok(held)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputReductionError {
    DuplicatePress {
        source: InputSourceId,
        action: LogicalAction,
    },
    UnmatchedRelease {
        source: InputSourceId,
        action: LogicalAction,
    },
    SequenceGap {
        expected: u64,
        actual: u64,
    },
    InvalidWatermark,
    HeldStateMismatch,
}

#[derive(Debug, Default)]
pub struct LogicalEventReducer {
    active_sources: HashMap<(InputSourceId, LogicalAction), SourcePress>,
    active_actions: HashMap<LogicalAction, ActionPress>,
    next_press_id: u64,
}

#[derive(Clone, Copy, Debug)]
struct SourcePress {
    epoch: SourceEpoch,
}

#[derive(Clone, Copy, Debug)]
struct ActionPress {
    press_id: PressId,
    source: InputSourceId,
    source_epoch: SourceEpoch,
}

impl LogicalEventReducer {
    pub fn transition(
        &mut self,
        source: InputSourceId,
        source_epoch: SourceEpoch,
        action: LogicalAction,
        phase: InputPhase,
        captured_at_us: u64,
    ) -> Result<Option<PendingInputEvent>, InputReductionError> {
        let key = (source, action);
        match phase {
            InputPhase::Pressed => {
                if self.active_sources.contains_key(&key) {
                    return Err(InputReductionError::DuplicatePress { source, action });
                }
                self.active_sources.insert(
                    key,
                    SourcePress {
                        epoch: source_epoch,
                    },
                );
                if self.active_actions.contains_key(&action) {
                    return Ok(None);
                }
                self.next_press_id = self.next_press_id.saturating_add(1).max(1);
                let action_press = ActionPress {
                    press_id: PressId(self.next_press_id),
                    source,
                    source_epoch,
                };
                self.active_actions.insert(action, action_press);
                Ok(Some(PendingInputEvent {
                    source,
                    source_epoch,
                    press_id: action_press.press_id,
                    captured_at_us,
                    action,
                    phase,
                }))
            }
            InputPhase::Released => {
                let Some(source_press) = self.active_sources.remove(&key) else {
                    return Err(InputReductionError::UnmatchedRelease { source, action });
                };
                if source_press.epoch != source_epoch {
                    return Err(InputReductionError::UnmatchedRelease { source, action });
                }
                if self
                    .active_sources
                    .keys()
                    .any(|(_, active_action)| *active_action == action)
                {
                    return Ok(None);
                }
                let action_press = self
                    .active_actions
                    .remove(&action)
                    .ok_or(InputReductionError::UnmatchedRelease { source, action })?;
                Ok(Some(PendingInputEvent {
                    source: action_press.source,
                    source_epoch: action_press.source_epoch,
                    press_id: action_press.press_id,
                    captured_at_us,
                    action,
                    phase,
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE_1: InputSourceId = InputSourceId {
        kind: InputSourceKind::MainProxy,
        instance: 1,
    };
    const SOURCE_2: InputSourceId = InputSourceId {
        kind: InputSourceKind::Preview,
        instance: 2,
    };
    const EPOCH: SourceEpoch = SourceEpoch(7);

    #[test]
    fn same_drain_press_and_release_remain_two_events() {
        let mut reducer = LogicalEventReducer::default();
        let pressed = reducer
            .transition(
                SOURCE_1,
                EPOCH,
                LogicalAction::Down,
                InputPhase::Pressed,
                10,
            )
            .unwrap()
            .unwrap()
            .with_sequence(1);
        let released = reducer
            .transition(
                SOURCE_1,
                EPOCH,
                LogicalAction::Down,
                InputPhase::Released,
                11,
            )
            .unwrap()
            .unwrap()
            .with_sequence(2);

        assert_eq!(pressed.phase, InputPhase::Pressed);
        assert_eq!(released.phase, InputPhase::Released);
        assert_eq!(pressed.press_id, released.press_id);
    }

    #[test]
    fn rapid_taps_keep_distinct_press_ids() {
        let mut reducer = LogicalEventReducer::default();
        let mut press_ids = Vec::new();
        for timestamp in 0..10 {
            let pressed = reducer
                .transition(
                    SOURCE_1,
                    EPOCH,
                    LogicalAction::Activate,
                    InputPhase::Pressed,
                    timestamp * 2,
                )
                .unwrap()
                .unwrap();
            press_ids.push(pressed.press_id);
            reducer
                .transition(
                    SOURCE_1,
                    EPOCH,
                    LogicalAction::Activate,
                    InputPhase::Released,
                    timestamp * 2 + 1,
                )
                .unwrap()
                .unwrap();
        }
        press_ids.dedup();
        assert_eq!(press_ids.len(), 10);
    }

    #[test]
    fn duplicate_phases_are_rejected() {
        let mut reducer = LogicalEventReducer::default();
        reducer
            .transition(
                SOURCE_1,
                EPOCH,
                LogicalAction::Activate,
                InputPhase::Pressed,
                1,
            )
            .unwrap();
        assert!(matches!(
            reducer.transition(
                SOURCE_1,
                EPOCH,
                LogicalAction::Activate,
                InputPhase::Pressed,
                2,
            ),
            Err(InputReductionError::DuplicatePress { .. })
        ));
    }

    #[test]
    fn merged_action_releases_only_after_every_source() {
        let mut reducer = LogicalEventReducer::default();
        assert!(
            reducer
                .transition(SOURCE_1, EPOCH, LogicalAction::Down, InputPhase::Pressed, 1,)
                .unwrap()
                .is_some()
        );
        assert!(
            reducer
                .transition(SOURCE_2, EPOCH, LogicalAction::Down, InputPhase::Pressed, 2,)
                .unwrap()
                .is_none()
        );
        assert!(
            reducer
                .transition(
                    SOURCE_1,
                    EPOCH,
                    LogicalAction::Down,
                    InputPhase::Released,
                    3,
                )
                .unwrap()
                .is_none()
        );
        assert!(
            reducer
                .transition(
                    SOURCE_2,
                    EPOCH,
                    LogicalAction::Down,
                    InputPhase::Released,
                    4,
                )
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn batch_watermark_and_held_state_are_consistent() {
        let pressed = PendingInputEvent {
            source: SOURCE_1,
            source_epoch: EPOCH,
            press_id: PressId(1),
            captured_at_us: 1,
            action: LogicalAction::Right,
            phase: InputPhase::Pressed,
        }
        .with_sequence(41);
        let mut held = HeldState::default();
        held.apply_event(&pressed).unwrap();
        let batch = InputBatch {
            source_epoch: EPOCH,
            first_sequence: Some(41),
            last_sequence: Some(41),
            events: vec![pressed],
            held_after_last: held,
            ..InputBatch::default()
        };

        assert_eq!(batch.validate_from(HeldState::default()), Ok(held));
        assert!(held.is_held(LogicalAction::Right));
    }
}
