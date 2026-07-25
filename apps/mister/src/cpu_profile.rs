// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Optional CPU sampling profiler (`--features profile`, env `MISTER_PPROF=1`).
//!
//! Uses `SIGPROF`/`ITIMER_PROF` sampling from the `pprof` crate — no `perf` CLI required.
//! Build with `scripts/agent build runtime-profile`, run with `MISTER_PPROF=1`, pull the SVG
//! and/or folded stack output.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

const SCREENSAVER_TRIGGER: &str = "screensaver";
const DEFAULT_SCREENSAVER_PROFILE_SECS: u64 = 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum ScreensaverProfileState {
    Disabled,
    Waiting,
    Active,
    Complete,
    Failed,
}

impl ScreensaverProfileState {
    const fn label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Waiting => "waiting",
            Self::Active => "active",
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }
}

static SCREENSAVER_PROFILE_STATE: AtomicU8 = AtomicU8::new(ScreensaverProfileState::Disabled as u8);

fn set_screensaver_profile_state(state: ScreensaverProfileState) {
    SCREENSAVER_PROFILE_STATE.store(state as u8, Ordering::Relaxed);
}

pub fn screensaver_profile_state() -> &'static str {
    match SCREENSAVER_PROFILE_STATE.load(Ordering::Relaxed) {
        value if value == ScreensaverProfileState::Waiting as u8 => {
            ScreensaverProfileState::Waiting.label()
        }
        value if value == ScreensaverProfileState::Active as u8 => {
            ScreensaverProfileState::Active.label()
        }
        value if value == ScreensaverProfileState::Complete as u8 => {
            ScreensaverProfileState::Complete.label()
        }
        value if value == ScreensaverProfileState::Failed as u8 => {
            ScreensaverProfileState::Failed.label()
        }
        _ => ScreensaverProfileState::Disabled.label(),
    }
}

fn screensaver_profile_requested() -> bool {
    std::env::var("MISTER_PPROF").ok().as_deref() == Some("1")
        && std::env::var("MISTER_PPROF_TRIGGER").ok().as_deref() == Some(SCREENSAVER_TRIGGER)
}

fn screensaver_profile_duration() -> Duration {
    screensaver_profile_duration_from_value(
        std::env::var("MISTER_PPROF_DURATION_SECS").ok().as_deref(),
    )
}

fn screensaver_profile_duration_from_value(value: Option<&str>) -> Duration {
    Duration::from_secs(
        value
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_SCREENSAVER_PROFILE_SECS)
            .clamp(1, 300),
    )
}

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
    use super::{
        CpuProfileSummary, ScreensaverProfileState, screensaver_profile_duration,
        screensaver_profile_requested, set_screensaver_profile_state,
    };
    use serde_json::json;
    use std::fs;
    use std::time::{Duration, Instant};

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
        if screensaver_profile_requested() {
            return None;
        }
        start_enabled()
    }

    fn start_enabled() -> Option<CpuProfiler> {
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

    enum State {
        Disabled,
        Waiting,
        Active {
            profiler: CpuProfiler,
            started: Instant,
        },
        Complete,
        Failed,
    }

    pub struct ScreensaverProfiler {
        state: State,
        duration: Duration,
        complete_path: Option<String>,
    }

    impl ScreensaverProfiler {
        pub fn from_env() -> Self {
            let requested = screensaver_profile_requested();
            let state = if requested {
                set_screensaver_profile_state(ScreensaverProfileState::Waiting);
                State::Waiting
            } else {
                set_screensaver_profile_state(ScreensaverProfileState::Disabled);
                State::Disabled
            };
            Self {
                state,
                duration: screensaver_profile_duration(),
                complete_path: std::env::var("MISTER_PPROF_COMPLETE").ok(),
            }
        }

        pub fn begin(&mut self) {
            if !matches!(self.state, State::Waiting) {
                return;
            }
            match start_enabled() {
                Some(profiler) => {
                    self.state = State::Active {
                        profiler,
                        started: Instant::now(),
                    };
                    set_screensaver_profile_state(ScreensaverProfileState::Active);
                }
                None => {
                    self.fail("profiler-start-failed");
                }
            }
        }

        pub fn poll(&mut self) {
            let elapsed = match &self.state {
                State::Active { started, .. } => started.elapsed(),
                _ => return,
            };
            if elapsed < self.duration {
                return;
            }
            let state = std::mem::replace(&mut self.state, State::Failed);
            let State::Active { profiler, .. } = state else {
                return;
            };
            match finish(Some(profiler)) {
                Ok(Some(summary)) => {
                    let metadata = json!({
                        "schema": "mister-magik-screensaver-pprof-v1",
                        "state": "complete",
                        "duration_secs": summary.duration_secs,
                        "hz": summary.hz,
                        "sample_stacks": summary.sample_stacks,
                        "sample_hits": summary.sample_hits,
                        "out_path": summary.out_path,
                        "bytes": summary.bytes,
                    });
                    if let Err(error) = self.write_completion(&metadata.to_string()) {
                        self.fail(&format!("completion-write-failed:{error}"));
                        return;
                    }
                    self.state = State::Complete;
                    set_screensaver_profile_state(ScreensaverProfileState::Complete);
                }
                Ok(None) => self.fail("profiler-produced-no-summary"),
                Err(error) => self.fail(&error),
            }
        }

        fn fail(&mut self, error: &str) {
            let metadata = json!({
                "schema": "mister-magik-screensaver-pprof-v1",
                "state": "failed",
                "error": error,
            });
            let _ = self.write_completion(&metadata.to_string());
            self.state = State::Failed;
            set_screensaver_profile_state(ScreensaverProfileState::Failed);
            crate::ui_errln!("screensaver cpu profile failed: {error}");
        }

        fn write_completion(&self, text: &str) -> Result<(), String> {
            let Some(path) = self.complete_path.as_deref() else {
                return Err("MISTER_PPROF_COMPLETE is missing".into());
            };
            if let Some(parent) = std::path::Path::new(path).parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            fs::write(path, format!("{text}\n")).map_err(|error| error.to_string())
        }
    }
}

#[cfg(feature = "profile")]
pub use imp::{ScreensaverProfiler, finish, start};

#[cfg(not(feature = "profile"))]
mod stub {
    use super::{
        CpuProfileSummary, ScreensaverProfileState, screensaver_profile_requested,
        set_screensaver_profile_state,
    };

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

    pub struct ScreensaverProfiler;

    impl ScreensaverProfiler {
        pub fn from_env() -> Self {
            set_screensaver_profile_state(if screensaver_profile_requested() {
                ScreensaverProfileState::Failed
            } else {
                ScreensaverProfileState::Disabled
            });
            Self
        }

        pub fn begin(&mut self) {}

        pub fn poll(&mut self) {}
    }
}

#[cfg(not(feature = "profile"))]
pub use stub::{ScreensaverProfiler, finish, start};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screensaver_profile_duration_is_bounded() {
        assert_eq!(
            screensaver_profile_duration_from_value(Some("0")),
            Duration::from_secs(1)
        );
        assert_eq!(
            screensaver_profile_duration_from_value(Some("999")),
            Duration::from_secs(300)
        );
        assert_eq!(
            screensaver_profile_duration_from_value(None),
            Duration::from_secs(30)
        );
    }
}
