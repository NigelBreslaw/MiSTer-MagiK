// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::tracefs::{SCHEDULER_TRACE_SPEC, TracefsCapture, summarize_scheduler_trace};
use super::*;
use std::fmt::Write as _;

pub(super) fn profile_installed_scheduler_trace(
    config: &NativeDeviceConfig,
    output_dir: &Path,
) -> Result<String> {
    fs::create_dir_all(output_dir)?;
    let session = connect_with(&config.connection, 30)?;
    let manifest = remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE)
        .ok_or("development manifest is unavailable before scheduler tracing")?;
    let installed_identity = streamline_installed_identity(&session, config.agent()?, &manifest)?;
    let original_reply = exec_checked_output(
        &session,
        "query scheduler trace display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    if parse_display_reply_pending(original_reply.stdout.trim())?.is_some() {
        return Err("scheduler trace cannot start during a display transaction".into());
    }
    let original_id = parse_display_reply_active(original_reply.stdout.trim())?;
    let original_mode = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == original_id)
        .copied()
        .ok_or_else(|| format!("scheduler trace cannot restore unknown mode {original_id}"))?;
    let capture_mode = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == "hdmi-1280x720p60")
        .copied()
        .ok_or("missing scheduler trace display mode")?;
    let capture = TracefsCapture::new(&session, output_dir, SCHEDULER_TRACE_SPEC);
    capture.prepare()?;
    let display_result = apply_confirmed_display_mode(config, capture_mode, "scheduler trace");
    if let Err(error) = display_result {
        let cleanup = capture.cleanup();
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "scheduler trace display setup failed: {error}; trace cleanup failed: {cleanup}"
            )
            .into()),
        };
    }

    let run_result = (|| -> Result<(Value, Value, String, String, String, String)> {
        capture.start()?;
        let route = run_gui_frame_profile_arm(config, &session, &output_dir.join("route"), false);
        let stop = capture.stop();
        let retained = stop.and_then(|()| capture.retain("scheduler-trace.txt"));
        let route = route?;
        let retained = retained?;
        let raw = fs::read_to_string(&retained.raw_path)?;
        let (scheduler, threads, cpus, irqs) = summarize_scheduler_trace(&raw, &retained.stats)?;
        fs::write(output_dir.join("thread-summary.tsv"), &threads)?;
        fs::write(output_dir.join("cpu-summary.tsv"), &cpus)?;
        fs::write(output_dir.join("irq-summary.tsv"), &irqs)?;
        let trace = json!({
            "path": "scheduler-trace.txt",
            "sha256": retained.sha256,
            "clock": "mono",
            "buffer_kb_per_cpu": 4096,
            "capabilities": parse_trace_capabilities(&retained.capabilities),
            "stats": "trace-stats.txt",
        });
        Ok((
            route,
            scheduler,
            threads,
            cpus,
            irqs,
            serde_json::to_string(&trace)?,
        ))
    })();
    let trace_cleanup = capture.cleanup();
    let launcher_restore = launcher_restart(
        &session,
        &LauncherRestartOptions {
            clear_env: true,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str().into(),
            timeout_secs: 45,
            ..LauncherRestartOptions::default()
        },
    );
    let route_cleanup = exec_checked(
        &session,
        "clean scheduler trace GUI route state",
        &gui_profile_route_cleanup_command(),
    );
    let display_restore =
        apply_confirmed_display_mode(config, original_mode, "scheduler trace display restoration");
    let (route, scheduler, _threads, _cpus, _irqs, trace) = match (
        run_result,
        trace_cleanup,
        launcher_restore,
        route_cleanup,
        display_restore,
    ) {
        (Ok(result), Ok(()), Ok(()), Ok(()), Ok(())) => result,
        (run, trace, launcher, route, display) => {
            return Err(format!(
                "scheduler trace failed: run={:?}; trace_cleanup={:?}; launcher_restore={:?}; route_cleanup={:?}; display_restore={:?}",
                run.err(),
                trace.err(),
                launcher.err(),
                route.err(),
                display.err()
            )
            .into());
        }
    };
    let final_manifest = remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE)
        .ok_or("development manifest is unavailable after scheduler tracing")?;
    if final_manifest != manifest {
        return Err("installed platform manifest changed during scheduler tracing".into());
    }
    let final_identity = streamline_installed_identity(&session, config.agent()?, &final_manifest)?;
    if final_identity != installed_identity {
        return Err("installed identity changed during scheduler tracing".into());
    }
    let trace: Value = serde_json::from_str(&trace)?;
    let summary = json!({
        "schema": "mister-magik-scheduler-trace-v1",
        "artifact_status": "passed",
        "product_quality_status": "not-applicable-attribution-only",
        "performance_authority": "diagnostic-observer",
        "identity": installed_identity_json(&installed_identity),
        "display_mode": capture_mode.id,
        "refresh_hz": 60,
        "workload": "fixed-gui-profile-route",
        "route": route,
        "trace": trace,
        "scheduler": scheduler,
        "artifacts": {
            "threads": "thread-summary.tsv",
            "cpus": "cpu-summary.tsv",
            "interrupts": "irq-summary.tsv",
            "capabilities": "trace-capabilities.tsv",
        },
    });
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    fs::write(output_dir.join("report.md"), scheduler_report(&summary)?)?;
    serde_json::to_string(&summary).map_err(Into::into)
}

