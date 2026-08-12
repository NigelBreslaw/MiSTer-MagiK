// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::{Value, json};

const ON_CPU_TOLERANCE_US: u64 = 250;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ThreadExecutionStamp {
    thread_cpu_us: Option<u64>,
    voluntary_context_switches: Option<u64>,
    involuntary_context_switches: Option<u64>,
    cpu: Option<i32>,
}

impl ThreadExecutionStamp {
    #[must_use]
    pub(super) fn capture() -> Self {
        capture_thread_execution()
    }

    #[must_use]
    pub(super) fn json(self) -> Value {
        json!({
            "thread_cpu_us": self.thread_cpu_us,
            "voluntary_context_switches": self.voluntary_context_switches,
            "involuntary_context_switches": self.involuntary_context_switches,
            "cpu": self.cpu,
        })
    }

    #[must_use]
    pub(super) fn interval_json(self, finished: Self, wall_us: u64) -> Value {
        let thread_cpu_us = self
            .thread_cpu_us
            .zip(finished.thread_cpu_us)
            .map(|(before, after)| after.saturating_sub(before));
        let voluntary_context_switches = self
            .voluntary_context_switches
            .zip(finished.voluntary_context_switches)
            .map(|(before, after)| after.saturating_sub(before));
        let involuntary_context_switches = self
            .involuntary_context_switches
            .zip(finished.involuntary_context_switches)
            .map(|(before, after)| after.saturating_sub(before));
        let off_cpu_us = thread_cpu_us.map(|cpu| wall_us.saturating_sub(cpu.min(wall_us)));
        let classification = classify_execution(
            off_cpu_us,
            voluntary_context_switches,
            involuntary_context_switches,
        );
        json!({
            "classification": classification,
            "wall_us": wall_us,
            "thread_cpu_us": thread_cpu_us,
            "off_cpu_us": off_cpu_us,
            "voluntary_context_switches": voluntary_context_switches,
            "involuntary_context_switches": involuntary_context_switches,
            "cpu_start": self.cpu,
            "cpu_end": finished.cpu,
            "cpu_migrated": self.cpu.zip(finished.cpu).map(|(before, after)| before != after),
        })
    }
}

fn classify_execution(
    off_cpu_us: Option<u64>,
    voluntary_context_switches: Option<u64>,
    involuntary_context_switches: Option<u64>,
) -> &'static str {
    let (Some(off_cpu_us), Some(voluntary), Some(involuntary)) = (
        off_cpu_us,
        voluntary_context_switches,
        involuntary_context_switches,
    ) else {
        return "unavailable";
    };
    if off_cpu_us <= ON_CPU_TOLERANCE_US {
        "on-cpu"
    } else if voluntary > 0 && involuntary > 0 {
        "mixed-off-cpu"
    } else if involuntary > 0 {
        "preempted"
    } else if voluntary > 0 {
        "voluntary-wait"
    } else {
        "off-cpu-unclassified"
    }
}

#[cfg(target_os = "linux")]
fn capture_thread_execution() -> ThreadExecutionStamp {
    let mut cpu_time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `cpu_time` is writable storage and the clock call retains no pointer.
    let cpu_time_available =
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut cpu_time) } == 0;
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `usage` is writable storage and is read only after a successful call.
    let usage_available = unsafe { libc::getrusage(libc::RUSAGE_THREAD, usage.as_mut_ptr()) } == 0;
    let usage = usage_available.then(|| {
        // SAFETY: successful `getrusage` initialized every field in `usage`.
        unsafe { usage.assume_init() }
    });
    // SAFETY: `sched_getcpu` has no pointer arguments and retains no state.
    let cpu = unsafe { libc::sched_getcpu() };
    ThreadExecutionStamp {
        thread_cpu_us: cpu_time_available.then(|| {
            u64::try_from(cpu_time.tv_sec)
                .unwrap_or(0)
                .saturating_mul(1_000_000)
                .saturating_add(u64::try_from(cpu_time.tv_nsec).unwrap_or(0) / 1_000)
        }),
        voluntary_context_switches: usage
            .as_ref()
            .map(|usage| u64::try_from(usage.ru_nvcsw).unwrap_or(0)),
        involuntary_context_switches: usage
            .as_ref()
            .map(|usage| u64::try_from(usage.ru_nivcsw).unwrap_or(0)),
        cpu: (cpu >= 0).then_some(cpu),
    }
}

#[cfg(not(target_os = "linux"))]
fn capture_thread_execution() -> ThreadExecutionStamp {
    ThreadExecutionStamp::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(cpu_us: u64, voluntary: u64, involuntary: u64, cpu: i32) -> ThreadExecutionStamp {
        ThreadExecutionStamp {
            thread_cpu_us: Some(cpu_us),
            voluntary_context_switches: Some(voluntary),
            involuntary_context_switches: Some(involuntary),
            cpu: Some(cpu),
        }
    }

    #[test]
    fn execution_interval_classifies_on_cpu_and_migration() {
        let value = stamp(1_000, 2, 3, 0).interval_json(stamp(1_900, 2, 3, 1), 1_000);
        assert_eq!(value["classification"], "on-cpu");
        assert_eq!(value["off_cpu_us"], 100);
        assert_eq!(value["cpu_migrated"], true);
    }

    #[test]
    fn execution_interval_distinguishes_wait_and_preemption() {
        let voluntary = stamp(1_000, 2, 3, 0).interval_json(stamp(1_100, 3, 3, 0), 1_000);
        assert_eq!(voluntary["classification"], "voluntary-wait");
        let preempted = stamp(1_000, 2, 3, 0).interval_json(stamp(1_100, 2, 4, 0), 1_000);
        assert_eq!(preempted["classification"], "preempted");
        let mixed = stamp(1_000, 2, 3, 0).interval_json(stamp(1_100, 3, 4, 0), 1_000);
        assert_eq!(mixed["classification"], "mixed-off-cpu");
    }

    #[test]
    fn execution_interval_reports_unavailable_stamps() {
        let value =
            ThreadExecutionStamp::default().interval_json(ThreadExecutionStamp::default(), 1_000);
        assert_eq!(value["classification"], "unavailable");
        assert!(value["thread_cpu_us"].is_null());
    }
}
