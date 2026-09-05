// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! One measured legacy upload to its normal SD staging path; never activate it.
use super::*;
use crate::commands::device::TransferCheckArgs;

pub(super) fn run(args: &TransferCheckArgs, config: &NativeDeviceConfig) -> Result<()> {
    let session = connect_with(&config.connection, 10)?;
    session.set_timeout(120_000);
    let paths = platform_manifest_contract::DEVELOPMENT_PATHS;
    if args.fetch_installed {
        if args.artifact.exists() {
            return Err("refusing to overwrite the local comparison artifact".into());
        }
        get(&session, paths.gui, &args.artifact)?;
    }
    let bytes = fs::metadata(&args.artifact)?.len();
    if bytes == 0 {
        return Err("comparison artifact is empty".into());
    }
    let hash = file_sha256(args.artifact.clone())?;
    let upload = format!("{}.upload", paths.gui);
    let part = format!("{}.part", upload);
    let lock = format!("{}/deploy.lock", paths.root);
    let prepare = format!(
        "set -eu; test ! -e {0}; test ! -e {1}; (set -C; : > {2})",
        sh(&upload),
        sh(&part),
        sh(&lock)
    );
    let _signals = AttendedOperationSignalGuard::install();
    let prepared = exec(&session, &prepare, true)?;
    if prepared.rc != 0 {
        return Err("legacy upload staging is already in use; nothing was removed".into());
    }
    let started = Instant::now();
    let transfer = agent_runtime_upload_at(
        config.agent()?,
        &args.artifact,
        bytes,
        &hash,
        Duration::from_secs(120),
    );
    let elapsed_ms = started.elapsed().as_millis();
    // The uploader acknowledges only after hash verification, file sync, rename
    // and directory sync. Cleanup is deliberately outside the timed interval.
    let cleanup = exec(
        &session,
        &format!(
            "set -eu; rm -f {0} {1} {2}; test ! -e {0}; test ! -e {1}; test ! -e {2}",
            sh(&upload),
            sh(&part),
            sh(&lock)
        ),
        true,
    );
    let cleanup_error = match cleanup {
        Ok(output) if output.rc == 0 => None,
        Ok(_) => Some("legacy staging cleanup failed".to_owned()),
        Err(error) => Some(error.to_string()),
    };
    match transfer {
        Ok(result) => {
            println!(
                "{}",
                json!({"system":"legacy", "bytes":bytes,"sha256":hash,"elapsed_ms":elapsed_ms,"receive_ms":result.receive_ms,"bytes_per_second":result.bytes_per_second,"mb_per_second":result.bytes_per_second as f64 / 1_000_000.0,"mbit_per_second":result.bytes_per_second as f64 * 8.0 / 1_000_000.0,"cleanup_error":cleanup_error})
            );
            if let Some(error) = cleanup_error {
                return Err(error.into());
            }
            if attended_operation_interrupted() {
                return Err("comparison interrupted after cleanup".into());
            }
            Ok(())
        }
        Err(error) => {
            eprintln!(
                "{}",
                json!({"system":"legacy","bytes":bytes,"sha256":hash,"elapsed_ms":elapsed_ms,"error":error.to_string(),"cleanup_error":cleanup_error})
            );
            Err(error)
        }
    }
}
