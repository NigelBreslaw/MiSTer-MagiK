// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lightweight memory-pressure sampling for the launcher.

use std::time::{Duration, Instant};

const DEFAULT_LOW_MEMORY_AVAILABLE_KIB: u64 = 256 * 1024;
const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MemoryPressureSample {
    pub(crate) available_kib: u64,
    pub(crate) threshold_kib: u64,
    pub(crate) active: bool,
    pub(crate) changed: bool,
}

#[derive(Debug)]
pub(crate) struct MemoryPressureGuard {
    threshold_kib: u64,
    sample_interval: Duration,
    last_sample_at: Option<Instant>,
    active: bool,
}

impl MemoryPressureGuard {
    pub(crate) fn from_env() -> Self {
        let threshold_kib = std::env::var("MISTER_LOW_MEMORY_AVAILABLE_KIB")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_LOW_MEMORY_AVAILABLE_KIB);
        Self {
            threshold_kib,
            sample_interval: DEFAULT_SAMPLE_INTERVAL,
            last_sample_at: None,
            active: false,
        }
    }

    pub(crate) fn tick(&mut self, now: Instant) -> Option<MemoryPressureSample> {
        if self
            .last_sample_at
            .is_some_and(|last| now.saturating_duration_since(last) < self.sample_interval)
        {
            return None;
        }
        self.last_sample_at = Some(now);
        self.sample_with_available_kib(mem_available_kib()?)
    }

    fn sample_with_available_kib(&mut self, available_kib: u64) -> Option<MemoryPressureSample> {
        let active = available_kib < self.threshold_kib;
        let changed = active != self.active;
        self.active = active;
        Some(MemoryPressureSample {
            available_kib,
            threshold_kib: self.threshold_kib,
            active,
            changed,
        })
    }

    pub(crate) fn active(&self) -> bool {
        self.active
    }
}

fn mem_available_kib() -> Option<u64> {
    parse_mem_available_kib(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

fn parse_mem_available_kib(meminfo: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemAvailable:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mem_available_from_proc_meminfo() {
        assert_eq!(
            parse_mem_available_kib("MemTotal: 123 kB\nMemAvailable:     456 kB\n"),
            Some(456)
        );
        assert_eq!(parse_mem_available_kib("MemTotal: 123 kB\n"), None);
    }

    #[test]
    fn guard_reports_threshold_crossing_changes() {
        let mut guard = MemoryPressureGuard {
            threshold_kib: 100,
            sample_interval: Duration::from_secs(1),
            last_sample_at: None,
            active: false,
        };
        let now = Instant::now();

        let healthy = guard.sample_with_available_kib(150).unwrap();
        assert_eq!(
            healthy,
            MemoryPressureSample {
                available_kib: 150,
                threshold_kib: 100,
                active: false,
                changed: false,
            }
        );

        let low = guard.sample_with_available_kib(50).unwrap();
        assert_eq!(
            low,
            MemoryPressureSample {
                available_kib: 50,
                threshold_kib: 100,
                active: true,
                changed: true,
            }
        );

        guard.last_sample_at = Some(now);
        assert!(guard
            .last_sample_at
            .is_some_and(|last| now.saturating_duration_since(last) < guard.sample_interval));
    }
}
