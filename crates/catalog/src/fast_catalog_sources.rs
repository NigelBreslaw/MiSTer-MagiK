// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Independent source adapters for the fast nine-system catalog.
//!
//! These adapters consume installed files and the dedicated Arcade metadata
//! contract directly. They never read retired catalog artifacts or scanner state.

use crate::arcade_catalog::{
    ArcadeCatalog, ArcadeGameEntry, GameSystemEntry, StructuredLaunchPlan,
};
use crate::catalog_scan::FoundFile;
use crate::fast_five_catalog::{
    FAST_FIVE_SNAPSHOT_SCHEMA, FastFiveGameVariant, FastFiveSnapshot, FastFiveSystem,
    FastFiveVariantRelation, collapse_c64_cross_source_variants,
};
use crate::generic_system_catalog::{
    GenericSourceWatchObservations, discover_generic_systems_from_plan_excluding_with_progress,
    inventory_prepared_extension_under_named_roots, rebuild_installed_generic_system,
};
use crate::launch_profiles::{CatalogScanPlan, CollectionListing, LaunchProfile, ProfileSet};
use crate::media_identity::ScreenshotAssetId;
use crate::mra_header::{PrimaryRomRequirement, RomNamespace};
use crate::prepared_collections::{
    PreparedPayloadIndex, observed_oneload64_path_is_valid, validate_prepared_launch_path,
};
use crate::system_shard::{SystemGame, SystemLaunchPlan};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const FAST_SOURCE_ADAPTER_VERSION: u32 = 12;
const PREPARED_SYSTEM_IDS: [&str; 5] = ["arcade", "amiga", "c64", "dos", "x68000"];

