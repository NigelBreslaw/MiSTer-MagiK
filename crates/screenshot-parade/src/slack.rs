// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
struct SlackState {
    live: bool,
    cancelled: bool,
    ready_depth: usize,
    render_active: bool,
    preparation_active: bool,
}

/// Cooperative gate that confines card preparation to proven render slack.
pub struct PreparationSlack {
    state: Mutex<SlackState>,
    changed: Condvar,
}

impl Default for PreparationSlack {
    fn default() -> Self {
        Self::new()
    }
}

impl PreparationSlack {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(SlackState {
                live: false,
                cancelled: false,
                ready_depth: 0,
                render_active: false,
                preparation_active: false,
            }),
            changed: Condvar::new(),
        }
    }

    pub fn begin_live(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.live = true;
        self.changed.notify_all();
    }

    pub fn set_ready_depth(&self, ready_depth: usize) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.ready_depth = ready_depth;
        self.changed.notify_all();
    }

    pub fn set_render_active(&self, render_active: bool) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.render_active = render_active;
        self.changed.notify_all();
    }

    pub fn checkpoint(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.preparation_active = false;
        self.changed.notify_all();
        while state.live && !state.cancelled && (state.ready_depth < 2 || state.render_active) {
            let waited = self
                .changed
                .wait_timeout(state, Duration::from_millis(1))
                .unwrap_or_else(|error| error.into_inner());
            state = waited.0;
        }
        state.preparation_active = !state.cancelled;
    }

    pub fn finish_preparation(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.preparation_active = false;
        self.changed.notify_all();
    }

    pub fn preparation_active(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .preparation_active
    }

    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.cancelled = true;
        state.preparation_active = false;
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn live_checkpoint_waits_for_two_ready_frames_and_idle_render() {
        let slack = Arc::new(PreparationSlack::new());
        slack.begin_live();
        let passed = Arc::new(AtomicBool::new(false));
        let worker_slack = Arc::clone(&slack);
        let worker_passed = Arc::clone(&passed);
        let worker = std::thread::spawn(move || {
            worker_slack.checkpoint();
            worker_passed.store(true, Ordering::Release);
            worker_slack.finish_preparation();
        });
        std::thread::sleep(Duration::from_millis(2));
        assert!(!passed.load(Ordering::Acquire));
        slack.set_render_active(true);
        slack.set_ready_depth(2);
        std::thread::sleep(Duration::from_millis(2));
        assert!(!passed.load(Ordering::Acquire));
        slack.set_render_active(false);
        worker.join().unwrap();
        assert!(passed.load(Ordering::Acquire));
    }
}
