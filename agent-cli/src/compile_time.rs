// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reproducible, build-only compile-time measurements.

use crate::build::{BuildSpec, execute_quiet_at_target_dir};
use crate::error::AgentResult;
use crate::process;
use crate::progress::{EventKind, Reporter};
use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{FileTimes, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

const BUILD_DEADLINE: Duration = Duration::from_secs(30 * 60);
const NO_OP_SAMPLES: usize = 5;
const EDIT_REBUILD_SAMPLES: usize = 5;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum RevisionComparisonScenario {
    PrePushCatalog,
    ArmRuntimeCi,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum CompileTimeTarget {
    FramebufferLabArm,
    FramebufferLabMacos,
    MagikFullAppArm,
    MagikFullAppMacos,
    MagikReleaseDeviceArmAll,
    MagikReleaseDeviceArmProduction,
    MagikReleaseDeviceArmThin,
    MagikReleaseDeviceArmThinStripped,
    MagikReleaseDeviceArmThinCgu32,
    FramebufferSceneLabArm,
    FramebufferSceneLabMacos,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum CompileTimeEdit {
    #[default]
    SharedMagik,
    SharedNavigation,
    SharedScreenshotParade,
    LabHost,
}

impl CompileTimeEdit {
    const fn label(self) -> &'static str {
        match self {
            Self::SharedMagik => "shared-magik",
            Self::SharedNavigation => "shared-navigation",
            Self::SharedScreenshotParade => "shared-screenshot-parade",
            Self::LabHost => "lab-host",
        }
    }
}

impl CompileTimeTarget {
    const fn label(self) -> &'static str {
        match self {
            Self::FramebufferLabArm => "framebuffer-lab-arm",
            Self::FramebufferLabMacos => "framebuffer-lab-macos",
            Self::MagikFullAppArm => "magik-full-app-arm",
            Self::MagikFullAppMacos => "magik-full-app-macos",
            Self::MagikReleaseDeviceArmAll => "magik-release-device-arm-all",
            Self::MagikReleaseDeviceArmProduction => "magik-release-device-arm-production",
            Self::MagikReleaseDeviceArmThin => "magik-release-device-arm-thin",
            Self::MagikReleaseDeviceArmThinStripped => "magik-release-device-arm-thin-stripped",
            Self::MagikReleaseDeviceArmThinCgu32 => "magik-release-device-arm-thin-cgu32",
            Self::FramebufferSceneLabArm => "framebuffer-scene-lab-arm",
            Self::FramebufferSceneLabMacos => "framebuffer-scene-lab-macos",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum CompileTimeCommand {
    /// Perform one safe build without running or installing its artifact.
    Build {
        #[arg(value_enum)]
        target: CompileTimeTarget,
        #[arg(long, value_name = "ABSOLUTE_PATH")]
        target_dir: PathBuf,
    },
    /// Record one cold, five no-op, and five selected-source rebuild samples.
    Measure {
        #[arg(value_enum)]
        target: CompileTimeTarget,
        #[arg(long, value_name = "NEW_ABSOLUTE_PATH")]
        target_dir: PathBuf,
        #[arg(long, value_name = "NEW_JSON_PATH")]
        output: PathBuf,
        #[arg(long, value_enum, default_value = "shared-magik")]
        edit: CompileTimeEdit,
    },
    /// Compare compile latency between two clean repository revisions.
    CompareRevisions {
        #[arg(long, value_name = "ABSOLUTE_PATH")]
        baseline_repository: PathBuf,
        #[arg(long, value_name = "ABSOLUTE_PATH")]
        candidate_repository: PathBuf,
        #[arg(long, value_name = "NEW_ABSOLUTE_PATH")]
        work_root: PathBuf,
        #[arg(long, value_name = "NEW_JSON_PATH")]
        output: PathBuf,
        #[arg(long, value_enum, required = true)]
        scenario: Vec<RevisionComparisonScenario>,
    },
    /// Measure one candidate and advance the sequential compile-time baseline only on a win.
    Campaign {
        #[arg(value_enum)]
        target: CompileTimeTarget,
        #[arg(long, value_name = "NEW_ABSOLUTE_PATH")]
        target_dir: PathBuf,
        #[arg(long, value_name = "NEW_JSON_PATH")]
        candidate_output: PathBuf,
        #[arg(long, value_name = "JSON_PATH")]
        baseline: Option<PathBuf>,
        #[arg(long, value_name = "NEW_JSON_PATH")]
        output: PathBuf,
        #[arg(long, value_name = "NEW_JSON_PATH")]
        next_baseline: PathBuf,
        #[arg(long, value_enum, default_value = "shared-magik")]
        edit: CompileTimeEdit,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CompileTimeReport {
    schema: String,
    target: CompileTimeTarget,
    edit: CompileTimeEdit,
    source_revision: String,
    source_path: String,
    source_sha256_before: String,
    source_sha256_after: String,
    target_dir: String,
    machine_arch: String,
    macos_version: String,
    rustc: String,
    cargo: String,
    cold_ms: u128,
    no_op_ms: Vec<u128>,
    edit_warmup_ms: u128,
    edit_rebuild_ms: Vec<u128>,
}

#[derive(Debug, Serialize)]
struct CampaignReport {
    schema: &'static str,
    target: CompileTimeTarget,
    edit: CompileTimeEdit,
    baseline_report: Option<String>,
    candidate_report: String,
    next_baseline: String,
    baseline_median_ms: Option<u128>,
    candidate_median_ms: u128,
    candidate_samples_below_baseline: usize,
    winning: bool,
}

#[derive(Debug, Serialize)]
struct RevisionComparisonReport {
    schema: &'static str,
    baseline_repository: String,
    candidate_repository: String,
    work_root: String,
    machine_arch: String,
    macos_version: String,
    rustc: String,
    cargo: String,
    scenarios: Vec<ScenarioComparisonReport>,
}

#[derive(Debug, Serialize)]
struct ScenarioComparisonReport {
    scenario: RevisionComparisonScenario,
    variants: Vec<RevisionVariantReport>,
    comparisons: Vec<SpeedupReport>,
}

#[derive(Debug, Serialize)]
struct RevisionVariantReport {
    label: String,
    repository: String,
    source_revision: String,
    source_path: String,
    source_sha256_before: String,
    source_sha256_after: String,
    effective_profile: String,
    command: String,
    target_dir: String,
    cold_ms: u128,
    edit_warmup_ms: u128,
    edit_rebuild_ms: Vec<u128>,
    edit_median_ms: u128,
}

#[derive(Debug, Serialize)]
struct SpeedupReport {
    baseline: String,
    candidate: String,
    cold_ratio: f64,
    edit_median_ratio: f64,
}

pub fn execute(
    repository: &Path,
    command: &CompileTimeCommand,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    match command {
        CompileTimeCommand::Build { target, target_dir } => {
            validate_target_dir(repository, target_dir, false)?;
            reporter.emit(
                EventKind::Progress,
                "compile-time-build",
                &format!(
                    "building {} without running or installing it",
                    target.label()
                ),
                Some(10),
            )?;
            run_build(repository, *target, target_dir)?;
            reporter.emit(
                EventKind::Completed,
                "compile-time-build",
                "build-only compile completed",
                Some(100),
            )?;
            Ok(())
        }
        CompileTimeCommand::Measure {
            target,
            target_dir,
            output,
            edit,
        } => measure(repository, *target, *edit, target_dir, output, reporter),
        CompileTimeCommand::CompareRevisions {
            baseline_repository,
            candidate_repository,
            work_root,
            output,
            scenario,
        } => compare_revisions(
            baseline_repository,
            candidate_repository,
            work_root,
            output,
            scenario,
            reporter,
        ),
        CompileTimeCommand::Campaign {
            target,
            target_dir,
            candidate_output,
            baseline,
            output,
            next_baseline,
            edit,
        } => campaign(
            repository,
            *target,
            *edit,
            CompileTimeCampaignPaths {
                target_dir,
                candidate_output,
                baseline: baseline.as_deref(),
                output,
                next_baseline,
            },
            reporter,
        ),
    }
}

struct CompileTimeCampaignPaths<'a> {
    target_dir: &'a Path,
    candidate_output: &'a Path,
    baseline: Option<&'a Path>,
    output: &'a Path,
    next_baseline: &'a Path,
}

fn campaign(
    repository: &Path,
    target: CompileTimeTarget,
    edit: CompileTimeEdit,
    paths: CompileTimeCampaignPaths<'_>,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    let CompileTimeCampaignPaths {
        target_dir,
        candidate_output,
        baseline,
        output,
        next_baseline,
    } = paths;
    validate_target_dir(repository, target_dir, true)?;
    for (label, path) in [
        ("candidate output", candidate_output),
        ("campaign output", output),
        ("next baseline", next_baseline),
    ] {
        if path.exists() {
            return Err(format!("{label} already exists: {}", path.display()).into());
        }
    }
    if baseline.is_some_and(|path| !path.is_file()) {
        return Err("campaign baseline must be an existing JSON report".into());
    }
    measure(
        repository,
        target,
        edit,
        target_dir,
        candidate_output,
        reporter,
    )?;
    let candidate = read_compile_time_report(candidate_output)?;
    let (baseline_median_ms, candidate_samples_below_baseline, winning) =
        if let Some(path) = baseline {
            let baseline_report = read_compile_time_report(path)?;
            let baseline_median_ms = median(&baseline_report.edit_rebuild_ms)?;
            let candidate_median_ms = median(&candidate.edit_rebuild_ms)?;
            let below = candidate
                .edit_rebuild_ms
                .iter()
                .filter(|sample| **sample < baseline_median_ms)
                .count();
            (
                Some(baseline_median_ms),
                below,
                candidate_median_ms < baseline_median_ms && below >= 3,
            )
        } else {
            (None, 0, true)
        };
    let candidate_median_ms = median(&candidate.edit_rebuild_ms)?;
    std::fs::copy(
        if winning {
            candidate_output
        } else {
            baseline.ok_or("losing campaign requires a baseline report")?
        },
        next_baseline,
    )
    .map_err(|error| format!("cannot advance campaign baseline: {error}"))?;
    let report = CampaignReport {
        schema: "mister-magik-compile-campaign-v1",
        target,
        edit,
        baseline_report: baseline.map(|path| path.display().to_string()),
        candidate_report: candidate_output.display().to_string(),
        next_baseline: next_baseline.display().to_string(),
        baseline_median_ms,
        candidate_median_ms,
        candidate_samples_below_baseline,
        winning,
    };
    write_json_report(output, &report)?;
    reporter.emit(
        if winning {
            EventKind::Completed
        } else {
            EventKind::Warning
        },
        "compile-time-campaign",
        &format!(
            "campaign candidate={} median_ms={} baseline_ms={} winning={winning}",
            candidate_output.display(),
            candidate_median_ms,
            baseline_median_ms.map_or_else(|| "none".to_owned(), |value| value.to_string()),
        ),
        Some(100),
    )?;
    Ok(())
}

fn read_compile_time_report(path: &Path) -> AgentResult<CompileTimeReport> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| {
        format!(
            "cannot parse compile-time report {}: {error}",
            path.display()
        )
        .into()
    })
}

fn measure(
    repository: &Path,
    target: CompileTimeTarget,
    edit: CompileTimeEdit,
    target_dir: &Path,
    output: &Path,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    validate_target_dir(repository, target_dir, true)?;
    if output.exists() {
        return Err(format!("compile-time output already exists: {}", output.display()).into());
    }
    let source = repository.join(target.source_path(edit)?);
    require_clean_source(repository, &source)?;
    let mut source_guard = SourceStampGuard::new(&source)?;
    std::fs::create_dir_all(target_dir)
        .map_err(|error| format!("cannot create {}: {error}", target_dir.display()))?;

    reporter.emit(
        EventKind::Progress,
        "compile-time-cold",
        &format!("measuring cold {} build", target.label()),
        Some(5),
    )?;
    let cold_ms = timed_build(repository, target, target_dir)?;

    let mut no_op_ms = Vec::with_capacity(NO_OP_SAMPLES);
    for sample in 0..NO_OP_SAMPLES {
        reporter.emit(
            EventKind::Progress,
            "compile-time-no-op",
            &format!("measuring no-op sample {}/{}", sample + 1, NO_OP_SAMPLES),
            Some(10 + (sample as u8 * 5)),
        )?;
        no_op_ms.push(timed_build(repository, target, target_dir)?);
    }

    source_guard.force_rebuild()?;
    reporter.emit(
        EventKind::Progress,
        "compile-time-warmup",
        &format!("warming the {} rebuild path", edit.label()),
        Some(40),
    )?;
    let edit_warmup_ms = timed_build(repository, target, target_dir)?;

    let mut edit_rebuild_ms = Vec::with_capacity(EDIT_REBUILD_SAMPLES);
    for sample in 0..EDIT_REBUILD_SAMPLES {
        source_guard.force_rebuild()?;
        reporter.emit(
            EventKind::Progress,
            "compile-time-edit",
            &format!(
                "measuring {} rebuild sample {}/{}",
                edit.label(),
                sample + 1,
                EDIT_REBUILD_SAMPLES
            ),
            Some(50 + (sample as u8 * 8)),
        )?;
        edit_rebuild_ms.push(timed_build(repository, target, target_dir)?);
    }

    let source_sha256_before = source_guard.original_sha256.clone();
    let source_sha256_after = source_guard.finish()?;
    require_clean_source(repository, &source)?;
    let report = CompileTimeReport {
        schema: "mister-magik-compile-time-v3".into(),
        target,
        edit,
        source_revision: command_output(repository, "git", &["rev-parse", "HEAD"])?,
        source_path: source
            .strip_prefix(repository)
            .unwrap_or(&source)
            .display()
            .to_string(),
        source_sha256_before,
        source_sha256_after,
        target_dir: target_dir.display().to_string(),
        machine_arch: command_output(repository, "uname", &["-m"])?,
        macos_version: command_output(repository, "sw_vers", &["-productVersion"])
            .unwrap_or_else(|_| "not-macos".into()),
        rustc: command_output(repository, "rustc", &["--version"])?,
        cargo: command_output(repository, "cargo", &["--version"])?,
        cold_ms,
        no_op_ms,
        edit_warmup_ms,
        edit_rebuild_ms,
    };
    write_report(output, &report)?;
    reporter.emit(
        EventKind::Completed,
        "compile-time-measure",
        &format!("wrote {}", output.display()),
        Some(100),
    )?;
    Ok(())
}

fn timed_build(
    repository: &Path,
    target: CompileTimeTarget,
    target_dir: &Path,
) -> AgentResult<u128> {
    let started = Instant::now();
    run_build(repository, target, target_dir)?;
    Ok(started.elapsed().as_millis())
}

fn run_build(repository: &Path, target: CompileTimeTarget, target_dir: &Path) -> AgentResult<()> {
    match target {
        CompileTimeTarget::FramebufferLabArm => execute_quiet_at_target_dir(
            repository,
            &BuildSpec::framebuffer_lab_device(),
            target_dir,
        ),
        CompileTimeTarget::FramebufferLabMacos => build_macos_lab(repository, target_dir),
        CompileTimeTarget::MagikFullAppArm => execute_quiet_at_target_dir(
            repository,
            &BuildSpec::magik_full_app_baseline(),
            target_dir,
        ),
        CompileTimeTarget::MagikFullAppMacos => build_macos_full_app(repository, target_dir),
        CompileTimeTarget::MagikReleaseDeviceArmAll => execute_quiet_at_target_dir(
            repository,
            &BuildSpec::canonical(crate::deploy::UiScope::All),
            target_dir,
        ),
        CompileTimeTarget::MagikReleaseDeviceArmProduction => execute_quiet_at_target_dir(
            repository,
            &BuildSpec::canonical(crate::deploy::UiScope::Production),
            target_dir,
        ),
        CompileTimeTarget::MagikReleaseDeviceArmThin => execute_quiet_at_target_dir(
            repository,
            &BuildSpec::canonical_profile(
                crate::deploy::UiScope::Production,
                "release-device-thin",
            ),
            target_dir,
        ),
        CompileTimeTarget::MagikReleaseDeviceArmThinStripped => execute_quiet_at_target_dir(
            repository,
            &BuildSpec::canonical_profile(
                crate::deploy::UiScope::Production,
                "release-device-thin-stripped",
            ),
            target_dir,
        ),
        CompileTimeTarget::MagikReleaseDeviceArmThinCgu32 => execute_quiet_at_target_dir(
            repository,
            &BuildSpec::canonical_profile(
                crate::deploy::UiScope::Production,
                "release-device-thin-cgu32",
            ),
            target_dir,
        ),
        CompileTimeTarget::FramebufferSceneLabArm => execute_quiet_at_target_dir(
            repository,
            &BuildSpec::framebuffer_scene_lab_device(),
            target_dir,
        ),
        CompileTimeTarget::FramebufferSceneLabMacos => {
            build_macos_framebuffer_scene_lab(repository, target_dir)
        }
    }
}

impl CompileTimeTarget {
    fn source_path(self, edit: CompileTimeEdit) -> AgentResult<&'static str> {
        match self {
            Self::FramebufferLabArm | Self::FramebufferLabMacos => {
                if edit == CompileTimeEdit::SharedMagik {
                    Ok("apps/framebuffer-lab/src/particles/showcase.rs")
                } else {
                    Err("the framebuffer showcase supports only --edit shared-magik".into())
                }
            }
            Self::MagikFullAppArm | Self::MagikFullAppMacos => {
                if edit == CompileTimeEdit::SharedMagik {
                    Ok("crates/particles/src/magik.rs")
                } else {
                    Err("the full-app compile target supports only --edit shared-magik".into())
                }
            }
            Self::MagikReleaseDeviceArmAll
            | Self::MagikReleaseDeviceArmProduction
            | Self::MagikReleaseDeviceArmThin
            | Self::MagikReleaseDeviceArmThinStripped
            | Self::MagikReleaseDeviceArmThinCgu32 => {
                if edit == CompileTimeEdit::SharedMagik {
                    Ok("apps/mister/src/main.rs")
                } else {
                    Err(
                        "the release-device compile target supports only --edit shared-magik"
                            .into(),
                    )
                }
            }
            Self::FramebufferSceneLabArm | Self::FramebufferSceneLabMacos => Ok(match edit {
                CompileTimeEdit::SharedMagik => "crates/particles/src/magik.rs",
                CompileTimeEdit::SharedNavigation => "crates/framebuffer-scenes/src/navigation.rs",
                CompileTimeEdit::SharedScreenshotParade => {
                    "crates/screenshot-parade/src/schedule.rs"
                }
                CompileTimeEdit::LabHost => "apps/framebuffer-scene-lab/src/main.rs",
            }),
        }
    }
}

fn build_macos_full_app(repository: &Path, target_dir: &Path) -> AgentResult<()> {
    let mut child = Command::new("cargo")
        .current_dir(repository.join("apps/mister"))
        .env("CARGO_TARGET_DIR", target_dir)
        .env("RUSTC_WRAPPER", "")
        .env("SLINT_EMIT_DEBUG_INFO", "1")
        .args([
            "build",
            "--locked",
            "--bin",
            "mister-magik-ui-preview",
            "--features",
            "ui-preview",
        ])
        .stdin(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot start macOS full-app Magik build: {error}"))?;
    let status = process::wait(
        &mut child,
        Some(BUILD_DEADLINE),
        "macOS full-app Magik build",
        None,
        || Ok(()),
    )?;
    if !status.success() {
        return Err(format!("macOS full-app Magik build exited with {status}").into());
    }
    Ok(())
}

fn build_macos_lab(repository: &Path, target_dir: &Path) -> AgentResult<()> {
    let mut child = Command::new("cargo")
        .current_dir(repository.join("apps/framebuffer-lab"))
        .env("CARGO_TARGET_DIR", target_dir)
        .env("RUSTC_WRAPPER", "")
        .args(["build", "--locked"])
        .stdin(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot start macOS framebuffer lab build: {error}"))?;
    let status = process::wait(
        &mut child,
        Some(BUILD_DEADLINE),
        "macOS framebuffer lab build",
        None,
        || Ok(()),
    )?;
    if !status.success() {
        return Err(format!("macOS framebuffer lab build exited with {status}").into());
    }
    Ok(())
}

fn build_macos_framebuffer_scene_lab(repository: &Path, target_dir: &Path) -> AgentResult<()> {
    let mut child = Command::new("cargo")
        .current_dir(repository.join("apps/framebuffer-scene-lab"))
        .env("CARGO_TARGET_DIR", target_dir)
        .env("RUSTC_WRAPPER", "")
        .args(["build", "--locked", "--profile", "release-live"])
        .stdin(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot start macOS startup particle lab build: {error}"))?;
    let status = process::wait(
        &mut child,
        Some(BUILD_DEADLINE),
        "macOS startup particle lab build",
        None,
        || Ok(()),
    )?;
    if !status.success() {
        return Err(format!("macOS startup particle lab build exited with {status}").into());
    }
    Ok(())
}

fn compare_revisions(
    baseline_repository: &Path,
    candidate_repository: &Path,
    work_root: &Path,
    output: &Path,
    scenarios: &[RevisionComparisonScenario],
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    validate_comparison_inputs(baseline_repository, candidate_repository, work_root, output)?;
    std::fs::create_dir_all(work_root)
        .map_err(|error| format!("cannot create {}: {error}", work_root.display()))?;
    let mut reports = Vec::with_capacity(scenarios.len());
    for (index, scenario) in scenarios.iter().copied().enumerate() {
        reporter.emit(
            EventKind::Progress,
            "compile-time-compare",
            &format!("measuring revision scenario {scenario:?}"),
            Some(u8::try_from(5 + index * 80 / scenarios.len().max(1)).unwrap_or(90)),
        )?;
        reports.push(match scenario {
            RevisionComparisonScenario::PrePushCatalog => compare_pre_push_catalog(
                baseline_repository,
                candidate_repository,
                &work_root.join("pre-push-catalog"),
            )?,
            RevisionComparisonScenario::ArmRuntimeCi => compare_arm_runtime_ci(
                baseline_repository,
                candidate_repository,
                &work_root.join("arm-runtime-ci"),
            )?,
        });
    }
    let report = RevisionComparisonReport {
        schema: "mister-magik-compile-revision-comparison-v1",
        baseline_repository: baseline_repository.display().to_string(),
        candidate_repository: candidate_repository.display().to_string(),
        work_root: work_root.display().to_string(),
        machine_arch: command_output(candidate_repository, "uname", &["-m"])?,
        macos_version: command_output(candidate_repository, "sw_vers", &["-productVersion"])
            .unwrap_or_else(|_| "not-macos".into()),
        rustc: command_output(candidate_repository, "rustc", &["--version"])?,
        cargo: command_output(candidate_repository, "cargo", &["--version"])?,
        scenarios: reports,
    };
    write_json_report(output, &report)?;
    reporter.emit(
        EventKind::Completed,
        "compile-time-compare",
        &format!("wrote {}", output.display()),
        Some(100),
    )?;
    Ok(())
}

fn validate_comparison_inputs(
    baseline_repository: &Path,
    candidate_repository: &Path,
    work_root: &Path,
    output: &Path,
) -> AgentResult<()> {
    for (label, repository) in [
        ("baseline", baseline_repository),
        ("candidate", candidate_repository),
    ] {
        if !repository.is_absolute() || !repository.is_dir() {
            return Err(format!("{label} repository must be an absolute directory").into());
        }
        require_clean_repository(repository)?;
    }
    if baseline_repository == candidate_repository {
        return Err("baseline and candidate repositories must differ".into());
    }
    if !work_root.is_absolute()
        || work_root == Path::new("/")
        || work_root.starts_with(baseline_repository)
        || work_root.starts_with(candidate_repository)
    {
        return Err("comparison work root must be an absolute external path".into());
    }
    if work_root.exists() {
        return Err(format!(
            "comparison work root already exists: {}",
            work_root.display()
        )
        .into());
    }
    if output.exists() {
        return Err(format!("compile-time output already exists: {}", output.display()).into());
    }
    Ok(())
}

fn require_clean_repository(repository: &Path) -> AgentResult<()> {
    let status = command_output(
        repository,
        "git",
        &["status", "--porcelain", "--untracked-files=all"],
    )?;
    if !status.is_empty() {
        return Err(format!(
            "comparison repository is not clean: {}",
            repository.display()
        )
        .into());
    }
    Ok(())
}

fn compare_pre_push_catalog(
    baseline_repository: &Path,
    candidate_repository: &Path,
    work_root: &Path,
) -> AgentResult<ScenarioComparisonReport> {
    let baseline = measure_assurance_revision(
        "baseline",
        baseline_repository,
        &work_root.join("baseline"),
        "legacy revision defaults",
    )?;
    let candidate = measure_assurance_revision(
        "candidate",
        candidate_repository,
        &work_root.join("candidate"),
        "fast agent + fast assurance",
    )?;
    let comparisons = vec![speedup(&baseline, &candidate)];
    Ok(ScenarioComparisonReport {
        scenario: RevisionComparisonScenario::PrePushCatalog,
        variants: vec![baseline, candidate],
        comparisons,
    })
}

fn measure_assurance_revision(
    label: &str,
    repository: &Path,
    work_root: &Path,
    effective_profile: &str,
) -> AgentResult<RevisionVariantReport> {
    let target_dir = work_root.join("target");
    std::fs::create_dir_all(&target_dir)
        .map_err(|error| format!("cannot create {}: {error}", target_dir.display()))?;
    let relative_source = Path::new("crates/catalog/src/lib.rs");
    let source = repository.join(relative_source);
    require_clean_source(repository, &source)?;
    let mut source_guard = SourceStampGuard::new(&source)?;
    let cold_ms = timed_assurance(repository, &target_dir, &work_root.join("state-cold"))?;
    source_guard.force_rebuild()?;
    let edit_warmup_ms = timed_assurance(repository, &target_dir, &work_root.join("state-warmup"))?;
    let mut edit_rebuild_ms = Vec::with_capacity(EDIT_REBUILD_SAMPLES);
    for sample in 0..EDIT_REBUILD_SAMPLES {
        source_guard.force_rebuild()?;
        edit_rebuild_ms.push(timed_assurance(
            repository,
            &target_dir,
            &work_root.join(format!("state-edit-{sample}")),
        )?);
    }
    let source_sha256_before = source_guard.original_sha256.clone();
    let source_sha256_after = source_guard.finish()?;
    require_clean_repository(repository)?;
    let edit_median_ms = median(&edit_rebuild_ms)?;
    Ok(RevisionVariantReport {
        label: label.into(),
        repository: repository.display().to_string(),
        source_revision: command_output(repository, "git", &["rev-parse", "HEAD"])?,
        source_path: relative_source.display().to_string(),
        source_sha256_before,
        source_sha256_after,
        effective_profile: effective_profile.into(),
        command: "scripts/agent ci host-assurance --paths crates/catalog/src/lib.rs".into(),
        target_dir: target_dir.display().to_string(),
        cold_ms,
        edit_warmup_ms,
        edit_rebuild_ms,
        edit_median_ms,
    })
}

fn timed_assurance(repository: &Path, target_dir: &Path, state_dir: &Path) -> AgentResult<u128> {
    if state_dir.exists() {
        return Err(format!(
            "comparison state path already exists: {}",
            state_dir.display()
        )
        .into());
    }
    let started = Instant::now();
    let mut child = Command::new(repository.join("scripts/agent"))
        .current_dir(repository)
        .env("CARGO_TARGET_DIR", target_dir)
        .env("MISTER_AGENT_CLI_STATE_DIR", state_dir)
        .env("RUSTC_WRAPPER", "")
        .args([
            "ci",
            "host-assurance",
            "--paths",
            "crates/catalog/src/lib.rs",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot start comparison assurance: {error}"))?;
    let status = process::wait(
        &mut child,
        Some(BUILD_DEADLINE),
        "comparison host assurance",
        None,
        || Ok(()),
    )?;
    if !status.success() {
        return Err(format!("comparison host assurance exited with {status}").into());
    }
    Ok(started.elapsed().as_millis())
}

fn compare_arm_runtime_ci(
    baseline_repository: &Path,
    candidate_repository: &Path,
    work_root: &Path,
) -> AgentResult<ScenarioComparisonReport> {
    let baseline_pr = measure_arm_revision(
        "baseline-pr-release",
        baseline_repository,
        &work_root.join("baseline-pr-release"),
        &BuildSpec::runtime_release_baseline(),
    )?;
    let baseline_main = measure_arm_revision(
        "baseline-main-release-device",
        baseline_repository,
        &work_root.join("baseline-main-release-device"),
        &BuildSpec::canonical(crate::deploy::UiScope::All),
    )?;
    let candidate = measure_arm_revision(
        "candidate-ci-fast",
        candidate_repository,
        &work_root.join("candidate-ci-fast"),
        &BuildSpec::runtime_ci(),
    )?;
    let comparisons = vec![
        speedup(&baseline_pr, &candidate),
        speedup(&baseline_main, &candidate),
    ];
    Ok(ScenarioComparisonReport {
        scenario: RevisionComparisonScenario::ArmRuntimeCi,
        variants: vec![baseline_pr, baseline_main, candidate],
        comparisons,
    })
}

fn measure_arm_revision(
    label: &str,
    repository: &Path,
    target_dir: &Path,
    spec: &BuildSpec,
) -> AgentResult<RevisionVariantReport> {
    validate_target_dir(repository, target_dir, true)?;
    std::fs::create_dir_all(target_dir)
        .map_err(|error| format!("cannot create {}: {error}", target_dir.display()))?;
    let relative_source = Path::new("crates/particles/src/magik.rs");
    let source = repository.join(relative_source);
    require_clean_source(repository, &source)?;
    let mut source_guard = SourceStampGuard::new(&source)?;
    let cold_ms = timed_build_spec(repository, spec, target_dir)?;
    source_guard.force_rebuild()?;
    let edit_warmup_ms = timed_build_spec(repository, spec, target_dir)?;
    let mut edit_rebuild_ms = Vec::with_capacity(EDIT_REBUILD_SAMPLES);
    for _ in 0..EDIT_REBUILD_SAMPLES {
        source_guard.force_rebuild()?;
        edit_rebuild_ms.push(timed_build_spec(repository, spec, target_dir)?);
    }
    let source_sha256_before = source_guard.original_sha256.clone();
    let source_sha256_after = source_guard.finish()?;
    require_clean_repository(repository)?;
    let edit_median_ms = median(&edit_rebuild_ms)?;
    Ok(RevisionVariantReport {
        label: label.into(),
        repository: repository.display().to_string(),
        source_revision: command_output(repository, "git", &["rev-parse", "HEAD"])?,
        source_path: relative_source.display().to_string(),
        source_sha256_before,
        source_sha256_after,
        effective_profile: spec.profile().into(),
        command: format!("typed ARM runtime build --profile {}", spec.profile()),
        target_dir: target_dir.display().to_string(),
        cold_ms,
        edit_warmup_ms,
        edit_rebuild_ms,
        edit_median_ms,
    })
}

fn timed_build_spec(repository: &Path, spec: &BuildSpec, target_dir: &Path) -> AgentResult<u128> {
    let started = Instant::now();
    execute_quiet_at_target_dir(repository, spec, target_dir)?;
    Ok(started.elapsed().as_millis())
}

fn median(samples: &[u128]) -> AgentResult<u128> {
    if samples.is_empty() {
        return Err("cannot compute a median without samples".into());
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    Ok(sorted[sorted.len() / 2])
}

fn speedup(baseline: &RevisionVariantReport, candidate: &RevisionVariantReport) -> SpeedupReport {
    SpeedupReport {
        baseline: baseline.label.clone(),
        candidate: candidate.label.clone(),
        cold_ratio: ratio(baseline.cold_ms, candidate.cold_ms),
        edit_median_ratio: ratio(baseline.edit_median_ms, candidate.edit_median_ms),
    }
}

fn ratio(baseline: u128, candidate: u128) -> f64 {
    baseline as f64 / candidate.max(1) as f64
}

fn validate_target_dir(repository: &Path, target_dir: &Path, require_new: bool) -> AgentResult<()> {
    if !target_dir.is_absolute() {
        return Err("compile-time target directory must be absolute".into());
    }
    if target_dir == Path::new("/") || target_dir == repository {
        return Err("compile-time target directory is too broad".into());
    }
    if target_dir.starts_with(repository) {
        return Err("compile-time target directory must be outside the repository".into());
    }
    if require_new && target_dir.exists() {
        return Err(format!(
            "compile-time measurement target must not exist: {}",
            target_dir.display()
        )
        .into());
    }
    Ok(())
}

fn require_clean_source(repository: &Path, source: &Path) -> AgentResult<()> {
    let relative = source
        .strip_prefix(repository)
        .map_err(|_| "compile-time edit source is outside the repository")?;
    let status = command_output(
        repository,
        "git",
        &[
            "status",
            "--porcelain",
            "--",
            &relative.display().to_string(),
        ],
    )?;
    if !status.is_empty() {
        return Err(format!(
            "compile-time edit source must be clean before measurement: {}",
            relative.display()
        )
        .into());
    }
    Ok(())
}

struct SourceStampGuard {
    path: PathBuf,
    original_bytes: Vec<u8>,
    original_sha256: String,
    original_modified: Option<SystemTime>,
    original_accessed: Option<SystemTime>,
    generation: u64,
    finished: bool,
}

impl SourceStampGuard {
    fn new(path: &Path) -> AgentResult<Self> {
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        let original_bytes = std::fs::read(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            original_sha256: sha256_bytes(&original_bytes),
            original_bytes,
            original_modified: metadata.modified().ok(),
            original_accessed: metadata.accessed().ok(),
            generation: 0,
            finished: false,
        })
    }

    fn force_rebuild(&mut self) -> AgentResult<()> {
        self.generation = self.generation.saturating_add(1);
        let mut bytes = self.original_bytes.clone();
        bytes.extend_from_slice(
            format!("\n// compile-time-edit-generation:{}\n", self.generation).as_bytes(),
        );
        std::fs::write(&self.path, bytes)
            .map_err(|error| format!("cannot mark {} for rebuild: {error}", self.path.display()))?;
        Ok(())
    }

    fn finish(&mut self) -> AgentResult<String> {
        self.restore_source()?;
        let after = sha256(&self.path)?;
        if after != self.original_sha256 {
            return Err("edit source was not restored after compile-time measurement".into());
        }
        self.finished = true;
        Ok(after)
    }

    fn restore_times(&self) -> AgentResult<()> {
        let file = OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(|error| format!("cannot restore {}: {error}", self.path.display()))?;
        let mut times = FileTimes::new();
        if let Some(modified) = self.original_modified {
            times = times.set_modified(modified);
        }
        if let Some(accessed) = self.original_accessed {
            times = times.set_accessed(accessed);
        }
        file.set_times(times)
            .map_err(|error| format!("cannot restore {} times: {error}", self.path.display()))?;
        Ok(())
    }

    fn restore_source(&self) -> AgentResult<()> {
        std::fs::write(&self.path, &self.original_bytes)
            .map_err(|error| format!("cannot restore {}: {error}", self.path.display()))?;
        self.restore_times()
    }
}

impl Drop for SourceStampGuard {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.restore_source();
        }
    }
}

fn sha256(path: &Path) -> AgentResult<String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn command_output(repository: &Path, program: &str, arguments: &[&str]) -> AgentResult<String> {
    let output = Command::new(program)
        .current_dir(repository)
        .args(arguments)
        .output()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{program} exited with {}", output.status).into());
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|error| format!("{program} output is not UTF-8: {error}").into())
}

fn write_report(path: &Path, report: &CompileTimeReport) -> AgentResult<()> {
    write_json_report(path, report)
}

fn write_json_report(path: &Path, report: &impl Serialize) -> AgentResult<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, report)
        .map_err(|error| format!("cannot serialize compile-time report: {error}"))?;
    file.write_all(b"\n")
        .map_err(|error| format!("cannot write {}: {error}", path.display()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_directory_must_be_absolute_external_and_new_for_measurement() {
        let repository = Path::new("/repo");
        assert!(validate_target_dir(repository, Path::new("relative"), false).is_err());
        assert!(validate_target_dir(repository, Path::new("/"), false).is_err());
        assert!(validate_target_dir(repository, Path::new("/repo/target"), false).is_err());
        assert!(validate_target_dir(repository, Path::new("/tmp/target"), false).is_ok());
    }

    #[test]
    fn compile_target_labels_are_stable() {
        assert_eq!(
            CompileTimeTarget::FramebufferLabArm.label(),
            "framebuffer-lab-arm"
        );
        assert_eq!(
            CompileTimeTarget::FramebufferLabMacos.label(),
            "framebuffer-lab-macos"
        );
        assert_eq!(
            CompileTimeTarget::MagikFullAppArm.label(),
            "magik-full-app-arm"
        );
        assert_eq!(
            CompileTimeTarget::MagikFullAppMacos.label(),
            "magik-full-app-macos"
        );
        assert_eq!(
            CompileTimeTarget::MagikReleaseDeviceArmProduction.label(),
            "magik-release-device-arm-production"
        );
        assert_eq!(
            CompileTimeTarget::MagikReleaseDeviceArmThinCgu32.label(),
            "magik-release-device-arm-thin-cgu32"
        );
        assert_eq!(
            CompileTimeTarget::FramebufferSceneLabArm.label(),
            "framebuffer-scene-lab-arm"
        );
        assert_eq!(
            CompileTimeTarget::FramebufferSceneLabMacos.label(),
            "framebuffer-scene-lab-macos"
        );
    }

    #[test]
    fn scene_lab_measurements_select_each_real_edit_boundary() {
        let target = CompileTimeTarget::FramebufferSceneLabMacos;
        assert_eq!(
            target.source_path(CompileTimeEdit::SharedMagik).unwrap(),
            "crates/particles/src/magik.rs"
        );
        assert_eq!(
            target
                .source_path(CompileTimeEdit::SharedNavigation)
                .unwrap(),
            "crates/framebuffer-scenes/src/navigation.rs"
        );
        assert_eq!(
            target.source_path(CompileTimeEdit::LabHost).unwrap(),
            "apps/framebuffer-scene-lab/src/main.rs"
        );
        assert_eq!(
            target
                .source_path(CompileTimeEdit::SharedScreenshotParade)
                .unwrap(),
            "crates/screenshot-parade/src/schedule.rs"
        );
    }

    #[test]
    fn rebuild_guard_changes_bytes_and_restores_the_source() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-compile-time-guard-{}",
            std::process::id()
        ));
        let original = b"fn particle_source() {}\n";
        std::fs::write(&path, original).unwrap();
        let mut guard = SourceStampGuard::new(&path).unwrap();

        guard.force_rebuild().unwrap();
        let first = std::fs::read(&path).unwrap();
        guard.force_rebuild().unwrap();
        let second = std::fs::read(&path).unwrap();

        assert_ne!(first, second);
        assert_eq!(guard.finish().unwrap(), sha256_bytes(original));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn comparison_statistics_use_the_middle_sample_and_baseline_ratio() {
        assert_eq!(median(&[900, 100, 500, 300, 700]).unwrap(), 500);
        assert!(median(&[]).is_err());
        assert_eq!(ratio(2_000, 500), 4.0);
    }
}