#[derive(Clone, Debug, Serialize)]
pub struct FastSourceBuildReport {
    pub elapsed_us: u64,
    pub phases: FastSourcePhaseReport,
    pub systems: Vec<FastSourceSystemReport>,
    pub legacy_inputs: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FastSourcePhaseReport {
    pub prepared_systems_us: u64,
    pub profile_discovery_us: u64,
    pub system_planning_us: u64,
    pub generic_systems_us: u64,
    pub preview_identity_us: u64,
    pub merge_us: u64,
    pub fingerprint_us: u64,
    pub validation_us: u64,
    pub residual_us: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FastSourceSystemReport {
    pub system_id: String,
    pub files_visited: usize,
    pub games: usize,
    pub invalid: usize,
    pub elapsed_us: u64,
    pub helper_hits: usize,
    pub fallback_validations: usize,
}

pub fn build_independent_fast_snapshot(
    storage_root: &Path,
) -> Result<(FastFiveSnapshot, FastSourceBuildReport), String> {
    build_independent_fast_snapshot_with_progress(storage_root, |_| {})
}

pub fn build_independent_fast_snapshot_with_progress(
    storage_root: &Path,
    mut system_complete: impl FnMut(&str),
) -> Result<(FastFiveSnapshot, FastSourceBuildReport), String> {
    let build = build_independent_fast_snapshot_for_refresh_with_progress(
        storage_root,
        |_| {},
        |system| system_complete(&system.system_id),
    )?;
    Ok((build.snapshot, build.report))
}

pub(crate) struct FastSourceRefreshBuild {
    pub snapshot: FastFiveSnapshot,
    pub report: FastSourceBuildReport,
    pub profiles: Vec<LaunchProfile>,
    pub generic_watch_observations: BTreeMap<String, GenericSourceWatchObservations>,
}

pub(crate) fn build_independent_fast_snapshot_for_refresh_with_progress(
    storage_root: &Path,
    mut plan_ready: impl FnMut(&[String]),
    mut system_complete: impl FnMut(&FastFiveSystem),
) -> Result<FastSourceRefreshBuild, String> {
    let started = Instant::now();
    let mut systems = BTreeMap::new();
    let mut reports = BTreeMap::new();
    let mut prepared_watch_observations = BTreeMap::new();
    build_and_record_prepared_system(
        storage_root,
        "arcade",
        &mut systems,
        &mut reports,
        &mut prepared_watch_observations,
        &mut system_complete,
    )?;
    let roots = [storage_root.display().to_string()];
    let phase_started = Instant::now();
    let plan = CatalogScanPlan::try_for_roots(&roots)?;
    let profile_discovery_us = elapsed_us(phase_started);
    let (mut generic_systems, generic, profiles, mut generic_watch_observations) =
        discover_generic_systems_from_plan_excluding_with_progress(
            storage_root,
            &plan,
            &PREPARED_SYSTEM_IDS,
            |_| {},
        )?;
    let generic_systems_us = generic.elapsed_us;
    let phase_started = Instant::now();
    let planned_system_ids = discover_independent_system_ids_from_profiles(storage_root, &profiles);
    plan_ready(&planned_system_ids);
    let system_planning_us = elapsed_us(phase_started);
    for system_id in PREPARED_SYSTEM_IDS
        .iter()
        .copied()
        .filter(|system_id| *system_id != "arcade")
    {
        build_and_record_prepared_system(
            storage_root,
            system_id,
            &mut systems,
            &mut reports,
            &mut prepared_watch_observations,
            &mut system_complete,
        )?;
    }
    generic_watch_observations.extend(prepared_watch_observations);
    for system in &generic_systems {
        system_complete(system);
    }
    let phase_started = Instant::now();
    enrich_fast_preview_identities(storage_root, &mut generic_systems);
    let preview_identity_us = elapsed_us(phase_started);
    let phase_started = Instant::now();
    systems.extend(
        generic_systems
            .into_iter()
            .map(|system| (system.system_id.clone(), system)),
    );
    reports.extend(generic.systems.into_iter().map(|system| {
        (
            system.system_id.clone(),
            FastSourceSystemReport {
                system_id: system.system_id,
                files_visited: system.files,
                games: system.games,
                invalid: system.read_errors.saturating_add(system.archive_errors),
                elapsed_us: system.elapsed_us,
                helper_hits: 0,
                fallback_validations: 0,
            },
        )
    }));
    let merge_us = elapsed_us(phase_started);
    let prepared_systems_us: u64 = reports
        .values()
        .filter(|report| PREPARED_SYSTEM_IDS.contains(&report.system_id.as_str()))
        .map(|report| report.elapsed_us)
        .sum();
    let phase_started = Instant::now();
    let source_fingerprint = fingerprint_systems(&systems.values().cloned().collect::<Vec<_>>())?;
    let fingerprint_us = elapsed_us(phase_started);
    let snapshot = FastFiveSnapshot {
        schema: FAST_FIVE_SNAPSHOT_SCHEMA.to_string(),
        source_fingerprint,
        systems: systems.into_values().collect(),
    };
    let phase_started = Instant::now();
    snapshot.validate()?;
    let validation_us = elapsed_us(phase_started);
    let total_us = elapsed_us(started);
    let accounted_us = prepared_systems_us
        .saturating_add(profile_discovery_us)
        .saturating_add(system_planning_us)
        .saturating_add(generic_systems_us)
        .saturating_add(preview_identity_us)
        .saturating_add(merge_us)
        .saturating_add(fingerprint_us)
        .saturating_add(validation_us);
    let phases = FastSourcePhaseReport {
        prepared_systems_us,
        profile_discovery_us,
        system_planning_us,
        generic_systems_us,
        preview_identity_us,
        merge_us,
        fingerprint_us,
        validation_us,
        residual_us: total_us.saturating_sub(accounted_us),
    };
    crate::catalog_logln!(
        "fast_catalog_source_phase_tsv\ttotal_us={}\tprepared_us={}\tprofiles_us={}\tplanning_us={}\tgeneric_us={}\tpreview_identity_us={}\tmerge_us={}\tfingerprint_us={}\tvalidation_us={}\tresidual_us={}",
        total_us,
        phases.prepared_systems_us,
        phases.profile_discovery_us,
        phases.system_planning_us,
        phases.generic_systems_us,
        phases.preview_identity_us,
        phases.merge_us,
        phases.fingerprint_us,
        phases.validation_us,
        phases.residual_us,
    );
    Ok(FastSourceRefreshBuild {
        snapshot,
        report: FastSourceBuildReport {
            elapsed_us: total_us,
            phases,
            systems: reports.into_values().collect(),
            legacy_inputs: 0,
        },
        profiles,
        generic_watch_observations,
    })
}

fn build_and_record_prepared_system(
    storage_root: &Path,
    system_id: &str,
    systems: &mut BTreeMap<String, FastFiveSystem>,
    reports: &mut BTreeMap<String, FastSourceSystemReport>,
    watch_observations: &mut BTreeMap<String, GenericSourceWatchObservations>,
    system_complete: &mut impl FnMut(&FastFiveSystem),
) -> Result<(), String> {
    let system_started = Instant::now();
    let (mut system, mut report, watch) = build_prepared_system(storage_root, system_id, true)?;
    if system_id == "c64" {
        collapse_c64_cross_source_variants(&mut system);
    }
    enrich_fast_preview_identities(storage_root, std::slice::from_mut(&mut system));
    report.elapsed_us = elapsed_us(system_started);
    report.games = system.games.len();
    crate::catalog_logln!(
        "fast_catalog_source_tsv\tadapter=prepared\tsystem={}\telapsed_us={}\tfiles={}\tgames={}\tinvalid={}\thelper_hits={}\tfallback_validations={}",
        report.system_id,
        report.elapsed_us,
        report.files_visited,
        report.games,
        report.invalid,
        report.helper_hits,
        report.fallback_validations,
    );
    if !system.games.is_empty() || !system.variants.is_empty() {
        if let Some(watch) = watch {
            watch_observations.insert(system_id.to_string(), watch);
        }
        system_complete(&system);
        systems.insert(system_id.to_string(), system);
        reports.insert(system_id.to_string(), report);
    }
    Ok(())
}

pub fn rebuild_independent_system(
    storage_root: &Path,
    _snapshot: &FastFiveSnapshot,
    system_id: &str,
) -> Result<Option<(FastFiveSystem, FastSourceSystemReport)>, String> {
    let started = Instant::now();
    let prepared = PREPARED_SYSTEM_IDS
        .contains(&system_id)
        .then(|| build_prepared_system(storage_root, system_id, false))
        .transpose()?;
    let generic = if prepared.is_some() {
        None
    } else {
        rebuild_installed_generic_system(storage_root, system_id)?
    };
    let (mut system, mut report) = match (prepared, generic) {
        (Some((mut prepared, mut report, _)), Some((generic, generic_report))) => {
            merge_system_rows(&mut prepared, generic);
            merge_source_report(
                &mut report,
                &FastSourceSystemReport {
                    system_id: generic_report.system_id,
                    files_visited: generic_report.files,
                    games: generic_report.games,
                    invalid: generic_report
                        .read_errors
                        .saturating_add(generic_report.archive_errors),
                    elapsed_us: generic_report.elapsed_us,
                    helper_hits: 0,
                    fallback_validations: 0,
                },
            );
            (prepared, report)
        }
        (Some((system, report, _)), None) => (system, report),
        (None, Some((system, generic_report))) => (
            system,
            FastSourceSystemReport {
                system_id: generic_report.system_id,
                files_visited: generic_report.files,
                games: generic_report.games,
                invalid: generic_report
                    .read_errors
                    .saturating_add(generic_report.archive_errors),
                elapsed_us: generic_report.elapsed_us,
                helper_hits: 0,
                fallback_validations: 0,
            },
        ),
        (None, None) => return Ok(None),
    };
    if system_id == "c64" {
        collapse_c64_cross_source_variants(&mut system);
    }
    enrich_fast_preview_identities(storage_root, std::slice::from_mut(&mut system));
    report.elapsed_us = elapsed_us(started);
    report.games = system.games.len();
    if system.games.is_empty() && system.variants.is_empty() {
        Ok(None)
    } else {
        Ok(Some((system, report)))
    }
}

pub fn discover_independent_system_ids(storage_root: &Path) -> Result<Vec<String>, String> {
    let roots = [storage_root.display().to_string()];
    let profiles = ProfileSet::try_for_roots(&roots)?.into_profiles();
    Ok(discover_independent_system_ids_from_profiles(
        storage_root,
        &profiles,
    ))
}

fn discover_independent_system_ids_from_profiles(
    storage_root: &Path,
    profiles: &[LaunchProfile],
) -> Vec<String> {
    let mut systems = profiles
        .iter()
        .filter(|profile| {
            profile
                .game_dirs
                .iter()
                .any(|game_dir| storage_root.join("games").join(game_dir).is_dir())
        })
        .map(|profile| profile.system_id.clone())
        .collect::<BTreeSet<_>>();
    for (system_id, present) in [
        ("amiga", storage_root.join("games/Amiga").is_dir()),
        ("arcade", storage_root.join("_Arcade").is_dir()),
        ("c64", storage_root.join("games/C64").is_dir()),
        ("dos", storage_root.join("_DOS Games").is_dir()),
        (
            "x68000",
            storage_root.join("_Computer/_X68000 Games").is_dir()
                || storage_root.join("_Computer/X68000 Games").is_dir(),
        ),
    ] {
        if present {
            systems.insert(system_id.to_string());
        }
    }
    systems.into_iter().collect()
}

pub fn launcher_catalog_for_fast_system(
    arcade_root: &Path,
    system: &FastFiveSystem,
) -> ArcadeCatalog {
    let games = system
        .games
        .iter()
        .map(|game| ArcadeGameEntry {
            title: Arc::from(game.title.as_str()),
            mra_path: Arc::from(game.launch_ref.as_str()),
            preview_archive_path: Arc::from(game.preview_archive_path.as_str()),
            preview_asset_key: Arc::from(game.preview_asset_key.as_str()),
            has_preview: game.has_preview,
            system_id: Arc::from(system.system_id.as_str()),
            year: game.year,
            manufacturer: Arc::from(game.manufacturer.as_str()),
            category: Arc::from(game.category.as_str()),
            players: game.players,
            control: Arc::from(game.control.as_str()),
            is_new: game.is_new,
        })
        .collect::<Vec<_>>();
    let launch_plans = system
        .games
        .iter()
        .filter_map(|game| game.launch_plan.as_ref())
        .map(|plan| StructuredLaunchPlan {
            launch_ref: Arc::from(plan.launch_ref.as_str()),
            title: Arc::from(plan.title.as_str()),
            system_id: Arc::from(plan.system_id.as_str()),
            core_path: Arc::from(plan.core_path.as_str()),
            payload_path: Arc::from(plan.payload_path.as_str()),
            mount_kind: Arc::from(plan.mount_kind.as_str()),
            mount_index: plan.mount_index,
            delay_secs: plan.delay_secs,
        })
        .collect();
    let systems = vec![GameSystemEntry {
        id: system.system_id.clone(),
        title: system.display_title.clone(),
        count: games.len(),
    }];
    ArcadeCatalog::new_with_deferred_text_indexes(
        arcade_root.to_path_buf(),
        games,
        systems,
        launch_plans,
    )
}

fn merge_system_rows(target: &mut FastFiveSystem, mut additional: FastFiveSystem) {
    target.games.append(&mut additional.games);
    target.variants.append(&mut additional.variants);
    target.games.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| left.stable_key.cmp(&right.stable_key))
    });
    target
        .games
        .dedup_by(|left, right| left.launch_ref == right.launch_ref);
}

