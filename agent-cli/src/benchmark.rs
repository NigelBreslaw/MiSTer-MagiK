// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::build::{BuildRecipe, BuildSpec};
use crate::device::DeviceClient;
use crate::error::AgentResult;
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::{BenchmarkScenario, ColdBenchmarkScenario, DeviceRequest, Layout};
use serde::Serialize;
use std::path::{Path, PathBuf};

const REMOTE_RUNTIME: &str = "/media/fat/mister-magik-dev/mister-magik-fb";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Select,
    QualifyBuild,
    PrepareDevice,
    Warmup,
    Capture,
    Analyze,
    Evaluate,
    Restore,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::QualifyBuild => "qualify-build",
            Self::PrepareDevice => "prepare-device",
            Self::Warmup => "warmup",
            Self::Capture => "capture",
            Self::Analyze => "analyze",
            Self::Evaluate => "evaluate",
            Self::Restore => "restore",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BenchmarkResult {
    pub scenario: &'static str,
    pub resolution: String,
    pub phase: String,
    pub frames: usize,
    pub average_fps: f64,
    pub p99_work_us: u64,
    pub p99_wall_us: u64,
    pub max_wall_us: u64,
    pub p99_draw_us: u64,
    pub p99_compose_us: u64,
    pub p99_present_us: u64,
    pub present_errors: usize,
    pub vsync_misses: usize,
    pub latch_drop_delta: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColdPhase {
    Inspect,
    SnapshotData,
    EstablishFixture,
    Execute,
    CollectEvents,
    Evaluate,
    Restore,
}

impl ColdPhase {
    fn label(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::SnapshotData => "snapshot-data",
            Self::EstablishFixture => "establish-fixture",
            Self::Execute => "execute",
            Self::CollectEvents => "collect-events",
            Self::Evaluate => "evaluate",
            Self::Restore => "restore",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ColdBenchmarkResult {
    pub scenario: &'static str,
    pub event: String,
    pub elapsed_ms: u64,
    pub status: String,
}

pub trait ColdBenchmarkActions {
    fn run(&mut self, phase: ColdPhase) -> AgentResult<()>;
    fn needs_restore(&self) -> bool;
    fn restore(&mut self) -> AgentResult<()>;
}

pub fn run_cold_workflow(
    actions: &mut dyn ColdBenchmarkActions,
    progress: &mut dyn FnMut(ColdPhase, u8) -> AgentResult<()>,
) -> AgentResult<()> {
    const PHASES: &[(ColdPhase, u8)] = &[
        (ColdPhase::Inspect, 5),
        (ColdPhase::SnapshotData, 20),
        (ColdPhase::EstablishFixture, 35),
        (ColdPhase::Execute, 55),
        (ColdPhase::CollectEvents, 75),
        (ColdPhase::Evaluate, 90),
        (ColdPhase::Restore, 100),
    ];
    crate::workflow::run_restorable_phases(
        actions,
        PHASES,
        progress,
        |actions, phase| actions.run(phase),
        ColdBenchmarkActions::restore,
        |actions| actions.needs_restore(),
        |phase| phase == ColdPhase::Restore,
        ColdPhase::label,
        "cold benchmark",
    )
}

pub trait BenchmarkActions {
    fn run(&mut self, phase: Phase) -> AgentResult<()>;
    fn needs_restore(&self) -> bool;
    fn restore(&mut self) -> AgentResult<()>;
}

pub fn run_workflow(
    actions: &mut dyn BenchmarkActions,
    progress: &mut dyn FnMut(Phase, u8) -> AgentResult<()>,
) -> AgentResult<()> {
    const PHASES: &[(Phase, u8)] = &[
        (Phase::Select, 2),
        (Phase::QualifyBuild, 10),
        (Phase::PrepareDevice, 35),
        (Phase::Warmup, 45),
        (Phase::Capture, 58),
        (Phase::Analyze, 78),
        (Phase::Evaluate, 90),
        (Phase::Restore, 100),
    ];
    crate::workflow::run_restorable_phases(
        actions,
        PHASES,
        progress,
        |actions, phase| actions.run(phase),
        BenchmarkActions::restore,
        |actions| actions.needs_restore(),
        |phase| phase == Phase::Restore,
        Phase::label,
        "benchmark",
    )
}

pub fn infer_scenario(paths: &[PathBuf]) -> AgentResult<BenchmarkScenario> {
    if paths
        .iter()
        .any(|path| path.ends_with("ui_runner/launcher_screensaver.rs"))
    {
        return Ok(BenchmarkScenario::ScreensaverVelocity);
    }
    if paths.iter().any(|path| {
        path.starts_with("mister/platform/runtime/src/framebuffer")
            || path.starts_with("mister/platform/contracts")
            || path.starts_with("mister/platform/kernel")
            || path.starts_with("mister/platform/fpga")
            || path.starts_with("apps/mister/src/ui_runner/launcher_present")
    }) {
        return Ok(BenchmarkScenario::FramebufferVelocity);
    }
    if paths.iter().any(|path| {
        path.starts_with("apps/mister")
            || path.starts_with("crates/catalog")
            || path.starts_with("crates/media-contract")
    }) {
        return Ok(BenchmarkScenario::LauncherVelocity);
    }
    Err("no canonical device benchmark is owned by the changed components".into())
}

pub fn execute(
    repository: &Path,
    paths: &[PathBuf],
    expected_commit: &str,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    if let Some(scenario) = infer_cold_scenario(paths) {
        return execute_cold(repository, expected_commit, scenario, reporter);
    }
    let scenario = infer_scenario(paths)?;
    let mut actions = ProcessActions {
        repository,
        expected_commit,
        scenario,
        build: BuildSpec::for_recipe(BuildRecipe::RuntimeBenchmark),
        snapshot_created: false,
        trace: None,
        results: Vec::new(),
        evaluation_failure: None,
        device: DeviceClient::default(),
    };
    run_workflow(&mut actions, &mut |phase, percent| {
        Ok(reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("benchmark {}", phase.label()),
            Some(percent),
        )?)
    })?;
    if actions.results.is_empty() {
        return Err("benchmark produced no result".into());
    }
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&actions.results).map_err(|error| error.to_string())?,
        Some(100),
    )?;
    if let Some(failure) = actions.evaluation_failure {
        return Err(failure.into());
    }
    Ok(Outcome::Passed)
}

