// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic joystick browsing state for the Mini launcher.
use crate::spring_animation::{SpringAnimation, SpringConfiguration};
use std::collections::VecDeque;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowseDirection {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowsePhase {
    Settled,
    Flipping,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrowseFrame {
    pub selected: usize,
    pub target: usize,
    pub phase: BrowsePhase,
    pub direction: Option<BrowseDirection>,
    /// Elapsed timeline milliseconds, or integrated position units when
    /// `duration_millis == SPRING_POSITION_UNITS` (not eased a second time).
    pub progress_millis: u32,
    pub duration_millis: u32,
    pub outgoing: Option<OutgoingFlip>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutgoingFlip {
    pub card: usize,
    pub direction: BrowseDirection,
    pub progress_millis: u32,
}

pub const TAP_FLIP_MS: u64 = 460;
pub const CARD_FLIP_MS: u64 = 650;
pub const OUTGOING_FLIP_MS: u64 = CARD_FLIP_MS;
pub const HOLD_THRESHOLD_MS: u64 = 300;
pub const SPRING_POSITION_UNITS: u32 = 65536;

#[derive(Clone, Debug)]
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
    pending: VecDeque<BrowseDirection>,
    neutral_required: bool,
    continuous: bool,
    position: SpringAnimation,
    speed: SpringAnimation,
    last_ms: u64,
    settle_sign: f64,
    outgoing: Option<ActiveOutgoingFlip>,
    last_continuous_outgoing: Option<(i64, BrowseDirection)>,
}

#[derive(Clone, Copy, Debug)]
struct ActiveOutgoingFlip {
    card: usize,
    direction: BrowseDirection,
    started_ms: u64,
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
            duration_ms: CARD_FLIP_MS,
            hold_started_ms: 0,
            left: false,
            right: false,
            pending: VecDeque::new(),
            neutral_required: true,
            continuous: false,
            position: SpringAnimation::new(selected as f64, SpringConfiguration::smooth()),
            speed: SpringAnimation::new(0.0, SpringConfiguration::smooth()),
            last_ms: 0,
            settle_sign: 1.0,
            outgoing: None,
            last_continuous_outgoing: None,
        }
    }

    pub fn press(&mut self, direction: BrowseDirection, now_ms: u64) {
        if self.neutral_required {
            return;
        }
        if self.continuous {
            let _ = self.frame(now_ms);
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
        if self.continuous {
            self.pending.clear();
            return;
        }
        if self.direction.is_some() {
            self.pending.push_back(direction);
        } else {
            if !(self.left && self.right) {
                self.start(direction, now_ms);
            }
        }
    }

    pub fn release(&mut self, direction: BrowseDirection) {
        match direction {
            BrowseDirection::Left => self.left = false,
            BrowseDirection::Right => self.right = false,
        }
        if self.continuous && !(self.left ^ self.right) {
            self.settle();
        }
    }

    pub fn release_at(&mut self, direction: BrowseDirection, now_ms: u64) {
        let _ = self.frame(now_ms);
        self.release(direction);
    }

    fn settle(&mut self) {
        let value = self.position.value();
        let velocity = self.position.velocity();
        let sign = if velocity.abs() > 0.0001 {
            velocity.signum()
        } else {
            self.settle_sign
        };
        let runway = velocity.abs() / self.position.configuration().angular_frequency();
        let target = if sign > 0.0 {
            (value + runway).ceil()
        } else {
            (value - runway).floor()
        };
        self.position.set_target(target);
        self.settle_sign = sign;
    }

    fn begin_continuous(&mut self, now_ms: u64) {
        self.continuous = true;
        self.position.snap_to(self.selected as f64);
        let sign = if self.left { -1.0 } else { 1.0 };
        let initial_speed = sign * 1000.0 / TAP_FLIP_MS as f64;
        self.position.set_state(self.selected as f64, initial_speed);
        self.speed.snap_to(initial_speed);
        self.last_ms = now_ms;
        self.settle_sign = sign;
        self.last_continuous_outgoing = None;
    }

    fn continuous_frame(&mut self, now_ms: u64) -> BrowseFrame {
        let held = self.left ^ self.right;
        let elapsed = now_ms.saturating_sub(self.last_ms);
        // A stalled UI must not race through the catalog on recovery. Spring
        // settling, unlike cruise integration, can safely consume the full gap.
        let dt = Duration::from_millis(if held { elapsed.min(100) } else { elapsed });
        self.last_ms = now_ms;
        if held {
            let sign = if self.left { -1.0 } else { 1.0 };
            self.speed.set_target(sign * 1000.0 / TAP_FLIP_MS as f64);
            let previous = self.speed.value();
            let velocity = self.speed.advance(dt);
            self.position.set_state(
                self.position.value() + (previous + velocity) * 0.5 * dt.as_secs_f64(),
                velocity,
            );
            self.settle_sign = if velocity.abs() > 0.0001 {
                velocity.signum()
            } else {
                sign
            };
            self.settle();
        } else {
            self.position.advance(dt);
            self.speed.set_state(self.position.velocity(), 0.0);
        }
        let value = self.position.value();
        if !held && self.position.is_settled() {
            self.selected = (value.round() as i64).rem_euclid(self.count as i64) as usize;
            self.target = self.selected;
            self.direction = None;
            self.continuous = false;
            return self.snapshot(0, now_ms);
        }
        let right = self.settle_sign > 0.0;
        let base = if right { value.floor() } else { value.ceil() };
        self.selected = (base as i64).rem_euclid(self.count as i64) as usize;
        self.target =
            (base as i64 + if right { 1 } else { -1 }).rem_euclid(self.count as i64) as usize;
        self.direction = Some(if right {
            BrowseDirection::Right
        } else {
            BrowseDirection::Left
        });
        let continuous_step = (base as i64, self.direction.expect("continuous direction"));
        if self.last_continuous_outgoing != Some(continuous_step) {
            self.outgoing = Some(ActiveOutgoingFlip {
                card: self.selected,
                direction: continuous_step.1,
                started_ms: now_ms,
            });
            self.last_continuous_outgoing = Some(continuous_step);
        }
        BrowseFrame {
            selected: self.selected,
            target: self.target,
            direction: self.direction,
            phase: BrowsePhase::Flipping,
            // Already spring-integrated Q16 progress; renderer must not ease twice.
            progress_millis: ((value - base).abs() * f64::from(SPRING_POSITION_UNITS))
                .round()
                .clamp(0.0, f64::from(SPRING_POSITION_UNITS - 1))
                as u32,
            duration_millis: SPRING_POSITION_UNITS,
            outgoing: self.outgoing_frame(now_ms),
        }
    }

    pub fn reset(&mut self) {
        self.direction = None;
        self.target = self.selected;
        self.pending.clear();
        self.left = false;
        self.right = false;
        self.neutral_required = true;
        self.continuous = false;
        self.position.snap_to(self.selected as f64);
        self.speed.snap_to(0.0);
        self.outgoing = None;
        self.last_continuous_outgoing = None;
    }

    pub fn neutral(&mut self) {
        if !self.left && !self.right {
            self.neutral_required = false;
        }
    }

    #[must_use]
    pub fn frame(&mut self, now_ms: u64) -> BrowseFrame {
        self.update_outgoing(now_ms);
        if self.continuous {
            return self.continuous_frame(now_ms);
        }
        if self.direction.is_some() {
            let elapsed = now_ms.saturating_sub(self.started_ms);
            if elapsed < self.duration_ms {
                return self.snapshot(elapsed, now_ms);
            }
            self.selected = self.target;
            self.direction = None;
            if self.count == 0 || (self.left && self.right) {
                self.pending.clear();
            } else {
                if let Some(next) = self.pending.pop_front() {
                    self.start(next, now_ms);
                } else if (self.left || self.right)
                    && now_ms.saturating_sub(self.hold_started_ms) >= HOLD_THRESHOLD_MS
                {
                    let next_started_ms = self.started_ms.saturating_add(self.duration_ms);
                    self.begin_continuous(next_started_ms);
                    return self.continuous_frame(now_ms);
                }
            }
        }
        if self.direction.is_none()
            && self.count > 1
            && (self.left ^ self.right)
            && now_ms.saturating_sub(self.hold_started_ms) >= HOLD_THRESHOLD_MS
        {
            let direction = if self.left {
                BrowseDirection::Left
            } else {
                BrowseDirection::Right
            };
            self.start(direction, now_ms);
            return self.snapshot(0, now_ms);
        }
        let elapsed = self
            .direction
            .map_or(0, |_| now_ms.saturating_sub(self.started_ms));
        self.snapshot(elapsed, now_ms)
    }

    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    fn start(&mut self, direction: BrowseDirection, now_ms: u64) {
        if self.count <= 1 {
            return;
        }
        self.direction = Some(direction);
        self.started_ms = now_ms;
        self.duration_ms = CARD_FLIP_MS;
        self.target = match direction {
            BrowseDirection::Left => (self.selected + self.count - 1) % self.count,
            BrowseDirection::Right => (self.selected + 1) % self.count,
        };
        self.outgoing = Some(ActiveOutgoingFlip {
            card: self.selected,
            direction,
            started_ms: now_ms,
        });
    }

    fn snapshot(&self, elapsed: u64, now_ms: u64) -> BrowseFrame {
        BrowseFrame {
            selected: self.selected,
            target: self.target,
            phase: if self.direction.is_none() {
                BrowsePhase::Settled
            } else {
                BrowsePhase::Flipping
            },
            direction: self.direction,
            progress_millis: elapsed.min(TAP_FLIP_MS) as u32,
            duration_millis: TAP_FLIP_MS as u32,
            outgoing: self.outgoing_frame(now_ms),
        }
    }

    fn update_outgoing(&mut self, now_ms: u64) {
        if self
            .outgoing
            .is_some_and(|flip| now_ms.saturating_sub(flip.started_ms) >= OUTGOING_FLIP_MS)
        {
            self.outgoing = None;
        }
    }

    fn outgoing_frame(&self, now_ms: u64) -> Option<OutgoingFlip> {
        self.outgoing.and_then(|flip| {
            let elapsed = now_ms.saturating_sub(flip.started_ms);
            (elapsed < OUTGOING_FLIP_MS).then_some(OutgoingFlip {
                card: flip.card,
                direction: flip.direction,
                progress_millis: elapsed as u32,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_hold_release_preserves_velocity_and_settles_forward_without_recoil() {
        for direction in [BrowseDirection::Left, BrowseDirection::Right] {
            let mut browser = ready(5);
            browser.press(direction, 0);
            for now in (0..=2400).step_by(16) {
                let _ = browser.frame(now);
            }
            let position = browser.position.value();
            let velocity = browser.position.velocity();
            assert!(velocity.abs() > 1.0);
            browser.release_at(direction, 2400);
            assert_eq!(browser.position.value(), position);
            assert_eq!(browser.position.velocity(), velocity);
            let sign = velocity.signum();
            let target = browser.position.target();
            let mut previous = position;
            assert!(
                (target - position) * sign
                    >= velocity.abs() / browser.position.configuration().angular_frequency()
            );
            assert_eq!(browser.frame(2416).phase, BrowsePhase::Flipping);
            for now in (2432..=4400).step_by(16) {
                let _ = browser.frame(now);
                let value = browser.position.value();
                assert!((value - previous) * sign >= -1e-10);
                assert!((target - value) * sign >= -1e-10);
                previous = value;
            }
            assert_eq!(browser.frame(4400).phase, BrowsePhase::Settled);
            assert_eq!(browser.position.value(), target);
        }
    }

    #[test]
    fn reversal_changes_velocity_continuously_and_reset_stops_all_springs() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 0);
        for now in (0..=1600).step_by(16) {
            let _ = browser.frame(now);
        }
        let position = browser.position.value();
        let velocity = browser.position.velocity();
        browser.press(BrowseDirection::Left, 1600);
        browser.release_at(BrowseDirection::Right, 1600);
        assert_eq!(browser.position.value(), position);
        assert_eq!(browser.position.velocity(), velocity);
        let _ = browser.frame(1616);
        assert!(
            browser.position.velocity() > 0.0,
            "no instantaneous velocity reversal"
        );
        for now in (1632..=2400).step_by(16) {
            let _ = browser.frame(now);
        }
        assert!(browser.position.velocity() < -1.0);
        browser.reset();
        assert_eq!(browser.frame(3000).phase, BrowsePhase::Settled);
        assert_eq!(browser.speed.value(), 0.0);
        assert_eq!(browser.position.velocity(), 0.0);
    }

    #[test]
    fn held_flip_mode_flips_every_step_and_release_finishes_current_flip() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 0);
        assert_eq!(browser.frame(300).phase, BrowsePhase::Flipping);
        assert_eq!(
            browser.frame(TAP_FLIP_MS - 1).duration_millis,
            TAP_FLIP_MS as u32
        );
        let next_flip = browser.frame(CARD_FLIP_MS);
        assert_eq!(
            (
                next_flip.selected,
                next_flip.target,
                next_flip.phase,
                next_flip.duration_millis
            ),
            (1, 2, BrowsePhase::Flipping, SPRING_POSITION_UNITS)
        );
        assert_eq!(next_flip.progress_millis, 0);
        let moving = browser.frame(CARD_FLIP_MS + 16);
        assert_eq!(moving.phase, BrowsePhase::Flipping);
        assert_eq!(moving.duration_millis, SPRING_POSITION_UNITS);
        assert!(moving.progress_millis > 0, "held motion does not pause");
        let cruising = browser.frame(920);
        assert_eq!(cruising.phase, BrowsePhase::Flipping);
        assert_eq!(cruising.target, (cruising.selected + 1) % 5);
        browser.release(BrowseDirection::Right);
        assert_eq!(browser.frame(1_200).phase, BrowsePhase::Flipping);
        let settled = browser.frame(2_000);
        assert_eq!(settled.phase, BrowsePhase::Settled);
        browser.press(BrowseDirection::Left, 2_100);
        browser.release(BrowseDirection::Left);
        assert_eq!(browser.frame(2_400).phase, BrowsePhase::Flipping);
        assert_eq!(browser.frame(2_750).selected, (settled.selected + 4) % 5);
    }

    #[test]
    fn outgoing_flip_starts_immediately_and_matches_incoming_duration() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 0);
        browser.release(BrowseDirection::Right);

        let started = browser.frame(0).outgoing.unwrap();
        assert_eq!(started.card, 0);
        assert_eq!(started.direction, BrowseDirection::Right);
        assert_eq!(started.progress_millis, 0);

        let almost_finished = browser.frame(OUTGOING_FLIP_MS - 1);
        assert_eq!(almost_finished.selected, 0);
        assert_eq!(almost_finished.progress_millis, TAP_FLIP_MS as u32);
        assert_eq!(almost_finished.outgoing.unwrap().progress_millis, 649);
        let finished = browser.frame(OUTGOING_FLIP_MS);
        assert_eq!(finished.selected, 1);
        assert_eq!(finished.outgoing, None);
    }

    #[test]
    fn rapid_flip_taps_finish_in_order_without_replacing_the_active_pair() {
        let mut browser = ready(5);
        for (direction, now_ms) in [
            (BrowseDirection::Right, 0),
            (BrowseDirection::Right, 20),
            (BrowseDirection::Left, 40),
        ] {
            browser.press(direction, now_ms);
            browser.release(direction);
        }

        assert_eq!(browser.frame(CARD_FLIP_MS - 1).selected, 0);
        let second = browser.frame(CARD_FLIP_MS);
        assert_eq!((second.selected, second.target), (1, 2));
        let third = browser.frame(CARD_FLIP_MS * 2);
        assert_eq!((third.selected, third.target), (2, 1));
        let finished = browser.frame(CARD_FLIP_MS * 3);
        assert_eq!(finished.selected, 1);
        assert_eq!(finished.phase, BrowsePhase::Settled);
    }

    #[test]
    fn continuous_motion_starts_each_outgoing_flip_only_once() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 0);
        assert_eq!(browser.frame(0).outgoing.unwrap().card, 0);
        let next = browser.frame(CARD_FLIP_MS).outgoing.unwrap();
        assert_eq!(next.card, 1);
        assert_eq!(next.progress_millis, 0);
        assert_eq!(
            browser.frame(CARD_FLIP_MS + OUTGOING_FLIP_MS).outgoing,
            None
        );
    }

    #[test]
    fn flip_reversal_retap_and_long_stall_are_bounded() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Left, 0);
        browser.release(BrowseDirection::Left);
        browser.press(BrowseDirection::Right, 30);
        browser.release(BrowseDirection::Right);
        let frame = browser.frame(5000);
        assert_eq!(
            (
                frame.selected,
                frame.target,
                frame.phase,
                frame.progress_millis
            ),
            (4, 0, BrowsePhase::Flipping, 0)
        );
        assert_eq!(browser.frame(5650).selected, 0);
    }

    #[test]
    fn twelve_held_steps_keep_flipping_and_both_held_finish_only_once() {
        let mut browser = ready(5);
        browser.press(BrowseDirection::Right, 0);
        for step in 0..12 {
            let frame = browser.frame(CARD_FLIP_MS + TAP_FLIP_MS * step);
            assert_eq!(frame.phase, BrowsePhase::Flipping);
            assert_eq!(frame.duration_millis, SPRING_POSITION_UNITS);
        }
        browser.press(BrowseDirection::Left, 5_800);
        let final_frame = browser.frame(8_000);
        assert_eq!(final_frame.phase, BrowsePhase::Settled);
        assert_eq!(browser.frame(9_000), final_frame);
    }

    fn ready(count: usize) -> LauncherBrowser {
        let mut browser = LauncherBrowser::new(count, 0);
        browser.neutral();
        browser
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
