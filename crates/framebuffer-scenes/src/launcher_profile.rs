// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Explicit diagnostic runs only. Disabled runs never read a clock or PMU.
use mister_magik_perf_events as pmu;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
thread_local! {
    static TOTALS: RefCell<BTreeMap<&'static str, Stage>> = const { RefCell::new(BTreeMap::new()) };
    static WORKER: RefCell<Option<Report>> = const { RefCell::new(None) };
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Transfer a drained worker report to the consumer's next measurement window.
/// Only explicit instrumented runs call this; normal frames allocate nothing.
pub fn absorb_worker(report: Report) {
    WORKER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(total) = slot.as_mut() {
            total.merge(report);
        } else {
            *slot = Some(report);
        }
    });
}

#[derive(Default, serde::Serialize)]
pub struct Stage {
    calls: u64,
    wall_ns: u64,
    max_ns: u64,
}

/// Call once before rendering. Counters are sampled every 16 calls per label.
pub fn enable() -> Result<(), &'static str> {
    pmu::install_process_config(pmu::PmuProfileConfig::capture_with(|key| match key {
        "MISTER_PMU_PROFILE" => Some("1"),
        "MISTER_PMU_COUNTER_SET" => Some("cortex-a9-neon"),
        _ => None,
    }))?;
    ENABLED.store(true, Ordering::Relaxed);
    Ok(())
}

pub struct Span {
    label: &'static str,
    started: Instant,
    pmu: Option<pmu::SampledSpan>,
}

pub fn span(label: &'static str) -> Option<Span> {
    ENABLED.load(Ordering::Relaxed).then(|| Span {
        label,
        pmu: pmu::sampled_span(label),
        started: Instant::now(),
    })
}
impl Drop for Span {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed().as_nanos() as u64;
        drop(self.pmu.take());
        TOTALS.with(|totals| {
            let mut totals = totals.borrow_mut();
            let stage = totals.entry(self.label).or_default();
            stage.calls += 1;
            stage.wall_ns += elapsed;
            stage.max_ns = stage.max_ns.max(elapsed);
        });
    }
}

#[derive(serde::Serialize)]
pub struct Report {
    pub stages: BTreeMap<&'static str, Stage>,
    pub hardware: pmu::ThreadProfile,
    pub hardware_stages: BTreeMap<String, HardwareStage>,
}

#[derive(Default, serde::Serialize)]
pub struct HardwareStage {
    samples: u64,
    time_enabled_ns: u64,
    time_running_ns: u64,
    counters: BTreeMap<pmu::HardwareEvent, u64>,
}

impl Report {
    fn merge(&mut self, other: Self) {
        for (label, source) in other.stages {
            let target = self.stages.entry(label).or_default();
            target.calls += source.calls;
            target.wall_ns += source.wall_ns;
            target.max_ns = target.max_ns.max(source.max_ns);
        }
        for (label, source) in other.hardware_stages {
            let target = self.hardware_stages.entry(label).or_default();
            target.samples += source.samples;
            target.time_enabled_ns += source.time_enabled_ns;
            target.time_running_ns += source.time_running_ns;
            for (event, value) in source.counters {
                *target.counters.entry(event).or_default() += value;
            }
        }
        self.hardware.enabled |= other.hardware.enabled;
        self.hardware.attempted_spans += other.hardware.attempted_spans;
        self.hardware.dropped_spans += other.hardware.dropped_spans;
        if self.hardware.failure.is_none() {
            self.hardware.failure = other.hardware.failure;
        }
        // Stage counters are thread-scoped; their sums are CPU work, not
        // elapsed critical-path time. Preserve unavailable scheduling times.
    }
}

/// Drain at window boundaries; never serialize or read procfs in the pixel loop.
pub fn take() -> Report {
    let mut hardware = pmu::take_thread_profile();
    let mut hardware_stages: BTreeMap<String, HardwareStage> = BTreeMap::new();
    // Keep transport size bounded by stage count, not frames sampled. Retain
    // enabled/running times and explicit failures; never silently scale counts.
    for record in hardware.records.drain(..) {
        let stage = hardware_stages.entry(record.name).or_default();
        stage.samples += 1;
        stage.time_enabled_ns += record.counters.time_enabled_ns;
        stage.time_running_ns += record.counters.time_running_ns;
        for (event, value) in record.counters.counters.iter() {
            *stage.counters.entry(event).or_default() += value;
        }
    }
    let mut report = Report {
        stages: TOTALS.with(|totals| std::mem::take(&mut *totals.borrow_mut())),
        hardware,
        hardware_stages,
    };
    WORKER.with(|slot| {
        if let Some(worker) = slot.borrow_mut().take() {
            report.merge(worker);
        }
    });
    report
}
