// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production V3 projection from the canonical RAM catalog into system shards.

use crate::arcade_catalog::ArcadeCatalog;
use crate::catalog_classify::{LauncherSection, SystemId, system_definition};
use crate::catalog_domain::ScanUnitId;
use crate::reconciliation_executor::{
    MaterializedSystem, ReconciliationError, ReconciliationEvent, ReconciliationMaterializer,
    ReconciliationOutcome, execute_reconciliation, execute_reconciliation_with_events,
};
use crate::shard_registry::{
    ManifestSystem, RegistryLimits, manifest_slots_present, read_latest_manifest,
    read_latest_manifest_lazy,
};
use crate::sharded_catalog::{PlannedSystem, PlannedSystemAction, ReconcilePlan, ReconcileReason};
use crate::system_shard::{
    SystemGame, SystemLaunchPlan, SystemShardProjectionStats, open_system_shard,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use crate::catalog_format::{BINDING_SCHEMA_VERSION, CatalogFormatDescriptor, CatalogFormatStatus};
pub use crate::sharded_catalog::PRODUCTION_PROJECTION_CONTRACT;
const BINDING_FILE: &str = "catalog.binding.json";
const MAX_BINDING_BYTES: u64 = 4096;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct CatalogBinding {
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    format: Option<CatalogFormatDescriptor>,
    projection_contract: String,
    manifest_generation: u64,
    catalog_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProductionBindingStatus {
    Current {
        fingerprint: String,
    },
    UpgradeRequired {
        fingerprint: String,
        installed: String,
        required: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductionProjectionOutcome {
    pub generation: u64,
    pub systems: usize,
    pub games: usize,
    pub rebuilt_systems: usize,
    pub removed_systems: usize,
    pub rebuilt: Vec<SystemId>,
    pub removed: Vec<SystemId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewAvailabilityReconciliationOutcome {
    pub system_id: SystemId,
    pub previous_generation: u64,
    pub generation: u64,
    pub candidate_rows: usize,
    pub available_rows: usize,
    pub changed_rows: usize,
    pub games: Vec<SystemGame>,
}

pub fn reconcile_production_preview_availability(
    storage_root: &Path,
    system_id: &SystemId,
    pack_path: &Path,
    limits: RegistryLimits,
) -> Result<PreviewAvailabilityReconciliationOutcome, ReconciliationError> {
    let manifest = read_latest_manifest(storage_root, limits)
        .map_err(|error| ReconciliationError::new("preview-availability", error.to_string()))?;
    let fingerprint = validate_production_binding(storage_root, manifest.generation)?;
    let published = manifest
        .systems
        .iter()
        .find(|system| &system.system_id == system_id)
        .ok_or_else(|| ReconciliationError::new("preview-availability", "system is absent"))?;
    let loaded = open_system_shard(
        &storage_root.join(&published.active.sqlite_path),
        &storage_root.join(&published.active.navigation_path),
        system_id,
        published.active.generation,
        limits.shard,
    )
    .map_err(|error| ReconciliationError::new("preview-availability", error.to_string()))?;
    let (games, candidate_rows, available_rows, changed_rows) =
        reconcile_preview_rows(system_id, pack_path, loaded.games)?;
    if changed_rows == 0 {
        return Ok(PreviewAvailabilityReconciliationOutcome {
            system_id: system_id.clone(),
            previous_generation: manifest.generation,
            generation: manifest.generation,
            candidate_rows,
            available_rows,
            changed_rows,
            games,
        });
    }
    let materialized = MaterializedSystem {
        system_id: system_id.clone(),
        display_title: published.display_title.clone(),
        section: published.section.clone(),
        family: published.family.clone(),
        order: published.order,
        producers: published.producers.clone(),
        projection_stats: loaded.projection_stats,
        games: games.clone(),
    };
    let next_generation = manifest
        .generation
        .checked_add(1)
        .ok_or_else(|| ReconciliationError::new("preview-availability", "generation overflow"))?;
    let plan = ReconcilePlan {
        current_generation: Some(manifest.generation),
        intended_generation: next_generation,
        scan_units: Vec::new(),
        systems: vec![PlannedSystem {
            system_id: system_id.clone(),
            action: PlannedSystemAction::Rebuild,
            reasons: vec![ReconcileReason::MetadataChanged],
        }],
        global_rebuild: false,
        manifest_only: false,
    };
    let mut materializer = PreviewAvailabilityMaterializer(Some(materialized));
    execute_reconciliation(storage_root, &plan, limits, &mut materializer)?;
    write_binding(
        storage_root,
        &CatalogBinding {
            schema_version: BINDING_SCHEMA_VERSION,
            format: Some(CatalogFormatDescriptor::current()),
            projection_contract: PRODUCTION_PROJECTION_CONTRACT.to_string(),
            manifest_generation: next_generation,
            catalog_fingerprint: fingerprint,
        },
    )?;
    Ok(PreviewAvailabilityReconciliationOutcome {
        system_id: system_id.clone(),
        previous_generation: manifest.generation,
        generation: next_generation,
        candidate_rows,
        available_rows,
        changed_rows,
        games,
    })
}

/// Reconcile a downloaded pack against an independent fast-catalog shard.
///
/// Fast catalogs deliberately have no production binding. Availability is
/// applied to the launcher's in-memory rows and never republishes catalog
/// artifacts merely because a media pack changed.
pub fn reconcile_fast_preview_availability(
    storage_root: &Path,
    system_id: &SystemId,
    pack_path: &Path,
    limits: RegistryLimits,
) -> Result<PreviewAvailabilityReconciliationOutcome, ReconciliationError> {
    let manifest = read_latest_manifest(storage_root, limits)
        .map_err(|error| ReconciliationError::new("preview-availability", error.to_string()))?;
    let published = manifest
        .systems
        .iter()
        .find(|system| &system.system_id == system_id)
        .ok_or_else(|| ReconciliationError::new("preview-availability", "system is absent"))?;
    let games = open_fast_navpack_games(storage_root, published)?;
    let (games, candidate_rows, available_rows, changed_rows) =
        reconcile_preview_rows(system_id, pack_path, games)?;
    Ok(PreviewAvailabilityReconciliationOutcome {
        system_id: system_id.clone(),
        previous_generation: manifest.generation,
        generation: manifest.generation,
        candidate_rows,
        available_rows,
        changed_rows,
        games,
    })
}

fn open_fast_navpack_games(
    storage_root: &Path,
    published: &ManifestSystem,
) -> Result<Vec<SystemGame>, ReconciliationError> {
    let generation = &published.active;
    let descriptor = generation.navpack.as_ref().ok_or_else(|| {
        ReconciliationError::new("preview-availability", "active system has no NavPack")
    })?;
    let game_count = usize::try_from(generation.games).map_err(|_| {
        ReconciliationError::new(
            "preview-availability",
            "system game count exceeds platform size",
        )
    })?;
    let (navpack, _) = crate::navpack::MappedNavPack::open(
        &storage_root.join(&descriptor.path),
        descriptor.bytes,
        published.system_id.as_str(),
        generation.generation,
        game_count,
    )
    .map_err(|error| ReconciliationError::new("preview-availability", error))?;
    let mut games = Vec::with_capacity(game_count);
    for ordinal in 0..game_count {
        let row = navpack
            .row(ordinal)
            .map_err(|error| ReconciliationError::new("preview-availability", error))?;
        let metadata = navpack
            .metadata(ordinal)
            .map_err(|error| ReconciliationError::new("preview-availability", error))?;
        let launch_plan = row
            .launch_index
            .map(|index| {
                navpack.launch(index).map(|launch| SystemLaunchPlan {
                    launch_ref: launch.launch_ref.to_string(),
                    title: launch.title.to_string(),
                    system_id: launch.system_id.to_string(),
                    core_path: launch.core_path.to_string(),
                    payload_path: launch.payload_path.to_string(),
                    mount_kind: launch.mount_kind.to_string(),
                    mount_index: launch.mount_index,
                    delay_secs: launch.delay_secs,
                })
            })
            .transpose()
            .map_err(|error| ReconciliationError::new("preview-availability", error))?;
        games.push(SystemGame {
            stable_key: format!(
                "{}\u{1f}{}\u{1f}{}",
                published.system_id,
                row.title.to_ascii_lowercase(),
                row.launch_ref
            ),
            title: row.title.to_string(),
            launch_ref: row.launch_ref.to_string(),
            preview_archive_path: row.preview_archive_path.to_string(),
            preview_asset_key: row.preview_asset_key.to_string(),
            has_preview: row.has_preview,
            year: metadata.year,
            manufacturer: metadata.manufacturer.to_string(),
            category: metadata.category.to_string(),
            players: metadata.players,
            control: metadata.control.to_string(),
            is_new: row.is_new,
            launch_plan,
        });
    }
    Ok(games)
}

fn reconcile_preview_rows(
    system_id: &SystemId,
    pack_path: &Path,
    mut games: Vec<SystemGame>,
) -> Result<(Vec<SystemGame>, usize, usize, usize), ReconciliationError> {
    let stems = crate::preview_worker::preview_archive_sidecar_entry_stems(pack_path)
        .map_err(|error| ReconciliationError::new("preview-availability", error))?
        .ok_or_else(|| ReconciliationError::new("preview-availability", "pack index is missing"))?;
    let entries = stems.entries.into_iter().collect::<HashSet<_>>();
    let stable_archive_path =
        crate::preview_worker::preview_archive_path_for_system(system_id.as_str());
    let mut candidate_rows = 0;
    let mut available_rows = 0;
    let mut changed_rows = 0;
    for game in &mut games {
        if game.preview_asset_key.is_empty() {
            continue;
        }
        candidate_rows += 1;
        let available = entries.contains(&game.preview_asset_key.to_ascii_lowercase());
        available_rows += usize::from(available);
        let archive_path = if available {
            stable_archive_path.as_str()
        } else {
            ""
        };
        if game.has_preview != available || game.preview_archive_path != archive_path {
            game.has_preview = available;
            game.preview_archive_path = archive_path.to_string();
            changed_rows += 1;
        }
    }
    Ok((games, candidate_rows, available_rows, changed_rows))
}

struct PreviewAvailabilityMaterializer(Option<MaterializedSystem>);

impl ReconciliationMaterializer for PreviewAvailabilityMaterializer {
    fn materialize(
        &mut self,
        system_id: &SystemId,
        _generation: u64,
    ) -> Result<MaterializedSystem, ReconciliationError> {
        self.0
            .take()
            .filter(|system| &system.system_id == system_id)
            .ok_or_else(|| {
                ReconciliationError::new("preview-availability", "materializer mismatch")
            })
    }

    fn commit_facts(&mut self) -> Result<(), ReconciliationError> {
        Ok(())
    }
}

pub fn production_registry_limits() -> RegistryLimits {
    crate::shard_registry::production_registry_limits()
}

pub fn publish_production_projection(
    storage_root: &Path,
    catalog: &ArcadeCatalog,
    limits: RegistryLimits,
) -> Result<ProductionProjectionOutcome, ReconciliationError> {
    publish_production_projection_with_events(storage_root, catalog, limits, &mut |_| {})
}

pub fn publish_production_projection_with_events(
    storage_root: &Path,
    catalog: &ArcadeCatalog,
    limits: RegistryLimits,
    emit: &mut dyn FnMut(ReconciliationEvent),
) -> Result<ProductionProjectionOutcome, ReconciliationError> {
    publish_production_projection_with_options(storage_root, catalog, limits, false, emit)
}

fn publish_production_projection_with_options(
    storage_root: &Path,
    catalog: &ArcadeCatalog,
    limits: RegistryLimits,
    force_all_systems: bool,
    emit: &mut dyn FnMut(ReconciliationEvent),
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
    let format_rebuild_required = match current_manifest
        .as_ref()
        .and_then(|manifest| manifest.format.as_ref())
    {
        None => current_manifest.is_some(),
        Some(format) => match crate::catalog_format::classify(format) {
            CatalogFormatStatus::Current => false,
            CatalogFormatStatus::UpgradeRequired { .. } => true,
            CatalogFormatStatus::UnsupportedFuture {
                installed,
                required,
            } => {
                return Err(ReconciliationError::new(
                    "projection",
                    format!(
                        "unsupported future catalog format: installed {}, required {}",
                        installed.label(),
                        required.label()
                    ),
                ));
            }
            CatalogFormatStatus::Corrupt {
                installed,
                required,
            } => {
                return Err(ReconciliationError::new(
                    "projection",
                    format!(
                        "incoherent catalog format: installed {}, required {}",
                        installed.label(),
                        required.label()
                    ),
                ));
            }
        },
    };
    let force_all_systems = force_all_systems || format_rebuild_required;
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
        let unchanged = !force_all_systems
            && match published {
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
        global_rebuild: force_all_systems,
        manifest_only: false,
    };
    let reconciliation_started = Instant::now();
    let result =
        execute_reconciliation_with_events(storage_root, &plan, limits, &mut materializer, emit)?;
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
            rebuilt,
            removed,
        }),
        ReconciliationOutcome::Unchanged { generation } => Ok(ProductionProjectionOutcome {
            generation: generation.unwrap_or(0),
            systems: systems.len(),
            games: catalog.len(),
            rebuilt_systems: 0,
            removed_systems: 0,
            rebuilt: Vec::new(),
            removed: Vec::new(),
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
    let loaded = match open_system_shard(
        &storage_root.join(&published.active.sqlite_path),
        &storage_root.join(&published.active.navigation_path),
        &published.system_id,
        published.active.generation,
        limits.shard,
    ) {
        Ok(loaded) => loaded,
        Err(error) if error.is_older_schema() => {
            crate::catalog_logln!(
                "catalog_system_rebuild_required_tsv\treason=schema-upgrade\tsystem={}\tgeneration={}\tdetail={}",
                published.system_id,
                published.active.generation,
                error.to_string().replace(['\t', '\n'], " ")
            );
            return Ok(false);
        }
        Err(error) => {
            return Err(ReconciliationError::new(
                "projection-compare",
                error.to_string(),
            ));
        }
    };
    Ok(loaded.games == candidate.games
        && candidate
            .projection_stats
            .is_none_or(|stats| loaded.projection_stats == Some(stats)))
}

pub fn publish_bound_production_projection(
    storage_root: &Path,
    catalog: &ArcadeCatalog,
    catalog_fingerprint: &str,
    limits: RegistryLimits,
) -> Result<ProductionProjectionOutcome, ReconciliationError> {
    publish_bound_production_projection_with_events(
        storage_root,
        catalog,
        catalog_fingerprint,
        limits,
        false,
        &mut |_| {},
    )
}

pub fn publish_bound_production_projection_with_events(
    storage_root: &Path,
    catalog: &ArcadeCatalog,
    catalog_fingerprint: &str,
    limits: RegistryLimits,
    force_all_systems: bool,
    emit: &mut dyn FnMut(ReconciliationEvent),
) -> Result<ProductionProjectionOutcome, ReconciliationError> {
    let outcome = publish_production_projection_with_options(
        storage_root,
        catalog,
        limits,
        force_all_systems,
        emit,
    )?;
    write_binding(
        storage_root,
        &CatalogBinding {
            schema_version: BINDING_SCHEMA_VERSION,
            format: Some(CatalogFormatDescriptor::current()),
            projection_contract: PRODUCTION_PROJECTION_CONTRACT.to_string(),
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
    match inspect_production_binding(storage_root, manifest_generation)? {
        ProductionBindingStatus::Current { fingerprint } => Ok(fingerprint),
        ProductionBindingStatus::UpgradeRequired {
            installed,
            required,
            ..
        } => Err(ReconciliationError::new(
            "binding",
            format!(
                "catalog projection upgrade required: installed {installed}, required {required}"
            ),
        )),
    }
}

pub fn inspect_production_binding(
    storage_root: &Path,
    manifest_generation: u64,
) -> Result<ProductionBindingStatus, ReconciliationError> {
    let binding = read_binding(storage_root)?;
    let manifest = read_latest_manifest_lazy(storage_root, production_registry_limits())
        .map_err(|error| ReconciliationError::new("binding", error.to_string()))?;
    let state = crate::catalog_state::read(&crate::catalog_state::path_for_root(storage_root))
        .map_err(|error| ReconciliationError::new("binding", error))?;
    let state_fingerprint = state.stamp.fingerprint_hex();
    if binding.schema_version != BINDING_SCHEMA_VERSION
        || binding.manifest_generation != manifest_generation
        || manifest.generation != manifest_generation
        || binding.catalog_fingerprint != state_fingerprint
    {
        return Err(ReconciliationError::new(
            "binding",
            "catalog binding does not match the active manifest and V3 state",
        ));
    }
    let legacy_format = if binding.format.is_none() && manifest.format.is_none() {
        CatalogFormatDescriptor::from_legacy_stamp_lines(state.stamp.lines())
    } else {
        None
    };
    if let Some(format) = binding
        .format
        .as_ref()
        .or(manifest.format.as_ref())
        .or(legacy_format.as_ref())
    {
        if binding.format.as_ref().is_some_and(|binding_format| {
            manifest
                .format
                .as_ref()
                .is_some_and(|manifest_format| binding_format != manifest_format)
        }) {
            return Err(ReconciliationError::new(
                "binding",
                "catalog binding and manifest format descriptors disagree",
            ));
        }
        match crate::catalog_format::classify(format) {
            CatalogFormatStatus::Current => {}
            CatalogFormatStatus::UpgradeRequired {
                installed,
                required,
            } => {
                return Ok(ProductionBindingStatus::UpgradeRequired {
                    fingerprint: state_fingerprint,
                    installed: installed.label(),
                    required: required.label(),
                });
            }
            CatalogFormatStatus::UnsupportedFuture {
                installed,
                required,
            } => {
                return Err(ReconciliationError::new(
                    "binding",
                    format!(
                        "unsupported future catalog format: installed {}, required {}",
                        installed.label(),
                        required.label()
                    ),
                ));
            }
            CatalogFormatStatus::Corrupt {
                installed,
                required,
            } => {
                return Err(ReconciliationError::new(
                    "binding",
                    format!(
                        "incoherent catalog format: installed {}, required {}",
                        installed.label(),
                        required.label()
                    ),
                ));
            }
        }
    }
    if binding.projection_contract != PRODUCTION_PROJECTION_CONTRACT {
        return Ok(ProductionBindingStatus::UpgradeRequired {
            fingerprint: state_fingerprint,
            installed: binding.projection_contract,
            required: PRODUCTION_PROJECTION_CONTRACT.to_string(),
        });
    }
    Ok(ProductionBindingStatus::Current {
        fingerprint: state_fingerprint,
    })
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
            category: game.category.to_string(),
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
        projection_stats: catalog
            .system_projection_stats(system_id.as_str())
            .map(|stats| SystemShardProjectionStats {
                source_games: stats.source_games,
                visible_families: stats.visible_families,
                collapsed_variants: stats.collapsed_variants,
            }),
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
    fn explicit_full_rebuild_republishes_every_system() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-production-projection-full-rebuild-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let catalog = two_system_catalog("Super Game");
        publish_production_projection(&root, &catalog, limits()).unwrap();

        let outcome = publish_production_projection_with_options(
            &root,
            &catalog,
            limits(),
            true,
            &mut |_| {},
        )
        .unwrap();

        assert_eq!(outcome.generation, 2);
        assert_eq!(outcome.rebuilt_systems, 2);
        assert_eq!(outcome.removed_systems, 0);
        assert_eq!(
            outcome
                .rebuilt
                .iter()
                .map(|system_id| system_id.as_str())
                .collect::<Vec<_>>(),
            vec!["c64", "snes"]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn format_upgrade_republishes_every_system_even_when_rows_match() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-production-projection-format-upgrade-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let catalog = two_system_catalog("Super Game");
        publish_production_projection(&root, &catalog, limits()).unwrap();
        let manifest_path = root.join("registry/manifest-a.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["format"] = serde_json::to_value(
            crate::catalog_format::CatalogFormatDescriptor::entry_prelude_predecessor(),
        )
        .unwrap();
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let outcome = publish_production_projection(&root, &catalog, limits()).unwrap();
        let upgraded = read_latest_manifest(&root, limits()).unwrap();

        assert_eq!(outcome.generation, 2);
        assert_eq!(outcome.rebuilt_systems, 2);
        assert_eq!(
            upgraded.format,
            Some(crate::catalog_format::CatalogFormatDescriptor::current())
        );
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
    fn older_shard_schema_rebuilds_all_affected_systems_atomically() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-production-projection-schema-upgrade-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let catalog = two_system_catalog("Super Game");
        publish_production_projection(&root, &catalog, limits()).unwrap();
        let before = read_latest_manifest(&root, limits()).unwrap();
        for system in &before.systems {
            let sqlite_path = root.join(&system.active.sqlite_path);
            let connection = rusqlite::Connection::open(&sqlite_path).unwrap();
            connection
                .execute(
                    "UPDATE shard_meta SET value=?1 WHERE key='schema_version'",
                    [crate::sharded_catalog::SHARD_SCHEMA_VERSION - 1],
                )
                .unwrap();
            drop(connection);

            let mut hash = 0xcbf2_9ce4_8422_2325u64;
            for byte in fs::read(&sqlite_path).unwrap() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            let manifest_path = root.join("registry/manifest-a.json");
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
            let stored_system = manifest["systems"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|stored| stored["system_id"] == system.system_id.as_str())
                .unwrap();
            stored_system["active"]["sqlite_hash"] = format!("{hash:016x}").into();
            fs::write(manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
        }

        let outcome = publish_production_projection(&root, &catalog, limits()).unwrap();
        let after = read_latest_manifest(&root, limits()).unwrap();

        assert_eq!(before.generation, 1);
        assert_eq!(outcome.generation, 2);
        assert_eq!(outcome.rebuilt_systems, 2);
        assert_eq!(after.generation, 2);
        let reader = LazyShardedCatalogReader::open(&root, limits()).unwrap();
        for system in reader.open_registry().unwrap().systems() {
            reader.open_system(&system.system_id).unwrap();
        }
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
        let different_fingerprint = different_state.stamp.fingerprint_hex();
        let different_catalog = ArcadeCatalog::new(
            PathBuf::from("/fixture"),
            vec![game("Different Game", "/games/SNES/Different.sfc", "snes")],
            vec![GameSystemEntry {
                id: "snes".to_string(),
                title: "SNES".to_string(),
                count: 1,
            }],
        );
        let interrupted_outcome = publish_bound_production_projection(
            &storage,
            &different_catalog,
            &different_fingerprint,
            limits(),
        )
        .unwrap();
        assert!(validate_production_binding(&storage, interrupted_outcome.generation).is_err());
        crate::catalog_state::write(
            &crate::catalog_state::path_for_root(&storage),
            &different_state,
        )
        .unwrap();
        validate_production_binding(&storage, interrupted_outcome.generation).unwrap();
        assert!(validate_production_binding(&storage, outcome.generation).is_err());
        assert!(!root.join("library.sqlite3").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn previous_projection_contract_is_reported_as_upgrade_required() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-production-projection-binding-upgrade-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state = fixture_state();
        let fingerprint = state.stamp.fingerprint_hex();
        let catalog = two_system_catalog("Super Game");
        let outcome =
            publish_bound_production_projection(&root, &catalog, &fingerprint, limits()).unwrap();
        crate::catalog_state::write(&crate::catalog_state::path_for_root(&root), &state).unwrap();
        write_binding(
            &root,
            &CatalogBinding {
                schema_version: BINDING_SCHEMA_VERSION,
                format: None,
                projection_contract: "rich-game-v1".to_string(),
                manifest_generation: outcome.generation,
                catalog_fingerprint: fingerprint.clone(),
            },
        )
        .unwrap();

        assert_eq!(
            inspect_production_binding(&root, outcome.generation).unwrap(),
            ProductionBindingStatus::UpgradeRequired {
                fingerprint,
                installed: "rich-game-v1".to_string(),
                required: PRODUCTION_PROJECTION_CONTRACT.to_string(),
            }
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preview_availability_reconciliation_publishes_once_and_repairs_bound_shard() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-preview-availability-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state = fixture_state();
        let fingerprint = state.stamp.fingerprint_hex();
        let mut present = game("Present", "/arcade/present.mra", "arcade");
        present.preview_asset_key = "present".into();
        let mut absent = game("Absent", "/arcade/absent.mra", "arcade");
        absent.preview_asset_key = "absent".into();
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/fixture"),
            vec![present, absent],
            vec![GameSystemEntry {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 2,
            }],
        );
        publish_bound_production_projection(&root, &catalog, &fingerprint, limits()).unwrap();
        crate::catalog_state::write(&crate::catalog_state::path_for_root(&root), &state).unwrap();
        let pack = root.join("arcade-pack.mmlz4b");
        write_preview_sidecar_index(&pack, &["present.rgb565"]);

        let repaired = reconcile_production_preview_availability(
            &root,
            &SystemId::parse("arcade").unwrap(),
            &pack,
            limits(),
        )
        .unwrap();

        assert_eq!(repaired.previous_generation, 1);
        assert_eq!(repaired.generation, 2);
        assert_eq!(repaired.candidate_rows, 2);
        assert_eq!(repaired.available_rows, 1);
        assert_eq!(repaired.changed_rows, 1);
        validate_production_binding(&root, 2).unwrap();
        let reader = LazyShardedCatalogReader::open(&root, limits()).unwrap();
        let system = reader
            .open_system(&SystemId::parse("arcade").unwrap())
            .unwrap();
        assert!(system.games()[0].has_preview);
        assert!(
            system.games()[0]
                .preview_archive_path
                .ends_with("/arcade-screenshots.mmlz4b")
        );
        assert!(!system.games()[1].has_preview);

        let unchanged = reconcile_production_preview_availability(
            &root,
            &SystemId::parse("arcade").unwrap(),
            &pack,
            limits(),
        )
        .unwrap();
        assert_eq!(unchanged.generation, 2);
        assert_eq!(unchanged.changed_rows, 0);
        assert_eq!(read_latest_manifest(&root, limits()).unwrap().generation, 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fast_preview_reconciliation_needs_no_binding_and_does_not_publish() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-fast-preview-availability-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let fingerprint = fixture_state().stamp.fingerprint_hex();
        let mut present = game("Present", "/arcade/present.mra", "arcade");
        present.preview_asset_key = "present".into();
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/fixture"),
            vec![present],
            vec![GameSystemEntry {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 1,
            }],
        );
        publish_bound_production_projection(&root, &catalog, &fingerprint, limits()).unwrap();
        fs::remove_file(root.join(BINDING_FILE)).unwrap();
        let manifest = read_latest_manifest(&root, limits()).unwrap();
        let published = manifest
            .systems
            .iter()
            .find(|system| system.system_id.as_str() == "arcade")
            .unwrap();
        let pack = root.join("arcade-pack.mmlz4b");
        write_preview_sidecar_index(&pack, &["present.rgb565"]);

        let reconciled = reconcile_fast_preview_availability(
            &root,
            &SystemId::parse("arcade").unwrap(),
            &pack,
            limits(),
        )
        .unwrap();

        assert_eq!(reconciled.previous_generation, 1);
        assert_eq!(reconciled.generation, 1);
        assert_eq!(reconciled.candidate_rows, 1);
        assert_eq!(reconciled.available_rows, 1);
        assert_eq!(reconciled.changed_rows, 1);
        assert!(reconciled.games[0].has_preview);
        assert_eq!(read_latest_manifest(&root, limits()).unwrap().generation, 1);
        assert!(!root.join(BINDING_FILE).exists());
        assert!(!open_fast_navpack_games(&root, published).unwrap()[0].has_preview);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn canonical_preview_rows_do_not_publish_a_repair_generation() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-preview-availability-canonical-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state = fixture_state();
        let fingerprint = state.stamp.fingerprint_hex();
        let stable_path = crate::preview_worker::preview_archive_path_for_system("arcade");
        let mut present = game("Present", "/arcade/present.mra", "arcade");
        present.preview_asset_key = "present".into();
        present.preview_archive_path = stable_path.into();
        present.has_preview = true;
        let mut absent = game("Absent", "/arcade/absent.mra", "arcade");
        absent.preview_asset_key = "absent".into();
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/fixture"),
            vec![present, absent],
            vec![GameSystemEntry {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 2,
            }],
        );
        publish_bound_production_projection(&root, &catalog, &fingerprint, limits()).unwrap();
        crate::catalog_state::write(&crate::catalog_state::path_for_root(&root), &state).unwrap();
        let pack = root.join("arcade-pack.mmlz4b");
        write_preview_sidecar_index(&pack, &["present.rgb565"]);

        let unchanged = reconcile_production_preview_availability(
            &root,
            &SystemId::parse("arcade").unwrap(),
            &pack,
            limits(),
        )
        .unwrap();

        assert_eq!(unchanged.previous_generation, 1);
        assert_eq!(unchanged.generation, 1);
        assert_eq!(unchanged.available_rows, 1);
        assert_eq!(unchanged.changed_rows, 0);
        assert_eq!(read_latest_manifest(&root, limits()).unwrap().generation, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preview_availability_reconciliation_rejects_missing_index_without_publication() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-preview-availability-missing-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state = fixture_state();
        let fingerprint = state.stamp.fingerprint_hex();
        let catalog = two_system_catalog("Super Game");
        publish_bound_production_projection(&root, &catalog, &fingerprint, limits()).unwrap();
        crate::catalog_state::write(&crate::catalog_state::path_for_root(&root), &state).unwrap();
        let pack = root.join("missing-index.mmlz4b");
        fs::write(&pack, b"pack").unwrap();

        assert!(
            reconcile_production_preview_availability(
                &root,
                &SystemId::parse("snes").unwrap(),
                &pack,
                limits(),
            )
            .is_err()
        );
        assert_eq!(read_latest_manifest(&root, limits()).unwrap().generation, 1);
        validate_production_binding(&root, 1).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    fn fixture_state() -> crate::catalog_state::CatalogState {
        crate::catalog_state::CatalogState {
            stamp: crate::catalog_stamp::CatalogStamp::from_lines(vec!["fixture".to_string()]),
            checkpoint: crate::catalog_checkpoint::CatalogDiscoveryCheckpoint::from_lines(vec![
                "fixture".to_string(),
            ]),
            stats: crate::catalog_state::CatalogStateStats {
                discoveries: 1,
                ..crate::catalog_state::CatalogStateStats::default()
            },
        }
    }

    fn write_preview_sidecar_index(pack: &Path, names: &[&str]) {
        fs::write(pack, b"pack").unwrap();
        let mut index = Vec::new();
        index.extend_from_slice(b"MMIDX02\0");
        index.extend_from_slice(&4u64.to_le_bytes());
        index
            .extend_from_slice(b"0000000000000000000000000000000000000000000000000000000000000000");
        index.extend_from_slice(&(names.len() as u32).to_le_bytes());
        for name in names {
            index.extend_from_slice(&(name.len() as u16).to_le_bytes());
            index.extend_from_slice(&1u32.to_le_bytes());
            index.extend_from_slice(&1u32.to_le_bytes());
            index.extend_from_slice(&2u32.to_le_bytes());
            index.extend_from_slice(&2u32.to_le_bytes());
            index.push(1);
            index.extend_from_slice(&2u32.to_le_bytes());
            index.extend_from_slice(&0u64.to_le_bytes());
            index.extend_from_slice(name.as_bytes());
        }
        fs::write(
            crate::preview_worker::preview_archive_sidecar_path_for_archive(pack),
            index,
        )
        .unwrap();
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
            category: "".into(),
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