fn merge_source_report(target: &mut FastSourceSystemReport, additional: &FastSourceSystemReport) {
    target.files_visited = target
        .files_visited
        .saturating_add(additional.files_visited);
    target.games = target.games.saturating_add(additional.games);
    target.invalid = target.invalid.saturating_add(additional.invalid);
    target.elapsed_us = target.elapsed_us.saturating_add(additional.elapsed_us);
    target.helper_hits = target.helper_hits.saturating_add(additional.helper_hits);
    target.fallback_validations = target
        .fallback_validations
        .saturating_add(additional.fallback_validations);
}

fn build_prepared_system(
    storage_root: &Path,
    system_id: &str,
    capture_watch: bool,
) -> Result<
    (
        FastFiveSystem,
        FastSourceSystemReport,
        Option<GenericSourceWatchObservations>,
    ),
    String,
> {
    let mut report = FastSourceSystemReport {
        system_id: system_id.to_string(),
        ..FastSourceSystemReport::default()
    };
    let mut watch = None;
    let (mut games, mut variants) = match system_id {
        "arcade" => {
            let scan = scan_arcade(storage_root, &mut report)?;
            (scan.games, scan.variants)
        }
        "amiga" => (scan_amiga(storage_root, &mut report)?, Vec::new()),
        "dos" => (
            scan_prepared_mgl(
                &[storage_root.join("_DOS Games")],
                "dos",
                "DOS",
                &mut report,
            )?,
            Vec::new(),
        ),
        "x68000" => (
            scan_prepared_mgl(
                &[
                    storage_root.join("_Computer/_X68000 Games"),
                    storage_root.join("_Computer/X68000 Games"),
                ],
                "x68000",
                "X68000",
                &mut report,
            )?,
            Vec::new(),
        ),
        "c64" => {
            let (games, observations) =
                scan_oneload64_with_observations(storage_root, &mut report, capture_watch)?;
            watch = observations;
            (games, Vec::new())
        }
        _ => return Err(format!("unsupported prepared fast system {system_id}")),
    };
    games.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| left.stable_key.cmp(&right.stable_key))
    });
    games.dedup_by(|left, right| left.launch_ref == right.launch_ref);
    variants.sort_by(|left, right| {
        left.family_stable_key
            .cmp(&right.family_stable_key)
            .then_with(|| {
                left.game
                    .title
                    .to_ascii_lowercase()
                    .cmp(&right.game.title.to_ascii_lowercase())
            })
            .then_with(|| left.game.launch_ref.cmp(&right.game.launch_ref))
    });
    Ok((
        FastFiveSystem {
            system_id: system_id.to_string(),
            display_title: display_title(system_id).to_string(),
            games,
            variants,
        },
        report,
        watch,
    ))
}

#[derive(Clone, Debug)]
struct ArcadeCandidate {
    game: SystemGame,
    identity_id: String,
    family_id: String,
}

#[derive(Clone, Debug)]
struct ArcadeScan {
    games: Vec<SystemGame>,
    variants: Vec<FastFiveGameVariant>,
}

