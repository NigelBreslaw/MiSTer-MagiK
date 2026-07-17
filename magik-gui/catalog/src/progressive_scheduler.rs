// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic Arcade-first scheduling for bounded catalog work slices.

use crate::catalog_classify::SystemId;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiPressure {
    Idle,
    Interactive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SystemBuildState {
    Queued,
    Scanning,
    Ready { generation: u64, games: u64 },
    Failed { message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemScheduleStatus {
    pub system_id: SystemId,
    pub state: SystemBuildState,
    pub requested: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScheduleDecision {
    /// Execute one implementation-defined, bounded unit, then poll again.
    RunSlice {
        system_id: SystemId,
    },
    PausedForUi {
        system_id: SystemId,
    },
    Complete,
}

#[derive(Clone, Debug)]
struct Entry {
    state: SystemBuildState,
    order: u32,
    insertion: u64,
    requested: bool,
}

/// The scheduler grants work only one bounded slice at a time. Once the first
/// visible system is ready, there is deliberately no fairness bypass: active
/// UI pressure can pause background catalog work indefinitely.
pub struct ProgressiveCatalogScheduler {
    first_visible: SystemId,
    entries: BTreeMap<SystemId, Entry>,
    current: Option<SystemId>,
    next_insertion: u64,
}

impl ProgressiveCatalogScheduler {
    pub fn new(first_visible: SystemId) -> Self {
        Self {
            first_visible,
            entries: BTreeMap::new(),
            current: None,
            next_insertion: 0,
        }
    }

    pub fn enqueue(&mut self, system_id: SystemId, order: u32) {
        if self.entries.contains_key(&system_id) {
            return;
        }
        let insertion = self.next_insertion;
        self.next_insertion = self.next_insertion.saturating_add(1);
        self.entries.insert(
            system_id,
            Entry {
                state: SystemBuildState::Queued,
                order,
                insertion,
                requested: false,
            },
        );
    }

    /// Promote a placeholder selected by the user without interrupting the
    /// currently materializing shard.
    pub fn request(&mut self, system_id: &SystemId) {
        if let Some(entry) = self.entries.get_mut(system_id) {
            if matches!(entry.state, SystemBuildState::Queued) {
                entry.requested = true;
            }
        }
    }

    pub fn poll(&mut self, pressure: UiPressure) -> ScheduleDecision {
        if self.current.is_none() {
            self.current = self.select_next();
            if let Some(system_id) = &self.current {
                self.entries
                    .get_mut(system_id)
                    .expect("selected system exists")
                    .state = SystemBuildState::Scanning;
            }
        }
        let Some(system_id) = self.current.clone() else {
            return ScheduleDecision::Complete;
        };
        if self.visible_system_ready() && pressure == UiPressure::Interactive {
            ScheduleDecision::PausedForUi { system_id }
        } else {
            ScheduleDecision::RunSlice { system_id }
        }
    }

    pub fn mark_ready(
        &mut self,
        system_id: &SystemId,
        generation: u64,
        games: u64,
    ) -> Result<(), SchedulerError> {
        self.require_current(system_id)?;
        let entry = self
            .entries
            .get_mut(system_id)
            .expect("current entry exists");
        entry.state = SystemBuildState::Ready { generation, games };
        entry.requested = false;
        self.current = None;
        Ok(())
    }

    pub fn mark_failed(
        &mut self,
        system_id: &SystemId,
        message: impl Into<String>,
    ) -> Result<(), SchedulerError> {
        self.require_current(system_id)?;
        let entry = self
            .entries
            .get_mut(system_id)
            .expect("current entry exists");
        entry.state = SystemBuildState::Failed {
            message: message.into(),
        };
        entry.requested = false;
        self.current = None;
        Ok(())
    }

    pub fn status(&self) -> Vec<SystemScheduleStatus> {
        let mut status = self
            .entries
            .iter()
            .map(|(system_id, entry)| SystemScheduleStatus {
                system_id: system_id.clone(),
                state: entry.state.clone(),
                requested: entry.requested,
            })
            .collect::<Vec<_>>();
        status.sort_by(|left, right| {
            let left_entry = self.entries.get(&left.system_id).expect("status entry");
            let right_entry = self.entries.get(&right.system_id).expect("status entry");
            left_entry
                .order
                .cmp(&right_entry.order)
                .then_with(|| left_entry.insertion.cmp(&right_entry.insertion))
                .then_with(|| left.system_id.cmp(&right.system_id))
        });
        status
    }

    fn visible_system_ready(&self) -> bool {
        self.entries
            .values()
            .any(|entry| matches!(entry.state, SystemBuildState::Ready { .. }))
    }

    fn select_next(&self) -> Option<SystemId> {
        let queued = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.state, SystemBuildState::Queued))
            .collect::<Vec<_>>();
        if queued.is_empty() {
            return None;
        }
        if !self.visible_system_ready()
            && queued
                .iter()
                .any(|(system_id, _)| *system_id == &self.first_visible)
        {
            return Some(self.first_visible.clone());
        }
        queued
            .into_iter()
            .min_by_key(|(system_id, entry)| {
                (
                    !entry.requested,
                    entry.order,
                    entry.insertion,
                    (*system_id).clone(),
                )
            })
            .map(|(system_id, _)| system_id.clone())
    }

    fn require_current(&self, system_id: &SystemId) -> Result<(), SchedulerError> {
        if self.current.as_ref() != Some(system_id) {
            return Err(SchedulerError);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcade_is_the_first_blocking_system_regardless_of_presentation_order() {
        let arcade = system("arcade");
        let mut scheduler = ProgressiveCatalogScheduler::new(arcade.clone());
        scheduler.enqueue(system("snes"), 1);
        scheduler.enqueue(arcade.clone(), 100);
        scheduler.enqueue(system("c64"), 2);
        assert_eq!(
            scheduler.poll(UiPressure::Interactive),
            ScheduleDecision::RunSlice { system_id: arcade }
        );
    }

    #[test]
    fn interactive_ui_pauses_every_post_bootstrap_slice_without_fairness_escape() {
        let arcade = system("arcade");
        let snes = system("snes");
        let mut scheduler = ProgressiveCatalogScheduler::new(arcade.clone());
        scheduler.enqueue(arcade.clone(), 0);
        scheduler.enqueue(snes.clone(), 1);
        assert!(matches!(
            scheduler.poll(UiPressure::Idle),
            ScheduleDecision::RunSlice { .. }
        ));
        scheduler.mark_ready(&arcade, 1, 50).unwrap();
        for _ in 0..10_000 {
            assert_eq!(
                scheduler.poll(UiPressure::Interactive),
                ScheduleDecision::PausedForUi {
                    system_id: snes.clone()
                }
            );
        }
        assert_eq!(
            scheduler.poll(UiPressure::Idle),
            ScheduleDecision::RunSlice { system_id: snes }
        );
    }

    #[test]
    fn selected_placeholder_is_next_but_does_not_interrupt_current_work() {
        let arcade = system("arcade");
        let c64 = system("c64");
        let snes = system("snes");
        let mut scheduler = ProgressiveCatalogScheduler::new(arcade.clone());
        scheduler.enqueue(arcade.clone(), 0);
        scheduler.enqueue(c64.clone(), 1);
        scheduler.enqueue(snes.clone(), 2);
        scheduler.poll(UiPressure::Idle);
        scheduler.mark_ready(&arcade, 1, 1).unwrap();
        assert_eq!(
            scheduler.poll(UiPressure::Idle),
            ScheduleDecision::RunSlice {
                system_id: c64.clone()
            }
        );
        scheduler.request(&snes);
        assert_eq!(
            scheduler.poll(UiPressure::Idle),
            ScheduleDecision::RunSlice { system_id: c64 }
        );
        scheduler.mark_ready(&system("c64"), 2, 1).unwrap();
        assert_eq!(
            scheduler.poll(UiPressure::Idle),
            ScheduleDecision::RunSlice { system_id: snes }
        );
    }

    #[test]
    fn failures_are_visible_and_do_not_block_later_systems() {
        let arcade = system("arcade");
        let snes = system("snes");
        let mut scheduler = ProgressiveCatalogScheduler::new(arcade.clone());
        scheduler.enqueue(arcade.clone(), 0);
        scheduler.enqueue(snes.clone(), 1);
        scheduler.poll(UiPressure::Idle);
        scheduler.mark_failed(&arcade, "bad input").unwrap();
        assert_eq!(
            scheduler.poll(UiPressure::Idle),
            ScheduleDecision::RunSlice {
                system_id: snes.clone()
            }
        );
        assert!(scheduler.status().iter().any(|status| {
            status.system_id == arcade
                && status.state
                    == SystemBuildState::Failed {
                        message: "bad input".to_string(),
                    }
        }));
    }

    #[test]
    fn duplicate_enqueue_is_idempotent() {
        let arcade = system("arcade");
        let mut scheduler = ProgressiveCatalogScheduler::new(arcade.clone());
        scheduler.enqueue(arcade.clone(), 1);
        scheduler.enqueue(arcade, 2);
        assert_eq!(scheduler.status().len(), 1);
    }

    fn system(value: &str) -> SystemId {
        SystemId::parse(value).unwrap()
    }
}
