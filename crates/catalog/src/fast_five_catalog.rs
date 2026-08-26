// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Independent five-system catalog interchange and publication.
//!
//! This deliberately has no dependency on the whole-card scanner, catalog
//! database, resume journal, or catalog state. System-specific fast builders
//! produce final UI rows, and this module publishes only the immutable shards
//! and manifest consumed by the launcher.

use crate::catalog_classify::SystemId;
#[cfg(feature = "builder")]
use crate::catalog_classify::{LauncherSection, system_definition};
#[cfg(feature = "builder")]
use crate::shard_registry::CatalogManifest;
use crate::shard_registry::{RegistryLimits, read_latest_manifest_lazy};
use crate::system_shard::SystemGame;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::Path;

pub const FAST_FIVE_SNAPSHOT_SCHEMA: &str = "mister-magik-fast-five-snapshot-v1";
pub const FAST_FIVE_SYSTEM_IDS: [&str; 5] = ["amiga", "arcade", "c64", "dos", "x68000"];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastFiveSnapshot {
    pub schema: String,
    pub source_fingerprint: String,
    pub systems: Vec<FastFiveSystem>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastFiveSystem {
    pub system_id: String,
    pub display_title: String,
    pub games: Vec<SystemGame>,
}

impl FastFiveSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != FAST_FIVE_SNAPSHOT_SCHEMA {
            return Err(format!("unsupported fast-five schema {}", self.schema));
        }
        validate_fingerprint(&self.source_fingerprint)?;
        let expected = FAST_FIVE_SYSTEM_IDS.into_iter().collect::<BTreeSet<_>>();
        let actual = self
            .systems
            .iter()
            .map(|system| system.system_id.as_str())
            .collect::<BTreeSet<_>>();
        if actual != expected || self.systems.len() != expected.len() {
            return Err(format!(
                "fast-five snapshot systems differ: expected={expected:?} actual={actual:?}"
            ));
        }
        for system in &self.systems {
            SystemId::parse(&system.system_id)
                .map_err(|error| format!("invalid system {}: {error}", system.system_id))?;
            if system.display_title.trim().is_empty() {
                return Err(format!("{} display title is empty", system.system_id));
            }
            let mut stable_keys = BTreeSet::new();
            for game in &system.games {
                if game.title.trim().is_empty() || game.launch_ref.trim().is_empty() {
                    return Err(format!(
                        "{} has an empty title or launch reference",
                        system.system_id
                    ));
                }
                if !game
                    .stable_key
                    .starts_with(&format!("{}\u{1f}", system.system_id))
                {
                    return Err(format!(
                        "{} row has a foreign stable key: {}",
                        system.system_id, game.stable_key
                    ));
                }
                if !stable_keys.insert(game.stable_key.as_str()) {
                    return Err(format!(
                        "{} contains duplicate stable key {}",
                        system.system_id, game.stable_key
                    ));
                }
                if let Some(plan) = &game.launch_plan {
                    if plan.launch_ref != game.launch_ref || plan.system_id != system.system_id {
                        return Err(format!(
                            "{} launch plan is not bound to {}",
                            system.system_id, game.stable_key
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn game_count(&self) -> usize {
        self.systems.iter().map(|system| system.games.len()).sum()
    }
}

pub fn registry_fingerprint(storage_root: &Path, limits: RegistryLimits) -> Result<String, String> {
    let manifest = read_latest_manifest_lazy(storage_root, limits)
        .map_err(|error| format!("read fast-five manifest: {error}"))?;
    let mut digest = Sha256::new();
    digest.update(FAST_FIVE_SNAPSHOT_SCHEMA.as_bytes());
    digest.update(manifest.generation.to_le_bytes());
    for system in &manifest.systems {
        digest.update(system.system_id.as_str().as_bytes());
        digest.update([0]);
        digest.update(system.display_title.as_bytes());
        digest.update([0]);
        digest.update(system.active.generation.to_le_bytes());
        digest.update(system.active.games.to_le_bytes());
        digest.update(system.active.sqlite_hash.as_bytes());
        digest.update(system.active.navigation_hash.as_bytes());
        if let Some(navpack) = &system.active.navpack {
            digest.update(navpack.hash.as_bytes());
        }
    }
    Ok(hex(&digest.finalize()))
}

#[cfg(feature = "builder")]
pub fn replace_arcade_from_active(
    mut snapshot: FastFiveSnapshot,
    active_bytes: &[u8],
) -> Result<FastFiveSnapshot, String> {
    let active = crate::arcade_catalog_prototype_model::decode_active(active_bytes)?;
    let previous_fingerprint = snapshot.source_fingerprint.clone();
    let arcade = snapshot
        .systems
        .iter_mut()
        .find(|system| system.system_id == "arcade")
        .ok_or_else(|| "fast-five snapshot has no Arcade system".to_string())?;
    let previous = arcade
        .games
        .iter()
        .map(|game| (game.launch_ref.as_str(), game))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut games = active
        .records
        .into_iter()
        .map(|record| {
            let old = previous.get(record.path.as_str()).copied();
            SystemGame {
                stable_key: format!("arcade\u{1f}{}\u{1f}{}", record.title, record.path),
                title: record.title,
                launch_ref: record.path,
                preview_archive_path: old
                    .map(|game| game.preview_archive_path.clone())
                    .unwrap_or_default(),
                preview_asset_key: old
                    .map(|game| game.preview_asset_key.clone())
                    .unwrap_or_default(),
                has_preview: old.is_some_and(|game| game.has_preview),
                year: record.year,
                manufacturer: record.manufacturer,
                category: record.category,
                players: record.players,
                control: record.control,
                is_new: old.is_some_and(|game| game.is_new),
                launch_plan: None,
            }
        })
        .collect::<Vec<_>>();
    games.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| left.stable_key.cmp(&right.stable_key))
    });
    arcade.games = games;
    let mut digest = Sha256::new();
    digest.update(previous_fingerprint.as_bytes());
    digest.update(active.source_sha256);
    for game in &arcade.games {
        digest.update(game.stable_key.as_bytes());
        digest.update([0]);
    }
    snapshot.source_fingerprint = hex(&digest.finalize());
    snapshot.validate()?;
    Ok(snapshot)
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug, Serialize)]
pub struct FastFivePublishReport {
    pub generation: u64,
    pub systems: usize,
    pub games: usize,
    pub elapsed_us: u64,
    pub registry_fingerprint: String,
    pub system_builds: Vec<FastFiveSystemBuildReport>,
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug, Serialize)]
pub struct FastFiveSystemBuildReport {
    pub system_id: String,
    pub games: usize,
    pub elapsed_us: u64,
}

#[cfg(feature = "builder")]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum C64ArtifactExperimentProfile {
    MediaImmediate,
    TmpfsImmediate,
    TmpfsDeferred,
    TmpfsDeferredMemory,
    TmpfsImmediateNoOptimize,
    TmpfsImmediateFtsColumn,
    TmpfsImmediateFtsColumnNoOptimize,
}