#[derive(Debug, Default)]
struct ArcadeUpdaterEvidence {
    status: &'static str,
    path: String,
    error: String,
    rows: usize,
    file_sha256: String,
    load_us: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct FastArcadeAuditCandidate {
    pub title: String,
    pub launch_ref: String,
    pub family_id: String,
}

fn scan_arcade(
    storage_root: &Path,
    report: &mut FastSourceSystemReport,
) -> Result<ArcadeScan, String> {
    Ok(collapse_arcade_candidates(scan_arcade_candidates(
        storage_root,
        report,
    )?))
}

pub(crate) fn audit_arcade_candidates(storage_root: &Path) -> Vec<FastArcadeAuditCandidate> {
    let mut report = FastSourceSystemReport::default();
    scan_arcade_candidates(storage_root, &mut report)
        .unwrap_or_default()
        .into_iter()
        .map(|candidate| FastArcadeAuditCandidate {
            title: candidate.game.title,
            launch_ref: candidate.game.launch_ref,
            family_id: candidate.family_id,
        })
        .collect()
}

fn scan_arcade_candidates(
    storage_root: &Path,
    report: &mut FastSourceSystemReport,
) -> Result<Vec<ArcadeCandidate>, String> {
    let roms = arcade_rom_inventory(storage_root, report)?;
    let cores = arcade_core_inventory(storage_root, report)?;
    let (updater, updater_evidence) = arcade_updater_rows(storage_root);
    let mut files = Vec::new();
    collect_arcade_mras(
        &storage_root.join("_Arcade"),
        &mut report.files_visited,
        &mut files,
    )?;
    let mut games = Vec::new();
    let mut updater_hits = 0usize;
    let mut updater_misses = 0usize;
    for path in files {
        let relative = path
            .strip_prefix(storage_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase();
        if let Some(row) = updater.get(&relative) {
            updater_hits = updater_hits.saturating_add(1);
            let valid_rom = match &row.primary_rom {
                PrimaryRomRequirement::None => true,
                PrimaryRomRequirement::Archive { namespace, setname } => {
                    roms.contains(&(rom_namespace_label(namespace), normalize_name(setname)))
                }
                PrimaryRomRequirement::Ambiguous => false,
            };
            let valid_core = row
                .header
                .rbf
                .as_deref()
                .map(normalize_name)
                .filter(|expected| !expected.is_empty())
                .is_some_and(|expected| cores.iter().any(|core| core.starts_with(&expected)));
            if !valid_rom || !valid_core {
                report.invalid += 1;
                continue;
            }
            let title = row
                .catalog_metadata
                .as_ref()
                .map(|metadata| metadata.title.clone())
                .or_else(|| row.header.name.clone())
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| display_name(&path));
            let mut game = direct_row("arcade", "Arcade", &path, title);
            game.year = row
                .catalog_metadata
                .as_ref()
                .and_then(|metadata| metadata.year)
                .or_else(|| {
                    row.header
                        .year
                        .as_deref()
                        .and_then(|year| year.parse().ok())
                });
            game.manufacturer = row
                .catalog_metadata
                .as_ref()
                .map(|metadata| metadata.manufacturer.clone())
                .or_else(|| row.header.manufacturer.clone())
                .unwrap_or_default();
            if let Some(metadata) = &row.catalog_metadata {
                game.category = metadata.category.clone();
                game.players = metadata.players;
                game.control = metadata.control.clone();
                game.preview_asset_key =
                    arcade_preview_asset_key(&metadata.identity_id, &metadata.family_id);
            }
            if game.preview_asset_key.is_empty() {
                game.preview_asset_key = arcade_requirement_preview_asset_key(&row.primary_rom);
            }
            let (identity_id, family_id) = row
                .catalog_metadata
                .as_ref()
                .map(|metadata| (metadata.identity_id.clone(), metadata.family_id.clone()))
                .unwrap_or_default();
            games.push(ArcadeCandidate {
                game,
                identity_id,
                family_id,
            });
            continue;
        }
        updater_misses = updater_misses.saturating_add(1);
        let bytes = match fs::read(&path) {
            Ok(bytes) if bytes.len() <= 1024 * 1024 => bytes,
            _ => {
                report.invalid += 1;
                continue;
            }
        };
        let inspection = match crate::mra_header::inspect(&bytes) {
            Ok(inspection) => inspection,
            Err(_) => {
                report.invalid += 1;
                continue;
            }
        };
        let valid_rom = match &inspection.primary_rom {
            PrimaryRomRequirement::None => true,
            PrimaryRomRequirement::Archive { namespace, setname } => {
                roms.contains(&(rom_namespace_label(namespace), normalize_name(setname)))
            }
            PrimaryRomRequirement::Ambiguous => false,
        };
        let valid_core = inspection
            .header
            .rbf
            .as_deref()
            .map(normalize_name)
            .filter(|expected| !expected.is_empty())
            .is_some_and(|expected| cores.iter().any(|core| core.starts_with(&expected)));
        if !valid_rom || !valid_core {
            report.invalid += 1;
            continue;
        }
        let title = inspection
            .catalog_metadata
            .as_ref()
            .map(|metadata| metadata.title.clone())
            .or(inspection.header.name)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| display_name(&path));
        let mut game = direct_row("arcade", "Arcade", &path, title);
        if let Some(metadata) = &inspection.catalog_metadata {
            game.preview_asset_key =
                arcade_preview_asset_key(&metadata.identity_id, &metadata.family_id);
            game.category = metadata.category.clone();
            game.players = metadata.players;
            game.control = metadata.control.clone();
        }
        if game.preview_asset_key.is_empty() {
            game.preview_asset_key = arcade_requirement_preview_asset_key(&inspection.primary_rom);
        }
        game.year = inspection
            .header
            .year
            .as_deref()
            .and_then(|year| year.parse::<u16>().ok());
        game.manufacturer = inspection.header.manufacturer.unwrap_or_default();
        let (identity_id, family_id) = inspection
            .catalog_metadata
            .as_ref()
            .map(|metadata| (metadata.identity_id.clone(), metadata.family_id.clone()))
            .unwrap_or_default();
        games.push(ArcadeCandidate {
            game,
            identity_id,
            family_id,
        });
    }
    crate::catalog_logln!(
        "library_scan_timing\tarcade_mra_prefetch\t{}\tindex_status={} index_path={} index_error={} index_rows={} index_file_sha256={} index_hits={} index_misses={} fallback_reads={} files={} index_load_us={}",
        updater_evidence.load_us,
        updater_evidence.status,
        updater_evidence.path,
        updater_evidence.error,
        updater_evidence.rows,
        updater_evidence.file_sha256,
        updater_hits,
        updater_misses,
        updater_misses,
        updater_hits.saturating_add(updater_misses),
        updater_evidence.load_us,
    );
    Ok(games)
}

fn collapse_arcade_candidates(mut candidates: Vec<ArcadeCandidate>) -> ArcadeScan {
    candidates.sort_by(|left, right| left.game.launch_ref.cmp(&right.game.launch_ref));
    candidates.dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);
    let mut families = BTreeMap::<String, Vec<ArcadeCandidate>>::new();
    for candidate in candidates {
        let family = if candidate.family_id.trim().is_empty() {
            format!("launch:{}", candidate.game.launch_ref.to_ascii_lowercase())
        } else {
            format!("family:{}", candidate.family_id.to_ascii_lowercase())
        };
        families.entry(family).or_default().push(candidate);
    }
    let mut games = Vec::with_capacity(families.len());
    let mut variants = Vec::new();
    for mut family in families.into_values() {
        family.sort_by(|left, right| {
            let left_parent = !left.family_id.is_empty()
                && left.identity_id.eq_ignore_ascii_case(&left.family_id);
            let right_parent = !right.family_id.is_empty()
                && right.identity_id.eq_ignore_ascii_case(&right.family_id);
            right_parent
                .cmp(&left_parent)
                .then_with(|| {
                    left.game
                        .title
                        .to_ascii_lowercase()
                        .cmp(&right.game.title.to_ascii_lowercase())
                })
                .then_with(|| left.game.launch_ref.cmp(&right.game.launch_ref))
        });
        let preferred = family.remove(0).game;
        let family_stable_key = preferred.stable_key.clone();
        games.push(preferred);
        variants.extend(family.into_iter().map(|candidate| FastFiveGameVariant {
            family_stable_key: family_stable_key.clone(),
            relation: FastFiveVariantRelation::ArcadeVariant,
            game: candidate.game,
        }));
    }
    ArcadeScan { games, variants }
}

