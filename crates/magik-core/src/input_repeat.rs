// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! D-pad hold-to-repeat: first press immediate, 300 ms pause, then every 80 ms.

use std::time::{Duration, Instant};

const INITIAL_DELAY: Duration = Duration::from_millis(300);
const REPEAT_INTERVAL: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Debug, Default)]
pub struct RepeatGate {
    hold_start: Option<Instant>,
    last_fire: Option<Instant>,
}

impl RepeatGate {
    /// Returns `true` when the bound action should fire this frame.
    pub fn tick(&mut self, held: bool, now: Instant) -> bool {
        if !held {
            self.hold_start = None;
            self.last_fire = None;
            return false;
        }

        match self.hold_start {
            None => {
                self.hold_start = Some(now);
                self.last_fire = Some(now);
                true
            }
            Some(start) => {
                let last = self.last_fire.unwrap_or(start);
                let elapsed = now.duration_since(start);
                if elapsed < INITIAL_DELAY {
                    false
                } else if now.duration_since(last) >= REPEAT_INTERVAL {
                    self.last_fire = Some(now);
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn repeat_active(&self, now: Instant) -> bool {
        self.hold_start
            .is_some_and(|start| now.saturating_duration_since(start) >= INITIAL_DELAY)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RepeatNav {
    pub up: RepeatGate,
    pub down: RepeatGate,
    pub left: RepeatGate,
    pub right: RepeatGate,
}

impl RepeatNav {
    pub fn tick_up(&mut self, held: bool, now: Instant) -> bool {
        self.up.tick(held, now)
    }

    pub fn tick_down(&mut self, held: bool, now: Instant) -> bool {
        self.down.tick(held, now)
    }

    pub fn tick_left(&mut self, held: bool, now: Instant) -> bool {
        self.left.tick(held, now)
    }

    pub fn tick_right(&mut self, held: bool, now: Instant) -> bool {
        self.right.tick(held, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_immediately_then_waits() {
        let mut gate = RepeatGate::default();
        let t0 = Instant::now();
        assert!(gate.tick(true, t0));
        assert!(!gate.tick(true, t0 + Duration::from_millis(150)));
        assert!(!gate.tick(true, t0 + Duration::from_millis(299)));
    }

    #[test]
    fn repeats_after_delay() {
        let mut gate = RepeatGate::default();
        let t0 = Instant::now();
        assert!(gate.tick(true, t0));
        assert!(gate.tick(true, t0 + Duration::from_millis(300)));
        assert!(!gate.tick(true, t0 + Duration::from_millis(350)));
        assert!(gate.tick(true, t0 + Duration::from_millis(380)));
    }

    #[test]
    fn repeat_active_excludes_the_initial_press_and_delay() {
        let mut gate = RepeatGate::default();
        let t0 = Instant::now();
        assert!(gate.tick(true, t0));
        assert!(!gate.repeat_active(t0));
        assert!(!gate.repeat_active(t0 + Duration::from_millis(299)));
        assert!(gate.repeat_active(t0 + Duration::from_millis(300)));
        gate.tick(false, t0 + Duration::from_millis(301));
        assert!(!gate.repeat_active(t0 + Duration::from_millis(301)));
    }

    #[test]
    fn release_clears() {
        let mut gate = RepeatGate::default();
        let t0 = Instant::now();
        assert!(gate.tick(true, t0));
        gate.tick(false, t0 + Duration::from_millis(10));
        assert!(gate.tick(true, t0 + Duration::from_millis(20)));
    }

    #[test]
    fn navigation_directions_keep_independent_repeat_state() {
        let mut nav = RepeatNav::default();
        let t0 = Instant::now();

        assert!(nav.tick_up(true, t0));
        assert!(nav.tick_down(true, t0 + Duration::from_millis(1)));
        assert!(nav.tick_left(true, t0 + Duration::from_millis(2)));
        assert!(nav.tick_right(true, t0 + Duration::from_millis(3)));
        assert!(!nav.tick_up(true, t0 + Duration::from_millis(299)));
        assert!(!nav.tick_down(false, t0 + Duration::from_millis(500)));
        assert!(nav.tick_down(true, t0 + Duration::from_millis(501)));
    }
}
