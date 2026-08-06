// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

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

    /// Claims the render critical section and, once presentation is live,
    /// waits for the preparation worker to acknowledge its next checkpoint.
    ///
    /// The returned time is the cooperative pause response. Warmup deliberately
    /// permits overlap and therefore reports zero.
    pub fn begin_render(&self) -> Duration {
        let started = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.render_active = true;
        self.changed.notify_all();
        if !state.live {
            return Duration::ZERO;
        }
        while !state.cancelled && state.preparation_active {
            let waited = self
                .changed
                .wait_timeout(state, Duration::from_millis(1))
                .unwrap_or_else(|error| error.into_inner());
            state = waited.0;
        }
        started.elapsed()
    }

    pub fn finish_render(&self) {
        self.set_render_active(false);
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

    #[test]
    fn live_render_waits_for_preparation_checkpoint_acknowledgement() {
        let slack = Arc::new(PreparationSlack::new());
        slack.set_ready_depth(2);
        slack.checkpoint();
        slack.begin_live();

        let render_entered = Arc::new(AtomicBool::new(false));
        let render_slack = Arc::clone(&slack);
        let render_entered_worker = Arc::clone(&render_entered);
        let renderer = std::thread::spawn(move || {
            render_slack.begin_render();
            render_entered_worker.store(true, Ordering::Release);
            render_slack.finish_render();
        });
        std::thread::sleep(Duration::from_millis(2));
        assert!(!render_entered.load(Ordering::Acquire));

        slack.checkpoint();
        renderer.join().unwrap();
        assert!(render_entered.load(Ordering::Acquire));
        slack.finish_preparation();
    }
}
