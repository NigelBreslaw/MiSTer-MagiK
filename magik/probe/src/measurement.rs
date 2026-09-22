// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settled hardware evidence, independent of renderer completion counts.
use mister_magik_latch_contract::{PresentationTelemetry, validate_presentation_telemetry_window};
use mister_magik_tooling_support::measurement::PresentationMetrics;
use std::time::Instant;

#[derive(Default)]
pub struct Evidence {
    previous: Option<(PresentationTelemetry, Instant)>,
    latch_drop: Option<u16>,
}
impl Evidence {
    pub fn observe(
        &mut self,
        sample: PresentationTelemetry,
        latch_drop: u16,
        metrics: &mut PresentationMetrics,
    ) {
        let now = Instant::now();
        if let Some((previous, at)) = self.previous {
            // The conservative 120 Hz bound validates plausibility without assuming 60 Hz.
            match validate_presentation_telemetry_window(
                previous,
                sample,
                now.duration_since(at).as_micros().max(1) as u64,
                8_333,
            ) {
                Ok(delta) => {
                    metrics.counters.drops += u64::from(delta.repeated_vblank_delta);
                    metrics.counters.owned_vblanks += u64::from(delta.owned_vblank_delta);
                    metrics.counters.presented_vblanks += u64::from(delta.presented_vblank_delta);
                }
                Err(error) => metrics.error = Some(error.to_string()),
            }
        } else if !sample.magik_ownership()
            || sample.pending()
            || !sample.lifetime_invariant_valid()
        {
            metrics.error =
                Some("initial presentation telemetry is not owned, settled and valid".into());
        }
        if let Some(previous) = self.latch_drop {
            metrics.counters.latch_drops += u64::from(latch_drop.wrapping_sub(previous));
        }
        self.latch_drop = Some(latch_drop);
        self.previous = Some((sample, now));
        metrics.last_physical_drop_count = Some(latch_drop);
    }
}

pub fn resources(metrics: &mut PresentationMetrics) {
    // SAFETY: both calls initialize the supplied stack records on success.
    let mut clock = unsafe { std::mem::zeroed::<libc::timespec>() };
    if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut clock) } == 0 {
        metrics.process_cpu_us =
            Some(clock.tv_sec as u64 * 1_000_000 + clock.tv_nsec as u64 / 1_000);
    } else {
        metrics.process_cpu_us = None;
        metrics.error = Some("process CPU clock unavailable".into());
    }
    let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } == 0 {
        #[cfg(target_os = "macos")]
        let bytes = usage.ru_maxrss as u64;
        #[cfg(not(target_os = "macos"))]
        let bytes = usage.ru_maxrss as u64 * 1024;
        metrics.peak_rss_bytes = Some(bytes);
    } else {
        metrics.peak_rss_bytes = None;
        metrics.error = Some("process RSS unavailable".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP;
    fn sample(n: u32) -> PresentationTelemetry {
        PresentationTelemetry {
            owned_vblank_count: n,
            presented_vblank_count: n,
            repeated_vblank_count: 0,
            ownership_loss_count: 0,
            active_sequence: 1,
            flags: 1 << STATUS_MAGIK_OWNERSHIP,
            crc: 0,
        }
    }
    #[test]
    fn separates_wrap_and_latch_drops() {
        let mut e = Evidence::default();
        let mut m = PresentationMetrics::default();
        e.observe(sample(u32::MAX), u16::MAX, &mut m);
        e.observe(sample(0), 0, &mut m);
        assert_eq!(m.counters.owned_vblanks, 1);
        assert_eq!(m.counters.drops, 0);
        assert_eq!(m.counters.latch_drops, 1);
        assert!(m.error.is_none());
    }
    #[test]
    fn ownership_loss_invalidates_evidence() {
        let mut e = Evidence::default();
        let mut m = PresentationMetrics::default();
        e.observe(sample(1), 0, &mut m);
        let mut lost = sample(2);
        lost.ownership_loss_count = 1;
        e.observe(lost, 0, &mut m);
        assert!(m.error.is_some());
    }
}
