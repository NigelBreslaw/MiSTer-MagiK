// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::device::DeviceClient;
use crate::error::AgentResult;
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::{DeviceRequest, Layout};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const BENCHMARK_DISPLAY_MODE: &str = "hdmi-1280x720p60";

pub fn execute(repository: &Path, reporter: &mut Reporter<'_>) -> AgentResult<Outcome> {
    let mut device = DeviceClient::default();
    reporter.emit(
        EventKind::Progress,
        "preflight",
        "benchmark installed screensaver preflight",
        Some(10),
    )?;
    device.execute(DeviceRequest::Discover)?;
    device.execute(DeviceRequest::VerifyDevelopmentPlatform)?;
    device.execute(DeviceRequest::VerifyHealth(Layout::Development))?;
    let manifest = device.execute(DeviceRequest::ReadDevelopmentManifest)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    let output_dir = repository
        .join("build/agent-benchmarks/screensaver")
        .join(timestamp.to_string());

    reporter.emit(
        EventKind::Progress,
        "profile",
        "profiling installed screensaver twice",
        Some(35),
    )?;
    let detail = device.execute(DeviceRequest::ProfileInstalledScreensaver {
        output_dir: output_dir.clone(),
        display_mode: BENCHMARK_DISPLAY_MODE.into(),
    })?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    device.execute(DeviceRequest::VerifyHealth(Layout::Development))?;
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

fn evaluate_summary(summary: &Value) -> AgentResult<()> {
    let runs = summary
        .get("runs")
        .and_then(Value::as_array)
        .ok_or("screensaver benchmark summary has no runs")?;
    if runs.len() != 2 {
        return Err(format!(
            "screensaver benchmark expected two runs, received {}",
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
    let fps = steady
        .get("average_fps")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let frames = u64_field(steady, "frames", 0);
    let p99_work = u64_field(steady, "p99_work_us", u64::MAX);
    let p99_wall = u64_field(steady, "p99_wall_us", u64::MAX);
    let max_wall = u64_field(steady, "max_wall_us", u64::MAX);
    let refresh = u64_field(steady, "refresh_period_us", 16_667);
    let over_budget = u64_field(steady, "over_budget_frames", u64::MAX);
    let drops = u64_field(run, "latch_drop_delta", u64::MAX);
    let misses = u64_field(steady, "vsync_misses", u64::MAX);
    let errors = u64_field(run, "present_errors", u64::MAX);
    if fps < 55.0 || frames == 0 || over_budget != 0 || drops != 0 || misses != 0 || errors != 0 {
        return Err(format!(
            "screensaver profile run {id} failed after warm-up: frames={frames} fps={fps:.1} over_budget_frames={over_budget} p99_work_us={p99_work} p99_wall_us={p99_wall} max_wall_us={max_wall} refresh_period_us={refresh} latch_drops={drops} vsync_misses={misses} present_errors={errors}"
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
                "vsync_misses": 0
            },
            "latch_drop_delta": 0,
            "present_errors": 0,
        })
    }

    #[test]
    fn installed_screensaver_requires_exactly_two_passing_runs() {
        assert!(evaluate_summary(&json!({"runs": [passing_run(1), passing_run(2)]})).is_ok());
        assert!(evaluate_summary(&json!({"runs": [passing_run(1)]})).is_err());
    }

    #[test]
    fn installed_screensaver_rejects_performance_or_platform_errors() {
        let mut slow = passing_run(1);
        slow["steady_state"]["average_fps"] = json!(40.0);
        assert!(evaluate_run(&slow).is_err());
        let mut dropped = passing_run(1);
        dropped["latch_drop_delta"] = json!(1);
        assert!(evaluate_run(&dropped).is_err());
        let mut late_start = passing_run(1);
        late_start["startup"]["max_wall_us"] = json!(5_000_000);
        assert!(evaluate_run(&late_start).is_ok());
        let mut steady_drop = passing_run(1);
        steady_drop["steady_state"]["over_budget_frames"] = json!(1);
        assert!(evaluate_run(&steady_drop).is_err());
    }
}