#[cfg(feature = "builder")]
impl C64ArtifactExperimentProfile {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "media-immediate" => Ok(Self::MediaImmediate),
            "tmpfs-immediate" => Ok(Self::TmpfsImmediate),
            "tmpfs-deferred" => Ok(Self::TmpfsDeferred),
            "tmpfs-deferred-memory" => Ok(Self::TmpfsDeferredMemory),
            "tmpfs-immediate-no-optimize" => Ok(Self::TmpfsImmediateNoOptimize),
            "tmpfs-immediate-fts-column" => Ok(Self::TmpfsImmediateFtsColumn),
            "tmpfs-immediate-fts-column-no-optimize" => Ok(Self::TmpfsImmediateFtsColumnNoOptimize),
            _ => Err(format!("unknown C64 artifact experiment profile {value}")),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MediaImmediate => "media-immediate",
            Self::TmpfsImmediate => "tmpfs-immediate",
            Self::TmpfsDeferred => "tmpfs-deferred",
            Self::TmpfsDeferredMemory => "tmpfs-deferred-memory",
            Self::TmpfsImmediateNoOptimize => "tmpfs-immediate-no-optimize",
            Self::TmpfsImmediateFtsColumn => "tmpfs-immediate-fts-column",
            Self::TmpfsImmediateFtsColumnNoOptimize => "tmpfs-immediate-fts-column-no-optimize",
        }
    }
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug, Serialize)]
pub struct C64ArtifactExperimentReport {
    pub status: &'static str,
    pub profile: C64ArtifactExperimentProfile,
    pub games: usize,
    pub build_us: u64,
    pub publish_us: u64,
    pub published_validate_us: u64,
    pub elapsed_us: u64,
    pub sqlite_bytes: u64,
    pub navigation_bytes: u64,
    pub navpack_bytes: u64,
    pub search_probe_us: u64,
    pub search_fingerprint: String,
}