fn arcade_updater_rows(
    storage_root: &Path,
) -> (
    BTreeMap<String, crate::arcade_updater_index::ArcadeUpdaterRow>,
    ArcadeUpdaterEvidence,
) {
    let started = Instant::now();
    let candidates = [
        storage_root.join("mister-magik-dev/arcade-updater-index-v1.lz4b"),
        storage_root.join("mister-magik/arcade-updater-index-v1.lz4b"),
    ];
    let mut last_error = String::new();
    for path in candidates {
        match crate::arcade_updater_index::ArcadeUpdaterIndex::read_with_file_sha256(&path) {
            Ok((index, file_sha256)) => {
                let rows = index.rows.len();
                return (
                    index
                        .rows
                        .into_iter()
                        .map(|row| (row.path.to_ascii_lowercase(), row))
                        .collect(),
                    ArcadeUpdaterEvidence {
                        status: "loaded",
                        path: path.to_string_lossy().into_owned(),
                        rows,
                        file_sha256,
                        load_us: elapsed_us(started),
                        ..ArcadeUpdaterEvidence::default()
                    },
                );
            }
            Err(error) => last_error = error,
        }
    }
    (
        BTreeMap::new(),
        ArcadeUpdaterEvidence {
            status: "missing",
            error: last_error.split_whitespace().collect::<Vec<_>>().join("_"),
            load_us: elapsed_us(started),
            ..ArcadeUpdaterEvidence::default()
        },
    )
}

fn collect_arcade_mras(
    root: &Path,
    visited: &mut usize,
    output: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let Some(mut entries) = read_dir_entries_checked(root)? else {
        return Ok(());
    };
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    for entry in entries {
        *visited = visited.saturating_add(1);
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if should_ignore_arcade_component(&name) {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| format!("inspect {}: {error}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_arcade_mras(&path, visited, output)?;
        } else if file_type.is_file()
            && extension_is(&path, "mra")
            && !matches!(
                name.to_ascii_lowercase().as_str(),
                "neogeo pocket.mra" | "neogeo pocket color.mra"
            )
        {
            output.push(path);
        }
    }
    Ok(())
}

fn should_ignore_arcade_component(component: &str) -> bool {
    (component.len() > 1 && component.starts_with('.'))
        || [
            ".____padding_file",
            "__macosx",
            "images",
            "manuals",
            "media",
            "cores",
            "screenshot",
            "screenshots",
            "screenshot-magik",
            "_organized",
            "boxart",
        ]
        .iter()
        .any(|ignored| component.eq_ignore_ascii_case(ignored))
}

fn arcade_rom_inventory(
    storage_root: &Path,
    report: &mut FastSourceSystemReport,
) -> Result<BTreeSet<(&'static str, String)>, String> {
    let mut inventory = BTreeSet::new();
    for (namespace, directory) in [("mame", "mame"), ("hbmame", "hbmame")] {
        for root in [
            storage_root.join("games").join(directory),
            storage_root.join("_Arcade").join(directory),
        ] {
            let Some(entries) = read_dir_entries_checked(&root)? else {
                continue;
            };
            for entry in entries {
                report.files_visited = report.files_visited.saturating_add(1);
                let path = entry.path();
                if extension_is(&path, "zip")
                    && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
                {
                    inventory.insert((namespace, normalize_name(stem)));
                }
            }
        }
    }
    Ok(inventory)
}

fn arcade_core_inventory(
    storage_root: &Path,
    report: &mut FastSourceSystemReport,
) -> Result<BTreeSet<String>, String> {
    let mut files = Vec::new();
    collect_matching_files(
        &storage_root.join("_Arcade/cores"),
        &mut report.files_visited,
        &mut files,
        |path| extension_is(path, "rbf"),
    )?;
    Ok(files
        .into_iter()
        .filter_map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(normalize_name)
        })
        .collect())
}

fn rom_namespace_label(namespace: &RomNamespace) -> &'static str {
    match namespace {
        RomNamespace::Mame => "mame",
        RomNamespace::Hbmame => "hbmame",
    }
}

fn scan_amiga(
    storage_root: &Path,
    report: &mut FastSourceSystemReport,
) -> Result<Vec<SystemGame>, String> {
    let amiga = storage_root.join("games/Amiga");
    let mut metadata_files = Vec::new();
    collect_matching_files(
        &amiga,
        &mut report.files_visited,
        &mut metadata_files,
        |path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_ascii_lowercase)
                .is_some_and(|name| {
                    matches!(
                        name.as_str(),
                        "amigavision.hdf" | "megaags.hdf" | "games.txt" | "demos.txt"
                    )
                })
        },
    )?;
    let has_payload = metadata_files.iter().any(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("AmigaVision.hdf")
                    || name.eq_ignore_ascii_case("MegaAGS.hdf")
            })
    });
    let mut games = Vec::new();
    let mut has_collection = false;
    if has_payload {
        for path in metadata_files
            .iter()
            .filter(|path| extension_is(path, "txt"))
        {
            let kind = if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("demos.txt"))
            {
                "demos"
            } else {
                "games"
            };
            let contents = match fs::read_to_string(path) {
                Ok(contents) => contents,
                Err(_) => continue,
            };
            has_collection = true;
            for title in contents
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
            {
                let launch_ref = format!("magik-amigavision:{kind}:{}", encode_component(title));
                let mut game = row("amiga", "Computer", title, &launch_ref, None);
                game.preview_asset_key =
                    ScreenshotAssetId::from_amigavision_title(title).into_string();
                games.push(game);
            }
        }
    }
    if !has_collection {
        let mut archives = Vec::new();
        collect_matching_files(&amiga, &mut report.files_visited, &mut archives, |path| {
            extension_is(path, "7z")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_ascii_lowercase)
                    .is_some_and(|name| name.contains("amigavision") || name.contains("megaags"))
        })?;
        for archive in archives {
            let Ok(metadata) = fs::metadata(&archive) else {
                report.invalid += 1;
                continue;
            };
            let found = FoundFile {
                path: archive,
                ext: "7z".to_string(),
                size: metadata.len(),
                mtime_secs: metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |duration| duration.as_secs() as i64),
            };
            for (entry_path, kind) in [
                ("games/Amiga/listings/games.txt", "games"),
                ("games/Amiga/listings/demos.txt", "demos"),
            ] {
                let listing = CollectionListing {
                    entry_path: entry_path.to_string(),
                    genre: if kind == "games" {
                        "AmigaVision"
                    } else {
                        "AmigaVision demos"
                    }
                    .to_string(),
                };
                let Some(contents) = crate::media_metadata::collection_listing_text_with_tool(
                    &found,
                    &listing,
                    Path::new("/media/fat/linux/7za"),
                    Duration::from_secs(10),
                ) else {
                    continue;
                };
                has_collection = true;
                for title in contents
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                {
                    let launch_ref =
                        format!("magik-amigavision:{kind}:{}", encode_component(title));
                    let mut game = row("amiga", "Computer", title, &launch_ref, None);
                    game.preview_asset_key =
                        ScreenshotAssetId::from_amigavision_title(title).into_string();
                    games.push(game);
                }
            }
        }
    }
    if has_collection {
        games.push(row(
            "amiga",
            "Computer",
            "AmigaVision",
            "magik-amigavision-launcher",
            None,
        ));
    }
    let mut files = Vec::new();
    collect_matching_files(&amiga, &mut report.files_visited, &mut files, |path| {
        ["adf", "cue", "chd", "iso"]
            .into_iter()
            .any(|extension| extension_is(path, extension))
    })?;
    games.extend(
        files
            .into_iter()
            .map(|path| direct_row("amiga", "Computer", &path, display_name(&path))),
    );
    Ok(games)
}

