//! Optional CPU sampling profiler (`--features profile`, env `MISTER_PPROF=1`).
//!
//! Uses `SIGPROF`/`ITIMER_PROF` sampling from the `pprof` crate — no `perf` CLI required.
//! Build with `build-arm.sh --profile`, run with `MISTER_PPROF=1`, pull the SVG.

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
        println!("cpu_profile: sampling at {hz} Hz → {out_path}");
        match pprof::ProfilerGuard::new(hz) {
            Ok(guard) => Some(CpuProfiler {
                guard,
                hz,
                out_path,
            }),
            Err(e) => {
                eprintln!("cpu_profile: ProfilerGuard::new failed: {e}");
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
        println!(
            "cpu_profile: {} unique stacks, {} sample hits, {:.1}s at {} Hz",
            sample_stacks, sample_hits, duration_secs, p.hz
        );
        if sample_hits == 0 {
            return Err(
                "cpu_profile: no CPU samples collected from SIGPROF/ITIMER_PROF timer".into(),
            );
        }
        match std::fs::File::create(&p.out_path) {
            Ok(mut file) => {
                if let Err(e) = report.flamegraph(&mut file) {
                    return Err(format!("cpu_profile: flamegraph write failed: {e}"));
                }
                let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
                println!(
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
}

#[cfg(feature = "profile")]
pub use imp::{finish, start};

#[cfg(not(feature = "profile"))]
mod stub {
    use super::CpuProfileSummary;

    pub struct CpuProfiler;

    pub fn start() -> Option<CpuProfiler> {
        if std::env::var("MISTER_PPROF").ok().as_deref() == Some("1") {
            eprintln!(
                "cpu_profile: MISTER_PPROF=1 ignored — rebuild with \
                 `build-arm.sh --profile` (Cargo feature `profile`)"
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
