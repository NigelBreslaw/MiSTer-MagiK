// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded on-device SIGPROF sampling for closed scene-lab profiles.

use serde::Serialize;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const PROFILE_SCHEMA: &str = "mister-magik-scene-lab-pprof-v1";
const PROFILE_SVG_ENV: &str = "MISTER_SCENE_LAB_PPROF_OUT";
const PROFILE_FOLDED_ENV: &str = "MISTER_SCENE_LAB_PPROF_FOLDED_OUT";
const PROFILE_COMPLETE_ENV: &str = "MISTER_SCENE_LAB_PPROF_COMPLETE";

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

#[derive(Clone, Debug, Serialize)]
pub struct CpuProfileSummary {
    pub schema: &'static str,
    pub state: &'static str,
    pub scene: &'static str,
    pub hz: i32,
    pub duration_secs: f64,
    pub sample_hits: isize,
    pub sample_stacks: usize,
    pub svg_bytes: u64,
    pub folded_bytes: u64,
}

#[derive(Clone, Debug)]
struct ProfileArtifacts {
    svg: PathBuf,
    folded: PathBuf,
    complete: PathBuf,
}

impl ProfileArtifacts {
    fn from_environment() -> Result<Option<Self>, String> {
        let svg = std::env::var_os(PROFILE_SVG_ENV).map(PathBuf::from);
        let folded = std::env::var_os(PROFILE_FOLDED_ENV).map(PathBuf::from);
        let complete = std::env::var_os(PROFILE_COMPLETE_ENV).map(PathBuf::from);
        match (svg, folded, complete) {
            (None, None, None) => Ok(None),
            (Some(svg), Some(folded), Some(complete)) => Ok(Some(Self {
                svg,
                folded,
                complete,
            })),
            _ => Err(format!(
                "{PROFILE_SVG_ENV}, {PROFILE_FOLDED_ENV}, and {PROFILE_COMPLETE_ENV} must be supplied together"
            )),
        }
    }
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

pub fn finish(profiler: CpuProfiler) -> Result<CpuProfileSummary, String> {
    let label = profiler.scene.label();
    let artifacts = ProfileArtifacts::from_environment()?;
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
    for (rank, (hits, stack)) in stacks.iter().take(24).enumerate() {
        println!(
            "{label}-profile-stack rank={} hits={} stack={}",
            rank + 1,
            hits,
            stack
        );
    }
    let mut summary = CpuProfileSummary {
        schema: PROFILE_SCHEMA,
        state: "complete",
        scene: label,
        hz: profiler.hz,
        duration_secs: report.timing.duration.as_secs_f64(),
        sample_hits,
        sample_stacks,
        svg_bytes: 0,
        folded_bytes: 0,
    };
    if let Some(artifacts) = artifacts {
        ensure_parent(&artifacts.svg)?;
        ensure_parent(&artifacts.folded)?;
        ensure_parent(&artifacts.complete)?;
        let mut svg = std::fs::File::create(&artifacts.svg)
            .map_err(|error| format!("create {}: {error}", artifacts.svg.display()))?;
        report
            .flamegraph(&mut svg)
            .map_err(|error| format!("write {}: {error}", artifacts.svg.display()))?;
        summary.svg_bytes = svg
            .metadata()
            .map_err(|error| format!("stat {}: {error}", artifacts.svg.display()))?
            .len();

        let mut folded = std::fs::File::create(&artifacts.folded)
            .map_err(|error| format!("create {}: {error}", artifacts.folded.display()))?;
        stacks.sort_unstable_by(|left, right| left.1.cmp(&right.1));
        for (hits, stack) in &stacks {
            writeln!(folded, "{stack} {hits}")
                .map_err(|error| format!("write {}: {error}", artifacts.folded.display()))?;
        }
        summary.folded_bytes = folded
            .metadata()
            .map_err(|error| format!("stat {}: {error}", artifacts.folded.display()))?
            .len();
        write_json_atomic(&artifacts.complete, &summary)?;
    }
    Ok(summary)
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    Ok(())
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let temporary = path.with_extension("json.next");
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("publish {}: {error}", path.display()))
}
