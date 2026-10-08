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
    idle_remainder: Duration,
}

impl FrameClock {
    pub fn new(epoch: Instant, period: Duration) -> Self {
        assert!(!period.is_zero(), "frame period must be positive");
        Self {
            epoch,
            period,
            elapsed: Duration::ZERO,
            idle_remainder: Duration::ZERO,
        }
    }

    pub const fn period(&self) -> Duration {
        self.period
    }

    /// Animation time since the epoch.
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

    /// Moves to the next frame. Call once per produced frame, never per
    /// wake-up, so a repeated or missed refresh does not create time.
    pub fn advance(&mut self) {
        self.idle_remainder = Duration::ZERO;
        self.step();
    }

    /// Accounts for wall time that passed without a produced frame, in whole
    /// display periods. The sub-period remainder carries into the next call
    /// and is dropped once a real frame is produced.
    pub fn advance_idle(&mut self, wall: Duration) {
        let total = self.idle_remainder.saturating_add(wall);
        let remainder_ns = total.as_nanos() % self.period.as_nanos();
        self.idle_remainder = Duration::new(
            (remainder_ns / 1_000_000_000) as u64,
            (remainder_ns % 1_000_000_000) as u32,
        );
        self.elapsed = self.elapsed.saturating_add(total - self.idle_remainder);
    }

    fn step(&mut self) {
        self.elapsed = self.elapsed.saturating_add(self.period);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_wall_time_accounts_for_more_than_u32_max_periods() {
        let period = Duration::from_nanos(1);
        let mut clock = FrameClock::new(Instant::now(), period);
        clock.advance_idle(Duration::from_secs(5));
        assert_eq!(clock.elapsed(), Duration::from_secs(5));
        assert_eq!(clock.elapsed_us(), 5_000_000);
        assert_eq!(clock.period(), period);
        clock.advance_idle(Duration::ZERO);
        assert_eq!(clock.elapsed(), Duration::from_secs(5));
    }

    #[test]
    fn idle_wall_time_handles_duration_limits_without_wrapping() {
        let period = Duration::new(u64::MAX / 2, 0);
        let mut clock = FrameClock::new(Instant::now(), period);
        clock.advance_idle(Duration::MAX);
        assert_eq!(clock.elapsed(), period * 2);
        clock.advance_idle(period);
        assert_eq!(clock.elapsed(), Duration::MAX);
    }

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
    fn idle_wall_time_counts_whole_periods_and_carries_the_remainder() {
        let mut clock = FrameClock::new(Instant::now(), Duration::from_millis(20));
        clock.advance_idle(Duration::from_millis(15));
        assert_eq!(clock.elapsed(), Duration::ZERO);
        clock.advance_idle(Duration::from_millis(30));
        assert_eq!(clock.elapsed(), Duration::from_millis(40));
        // 5 ms was left over; 15 more completes the next period.
        clock.advance_idle(Duration::from_millis(15));
        assert_eq!(clock.elapsed(), Duration::from_millis(60));
    }

    #[test]
    fn a_produced_frame_discards_idle_remainder() {
        let mut clock = FrameClock::new(Instant::now(), Duration::from_millis(20));
        clock.advance_idle(Duration::from_millis(19));
        clock.advance();
        clock.advance_idle(Duration::from_millis(1));
        assert_eq!(clock.elapsed(), Duration::from_millis(20));
    }
}
