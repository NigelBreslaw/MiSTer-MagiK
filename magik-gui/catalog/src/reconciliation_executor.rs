// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Transactional execution of exact sharded reconciliation plans.

use crate::catalog_classify::SystemId;
use crate::catalog_domain::ScanUnitId;
use crate::shard_registry::{
    garbage_collect_unreferenced, manifest_slots_present, publish_manifest,
    publish_system_artifacts, read_latest_manifest, CatalogManifest, ManifestSystem,
    RegistryLimits,
};
use crate::sharded_catalog::{PlannedSystemAction, ReconcilePlan};
use crate::system_shard::{write_system_shard, SystemGame, SystemShardData};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterializedSystem {
    pub system_id: SystemId,
    pub display_title: String,
    pub section: String,
    pub family: String,
    pub order: u32,
    pub producers: Vec<ScanUnitId>,
    pub games: Vec<SystemGame>,
}

pub trait ReconciliationMaterializer {
    fn materialize(
        &mut self,
        system_id: &SystemId,
        generation: u64,
    ) -> Result<MaterializedSystem, ReconciliationError>;

    fn refresh_manifest(
        &mut self,
        _systems: &mut Vec<ManifestSystem>,
    ) -> Result<(), ReconciliationError> {
        Ok(())
    }

    /// Persist the input facts represented by the plan. This runs only after
    /// the new manifest is durable, so facts can never get ahead of readers.
    fn commit_facts(&mut self) -> Result<(), ReconciliationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReconciliationOutcome {
    Unchanged {
        generation: Option<u64>,
    },
    Published {
        generation: u64,
        rebuilt: Vec<SystemId>,
        removed: Vec<SystemId>,
    },
}

pub fn execute_reconciliation(
    storage_root: &Path,
    plan: &ReconcilePlan,
    limits: RegistryLimits,
    materializer: &mut impl ReconciliationMaterializer,
) -> Result<ReconciliationOutcome, ReconciliationError> {
    fs::create_dir_all(storage_root)
        .map_err(|error| ReconciliationError::with("storage", error))?;
    let current = match read_latest_manifest(storage_root, limits) {
        Ok(manifest) => Some(manifest),
        Err(_) if manifest_slots_present(storage_root) => {
            return Err(ReconciliationError::new(
                "read",
                "manifest slots exist but none is valid",
            ));
        }
        Err(_) => None,
    };
    let actual_generation = current.as_ref().map(|manifest| manifest.generation);
    if actual_generation != plan.current_generation {
        return Err(ReconciliationError::new(
            "plan",
            "plan generation does not match the active manifest",
        ));
    }
    if plan.is_unchanged() {
        return Ok(ReconciliationOutcome::Unchanged {
            generation: actual_generation,
        });
    }
    let expected_generation = actual_generation
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| ReconciliationError::new("plan", "manifest generation overflow"))?;
    if plan.intended_generation != expected_generation {
        return Err(ReconciliationError::new(
            "plan",
            "plan intended generation is not the next generation",
        ));
    }
    let mut unique_systems = BTreeSet::new();
    if plan
        .systems
        .iter()
        .any(|system| !unique_systems.insert(system.system_id.clone()))
    {
        return Err(ReconciliationError::new(
            "plan",
            "plan contains duplicate system actions",
        ));
    }

    garbage_collect_unreferenced(
        storage_root,
        current.as_ref().unwrap_or(&CatalogManifest {
            generation: 0,
            systems: Vec::new(),
        }),
    )
    .map_err(|error| ReconciliationError::new("garbage-collect", error.to_string()))?;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ReconciliationError::new("stage", "clock predates Unix epoch"))?
        .as_nanos();
    let mut systems = current
        .as_ref()
        .map_or_else(Vec::new, |manifest| manifest.systems.clone());
    let mut rebuilt = Vec::new();
    let mut removed = Vec::new();
    for planned in &plan.systems {
        match planned.action {
            PlannedSystemAction::Remove => {
                systems.retain(|system| system.system_id != planned.system_id);
                removed.push(planned.system_id.clone());
            }
            PlannedSystemAction::Rebuild => {
                let materialized =
                    materializer.materialize(&planned.system_id, expected_generation)?;
                validate_materialized(&planned.system_id, &materialized)?;
                let staging = storage_root.join("staging").join(format!(
                    "reconcile-{}-{expected_generation}-{nonce}-{}",
                    std::process::id(),
                    planned.system_id.as_str()
                ));
                fs::create_dir_all(&staging)
                    .map_err(|error| ReconciliationError::with("stage", error))?;
                let sqlite = staging.join("system.sqlite3");
                let navigation = staging.join("system.nav.lz4b");
                let game_count = materialized.games.len() as u64;
                if let Err(error) = write_system_shard(
                    &sqlite,
                    &navigation,
                    &SystemShardData {
                        system_id: planned.system_id.clone(),
                        generation: expected_generation,
                        games: materialized.games,
                    },
                    limits.shard,
                ) {
                    let _ = fs::remove_dir_all(&staging);
                    return Err(ReconciliationError::new("write", error.to_string()));
                }
                let active = publish_system_artifacts(
                    storage_root,
                    &sqlite,
                    &navigation,
                    &planned.system_id,
                    expected_generation,
                    game_count,
                    limits,
                );
                let active = match active {
                    Ok(active) => active,
                    Err(error) => {
                        let _ = fs::remove_dir_all(&staging);
                        return Err(ReconciliationError::new(
                            "publish-artifact",
                            error.to_string(),
                        ));
                    }
                };
                let previous = systems
                    .iter()
                    .find(|system| system.system_id == planned.system_id)
                    .map(|system| system.active.clone());
                systems.retain(|system| system.system_id != planned.system_id);
                systems.push(ManifestSystem {
                    system_id: materialized.system_id,
                    display_title: materialized.display_title,
                    section: materialized.section,
                    family: materialized.family,
                    order: materialized.order,
                    producers: materialized.producers,
                    active,
                    previous,
                });
                let _ = fs::remove_dir(staging);
                rebuilt.push(planned.system_id.clone());
            }
        }
    }
    materializer.refresh_manifest(&mut systems)?;
    systems.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    publish_manifest(
        storage_root,
        &CatalogManifest {
            generation: expected_generation,
            systems,
        },
        limits,
    )
    .map_err(|error| ReconciliationError::new("publish-manifest", error.to_string()))?;
    materializer.commit_facts()?;
    Ok(ReconciliationOutcome::Published {
        generation: expected_generation,
        rebuilt,
        removed,
    })
}

