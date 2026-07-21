// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Optional CPU sampling profiler (`--features profile`, env `MISTER_PPROF=1`).
//!
//! Uses `SIGPROF`/`ITIMER_PROF` sampling from the `pprof` crate — no `perf` CLI required.
//! Build with `scripts/agent build runtime-profile`, run with `MISTER_PPROF=1`, pull the SVG
//! and/or folded stack output.

#[derive(Debug, Clone)]
pub struct CpuProfileSummary {
    pub sample_stacks: usize,
    pub sample_hits: isize,
    pub duration_secs: f64,
    pub hz: i32,
    pub out_path: String,
    pub bytes: u64,
}

#[cfg(feature = "profile")]
mod imp {
    use super::CpuProfileSummary;

    pub struct CpuProfiler {
        guard: pprof::ProfilerGuard<'static>,
        hz: i32,
        out_path: String,
        folded_out_path: Option<String>,
    }

    pub fn start() -> Option<CpuProfiler> {
        if std::env::var("MISTER_PPROF").ok().as_deref() != Some("1") {
            return None;
        }
        let hz = std::env::var("MISTER_PPROF_HZ")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(99);
        let out_path =
            std::env::var("MISTER_PPROF_OUT").unwrap_or_else(|_| "/tmp/mister-pprof.svg".into());
        let folded_out_path = std::env::var("MISTER_PPROF_FOLDED_OUT").ok();
        crate::ui_logln!("cpu_profile: sampling at {hz} Hz → {out_path}");
        if let Some(path) = folded_out_path.as_deref() {
            crate::ui_logln!("cpu_profile: folded stacks → {path}");
        }
        match pprof::ProfilerGuard::new(hz) {
            Ok(guard) => Some(CpuProfiler {
                guard,
                hz,
                out_path,
                folded_out_path,
            }),
            Err(e) => {
                crate::ui_errln!("cpu_profile: ProfilerGuard::new failed: {e}");
                None
            }
        }
    }

    pub fn finish(profiler: Option<CpuProfiler>) -> Result<Option<CpuProfileSummary>, String> {
        let Some(p) = profiler else { return Ok(None) };
        let report = match p.guard.report().build() {
            Ok(r) => r,
            Err(e) => return Err(format!("cpu_profile: report build failed: {e}")),
        };
        let sample_stacks = report.data.len();
        let sample_hits: isize = report.data.values().sum();
        let duration_secs = report.timing.duration.as_secs_f64();
        crate::ui_logln!(
            "cpu_profile: {} unique stacks, {} sample hits, {:.1}s at {} Hz",
            sample_stacks,
            sample_hits,
            duration_secs,
            p.hz
        );
        if sample_hits == 0 {
            return Err(
                "cpu_profile: no CPU samples collected from SIGPROF/ITIMER_PROF timer".into(),
            );
        }
        if let Some(path) = p.folded_out_path.as_deref() {
            write_folded_report(&report, path)?;
        }
        match std::fs::File::create(&p.out_path) {
            Ok(mut file) => {
                if let Err(e) = report.flamegraph(&mut file) {
                    return Err(format!("cpu_profile: flamegraph write failed: {e}"));
                }
                let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
                crate::ui_logln!(
                    "cpu_profile: wrote flamegraph to {} ({bytes} bytes)",
                    p.out_path
                );
                Ok(Some(CpuProfileSummary {
                    sample_stacks,
                    sample_hits,
                    duration_secs,
                    hz: p.hz,
                    out_path: p.out_path,
                    bytes,
                }))
            }
            Err(e) => Err(format!("cpu_profile: create {} failed: {e}", p.out_path)),
        }
    }

    fn write_folded_report(report: &pprof::Report, path: &str) -> Result<u64, String> {
        use std::fmt::Write as _;
        use std::io::Write as _;

        let mut lines: Vec<String> = report
            .data
            .iter()
            .map(|(key, value)| {
                let mut line = key.thread_name_or_id();
                line.push(';');
                for frame in key.frames.iter().rev() {
                    for symbol in frame.iter().rev() {
                        write!(&mut line, "{};", symbol)
                            .expect("writing folded stack line to String cannot fail");
                    }
                }
                line.pop();
                write!(&mut line, " {}", value)
                    .expect("writing folded stack count to String cannot fail");
                line
            })
            .collect();
        lines.sort();

        let mut file = std::fs::File::create(path)
            .map_err(|e| format!("cpu_profile: create folded stack file {path} failed: {e}"))?;
        for line in lines {
            writeln!(file, "{line}")
                .map_err(|e| format!("cpu_profile: write folded stack file {path} failed: {e}"))?;
        }
        let bytes = file
            .metadata()
            .map(|m| m.len())
            .map_err(|e| format!("cpu_profile: stat folded stack file {path} failed: {e}"))?;
        crate::ui_logln!("cpu_profile: wrote folded stacks to {path} ({bytes} bytes)");
        Ok(bytes)
    }
}

#[cfg(feature = "profile")]
pub use imp::{finish, start};

#[cfg(not(feature = "profile"))]
mod stub {
    use super::CpuProfileSummary;

    pub struct CpuProfiler;

    pub fn start() -> Option<CpuProfiler> {
        if std::env::var("MISTER_PPROF").ok().as_deref() == Some("1") {
            crate::ui_errln!(
                "cpu_profile: MISTER_PPROF=1 ignored — rebuild with \
                 `scripts/agent build runtime-profile` (Cargo feature `profile`)"
            );
        }
        None
    }

    pub fn finish(_: Option<CpuProfiler>) -> Result<Option<CpuProfileSummary>, String> {
        Ok(None)
    }
}

#[cfg(not(feature = "profile"))]
pub use stub::{finish, start};
