// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Explicit native validation probe; never changes the installed catalog.

use std::path::Path;

const CHILD_COMMAND: &str = "__catalog-equivalence";

fn requested_directory() -> Option<std::path::PathBuf> {
    let directory = std::path::PathBuf::from(std::env::var_os("MISTER_MAGIK2_PROFILE_DIR")?);
    let id = directory.file_name()?.to_str()?;
    (directory.starts_with("/tmp/mister-magik2/profiles") && id.starts_with("catalog-equivalence-"))
        .then_some(directory)
}

pub fn start_requested_probe() {
    let Some(directory) = requested_directory() else {
        return;
    };
    std::thread::Builder::new()
        .name("catalog-equivalence".to_owned())
        .spawn(move || {
            if let Err(error) = run_child_process()
                && let Err(persist_error) = persist(&directory, Err(error))
            {
                crate::ui_errln!("catalog_equivalence report failed: {persist_error}");
            }
        })
        .expect("start explicitly requested catalog equivalence supervisor");
}

fn run_child_process() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = std::process::Command::new(executable);
    command.arg(CHILD_COMMAND);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let parent = std::process::id() as libc::pid_t;
        // SAFETY: the post-fork hook only calls async-signal-safe syscalls.
        // Stopping the app must also stop its isolated scratch builder.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                Ok(())
            });
        }
    }
    let status = command.status().map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("catalog probe child exited: {status}"))
    }
}

pub fn run_requested_child() -> bool {
    if std::env::args().nth(1).as_deref() != Some(CHILD_COMMAND) {
        return false;
    }
    let Some(directory) = requested_directory() else {
        crate::ui_errln!("catalog equivalence child requires an explicit native probe run");
        std::process::exit(2);
    };
    let destination = Path::new("/media/fat/mister-magik2/catalog-equivalence")
        .join(directory.file_name().expect("validated probe name"));
    if let Err(error) = persist(&directory, run(&destination)) {
        crate::ui_errln!("catalog_equivalence report failed: {error}");
        std::process::exit(1);
    }
    true
}

fn persist(directory: &Path, result: Result<serde_json::Value, String>) -> Result<(), String> {
    let value = match result {
        Ok(value) => value,
        Err(error) => serde_json::json!({"ok":false,"error":error}),
    };
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?;
    let temporary = directory.join("catalog.next");
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(temporary, directory.join("catalog.json")).map_err(|error| error.to_string())
}

fn run(destination: &Path) -> Result<serde_json::Value, String> {
    use mister_magik_catalog::{catalog_acceptance, fast_catalog_refresh, shard_registry};
    if destination.exists() {
        return Err(format!(
            "fresh destination already exists: {}",
            destination.display()
        ));
    }
    let build = fast_catalog_refresh::build_fresh_catalog(Path::new("/media/fat"), destination)?;
    let inspection = catalog_acceptance::inspect_catalog(destination)?;
    let manifest = shard_registry::read_latest_manifest(
        destination,
        shard_registry::production_registry_limits(),
    )
    .map_err(|error| error.to_string())?;
    let refresh = fast_catalog_refresh::read_latest_refresh_manifest(destination)?;
    let systems = refresh
        .systems
        .iter()
        .map(|system| {
            let registry = manifest
                .systems
                .iter()
                .find(|entry| entry.system_id.as_str() == system.system_id)
                .ok_or_else(|| format!("missing registry system {}", system.system_id))?;
            Ok(serde_json::json!({
                "system_id":system.system_id,
                "games":system.games,
                "variants":system.variants,
                "rows_sha256":system.row_fingerprint,
                "display_title":registry.display_title,
                "section":registry.section,
                "family":registry.family,
                "order":registry.order,
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(serde_json::json!({
        "ok":true,
        "artifact_sha256":std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256").unwrap_or_default(),
        "destination":destination,
        "systems":systems,
        "inspection":inspection,
        "build":build,
    }))
}
