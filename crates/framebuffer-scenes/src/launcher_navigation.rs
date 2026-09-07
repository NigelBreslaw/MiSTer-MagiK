//! Deterministic joystick browsing state for the Mini launcher.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowseDirection {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowsePhase {
    Settled,
    Sliding,
    Held,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrowseFrame {
    pub selected: usize,
    pub target: usize,
    pub phase: BrowsePhase,
    pub direction: Option<BrowseDirection>,
    pub progress_millis: u32,
    pub duration_millis: u32,
}

pub const TAP_SLIDE_MS: u64 = 180;
pub const HOLD_THRESHOLD_MS: u64 = 300;
pub const HELD_STEP_MS: u64 = 150;

#[derive(Clone, Copy, Debug)]
pub struct LauncherBrowser {
    count: usize,
    selected: usize,
    target: usize,
    direction: Option<BrowseDirection>,
    started_ms: u64,
    duration_ms: u64,
    hold_started_ms: u64,
    left: bool,
    right: bool,
    pending: Option<BrowseDirection>,
    neutral_required: bool,
}

impl LauncherBrowser {
    #[must_use]
    pub fn new(count: usize, selected: usize) -> Self {
        let selected = if count == 0 { 0 } else { selected % count };
        Self {
            count,
            selected,
            target: selected,
            direction: None,
            started_ms: 0,
            duration_ms: TAP_SLIDE_MS,
            hold_started_ms: 0,
            left: false,
            right: false,
            pending: None,
            neutral_required: true,
        }
    }

    pub fn press(&mut self, direction: BrowseDirection, now_ms: u64) {
        if self.neutral_required {
            return;
        }
        let already_held = match direction {
            BrowseDirection::Left => self.left,
            BrowseDirection::Right => self.right,
        };
        if already_held {
            return;
        }
        match direction {
            BrowseDirection::Left => self.left = true,
            BrowseDirection::Right => self.right = true,
        }
        self.hold_started_ms = now_ms;
        if self.direction.is_some() {
            self.pending = Some(direction);
        } else {
            if !(self.left && self.right) {
                self.start(direction, now_ms, TAP_SLIDE_MS);
            }
        }
    }

    pub fn release(&mut self, direction: BrowseDirection) {
        match direction {
            BrowseDirection::Left => self.left = false,
            BrowseDirection::Right => self.right = false,
        }
    }

    pub fn reset(&mut self) {
        self.direction = None;
        self.target = self.selected;
        self.pending = None;
        self.left = false;
        self.right = false;
        self.neutral_required = true;
    }

    pub fn neutral(&mut self) {
        if !self.left && !self.right {
            self.neutral_required = false;
        }
    }

    #[must_use]
    pub fn frame(&mut self, now_ms: u64) -> BrowseFrame {
        if self.direction.is_some() {
            let elapsed = now_ms.saturating_sub(self.started_ms);
            if elapsed < self.duration_ms {
                let held = (self.left || self.right)
                    && now_ms.saturating_sub(self.hold_started_ms) >= HOLD_THRESHOLD_MS;
                return self.snapshot(held, elapsed);
            }
            self.selected = self.target;
            self.direction = None;
            if self.count == 0 || (self.left && self.right) {
                self.pending = None;
            } else {
                if let Some(next) = self.pending.take() {
                    self.start(next, now_ms, TAP_SLIDE_MS);
                } else if (self.left || self.right)
                    && now_ms.saturating_sub(self.hold_started_ms) >= HOLD_THRESHOLD_MS
                {
                    let next = if self.left {
                        BrowseDirection::Left
                    } else {
                        BrowseDirection::Right
                    };
                    self.start(next, now_ms, HELD_STEP_MS);
                }
            }
        }
        if self.direction.is_none()
            && self.count > 1
            && (self.left ^ self.right)
            && now_ms.saturating_sub(self.hold_started_ms) >= HOLD_THRESHOLD_MS
        {
            let next = if self.left {
                BrowseDirection::Left
            } else {
                BrowseDirection::Right
            };
            self.start(next, now_ms, HELD_STEP_MS);
        }
        let elapsed = self
            .direction
            .map_or(0, |_| now_ms.saturating_sub(self.started_ms));
        let held = self.direction.is_some()
            && (self.left || self.right)
            && now_ms.saturating_sub(self.hold_started_ms) >= HOLD_THRESHOLD_MS;
        self.snapshot(held, elapsed)
    }

    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    fn start(&mut self, direction: BrowseDirection, now_ms: u64, duration_ms: u64) {
        if self.count <= 1 {
            return;
        }
        self.direction = Some(direction);
        self.started_ms = now_ms;
        self.duration_ms = duration_ms;
        self.target = match direction {
            BrowseDirection::Left => (self.selected + self.count - 1) % self.count,
            BrowseDirection::Right => (self.selected + 1) % self.count,
        };
    }