fn installed_identity_json(identity: &StreamlineInstalledIdentity) -> Value {
    json!({
        "boot_id": identity.boot_id,
        "platform_manifest_sha256": identity.platform_manifest_sha256,
        "magik_revision": identity.magik_revision,
        "gui_sha256": identity.gui_sha256,
        "agent_sha256": identity.agent_sha256,
        "agent_bytes": identity.agent_bytes,
        "agent_version": identity.agent_version,
    })
}

fn parse_trace_capabilities(text: &str) -> Value {
    Value::Array(
        text.lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(event, status)| json!({"event": event, "status": status}))
            .collect(),
    )
}

fn scheduler_report(summary: &Value) -> Result<String> {
    let scheduler = &summary["scheduler"];
    let mut report = String::from("# Scheduler trace\n\n");
    writeln!(
        report,
        "Artifact: **{}**\n",
        summary["artifact_status"].as_str().unwrap_or("failed")
    )?;
    writeln!(
        report,
        "- Duration: {} us",
        scheduler["duration_us"].as_u64().unwrap_or(0)
    )?;
    writeln!(
        report,
        "- Dual-core overlap: {} us ({:.3}%)",
        scheduler["dual_core_overlap_us"].as_u64().unwrap_or(0),
        scheduler["dual_core_overlap_pct"].as_f64().unwrap_or(0.0)
    )?;
    writeln!(
        report,
        "- Parsed events: {}",
        scheduler["event_count"].as_u64().unwrap_or(0)
    )?;
    writeln!(
        report,
        "- Trace overruns: {}\n",
        scheduler["trace_overruns"].as_u64().unwrap_or(u64::MAX)
    )?;
    report.push_str("This capture is diagnostic attribution only; the unprofiled route remains product-quality authority.\n");
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_preserve_required_and_missing_status() {
        let capabilities = parse_trace_capabilities(
            "sched:sched_switch\trequired\nblock:block_rq_issue\tmissing\n",
        );
        assert_eq!(capabilities.as_array().map(Vec::len), Some(2));
        assert_eq!(capabilities[1]["status"], "missing");
    }

    #[test]
    fn scheduler_report_keeps_attribution_separate_from_quality() {
        let summary = json!({
            "artifact_status": "passed",
            "scheduler": {
                "duration_us": 1000,
                "dual_core_overlap_us": 250,
                "dual_core_overlap_pct": 25.0,
                "event_count": 42,
                "trace_overruns": 0,
            }
        });
        let report = scheduler_report(&summary).unwrap();
        assert!(report.contains("diagnostic attribution only"));
        assert!(report.contains("25.000%"));
    }
}
