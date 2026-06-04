//! Optional CPU sampling profiler (`--features profile`, env `MISTER_PPROF=1`).
//!
//! Uses Linux `perf_event_open` from the `pprof` crate — no `perf` CLI required.
//! Build with `build-arm.sh --profile`, run with `MISTER_PPROF=1`, pull the SVG.

#[cfg(feature = "profile")]
mod imp {
    use std::io::Write;

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
        let out_path = std::env::var("MISTER_PPROF_OUT")
            .unwrap_or_else(|_| "/tmp/mister-pprof.svg".into());
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

    pub fn finish(profiler: Option<CpuProfiler>) {
        let Some(p) = profiler else { return };
        let report = match p.guard.report().build() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("cpu_profile: report build failed: {e}");
                return;
            }
        };
        let sample_stacks = report.data.len();
        let sample_hits: isize = report.data.values().sum();
        println!(
            "cpu_profile: {} unique stacks, {} sample hits, {:.1}s at {} Hz",
            sample_stacks,
            sample_hits,
            report.timing.duration.as_secs_f64(),
            p.hz
        );
        if sample_hits == 0 {
            eprintln!(
                "cpu_profile: no CPU samples collected — check perf_event_paranoid \
                 (try: echo -1 > /proc/sys/kernel/perf_event_paranoid)"
            );
            return;
        }
        match std::fs::File::create(&p.out_path) {
            Ok(mut file) => {
                if let Err(e) = report.flamegraph(&mut file) {
                    eprintln!("cpu_profile: flamegraph write failed: {e}");
                    return;
                }
                let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
                println!("cpu_profile: wrote flamegraph to {} ({bytes} bytes)", p.out_path);
            }
            Err(e) => eprintln!("cpu_profile: create {} failed: {e}", p.out_path),
        }
    }
}

#[cfg(feature = "profile")]
pub use imp::{finish, start};

#[cfg(not(feature = "profile"))]
mod stub {
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

    pub fn finish(_: Option<CpuProfiler>) {}
}

#[cfg(not(feature = "profile"))]
pub use stub::{finish, start};
