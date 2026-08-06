// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
struct SlackState {
    live: bool,
    cancelled: bool,
    ready_depth: usize,
    render_requested: bool,
    raster_active: bool,
    decode_active: bool,
    raster_epoch: u64,
    decode_epoch: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PreparationSlackSnapshot {
    pub raster_active: bool,
    pub decode_active: bool,
    pub raster_epoch: u64,
    pub decode_epoch: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderPauseReceipt {
    pub waited_us: u64,
    pub waited: bool,
    pub timed_out: bool,
}

pub struct RenderPauseGuard<'a> {
    slack: &'a PreparationSlack,
    receipt: RenderPauseReceipt,
}

impl RenderPauseGuard<'_> {
    #[must_use]
    pub const fn receipt(&self) -> RenderPauseReceipt {
        self.receipt
    }
}

impl Drop for RenderPauseGuard<'_> {
    fn drop(&mut self) {
        let mut state = self
            .slack
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.render_requested = false;
        self.slack.changed.notify_all();
    }
}

pub struct PreparationDecodeGuard<'a> {
    slack: &'a PreparationSlack,
}

impl Drop for PreparationDecodeGuard<'_> {
    fn drop(&mut self) {
        let mut state = self
            .slack
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.decode_active {
            state.decode_active = false;
            state.decode_epoch = state.decode_epoch.wrapping_add(1);
        }
        self.slack.changed.notify_all();
    }
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
                render_requested: false,
                raster_active: false,
                decode_active: false,
                raster_epoch: 0,
                decode_epoch: 0,
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

    pub fn begin_decode(&self) -> PreparationDecodeGuard<'_> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while state.live && !state.cancelled && (state.ready_depth < 2 || state.render_requested) {
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(1))
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
        if !state.cancelled {
            state.decode_active = true;
            state.decode_epoch = state.decode_epoch.wrapping_add(1);
        }
        PreparationDecodeGuard { slack: self }
    }

    /// Claims the render critical section and, once presentation is live,
    /// waits for the preparation worker to acknowledge its next checkpoint.
    ///
    /// The returned time is the cooperative pause response. Warmup deliberately
    /// permits overlap and therefore reports zero.
    pub fn begin_render(&self, maximum_wait: Duration) -> RenderPauseGuard<'_> {
        let started = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.render_requested = true;
        self.changed.notify_all();
        if !state.live {
            return RenderPauseGuard {
                slack: self,
                receipt: RenderPauseReceipt::default(),
            };
        }
        let waited = state.raster_active;
        let mut timed_out = false;
        while !state.cancelled && state.raster_active {
            let elapsed = started.elapsed();
            let Some(remaining) = maximum_wait.checked_sub(elapsed) else {
                timed_out = true;
                break;
            };
            let waited = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|error| error.into_inner());
            state = waited.0;
            if waited.1.timed_out() && state.raster_active {
                timed_out = true;
                break;
            }
        }
        RenderPauseGuard {
            slack: self,
            receipt: RenderPauseReceipt {
                waited_us: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
                waited,
                timed_out,
            },
        }
    }

    pub fn checkpoint(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.raster_active {
            state.raster_active = false;
            state.raster_epoch = state.raster_epoch.wrapping_add(1);
        }
        self.changed.notify_all();
        while state.live && !state.cancelled && (state.ready_depth < 2 || state.render_requested) {
            let waited = self
                .changed
                .wait_timeout(state, Duration::from_millis(1))
                .unwrap_or_else(|error| error.into_inner());
            state = waited.0;
        }
        if !state.cancelled {
            state.raster_active = true;
            state.raster_epoch = state.raster_epoch.wrapping_add(1);
        }
    }

    pub fn finish_preparation(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.raster_active {
            state.raster_active = false;
            state.raster_epoch = state.raster_epoch.wrapping_add(1);
        }
        self.changed.notify_all();
    }

    pub fn preparation_active(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .raster_active
    }

    #[must_use]
    pub fn snapshot(&self) -> PreparationSlackSnapshot {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        PreparationSlackSnapshot {
            raster_active: state.raster_active,
            decode_active: state.decode_active,
            raster_epoch: state.raster_epoch,
            decode_epoch: state.decode_epoch,
        }
    }

    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.cancelled = true;
        state.render_requested = false;
        state.raster_active = false;
        state.decode_active = false;
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
        let render = slack.begin_render(Duration::from_millis(10));
        slack.set_ready_depth(2);
        std::thread::sleep(Duration::from_millis(2));
        assert!(!passed.load(Ordering::Acquire));
        drop(render);
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
            let render = render_slack.begin_render(Duration::from_millis(10));
            render_entered_worker.store(true, Ordering::Release);
            drop(render);
        });
        std::thread::sleep(Duration::from_millis(2));
        assert!(!render_entered.load(Ordering::Acquire));

        slack.checkpoint();
        renderer.join().unwrap();
        assert!(render_entered.load(Ordering::Acquire));
        slack.finish_preparation();
    }
}