pub fn infer_cold_scenario(paths: &[PathBuf]) -> Option<ColdBenchmarkScenario> {
    if paths.iter().any(|path| {
        let path = path.to_string_lossy();
        path.contains("library") || path.contains("sqlite_catalog")
    }) {
        return Some(ColdBenchmarkScenario::LibraryPersistence);
    }
    if paths.iter().any(|path| {
        path.starts_with("crates/media-contract")
            || path.to_string_lossy().contains("preview")
            || path.to_string_lossy().contains("screenshot")
    }) {
        return Some(ColdBenchmarkScenario::PreviewColdStart);
    }
    if paths.iter().any(|path| {
        path.starts_with("crates/catalog") || path.to_string_lossy().contains("catalog")
    }) {
        return Some(ColdBenchmarkScenario::CatalogLifecycle);
    }
    None
}

fn execute_cold(
    repository: &Path,
    expected_commit: &str,
    scenario: ColdBenchmarkScenario,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    let mut actions = ProcessColdActions {
        repository,
        expected_commit,
        scenario,
        build: BuildSpec::for_recipe(BuildRecipe::RuntimeBenchmark),
        restore_required: false,
        events: None,
        result: None,
        device: DeviceClient::default(),
    };
    run_cold_workflow(&mut actions, &mut |phase, percent| {
        Ok(reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("benchmark {}", phase.label()),
            Some(percent),
        )?)
    })?;
    let result = actions.result.ok_or("cold benchmark produced no result")?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&result).map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

struct ProcessActions<'a> {
    repository: &'a Path,
    expected_commit: &'a str,
    scenario: BenchmarkScenario,
    build: BuildSpec,
    snapshot_created: bool,
    trace: Option<String>,
    results: Vec<BenchmarkResult>,
    evaluation_failure: Option<String>,
    device: DeviceClient,
}