    fn snapshot(&self, held: bool, elapsed: u64) -> BrowseFrame {
        BrowseFrame {
            selected: self.selected,
            target: self.target,
            phase: if self.direction.is_none() {
                BrowsePhase::Settled
            } else if held {
                BrowsePhase::Held
            } else {
                BrowsePhase::Sliding
            },
            direction: self.direction,
            progress_millis: elapsed.min(self.duration_ms) as u32,
            duration_millis: self.duration_ms as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(count: usize) -> LauncherBrowser {
        let mut browser = LauncherBrowser::new(count, 0);
        browser.neutral();
        browser
    }

    #[test]
    fn tap_moves_only_after_a_complete_slide() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 10);
        assert_eq!(browser.frame(100).selected, 0);
        browser.release(BrowseDirection::Right);
        assert_eq!(browser.frame(190).target, 1);
        assert_eq!(browser.frame(190).phase, BrowsePhase::Settled);
        assert_eq!(browser.selected(), 1);
    }

    #[test]
    fn hold_repeats_at_boundaries_and_wraps() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 0);
        let frame = browser.frame(700);
        assert_eq!(frame.selected, 1);
        assert_eq!(frame.target, 2);
        assert_eq!(frame.phase, BrowsePhase::Held);
        assert_eq!(browser.frame(1000).selected, 2);
    }

    #[test]
    fn hold_waits_for_threshold_after_first_tap() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 0);
        assert_eq!(browser.frame(180).phase, BrowsePhase::Settled);
        assert_eq!(browser.frame(299).phase, BrowsePhase::Settled);
        assert_eq!(browser.frame(300).target, 2);
        assert_eq!(browser.frame(300).phase, BrowsePhase::Held);
    }

    #[test]
    fn reversal_is_bounded_and_both_directions_cancel() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 0);
        browser.press(BrowseDirection::Left, 20);
        browser.release(BrowseDirection::Right);
        assert_eq!(browser.frame(180).selected, 1);
        assert_eq!(browser.frame(180).target, 0);
        assert_eq!(browser.frame(181).target, 0);
        browser.release(BrowseDirection::Left);
        browser.press(BrowseDirection::Right, 200);
        assert_eq!(browser.frame(400).phase, BrowsePhase::Sliding);
        assert_eq!(browser.frame(400).target, 1);
    }

    #[test]
    fn rising_edge_retap_queues_one_same_direction_move() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 0);
        browser.release(BrowseDirection::Right);
        browser.press(BrowseDirection::Right, 20);
        browser.release(BrowseDirection::Right);
        assert_eq!(browser.frame(180).selected, 1);
        assert_eq!(browser.frame(180).target, 2);
    }

    #[test]
    fn both_directions_held_complete_once_then_stay_idle() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 0);
        browser.press(BrowseDirection::Left, 20);
        assert_eq!(browser.frame(180).selected, 1);
        assert_eq!(browser.frame(1_200).phase, BrowsePhase::Settled);
        assert_eq!(browser.selected(), 1);
        assert_eq!(browser.frame(2_000).phase, BrowsePhase::Settled);
        assert_eq!(browser.selected(), 1);
    }

    #[test]
    fn reset_requires_a_neutral_sample() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 0);
        browser.reset();
        browser.press(BrowseDirection::Right, 1);
        assert_eq!(browser.frame(100).phase, BrowsePhase::Settled);
        browser.neutral();
        browser.press(BrowseDirection::Right, 200);
        assert_eq!(browser.frame(201).target, 1);
    }

    #[test]
    fn one_card_never_animates() {
        let mut browser = ready(1);
        browser.press(BrowseDirection::Right, 0);
        assert_eq!(browser.frame(10).phase, BrowsePhase::Settled);
        assert_eq!(browser.selected(), 0);
    }
}
