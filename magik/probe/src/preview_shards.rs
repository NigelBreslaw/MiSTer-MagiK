// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Actual incremental source rebuilds with isolated, on-storage metadata.
use mister_magik_catalog::{
    fast_catalog_sources as sources, fast_five_catalog as fast, io_test_metrics,
    runtime_metadata as meta,
};
use std::path::{Path, PathBuf};
use std::time::Instant;
const ITEMS: usize = 10_000;
const FIXTURE: &str = "a2c50bc0f679d7118307f17f0a6846d335a1d2afeeb3fa1756cd197624720bdf";
// Catalog diagnostics can exceed the service output bound. Keep the same
// production log writes in an isolated fixture file, preserving the result pipe.
struct QuietLogs {
    #[cfg(unix)]
    saved: std::os::fd::OwnedFd,
}
impl QuietLogs {
    fn new(root: &Path) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let file =
                std::fs::File::create(root.join("catalog.log")).map_err(|e| e.to_string())?;
            let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
            if saved < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let saved = unsafe { std::os::fd::OwnedFd::from_raw_fd(saved) };
            if unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) } < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(Self { saved })
        }
        #[cfg(not(unix))]
        {
            let _ = root;
            Ok(Self {})
        }
    }
}
impl Drop for QuietLogs {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::dup2(self.saved.as_raw_fd(), libc::STDERR_FILENO);
            }
        }
    }
}
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> Result<Fixture, String> {
    #[cfg(target_os = "linux")]
    let base = PathBuf::from("/media/fat/mister-magik-dev/benchmark-fixtures");
    #[cfg(not(target_os = "linux"))]
    let base = std::env::temp_dir().join("mister-magik-benchmark-fixtures");
    let root = base.join(format!(
        "preview-shards-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let fixture = Fixture(root);
    for relative in [
        "_Console/SNES_20260826.rbf",
        "_Console/Saturn_20260826.rbf",
        "games/SNES/Title00000.sfc",
        "games/Saturn/Title00000.chd",
    ] {
        let path = fixture.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(path, b"fixture").map_err(|e| e.to_string())?;
    }
    let mut builder = meta::MetadataFileBuilder::new();
    for system in ["snes", "saturn"] {
        let items = (0..ITEMS)
            .map(|i| meta::SoftwareItem {
                name: format!("item{i:05}"),
                parent_name: Some("family".into()),
                description: format!("Title{i:05}"),
                year: None,
                publisher: None,
                region: None,
            })
            .collect();
        builder.add_software(
            system,
            &meta::SoftwareShard {
                items,
                ..Default::default()
            },
        )?;
    }
    builder.write_to(&fixture.0.join("mister-magik").join(meta::FILE_NAME))?;
    Ok(fixture)
}
fn execute(root: &Path, case: &str) -> Result<(), String> {
    let snapshot = if case == "both" {
        sources::build_independent_fast_snapshot(root)?.0
    } else {
        let unused = fast::FastFiveSnapshot {
            schema: fast::FAST_FIVE_SNAPSHOT_SCHEMA.into(),
            source_fingerprint: "0".repeat(64),
            systems: vec![],
        };
        let (system, _) = sources::rebuild_independent_system(root, &unused, case)?
            .ok_or("fixture system missing")?;
        fast::FastFiveSnapshot {
            systems: vec![system],
            ..unused
        }
    };
    let expected = if case == "both" { 2 } else { 1 };
    if snapshot.systems.len() != expected {
        return Err("unexpected fixture system count".into());
    }
    for system in snapshot.systems {
        if system.games.len() != 1
            || system.games[0].preview_asset_key
                != format!("mame-software__{}__family", system.system_id)
            || system.games[0].launch_plan.is_none()
        {
            return Err(format!(
                "preview/launch parity failed: {}",
                system.system_id
            ));
        }
    }
    Ok(())
}
pub(super) fn run() -> Result<serde_json::Value, String> {
    let fixture = fixture()?;
    let _logs = QuietLogs::new(&fixture.0)?;
    let mut samples = Vec::new();
    let work_count = 3;
    for repetition in 0..2 {
        let mut cases = Vec::new();
        let mut duration_ns = 0u64;
        for case in ["snes", "saturn", "both"] {
            let before = io_test_metrics::software_decodes();
            let start = Instant::now();
            execute(&fixture.0, case)?;
            let ns = start.elapsed().as_nanos() as u64;
            let decodes = io_test_metrics::software_decodes() - before;
            cases.push(
                serde_json::json!({"requested":case,"duration_ns":ns,"software_decodes":decodes}),
            );
            duration_ns += ns;
        }
        samples.push(serde_json::json!({"repetition":repetition,"fixture_identity":FIXTURE,"work_count":work_count,"duration_ns":duration_ns,"ns_per_pixel":duration_ns as f64/work_count as f64,"cases":cases}));
    }
    let sha = std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256")
        .map_err(|_| "native artifact identity missing")?;
    Ok(
        serde_json::json!({"schema_version":1,"workload":"preview-shards","mode":"timing","artifact_sha256":sha,"correctness":"passed","work_count":work_count,
        "fixture":{"identity":FIXTURE,"items_per_shard":ITEMS,"systems":["snes","saturn"],"storage":"isolated development SD fixture; RAII cleanup","timed_work":"actual per-system rebuild or both-system build, including profile discovery, loader, decode, title indexing and enrichment","observation":"same test-only per-thread software decode counter on both revisions"},"samples":samples}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_rebuild_keeps_preview_family_and_launch_plan() {
        let f = fixture().unwrap();
        for case in ["snes", "saturn", "both"] {
            execute(&f.0, case).unwrap();
        }
    }
}