#[cfg(feature = "builder")]
fn elapsed_us(started: std::time::Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

#[cfg(feature = "builder")]
fn c64_search_probe(sqlite_path: &Path, games: &[SystemGame]) -> Result<(u64, String), String> {
    use crate::persisted_search::search_system_shard;
    use std::time::Instant;

    let mut queries = BTreeSet::new();
    let stride = (games.len() / 8).max(1);
    for game in games.iter().step_by(stride).take(8) {
        let Some(word) = game
            .title
            .split(|character: char| !character.is_alphanumeric())
            .find(|word| word.chars().count() >= 3)
        else {
            continue;
        };
        let word = word.to_lowercase();
        queries.insert(word.clone());
        queries.insert(word.chars().take(3).collect::<String>());
    }
    let started = Instant::now();
    let mut fingerprint = Sha256::new();
    for query in queries {
        let result = search_system_shard(sqlite_path, &query)
            .map_err(|error| format!("probe C64 search {query}: {error}"))?;
        fingerprint.update((query.len() as u64).to_le_bytes());
        fingerprint.update(query.as_bytes());
        fingerprint.update((result.matches.len() as u64).to_le_bytes());
        for matched in result.matches {
            fingerprint.update((matched.ordinal as u64).to_le_bytes());
            fingerprint.update(matched.rank.to_bits().to_le_bytes());
        }
        if let Some(autocomplete) = result.autocomplete {
            fingerprint.update([1]);
            fingerprint.update((autocomplete.word.len() as u64).to_le_bytes());
            fingerprint.update(autocomplete.word.as_bytes());
            fingerprint.update([autocomplete.source_rank]);
            fingerprint.update(autocomplete.score.to_le_bytes());
        } else {
            fingerprint.update([0]);
        }
    }
    Ok((elapsed_us(started), hex::encode(fingerprint.finalize())))
}

#[cfg(feature = "builder")]
pub fn run_c64_artifact_experiment(
    storage_root: &Path,
    scratch_root: &Path,
    snapshot: &FastFiveSnapshot,
    profile: C64ArtifactExperimentProfile,
    limits: RegistryLimits,
) -> Result<C64ArtifactExperimentReport, String> {
    use crate::shard_registry::{
        publish_prevalidated_system_artifacts_deferred, publish_system_artifacts,
        sync_artifact_batch,
    };
    use crate::system_shard::{
        ShardDurability, ShardSearchTuning, ShardSqliteTuning, SystemShardData, open_system_shard,
        write_system_shard_with_options,
    };
    use std::fs;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    let started = Instant::now();
    snapshot.validate()?;
    let source = snapshot
        .systems
        .iter()
        .find(|system| system.system_id == "c64")
        .ok_or("fast-five snapshot has no C64 system")?;
    let system_id = SystemId::parse("c64").map_err(|error| error.to_string())?;
    let (staging_parent, durability, sqlite_tuning, search_tuning) = match profile {
        C64ArtifactExperimentProfile::MediaImmediate => (
            storage_root.join("staging"),
            ShardDurability::Immediate,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::FullOptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsImmediate => (
            scratch_root.to_path_buf(),
            ShardDurability::Immediate,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::FullOptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsDeferred => (
            scratch_root.to_path_buf(),
            ShardDurability::Deferred,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::FullOptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsDeferredMemory => (
            scratch_root.to_path_buf(),
            ShardDurability::Deferred,
            ShardSqliteTuning::MemoryHeavy,
            ShardSearchTuning::FullOptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsImmediateNoOptimize => (
            scratch_root.to_path_buf(),
            ShardDurability::Immediate,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::FullUnoptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsImmediateFtsColumn => (
            scratch_root.to_path_buf(),
            ShardDurability::Immediate,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::ColumnOptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsImmediateFtsColumnNoOptimize => (
            scratch_root.to_path_buf(),
            ShardDurability::Immediate,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::ColumnUnoptimized,
        ),
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock predates Unix epoch".to_string())?
        .as_nanos();
    let staging = staging_parent.join(format!(
        "c64-artifact-{}-{nonce}-{}",
        profile.as_str(),
        std::process::id()
    ));
    fs::create_dir_all(&staging)
        .map_err(|error| format!("create C64 experiment staging: {error}"))?;
    fs::create_dir_all(storage_root)
        .map_err(|error| format!("create C64 experiment root: {error}"))?;
    let sqlite = staging.join("system.sqlite3");
    let navigation = staging.join("system.nav.lz4b");
    let build_started = Instant::now();
    let staged = write_system_shard_with_options(
        &sqlite,
        &navigation,
        SystemShardData {
            system_id: system_id.clone(),
            generation: 1,
            projection_stats: None,
            games: source.games.clone(),
        },
        limits.shard,
        durability,
        sqlite_tuning,
        search_tuning,
    )
    .map_err(|error| format!("build C64 experiment shard: {error}"))?;
    let build_us = elapsed_us(build_started);
    if staged.games != source.games {
        return Err("staged C64 experiment rows differ from the snapshot".to_string());
    }
    let publish_started = Instant::now();
    let published = if profile == C64ArtifactExperimentProfile::MediaImmediate {
        publish_system_artifacts(
            storage_root,
            &sqlite,
            &navigation,
            &system_id,
            1,
            source.games.len() as u64,
            limits,
        )
        .map_err(|error| format!("publish C64 experiment shard: {error}"))?
    } else {
        let publication = publish_prevalidated_system_artifacts_deferred(
            storage_root,
            &sqlite,
            &navigation,
            &system_id,
            1,
            source.games.len() as u64,
            limits,
        )
        .map_err(|error| format!("copy C64 experiment shard: {error}"))?;
        sync_artifact_batch(storage_root)
            .map_err(|error| format!("sync C64 experiment shard: {error}"))?;
        publication.generation
    };
    let publish_us = elapsed_us(publish_started);
    let validate_started = Instant::now();
    let loaded = open_system_shard(
        &storage_root.join(&published.sqlite_path),
        &storage_root.join(&published.navigation_path),
        &system_id,
        1,
        limits.shard,
    )
    .map_err(|error| format!("validate published C64 experiment shard: {error}"))?;
    let published_validate_us = elapsed_us(validate_started);
    if loaded.games != source.games {
        return Err("published C64 experiment rows differ from the snapshot".to_string());
    }
    let (search_probe_us, search_fingerprint) =
        c64_search_probe(&storage_root.join(&published.sqlite_path), &source.games)?;
    let _ = fs::remove_dir_all(&staging);
    Ok(C64ArtifactExperimentReport {
        status: "exact",
        profile,
        games: source.games.len(),
        build_us,
        publish_us,
        published_validate_us,
        elapsed_us: elapsed_us(started),
        sqlite_bytes: published.sqlite_bytes,
        navigation_bytes: published.navigation_bytes,
        navpack_bytes: published.navpack.map_or(0, |navpack| navpack.bytes),
        search_probe_us,
        search_fingerprint,
    })
}

#[cfg(feature = "builder")]
pub fn publish_snapshot(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
    limits: RegistryLimits,
) -> Result<FastFivePublishReport, String> {
    use crate::catalog_domain::ScanUnitId;
    use crate::catalog_format::CatalogFormatDescriptor;
    use crate::shard_registry::{
        ManifestSystem, garbage_collect_unreferenced, manifest_slots_present, publish_manifest,
        publish_system_artifacts,
    };
    use crate::system_shard::{SystemShardData, write_system_shard};
    use std::fs;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    let started = Instant::now();
    snapshot.validate()?;
    fs::create_dir_all(storage_root)
        .map_err(|error| format!("create fast-five root {}: {error}", storage_root.display()))?;
    let current = match read_latest_manifest_lazy(storage_root, limits) {
        Ok(manifest) => Some(manifest),
        Err(_) if manifest_slots_present(storage_root) => {
            return Err("fast-five manifest slots exist but neither is valid".to_string());
        }
        Err(_) => None,
    };
    garbage_collect_unreferenced(
        storage_root,
        current.as_ref().unwrap_or(&CatalogManifest {
            format: None,
            generation: 0,
            systems: Vec::new(),
        }),
    )
    .map_err(|error| format!("collect interrupted fast-five artifacts: {error}"))?;
    let generation = current.as_ref().map_or(Ok(1), |manifest| {
        manifest
            .generation
            .checked_add(1)
            .ok_or_else(|| "fast-five generation overflow".to_string())
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock predates Unix epoch".to_string())?
        .as_nanos();
    let mut manifest_systems = Vec::with_capacity(snapshot.systems.len());
    let mut system_builds = Vec::with_capacity(snapshot.systems.len());
    for source in &snapshot.systems {
        let system_started = Instant::now();
        let system_id = SystemId::parse(&source.system_id)
            .map_err(|error| format!("invalid system {}: {error}", source.system_id))?;
        let staging = storage_root.join("staging").join(format!(
            "fast-five-{}-{generation}-{nonce}-{}",
            std::process::id(),
            source.system_id
        ));
        fs::create_dir_all(&staging)
            .map_err(|error| format!("create {} staging: {error}", source.system_id))?;
        let sqlite = staging.join("system.sqlite3");
        let navigation = staging.join("system.nav.lz4b");
        write_system_shard(
            &sqlite,
            &navigation,
            &SystemShardData {
                system_id: system_id.clone(),
                generation,
                projection_stats: None,
                games: source.games.clone(),
            },
            limits.shard,
        )
        .map_err(|error| format!("write {} shard: {error}", source.system_id))?;
        let active = publish_system_artifacts(
            storage_root,
            &sqlite,
            &navigation,
            &system_id,
            generation,
            source.games.len() as u64,
            limits,
        )
        .map_err(|error| format!("publish {} shard: {error}", source.system_id))?;
        let _ = fs::remove_dir(staging);
        let previous = current.as_ref().and_then(|manifest| {
            manifest
                .systems
                .iter()
                .find(|system| system.system_id == system_id)
                .map(|system| system.active.clone())
        });
        let definition = system_definition(&source.system_id);
        manifest_systems.push(ManifestSystem {
            system_id,
            display_title: source.display_title.clone(),
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
                ScanUnitId::parse(&format!("fast-five-{}", source.system_id))
                    .map_err(|error| format!("create producer id: {error}"))?,
            ],
            active,
            previous,
        });
        system_builds.push(FastFiveSystemBuildReport {
            system_id: source.system_id.clone(),
            games: source.games.len(),
            elapsed_us: system_started
                .elapsed()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX),
        });
    }
    manifest_systems.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    let manifest = CatalogManifest {
        format: Some(CatalogFormatDescriptor::current()),
        generation,
        systems: manifest_systems,
    };
    publish_manifest(storage_root, &manifest, limits)
        .map_err(|error| format!("publish fast-five manifest: {error}"))?;
    garbage_collect_unreferenced(storage_root, &manifest)
        .map_err(|error| format!("collect fast-five artifacts: {error}"))?;
    Ok(FastFivePublishReport {
        generation,
        systems: snapshot.systems.len(),
        games: snapshot.game_count(),
        elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        registry_fingerprint: registry_fingerprint(storage_root, limits)?,
        system_builds,
    })
}

#[cfg(feature = "builder")]
pub fn snapshot_reference(
    storage_root: &Path,
    limits: RegistryLimits,
) -> Result<FastFiveSnapshot, String> {
    use crate::lazy_sharded_reader::LazyShardedCatalogReader;
    use crate::sharded_catalog::CatalogReader;
    use crate::system_shard::SystemLaunchPlan;

    let reader = LazyShardedCatalogReader::open(storage_root, limits)
        .map_err(|error| format!("open reference catalog: {error}"))?;
    let registry = reader
        .open_registry()
        .map_err(|error| format!("open reference registry: {error}"))?;
    let titles = registry
        .systems()
        .iter()
        .map(|summary| {
            (
                summary.system_id.as_str().to_string(),
                summary.display_title.clone(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut systems = Vec::with_capacity(FAST_FIVE_SYSTEM_IDS.len());
    for id in FAST_FIVE_SYSTEM_IDS {
        let system_id = SystemId::parse(id).map_err(|error| error.to_string())?;
        let catalog = reader
            .open_system(&system_id)
            .map_err(|error| format!("open reference {id}: {error}"))?;
        let games = catalog
            .games()
            .iter()
            .map(|game| SystemGame {
                stable_key: game.stable_key.clone(),
                title: game.title.clone(),
                launch_ref: game.launch_ref.clone(),
                preview_archive_path: game.preview_archive_path.clone(),
                preview_asset_key: game.preview_asset_key.clone(),
                has_preview: game.has_preview,
                year: game.year,
                manufacturer: game.manufacturer.clone(),
                category: game.category.clone(),
                players: game.players,
                control: game.control.clone(),
                is_new: game.is_new,
                launch_plan: game.launch_plan.as_ref().map(|plan| SystemLaunchPlan {
                    launch_ref: plan.launch_ref.clone(),
                    title: plan.title.clone(),
                    system_id: plan.system_id.clone(),
                    core_path: plan.core_path.clone(),
                    payload_path: plan.payload_path.clone(),
                    mount_kind: plan.mount_kind.clone(),
                    mount_index: plan.mount_index,
                    delay_secs: plan.delay_secs,
                }),
            })
            .collect();
        systems.push(FastFiveSystem {
            system_id: id.to_string(),
            display_title: titles.get(id).cloned().unwrap_or_else(|| id.to_string()),
            games,
        });
    }
    let snapshot = FastFiveSnapshot {
        schema: FAST_FIVE_SNAPSHOT_SCHEMA.to_string(),
        source_fingerprint: registry_fingerprint(storage_root, limits)?,
        systems,
    };
    snapshot.validate()?;
    Ok(snapshot)
}

#[cfg(feature = "builder")]
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

fn validate_fingerprint(value: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("fast-five source fingerprint is not SHA-256".to_string())
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_rejects_missing_system() {
        let snapshot = FastFiveSnapshot {
            schema: FAST_FIVE_SNAPSHOT_SCHEMA.to_string(),
            source_fingerprint: "0".repeat(64),
            systems: Vec::new(),
        };
        assert!(snapshot.validate().is_err());
    }
}

#[cfg(all(test, feature = "builder"))]
mod builder_tests {
    use super::*;
    use crate::lazy_sharded_reader::LazyShardedCatalogReader;
    use crate::sharded_catalog::CatalogReader;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn publishes_only_the_five_expected_systems() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-fast-five-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let snapshot = FastFiveSnapshot {
            schema: FAST_FIVE_SNAPSHOT_SCHEMA.to_string(),
            source_fingerprint: "0".repeat(64),
            systems: FAST_FIVE_SYSTEM_IDS
                .into_iter()
                .map(|system_id| FastFiveSystem {
                    system_id: system_id.to_string(),
                    display_title: system_id.to_string(),
                    games: Vec::new(),
                })
                .collect(),
        };
        let limits = crate::shard_registry::production_registry_limits();
        let report = publish_snapshot(&root, &snapshot, limits).unwrap();
        assert_eq!(report.systems, 5);
        let reader = LazyShardedCatalogReader::open(&root, limits).unwrap();
        let registry = reader.open_registry().unwrap();
        assert_eq!(registry.systems().len(), 5);
        assert_eq!(registry_fingerprint(&root, limits).unwrap().len(), 64);
        fs::remove_dir_all(root).unwrap();
    }
}