impl ProcessActions<'_> {
    fn qualify_build(&self) -> AgentResult<()> {
        let head = crate::git::value(self.repository, &["rev-parse", "HEAD"])?;
        let dirty = !crate::git::value(self.repository, &["status", "--porcelain"])?.is_empty();
        if head != self.expected_commit || dirty {
            return Err("benchmark requires the exact clean committed source".into());
        }
        crate::build::execute_quiet(self.repository, &self.build)?;
        let receipt = self.build.verify(self.repository)?;
        if receipt.source_commit != self.expected_commit || receipt.source_dirty {
            return Err("benchmark artifact identity does not match the exact commit".into());
        }
        Ok(())
    }

    fn prepare(&mut self) -> AgentResult<()> {
        self.device.execute(DeviceRequest::Discover)?;
        self.device.execute(DeviceRequest::SnapshotRuntime {
            remote: REMOTE_RUNTIME.into(),
        })?;
        self.snapshot_created = true;
        self.device.execute(DeviceRequest::DeployRuntime {
            local: self.build.artifact().to_path_buf(),
            remote: REMOTE_RUNTIME.into(),
        })?;
        self.device
            .execute(DeviceRequest::PrepareBenchmark(self.scenario))?;
        Ok(())
    }

    fn restore_all(&mut self) -> AgentResult<()> {
        if !self.snapshot_created {
            return Ok(());
        }
        let mut errors = Vec::new();
        for request in [
            DeviceRequest::RestoreBenchmark,
            DeviceRequest::RollbackRuntime {
                remote: REMOTE_RUNTIME.into(),
            },
            DeviceRequest::VerifyHealth(Layout::Development),
        ] {
            if let Err(error) = self.device.execute(request) {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            self.snapshot_created = false;
            Ok(())
        } else {
            Err(errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ")
                .into())
        }
    }
}

impl BenchmarkActions for ProcessActions<'_> {
    fn run(&mut self, phase: Phase) -> AgentResult<()> {
        match phase {
            Phase::Select => Ok(()),
            Phase::QualifyBuild => self.qualify_build(),
            Phase::PrepareDevice => self.prepare(),
            Phase::Warmup => self
                .device
                .execute(DeviceRequest::WarmupBenchmark(self.scenario))
                .map(|_| ()),
            Phase::Capture => {
                self.trace = Some(
                    self.device
                        .execute(DeviceRequest::CaptureBenchmark(self.scenario))?,
                );
                Ok(())
            }
            Phase::Analyze => {
                self.results = analyze_trace(
                    self.trace.as_deref().ok_or("benchmark trace is missing")?,
                    self.scenario,
                )?;
                Ok(())
            }
            Phase::Evaluate => {
                if self.results.is_empty() {
                    return Err("benchmark analysis is missing".into());
                }
                if let Err(error) = evaluate(&self.results) {
                    self.evaluation_failure = Some(error.to_string());
                }
                Ok(())
            }
            Phase::Restore => unreachable!("restore has a dedicated action"),
        }
    }

    fn needs_restore(&self) -> bool {
        self.snapshot_created
    }

    fn restore(&mut self) -> AgentResult<()> {
        self.restore_all()
    }
}

struct ProcessColdActions<'a> {
    repository: &'a Path,
    expected_commit: &'a str,
    scenario: ColdBenchmarkScenario,
    build: BuildSpec,
    restore_required: bool,
    events: Option<String>,
    result: Option<ColdBenchmarkResult>,
    device: DeviceClient,
}

