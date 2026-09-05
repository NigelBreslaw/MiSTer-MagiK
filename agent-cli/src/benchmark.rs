// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::device::DeviceClient;
use crate::error::AgentResult;
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Physical Main/proxy/kernel input qualification. Application scenarios use 2.0.
pub fn execute(repository: &Path, reporter: &mut Reporter<'_>) -> AgentResult<Outcome> {
    let head = crate::git::value(repository, &["rev-parse", "HEAD"])?;
    if !crate::git::value(repository, &["status", "--porcelain"])?.is_empty() {
        return Err("benchmark requires a clean exact-commit worktree".into());
    }
    let mut device = DeviceClient::default();
    device.read(crate::NativeDevice::discover)?;
    device.read(crate::NativeDevice::verify_development_platform)?;
    require_active_development_runtime(&device.read(crate::NativeDevice::read_active_runtime)?)?;
    device.read(crate::NativeDevice::verify_development_health)?;
    let manifest = device.read(crate::NativeDevice::read_development_manifest)?;
    let reconciliation = crate::deploy::reconcile(repository, &manifest, &head);
    if reconciliation.decision != crate::deploy::DeliveryDecision::NoOp {
        return Err(format!(
            "benchmark requires delivery reconciliation to be no-op, found {}; run scripts/agent deliver platform first",
            reconciliation.decision.label()
        )
        .into());
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    let output_dir = repository
        .join("build/agent-benchmarks")
        .join("input-integrity")
        .join(timestamp.to_string());

    reporter.emit(
        EventKind::Progress,
        "input-integrity",
        "driving bounded pulses through Main proxy v2 and the kernel input path",
        Some(35),
    )?;
    let detail = device.mutate(|device| device.verify_input_integrity(&output_dir))?;
    let summary: Value = serde_json::from_str(&detail).map_err(|error| error.to_string())?;
    evaluate_input_integrity_summary(&summary)?;
    device.read(crate::NativeDevice::verify_development_health)?;
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

fn evaluate_input_integrity_summary(summary: &Value) -> AgentResult<()> {
    let protocol = summary.get("protocol").and_then(Value::as_u64);
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-input-integrity-v2")
        || summary.get("status").and_then(Value::as_str) != Some("passed")
        || !matches!(protocol, Some(2 | 3))
        || summary.get("lost_actions").and_then(Value::as_u64) != Some(0)
        || summary.get("duplicated_actions").and_then(Value::as_u64) != Some(0)
        || summary.get("proxy_write_failures").and_then(Value::as_u64) != Some(0)
        || summary.get("journal_overflows").and_then(Value::as_u64) != Some(0)
        || summary.get("sequence_gaps").and_then(Value::as_u64) != Some(0)
    {
        return Err("input integrity qualification did not satisfy the zero-loss gates".into());
    }
    Ok(())
}

fn require_active_development_runtime(active: &crate::host::ActiveRuntime) -> AgentResult<()> {
    if active.is_development_launcher() {
        Ok(())
    } else {
        Err(format!(
            "benchmark requires the active development launcher, found {}; run scripts/agent deliver platform",
            active.description()
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_integrity_requires_every_zero_loss_gate() {
        let passing = json!({
            "schema": "mister-magik-input-integrity-v2",
            "status": "passed",
            "protocol": 2,
            "lost_actions": 0,
            "duplicated_actions": 0,
            "proxy_write_failures": 0,
            "journal_overflows": 0,
            "sequence_gaps": 0,
        });
        evaluate_input_integrity_summary(&passing).unwrap();
        let mut proxy_v3 = passing.clone();
        proxy_v3["protocol"] = json!(3);
        evaluate_input_integrity_summary(&proxy_v3).unwrap();
        let mut unsupported = passing.clone();
        unsupported["protocol"] = json!(4);
        assert!(evaluate_input_integrity_summary(&unsupported).is_err());
        let mut failed = passing;
        failed["lost_actions"] = json!(1);
        assert!(evaluate_input_integrity_summary(&failed).is_err());
    }
}
