// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Standalone full-versus-delta reconciliation benchmark.

use crate::catalog_classify::SystemId;
use crate::catalog_domain::ScanUnitId;
use crate::reconciliation_executor::{
    execute_reconciliation, MaterializedSystem, ReconciliationError, ReconciliationMaterializer,
};
use crate::shard_registry::RegistryLimits;
use crate::sharded_catalog::{PlannedSystem, PlannedSystemAction, ReconcilePlan, ReconcileReason};
use crate::system_shard::SystemGame;
use std::path::Path;
use std::time::Instant;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebuildBenchmarkOutcome {
    pub full_us: u64,
    pub delta_us: u64,
    pub full_systems: usize,
    pub delta_systems: usize,
    pub games_per_system: usize,
}

impl RebuildBenchmarkOutcome {
    pub fn elapsed_speedup(&self) -> f64 {
        self.full_us as f64 / self.delta_us.max(1) as f64
    }

    pub fn work_ratio(&self) -> f64 {
        self.full_systems as f64 / self.delta_systems.max(1) as f64
    }
}

pub fn run_rebuild_benchmark(
    storage_root: &Path,
    systems: usize,
    games_per_system: usize,
    limits: RegistryLimits,
) -> Result<RebuildBenchmarkOutcome, ReconciliationError> {
    if systems < 10 || games_per_system == 0 {
        return Err(ReconciliationError::new(
            "benchmark",
            "benchmark needs at least 10 systems and one game per system",
        ));
    }
    if storage_root.exists() {
        let mut entries = storage_root
            .read_dir()
            .map_err(|error| ReconciliationError::new("benchmark", error.to_string()))?;
        if entries.next().is_some() {
            return Err(ReconciliationError::new(
                "benchmark",
                "benchmark storage must be empty",
            ));
        }
    }
    let system_ids = (0..systems)
        .map(|index| SystemId::parse(&format!("fixture-{index:04}")))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ReconciliationError::new("benchmark", error.to_string()))?;
    let mut materializer = BenchmarkMaterializer { games_per_system };
    let full_plan = plan(None, 1, &system_ids);
    let full_started = Instant::now();
    execute_reconciliation(storage_root, &full_plan, limits, &mut materializer)?;
    let full_us = elapsed_us(full_started);
    let delta_plan = plan(Some(1), 2, &system_ids[..1]);
    let delta_started = Instant::now();
    execute_reconciliation(storage_root, &delta_plan, limits, &mut materializer)?;
    let delta_us = elapsed_us(delta_started);
    Ok(RebuildBenchmarkOutcome {
        full_us,
        delta_us,
        full_systems: systems,
        delta_systems: 1,
        games_per_system,
    })
}

fn plan(current: Option<u64>, intended: u64, systems: &[SystemId]) -> ReconcilePlan {
    ReconcilePlan {
        current_generation: current,
        intended_generation: intended,
        scan_units: Vec::new(),
        systems: systems
            .iter()
            .cloned()
            .map(|system_id| PlannedSystem {
                system_id,
                action: PlannedSystemAction::Rebuild,
                reasons: vec![ReconcileReason::SourceChanged],
            })
            .collect(),
        global_rebuild: current.is_none(),
        manifest_only: false,
    }
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

struct BenchmarkMaterializer {
    games_per_system: usize,
}

impl ReconciliationMaterializer for BenchmarkMaterializer {
    fn materialize(
        &mut self,
        system_id: &SystemId,
        generation: u64,
    ) -> Result<MaterializedSystem, ReconciliationError> {
        let games = (0..self.games_per_system)
            .map(|index| SystemGame {
                stable_key: format!("{index:08}"),
                title: format!("Synthetic Game {index:08}"),
                launch_ref: format!("/games/{}/{index:08}.rom", system_id.as_str()),
            })
            .collect();
        Ok(MaterializedSystem {
            system_id: system_id.clone(),
            display_title: system_id.as_str().to_string(),
            section: "Fixture".to_string(),
            family: "Fixture".to_string(),
            order: u32::try_from(generation)
                .map_err(|_| ReconciliationError::new("benchmark", "generation exceeds u32"))?,
            producers: vec![ScanUnitId::parse(&format!("{}-root", system_id.as_str()))
                .map_err(|error| ReconciliationError::new("benchmark", error.to_string()))?],
            games,
        })
    }

    fn commit_facts(&mut self) -> Result<(), ReconciliationError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_shard::SystemShardLimits;
    use std::fs;

    fn limits() -> RegistryLimits {
        RegistryLimits {
            max_manifest_bytes: 2 * 1024 * 1024,
            max_systems: 128,
            shard: SystemShardLimits {
                max_sqlite_bytes: 64 * 1024 * 1024,
                max_navigation_compressed_bytes: 16 * 1024 * 1024,
                max_navigation_decoded_bytes: 16 * 1024 * 1024,
                max_games: 100_000,
            },
        }
    }

    #[test]
    fn benchmark_compares_all_systems_with_one_system_delta() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-rebuild-bench-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let outcome = run_rebuild_benchmark(&root, 10, 20, limits()).unwrap();
        assert_eq!(outcome.full_systems, 10);
        assert_eq!(outcome.delta_systems, 1);
        assert_eq!(outcome.work_ratio(), 10.0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn benchmark_refuses_nonempty_storage() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-rebuild-bench-nonempty-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("keep"), b"user data").unwrap();
        let error = run_rebuild_benchmark(&root, 10, 1, limits()).unwrap_err();
        assert!(error.to_string().contains("storage must be empty"));
        assert_eq!(fs::read(root.join("keep")).unwrap(), b"user data");
        let _ = fs::remove_dir_all(root);
    }
}