fn scan_prepared_mgl(
    roots: &[PathBuf],
    system_id: &str,
    category: &str,
    report: &mut FastSourceSystemReport,
) -> Result<Vec<SystemGame>, String> {
    let mut files = Vec::new();
    for root in roots {
        collect_matching_files(root, &mut report.files_visited, &mut files, |path| {
            extension_is(path, "mgl")
        })?;
    }
    let prepared_index = (system_id == "dos").then(|| {
        PreparedPayloadIndex::from_library_roots(
            &roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        )
    });
    Ok(files
        .into_iter()
        .filter_map(|path| {
            if let Some(index) = &prepared_index
                && let Some(known) = crate::prepared_release_manifest::known_0mhz_launch(&path)
                && known.package.payloads.iter().all(|payload| {
                    index.path_is_file(&known.storage_root.join(&payload.relative_path))
                })
            {
                report.helper_hits = report.helper_hits.saturating_add(1);
                return Some(direct_row(system_id, category, &path, display_name(&path)));
            }
            report.fallback_validations = report.fallback_validations.saturating_add(1);
            match validate_prepared_launch_path(&path) {
                Ok(true) => Some(direct_row(system_id, category, &path, display_name(&path))),
                _ => {
                    report.invalid += 1;
                    None
                }
            }
        })
        .collect())
}

fn scan_oneload64_with_observations(
    storage_root: &Path,
    report: &mut FastSourceSystemReport,
    capture_watch: bool,
) -> Result<(Vec<SystemGame>, Option<GenericSourceWatchObservations>), String> {
    let base = storage_root.join("games/C64");
    let inventory = inventory_prepared_extension_under_named_roots(&base, "oneload64", "crt")?;
    report.files_visited = report.files_visited.saturating_add(inventory.files_visited);
    let watch = capture_watch.then_some(inventory.watch);
    let games = inventory
        .files
        .into_iter()
        .filter_map(|path| {
            if observed_oneload64_path_is_valid(&path) {
                Some(direct_row("c64", "Computer", &path, display_name(&path)))
            } else {
                report.invalid = report.invalid.saturating_add(1);
                None
            }
        })
        .collect();
    Ok((games, watch))
}

