// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::build::{BuildIntent, BuildSpec};
use crate::device::DeviceClient;
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::{BenchmarkScenario, DeviceRequest, Layout};
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
    pub frames: usize,
    pub average_fps: f64,
    pub p99_work_us: u64,
    pub p99_wall_us: u64,
    pub max_wall_us: u64,
    pub present_errors: usize,
}

pub trait BenchmarkActions {
    fn run(&mut self, phase: Phase) -> Result<(), String>;
    fn needs_restore(&self) -> bool;
    fn restore(&mut self) -> Result<(), String>;
}

pub fn run_workflow(
    actions: &mut dyn BenchmarkActions,
    progress: &mut dyn FnMut(Phase, u8) -> Result<(), String>,
) -> Result<(), String> {
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
    for (phase, percent) in PHASES {
        if let Err(error) = progress(*phase, *percent) {
            return restore_after_error(actions, format!("cancelled: {error}"));
        }
        let result = if *phase == Phase::Restore {
            actions.restore()
        } else {
            actions.run(*phase)
        };
        if let Err(error) = result {
            return restore_after_error(actions, format!("{}: {error}", phase.label()));
        }
    }
    Ok(())
}

fn restore_after_error(actions: &mut dyn BenchmarkActions, error: String) -> Result<(), String> {
    if !actions.needs_restore() {
        return Err(error);
    }
    match actions.restore() {
        Ok(()) => Err(format!("{error}; restore=complete")),
        Err(restore) => Err(format!(
            "recovery_required: {error}; benchmark restore failed ({restore})"
        )),
    }
}

pub fn infer_scenario(paths: &[PathBuf]) -> Result<BenchmarkScenario, String> {
    if paths.iter().any(|path| {
        path.starts_with("mister/platform/runtime/src/framebuffer")
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
) -> Result<Outcome, String> {
    let scenario = infer_scenario(paths)?;
    let mut actions = ProcessActions {
        repository,
        expected_commit,
        scenario,
        build: BuildSpec::infer(BuildIntent::RuntimeBenchmark)?,
        snapshot_created: false,
        trace: None,
        result: None,
        device: DeviceClient::default(),
    };
    run_workflow(&mut actions, &mut |phase, percent| {
        reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("benchmark {}", phase.label()),
            Some(percent),
        )
    })?;
    let result = actions.result.ok_or("benchmark produced no result")?;
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
    result: Option<BenchmarkResult>,
    device: DeviceClient,
}

impl ProcessActions<'_> {
    fn qualify_build(&self) -> Result<(), String> {
        let head = git_value(self.repository, &["rev-parse", "HEAD"])?;
        let dirty = !git_value(self.repository, &["status", "--porcelain"])?.is_empty();
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

    fn prepare(&mut self) -> Result<(), String> {
        self.device.execute(DeviceRequest::Discover)?;
        self.device.execute(DeviceRequest::SnapshotRuntime {
            remote: REMOTE_RUNTIME.into(),
        })?;
        self.snapshot_created = true;
        self.device.execute(DeviceRequest::DeployRuntime {
            local: self.build.artifact.clone(),
            remote: REMOTE_RUNTIME.into(),
        })?;
        self.device
            .execute(DeviceRequest::PrepareBenchmark(self.scenario))?;
        Ok(())
    }

    fn restore_all(&mut self) -> Result<(), String> {
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
            Err(errors.join("; "))
        }
    }
}

impl BenchmarkActions for ProcessActions<'_> {
    fn run(&mut self, phase: Phase) -> Result<(), String> {
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
                self.result = Some(analyze_trace(
                    self.trace.as_deref().ok_or("benchmark trace is missing")?,
                    self.scenario,
                )?);
                Ok(())
            }
            Phase::Evaluate => evaluate(
                self.result
                    .as_ref()
                    .ok_or("benchmark analysis is missing")?,
            ),
            Phase::Restore => unreachable!("restore has a dedicated action"),
        }
    }

    fn needs_restore(&self) -> bool {
        self.snapshot_created
    }

    fn restore(&mut self) -> Result<(), String> {
        self.restore_all()
    }
}

fn analyze_trace(text: &str, scenario: BenchmarkScenario) -> Result<BenchmarkResult, String> {
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
    let work_indices: Vec<_> = [
        "prepare_us",
        "slint_render_us",
        "custom_draw_us",
        "hidden_compose_us",
        "fb_present_us",
    ]
    .iter()
    .filter_map(|name| header.iter().position(|field| field == name))
    .collect();
    let status_index = header
        .iter()
        .position(|field| *field == "main_present_status");
    let mut walls = Vec::new();
    let mut work = Vec::new();
    let mut present_errors = 0;
    for line in lines {
        let fields: Vec<_> = line.split('\t').collect();
        let Some(wall) = fields
            .get(wall_index)
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        walls.push(wall);
        work.push(
            work_indices
                .iter()
                .filter_map(|index| fields.get(*index)?.parse::<u64>().ok())
                .sum(),
        );
        if status_index
            .and_then(|index| fields.get(index))
            .is_some_and(|status| !matches!(*status, "ok" | "latched" | "presented"))
        {
            present_errors += 1;
        }
    }
    if walls.len() < 120 {
        return Err(format!(
            "benchmark trace contains only {} usable frames",
            walls.len()
        ));
    }
    let average_wall = walls.iter().sum::<u64>() as f64 / walls.len() as f64;
    Ok(BenchmarkResult {
        scenario: scenario_label(scenario),
        frames: walls.len(),
        average_fps: 1_000_000.0 / average_wall,
        p99_work_us: percentile(&mut work, 99),
        p99_wall_us: percentile(&mut walls, 99),
        max_wall_us: walls.iter().copied().max().unwrap_or_default(),
        present_errors,
    })
}

fn evaluate(result: &BenchmarkResult) -> Result<(), String> {
    let mut failures = Vec::new();
    if result.average_fps < 55.0 {
        failures.push(format!("average_fps={:.1}<55", result.average_fps));
    }
    if result.p99_work_us > 14_500 {
        failures.push(format!("p99_work_us={}>14500", result.p99_work_us));
    }
    if result.p99_wall_us > 16_000 {
        failures.push(format!("p99_wall_us={}>16000", result.p99_wall_us));
    }
    if result.max_wall_us > 16_667 {
        failures.push(format!("max_wall_us={}>16667", result.max_wall_us));
    }
    if result.present_errors != 0 {
        failures.push(format!("present_errors={}", result.present_errors));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("benchmark gate failed: {}", failures.join(", ")))
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
    }
}

fn git_value(repository: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
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

    impl BenchmarkActions for FakeActions {
        fn run(&mut self, phase: Phase) -> Result<(), String> {
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

        fn restore(&mut self) -> Result<(), String> {
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
            infer_scenario(&[PathBuf::from("apps/mister/src/launcher.rs")]).unwrap(),
            BenchmarkScenario::LauncherVelocity
        );
        assert!(infer_scenario(&[PathBuf::from("docs/device.md")]).is_err());
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
            assert!(error.contains("restore=complete"));
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
        assert!(error.starts_with("cancelled:"));
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
    fn restore_failure_requires_recovery() {
        let mut actions = FakeActions {
            fail_at: Some(Phase::Capture),
            restore_fails: true,
            ..FakeActions::default()
        };
        assert!(run_workflow(&mut actions, &mut |_, _| Ok(()))
            .unwrap_err()
            .starts_with("recovery_required:"));
    }
}
