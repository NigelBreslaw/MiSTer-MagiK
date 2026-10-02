// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Vsync-locked animation time.
//!
//! Animation advances by exactly one display period per produced frame and is
//! never derived from the time the code happens to run. A late frame therefore
//! slows motion down instead of enlarging the next step, as on frame-locked
//! arcade hardware. Consumers keep taking `Instant` values, so the clock hands
//! out a synthetic timeline: `epoch + frames * period`.
//!
//! The one place real time is consulted is [`FrameClock::advance_idle`]. While
//! the launcher sleeps no frames are produced and nothing is animating, but
//! gaps such as "released, then pressed again within 350 ms" still have to
//! span the sleep. Idle wall time is therefore counted in whole display
//! periods, as if the refresh had kept ticking. Producing a frame never uses
//! it: every produced frame is exactly one period.

use std::time::{Duration, Instant};

/// Nominal 60 Hz display period, used until a display route reports its own.
pub const REFERENCE_FRAME_PERIOD: Duration = Duration::from_nanos(16_666_667);

#[derive(Clone, Copy, Debug)]
pub struct FrameClock {
    epoch: Instant,
    period: Duration,
    elapsed: Duration,
    frame: u64,
    idle_remainder: Duration,
}

impl FrameClock {
    pub fn new(epoch: Instant, period: Duration) -> Self {
        assert!(!period.is_zero(), "frame period must be positive");
        Self {
            epoch,
            period,
            elapsed: Duration::ZERO,
            frame: 0,
            idle_remainder: Duration::ZERO,
        }
    }

    pub fn from_period_us(epoch: Instant, period_us: u64) -> Self {
        Self::new(
            epoch,
            if period_us == 0 {
                REFERENCE_FRAME_PERIOD
            } else {
                Duration::from_micros(period_us)
            },
        )
    }

    pub const fn period(&self) -> Duration {
        self.period
    }

    pub const fn frame(&self) -> u64 {
        self.frame
    }

    /// Animation time since the epoch. Equal to `frame * period` while the
    /// period is unchanged.
    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub fn elapsed_us(&self) -> u64 {
        self.elapsed.as_micros().min(u128::from(u64::MAX)) as u64
    }

    /// The current frame's time on the same timeline as every other consumer.
    pub fn now(&self) -> Instant {
        self.epoch + self.elapsed
    }

    /// Changes the per-frame step without moving time backwards or forwards.
    pub fn set_period(&mut self, period: Duration) {
        assert!(!period.is_zero(), "frame period must be positive");
        self.period = period;
    }

    /// Moves to the next frame. Call once per produced frame, never per
    /// wake-up, so a repeated or missed refresh does not create time.
    pub fn advance(&mut self) {
        self.idle_remainder = Duration::ZERO;
        self.step();
    }

    /// Accounts for wall time that passed without a produced frame, in whole
    /// display periods. The sub-period remainder carries into the next call
    /// and is dropped once a real frame is produced. Returns the periods added.
    pub fn advance_idle(&mut self, wall: Duration) -> u64 {
        let total = self.idle_remainder.saturating_add(wall);
        let periods = (total.as_nanos() / self.period.as_nanos()).min(u128::from(u32::MAX)) as u32;
        self.idle_remainder = total.saturating_sub(self.period.saturating_mul(periods));
        for _ in 0..periods {
            self.step();
        }
        u64::from(periods)
    }

    fn step(&mut self) {
        self.frame = self.frame.saturating_add(1);
        self.elapsed = self.elapsed.saturating_add(self.period);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_advances_by_exactly_one_period() {
        let epoch = Instant::now();
        let mut clock = FrameClock::new(epoch, Duration::from_micros(16_667));
        let mut previous = clock.now();
        assert_eq!(previous, epoch);
        for frame in 1..=600_u32 {
            clock.advance();
            assert_eq!(
                clock.now().duration_since(previous),
                Duration::from_micros(16_667)
            );
            assert_eq!(clock.elapsed(), Duration::from_micros(16_667) * frame);
            previous = clock.now();
        }
        assert_eq!(clock.frame(), 600);
    }

    #[test]
    fn time_does_not_move_between_advances() {
        let mut clock = FrameClock::new(Instant::now(), Duration::from_millis(20));
        clock.advance();
        let first = clock.now();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(clock.now(), first);
    }

    #[test]
    fn fifty_hertz_steps_are_twenty_milliseconds() {
        let mut clock = FrameClock::from_period_us(Instant::now(), 20_000);
        for _ in 0..50 {
            clock.advance();
        }
        assert_eq!(clock.elapsed(), Duration::from_secs(1));
    }

    #[test]
    fn changing_period_keeps_time_continuous() {
        let mut clock = FrameClock::new(Instant::now(), Duration::from_millis(20));
        clock.advance();
        let before = clock.now();
        clock.set_period(Duration::from_micros(16_667));
        assert_eq!(clock.now(), before);
        clock.advance();
        assert_eq!(
            clock.now().duration_since(before),
            Duration::from_micros(16_667)
        );
    }

    #[test]
    fn idle_wall_time_counts_whole_periods_and_carries_the_remainder() {
        let mut clock = FrameClock::new(Instant::now(), Duration::from_millis(20));
        assert_eq!(clock.advance_idle(Duration::from_millis(15)), 0);
        assert_eq!(clock.frame(), 0);
        assert_eq!(clock.advance_idle(Duration::from_millis(30)), 2);
        assert_eq!(clock.elapsed(), Duration::from_millis(40));
        // 5 ms was left over; 15 more completes the next period.
        assert_eq!(clock.advance_idle(Duration::from_millis(15)), 1);
        assert_eq!(clock.elapsed(), Duration::from_millis(60));
    }

    #[test]
    fn a_produced_frame_discards_idle_remainder() {
        let mut clock = FrameClock::new(Instant::now(), Duration::from_millis(20));
        clock.advance_idle(Duration::from_millis(19));
        clock.advance();
        assert_eq!(clock.advance_idle(Duration::from_millis(1)), 0);
        assert_eq!(clock.elapsed(), Duration::from_millis(20));
    }

    #[test]
    fn zero_period_falls_back_to_the_sixty_hertz_reference() {
        let clock = FrameClock::from_period_us(Instant::now(), 0);
        assert_eq!(clock.period(), REFERENCE_FRAME_PERIOD);
    }
}
