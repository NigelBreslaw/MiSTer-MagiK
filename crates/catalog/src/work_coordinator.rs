// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cooperative process-wide arbitration between interactive and maintenance work.
//!
//! This intentionally does not change the first catalog builder's scheduling
//! policy.  It is for work which can safely defer a small unit while a user
//! action is pending (preview decode, launch preparation, and media I/O).

use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const BACKGROUND_WAIT: Duration = Duration::from_millis(2);
const FAIRNESS_YIELD_LIMIT: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkClass {
    Foreground,
    Background,
}

impl WorkClass {
    fn label(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
        }
    }
}

#[derive(Default)]
struct State {
    foreground_active: usize,
    background_active: usize,
    background_yields: u64,
    fairness_passes: u64,
    consecutive_background_yields: u32,
}

/// A small coordinator which deliberately favours foreground work while
/// allowing one bounded background unit after sustained contention.
pub struct WorkCoordinator {
    state: Mutex<State>,
    changed: Condvar,
}

impl WorkCoordinator {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        }
    }

    pub fn acquire(&'static self, class: WorkClass, label: &'static str) -> WorkLease {
        let started = Instant::now();
        let mut state = self.state.lock().expect("work coordinator lock poisoned");
        match class {
            WorkClass::Foreground => state.foreground_active += 1,
            WorkClass::Background => state.background_active += 1,
        }
        self.changed.notify_all();
        drop(state);
        crate::catalog_logln!(
            "work_coordinator_tsv\tphase=acquire\tclass={}\tlabel={}\twait_us={}",
            class.label(),
            label,
            started.elapsed().as_micros()
        );
        WorkLease {
            coordinator: self,
            class,
            label,
            acquired: started,
        }
    }

    fn cooperate_background(&self, label: &'static str) -> bool {
        let mut state = self.state.lock().expect("work coordinator lock poisoned");
        if state.foreground_active == 0 {
            state.consecutive_background_yields = 0;
            return false;
        }
        if state.consecutive_background_yields >= FAIRNESS_YIELD_LIMIT {
            state.consecutive_background_yields = 0;
            state.fairness_passes += 1;
            crate::catalog_logln!(
                "work_coordinator_tsv\tphase=fairness-pass\tclass=background\tlabel={}\tforeground_active={}",
                label,
                state.foreground_active
            );
            return false;
        }
        state.background_yields += 1;
        state.consecutive_background_yields += 1;
        let foreground_active = state.foreground_active;
        let started = Instant::now();
        let _ = self
            .changed
            .wait_timeout(state, BACKGROUND_WAIT)
            .expect("work coordinator wait poisoned");
        crate::catalog_logln!(
            "work_coordinator_tsv\tphase=yield\tclass=background\tlabel={}\thold_us={}\tforeground_active={}",
            label,
            started.elapsed().as_micros(),
            foreground_active
        );
        true
    }

    fn release(&self, class: WorkClass, label: &'static str, acquired: Instant) {
        let mut state = self.state.lock().expect("work coordinator lock poisoned");
        match class {
            WorkClass::Foreground => {
                state.foreground_active = state.foreground_active.saturating_sub(1)
            }
            WorkClass::Background => {
                state.background_active = state.background_active.saturating_sub(1)
            }
        }
        let foreground_active = state.foreground_active;
        let background_active = state.background_active;
        self.changed.notify_all();
        drop(state);
        crate::catalog_logln!(
            "work_coordinator_tsv\tphase=release\tclass={}\tlabel={}\thold_us={}\tforeground_active={}\tbackground_active={}",
            class.label(),
            label,
            acquired.elapsed().as_micros(),
            foreground_active,
            background_active
        );
    }
}

impl Default for WorkCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL: OnceLock<WorkCoordinator> = OnceLock::new();

pub fn foreground(label: &'static str) -> WorkLease {
    GLOBAL
        .get_or_init(WorkCoordinator::new)
        .acquire(WorkClass::Foreground, label)
}

pub fn background(label: &'static str) -> WorkLease {
    GLOBAL
        .get_or_init(WorkCoordinator::new)
        .acquire(WorkClass::Background, label)
}

/// Allows bounded background helpers to yield at a safe chunk boundary.
pub fn cooperate_background(label: &'static str) -> bool {
    GLOBAL
        .get_or_init(WorkCoordinator::new)
        .cooperate_background(label)
}

/// RAII lease.  Dropping it releases the class even during an error or panic.
pub struct WorkLease {
    coordinator: &'static WorkCoordinator,
    class: WorkClass,
    label: &'static str,
    acquired: Instant,
}

impl WorkLease {
    /// Background callers invoke this between bounded I/O or CPU units.
    /// Returns true when it yielded to foreground work.
    pub fn cooperate(&self) -> bool {
        matches!(self.class, WorkClass::Background)
            && self.coordinator.cooperate_background(self.label)
    }
}

impl Drop for WorkLease {
    fn drop(&mut self) {
        self.coordinator
            .release(self.class, self.label, self.acquired);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_foreground_lease_unblocks_background_cooperation() {
        let coordinator = Box::leak(Box::new(WorkCoordinator::new()));
        let background = coordinator.acquire(WorkClass::Background, "test-background");
        let foreground = coordinator.acquire(WorkClass::Foreground, "test-foreground");
        assert!(background.cooperate());
        drop(foreground);
        assert!(!background.cooperate());
    }

    #[test]
    fn fairness_bounds_repeated_background_yields() {
        let coordinator = Box::leak(Box::new(WorkCoordinator::new()));
        let background = coordinator.acquire(WorkClass::Background, "test-background");
        let _foreground = coordinator.acquire(WorkClass::Foreground, "test-foreground");
        for _ in 0..FAIRNESS_YIELD_LIMIT {
            assert!(background.cooperate());
        }
        assert!(!background.cooperate());
    }
}
