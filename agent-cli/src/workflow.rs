// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{ResourceClass, Risk, WorkflowPhase};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Planned,
    Cheap,
    Host,
    Expensive,
    External,
    Device,
    Compensating,
    Complete,
    Failed,
    RecoveryRequired,
}

impl State {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Cheap => "cheap checks",
            Self::Host => "host validation",
            Self::Expensive => "building",
            Self::External => "waiting for external validation",
            Self::Device => "device operation",
            Self::Compensating => "rolling back",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::RecoveryRequired => "recovery required",
        }
    }

    const fn rank(self) -> Option<u8> {
        match self {
            Self::Planned => Some(0),
            Self::Cheap => Some(1),
            Self::Host => Some(2),
            Self::Expensive => Some(3),
            Self::External => Some(4),
            Self::Device => Some(5),
            _ => None,
        }
    }
}

impl From<WorkflowPhase> for State {
    fn from(value: WorkflowPhase) -> Self {
        match value {
            WorkflowPhase::Cheap => Self::Cheap,
            WorkflowPhase::Host => Self::Host,
            WorkflowPhase::Expensive => Self::Expensive,
            WorkflowPhase::External => Self::External,
            WorkflowPhase::Device => Self::Device,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Advance(State),
    Fail,
    Compensate,
    CompensationFailed,
    Finish,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Action {
    pub id: String,
    pub risk: Risk,
    pub resource: ResourceClass,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Compensation {
    pub action: Action,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalOutcome {
    Complete,
    Failed,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transition {
    pub from: State,
    pub event: Event,
    pub to: State,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Machine {
    state: State,
}

impl Default for Machine {
    fn default() -> Self {
        Self {
            state: State::Planned,
        }
    }
}

impl Machine {
    #[must_use]
    pub const fn state(self) -> State {
        self.state
    }

    pub fn apply(&mut self, event: Event) -> Result<Transition, String> {
        let from = self.state;
        let to = match event {
            Event::Advance(next)
                if from
                    .rank()
                    .zip(next.rank())
                    .is_some_and(|(current, requested)| requested >= current) =>
            {
                next
            }
            Event::Fail if !matches!(from, State::Complete | State::RecoveryRequired) => {
                State::Failed
            }
            Event::Compensate if from == State::Failed => State::Compensating,
            Event::CompensationFailed if from == State::Compensating => State::RecoveryRequired,
            Event::Finish
                if matches!(
                    from,
                    State::Planned
                        | State::Cheap
                        | State::Host
                        | State::Expensive
                        | State::External
                        | State::Device
                        | State::Compensating
                ) =>
            {
                State::Complete
            }
            _ => return Err(format!("invalid workflow transition: {from:?} + {event:?}")),
        };
        self.state = to;
        Ok(Transition { from, event, to })
    }

    #[must_use]
    pub const fn outcome(self) -> Option<TerminalOutcome> {
        match self.state {
            State::Complete => Some(TerminalOutcome::Complete),
            State::Failed => Some(TerminalOutcome::Failed),
            State::RecoveryRequired => Some(TerminalOutcome::RecoveryRequired),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_advance_in_expense_and_risk_order() {
        let mut machine = Machine::default();
        for state in [State::Cheap, State::Host, State::Expensive, State::Device] {
            machine.apply(Event::Advance(state)).unwrap();
        }
        assert!(machine.apply(Event::Advance(State::Host)).is_err());
        machine.apply(Event::Finish).unwrap();
        assert_eq!(machine.outcome(), Some(TerminalOutcome::Complete));
    }

    #[test]
    fn failed_compensation_requires_attended_recovery() {
        let mut machine = Machine::default();
        machine.apply(Event::Advance(State::Device)).unwrap();
        machine.apply(Event::Fail).unwrap();
        machine.apply(Event::Compensate).unwrap();
        machine.apply(Event::CompensationFailed).unwrap();
        assert_eq!(machine.outcome(), Some(TerminalOutcome::RecoveryRequired));
    }

    #[test]
    fn terminal_states_are_immutable() {
        let mut machine = Machine::default();
        machine.apply(Event::Finish).unwrap();
        assert!(machine.apply(Event::Fail).is_err());
        assert!(machine.apply(Event::Advance(State::Device)).is_err());
    }
}