fn collect_matching_files(
    root: &Path,
    visited: &mut usize,
    output: &mut Vec<PathBuf>,
    matches: impl Fn(&Path) -> bool + Copy,
) -> Result<(), String> {
    let Some(mut entries) = read_dir_entries_checked(root)? else {
        return Ok(());
    };
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    for entry in entries {
        *visited += 1;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("inspect {}: {error}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_matching_files(&path, visited, output, matches)?;
        } else if file_type.is_file() && matches(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn read_dir_entries_checked(root: &Path) -> Result<Option<Vec<fs::DirEntry>>, String> {
    let mut last_error = None;
    for _ in 0..2 {
        match fs::read_dir(root) {
            Ok(entries) => match entries.collect::<Result<Vec<_>, _>>() {
                Ok(entries) => return Ok(Some(entries)),
                Err(error) => last_error = Some(error),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => last_error = Some(error),
        }
        crate::cooperative_work::checkpoint();
    }
    Err(format!(
        "enumerate {}: {}",
        root.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown directory error".to_string())
    ))
}

fn direct_row(system_id: &str, category: &str, path: &Path, title: String) -> SystemGame {
    let launch_ref = path.to_string_lossy().into_owned();
    let core_path = match system_id {
        "amiga" => "_Computer/Minimig",
        "c64" => "_Computer/C64",
        "dos" => "_Computer/ao486",
        "x68000" => "_Computer/X68000",
        _ => "",
    };
    let launch_plan =
        (!core_path.is_empty() && !extension_is(path, "mgl")).then(|| SystemLaunchPlan {
            launch_ref: launch_ref.clone(),
            title: title.clone(),
            system_id: system_id.to_string(),
            core_path: core_path.to_string(),
            payload_path: launch_ref.clone(),
            mount_kind: if system_id == "c64" {
                "load-file"
            } else {
                "mount-image"
            }
            .to_string(),
            mount_index: if system_id == "c64" { 1 } else { 0 },
            delay_secs: 0,
        });
    row(system_id, category, &title, &launch_ref, launch_plan)
}

fn row(
    system_id: &str,
    category: &str,
    title: &str,
    launch_ref: &str,
    launch_plan: Option<SystemLaunchPlan>,
) -> SystemGame {
    SystemGame {
        stable_key: format!(
            "{}\u{1f}{}\u{1f}{}",
            system_id,
            title.to_ascii_lowercase(),
            launch_ref
        ),
        title: title.to_string(),
        launch_ref: launch_ref.to_string(),
        preview_archive_path: String::new(),
        preview_asset_key: String::new(),
        has_preview: false,
        year: None,
        manufacturer: String::new(),
        category: category.to_string(),
        players: None,
        control: String::new(),
        is_new: false,
        launch_plan,
    }
}

fn arcade_preview_asset_key(identity_id: &str, family_id: &str) -> String {
    let key = if family_id.trim().is_empty() {
        identity_id
    } else {
        family_id
    };
    key.trim().to_ascii_lowercase()
}

fn arcade_requirement_preview_asset_key(requirement: &PrimaryRomRequirement) -> String {
    match requirement {
        PrimaryRomRequirement::Archive { setname, .. } => Path::new(setname.trim())
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(setname.trim())
            .to_ascii_lowercase(),
        PrimaryRomRequirement::None | PrimaryRomRequirement::Ambiguous => String::new(),
    }
}

fn enrich_fast_preview_identities(storage_root: &Path, systems: &mut [FastFiveSystem]) {
    let title_index = load_fast_console_preview_title_index(storage_root);
    for system in systems {
        for game in &mut system.games {
            match system.system_id.as_str() {
                "neogeo" => {
                    game.preview_asset_key = Path::new(&game.launch_ref)
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .map(str::to_ascii_lowercase)
                        .unwrap_or_default();
                }
                "snes" | "saturn" => {
                    game.preview_asset_key = title_index
                        .get(&(
                            system.system_id.clone(),
                            crate::library_db::canonical_variant_title(&game.title),
                        ))
                        .and_then(Clone::clone)
                        .unwrap_or_default();
                }
                _ => {}
            }
        }
    }
}

fn load_fast_console_preview_title_index(
    storage_root: &Path,
) -> BTreeMap<(String, String), Option<String>> {
    let database = [
        storage_root.join("mister-magik-dev/mame.sqlite3"),
        storage_root.join("mister-magik/mame.sqlite3"),
    ]
    .into_iter()
    .find(|path| path.is_file());
    let Some(database) = database else {
        return BTreeMap::new();
    };
    let Ok(connection) = crate::library_db::open_sqlite_read_only(&database) else {
        return BTreeMap::new();
    };
    if !crate::library_db::sqlite_table_exists(&connection, "mame_software_items").unwrap_or(false)
    {
        return BTreeMap::new();
    }
    let Ok(mut statement) = connection.prepare(
        "SELECT list_name,software_name,parent_name,description
         FROM mame_software_items
         WHERE list_name IN ('snes','saturn')",
    ) else {
        return BTreeMap::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
        ))
    }) else {
        return BTreeMap::new();
    };
    let mut index = BTreeMap::new();
    for row in rows.flatten() {
        let (list_name, software_name, parent_name, description) = row;
        let family = parent_name
            .as_deref()
            .filter(|parent| !parent.trim().is_empty())
            .unwrap_or(&software_name);
        let asset_key = ScreenshotAssetId::from_mame_software(&list_name, family).into_string();
        let key = (
            list_name,
            crate::library_db::canonical_variant_title(&description),
        );
        match index.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Some(asset_key));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if entry.get().as_deref() != Some(asset_key.as_str()) {
                    entry.insert(None);
                }
            }
        }
    }
    index
}

fn fingerprint_systems(systems: &[FastFiveSystem]) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(b"mister-magik-independent-fast-sources-v1\0");
    for system in systems {
        digest.update(
            postcard::to_allocvec(system)
                .map_err(|error| format!("encode {} source rows: {error}", system.system_id))?,
        );
    }
    Ok(hex_lower(&digest.finalize()))
}

fn display_title(system_id: &str) -> &'static str {
    match system_id {
        "amiga" => "Commodore Amiga",
        "arcade" => "Arcade",
        "c64" => "Commodore 64",
        "dos" => "DOS",
        "x68000" => "Sharp X68000",
        _ => "Games",
    }
}

