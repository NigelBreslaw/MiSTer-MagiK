// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production V3 projection from the canonical RAM catalog into system shards.

use crate::arcade_catalog::ArcadeCatalog;
use crate::catalog_classify::{system_definition, LauncherSection, SystemId};
use crate::catalog_domain::ScanUnitId;
use crate::reconciliation_executor::{
    execute_reconciliation, MaterializedSystem, ReconciliationError, ReconciliationMaterializer,
    ReconciliationOutcome,
};
use crate::shard_registry::{
    manifest_slots_present, read_latest_manifest_lazy, ManifestSystem, RegistryLimits,
};
use crate::sharded_catalog::{PlannedSystem, PlannedSystemAction, ReconcilePlan, ReconcileReason};
use crate::system_shard::{open_system_shard, SystemGame, SystemLaunchPlan};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

const BINDING_SCHEMA_VERSION: u32 = 1;
const PROJECTION_CONTRACT: &str = "rich-game-v1";
const BINDING_FILE: &str = "catalog.binding.json";
const MAX_BINDING_BYTES: u64 = 4096;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct CatalogBinding {
    schema_version: u32,
    projection_contract: String,
    manifest_generation: u64,
    catalog_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductionProjectionOutcome {
    pub generation: u64,
    pub systems: usize,
    pub games: usize,
    pub rebuilt_systems: usize,
    pub removed_systems: usize,
}

pub fn production_registry_limits() -> RegistryLimits {
    crate::shard_registry::production_registry_limits()
}

pub fn publish_production_projection(
    storage_root: &Path,
    catalog: &ArcadeCatalog,
    limits: RegistryLimits,
) -> Result<ProductionProjectionOutcome, ReconciliationError> {
    let projection_started = Instant::now();
    let current_manifest = match read_latest_manifest_lazy(storage_root, limits) {
        Ok(manifest) => Some(manifest),
        Err(error) if manifest_slots_present(storage_root) => {
            return Err(ReconciliationError::new("projection", error.to_string()));
        }
        Err(_) => None,
    };
    let current_generation = current_manifest
        .as_ref()
        .map(|manifest| manifest.generation);
    let systems = catalog
        .systems
        .iter()
        .map(|system| {
            SystemId::parse(&system.id)
                .map_err(|error| ReconciliationError::new("projection", error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if systems.is_empty() && !catalog.games.is_empty() {
        return Err(ReconciliationError::new(
            "projection",
            "non-empty catalog has no shardable systems",
        ));
    }
    let new_systems = systems.iter().cloned().collect::<BTreeSet<_>>();
    if new_systems.len() != systems.len() {
        return Err(ReconciliationError::new(
            "projection",
            "canonical catalog contains duplicate system IDs",
        ));
    }
    let removed_systems = current_manifest
        .as_ref()
        .map(|manifest| {
            manifest
                .systems
                .iter()
                .map(|system| system.system_id.clone())
                .filter(|system_id| !new_systems.contains(system_id))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut materializer = CatalogMaterializer::new(catalog);
    let mut planned_systems = Vec::new();
    let planning_started = Instant::now();
    for system_id in &systems {
        let published = current_manifest.as_ref().and_then(|manifest| {
            manifest
                .systems
                .iter()
                .find(|system| system.system_id == *system_id)
        });
        let unchanged = match published {
            Some(published) => {
                let candidate = materializer.project(system_id)?;
                published_system_matches(storage_root, published, &candidate, limits)?
            }
            None => false,
        };
        if unchanged {
            continue;
        }
        planned_systems.push(PlannedSystem {
            system_id: system_id.clone(),
            action: PlannedSystemAction::Rebuild,
            reasons: vec![ReconcileReason::SourceChanged],
        });
    }
    planned_systems.extend(removed_systems.into_iter().map(|system_id| PlannedSystem {
        system_id,
        action: PlannedSystemAction::Remove,
        reasons: vec![ReconcileReason::RemovedSystem],
    }));
    let changed = !planned_systems.is_empty();
    let planning_us = planning_started.elapsed().as_micros();
    let intended_generation = if changed {
        current_generation
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| ReconciliationError::new("projection", "generation overflow"))?
    } else {
        current_generation.unwrap_or(0)
    };
    let plan = ReconcilePlan {
        current_generation,
        intended_generation,
        scan_units: Vec::new(),
        systems: planned_systems,
        global_rebuild: false,
        manifest_only: false,
    };
    let reconciliation_started = Instant::now();
    let result = execute_reconciliation(storage_root, &plan, limits, &mut materializer)?;
    crate::catalog_logln!(
        "catalog_v3_projection_phases_tsv\tplanning_us={}\treconciliation_us={}\ttotal_us={}",
        planning_us,
        reconciliation_started.elapsed().as_micros(),
        projection_started.elapsed().as_micros(),
    );
    match result {
        ReconciliationOutcome::Published {
            generation,
            rebuilt,
            removed,
        } => Ok(ProductionProjectionOutcome {
            generation,
            systems: systems.len(),
            games: catalog.len(),
            rebuilt_systems: rebuilt.len(),
            removed_systems: removed.len(),
        }),
        ReconciliationOutcome::Unchanged { generation } => Ok(ProductionProjectionOutcome {
            generation: generation.unwrap_or(0),
            systems: systems.len(),
            games: catalog.len(),
            rebuilt_systems: 0,
            removed_systems: 0,
        }),
    }
}

fn published_system_matches(
    storage_root: &Path,
    published: &ManifestSystem,
    candidate: &MaterializedSystem,
    limits: RegistryLimits,
) -> Result<bool, ReconciliationError> {
    if published.display_title != candidate.display_title
        || published.section != candidate.section
        || published.family != candidate.family
        || published.order != candidate.order
        || published.producers != candidate.producers
    {
        return Ok(false);
    }
    let loaded = open_system_shard(
        &storage_root.join(&published.active.sqlite_path),
        &storage_root.join(&published.active.navigation_path),
        &published.system_id,
        published.active.generation,
        limits.shard,
    )
    .map_err(|error| ReconciliationError::new("projection-compare", error.to_string()))?;
    Ok(loaded.games == candidate.games)
}

pub fn publish_bound_production_projection(
    storage_root: &Path,
    catalog: &ArcadeCatalog,
    catalog_fingerprint: &str,
    limits: RegistryLimits,
) -> Result<ProductionProjectionOutcome, ReconciliationError> {
    let outcome = publish_production_projection(storage_root, catalog, limits)?;
    write_binding(
        storage_root,
        &CatalogBinding {
            schema_version: BINDING_SCHEMA_VERSION,
            projection_contract: PROJECTION_CONTRACT.to_string(),
            manifest_generation: outcome.generation,
            catalog_fingerprint: catalog_fingerprint.to_string(),
        },
    )?;
    Ok(outcome)
}

pub fn validate_production_binding(
    storage_root: &Path,
    manifest_generation: u64,
) -> Result<String, ReconciliationError> {
    let binding = read_binding(storage_root)?;
    let state = crate::catalog_state::read(&crate::catalog_state::path_for_root(storage_root))
        .map_err(|error| ReconciliationError::new("binding", error))?;
    let state_fingerprint = state.stamp.fingerprint_hex();
    if binding.schema_version != BINDING_SCHEMA_VERSION
        || binding.projection_contract != PROJECTION_CONTRACT
        || binding.manifest_generation != manifest_generation
        || binding.catalog_fingerprint != state_fingerprint
    {
        return Err(ReconciliationError::new(
            "binding",
            "catalog binding does not match the active manifest and V3 state",
        ));
    }
    Ok(state_fingerprint)
}

fn read_binding(storage_root: &Path) -> Result<CatalogBinding, ReconciliationError> {
    let path = storage_root.join(BINDING_FILE);
    let metadata = fs::metadata(&path)
        .map_err(|error| ReconciliationError::new("binding", error.to_string()))?;
    if metadata.len() > MAX_BINDING_BYTES {
        return Err(ReconciliationError::new(
            "binding",
            "catalog binding exceeds size limit",
        ));
    }
    serde_json::from_slice(
        &fs::read(&path).map_err(|error| ReconciliationError::new("binding", error.to_string()))?,
    )
    .map_err(|error| ReconciliationError::new("binding", error.to_string()))
}

fn write_binding(storage_root: &Path, binding: &CatalogBinding) -> Result<(), ReconciliationError> {
    fs::create_dir_all(storage_root)
        .map_err(|error| ReconciliationError::new("binding", error.to_string()))?;
    let encoded = serde_json::to_vec(binding)
        .map_err(|error| ReconciliationError::new("binding", error.to_string()))?;
    let temporary = storage_root.join(format!(".{BINDING_FILE}.tmp.{}", std::process::id()));
    let final_path = storage_root.join(BINDING_FILE);
    let result = (|| {
        let mut file = fs::File::create(&temporary)
            .map_err(|error| ReconciliationError::new("binding", error.to_string()))?;
        file.write_all(&encoded)
            .and_then(|()| file.sync_all())
            .map_err(|error| ReconciliationError::new("binding", error.to_string()))?;
        fs::rename(&temporary, &final_path)
            .map_err(|error| ReconciliationError::new("binding", error.to_string()))?;
        crate::sqlite_catalog::sync_parent_dir(&final_path);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

struct CatalogMaterializer<'a> {
    catalog: &'a ArcadeCatalog,
    titles: BTreeMap<SystemId, String>,
}

impl<'a> CatalogMaterializer<'a> {
    fn new(catalog: &'a ArcadeCatalog) -> Self {
        let titles = catalog
            .systems
            .iter()
            .filter_map(|system| {
                SystemId::parse(&system.id)
                    .ok()
                    .map(|system_id| (system_id, system.title.clone()))
            })
            .collect();
        Self { catalog, titles }
    }

    fn project(&self, system_id: &SystemId) -> Result<MaterializedSystem, ReconciliationError> {
        materialize_catalog_system(self.catalog, &self.titles, system_id)
    }
}

impl ReconciliationMaterializer for CatalogMaterializer<'_> {
    fn materialize(
        &mut self,
        system_id: &SystemId,
        _generation: u64,
    ) -> Result<MaterializedSystem, ReconciliationError> {
        self.project(system_id)
    }

    fn commit_facts(&mut self) -> Result<(), ReconciliationError> {
        Ok(())
    }
}

fn materialize_catalog_system(
    catalog: &ArcadeCatalog,
    titles: &BTreeMap<SystemId, String>,
    system_id: &SystemId,
) -> Result<MaterializedSystem, ReconciliationError> {
    let definition = system_definition(system_id.as_str());
    let games = catalog
        .system_game_view(system_id.as_str())
        .iter()
        .map(|game| SystemGame {
            stable_key: format!("{}\u{1f}{}\u{1f}{}", system_id, game.title, game.mra_path),
            title: game.title.to_string(),
            launch_ref: game.mra_path.to_string(),
            preview_archive_path: game.preview_archive_path.to_string(),
            preview_asset_key: game.preview_asset_key.to_string(),
            has_preview: game.has_preview,
            year: game.year,
            manufacturer: game.manufacturer.to_string(),
            players: game.players,
            control: game.control.to_string(),
            is_new: game.is_new,
            launch_plan: catalog
                .structured_launch_plan_for_ref(&game.mra_path)
                .map(|plan| SystemLaunchPlan {
                    launch_ref: plan.launch_ref.to_string(),
                    title: plan.title.to_string(),
                    system_id: plan.system_id.to_string(),
                    core_path: plan.core_path.to_string(),
                    payload_path: plan.payload_path.to_string(),
                    mount_kind: plan.mount_kind.to_string(),
                    mount_index: plan.mount_index,
                    delay_secs: plan.delay_secs,
                }),
        })
        .collect::<Vec<_>>();
    Ok(MaterializedSystem {
        system_id: system_id.clone(),
        display_title: titles
            .get(system_id)
            .cloned()
            .unwrap_or_else(|| system_id.as_str().to_string()),
        section: definition
            .map(|value| section_label(value.section))
            .unwrap_or("Other")
            .to_string(),
        family: definition
            .map(|value| value.family.as_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("Other")
            .to_string(),
        order: definition.map_or(1000, |value| u32::from(value.order)),
        producers: vec![
            ScanUnitId::parse(&format!("{}-catalog", system_id.as_str()))
                .map_err(|error| ReconciliationError::new("projection", error.to_string()))?,
        ],
        games,
    })
}

fn section_label(section: LauncherSection) -> &'static str {
    match section {
        LauncherSection::Arcade => "Arcade",
        LauncherSection::SnkNeogeo => "SNK Neo Geo",
        LauncherSection::Consoles => "Consoles",
        LauncherSection::Handhelds => "Handhelds",
        LauncherSection::Computers => "Computers",
        LauncherSection::Other => "Other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arcade_catalog::{ArcadeGameEntry, GameSystemEntry};
    use crate::lazy_sharded_reader::LazyShardedCatalogReader;
    use crate::shard_registry::read_latest_manifest;
    use crate::sharded_catalog::CatalogReader;
    use crate::system_shard::SystemShardLimits;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn canonical_ram_catalog_dual_publishes_and_opens_by_system() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-production-projection-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/fixture"),
            vec![game("Super Game", "/games/SNES/Super Game.sfc", "snes")],
            vec![GameSystemEntry {
                id: "snes".to_string(),
                title: "SNES".to_string(),
                count: 1,
            }],
        );
        let outcome = publish_production_projection(&root, &catalog, limits()).unwrap();
        assert_eq!(outcome.systems, 1);
        assert_eq!(outcome.games, 1);
        assert_eq!(outcome.rebuilt_systems, 1);
        assert_eq!(outcome.removed_systems, 0);
        let reader = LazyShardedCatalogReader::open(&root, limits()).unwrap();
        let registry = reader.open_registry().unwrap();
        assert_eq!(registry.systems()[0].display_title, "SNES");
        assert_eq!(
            reader
                .open_system(&SystemId::parse("snes").unwrap())
                .unwrap()
                .games()[0]
                .title,
            "Super Game"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unchanged_projection_reuses_all_system_artifacts_and_generation() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-production-projection-unchanged-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let catalog = two_system_catalog("Super Game");
        let initial = publish_production_projection(&root, &catalog, limits()).unwrap();
        let before = read_latest_manifest(&root, limits()).unwrap();
        let outcome = publish_production_projection(&root, &catalog, limits()).unwrap();
        let after = read_latest_manifest(&root, limits()).unwrap();

        assert_eq!(initial.generation, 1);
        assert_eq!(outcome.generation, 1);
        assert_eq!(outcome.rebuilt_systems, 0);
        assert_eq!(outcome.removed_systems, 0);
        assert_eq!(after, before);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn changed_projection_rebuilds_only_the_changed_system() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-production-projection-delta-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        publish_production_projection(&root, &two_system_catalog("Super Game"), limits()).unwrap();
        let before = read_latest_manifest(&root, limits()).unwrap();
        let c64_before = before
            .systems
            .iter()
            .find(|system| system.system_id.as_str() == "c64")
            .unwrap()
            .active
            .clone();

        let outcome = publish_production_projection(
            &root,
            &two_system_catalog("Changed Super Game"),
            limits(),
        )
        .unwrap();
        let after = read_latest_manifest(&root, limits()).unwrap();
        let c64_after = &after
            .systems
            .iter()
            .find(|system| system.system_id.as_str() == "c64")
            .unwrap()
            .active;
        let snes_after = &after
            .systems
            .iter()
            .find(|system| system.system_id.as_str() == "snes")
            .unwrap()
            .active;

        assert_eq!(outcome.generation, 2);
        assert_eq!(outcome.rebuilt_systems, 1);
        assert_eq!(outcome.removed_systems, 0);
        assert_eq!(c64_after, &c64_before);
        assert_eq!(snes_after.generation, 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn later_projection_removes_systems_absent_from_canonical_catalog() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-production-projection-remove-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let initial = ArcadeCatalog::new(
            PathBuf::from("/fixture"),
            vec![
                game("Super Game", "/games/SNES/Super.sfc", "snes"),
                game("C64 Game", "/games/C64/Game.d64", "c64"),
            ],
            vec![
                GameSystemEntry {
                    id: "snes".to_string(),
                    title: "SNES".to_string(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "c64".to_string(),
                    title: "Commodore 64".to_string(),
                    count: 1,
                },
            ],
        );
        publish_production_projection(&root, &initial, limits()).unwrap();
        let reduced = ArcadeCatalog::new(
            PathBuf::from("/fixture"),
            vec![game("Super Game", "/games/SNES/Super.sfc", "snes")],
            vec![GameSystemEntry {
                id: "snes".to_string(),
                title: "SNES".to_string(),
                count: 1,
            }],
        );
        publish_production_projection(&root, &reduced, limits()).unwrap();
        let reader = LazyShardedCatalogReader::open(&root, limits()).unwrap();
        let registry = reader.open_registry().unwrap();
        assert_eq!(registry.systems().len(), 1);
        assert_eq!(registry.systems()[0].system_id.as_str(), "snes");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn binding_is_owned_by_v3_state_without_a_v2_database() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-production-projection-binding-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let storage = root.join("catalog-v3");
        let state = crate::catalog_state::CatalogState {
            stamp: crate::catalog_stamp::CatalogStamp::from_lines(vec!["fixture".to_string()]),
            checkpoint: crate::catalog_checkpoint::CatalogDiscoveryCheckpoint::from_lines(vec![
                "fixture".to_string(),
            ]),
            stats: crate::catalog_state::CatalogStateStats {
                normal_files: 1,
                discoveries: 1,
                ..crate::catalog_state::CatalogStateStats::default()
            },
        };
        let fingerprint = state.stamp.fingerprint_hex();
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/fixture"),
            vec![game("Super Game", "/games/SNES/Super.sfc", "snes")],
            vec![GameSystemEntry {
                id: "snes".to_string(),
                title: "SNES".to_string(),
                count: 1,
            }],
        );
        let outcome =
            publish_bound_production_projection(&storage, &catalog, &fingerprint, limits())
                .unwrap();
        assert!(validate_production_binding(&storage, outcome.generation).is_err());
        crate::catalog_state::write(&crate::catalog_state::path_for_root(&storage), &state)
            .unwrap();
        validate_production_binding(&storage, outcome.generation).unwrap();
        let summary = crate::library_db::sharded_cached_summary(&storage, 123).unwrap();
        assert!(summary.skipped);
        assert_eq!(summary.scan_us, 123);
        assert_eq!(summary.normal_files, 1);
        assert_eq!(summary.discoveries, 1);
        assert!(summary.bytes > 0);
        let different_state = crate::catalog_state::CatalogState {
            stamp: crate::catalog_stamp::CatalogStamp::from_lines(vec!["different".to_string()]),
            checkpoint: state.checkpoint,
            stats: state.stats,
        };
        crate::catalog_state::write(
            &crate::catalog_state::path_for_root(&storage),
            &different_state,
        )
        .unwrap();
        assert!(validate_production_binding(&storage, outcome.generation).is_err());
        assert!(!root.join("library.sqlite3").exists());
        let _ = fs::remove_dir_all(root);
    }

    fn game(title: &str, path: &str, system_id: &str) -> ArcadeGameEntry {
        ArcadeGameEntry {
            title: title.into(),
            mra_path: path.into(),
            preview_archive_path: "".into(),
            preview_asset_key: "".into(),
            has_preview: false,
            system_id: system_id.into(),
            year: None,
            manufacturer: "".into(),
            players: None,
            control: "".into(),
            is_new: false,
        }
    }

    fn two_system_catalog(snes_title: &str) -> ArcadeCatalog {
        ArcadeCatalog::new(
            PathBuf::from("/fixture"),
            vec![
                game(snes_title, "/games/SNES/Super.sfc", "snes"),
                game("C64 Game", "/games/C64/Game.d64", "c64"),
            ],
            vec![
                GameSystemEntry {
                    id: "snes".to_string(),
                    title: "SNES".to_string(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "c64".to_string(),
                    title: "Commodore 64".to_string(),
                    count: 1,
                },
            ],
        )
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
}
