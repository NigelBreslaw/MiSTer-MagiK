//! D-pad hold-to-repeat: first press immediate, 1 s pause, then every 80 ms.

use std::time::{Duration, Instant};

const INITIAL_DELAY: Duration = Duration::from_millis(1000);
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
        assert!(!gate.tick(true, t0 + Duration::from_millis(500)));
        assert!(!gate.tick(true, t0 + Duration::from_millis(999)));
    }

    #[test]
    fn repeats_after_delay() {
        let mut gate = RepeatGate::default();
        let t0 = Instant::now();
        assert!(gate.tick(true, t0));
        assert!(gate.tick(true, t0 + Duration::from_millis(1000)));
        assert!(!gate.tick(true, t0 + Duration::from_millis(1050)));
        assert!(gate.tick(true, t0 + Duration::from_millis(1080)));
    }

    #[test]
    fn release_clears() {
        let mut gate = RepeatGate::default();
        let t0 = Instant::now();
        assert!(gate.tick(true, t0));
        gate.tick(false, t0 + Duration::from_millis(10));
        assert!(gate.tick(true, t0 + Duration::from_millis(20)));
    }
}