impl ProcessColdActions<'_> {
    fn inspect(&self) -> AgentResult<()> {
        let head = crate::git::value(self.repository, &["rev-parse", "HEAD"])?;
        let dirty = !crate::git::value(self.repository, &["status", "--porcelain"])?.is_empty();
        if head != self.expected_commit || dirty {
            return Err("benchmark requires the exact clean committed source".into());
        }
        crate::build::execute_quiet(self.repository, &self.build)?;
        let receipt = self.build.verify(self.repository)?;
        if receipt.source_commit != self.expected_commit || receipt.source_dirty {
            return Err("benchmark artifact identity does not match the exact commit".into());
        }
        Ok(())
    }

    fn snapshot(&mut self) -> AgentResult<()> {
        self.device.execute(DeviceRequest::Discover)?;
        self.device.execute(DeviceRequest::SnapshotRuntime {
            remote: REMOTE_RUNTIME.into(),
        })?;
        self.restore_required = true;
        self.device.execute(DeviceRequest::DeployRuntime {
            local: self.build.artifact().to_path_buf(),
            remote: REMOTE_RUNTIME.into(),
        })?;
        self.device
            .execute(DeviceRequest::SnapshotBenchmarkData(self.scenario))?;
        Ok(())
    }

    fn restore_all(&mut self) -> AgentResult<()> {
        if !self.restore_required {
            return Ok(());
        }
        let mut errors = Vec::new();
        for request in [
            DeviceRequest::RestoreBenchmarkData(self.scenario),
            DeviceRequest::RollbackRuntime {
                remote: REMOTE_RUNTIME.into(),
            },
            DeviceRequest::VerifyHealth(Layout::Development),
        ] {
            if let Err(error) = self.device.execute(request) {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            self.restore_required = false;
            Ok(())
        } else {
            Err(errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ")
                .into())
        }
    }
}

impl ColdBenchmarkActions for ProcessColdActions<'_> {
    fn run(&mut self, phase: ColdPhase) -> AgentResult<()> {
        match phase {
            ColdPhase::Inspect => self.inspect(),
            ColdPhase::SnapshotData => self.snapshot(),
            ColdPhase::EstablishFixture => self
                .device
                .execute(DeviceRequest::EstablishBenchmarkFixture(self.scenario))
                .map(|_| ()),
            ColdPhase::Execute => self
                .device
                .execute(DeviceRequest::ExecuteColdBenchmark(self.scenario))
                .map(|_| ()),
            ColdPhase::CollectEvents => {
                self.events = Some(
                    self.device
                        .execute(DeviceRequest::CollectBenchmarkEvents(self.scenario))?,
                );
                Ok(())
            }
            ColdPhase::Evaluate => {
                self.result = Some(analyze_cold_events(
                    self.events
                        .as_deref()
                        .ok_or("cold benchmark events are missing")?,
                    self.scenario,
                )?);
                Ok(())
            }
            ColdPhase::Restore => unreachable!("restore has a dedicated action"),
        }
    }

    fn needs_restore(&self) -> bool {
        self.restore_required
    }

    fn restore(&mut self) -> AgentResult<()> {
        self.restore_all()
    }
}

fn analyze_cold_events(
    text: &str,
    scenario: ColdBenchmarkScenario,
) -> AgentResult<ColdBenchmarkResult> {
    let expected = match scenario {
        ColdBenchmarkScenario::CatalogLifecycle => "catalog_lifecycle_complete",
        ColdBenchmarkScenario::PreviewColdStart => "preview_cold_start_complete",
        ColdBenchmarkScenario::LibraryPersistence => "library_persistence_complete",
    };
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let event: serde_json::Value =
            serde_json::from_str(line).map_err(|error| format!("invalid device event: {error}"))?;
        if event.get("event").and_then(|value| value.as_str()) != Some(expected) {
            continue;
        }
        let status = event
            .get("status")
            .and_then(|value| value.as_str())
            .ok_or("cold benchmark event has no status")?;
        if status != "ok" {
            return Err(format!("cold benchmark event status is {status}").into());
        }
        return Ok(ColdBenchmarkResult {
            scenario: cold_scenario_label(scenario),
            event: expected.into(),
            elapsed_ms: event
                .get("elapsed_ms")
                .and_then(serde_json::Value::as_u64)
                .ok_or("cold benchmark event has no elapsed_ms")?,
            status: status.into(),
        });
    }
    Err(format!("required structured event {expected} is missing").into())
}

#[derive(Clone, Copy)]
struct TraceSample {
    wall_us: u64,
    work_us: u64,
    draw_us: u64,
    compose_us: u64,
    present_us: u64,
    present_error: bool,
    vsync_miss: bool,
    latch_drop_count: u16,
}

