// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production dual-publication bridge from the canonical RAM catalog to V3.

use crate::arcade_catalog::ArcadeCatalog;
use crate::catalog_classify::{system_definition, LauncherSection, SystemId};
use crate::catalog_domain::ScanUnitId;
use crate::reconciliation_executor::{
    execute_reconciliation, MaterializedSystem, ReconciliationError, ReconciliationMaterializer,
    ReconciliationOutcome,
};
use crate::shard_registry::{read_latest_manifest, RegistryLimits};
use crate::sharded_catalog::{PlannedSystem, PlannedSystemAction, ReconcilePlan, ReconcileReason};
use crate::system_shard::{SystemGame, SystemLaunchPlan, SystemShardLimits};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::UNIX_EPOCH;

const BINDING_SCHEMA_VERSION: u32 = 1;
const PROJECTION_CONTRACT: &str = "rich-game-v1";
const BINDING_FILE: &str = "catalog.binding.json";
const MAX_BINDING_BYTES: u64 = 4096;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct CatalogBinding {
    schema_version: u32,
    projection_contract: String,
    manifest_generation: u64,
    sqlite_len: u64,
    sqlite_modified_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductionProjectionOutcome {
    pub generation: u64,
    pub systems: usize,
    pub games: usize,
}

pub fn production_registry_limits() -> RegistryLimits {
    RegistryLimits {
        max_manifest_bytes: 8 * 1024 * 1024,
        max_systems: 4096,
        shard: SystemShardLimits {
            max_sqlite_bytes: 8 * 1024 * 1024 * 1024,
            max_navigation_compressed_bytes: 512 * 1024 * 1024,
            max_navigation_decoded_bytes: 512 * 1024 * 1024,
            max_games: 2_000_000,
        },
    }
}

pub fn publish_production_projection(
    storage_root: &Path,
    catalog: &ArcadeCatalog,
    limits: RegistryLimits,
) -> Result<ProductionProjectionOutcome, ReconciliationError> {
    let current_manifest = read_latest_manifest(storage_root, limits).ok();
    let current_generation = current_manifest
        .as_ref()
        .map(|manifest| manifest.generation);
    let intended_generation = current_generation
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| ReconciliationError::new("projection", "generation overflow"))?;
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
    let mut planned_systems = systems
        .iter()
        .cloned()
        .map(|system_id| PlannedSystem {
            system_id,
            action: PlannedSystemAction::Rebuild,
            reasons: vec![ReconcileReason::SemanticVersionChanged],
        })
        .collect::<Vec<_>>();
    planned_systems.extend(removed_systems.into_iter().map(|system_id| PlannedSystem {
        system_id,
        action: PlannedSystemAction::Remove,
        reasons: vec![ReconcileReason::RemovedSystem],
    }));
    let plan = ReconcilePlan {
        current_generation,
        intended_generation,
        scan_units: Vec::new(),
        systems: planned_systems,
        global_rebuild: true,
        manifest_only: false,
    };
    let mut materializer = CatalogMaterializer::new(catalog);
    match execute_reconciliation(storage_root, &plan, limits, &mut materializer)? {
        ReconciliationOutcome::Published { generation, .. } => Ok(ProductionProjectionOutcome {
            generation,
            systems: systems.len(),
            games: materializer.games,
        }),
        ReconciliationOutcome::Unchanged { .. } => Err(ReconciliationError::new(
            "projection",
            "full projection unexpectedly produced no publication",
        )),
    }
}

pub fn publish_bound_production_projection(
    storage_root: &Path,
    catalog: &ArcadeCatalog,
    sqlite_path: &Path,
    limits: RegistryLimits,
) -> Result<ProductionProjectionOutcome, ReconciliationError> {
    let sqlite = sqlite_identity(sqlite_path)?;
    let outcome = publish_production_projection(storage_root, catalog, limits)?;
    write_binding(
        storage_root,
        &CatalogBinding {
            schema_version: BINDING_SCHEMA_VERSION,
            projection_contract: PROJECTION_CONTRACT.to_string(),
            manifest_generation: outcome.generation,
            sqlite_len: sqlite.0,
            sqlite_modified_ns: sqlite.1,
        },
    )?;
    Ok(outcome)
}

pub fn validate_production_binding(
    storage_root: &Path,
    sqlite_path: &Path,
    manifest_generation: u64,
) -> Result<(), ReconciliationError> {
    let path = storage_root.join(BINDING_FILE);
    let metadata = fs::metadata(&path)
        .map_err(|error| ReconciliationError::new("binding", error.to_string()))?;
    if metadata.len() > MAX_BINDING_BYTES {
        return Err(ReconciliationError::new(
            "binding",
            "catalog binding exceeds size limit",
        ));
    }
    let binding: CatalogBinding = serde_json::from_slice(
        &fs::read(&path).map_err(|error| ReconciliationError::new("binding", error.to_string()))?,
    )
    .map_err(|error| ReconciliationError::new("binding", error.to_string()))?;
    let sqlite = sqlite_identity(sqlite_path)?;
    if binding.schema_version != BINDING_SCHEMA_VERSION
        || binding.projection_contract != PROJECTION_CONTRACT
        || binding.manifest_generation != manifest_generation
        || (binding.sqlite_len, binding.sqlite_modified_ns) != sqlite
    {
        return Err(ReconciliationError::new(
            "binding",
            "catalog binding does not match the active manifest and SQLite",
        ));
    }
    Ok(())
}

fn sqlite_identity(path: &Path) -> Result<(u64, u64), ReconciliationError> {
    let metadata = fs::metadata(path)
        .map_err(|error| ReconciliationError::new("binding", error.to_string()))?;
    let modified_ns = metadata
        .modified()
        .map_err(|error| ReconciliationError::new("binding", error.to_string()))?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ReconciliationError::new("binding", "SQLite mtime predates Unix epoch"))?
        .as_nanos();
    Ok((
        metadata.len(),
        u64::try_from(modified_ns)
            .map_err(|_| ReconciliationError::new("binding", "SQLite mtime exceeds u64"))?,
    ))
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
    games: usize,
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
        Self {
            catalog,
            titles,
            games: 0,
        }
    }
}

impl ReconciliationMaterializer for CatalogMaterializer<'_> {
    fn materialize(
        &mut self,
        system_id: &SystemId,
        _generation: u64,
    ) -> Result<MaterializedSystem, ReconciliationError> {
        let definition = system_definition(system_id.as_str());
        let games = self
            .catalog
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
                launch_plan: self
                    .catalog
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
        self.games = self.games.saturating_add(games.len());
        Ok(MaterializedSystem {
            system_id: system_id.clone(),
            display_title: self
                .titles
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

    fn commit_facts(&mut self) -> Result<(), ReconciliationError> {
        Ok(())
    }
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
    fn binding_rejects_replaced_or_missing_v2_database() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-production-projection-binding-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let sqlite = root.join("library.sqlite3");
        fs::write(&sqlite, b"v2 generation one").unwrap();
        let storage = root.join("catalog-v3");
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
            publish_bound_production_projection(&storage, &catalog, &sqlite, limits()).unwrap();
        validate_production_binding(&storage, &sqlite, outcome.generation).unwrap();
        fs::write(&sqlite, b"different v2 generation with another size").unwrap();
        assert!(validate_production_binding(&storage, &sqlite, outcome.generation).is_err());
        fs::remove_file(&sqlite).unwrap();
        assert!(validate_production_binding(&storage, &sqlite, outcome.generation).is_err());
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
