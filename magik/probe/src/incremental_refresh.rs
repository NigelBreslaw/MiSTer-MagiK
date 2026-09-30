// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Incremental rebuild, publication and exact watch/artifact oracle on SD.
use mister_magik_catalog::{
    fast_catalog_refresh as refresh, fast_catalog_sources as sources, fast_five_catalog as fast,
    io_test_metrics as metrics,
};
use std::path::PathBuf;
use std::time::Instant;
const FIXTURE: &str = "479aef247eff7035950eeb58039a224002a49bd6d0c60a638f7ecca562d55e10";
const GAMES: usize = 100;
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> Result<Fixture, String> {
    #[cfg(all(target_os = "linux", target_arch = "arm", not(test)))]
    let base = PathBuf::from("/media/fat/mister-magik-dev/benchmark-fixtures");
    #[cfg(not(all(target_os = "linux", target_arch = "arm", not(test))))]
    let base = std::env::temp_dir().join("mister-magik-benchmark-fixtures");
    let root = base.join(format!(
        "incremental-refresh-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let fixture = Fixture(root);
    let core = fixture.0.join("_Console/SNES.rbf");
    std::fs::create_dir_all(core.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(core, b"core").map_err(|e| e.to_string())?;
    for i in 0..GAMES {
        let p = fixture
            .0
            .join(format!("games/SNES/Publisher{}/Game{i:03}.sfc", i % 4));
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(p, b"rom").map_err(|e| e.to_string())?;
    }
    Ok(fixture)
}
pub(super) fn run() -> Result<serde_json::Value, String> {
    let fixture = fixture()?;
    let _logs = crate::preview_shards::QuietLogs::new(&fixture.0)?;
    let catalog = fixture.0.join("catalog");
    refresh::build_fresh_catalog(&fixture.0, &catalog)?;
    let mut samples = Vec::new();
    let work_count = GAMES;
    for repetition in 0..2 {
        // Mutate only this isolated source before timing. Cold setup is excluded.
        std::fs::write(
            fixture
                .0
                .join(format!("games/SNES/Publisher0/Added{repetition}.sfc")),
            b"rom",
        )
        .map_err(|e| e.to_string())?;
        // SD directory timestamps can be coarse. Give each source mutation an
        // explicit distinct timestamp outside timing, so this is a changed-system
        // comparison rather than a race with the metadata-only unchanged path.
        std::fs::File::open(fixture.0.join("games/SNES/Publisher0"))
            .map_err(|e| e.to_string())?
            .set_times(std::fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH
                    + std::time::Duration::from_secs(1_600_000_000 + 10 * repetition as u64),
            ))
            .map_err(|e| e.to_string())?;
        let before = (
            metrics::source_walks(),
            metrics::watch_tree_walks(),
            metrics::watch_reuses(),
        );
        let start = Instant::now();
        let report = refresh::execute_fast_refresh(
            &fixture.0,
            &catalog,
            refresh::FastCatalogRefreshRequest::Update,
        )?;
        let duration_ns = start.elapsed().as_nanos() as u64;
        let counts = (
            metrics::source_walks() - before.0,
            metrics::watch_tree_walks() - before.1,
            metrics::watch_reuses() - before.2,
        );
        if report.updated != 1 || report.failed_retained != 0 {
            return Err(format!(
                "incremental outcome/count mismatch: updated={} retained={} games={} checks={:?}",
                report.updated, report.failed_retained, report.games, report.plan.checks
            ));
        }
        let manifest = refresh::read_latest_refresh_manifest(&catalog)?;
        let reference = manifest
            .systems
            .iter()
            .find(|s| s.system_id == "snes")
            .ok_or("published system missing")?;
        if reference.games != (GAMES + repetition + 1) as u64 {
            return Err("published game count mismatch".into());
        }
        let published_watch = refresh::read_system_watch(&catalog, reference)?;
        let independent_watch = refresh::capture_system_watch(&fixture.0, "snes")?;
        if published_watch != independent_watch {
            let differing_directory = published_watch
                .directories
                .iter()
                .zip(&independent_watch.directories)
                .find(|(a, b)| a != b);
            return Err(format!(
                "watch mismatch: roots={:?}/{:?}; first directory difference={differing_directory:?}",
                published_watch.roots, independent_watch.roots
            ));
        }
        let oracle = sources::build_independent_fast_snapshot(&fixture.0)?.0;
        fast::verify_snapshot_artifacts(
            &catalog,
            &oracle,
            mister_magik_catalog::shard_registry::production_registry_limits(),
        )?;
        samples.push(serde_json::json!({"repetition":repetition,"fixture_identity":FIXTURE,"work_count":work_count,"duration_ns":duration_ns,"ns_per_pixel":duration_ns as f64/work_count as f64,
            "source_walks":counts.0,"fallback_watch_walks":counts.1,"observation_reuses":counts.2,"published_games":reference.games,"reported_games":report.games,"updated_systems":report.updated,"failed_retained":report.failed_retained}));
    }
    let sha = std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256")
        .map_err(|_| "native artifact identity missing")?;
    Ok(
        serde_json::json!({"schema_version":1,"workload":"incremental-refresh","mode":"timing","artifact_sha256":sha,"correctness":"passed","work_count":work_count,
        "fixture":{"identity":FIXTURE,"initial_games":GAMES,"directories":5,"mutations":"one additional loose file per update","storage":"isolated development SD fixture; RAII cleanup",
            "timed_work":"actual incremental planning, source rebuild, watch capture and artifact/state publication","oracle":"independent full watch plus full snapshot artifact parity outside timing","observation":"same test-only source/watch counters","source_timestamp":"distinct 10s-spaced publisher directory mtimes outside timing"},"samples":samples}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_incremental_refresh_matches_fresh_artifacts() {
        let f = fixture().unwrap();
        let c = f.0.join("catalog");
        refresh::build_fresh_catalog(&f.0, &c).unwrap();
        std::fs::write(f.0.join("games/SNES/Publisher0/Added.sfc"), b"rom").unwrap();
        let r = refresh::execute_fast_refresh(&f.0, &c, refresh::FastCatalogRefreshRequest::Update)
            .unwrap();
        assert_eq!(r.updated, 1);
        assert_eq!(r.failed_retained, 0);
        let oracle = sources::build_independent_fast_snapshot(&f.0).unwrap().0;
        fast::verify_snapshot_artifacts(
            &c,
            &oracle,
            mister_magik_catalog::shard_registry::production_registry_limits(),
        )
        .unwrap();
    }
}