fn analyze_trace(text: &str, scenario: BenchmarkScenario) -> AgentResult<Vec<BenchmarkResult>> {
    if scenario != BenchmarkScenario::ScreensaverVelocity {
        let samples = parse_trace_samples(text, false)?;
        return Ok(vec![summarize_samples(
            scenario, "active", "overall", &samples,
        )?]);
    }

    let mut results = Vec::new();
    let mut resolution = None;
    let mut section = String::new();
    for line in text.lines() {
        if let Some(marker) = line.strip_prefix("benchmark_resolution\t") {
            if let Some(previous) = resolution.take() {
                summarize_screensaver_section(previous, &section, &mut results)?;
                section.clear();
            }
            resolution = marker
                .split('\t')
                .find_map(|field| field.strip_prefix("output="))
                .map(str::to_string);
        } else if resolution.is_some() {
            section.push_str(line);
            section.push('\n');
        }
    }
    if let Some(previous) = resolution {
        summarize_screensaver_section(previous, &section, &mut results)?;
    }
    if results.is_empty() {
        return Err("screensaver benchmark contains no resolution sections".into());
    }
    Ok(results)
}

fn summarize_screensaver_section(
    resolution: String,
    text: &str,
    results: &mut Vec<BenchmarkResult>,
) -> AgentResult<()> {
    let samples = parse_trace_samples(text, true)?;
    if samples.len() < 300 {
        return Err(format!(
            "screensaver benchmark {resolution} contains only {} active frames",
            samples.len()
        )
        .into());
    }
    let startup_end = 180.min(samples.len());
    results.push(summarize_samples(
        BenchmarkScenario::ScreensaverVelocity,
        &resolution,
        "overall",
        &samples,
    )?);
    results.push(summarize_samples(
        BenchmarkScenario::ScreensaverVelocity,
        &resolution,
        "startup-first-180",
        &samples[..startup_end],
    )?);
    results.push(summarize_samples(
        BenchmarkScenario::ScreensaverVelocity,
        &resolution,
        "steady",
        &samples[startup_end..],
    )?);
    Ok(())
}

fn parse_trace_samples(text: &str, screensaver_only: bool) -> AgentResult<Vec<TraceSample>> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header_line = lines
        .find(|line| line.split('\t').any(|field| field == "wall_us"))
        .ok_or("benchmark trace has no frame header")?;
    let header: Vec<_> = header_line.split('\t').collect();
    let column = |name: &str| {
        header
            .iter()
            .position(|field| *field == name)
            .ok_or_else(|| format!("benchmark trace is missing {name}"))
    };
    let wall_index = column("wall_us")?;
    let prepare_index = column("prepare_us")?;
    let draw_index = column("slint_render_us")?;
    let custom_index = column("custom_draw_us")?;
    let compose_index = column("hidden_compose_us")?;
    let present_index = column("fb_present_us")?;
    let status_index = header
        .iter()
        .position(|field| *field == "main_present_status");
    let miss_index = header
        .iter()
        .position(|field| *field == "vsync_miss_streak");
    let drop_index = header
        .iter()
        .position(|field| *field == "main_present_drop_count");
    let screensaver_index = header
        .iter()
        .position(|field| *field == "screensaver_active");
    let mut samples = Vec::new();
    for line in lines {
        let fields: Vec<_> = line.split('\t').collect();
        if screensaver_only
            && screensaver_index
                .and_then(|index| fields.get(index))
                .is_none_or(|value| *value != "1")
        {
            continue;
        }
        let Some(wall) = fields
            .get(wall_index)
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        let value = |index: usize| {
            fields
                .get(index)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_default()
        };
        let draw_us = value(draw_index);
        let compose_us = value(compose_index);
        let present_us = value(present_index);
        samples.push(TraceSample {
            wall_us: wall,
            work_us: value(prepare_index)
                .saturating_add(draw_us)
                .saturating_add(value(custom_index))
                .saturating_add(compose_us)
                .saturating_add(present_us),
            draw_us,
            compose_us,
            present_us,
            present_error: status_index
                .and_then(|index| fields.get(index))
                .is_some_and(|status| !matches!(*status, "ok" | "latched" | "presented")),
            vsync_miss: miss_index.map(|index| value(index) > 0).unwrap_or(false),
            latch_drop_count: drop_index
                .map(|index| value(index) as u16)
                .unwrap_or_default(),
        });
    }
    Ok(samples)
}

