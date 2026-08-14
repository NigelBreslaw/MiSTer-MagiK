// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Central launcher focus, capture, transition, and repeat policy.

use crate::input_event::{
    HeldState, InputBatch, InputEvent, InputPhase, InputProtocolHealth, InputSourceId,
    LogicalAction, PressId, SourceEpoch,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const MENU_REPEAT_DELAY: Duration = Duration::from_millis(300);
const MENU_REPEAT_INTERVAL: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InputContextKind {
    Disabled,
    Screensaver,
    LifecycleDialog,
    ControllerSetup,
    LauncherModal,
    Transition,
    Diagnostic,
    Screen,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FocusTarget {
    pub kind: InputContextKind,
    /// Identity of the concrete screen, dialog, or transition instance.
    pub owner: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ContextId {
    pub target: FocusTarget,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectionalPolicy {
    EdgeOnly,
    MenuRepeat,
    HomeContinuous,
    ArcadeContinuous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FocusRequest {
    pub target: FocusTarget,
    pub directional_policy: DirectionalPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchKind {
    Initial,
    Repeat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsumedReason {
    InputDisabled,
    OpposingDirections,
    StaleRelease,
    ExclusiveBatch,
    IntegrityFault,
    LaunchHandoff,
    TransitionActive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputFault {
    ProxyUnavailable,
    Overflow,
    Desync,
    SequenceGap,
    SourceChangedWhileHeld,
    WaitingForNeutral,
}

impl InputFault {
    pub const fn notice(self) -> &'static str {
        match self {
            Self::ProxyUnavailable => {
                "Controller input unavailable — Main input proxy v2 is required"
            }
            Self::Overflow => "Controller input paused after queue overflow — release all controls",
            Self::Desync => "Controller input paused after device desync — release all controls",
            Self::SequenceGap => {
                "Controller input paused after an event sequence gap — release all controls"
            }
            Self::SourceChangedWhileHeld => {
                "Controller input changed while held — release all controls"
            }
            Self::WaitingForNeutral => "Controller input paused — release all controls",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputOutcome {
    Dispatch {
        event: InputEvent,
        context: ContextId,
        kind: DispatchKind,
    },
    WakeScreensaver {
        event: InputEvent,
        context: ContextId,
    },
    Released {
        event: InputEvent,
        press_id: PressId,
        context: ContextId,
    },
    Consumed {
        press_id: PressId,
        reason: ConsumedReason,
    },
}

#[derive(Clone, Copy, Debug)]
struct Capture {
    context: ContextId,
    action: LogicalAction,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct CaptureKey {
    source: InputSourceId,
    source_epoch: SourceEpoch,
    press_id: PressId,
}

impl From<InputEvent> for CaptureKey {
    fn from(event: InputEvent) -> Self {
        Self {
            source: event.source,
            source_epoch: event.source_epoch,
            press_id: event.press_id,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RepeatState {
    event: InputEvent,
    context: ContextId,
    next_at: Instant,
}

#[derive(Debug)]
pub struct InputRouter {
    context: ContextId,
    policy: DirectionalPolicy,
    captures: HashMap<CaptureKey, Capture>,
    repeats: HashMap<LogicalAction, RepeatState>,
    horizontal_neutral_lock: bool,
    vertical_neutral_lock: bool,
    last_flush_reason: Option<ConsumedReason>,
    source_epoch: Option<SourceEpoch>,
    last_sequence: Option<u64>,
    validated_held: HeldState,
    overflow_count: u64,
    desync_count: u64,
    requires_neutral: bool,
}

impl InputRouter {
    pub fn new(initial: FocusRequest) -> Self {
        Self {
            context: ContextId {
                target: initial.target,
                generation: 1,
            },
            policy: initial.directional_policy,
            captures: HashMap::new(),
            repeats: HashMap::new(),
            horizontal_neutral_lock: false,
            vertical_neutral_lock: false,
            last_flush_reason: None,
            source_epoch: None,
            last_sequence: None,
            validated_held: HeldState::default(),
            overflow_count: 0,
            desync_count: 0,
            requires_neutral: true,
        }
    }

    /// Validate the hub's atomic watermark before any event reaches application
    /// focus. Fault recovery is explicit and always crosses a neutral barrier.
    pub fn accept_batch(&mut self, batch: &InputBatch) -> Result<(), InputFault> {
        if batch.health.protocol != InputProtocolHealth::ProxyV2 {
            self.integrity_flush();
            return Err(InputFault::ProxyUnavailable);
        }
        if batch.health.overflow_count != self.overflow_count {
            self.overflow_count = batch.health.overflow_count;
            self.integrity_flush();
            return Err(InputFault::Overflow);
        }
        if batch.health.desync_count != self.desync_count {
            self.desync_count = batch.health.desync_count;
            self.integrity_flush();
            return Err(InputFault::Desync);
        }
        if self.source_epoch != Some(batch.source_epoch) {
            let source_was_known = self.source_epoch.is_some();
            self.source_epoch = Some(batch.source_epoch);
            self.last_sequence = batch
                .first_sequence
                .map(|sequence| sequence.saturating_sub(1));
            self.integrity_flush();
            if source_was_known || batch.held_after_last != HeldState::default() {
                return Err(InputFault::SourceChangedWhileHeld);
            }
        }
        if let (Some(expected), Some(actual)) = (
            self.last_sequence.map(|value| value.saturating_add(1)),
            batch.first_sequence,
        ) && expected != actual
        {
            self.integrity_flush();
            return Err(InputFault::SequenceGap);
        }
        if batch.validate_from(self.validated_held).is_err() {
            self.integrity_flush();
            return Err(InputFault::Desync);
        }
        self.validated_held = batch.held_after_last;
        self.last_sequence = batch.last_sequence.or(self.last_sequence);
        if self.requires_neutral {
            if batch.held_after_last != HeldState::default() || !batch.events.is_empty() {
                return Err(InputFault::WaitingForNeutral);
            }
            self.requires_neutral = false;
        }
        Ok(())
    }

    fn integrity_flush(&mut self) {
        self.captures.clear();
        self.repeats.clear();
        self.horizontal_neutral_lock = false;
        self.vertical_neutral_lock = false;
        self.validated_held = HeldState::default();
        self.requires_neutral = true;
        self.last_flush_reason = Some(ConsumedReason::IntegrityFault);
    }

    #[must_use]
    pub const fn context(&self) -> ContextId {
        self.context
    }

    pub fn set_focus(&mut self, request: FocusRequest) -> ContextId {
        if self.context.target != request.target {
            self.context = ContextId {
                target: request.target,
                generation: self.context.generation.saturating_add(1),
            };
            self.repeats.clear();
            self.horizontal_neutral_lock = false;
            self.vertical_neutral_lock = false;
        }
        self.policy = request.directional_policy;
        self.context
    }

    pub fn route_event(
        &mut self,
        event: InputEvent,
        request: FocusRequest,
        now: Instant,
    ) -> InputOutcome {
        let context = self.set_focus(request);
        match event.phase {
            InputPhase::Pressed => self.route_pressed(event, context, now),
            InputPhase::Released => self.route_released(event),
        }
    }

    fn route_pressed(
        &mut self,
        event: InputEvent,
        context: ContextId,
        now: Instant,
    ) -> InputOutcome {
        let capture_key = CaptureKey::from(event);
        if self.captures.contains_key(&capture_key) {
            return InputOutcome::Consumed {
                press_id: event.press_id,
                reason: ConsumedReason::IntegrityFault,
            };
        }
        self.captures.insert(
            capture_key,
            Capture {
                context,
                action: event.action,
            },
        );
        if self.opposing_direction_locked(event.action) {
            self.repeats.remove(&event.action);
            if let Some(opposite) = opposite(event.action) {
                self.repeats.remove(&opposite);
            }
            return InputOutcome::Consumed {
                press_id: event.press_id,
                reason: ConsumedReason::OpposingDirections,
            };
        }

        match context.target.kind {
            InputContextKind::Disabled => InputOutcome::Consumed {
                press_id: event.press_id,
                reason: ConsumedReason::InputDisabled,
            },
            InputContextKind::Screensaver => InputOutcome::WakeScreensaver { event, context },
            InputContextKind::Transition => InputOutcome::Consumed {
                press_id: event.press_id,
                reason: ConsumedReason::TransitionActive,
            },
            _ => {
                self.arm_repeat(event, context, now);
                InputOutcome::Dispatch {
                    event,
                    context,
                    kind: DispatchKind::Initial,
                }
            }
        }
    }

    fn route_released(&mut self, event: InputEvent) -> InputOutcome {
        let Some(capture) = self.captures.remove(&CaptureKey::from(event)) else {
            return InputOutcome::Consumed {
                press_id: event.press_id,
                reason: ConsumedReason::StaleRelease,
            };
        };
        if !self.action_held_in_context(event.action, capture.context) {
            self.repeats.remove(&event.action);
        }
        self.update_neutral_locks(capture.context);
        debug_assert_eq!(capture.action, event.action);
        InputOutcome::Released {
            event,
            press_id: event.press_id,
            context: capture.context,
        }
    }

    fn arm_repeat(&mut self, event: InputEvent, context: ContextId, now: Instant) {
        if self.policy == DirectionalPolicy::MenuRepeat && is_direction(event.action) {
            self.repeats.insert(
                event.action,
                RepeatState {
                    event,
                    context,
                    next_at: now + MENU_REPEAT_DELAY,
                },
            );
        }
    }

    pub fn tick_repeat(&mut self, now: Instant) -> Option<InputOutcome> {
        for action in LogicalAction::ALL {
            let Some(repeat) = self.repeats.get_mut(&action) else {
                continue;
            };
            if repeat.context != self.context || now < repeat.next_at {
                continue;
            }
            repeat.next_at = now + MENU_REPEAT_INTERVAL;
            return Some(InputOutcome::Dispatch {
                event: repeat.event,
                context: repeat.context,
                kind: DispatchKind::Repeat,
            });
        }
        None
    }

    #[must_use]
    pub const fn last_flush_reason(&self) -> Option<ConsumedReason> {
        self.last_flush_reason
    }

    pub fn consume_remaining_batch(
        &mut self,
        events: impl IntoIterator<Item = InputEvent>,
        reason: ConsumedReason,
    ) -> Vec<InputOutcome> {
        events
            .into_iter()
            .map(|event| {
                if event.phase == InputPhase::Pressed {
                    self.captures.insert(
                        CaptureKey::from(event),
                        Capture {
                            context: self.context,
                            action: event.action,
                        },
                    );
                } else {
                    let context = self
                        .captures
                        .remove(&CaptureKey::from(event))
                        .map_or(self.context, |capture| capture.context);
                    self.update_neutral_locks(context);
                }
                InputOutcome::Consumed {
                    press_id: event.press_id,
                    reason,
                }
            })
            .collect()
    }

    fn opposing_direction_locked(&mut self, action: LogicalAction) -> bool {
        let Some(opposite) = opposite(action) else {
            return false;
        };
        let pair_held = self.action_held_in_context(opposite, self.context);
        match action {
            LogicalAction::Left | LogicalAction::Right => {
                self.horizontal_neutral_lock |= pair_held;
                self.horizontal_neutral_lock
            }
            LogicalAction::Up | LogicalAction::Down => {
                self.vertical_neutral_lock |= pair_held;
                self.vertical_neutral_lock
            }
            _ => false,
        }
    }

    fn update_neutral_locks(&mut self, context: ContextId) {
        if context != self.context {
            return;
        }
        if !self.action_held_in_context(LogicalAction::Left, context)
            && !self.action_held_in_context(LogicalAction::Right, context)
        {
            self.horizontal_neutral_lock = false;
        }
        if !self.action_held_in_context(LogicalAction::Up, context)
            && !self.action_held_in_context(LogicalAction::Down, context)
        {
            self.vertical_neutral_lock = false;
        }
    }

    fn action_held_in_context(&self, action: LogicalAction, context: ContextId) -> bool {
        self.captures
            .values()
            .any(|capture| capture.context == context && capture.action == action)
    }

    #[must_use]
    pub fn action_held(&self, action: LogicalAction) -> bool {
        if matches!(action, LogicalAction::Left | LogicalAction::Right)
            && self.horizontal_neutral_lock
        {
            return false;
        }
        if matches!(action, LogicalAction::Up | LogicalAction::Down) && self.vertical_neutral_lock {
            return false;
        }
        self.action_held_in_context(action, self.context)
    }
}

fn is_direction(action: LogicalAction) -> bool {
    matches!(
        action,
        LogicalAction::Up | LogicalAction::Down | LogicalAction::Left | LogicalAction::Right
    )
}

fn opposite(action: LogicalAction) -> Option<LogicalAction> {
    match action {
        LogicalAction::Up => Some(LogicalAction::Down),
        LogicalAction::Down => Some(LogicalAction::Up),
        LogicalAction::Left => Some(LogicalAction::Right),
        LogicalAction::Right => Some(LogicalAction::Left),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_event::{InputSourceId, InputSourceKind, SourceEpoch};

    fn request(kind: InputContextKind, owner: u64, policy: DirectionalPolicy) -> FocusRequest {
        FocusRequest {
            target: FocusTarget { kind, owner },
            directional_policy: policy,
        }
    }

    fn event(sequence: u64, action: LogicalAction, phase: InputPhase) -> InputEvent {
        InputEvent {
            source: InputSourceId {
                kind: InputSourceKind::Preview,
                instance: 1,
            },
            source_epoch: SourceEpoch(1),
            sequence,
            press_id: PressId(sequence.div_ceil(2)),
            captured_at_us: sequence,
            action,
            phase,
        }
    }

    fn healthy_batch(events: Vec<InputEvent>, held_after_last: HeldState) -> InputBatch {
        InputBatch {
            source_epoch: SourceEpoch(1),
            first_sequence: events.first().map(|event| event.sequence),
            last_sequence: events.last().map(|event| event.sequence),
            events,
            held_after_last,
            health: crate::input_event::InputHealth {
                protocol: InputProtocolHealth::ProxyV2,
                ..crate::input_event::InputHealth::default()
            },
            ..InputBatch::default()
        }
    }

    #[test]
    fn batch_gate_requires_healthy_v2_and_contiguous_watermarks() {
        let screen = request(InputContextKind::Screen, 1, DirectionalPolicy::MenuRepeat);
        let mut router = InputRouter::new(screen);
        assert_eq!(
            router.accept_batch(&InputBatch::default()),
            Err(InputFault::ProxyUnavailable)
        );
        assert_eq!(
            router.accept_batch(&healthy_batch(Vec::new(), HeldState::default())),
            Ok(())
        );

        let press = event(1, LogicalAction::Down, InputPhase::Pressed);
        let mut held = HeldState::default();
        held.apply_event(&press).unwrap();
        assert_eq!(
            router.accept_batch(&healthy_batch(vec![press], held)),
            Ok(())
        );

        let release = event(3, LogicalAction::Down, InputPhase::Released);
        assert_eq!(
            router.accept_batch(&healthy_batch(vec![release], HeldState::default())),
            Err(InputFault::SequenceGap)
        );
    }

    #[test]
    fn context_change_captures_release_to_original_owner() {
        let screen = request(InputContextKind::Screen, 1, DirectionalPolicy::MenuRepeat);
        let modal = request(
            InputContextKind::LauncherModal,
            2,
            DirectionalPolicy::MenuRepeat,
        );
        let mut router = InputRouter::new(screen);
        let now = Instant::now();
        let pressed = event(1, LogicalAction::Activate, InputPhase::Pressed);
        assert!(matches!(
            router.route_event(pressed, screen, now),
            InputOutcome::Dispatch { .. }
        ));
        router.set_focus(modal);
        let released = event(2, LogicalAction::Activate, InputPhase::Released);
        assert!(matches!(
            router.route_event(released, modal, now),
            InputOutcome::Released { context, .. } if context.target == screen.target
        ));
    }

    #[test]
    fn menu_repeat_starts_at_300ms_and_never_catches_up() {
        let screen = request(InputContextKind::Screen, 1, DirectionalPolicy::MenuRepeat);
        let mut router = InputRouter::new(screen);
        let now = Instant::now();
        router.route_event(
            event(1, LogicalAction::Down, InputPhase::Pressed),
            screen,
            now,
        );
        assert!(
            router
                .tick_repeat(now + Duration::from_millis(299))
                .is_none()
        );
        assert!(
            router
                .tick_repeat(now + Duration::from_millis(300))
                .is_some()
        );
        assert!(router.tick_repeat(now + Duration::from_secs(2)).is_some());
        assert!(
            router
                .tick_repeat(now + Duration::from_secs(2) + Duration::from_millis(79))
                .is_none()
        );
    }

    #[test]
    fn transition_swallows_every_pressed_action() {
        let transition = request(InputContextKind::Transition, 9, DirectionalPolicy::EdgeOnly);
        let now = Instant::now();
        for action in LogicalAction::ALL {
            let mut router = InputRouter::new(transition);
            assert!(matches!(
                router.route_event(event(1, action, InputPhase::Pressed), transition, now),
                InputOutcome::Consumed {
                    reason: ConsumedReason::TransitionActive,
                    ..
                }
            ));
        }
    }

    #[test]
    fn released_transition_tap_never_reaches_the_destination() {
        let transition = request(InputContextKind::Transition, 9, DirectionalPolicy::EdgeOnly);
        let screen = request(InputContextKind::Screen, 2, DirectionalPolicy::MenuRepeat);
        let mut router = InputRouter::new(transition);
        let now = Instant::now();
        router.route_event(
            event(1, LogicalAction::Down, InputPhase::Pressed),
            transition,
            now,
        );
        assert!(matches!(
            router.route_event(
                event(2, LogicalAction::Down, InputPhase::Released),
                transition,
                now,
            ),
            InputOutcome::Released { .. }
        ));
        router.set_focus(screen);
        assert!(!router.action_held(LogicalAction::Down));
        assert!(router.tick_repeat(now + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn held_transition_input_does_not_leak_into_the_destination() {
        let transition = request(InputContextKind::Transition, 9, DirectionalPolicy::EdgeOnly);
        let screen = request(InputContextKind::Screen, 2, DirectionalPolicy::MenuRepeat);
        let now = Instant::now();
        for action in LogicalAction::ALL {
            let mut router = InputRouter::new(transition);
            router.route_event(event(1, action, InputPhase::Pressed), transition, now);
            router.set_focus(screen);
            assert!(!router.action_held(action));
            assert!(router.tick_repeat(now + Duration::from_secs(1)).is_none());
            assert!(matches!(
                router.route_event(event(2, action, InputPhase::Released), screen, now),
                InputOutcome::Released { context, .. }
                    if context.target.kind == InputContextKind::Transition
            ));
        }
    }

    #[test]
    fn opposing_directions_require_full_neutral() {
        let screen = request(InputContextKind::Screen, 1, DirectionalPolicy::MenuRepeat);
        let mut router = InputRouter::new(screen);
        let now = Instant::now();
        router.route_event(
            event(1, LogicalAction::Left, InputPhase::Pressed),
            screen,
            now,
        );
        assert!(matches!(
            router.route_event(
                event(3, LogicalAction::Right, InputPhase::Pressed),
                screen,
                now
            ),
            InputOutcome::Consumed {
                reason: ConsumedReason::OpposingDirections,
                ..
            }
        ));
        assert!(!router.action_held(LogicalAction::Left));
        assert!(!router.action_held(LogicalAction::Right));
        router.route_event(
            event(2, LogicalAction::Left, InputPhase::Released),
            screen,
            now,
        );
        router.route_event(
            event(4, LogicalAction::Right, InputPhase::Released),
            screen,
            now,
        );
        assert!(matches!(
            router.route_event(
                event(5, LogicalAction::Right, InputPhase::Pressed),
                screen,
                now
            ),
            InputOutcome::Dispatch { .. }
        ));
        assert!(router.action_held(LogicalAction::Right));
    }

    #[test]
    fn equal_press_ids_from_distinct_sources_do_not_collide() {
        let screen = request(InputContextKind::Screen, 1, DirectionalPolicy::MenuRepeat);
        let mut router = InputRouter::new(screen);
        let now = Instant::now();
        let first = event(1, LogicalAction::Down, InputPhase::Pressed);
        let mut second = first;
        second.source.instance = 2;
        second.sequence = 2;
        assert!(matches!(
            router.route_event(first, screen, now),
            InputOutcome::Dispatch { .. }
        ));
        assert!(matches!(
            router.route_event(second, screen, now),
            InputOutcome::Dispatch { .. }
        ));

        let mut first_release = event(3, LogicalAction::Down, InputPhase::Released);
        first_release.press_id = first.press_id;
        assert!(matches!(
            router.route_event(first_release, screen, now),
            InputOutcome::Released { .. }
        ));
        assert!(router.action_held(LogicalAction::Down));

        let mut second_release = first_release;
        second_release.source.instance = 2;
        second_release.sequence = 4;
        assert!(matches!(
            router.route_event(second_release, screen, now),
            InputOutcome::Released { .. }
        ));
        assert!(!router.action_held(LogicalAction::Down));
    }

    #[test]
    fn held_action_does_not_leak_across_context_generation() {
        let screen = request(
            InputContextKind::Screen,
            1,
            DirectionalPolicy::HomeContinuous,
        );
        let modal = request(
            InputContextKind::LauncherModal,
            2,
            DirectionalPolicy::MenuRepeat,
        );
        let mut router = InputRouter::new(screen);
        let now = Instant::now();
        router.route_event(
            event(1, LogicalAction::Right, InputPhase::Pressed),
            screen,
            now,
        );
        assert!(router.action_held(LogicalAction::Right));
        router.set_focus(modal);
        assert!(!router.action_held(LogicalAction::Right));
    }

    #[test]
    fn disabled_and_screensaver_contexts_are_explicit() {
        let disabled = request(InputContextKind::Disabled, 0, DirectionalPolicy::EdgeOnly);
        let screensaver = request(
            InputContextKind::Screensaver,
            1,
            DirectionalPolicy::EdgeOnly,
        );
        let mut router = InputRouter::new(disabled);
        let now = Instant::now();
        assert!(matches!(
            router.route_event(
                event(1, LogicalAction::Activate, InputPhase::Pressed),
                disabled,
                now
            ),
            InputOutcome::Consumed {
                reason: ConsumedReason::InputDisabled,
                ..
            }
        ));
        assert!(matches!(
            router.route_event(
                event(3, LogicalAction::Activate, InputPhase::Pressed),
                screensaver,
                now
            ),
            InputOutcome::WakeScreensaver { .. }
        ));
    }
}