fn validate_materialized(
    requested: &SystemId,
    materialized: &MaterializedSystem,
) -> Result<(), ReconciliationError> {
    if &materialized.system_id != requested {
        return Err(ReconciliationError::new(
            "materialize",
            "materializer returned the wrong system",
        ));
    }
    if materialized.display_title.is_empty()
        || materialized.section.is_empty()
        || materialized.family.is_empty()
        || materialized.producers.is_empty()
    {
        return Err(ReconciliationError::new(
            "materialize",
            "materialized system metadata is incomplete",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationError {
    stage: &'static str,
    message: String,
}

impl ReconciliationError {
    pub fn new(stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }

    fn with(stage: &'static str, error: impl fmt::Display) -> Self {
        Self::new(stage, error.to_string())
    }

    pub fn stage(&self) -> &'static str {
        self.stage
    }
}

impl fmt::Display for ReconciliationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.stage, self.message)
    }
}

impl Error for ReconciliationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shard_registry::{read_latest_manifest, RegistryLimits};
    use crate::sharded_catalog::{PlannedSystem, ReconcilePlan, ReconcileReason};
    use crate::system_shard::SystemShardLimits;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn exact_delta_rebuild_preserves_unchanged_shard_bytes_and_mtime() {
        let root = temporary_root("exact-delta");
        let mut materializer = FixtureMaterializer::new();
        let first = plan(None, 1, &["c64", "snes"]);
        execute_reconciliation(&root, &first, limits(), &mut materializer).unwrap();
        assert_eq!(materializer.commits, 1);
        let original = read_latest_manifest(&root, limits()).unwrap();
        let c64_before = original
            .systems
            .iter()
            .find(|system| system.system_id.as_str() == "c64")
            .unwrap()
            .active
            .clone();
        let c64_sqlite = root.join(&c64_before.sqlite_path);
        let c64_navigation = root.join(&c64_before.navigation_path);
        let sqlite_bytes = fs::read(&c64_sqlite).unwrap();
        let navigation_bytes = fs::read(&c64_navigation).unwrap();
        let sqlite_mtime = fs::metadata(&c64_sqlite).unwrap().modified().unwrap();
        let navigation_mtime = fs::metadata(&c64_navigation).unwrap().modified().unwrap();

        materializer.calls.clear();
        let unchanged = plan(Some(1), 1, &[]);
        assert_eq!(
            execute_reconciliation(&root, &unchanged, limits(), &mut materializer).unwrap(),
            ReconciliationOutcome::Unchanged {
                generation: Some(1)
            }
        );
        assert!(materializer.calls.is_empty());
        assert_eq!(materializer.commits, 1);

        materializer.games.insert("snes", vec![game("Two")]);
        let delta = plan(Some(1), 2, &["snes"]);
        let outcome = execute_reconciliation(&root, &delta, limits(), &mut materializer).unwrap();
        assert_eq!(
            outcome,
            ReconciliationOutcome::Published {
                generation: 2,
                rebuilt: vec![system("snes")],
                removed: Vec::new(),
            }
        );
        assert_eq!(materializer.calls, vec![system("snes")]);
        assert_eq!(materializer.commits, 2);
        let updated = read_latest_manifest(&root, limits()).unwrap();
        let c64_after = &updated
            .systems
            .iter()
            .find(|system| system.system_id.as_str() == "c64")
            .unwrap()
            .active;
        assert_eq!(c64_after, &c64_before);
        assert_eq!(fs::read(&c64_sqlite).unwrap(), sqlite_bytes);
        assert_eq!(fs::read(&c64_navigation).unwrap(), navigation_bytes);
        assert_eq!(
            fs::metadata(&c64_sqlite).unwrap().modified().unwrap(),
            sqlite_mtime
        );
        assert_eq!(
            fs::metadata(&c64_navigation).unwrap().modified().unwrap(),
            navigation_mtime
        );

        materializer.calls.clear();
        let mut removal = plan(Some(2), 3, &[]);
        removal.systems.push(PlannedSystem {
            system_id: system("c64"),
            action: PlannedSystemAction::Remove,
            reasons: vec![ReconcileReason::RemovedSystem],
        });
        let removed = execute_reconciliation(&root, &removal, limits(), &mut materializer).unwrap();
        assert_eq!(
            removed,
            ReconciliationOutcome::Published {
                generation: 3,
                rebuilt: Vec::new(),
                removed: vec![system("c64")],
            }
        );
        assert!(materializer.calls.is_empty());
        assert_eq!(
            read_latest_manifest(&root, limits())
                .unwrap()
                .systems
                .iter()
                .map(|system| system.system_id.as_str())
                .collect::<Vec<_>>(),
            vec!["snes"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_multi_shard_run_keeps_old_manifest_and_retry_collects_orphans() {
        let root = temporary_root("failure");
        let mut materializer = FixtureMaterializer::new();
        execute_reconciliation(
            &root,
            &plan(None, 1, &["c64", "snes"]),
            limits(),
            &mut materializer,
        )
        .unwrap();
        let before = read_latest_manifest(&root, limits()).unwrap();
        materializer.calls.clear();
        materializer.fail_on = Some(system("snes"));
        let error = execute_reconciliation(
            &root,
            &plan(Some(1), 2, &["c64", "snes"]),
            limits(),
            &mut materializer,
        )
        .unwrap_err();
        assert_eq!(error.stage(), "fixture");
        assert_eq!(read_latest_manifest(&root, limits()).unwrap(), before);
        assert_eq!(materializer.commits, 1);
        assert!(root.join("systems/c64/2.sqlite3").exists());

        materializer.fail_on = None;
        materializer.calls.clear();
        execute_reconciliation(
            &root,
            &plan(Some(1), 2, &["c64", "snes"]),
            limits(),
            &mut materializer,
        )
        .unwrap();
        assert_eq!(read_latest_manifest(&root, limits()).unwrap().generation, 2);
        assert_eq!(materializer.commits, 2);
        fs::remove_dir_all(root).unwrap();
    }

    struct FixtureMaterializer {
        games: BTreeMap<&'static str, Vec<SystemGame>>,
        calls: Vec<SystemId>,
        commits: usize,
        fail_on: Option<SystemId>,
    }

    impl FixtureMaterializer {
        fn new() -> Self {
            Self {
                games: BTreeMap::from([("c64", vec![game("One")]), ("snes", vec![game("One")])]),
                calls: Vec::new(),
                commits: 0,
                fail_on: None,
            }
        }
    }

    impl ReconciliationMaterializer for FixtureMaterializer {
        fn materialize(
            &mut self,
            system_id: &SystemId,
            _generation: u64,
        ) -> Result<MaterializedSystem, ReconciliationError> {
            self.calls.push(system_id.clone());
            if self.fail_on.as_ref() == Some(system_id) {
                return Err(ReconciliationError::new("fixture", "injected failure"));
            }
            Ok(MaterializedSystem {
                system_id: system_id.clone(),
                display_title: system_id.as_str().to_ascii_uppercase(),
                section: "Fixture".to_string(),
                family: "Fixture".to_string(),
                order: 0,
                producers: vec![ScanUnitId::parse(&format!("{}-root", system_id.as_str())).unwrap()],
                games: self.games.get(system_id.as_str()).unwrap().clone(),
            })
        }

        fn commit_facts(&mut self) -> Result<(), ReconciliationError> {
            self.commits += 1;
            Ok(())
        }
    }

    fn plan(current: Option<u64>, intended: u64, rebuild: &[&str]) -> ReconcilePlan {
        ReconcilePlan {
            current_generation: current,
            intended_generation: intended,
            scan_units: Vec::new(),
            systems: rebuild
                .iter()
                .map(|id| PlannedSystem {
                    system_id: system(id),
                    action: PlannedSystemAction::Rebuild,
                    reasons: vec![ReconcileReason::SourceChanged],
                })
                .collect(),
            global_rebuild: false,
            manifest_only: false,
        }
    }

    fn system(value: &str) -> SystemId {
        SystemId::parse(value).unwrap()
    }

    fn game(title: &str) -> SystemGame {
        SystemGame {
            stable_key: title.to_ascii_lowercase(),
            title: title.to_string(),
            launch_ref: format!("/fixture/{title}"),
        }
    }

    fn limits() -> RegistryLimits {
        RegistryLimits {
            max_manifest_bytes: 1024 * 1024,
            max_systems: 100,
            shard: SystemShardLimits {
                max_sqlite_bytes: 4 * 1024 * 1024,
                max_navigation_compressed_bytes: 1024 * 1024,
                max_navigation_decoded_bytes: 1024 * 1024,
                max_games: 10_000,
            },
        }
    }

    fn temporary_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mister-magik-reconciliation-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
}