fn summarize_samples(
    scenario: BenchmarkScenario,
    resolution: &str,
    phase: &str,
    samples: &[TraceSample],
) -> AgentResult<BenchmarkResult> {
    if samples.len() < 120 {
        return Err(format!(
            "benchmark {resolution} {phase} contains only {} usable frames",
            samples.len()
        )
        .into());
    }
    let mut walls = samples
        .iter()
        .map(|sample| sample.wall_us)
        .collect::<Vec<_>>();
    let mut work = samples
        .iter()
        .map(|sample| sample.work_us)
        .collect::<Vec<_>>();
    let mut draw = samples
        .iter()
        .map(|sample| sample.draw_us)
        .collect::<Vec<_>>();
    let mut compose = samples
        .iter()
        .map(|sample| sample.compose_us)
        .collect::<Vec<_>>();
    let mut present = samples
        .iter()
        .map(|sample| sample.present_us)
        .collect::<Vec<_>>();
    let average_wall = walls.iter().sum::<u64>() as f64 / walls.len() as f64;
    Ok(BenchmarkResult {
        scenario: scenario_label(scenario),
        resolution: resolution.into(),
        phase: phase.into(),
        frames: walls.len(),
        average_fps: 1_000_000.0 / average_wall,
        p99_work_us: percentile(&mut work, 99),
        p99_wall_us: percentile(&mut walls, 99),
        max_wall_us: walls.iter().copied().max().unwrap_or_default(),
        p99_draw_us: percentile(&mut draw, 99),
        p99_compose_us: percentile(&mut compose, 99),
        p99_present_us: percentile(&mut present, 99),
        present_errors: samples.iter().filter(|sample| sample.present_error).count(),
        vsync_misses: samples.iter().filter(|sample| sample.vsync_miss).count(),
        latch_drop_delta: samples
            .last()
            .zip(samples.first())
            .map(|(last, first)| last.latch_drop_count.saturating_sub(first.latch_drop_count))
            .unwrap_or_default(),
    })
}

