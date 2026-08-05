// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded on-device SIGPROF sampling for closed scene-lab profiles.

use std::fmt::Write as _;

pub struct CpuProfiler {
    guard: pprof::ProfilerGuard<'static>,
    hz: i32,
    scene: CpuProfileScene,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuProfileScene {
    Cabinet,
    CardFlip,
}

impl CpuProfileScene {
    const fn label(self) -> &'static str {
        match self {
            Self::Cabinet => "cabinet",
            Self::CardFlip => "card-flip",
        }
    }
}

pub fn start(scene: CpuProfileScene) -> Result<CpuProfiler, String> {
    // SAFETY: no profiling timer is active yet; this gives pprof a harmless
    // SIGPROF disposition to restore after its bounded session.
    if unsafe { libc::signal(libc::SIGPROF, libc::SIG_IGN) } == libc::SIG_ERR {
        return Err(format!(
            "make between-session SIGPROF disposition safe: {}",
            std::io::Error::last_os_error()
        ));
    }
    let hz = 99;
    let guard = pprof::ProfilerGuard::new(hz)
        .map_err(|error| format!("start {hz} Hz {} CPU profile: {error}", scene.label()))?;
    Ok(CpuProfiler { guard, hz, scene })
}

pub fn finish(profiler: CpuProfiler) -> Result<(), String> {
    let label = profiler.scene.label();
    let report = profiler
        .guard
        .report()
        .build()
        .map_err(|error| format!("build {label} CPU profile: {error}"))?;
    let sample_stacks = report.data.len();
    let sample_hits: isize = report.data.values().sum();
    if sample_hits == 0 {
        return Err(format!("{label} CPU profile collected no samples"));
    }
    let mut stacks = report
        .data
        .iter()
        .map(|(key, hits)| {
            let mut stack = key.thread_name_or_id();
            for frame in key.frames.iter().rev() {
                for symbol in frame.iter().rev() {
                    write!(&mut stack, ";{symbol}")
                        .expect("formatting a sampled stack into String cannot fail");
                }
            }
            (*hits, stack)
        })
        .collect::<Vec<_>>();
    stacks.sort_unstable_by(|left, right| right.0.cmp(&left.0));
    println!(
        "{label}-profile hz={} duration_secs={:.3} sample_hits={} sample_stacks={}",
        profiler.hz,
        report.timing.duration.as_secs_f64(),
        sample_hits,
        sample_stacks,
    );
    for (rank, (hits, stack)) in stacks.into_iter().take(24).enumerate() {
        println!(
            "{label}-profile-stack rank={} hits={} stack={}",
            rank + 1,
            hits,
            stack
        );
    }
    Ok(())
}
