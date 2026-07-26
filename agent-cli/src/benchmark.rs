// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::device::DeviceClient;
use crate::error::AgentResult;
use crate::model::{BenchmarkScenario, Outcome};
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::{DeviceRequest, Layout as DeviceLayout};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn execute(
    repository: &Path,
    scenario: BenchmarkScenario,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    require_clean_installed_commit(repository, scenario, reporter)
}

fn require_clean_installed_commit(
    repository: &Path,
    scenario: BenchmarkScenario,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    let head = crate::git::value(repository, &["rev-parse", "HEAD"])?;
    if !crate::git::value(repository, &["status", "--porcelain"])?.is_empty() {
        return Err("benchmark requires a clean exact-commit worktree".into());
    }
    let mut device = DeviceClient::default();
    reporter.emit(
        EventKind::Progress,
        "preflight",
        &format!("benchmark {} installed-runtime preflight", scenario.label()),
        Some(10),
    )?;
    device.execute(DeviceRequest::Discover)?;
    device.execute(DeviceRequest::VerifyDevelopmentPlatform)?;
    device.execute(DeviceRequest::VerifyHealth(DeviceLayout::Development))?;
    let manifest = device.execute(DeviceRequest::ReadDevelopmentManifest)?;
    let installed = crate::platform_manifest::parse_installed(
        &manifest,
        crate::platform_manifest::Layout::Development,
    )?;
    if installed.magik_revision() != head {
        return Err(format!(
            "benchmark installed revision {} does not match clean local commit {head}; run scripts/agent deliver first",
            installed.magik_revision()
        )
        .into());
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    let output_dir = repository
        .join("build/agent-benchmarks")
        .join(scenario.label())
        .join(timestamp.to_string());

    match scenario {
        BenchmarkScenario::Screensaver => {
            execute_screensaver(&mut device, manifest, output_dir, reporter)
        }
        BenchmarkScenario::CatalogLifecycle => {
            execute_catalog_lifecycle(&mut device, manifest, output_dir, reporter)
        }
    }
}

fn execute_screensaver(
    device: &mut DeviceClient,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "profiling installed screensaver",
        Some(35),
    )?;
    let detail = device.execute(DeviceRequest::ProfileInstalledScreensaver {
        output_dir: output_dir.clone(),
    })?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    device.execute(DeviceRequest::VerifyHealth(DeviceLayout::Development))?;
    evaluate_summary(&summary)?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn execute_catalog_lifecycle(
    device: &mut DeviceClient,
    manifest: String,
    output_dir: std::path::PathBuf,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    reporter.emit(
        EventKind::Progress,
        "profile",
        "profiling isolated catalog lifecycle",
        Some(35),
    )?;
    let detail = device.execute(DeviceRequest::ProfileInstalledCatalogLifecycle {
        output_dir: output_dir.clone(),
    })?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    device.execute(DeviceRequest::VerifyHealth(DeviceLayout::Development))?;
    evaluate_catalog_lifecycle_summary(&summary)?;
    reporter.emit(
        EventKind::Progress,
        "benchmark-result",
        &serde_json::to_string(&json!({
            "installed_manifest": manifest,
            "summary": summary,
            "output_dir": output_dir,
        }))
        .map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn evaluate_catalog_lifecycle_summary(summary: &Value) -> AgentResult<()> {
    if summary.get("scenario").and_then(Value::as_str) != Some("catalog-lifecycle") {
        return Err("catalog lifecycle benchmark summary has the wrong scenario".into());
    }
    if summary.pointer("/catalog/valid").and_then(Value::as_bool) != Some(true) {
        return Err("catalog lifecycle benchmark did not produce a valid catalog".into());
    }
    let systems = summary
        .pointer("/catalog/systems")
        .and_then(Value::as_array)
        .ok_or("catalog lifecycle benchmark summary has no systems")?;
    if systems.is_empty() {
        return Err("catalog lifecycle benchmark produced no systems".into());
    }
    Ok(())
}

fn evaluate_summary(summary: &Value) -> AgentResult<()> {
    let runs = summary
        .get("runs")
        .and_then(Value::as_array)
        .ok_or("screensaver benchmark summary has no runs")?;
    if runs.len() != 1 {
        return Err(format!(
            "screensaver benchmark expected one run, received {}",
            runs.len()
        )
        .into());
    }
    for run in runs {
        evaluate_run(run)?;
    }
    Ok(())
}

fn evaluate_run(run: &Value) -> AgentResult<()> {
    let id = u64_field(run, "run", 0);
    let steady = run
        .get("steady_state")
        .ok_or_else(|| format!("screensaver profile run {id} has no steady-state evidence"))?;
    let physical = steady
        .get("physical_refresh")
        .ok_or_else(|| format!("screensaver profile run {id} has no physical refresh evidence"))?;
    let unique_fps = physical
        .get("unique_fps")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let refresh_hz = physical
        .get("refresh_hz")
        .and_then(Value::as_f64)
        .unwrap_or(f64::INFINITY);
    let repeats = u64_field(physical, "repeated_refreshes", u64::MAX);
    let long_gaps = physical
        .get("long_completion_intervals")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(usize::MAX);
    let frames = u64_field(steady, "frames", 0);
    let p99_work = u64_field(steady, "p99_work_us", u64::MAX);
    let p99_wall = u64_field(steady, "p99_wall_us", u64::MAX);
    let max_wall = u64_field(steady, "max_wall_us", u64::MAX);
    let refresh = u64_field(steady, "refresh_period_us", 16_667);
    let over_budget = u64_field(steady, "over_budget_frames", u64::MAX);
    let presentation_failures = steady
        .get("presentation_failures")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(usize::MAX);
    let drops = u64_field(run, "latch_drop_delta", u64::MAX);
    let misses = u64_field(steady, "vsync_misses", u64::MAX);
    let errors = u64_field(run, "present_errors", u64::MAX);
    let status = run
        .get("status_publishing")
        .ok_or_else(|| format!("screensaver profile run {id} has no status publishing evidence"))?;
    let status_mode = status
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let status_enqueue_p99 = u64_field(status, "enqueue_p99_us", u64::MAX);
    let status_worker_errors = u64_field(status, "worker_errors", u64::MAX);
    let status_submitted = u64_field(status, "final_submitted_sequence", 0);
    let status_written = u64_field(status, "final_written_sequence", 0);
    if unique_fps < refresh_hz - 0.1
        || repeats != 0
        || long_gaps != 0
        || frames == 0
        || presentation_failures != 0
        || drops != 0
        || misses != 0
        || errors != 0
        || status_mode != "async"
        || status_enqueue_p99 >= 250
        || status_worker_errors != 0
        || status_submitted == 0
        || status_written != status_submitted
    {
        return Err(format!(
            "screensaver profile run {id} failed after warm-up: frames={frames} unique_fps={unique_fps:.2}/{refresh_hz:.2} repeated_refreshes={repeats} long_completion_gaps={long_gaps} presentation_failures={presentation_failures} timing_overruns={over_budget} p99_work_us={p99_work} p99_wall_us={p99_wall} max_wall_us={max_wall} refresh_period_us={refresh} latch_drops={drops} vsync_misses={misses} present_errors={errors} status_mode={status_mode} status_enqueue_p99_us={status_enqueue_p99} status_worker_errors={status_worker_errors} status_sequences={status_submitted}/{status_written}"
        )
        .into());
    }
    Ok(())
}

fn u64_field(value: &Value, field: &str, default: u64) -> u64 {
    value.get(field).and_then(Value::as_u64).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing_run(run: u64) -> Value {
        json!({
            "run": run,
            "startup": {
                "ignored_frames": 3,
                "max_wall_us": 500_000
            },
            "steady_state": {
                "frames": 1_797,
                "average_fps": 59.9,
                "p99_work_us": 10_000,
                "p99_wall_us": 16_000,
                "max_wall_us": 16_667,
                "refresh_period_us": 16_667,
                "over_budget_frames": 0,
                "vsync_misses": 0,
                "presentation_failures": [],
                "physical_refresh": {
                    "refresh_hz": 60.0,
                    "unique_fps": 60.0,
                    "repeated_refreshes": 0,
                    "long_completion_intervals": []
                }
            },
            "latch_drop_delta": 0,
            "present_errors": 0,
            "status_publishing": {
                "mode": "async",
                "enqueue_p99_us": 100,
                "worker_errors": 0,
                "final_submitted_sequence": 31,
                "final_written_sequence": 31
            },
        })
    }

    #[test]
    fn installed_screensaver_requires_exactly_one_passing_run() {
        assert!(evaluate_summary(&json!({"runs": [passing_run(1)]})).is_ok());
        assert!(evaluate_summary(&json!({"runs": [passing_run(1), passing_run(2)]})).is_err());
    }

    #[test]
    fn installed_screensaver_rejects_performance_or_platform_errors() {
        let mut slow = passing_run(1);
        slow["steady_state"]["physical_refresh"]["unique_fps"] = json!(40.0);
        assert!(evaluate_run(&slow).is_err());
        let mut dropped = passing_run(1);
        dropped["latch_drop_delta"] = json!(1);
        assert!(evaluate_run(&dropped).is_err());
        let mut late_start = passing_run(1);
        late_start["startup"]["max_wall_us"] = json!(5_000_000);
        assert!(evaluate_run(&late_start).is_ok());
        let mut timing_overrun = passing_run(1);
        timing_overrun["steady_state"]["over_budget_frames"] = json!(1);
        assert!(evaluate_run(&timing_overrun).is_ok());
        let mut presentation_failure = passing_run(1);
        presentation_failure["steady_state"]["presentation_failures"] =
            json!([{"frame": 42, "kind": "sequence-gap"}]);
        assert!(evaluate_run(&presentation_failure).is_err());
        let mut repeated = passing_run(1);
        repeated["steady_state"]["physical_refresh"]["repeated_refreshes"] = json!(1);
        assert!(evaluate_run(&repeated).is_err());
        let mut long_gap = passing_run(1);
        long_gap["steady_state"]["physical_refresh"]["long_completion_intervals"] =
            json!([{"frame": 42, "interval_us": 33_334}]);
        assert!(evaluate_run(&long_gap).is_err());
        let mut blocking_status = passing_run(1);
        blocking_status["status_publishing"]["enqueue_p99_us"] = json!(250);
        assert!(evaluate_run(&blocking_status).is_err());
    }

    #[test]
    fn catalog_lifecycle_requires_a_valid_nonempty_catalog() {
        let passing = json!({
            "scenario": "catalog-lifecycle",
            "catalog": {
                "valid": true,
                "systems": [{"system": "atari2600", "games": 2}]
            }
        });
        assert!(evaluate_catalog_lifecycle_summary(&passing).is_ok());

        let mut invalid = passing.clone();
        invalid["catalog"]["valid"] = json!(false);
        assert!(evaluate_catalog_lifecycle_summary(&invalid).is_err());

        let mut empty = passing;
        empty["catalog"]["systems"] = json!([]);
        assert!(evaluate_catalog_lifecycle_summary(&empty).is_err());
    }
}