fn evaluate(results: &[BenchmarkResult]) -> AgentResult<()> {
    let mut failures = Vec::new();
    for result in results.iter().filter(|result| result.phase != "steady") {
        let label = format!("{} {}", result.resolution, result.phase);
        if result.average_fps < 55.0 {
            failures.push(format!("{label} average_fps={:.1}<55", result.average_fps));
        }
        if result.p99_work_us > 14_500 {
            failures.push(format!("{label} p99_work_us={}>14500", result.p99_work_us));
        }
        if result.p99_wall_us > 16_000 {
            failures.push(format!("{label} p99_wall_us={}>16000", result.p99_wall_us));
        }
        if result.max_wall_us > 16_667 {
            failures.push(format!("{label} max_wall_us={}>16667", result.max_wall_us));
        }
        if result.present_errors != 0 {
            failures.push(format!("{label} present_errors={}", result.present_errors));
        }
        if result.latch_drop_delta != 0 {
            failures.push(format!(
                "{label} latch_drop_delta={}",
                result.latch_drop_delta
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("benchmark gate failed: {}", failures.join(", ")).into())
    }
}

fn percentile(values: &mut [u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let index = (values.len() - 1) * percentile / 100;
    values[index]
}

fn scenario_label(scenario: BenchmarkScenario) -> &'static str {
    match scenario {
        BenchmarkScenario::LauncherVelocity => "launcher-velocity",
        BenchmarkScenario::FramebufferVelocity => "framebuffer-velocity",
        BenchmarkScenario::ScreensaverVelocity => "screensaver-velocity",
    }
}

fn cold_scenario_label(scenario: ColdBenchmarkScenario) -> &'static str {
    match scenario {
        ColdBenchmarkScenario::CatalogLifecycle => "catalog-lifecycle",
        ColdBenchmarkScenario::PreviewColdStart => "preview-cold-start",
        ColdBenchmarkScenario::LibraryPersistence => "library-persistence",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeActions {
        fail_at: Option<Phase>,
        prepared: bool,
        restored: usize,
        restore_fails: bool,
    }

    #[derive(Default)]
    struct FakeColdActions {
        fail_at: Option<ColdPhase>,
        snapshot_started: bool,
        restored: usize,
        restore_fails: bool,
    }

    impl ColdBenchmarkActions for FakeColdActions {
        fn run(&mut self, phase: ColdPhase) -> AgentResult<()> {
            if phase == ColdPhase::SnapshotData {
                self.snapshot_started = true;
            }
            if self.fail_at == Some(phase) {
                Err("injected failure".into())
            } else {
                Ok(())
            }
        }

        fn needs_restore(&self) -> bool {
            self.snapshot_started
        }

        fn restore(&mut self) -> AgentResult<()> {
            self.restored += 1;
            self.snapshot_started = false;
            if self.restore_fails {
                Err("restore failed".into())
            } else {
                Ok(())
            }
        }
    }

    impl BenchmarkActions for FakeActions {
        fn run(&mut self, phase: Phase) -> AgentResult<()> {
            if phase == Phase::PrepareDevice {
                self.prepared = true;
            }
            if self.fail_at == Some(phase) {
                Err("injected failure".into())
            } else {
                Ok(())
            }
        }

        fn needs_restore(&self) -> bool {
            self.prepared
        }

        fn restore(&mut self) -> AgentResult<()> {
            self.restored += 1;
            self.prepared = false;
            if self.restore_fails {
                Err("restore failed".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn scenario_inference_is_component_owned() {
        assert_eq!(
            infer_scenario(&[PathBuf::from(
                "mister/platform/runtime/src/framebuffer/copy.rs"
            )])
            .unwrap(),
            BenchmarkScenario::FramebufferVelocity
        );
        assert_eq!(
            infer_scenario(&[PathBuf::from(
                "mister/platform/contracts/scanout/src/lib.rs"
            )])
            .unwrap(),
            BenchmarkScenario::FramebufferVelocity
        );
        assert_eq!(
            infer_scenario(&[PathBuf::from("apps/mister/src/launcher.rs")]).unwrap(),
            BenchmarkScenario::LauncherVelocity
        );
        assert_eq!(
            infer_scenario(&[PathBuf::from(
                "apps/mister/src/ui_runner/launcher_screensaver.rs"
            )])
            .unwrap(),
            BenchmarkScenario::ScreensaverVelocity
        );
        assert!(infer_scenario(&[PathBuf::from("docs/device.md")]).is_err());
        assert_eq!(
            infer_cold_scenario(&[PathBuf::from("crates/catalog/src/builder.rs")]),
            Some(ColdBenchmarkScenario::CatalogLifecycle)
        );
        assert_eq!(
            infer_cold_scenario(&[PathBuf::from("apps/mister/src/preview_worker.rs")]),
            Some(ColdBenchmarkScenario::PreviewColdStart)
        );
        assert_eq!(
            infer_cold_scenario(&[PathBuf::from("apps/mister/src/library_store.rs")]),
            Some(ColdBenchmarkScenario::LibraryPersistence)
        );
    }

    #[test]
    fn failure_and_cancellation_restore_after_device_preparation() {
        for fail_at in [
            Phase::Warmup,
            Phase::Capture,
            Phase::Analyze,
            Phase::Evaluate,
        ] {
            let mut actions = FakeActions {
                fail_at: Some(fail_at),
                ..FakeActions::default()
            };
            let error = run_workflow(&mut actions, &mut |_, _| Ok(())).unwrap_err();
            assert!(error.to_string().contains("restore=complete"));
            assert_eq!(actions.restored, 1);
        }
        let mut actions = FakeActions::default();
        let error = run_workflow(&mut actions, &mut |phase, _| {
            if phase == Phase::Capture {
                Err("interrupted".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(error.to_string().starts_with("cancelled:"));
        assert_eq!(actions.restored, 1);
    }

    #[test]
    fn analyzer_and_fixed_thresholds_are_strict() {
        let mut trace = String::from(
            "frame\tprepare_us\tslint_render_us\tcustom_draw_us\thidden_compose_us\tfb_present_us\twall_us\tmain_present_status\n",
        );
        for frame in 0..120 {
            trace.push_str(&format!("{frame}\t100\t200\t100\t100\t100\t15000\tok\n"));
        }
        let result = analyze_trace(&trace, BenchmarkScenario::LauncherVelocity).unwrap();
        assert!(evaluate(&result).is_ok());
        let slow = trace.replace("\t15000\tok", "\t20000\tok");
        assert!(
            evaluate(&analyze_trace(&slow, BenchmarkScenario::LauncherVelocity).unwrap()).is_err()
        );
        assert!(analyze_trace("bad", BenchmarkScenario::LauncherVelocity).is_err());
    }

    #[test]
    fn screensaver_analyzer_reports_each_resolution_and_startup_window() {
        let mut trace = String::new();
        for (mode, output, framebuffer) in [
            ("hdmi-1920x1200p60", "1920x1200", "960x600"),
            ("hdmi-1280x720p60", "1280x720", "1280x720"),
        ] {
            trace.push_str(&format!(
                "benchmark_resolution\tmode={mode}\toutput={output}\tframebuffer={framebuffer}\n"
            ));
            trace.push_str("frame\tprepare_us\tslint_render_us\tcustom_draw_us\thidden_compose_us\tfb_present_us\twall_us\tmain_present_status\tvsync_miss_streak\tmain_present_drop_count\tscreensaver_active\n");
            for frame in 0..360 {
                trace.push_str(&format!(
                    "{frame}\t100\t200\t100\t100\t100\t15000\tok\t0\t0\t1\n"
                ));
            }
        }

        let results = analyze_trace(&trace, BenchmarkScenario::ScreensaverVelocity).unwrap();

        assert_eq!(results.len(), 6);
        assert_eq!(results[0].resolution, "1920x1200");
        assert_eq!(results[1].phase, "startup-first-180");
        assert_eq!(results[2].phase, "steady");
        assert_eq!(results[3].resolution, "1280x720");
        assert!(evaluate(&results).is_ok());
    }

    #[test]
    fn restore_failure_requires_recovery() {
        let mut actions = FakeActions {
            fail_at: Some(Phase::Capture),
            restore_fails: true,
            ..FakeActions::default()
        };
        assert!(
            run_workflow(&mut actions, &mut |_, _| Ok(()))
                .unwrap_err()
                .is_recovery_required()
        );
    }

    #[test]
    fn cold_failures_and_cancellation_restore_production_data() {
        for fail_at in [
            ColdPhase::SnapshotData,
            ColdPhase::EstablishFixture,
            ColdPhase::Execute,
            ColdPhase::CollectEvents,
            ColdPhase::Evaluate,
        ] {
            let mut actions = FakeColdActions {
                fail_at: Some(fail_at),
                ..FakeColdActions::default()
            };
            let error = run_cold_workflow(&mut actions, &mut |_, _| Ok(())).unwrap_err();
            assert!(error.to_string().contains("restore=complete"));
            assert_eq!(actions.restored, 1);
        }
        let mut actions = FakeColdActions::default();
        let error = run_cold_workflow(&mut actions, &mut |phase, _| {
            if phase == ColdPhase::Execute {
                Err("interrupted".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(error.to_string().starts_with("cancelled:"));
        assert_eq!(actions.restored, 1);
    }

    #[test]
    fn cold_events_are_structured_and_scenario_specific() {
        let result = analyze_cold_events(
            r#"{"event":"catalog_lifecycle_complete","elapsed_ms":1234,"status":"ok"}"#,
            ColdBenchmarkScenario::CatalogLifecycle,
        )
        .unwrap();
        assert_eq!(result.elapsed_ms, 1234);
        assert_eq!(result.status, "ok");
        assert!(
            analyze_cold_events(
                r#"{"event":"preview_cold_start_complete","elapsed_ms":1,"status":"failed"}"#,
                ColdBenchmarkScenario::PreviewColdStart,
            )
            .is_err()
        );
    }

    #[test]
    fn cold_restore_failure_requires_recovery() {
        let mut actions = FakeColdActions {
            fail_at: Some(ColdPhase::Execute),
            restore_fails: true,
            ..FakeColdActions::default()
        };
        assert!(
            run_cold_workflow(&mut actions, &mut |_, _| Ok(()))
                .unwrap_err()
                .is_recovery_required()
        );
    }
}
