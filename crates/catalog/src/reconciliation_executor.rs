// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Transactional execution of exact sharded reconciliation plans.

use crate::catalog_classify::SystemId;
use crate::catalog_domain::ScanUnitId;
use crate::shard_registry::{
    CatalogManifest, ManifestSystem, PublishedGeneration, RegistryLimits,
    garbage_collect_unreferenced_with_retained, manifest_slots_present, publish_manifest,
    publish_prevalidated_system_artifacts_deferred, read_latest_manifest, sync_artifact_batch,
    validate_published_system,
};
use crate::sharded_catalog::{PlannedSystem, PlannedSystemAction, ReconcilePlan};
use crate::system_shard::{
    ShardDurability, SystemGame, SystemShardData, write_system_shard_with_durability,
};
use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
thread_local! {
    static FAIL_ARTIFACT_BARRIER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_MANIFEST_PUBLICATION: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

fn run_artifact_barrier(storage_root: &Path) -> Result<(), ReconciliationError> {
    #[cfg(test)]
    if FAIL_ARTIFACT_BARRIER.with(|fail| fail.replace(false)) {
        return Err(ReconciliationError::new(
            "artifact-barrier",
            "injected artifact barrier failure",
        ));
    }
    sync_artifact_batch(storage_root)
        .map_err(|error| ReconciliationError::new("artifact-barrier", error.to_string()))
}

fn run_manifest_publication(
    storage_root: &Path,
    manifest: &CatalogManifest,
    limits: RegistryLimits,
) -> Result<(), ReconciliationError> {
    #[cfg(test)]
    #[cfg(test)]
    let injected_failure = FAIL_MANIFEST_PUBLICATION.with(|fail| fail.replace(0));
    #[cfg(test)]
    if injected_failure == 1 {
        return Err(ReconciliationError::new(
            "publish-manifest",
            "injected manifest publication failure",
        ));
    }
    publish_manifest(storage_root, manifest, limits)
        .map(|_| ())
        .map_err(|error| ReconciliationError::new("publish-manifest", error.to_string()))?;
    #[cfg(test)]
    if injected_failure == 2 {
        return Err(ReconciliationError::new(
            "publish-manifest",
            "injected post-rename manifest failure",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterializedSystem {
    pub system_id: SystemId,
    pub display_title: String,
    pub section: String,
    pub family: String,
    pub order: u32,
    pub producers: Vec<ScanUnitId>,
    pub projection_stats: Option<crate::system_shard::SystemShardProjectionStats>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReconciliationEvent {
    SystemScanning {
        system_id: SystemId,
    },
    SystemPrepared {
        system_id: SystemId,
        generation: u64,
    },
    SystemFailed {
        system_id: SystemId,
        stage: String,
        error: String,
    },
    ManifestPublished {
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
    execute_reconciliation_with_events(storage_root, plan, limits, materializer, &mut |_| {})
}

pub fn execute_reconciliation_with_events(
    storage_root: &Path,
    plan: &ReconcilePlan,
    limits: RegistryLimits,
    materializer: &mut impl ReconciliationMaterializer,
    emit: &mut dyn FnMut(ReconciliationEvent),
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
        crate::catalog_logln!(
            "catalog_v3_reconciliation_tsv\tgeneration={}\trebuilt=0\tmaterialize_us=0\tshard_workers=0\tshard_batch_us=0\tshard_build_wall_us=0\tshard_publication_wall_us=0\tshard_write_us=0\tartifact_publish_us=0\tartifact_copy_hash_us=0\tartifact_publish_bytes=0\tpipeline_overlap_us=0\tpipeline_queue_wait_us=0\tpipeline_peak_in_flight=0\tpipeline_fallbacks=0\tbarrier_us=0\tmanifest_us=0\tslowest_system=none\tslowest_us=0",
            actual_generation.unwrap_or(0)
        );
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

    let mut resume_journal = crate::build_progress::BuildProgressJournal::open_for_projection(
        &crate::build_progress::path_for_root(storage_root),
    )
    .ok();
    let mut saved_systems = HashMap::new();
    if let Some(journal) = resume_journal.as_ref() {
        for saved in journal.completed_shards().unwrap_or_default() {
            if saved.generation != expected_generation {
                continue;
            }
            if let Ok(system) = serde_json::from_str::<ManifestSystem>(&saved.manifest_system_json)
            {
                saved_systems.insert(system.system_id.clone(), system);
            }
        }
    }
    let retained = saved_systems.values().flat_map(|system| {
        [
            system.active.sqlite_path.clone(),
            system.active.navigation_path.clone(),
        ]
    });
    garbage_collect_unreferenced_with_retained(
        storage_root,
        current.as_ref().unwrap_or(&CatalogManifest {
            format: None,
            generation: 0,
            systems: Vec::new(),
        }),
        retained,
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
    let mut materialize_time = Duration::ZERO;
    let mut shard_write_time = Duration::ZERO;
    let mut artifact_publish_time = Duration::ZERO;
    let mut shard_build_wall_time = Duration::ZERO;
    let mut shard_publication_wall_time = Duration::ZERO;
    let mut artifact_copy_hash_time = Duration::ZERO;
    let mut artifact_publish_bytes = 0_u64;
    let mut slowest_shard = (String::new(), Duration::ZERO);
    let mut worker_count = 1;
    let mut pipeline_overlap = Duration::ZERO;
    let mut pipeline_queue_wait = Duration::ZERO;
    let mut pipeline_peak_in_flight = 1_usize;
    let mut pipeline_fallbacks = 0_usize;
    for planned in &plan.systems {
        if planned.action == PlannedSystemAction::Remove {
            systems.retain(|system| system.system_id != planned.system_id);
            removed.push(planned.system_id.clone());
        }
    }
    let shard_batch_started = Instant::now();
    let rebuilds = plan
        .systems
        .iter()
        .filter(|planned| planned.action == PlannedSystemAction::Rebuild)
        .collect::<Vec<_>>();
    let pipeline_enabled = current.is_none() && saved_systems.is_empty() && rebuilds.len() > 1;
    let completed_shards = if pipeline_enabled {
        worker_count = 2;
        for planned in &rebuilds {
            emit(ReconciliationEvent::SystemScanning {
                system_id: planned.system_id.clone(),
            });
        }
        let pipeline = execute_fresh_pipeline(
            storage_root,
            &rebuilds,
            expected_generation,
            nonce,
            limits,
            materializer,
        )
        .inspect_err(|_| remove_planned_generation(storage_root, &rebuilds, expected_generation))?;
        materialize_time += pipeline.materialize_time;
        pipeline_overlap = pipeline.overlap;
        pipeline_queue_wait = pipeline.queue_wait;
        pipeline_peak_in_flight = pipeline.peak_in_flight;
        pipeline_fallbacks = pipeline.fallbacks;
        shard_build_wall_time = pipeline.build_time;
        shard_publication_wall_time = pipeline.publish_time;
        if let Some(journal) = resume_journal.as_mut() {
            sync_artifact_batch(storage_root)
                .map_err(|error| ReconciliationError::new("shard-checkpoint", error.to_string()))?;
            for shard in &pipeline.completed {
                checkpoint_published_shard(storage_root, journal, shard, limits)?;
            }
        }
        for shard in &pipeline.completed {
            emit(ReconciliationEvent::SystemPrepared {
                system_id: shard.system.system_id.clone(),
                generation: expected_generation,
            });
        }
        pipeline.completed
    } else {
        let mut sequential = Vec::with_capacity(rebuilds.len());
        for planned in &rebuilds {
            emit(ReconciliationEvent::SystemScanning {
                system_id: planned.system_id.clone(),
            });
            crate::cooperative_work::checkpoint();
            let phase_started = Instant::now();
            let materialized =
                match materializer.materialize(&planned.system_id, expected_generation) {
                    Ok(materialized) => materialized,
                    Err(error) => {
                        emit(ReconciliationEvent::SystemFailed {
                            system_id: planned.system_id.clone(),
                            stage: error.stage().to_string(),
                            error: error.to_string(),
                        });
                        if resume_journal.is_none() {
                            remove_planned_generation(storage_root, &rebuilds, expected_generation);
                        }
                        return Err(error);
                    }
                };
            materialize_time += phase_started.elapsed();
            if let Err(error) = validate_materialized(&planned.system_id, &materialized) {
                emit(ReconciliationEvent::SystemFailed {
                    system_id: planned.system_id.clone(),
                    stage: error.stage().to_string(),
                    error: error.to_string(),
                });
                if resume_journal.is_none() {
                    remove_planned_generation(storage_root, &rebuilds, expected_generation);
                }
                return Err(error);
            }
            if let Some(saved) = saved_systems.remove(&planned.system_id) {
                match saved_system_matches(storage_root, &saved, &materialized, limits) {
                    Ok(true) => {
                        crate::catalog_logln!(
                            "catalog_resume_tsv\tphase=shard-reused\tsystem_id={}\tgeneration={}\treason=exact-match",
                            planned.system_id.as_str(),
                            expected_generation
                        );
                        sequential.push(CompletedShard {
                            system: saved,
                            write_time: Duration::ZERO,
                            publish_time: Duration::ZERO,
                            elapsed: Duration::ZERO,
                            artifact_bytes: 0,
                            copy_hash_time: Duration::ZERO,
                        });
                        emit(ReconciliationEvent::SystemPrepared {
                            system_id: planned.system_id.clone(),
                            generation: expected_generation,
                        });
                        continue;
                    }
                    Ok(false) => {
                        crate::catalog_logln!(
                            "catalog_resume_tsv\tphase=shard-invalidated\tsystem_id={}\tgeneration={}\treason=canonical-mismatch",
                            planned.system_id.as_str(),
                            expected_generation
                        );
                        remove_saved_system_artifacts(storage_root, &saved);
                    }
                    Err(error) => {
                        crate::catalog_logln!(
                            "catalog_resume_tsv\tphase=shard-invalidated\tsystem_id={}\tgeneration={}\treason={}",
                            planned.system_id.as_str(),
                            expected_generation,
                            error
                        );
                        remove_saved_system_artifacts(storage_root, &saved);
                    }
                }
            }
            let previous = systems
                .iter()
                .find(|system| system.system_id == planned.system_id)
                .map(|system| system.active.clone());
            let staging_root = shard_build_root(storage_root, &materialized.games);
            let shard = build_shard_job(
                storage_root,
                &staging_root,
                expected_generation,
                nonce,
                limits,
                ShardBuildJob {
                    materialized,
                    previous,
                },
            );
            match shard {
                Ok(shard) => {
                    if let Some(journal) = resume_journal.as_mut() {
                        sync_artifact_batch(storage_root).map_err(|error| {
                            ReconciliationError::new("shard-checkpoint", error.to_string())
                        })?;
                        checkpoint_published_shard(storage_root, journal, &shard, limits)?;
                    }
                    shard_build_wall_time += shard.elapsed.saturating_sub(shard.publish_time);
                    shard_publication_wall_time += shard.publish_time;
                    emit(ReconciliationEvent::SystemPrepared {
                        system_id: planned.system_id.clone(),
                        generation: expected_generation,
                    });
                    sequential.push(shard);
                }
                Err(error) => {
                    emit(ReconciliationEvent::SystemFailed {
                        system_id: planned.system_id.clone(),
                        stage: error.stage().to_string(),
                        error: error.to_string(),
                    });
                    if resume_journal.is_none() {
                        remove_planned_generation(storage_root, &rebuilds, expected_generation);
                    }
                    return Err(error);
                }
            }
        }
        sequential
    };
    for shard in completed_shards {
        shard_write_time += shard.write_time;
        artifact_publish_time += shard.publish_time;
        artifact_copy_hash_time += shard.copy_hash_time;
        artifact_publish_bytes = artifact_publish_bytes.saturating_add(shard.artifact_bytes);
        if shard.elapsed > slowest_shard.1 {
            slowest_shard = (shard.system.system_id.as_str().to_string(), shard.elapsed);
        }
        rebuilt.push(shard.system.system_id.clone());
        systems.retain(|system| system.system_id != shard.system.system_id);
        systems.push(shard.system);
    }
    rebuilt.sort();
    let shard_batch_time = shard_batch_started.elapsed();
    let finalize = (|| {
        materializer.refresh_manifest(&mut systems)?;
        systems.sort_by(|left, right| left.system_id.cmp(&right.system_id));
        let barrier_started = Instant::now();
        run_artifact_barrier(storage_root)?;
        let barrier_time = barrier_started.elapsed();
        let manifest_started = Instant::now();
        run_manifest_publication(
            storage_root,
            &CatalogManifest {
                format: Some(crate::catalog_format::CatalogFormatDescriptor::current()),
                generation: expected_generation,
                systems,
            },
            limits,
        )?;
        Ok((barrier_time, manifest_started.elapsed()))
    })();
    let (barrier_time, manifest_time) = match finalize {
        Ok(times) => times,
        Err(error) => {
            if resume_journal.is_none()
                && planned_generation_cleanup_is_safe(storage_root, limits, expected_generation)
            {
                remove_planned_generation(storage_root, &rebuilds, expected_generation);
            }
            return Err(error);
        }
    };
    materializer.commit_facts()?;
    emit(ReconciliationEvent::ManifestPublished {
        generation: expected_generation,
        rebuilt: rebuilt.clone(),
        removed: removed.clone(),
    });
    crate::catalog_logln!(
        "catalog_v3_reconciliation_tsv\tgeneration={}\trebuilt={}\tmaterialize_us={}\tshard_workers={}\tshard_batch_us={}\tshard_build_wall_us={}\tshard_publication_wall_us={}\tshard_write_us={}\tartifact_publish_us={}\tartifact_copy_hash_us={}\tartifact_publish_bytes={}\tpipeline_overlap_us={}\tpipeline_queue_wait_us={}\tpipeline_peak_in_flight={}\tpipeline_fallbacks={}\tbarrier_us={}\tmanifest_us={}\tslowest_system={}\tslowest_us={}",
        expected_generation,
        rebuilt.len(),
        materialize_time.as_micros(),
        worker_count,
        shard_batch_time.as_micros(),
        shard_build_wall_time.as_micros(),
        shard_publication_wall_time.as_micros(),
        shard_write_time.as_micros(),
        artifact_publish_time.as_micros(),
        artifact_copy_hash_time.as_micros(),
        artifact_publish_bytes,
        pipeline_overlap.as_micros(),
        pipeline_queue_wait.as_micros(),
        pipeline_peak_in_flight,
        pipeline_fallbacks,
        barrier_time.as_micros(),
        manifest_time.as_micros(),
        slowest_shard.0,
        slowest_shard.1.as_micros(),
    );
    Ok(ReconciliationOutcome::Published {
        generation: expected_generation,
        rebuilt,
        removed,
    })
}

fn checkpoint_published_shard(
    storage_root: &Path,
    journal: &mut crate::build_progress::BuildProgressJournal,
    shard: &CompletedShard,
    limits: RegistryLimits,
) -> Result<(), ReconciliationError> {
    validate_published_system(storage_root, &shard.system, limits)
        .map_err(|error| ReconciliationError::new("shard-checkpoint", error.to_string()))?;
    let active = &shard.system.active;
    journal
        .record_shard(&crate::build_progress::CompletedShard {
            system_id: shard.system.system_id.as_str().to_string(),
            generation: active.generation,
            sqlite_path: active.sqlite_path.display().to_string(),
            navigation_path: active.navigation_path.display().to_string(),
            content_hash: format!("{}:{}", active.sqlite_hash, active.navigation_hash),
            manifest_system_json: serde_json::to_string(&shard.system)
                .map_err(|error| ReconciliationError::new("shard-checkpoint", error.to_string()))?,
        })
        .map_err(|error| ReconciliationError::new("shard-checkpoint", error))?;
    crate::catalog_logln!(
        "catalog_resume_tsv\tphase=shard-committed\tsystem_id={}\tgeneration={}\treason=durable",
        shard.system.system_id.as_str(),
        active.generation
    );
    Ok(())
}

fn saved_system_matches(
    storage_root: &Path,
    saved: &ManifestSystem,
    candidate: &MaterializedSystem,
    limits: RegistryLimits,
) -> Result<bool, ReconciliationError> {
    validate_published_system(storage_root, saved, limits)
        .map_err(|error| ReconciliationError::new("shard-resume", error.to_string()))?;
    if saved.system_id != candidate.system_id
        || saved.display_title != candidate.display_title
        || saved.section != candidate.section
        || saved.family != candidate.family
        || saved.order != candidate.order
        || saved.producers != candidate.producers
    {
        return Ok(false);
    }
    let loaded = crate::system_shard::open_system_shard(
        &storage_root.join(&saved.active.sqlite_path),
        &storage_root.join(&saved.active.navigation_path),
        &saved.system_id,
        saved.active.generation,
        limits.shard,
    )
    .map_err(|error| ReconciliationError::new("shard-resume", error.to_string()))?;
    Ok(loaded.games == candidate.games)
}

fn remove_saved_system_artifacts(storage_root: &Path, saved: &ManifestSystem) {
    let expected = PathBuf::from("systems").join(saved.system_id.as_str());
    for path in [&saved.active.sqlite_path, &saved.active.navigation_path]
        .into_iter()
        .chain(saved.active.navpack.iter().map(|navpack| &navpack.path))
    {
        if path.parent() == Some(expected.as_path()) {
            let _ = fs::remove_file(storage_root.join(path));
        }
    }
}

fn planned_generation_cleanup_is_safe(
    storage_root: &Path,
    limits: RegistryLimits,
    intended_generation: u64,
) -> bool {
    match read_latest_manifest(storage_root, limits) {
        Ok(manifest) => manifest.generation != intended_generation,
        Err(_) => manifest_slots_definitely_absent(storage_root),
    }
}

fn manifest_slots_definitely_absent(storage_root: &Path) -> bool {
    ["registry/manifest-a.json", "registry/manifest-b.json"]
        .iter()
        .all(
            |relative| match fs::symlink_metadata(storage_root.join(relative)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                Ok(_) | Err(_) => false,
            },
        )
}

struct FreshPipelineOutcome {
    completed: Vec<CompletedShard>,
    materialize_time: Duration,
    overlap: Duration,
    queue_wait: Duration,
    peak_in_flight: usize,
    fallbacks: usize,
    build_time: Duration,
    publish_time: Duration,
}

fn execute_fresh_pipeline(
    storage_root: &Path,
    rebuilds: &[&PlannedSystem],
    generation: u64,
    nonce: u128,
    limits: RegistryLimits,
    materializer: &mut impl ReconciliationMaterializer,
) -> Result<FreshPipelineOutcome, ReconciliationError> {
    let pipeline_started = Instant::now();
    let background = crate::cooperative_work::in_background_scope();
    let (staged_tx, staged_rx) = mpsc::sync_channel::<StagedShard>(1);
    let (completed_tx, completed_rx) =
        mpsc::channel::<Result<CompletedShard, ReconciliationError>>();
    std::thread::scope(|scope| {
        let publisher = scope.spawn(move || {
            if background {
                crate::runtime_thread::apply_runtime_thread_policy(
                    crate::runtime_thread::RuntimeThreadRole::CatalogShardPublisher,
                );
            }
            let _background_scope =
                background.then(crate::cooperative_work::BackgroundScope::enter);
            let result =
                publish_staged_shards(storage_root, generation, limits, staged_rx, completed_tx);
            mister_magik_perf_events::submit_thread_profile("catalog-publisher");
            result
        });
        let mut completed = Vec::with_capacity(rebuilds.len());
        let mut in_flight = 0_usize;
        let mut peak_in_flight = 0_usize;
        let mut materialize_time = Duration::ZERO;
        let mut build_time = Duration::ZERO;
        let mut queue_wait = Duration::ZERO;
        let mut fallbacks = 0_usize;
        let mut failure = None;

        for planned in rebuilds {
            crate::cooperative_work::checkpoint();
            while in_flight >= 2 {
                if let Err(error) = receive_published_shard(
                    &completed_rx,
                    &mut completed,
                    &mut in_flight,
                    &mut queue_wait,
                ) {
                    failure = Some(error);
                    break;
                }
            }
            if failure.is_some() {
                break;
            }

            let phase_started = Instant::now();
            let materialized = match materializer.materialize(&planned.system_id, generation) {
                Ok(materialized) => materialized,
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            };
            materialize_time += phase_started.elapsed();
            if let Err(error) = validate_materialized(&planned.system_id, &materialized) {
                failure = Some(error);
                break;
            }

            let mut staging_root = shard_build_root(storage_root, &materialized.games);
            let device_fallback = storage_root.starts_with(Path::new("/media/fat"))
                && !staging_root.starts_with(Path::new("/tmp/mister-magik/catalog-v3-build"));
            if device_fallback {
                while in_flight > 0 {
                    if let Err(error) = receive_published_shard(
                        &completed_rx,
                        &mut completed,
                        &mut in_flight,
                        &mut queue_wait,
                    ) {
                        failure = Some(error);
                        break;
                    }
                }
                if failure.is_some() {
                    break;
                }
                staging_root = shard_build_root(storage_root, &materialized.games);
            }

            peak_in_flight = peak_in_flight.max(in_flight.saturating_add(1));
            let staged = match build_and_validate_shard(
                &staging_root,
                generation,
                nonce,
                limits,
                ShardBuildJob {
                    materialized,
                    previous: None,
                },
            ) {
                Ok(staged) => staged,
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            };
            build_time += staged.build_elapsed;
            let staged_system_id = staged
                .metadata
                .as_ref()
                .map(|metadata| metadata.system_id.as_str().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            crate::catalog_logln!(
                "catalog_v3_shard_phase_tsv\tsystem={}\tphase=build\telapsed_us={}\tgames={}",
                staged_system_id,
                staged.build_elapsed.as_micros(),
                staged.game_count,
            );

            let staged_on_media = storage_root.starts_with(Path::new("/media/fat"))
                && !staged
                    .staging
                    .starts_with(Path::new("/tmp/mister-magik/catalog-v3-build"));
            if staged_on_media {
                fallbacks += 1;
                match publish_staged_shard(storage_root, generation, limits, staged) {
                    Ok(shard) => completed.push(shard),
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
                continue;
            }

            let wait_started = Instant::now();
            if let Err(error) = staged_tx.send(staged) {
                failure = completed_rx.recv().ok().and_then(Result::err).or_else(|| {
                    Some(ReconciliationError::new(
                        "pipeline",
                        format!("publisher stopped before accepting shard: {error}"),
                    ))
                });
                break;
            }
            let shard_queue_wait = wait_started.elapsed();
            queue_wait += shard_queue_wait;
            crate::catalog_logln!(
                "catalog_v3_shard_phase_tsv\tsystem={}\tphase=queue-wait\telapsed_us={}",
                staged_system_id,
                shard_queue_wait.as_micros(),
            );
            in_flight += 1;
            peak_in_flight = peak_in_flight.max(in_flight);
            while let Ok(result) = completed_rx.try_recv() {
                in_flight = in_flight.saturating_sub(1);
                match result {
                    Ok(shard) => completed.push(shard),
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
            if failure.is_some() {
                break;
            }
        }

        drop(staged_tx);
        while failure.is_none() && in_flight > 0 {
            if let Err(error) = receive_published_shard(
                &completed_rx,
                &mut completed,
                &mut in_flight,
                &mut queue_wait,
            ) {
                failure = Some(error);
            }
        }
        let publisher_result = publisher
            .join()
            .map_err(|_| ReconciliationError::new("pipeline", "publisher thread panicked"));
        if failure.is_none() {
            failure = match publisher_result {
                Ok(result) => result.err(),
                Err(error) => Some(error),
            };
        }
        if let Some(error) = failure {
            return Err(error);
        }

        let publish_time = completed
            .iter()
            .fold(Duration::ZERO, |total, shard| total + shard.publish_time);
        let serial_time = materialize_time
            .saturating_add(build_time)
            .saturating_add(publish_time);
        Ok(FreshPipelineOutcome {
            completed,
            materialize_time,
            overlap: serial_time.saturating_sub(pipeline_started.elapsed()),
            queue_wait,
            peak_in_flight,
            fallbacks,
            build_time,
            publish_time,
        })
    })
}

fn publish_staged_shards(
    storage_root: &Path,
    generation: u64,
    limits: RegistryLimits,
    staged_rx: Receiver<StagedShard>,
    completed_tx: mpsc::Sender<Result<CompletedShard, ReconciliationError>>,
) -> Result<(), ReconciliationError> {
    for staged in staged_rx {
        crate::cooperative_work::checkpoint();
        let result = publish_staged_shard(storage_root, generation, limits, staged);
        let failed = result.is_err();
        if completed_tx.send(result).is_err() {
            return Ok(());
        }
        if failed {
            return Err(ReconciliationError::new(
                "pipeline",
                "publisher stopped after artifact failure",
            ));
        }
    }
    Ok(())
}

fn receive_published_shard(
    completed_rx: &Receiver<Result<CompletedShard, ReconciliationError>>,
    completed: &mut Vec<CompletedShard>,
    in_flight: &mut usize,
    queue_wait: &mut Duration,
) -> Result<(), ReconciliationError> {
    let wait_started = Instant::now();
    let result = completed_rx
        .recv()
        .map_err(|_| ReconciliationError::new("pipeline", "publisher disconnected"))?;
    *queue_wait += wait_started.elapsed();
    *in_flight = in_flight.saturating_sub(1);
    completed.push(result?);
    Ok(())
}

fn remove_planned_generation(storage_root: &Path, rebuilds: &[&PlannedSystem], generation: u64) {
    for planned in rebuilds {
        let directory = storage_root
            .join("systems")
            .join(planned.system_id.as_str());
        let _ = fs::remove_file(directory.join(format!("{generation}.sqlite3")));
        let _ = fs::remove_file(directory.join(format!("{generation}.nav.lz4b")));
        let _ = fs::remove_file(directory.join(format!("{generation}.navpack")));
    }
}

struct ShardBuildJob {
    materialized: MaterializedSystem,
    previous: Option<PublishedGeneration>,
}

struct StagedShard {
    staging: PathBuf,
    metadata: Option<MaterializedSystemMetadata>,
    game_count: u64,
    previous: Option<PublishedGeneration>,
    write_time: Duration,
    build_elapsed: Duration,
}

struct MaterializedSystemMetadata {
    system_id: SystemId,
    display_title: String,
    section: String,
    family: String,
    order: u32,
    producers: Vec<ScanUnitId>,
}

impl Drop for StagedShard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.staging);
    }
}

struct CompletedShard {
    system: ManifestSystem,
    write_time: Duration,
    publish_time: Duration,
    elapsed: Duration,
    artifact_bytes: u64,
    copy_hash_time: Duration,
}

fn build_shard_job(
    storage_root: &Path,
    staging_root: &Path,
    generation: u64,
    nonce: u128,
    limits: RegistryLimits,
    job: ShardBuildJob,
) -> Result<CompletedShard, ReconciliationError> {
    let staged = build_and_validate_shard(staging_root, generation, nonce, limits, job)?;
    publish_staged_shard(storage_root, generation, limits, staged)
}

fn build_and_validate_shard(
    staging_root: &Path,
    generation: u64,
    nonce: u128,
    limits: RegistryLimits,
    job: ShardBuildJob,
) -> Result<StagedShard, ReconciliationError> {
    let started = Instant::now();
    let system_id = job.materialized.system_id.clone();
    let staging = staging_root.join(format!(
        "reconcile-{}-{generation}-{nonce}-{}",
        std::process::id(),
        system_id.as_str()
    ));
    fs::create_dir_all(&staging).map_err(|error| ReconciliationError::with("stage", error))?;
    let sqlite = staging.join("system.sqlite3");
    let navigation = staging.join("system.nav.lz4b");
    let game_count = job.materialized.games.len() as u64;
    let metadata = MaterializedSystemMetadata {
        system_id: job.materialized.system_id.clone(),
        display_title: job.materialized.display_title,
        section: job.materialized.section,
        family: job.materialized.family,
        order: job.materialized.order,
        producers: job.materialized.producers,
    };
    let write_started = Instant::now();
    if let Err(error) = write_system_shard_with_durability(
        &sqlite,
        &navigation,
        SystemShardData {
            system_id: system_id.clone(),
            generation,
            projection_stats: job.materialized.projection_stats,
            games: job.materialized.games,
        },
        limits.shard,
        ShardDurability::Deferred,
    ) {
        let _ = fs::remove_dir_all(&staging);
        return Err(ReconciliationError::new("write", error.to_string()));
    }
    let write_time = write_started.elapsed();
    Ok(StagedShard {
        staging,
        metadata: Some(metadata),
        game_count,
        previous: job.previous,
        write_time,
        build_elapsed: started.elapsed(),
    })
}

fn publish_staged_shard(
    storage_root: &Path,
    generation: u64,
    limits: RegistryLimits,
    mut staged: StagedShard,
) -> Result<CompletedShard, ReconciliationError> {
    let sqlite = staged.staging.join("system.sqlite3");
    let navigation = staged.staging.join("system.nav.lz4b");
    let metadata = staged
        .metadata
        .take()
        .expect("staged shard owns system metadata");
    let system_id = metadata.system_id.clone();
    let publish_started = Instant::now();
    // The deferred writer has already reopened and fully validated both files.
    let copy_hash_pmu = mister_magik_perf_events::sampled_span(crate::pmu_phase::PUBLISH_COPY_HASH);
    let publication = publish_prevalidated_system_artifacts_deferred(
        storage_root,
        &sqlite,
        &navigation,
        &system_id,
        generation,
        staged.game_count,
        limits,
    )
    .map_err(|error| ReconciliationError::new("publish-artifact", error.to_string()));
    drop(copy_hash_pmu);
    let publication = match publication {
        Ok(publication) => publication,
        Err(error) => {
            return Err(error);
        }
    };
    let publish_time = publish_started.elapsed();
    let artifact_bytes = publication.copied_bytes;
    let copy_hash_time = publication.copy_hash_time;
    let active = publication.generation;
    crate::catalog_logln!(
        "catalog_v3_shard_phase_tsv\tsystem={}\tphase=publish\telapsed_us={}\tbytes={}\tcopy_hash_us={}\tcompleted_us={}",
        system_id.as_str(),
        publish_time.as_micros(),
        artifact_bytes,
        copy_hash_time.as_micros(),
        staged
            .build_elapsed
            .saturating_add(publish_time)
            .as_micros(),
    );
    Ok(CompletedShard {
        system: ManifestSystem {
            system_id: metadata.system_id,
            display_title: metadata.display_title,
            section: metadata.section,
            family: metadata.family,
            order: metadata.order,
            producers: metadata.producers,
            active,
            previous: staged.previous.take(),
        },
        write_time: staged.write_time,
        publish_time,
        elapsed: staged.build_elapsed.saturating_add(publish_time),
        artifact_bytes,
        copy_hash_time,
    })
}

fn shard_build_root(storage_root: &Path, games: &[SystemGame]) -> PathBuf {
    shard_build_root_for_available(
        storage_root,
        games,
        tmpfs_available_bytes(Path::new("/tmp")),
    )
}

fn shard_build_root_for_available(
    storage_root: &Path,
    games: &[SystemGame],
    available: Option<u64>,
) -> PathBuf {
    if cfg!(target_os = "linux")
        && storage_root.starts_with(Path::new("/media/fat"))
        && available.is_some_and(|available| available >= estimated_shard_build_bytes(games))
    {
        PathBuf::from("/tmp/mister-magik/catalog-v3-build")
    } else {
        storage_root.join("staging")
    }
}

fn estimated_shard_build_bytes(games: &[SystemGame]) -> u64 {
    const FIXED_HEADROOM: u64 = 32 * 1024 * 1024;
    const EXPANSION_HEADROOM: u64 = 6;
    let strings = games.iter().fold(0_u64, |total, game| {
        let game_bytes = [
            game.stable_key.len(),
            game.title.len(),
            game.launch_ref.len(),
            game.preview_archive_path.len(),
            game.preview_asset_key.len(),
            game.manufacturer.len(),
            game.control.len(),
        ]
        .into_iter()
        .fold(0_u64, |sum, len| sum.saturating_add(len as u64));
        total.saturating_add(game_bytes).saturating_add(256)
    });
    FIXED_HEADROOM.saturating_add(strings.saturating_mul(EXPANSION_HEADROOM))
}

#[cfg(target_os = "linux")]
fn tmpfs_available_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and a successful statvfs initializes stats.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    statvfs_value_to_u64(stats.f_bavail).checked_mul(statvfs_value_to_u64(stats.f_frsize))
}

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
fn statvfs_value_to_u64(value: u64) -> u64 {
    value
}

#[cfg(all(target_os = "linux", target_pointer_width = "32"))]
fn statvfs_value_to_u64<T: Into<u64>>(value: T) -> u64 {
    value.into()
}

#[cfg(not(target_os = "linux"))]
fn tmpfs_available_bytes(_path: &Path) -> Option<u64> {
    None
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
    use crate::shard_registry::{RegistryLimits, read_latest_manifest};
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
    fn failed_materialization_keeps_old_manifest_and_retry_succeeds() {
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
        assert!(!root.join("systems/c64/2.sqlite3").exists());
        assert!(!root.join("systems/c64/2.nav.lz4b").exists());
        assert!(!root.join("systems/c64/2.navpack").exists());

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

    #[test]
    fn failed_batch_barrier_keeps_previous_manifest_authoritative() {
        let root = temporary_root("barrier-failure");
        let mut materializer = FixtureMaterializer::new();
        execute_reconciliation(&root, &plan(None, 1, &["c64"]), limits(), &mut materializer)
            .unwrap();
        let before = read_latest_manifest(&root, limits()).unwrap();

        materializer.games.insert("c64", vec![game("Two")]);
        FAIL_ARTIFACT_BARRIER.with(|fail| fail.set(true));
        let error = execute_reconciliation(
            &root,
            &plan(Some(1), 2, &["c64"]),
            limits(),
            &mut materializer,
        )
        .unwrap_err();

        assert_eq!(error.stage(), "artifact-barrier");
        assert_eq!(read_latest_manifest(&root, limits()).unwrap(), before);
        assert_eq!(materializer.commits, 1);
        assert!(!root.join("systems/c64/2.sqlite3").exists());
        assert!(!root.join("systems/c64/2.nav.lz4b").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_manifest_publication_removes_uncommitted_generation() {
        let root = temporary_root("manifest-failure");
        let mut materializer = FixtureMaterializer::new();
        FAIL_MANIFEST_PUBLICATION.with(|fail| fail.set(1));

        let error =
            execute_reconciliation(&root, &plan(None, 1, &["c64"]), limits(), &mut materializer)
                .unwrap_err();

        assert_eq!(error.stage(), "publish-manifest");
        assert!(read_latest_manifest(&root, limits()).is_err());
        assert!(!root.join("systems/c64/1.sqlite3").exists());
        assert!(!root.join("systems/c64/1.nav.lz4b").exists());
        assert_eq!(materializer.commits, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_first_build_adopts_durable_unpublished_shard() {
        let root = temporary_root("resume-unpublished");
        let progress_path = crate::build_progress::path_for_root(&root);
        let contract = crate::build_progress::BuildContract {
            active_manifest_generation: None,
            roots: vec!["/fixture".into()],
            path_mapping: Vec::new(),
            scanner_version: 1,
            profile_version: "1".into(),
            taxonomy_version: "1".into(),
            namespace_backend: "fixture".into(),
            projection_contract: "1".into(),
        };
        let journal = crate::build_progress::BuildProgressJournal::open_or_create(
            &progress_path,
            &contract,
            &[],
        )
        .unwrap()
        .0;
        drop(journal);

        let mut interrupted = FixtureMaterializer::new();
        interrupted.fail_on = Some(system("snes"));
        let error = execute_reconciliation(
            &root,
            &plan(None, 1, &["c64", "snes"]),
            limits(),
            &mut interrupted,
        )
        .unwrap_err();
        assert_eq!(error.stage(), "fixture");
        let sqlite = root.join("systems/c64/1.sqlite3");
        let before = fs::read(&sqlite).unwrap();
        let journal =
            crate::build_progress::BuildProgressJournal::open_for_projection(&progress_path)
                .unwrap();
        assert_eq!(journal.completed_shards().unwrap().len(), 1);
        drop(journal);

        let mut resumed = FixtureMaterializer::new();
        execute_reconciliation(
            &root,
            &plan(None, 1, &["c64", "snes"]),
            limits(),
            &mut resumed,
        )
        .unwrap();
        assert_eq!(fs::read(&sqlite).unwrap(), before);
        assert_eq!(
            read_latest_manifest(&root, limits()).unwrap().systems.len(),
            2
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn post_rename_manifest_failure_preserves_authoritative_artifacts() {
        let root = temporary_root("post-rename-manifest-failure");
        let mut materializer = FixtureMaterializer::new();
        FAIL_MANIFEST_PUBLICATION.with(|fail| fail.set(2));

        let error =
            execute_reconciliation(&root, &plan(None, 1, &["c64"]), limits(), &mut materializer)
                .unwrap_err();

        assert_eq!(error.stage(), "publish-manifest");
        assert_eq!(read_latest_manifest(&root, limits()).unwrap().generation, 1);
        assert!(root.join("systems/c64/1.sqlite3").exists());
        assert!(root.join("systems/c64/1.nav.lz4b").exists());
        assert_eq!(materializer.commits, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ambiguous_manifest_read_failure_preserves_generation() {
        let root = temporary_root("ambiguous-manifest-read");
        fs::create_dir_all(root.join("registry")).unwrap();
        fs::write(root.join("registry/manifest-a.json"), b"not-json").unwrap();

        assert!(!planned_generation_cleanup_is_safe(&root, limits(), 1));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_metadata_error_is_not_treated_as_absence() {
        let root = temporary_root("manifest-metadata-error");
        fs::write(root.join("registry"), b"not-a-directory").unwrap();

        assert!(!manifest_slots_definitely_absent(&root));
        assert!(!planned_generation_cleanup_is_safe(&root, limits(), 1));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_materialized_metadata_never_publishes_artifacts() {
        let root = temporary_root("validation-failure");
        let mut materializer = FixtureMaterializer::new();
        materializer.invalid_on = Some(system("c64"));

        let error = execute_reconciliation(
            &root,
            &plan(None, 1, &["c64", "snes"]),
            limits(),
            &mut materializer,
        )
        .unwrap_err();

        assert_eq!(error.stage(), "materialize");
        assert!(read_latest_manifest(&root, limits()).is_err());
        assert!(!root.join("systems/c64/1.sqlite3").exists());
        assert!(!root.join("systems/snes/1.sqlite3").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn first_build_materializes_bounded_batches_instead_of_the_whole_catalog() {
        let root = temporary_root("bounded-materialization");
        let ids = (0..10)
            .map(|index| format!("fixture-{index:02}"))
            .collect::<Vec<_>>();
        let borrowed = ids.iter().map(String::as_str).collect::<Vec<_>>();
        let mut materializer = StreamingFixtureMaterializer {
            root: root.clone(),
            calls: 0,
        };

        execute_reconciliation(
            &root,
            &plan(None, 1, &borrowed),
            limits(),
            &mut materializer,
        )
        .unwrap();

        assert_eq!(materializer.calls, 10);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn device_catalog_builds_shards_in_tmpfs() {
        let device = Path::new("/media/fat/mister-magik/catalog-v3");
        let games = [game("One")];
        if cfg!(target_os = "linux")
            && tmpfs_available_bytes(Path::new("/tmp"))
                .is_some_and(|available| available >= estimated_shard_build_bytes(&games))
        {
            assert_eq!(
                shard_build_root(device, &games),
                PathBuf::from("/tmp/mister-magik/catalog-v3-build")
            );
        } else {
            assert_eq!(shard_build_root(device, &games), device.join("staging"));
        }
        let host = Path::new("/private/tmp/catalog-v3");
        assert_eq!(shard_build_root(host, &games), host.join("staging"));
    }

    #[test]
    fn shard_build_estimate_grows_with_materialized_rows() {
        let small = estimated_shard_build_bytes(&[game("One")]);
        let large = estimated_shard_build_bytes(&[game("One"), game("Two")]);
        assert!(large > small);
    }

    #[test]
    fn low_tmpfs_capacity_selects_on_media_staging() {
        let device = Path::new("/media/fat/mister-magik/catalog-v3");
        assert_eq!(
            shard_build_root_for_available(device, &[game("One")], Some(0)),
            device.join("staging")
        );
    }

    #[test]
    fn fresh_pipeline_publishes_exactly_seventy_one_systems_with_two_slots() {
        let root = temporary_root("pipeline-71");
        let planned = (0..71)
            .map(|index| PlannedSystem {
                system_id: system(&format!("fixture-{index:02}")),
                action: PlannedSystemAction::Rebuild,
                reasons: vec![ReconcileReason::SourceChanged],
            })
            .collect::<Vec<_>>();
        let borrowed = planned.iter().collect::<Vec<_>>();
        let mut materializer = StreamingFixtureMaterializer {
            root: root.clone(),
            calls: 0,
        };

        let outcome =
            execute_fresh_pipeline(&root, &borrowed, 1, 99, limits(), &mut materializer).unwrap();

        assert_eq!(outcome.completed.len(), 71);
        assert_eq!(outcome.peak_in_flight, 2);
        assert_eq!(outcome.fallbacks, 0);
        let unique = outcome
            .completed
            .iter()
            .map(|shard| shard.system.system_id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), 71);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publisher_failure_leaves_no_authoritative_fresh_manifest() {
        let root = temporary_root("pipeline-publisher-failure");
        let blocked = root.join("systems/c64/1.sqlite3");
        fs::create_dir_all(&blocked).unwrap();
        let mut materializer = FixtureMaterializer::new();

        let error = execute_reconciliation(
            &root,
            &plan(None, 1, &["c64", "snes"]),
            limits(),
            &mut materializer,
        )
        .unwrap_err();

        assert_eq!(error.stage(), "publish-artifact");
        assert!(
            materializer.calls == vec![system("c64")]
                || materializer.calls == vec![system("c64"), system("snes")]
        );
        assert!(read_latest_manifest(&root, limits()).is_err());
        assert!(!root.join("staging").join("reconcile").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn background_replacement_without_a_manifest_uses_the_fresh_pipeline() {
        let root = temporary_root("background-pipeline");
        let _background = crate::cooperative_work::BackgroundScope::enter();
        let mut materializer = SequentialProbeMaterializer {
            root: root.clone(),
            calls: 0,
        };

        execute_reconciliation(
            &root,
            &plan(None, 1, &["c64", "snes"]),
            limits(),
            &mut materializer,
        )
        .unwrap();

        assert_eq!(materializer.calls, 2);
        let journal = crate::build_progress::BuildProgressJournal::open_for_projection(
            &crate::build_progress::path_for_root(&root),
        )
        .unwrap();
        assert_eq!(journal.completed_shards().unwrap().len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fresh_pipeline_manifest_order_is_independent_of_plan_order() {
        let root = temporary_root("pipeline-order");
        let mut materializer = FixtureMaterializer::new();
        let reversed = plan(None, 1, &["snes", "c64"]);

        execute_reconciliation(&root, &reversed, limits(), &mut materializer).unwrap();

        let ids = read_latest_manifest(&root, limits())
            .unwrap()
            .systems
            .into_iter()
            .map(|system| system.system_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![system("c64"), system("snes")]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lifecycle_prepares_each_system_before_publishing_the_manifest_last() {
        let root = temporary_root("lifecycle-order");
        let mut materializer = FixtureMaterializer::new();
        let mut events = Vec::new();

        execute_reconciliation_with_events(
            &root,
            &plan(None, 1, &["c64", "snes"]),
            limits(),
            &mut materializer,
            &mut |event| events.push(event),
        )
        .unwrap();

        assert!(matches!(
            events.last(),
            Some(ReconciliationEvent::ManifestPublished {
                generation: 1,
                rebuilt,
                removed,
            }) if rebuilt == &vec![system("c64"), system("snes")] && removed.is_empty()
        ));
        for expected in ["c64", "snes"] {
            let scanning = events
                .iter()
                .position(|event| {
                    matches!(
                        event,
                        ReconciliationEvent::SystemScanning { system_id }
                            if system_id.as_str() == expected
                    )
                })
                .expect("system scanning event");
            let prepared = events
                .iter()
                .position(|event| {
                    matches!(
                        event,
                        ReconciliationEvent::SystemPrepared {
                            system_id,
                            generation: 1,
                        } if system_id.as_str() == expected
                    )
                })
                .expect("system prepared event");
            assert!(scanning < prepared);
            assert!(prepared < events.len() - 1);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staged_shard_owns_cleanup_until_publication() {
        let root = temporary_root("staged-cleanup");
        let staged = build_and_validate_shard(
            &root,
            1,
            42,
            limits(),
            ShardBuildJob {
                materialized: FixtureMaterializer::new()
                    .materialize(&system("c64"), 1)
                    .unwrap(),
                previous: None,
            },
        )
        .unwrap();
        let staging = staged.staging.clone();
        assert!(staging.join("system.sqlite3").exists());
        assert!(staging.join("system.nav.lz4b").exists());
        drop(staged);
        assert!(!staging.exists());
        fs::remove_dir_all(root).unwrap();
    }

    struct StreamingFixtureMaterializer {
        root: PathBuf,
        calls: usize,
    }

    struct SequentialProbeMaterializer {
        root: PathBuf,
        calls: usize,
    }

    impl ReconciliationMaterializer for SequentialProbeMaterializer {
        fn materialize(
            &mut self,
            system_id: &SystemId,
            _generation: u64,
        ) -> Result<MaterializedSystem, ReconciliationError> {
            if self.calls == 1 {
                assert!(
                    self.root.join("systems/c64/1.sqlite3").exists(),
                    "background replacement started a second build before publication"
                );
            }
            self.calls += 1;
            Ok(MaterializedSystem {
                system_id: system_id.clone(),
                display_title: system_id.as_str().to_string(),
                section: "Fixture".to_string(),
                family: "Fixture".to_string(),
                order: 0,
                producers: vec![ScanUnitId::parse("fixture-root").unwrap()],
                projection_stats: None,
                games: vec![game("One")],
            })
        }

        fn commit_facts(&mut self) -> Result<(), ReconciliationError> {
            Ok(())
        }
    }

    impl ReconciliationMaterializer for StreamingFixtureMaterializer {
        fn materialize(
            &mut self,
            system_id: &SystemId,
            _generation: u64,
        ) -> Result<MaterializedSystem, ReconciliationError> {
            if self.calls == 3 {
                let published = walkdir::WalkDir::new(self.root.join("systems"))
                    .into_iter()
                    .filter_map(Result::ok)
                    .any(|entry| entry.file_name() == "1.sqlite3");
                assert!(
                    published,
                    "fourth system was materialized before the first bounded batch was written"
                );
            }
            self.calls += 1;
            Ok(MaterializedSystem {
                system_id: system_id.clone(),
                display_title: system_id.as_str().to_string(),
                section: "Fixture".to_string(),
                family: "Fixture".to_string(),
                order: 0,
                producers: vec![ScanUnitId::parse("fixture-root").unwrap()],
                projection_stats: None,
                games: vec![game("One")],
            })
        }

        fn commit_facts(&mut self) -> Result<(), ReconciliationError> {
            Ok(())
        }
    }

    struct FixtureMaterializer {
        games: BTreeMap<&'static str, Vec<SystemGame>>,
        calls: Vec<SystemId>,
        commits: usize,
        fail_on: Option<SystemId>,
        invalid_on: Option<SystemId>,
    }

    impl FixtureMaterializer {
        fn new() -> Self {
            Self {
                games: BTreeMap::from([("c64", vec![game("One")]), ("snes", vec![game("One")])]),
                calls: Vec::new(),
                commits: 0,
                fail_on: None,
                invalid_on: None,
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
            let mut materialized = MaterializedSystem {
                system_id: system_id.clone(),
                display_title: system_id.as_str().to_ascii_uppercase(),
                section: "Fixture".to_string(),
                family: "Fixture".to_string(),
                order: 0,
                producers: vec![
                    ScanUnitId::parse(&format!("{}-root", system_id.as_str())).unwrap(),
                ],
                projection_stats: None,
                games: self.games.get(system_id.as_str()).unwrap().clone(),
            };
            if self.invalid_on.as_ref() == Some(system_id) {
                materialized.display_title.clear();
            }
            Ok(materialized)
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
            ..SystemGame::default()
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