fn display_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("Unknown")
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn normalize_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn encode_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(*byte as char);
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcade_requires_external_rom_and_core() {
        let root = crate::test_support::unique_temp_dir("fast-source-arcade");
        fs::create_dir_all(root.join("_Arcade/cores")).unwrap();
        fs::create_dir_all(root.join("games/mame")).unwrap();
        fs::write(root.join("_Arcade/cores/TestCore_20260826.rbf"), b"core").unwrap();
        fs::write(
            root.join("_Arcade/Test Game.mra"),
            br#"<misterromdescription><name>Test Game</name><rbf>TestCore</rbf><rom zip="test.zip"><part>00</part></rom></misterromdescription>"#,
        )
        .unwrap();
        let mut report = FastSourceSystemReport::default();
        assert!(scan_arcade(&root, &mut report).unwrap().games.is_empty());
        fs::write(root.join("games/mame/test.zip"), b"rom").unwrap();
        let games = scan_arcade(&root, &mut report).unwrap().games;
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].preview_asset_key, "test");
    }

    #[test]
    fn arcade_family_metadata_keeps_one_preferred_game_and_retains_variants() {
        let parent = direct_row(
            "arcade",
            "Arcade",
            Path::new("/media/fat/_Arcade/Example.mra"),
            "Example".to_string(),
        );
        let clone = direct_row(
            "arcade",
            "Arcade",
            Path::new("/media/fat/_Arcade/_alternatives/Example (Japan).mra"),
            "Example (Japan)".to_string(),
        );
        let standalone = direct_row(
            "arcade",
            "Arcade",
            Path::new("/media/fat/_Arcade/Standalone.mra"),
            "Standalone".to_string(),
        );
        let scan = collapse_arcade_candidates(vec![
            ArcadeCandidate {
                game: clone,
                identity_id: "examplej".to_string(),
                family_id: "example".to_string(),
            },
            ArcadeCandidate {
                game: parent,
                identity_id: "example".to_string(),
                family_id: "example".to_string(),
            },
            ArcadeCandidate {
                game: standalone,
                identity_id: String::new(),
                family_id: String::new(),
            },
        ]);
        assert_eq!(scan.games.len(), 2);
        assert_eq!(scan.variants.len(), 1);
        assert!(scan.games.iter().any(|game| game.title == "Example"));
        assert_eq!(scan.variants[0].game.title, "Example (Japan)");
        assert_eq!(
            scan.variants[0].relation,
            FastFiveVariantRelation::ArcadeVariant
        );
        let parent = scan
            .games
            .iter()
            .find(|game| game.title == "Example")
            .unwrap();
        assert_eq!(scan.variants[0].family_stable_key, parent.stable_key);
    }

    #[test]
    fn amigavision_rows_use_materialized_launch_contract() {
        let root = crate::test_support::unique_temp_dir("fast-source-amiga");
        fs::create_dir_all(root.join("games/Amiga/listings")).unwrap();
        fs::write(root.join("games/Amiga/AmigaVision.hdf"), b"hdf").unwrap();
        fs::write(
            root.join("games/Amiga/listings/games.txt"),
            "Alien Breed\nAgony & Pain\n",
        )
        .unwrap();
        let mut report = FastSourceSystemReport::default();
        let games = scan_amiga(&root, &mut report).unwrap();
        assert_eq!(games.len(), 3);
        assert!(games.iter().any(|game| game.title == "AmigaVision"));
        assert_eq!(
            games[1].launch_ref,
            "magik-amigavision:games:Agony%20%26%20Pain"
        );
        assert_eq!(
            games[1].preview_asset_key,
            ScreenshotAssetId::from_amigavision_title("Agony & Pain").as_str()
        );
    }

    #[test]
    fn c64_source_ignores_non_oneload_collections() {
        let root = crate::test_support::unique_temp_dir("fast-source-c64");
        fs::create_dir_all(root.join("games/C64/Personal")).unwrap();
        fs::write(root.join("games/C64/Personal/Game.crt"), b"rom").unwrap();
        fs::create_dir_all(root.join("games/C64/OneLoad64 Games/Publisher")).unwrap();
        fs::create_dir_all(root.join("games/C64/OneLoad64 Games/MultiLoad64")).unwrap();
        fs::write(
            root.join("games/C64/OneLoad64 Games/Publisher/Included.crt"),
            b"rom",
        )
        .unwrap();
        fs::write(
            root.join("games/C64/OneLoad64 Games/Publisher/._Hidden.crt"),
            b"sidecar",
        )
        .unwrap();
        let mut report = FastSourceSystemReport::default();
        let (games, watch) = scan_oneload64_with_observations(&root, &mut report, true).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].title, "Included");
        let watch = watch.expect("fresh C64 inventory must retain watch observations");
        assert_eq!(
            watch.roots,
            BTreeSet::from([root.join("games/C64").to_string_lossy().into_owned()])
        );
        #[cfg(target_os = "linux")]
        assert!(watch.complete);
    }

    #[test]
    fn prepared_source_rejects_unreadable_directory_shape() {
        let root = crate::test_support::unique_temp_dir("fast-source-directory-error");
        let not_a_directory = root.join("not-a-directory");
        fs::write(&not_a_directory, b"file").unwrap();
        let mut visited = 0;
        let mut files = Vec::new();

        let error = collect_matching_files(&not_a_directory, &mut visited, &mut files, |_| true)
            .expect_err("a source directory read failure must not become an empty catalog");

        assert!(error.contains("enumerate"));
        assert!(files.is_empty());
    }

    #[test]
    fn known_0mhz_rows_use_manifest_payload_receipts_before_mgl_fallback() {
        let root = crate::test_support::unique_temp_dir("fast-source-0mhz-helper");
        let launcher_root = root.join("_DOS Games");
        let payload = root.join("games/ao486/media/stunts/stunts.vhd");
        fs::create_dir_all(&launcher_root).unwrap();
        fs::create_dir_all(payload.parent().unwrap()).unwrap();
        fs::write(
            launcher_root.join("4D Sports Driving.mgl"),
            b"known launcher need not be parsed",
        )
        .unwrap();
        fs::write(&payload, b"payload").unwrap();
        let mut report = FastSourceSystemReport::default();
        let games = scan_prepared_mgl(
            std::slice::from_ref(&launcher_root),
            "dos",
            "DOS",
            &mut report,
        )
        .unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(report.helper_hits, 1);
        assert_eq!(report.fallback_validations, 0);

        fs::remove_file(payload).unwrap();
        let mut report = FastSourceSystemReport::default();
        assert!(
            scan_prepared_mgl(&[launcher_root], "dos", "DOS", &mut report)
                .unwrap()
                .is_empty()
        );
        assert_eq!(report.helper_hits, 0);
        assert_eq!(report.fallback_validations, 1);
    }

    #[test]
    fn independent_source_set_contains_no_legacy_input_kind() {
        assert_eq!(FAST_SOURCE_ADAPTER_VERSION, 11);
    }

    #[test]
    fn fast_preview_identity_enrichment_uses_pack_contracts_without_rom_hashing() {
        let root = crate::test_support::unique_temp_dir("fast-preview-identities");
        let database = root.join("mister-magik/mame.sqlite3");
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE mame_software_items(
                    list_name TEXT NOT NULL,
                    software_name TEXT NOT NULL,
                    parent_name TEXT,
                    description TEXT NOT NULL
                 );
                 INSERT INTO mame_software_items VALUES
                    ('snes','smw',NULL,'Super Mario World (USA)'),
                    ('saturn','vf2u','vf2','Virtua Fighter 2 [USA]'),
                    ('saturn','dupe1',NULL,'Ambiguous Game'),
                    ('saturn','dupe2',NULL,'Ambiguous Game');",
            )
            .unwrap();
        drop(connection);

        let mut systems = vec![
            FastFiveSystem {
                system_id: "neogeo".to_string(),
                display_title: "Neo Geo".to_string(),
                games: vec![direct_row(
                    "neogeo",
                    "Arcade",
                    Path::new("/media/fat/games/NEOGEO/mslug.zip"),
                    "Metal Slug".to_string(),
                )],
                variants: Vec::new(),
            },
            FastFiveSystem {
                system_id: "snes".to_string(),
                display_title: "SNES".to_string(),
                games: vec![direct_row(
                    "snes",
                    "Console",
                    Path::new("/media/fat/games/SNES/Super Mario World (USA).sfc"),
                    "Super Mario World (USA)".to_string(),
                )],
                variants: Vec::new(),
            },
            FastFiveSystem {
                system_id: "saturn".to_string(),
                display_title: "Saturn".to_string(),
                games: vec![
                    direct_row(
                        "saturn",
                        "Console",
                        Path::new("/media/fat/games/Saturn/Virtua Fighter 2.cue"),
                        "Virtua Fighter 2".to_string(),
                    ),
                    direct_row(
                        "saturn",
                        "Console",
                        Path::new("/media/fat/games/Saturn/Ambiguous Game.cue"),
                        "Ambiguous Game".to_string(),
                    ),
                ],
                variants: Vec::new(),
            },
        ];
        enrich_fast_preview_identities(&root, &mut systems);

        assert_eq!(systems[0].games[0].preview_asset_key, "mslug");
        assert_eq!(
            systems[1].games[0].preview_asset_key,
            "mame-software__snes__smw"
        );
        assert_eq!(
            systems[2].games[0].preview_asset_key,
            "mame-software__saturn__vf2"
        );
        assert!(systems[2].games[1].preview_asset_key.is_empty());
    }
}
