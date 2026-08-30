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
use crate::machine_family::{MachineFamilyResolver, MachineSource};
use crate::machine_family_projection::{MachineFamilyCandidate, project_machine_families};
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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const FAST_SOURCE_ADAPTER_VERSION: u32 = 14;
const PREPARED_SYSTEM_IDS: [&str; 5] = ["arcade", "amiga", "c64", "dos", "x68000"];
const MAX_DISCOVERY_ENTRIES: usize = 4_000_000;
const MAX_DISCOVERY_DEPTH: usize = 256;
const MAX_DIRECTORY_ENTRIES: usize = 1_000_000;
const MAX_MRA_BYTES: u64 = 1024 * 1024;
const MAX_COLLECTION_LISTING_BYTES: usize = 8 * 1024 * 1024;

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
    pub plan_ready_us: u64,
    pub system_complete_us: u64,
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
    pub family_raw: usize,
    pub family_resolved: usize,
    pub family_visible: usize,
    pub family_variants: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct FamilyProjectionStats {
    raw: usize,
    resolved: usize,
    visible: usize,
    variants: usize,
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
    pub row_fingerprints: BTreeMap<String, String>,
}

pub(crate) fn build_independent_fast_snapshot_for_refresh_with_progress(
    storage_root: &Path,
    mut plan_ready: impl FnMut(&[String]),
    mut system_discovering: impl FnMut(&str),
    mut system_complete: impl FnMut(&FastFiveSystem),
) -> Result<FastSourceRefreshBuild, String> {
    let started = Instant::now();
    let mut family_resolver = MachineFamilyResolver::for_storage_root(storage_root)?;
    let mut systems = BTreeMap::new();
    let mut reports = BTreeMap::new();
    let mut prepared_watch_observations = BTreeMap::new();
    let mut system_complete_us: u64 = 0;
    let mut timed_system_complete = |system: &FastFiveSystem| {
        let callback_started = Instant::now();
        system_complete(system);
        system_complete_us = system_complete_us.saturating_add(elapsed_us(callback_started));
    };
    system_discovering("Arcade");
    build_and_record_prepared_system(
        storage_root,
        "arcade",
        &mut systems,
        &mut reports,
        &mut prepared_watch_observations,
        &mut family_resolver,
        &mut timed_system_complete,
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
            &mut system_discovering,
            |_| {},
        )?;
    let generic_systems_us = generic.elapsed_us;
    let phase_started = Instant::now();
    let planned_system_ids = discover_independent_system_ids_from_profiles(storage_root, &profiles);
    let system_planning_us = elapsed_us(phase_started);
    let plan_ready_started = Instant::now();
    plan_ready(&planned_system_ids);
    let plan_ready_us = elapsed_us(plan_ready_started);
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
            &mut family_resolver,
            &mut timed_system_complete,
        )?;
    }
    generic_watch_observations.extend(prepared_watch_observations);
    let phase_started = Instant::now();
    let mut generic_family_stats = BTreeMap::new();
    for system in &mut generic_systems {
        if system.system_id == "neogeo" {
            generic_family_stats.insert(
                system.system_id.clone(),
                project_neogeo_system(system, &mut family_resolver)?,
            );
        }
    }
    enrich_fast_preview_identities(storage_root, &mut generic_systems);
    let preview_identity_us = elapsed_us(phase_started);
    let phase_started = Instant::now();
    systems.extend(
        generic_systems
            .into_iter()
            .map(|system| (system.system_id.clone(), system)),
    );
    reports.extend(generic.systems.into_iter().map(|system| {
        let system_id = system.system_id.clone();
        let family_stats = generic_family_stats
            .get(&system_id)
            .copied()
            .unwrap_or_default();
        (
            system_id.clone(),
            FastSourceSystemReport {
                system_id,
                files_visited: system.files,
                games: system.games,
                invalid: system.read_errors.saturating_add(system.archive_errors),
                elapsed_us: system.elapsed_us,
                helper_hits: 0,
                fallback_validations: 0,
                family_raw: family_stats.raw,
                family_resolved: family_stats.resolved,
                family_visible: family_stats.visible,
                family_variants: family_stats.variants,
            },
        )
    }));
    for (system_id, system) in &systems {
        if let Some(report) = reports.get_mut(system_id) {
            report.games = system.games.len();
        }
    }
    for system in systems.values() {
        if !PREPARED_SYSTEM_IDS.contains(&system.system_id.as_str()) {
            timed_system_complete(system);
        }
    }
    let merge_us = elapsed_us(phase_started);
    let prepared_systems_us: u64 = reports
        .values()
        .filter(|report| PREPARED_SYSTEM_IDS.contains(&report.system_id.as_str()))
        .map(|report| report.elapsed_us)
        .sum();
    let phase_started = Instant::now();
    let (source_fingerprint, row_fingerprints) = fingerprint_systems(systems.values())?;
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
        .saturating_add(plan_ready_us)
        .saturating_add(system_complete_us)
        .saturating_add(generic_systems_us)
        .saturating_add(preview_identity_us)
        .saturating_add(merge_us)
        .saturating_add(fingerprint_us)
        .saturating_add(validation_us);
    let phases = FastSourcePhaseReport {
        prepared_systems_us,
        profile_discovery_us,
        system_planning_us,
        plan_ready_us,
        system_complete_us,
        generic_systems_us,
        preview_identity_us,
        merge_us,
        fingerprint_us,
        validation_us,
        residual_us: total_us.saturating_sub(accounted_us),
    };
    crate::catalog_logln!(
        "fast_catalog_source_phase_tsv\ttotal_us={}\tprepared_us={}\tprofiles_us={}\tplanning_us={}\tplan_ready_us={}\tsystem_complete_us={}\tgeneric_us={}\tpreview_identity_us={}\tmerge_us={}\tfingerprint_us={}\tvalidation_us={}\tresidual_us={}",
        total_us,
        phases.prepared_systems_us,
        phases.profile_discovery_us,
        phases.system_planning_us,
        phases.plan_ready_us,
        phases.system_complete_us,
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
        row_fingerprints,
    })
}

fn build_and_record_prepared_system(
    storage_root: &Path,
    system_id: &str,
    systems: &mut BTreeMap<String, FastFiveSystem>,
    reports: &mut BTreeMap<String, FastSourceSystemReport>,
    watch_observations: &mut BTreeMap<String, GenericSourceWatchObservations>,
    family_resolver: &mut MachineFamilyResolver,
    system_complete: &mut impl FnMut(&FastFiveSystem),
) -> Result<(), String> {
    let system_started = Instant::now();
    let (mut system, mut report, watch) =
        build_prepared_system(storage_root, system_id, true, family_resolver)?;
    if system_id == "c64" {
        collapse_c64_cross_source_variants(&mut system);
    }
    if system_id == "neogeo" {
        project_neogeo_system(&mut system, &mut family_resolver)?;
    }
    enrich_fast_preview_identities(storage_root, std::slice::from_mut(&mut system));
    report.elapsed_us = elapsed_us(system_started);
    report.games = system.games.len();
    crate::catalog_logln!(
        "fast_catalog_source_tsv\tadapter=prepared\tsystem={}\telapsed_us={}\tfiles={}\tgames={}\tinvalid={}\thelper_hits={}\tfallback_validations={}\tfamily_raw={}\tfamily_resolved={}\tfamily_visible={}\tfamily_variants={}",
        report.system_id,
        report.elapsed_us,
        report.files_visited,
        report.games,
        report.invalid,
        report.helper_hits,
        report.fallback_validations,
        report.family_raw,
        report.family_resolved,
        report.family_visible,
        report.family_variants,
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
    let mut family_resolver = MachineFamilyResolver::for_storage_root(storage_root)?;
    let prepared = PREPARED_SYSTEM_IDS
        .contains(&system_id)
        .then(|| build_prepared_system(storage_root, system_id, false, &mut family_resolver))
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
                    family_raw: 0,
                    family_resolved: 0,
                    family_visible: 0,
                    family_variants: 0,
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
                family_raw: 0,
                family_resolved: 0,
                family_visible: 0,
                family_variants: 0,
            },
        ),
        (None, None) => return Ok(None),
    };
    if system_id == "c64" {
        collapse_c64_cross_source_variants(&mut system);
    }
    if system_id == "neogeo" {
        let stats = project_neogeo_system(&mut system, &mut family_resolver)?;
        report.family_raw = stats.raw;
        report.family_resolved = stats.resolved;
        report.family_visible = stats.visible;
        report.family_variants = stats.variants;
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
    target.family_raw = target.family_raw.saturating_add(additional.family_raw);
    target.family_resolved = target
        .family_resolved
        .saturating_add(additional.family_resolved);
    target.family_visible = target
        .family_visible
        .saturating_add(additional.family_visible);
    target.family_variants = target
        .family_variants
        .saturating_add(additional.family_variants);
}

fn build_prepared_system(
    storage_root: &Path,
    system_id: &str,
    capture_watch: bool,
    family_resolver: &mut MachineFamilyResolver,
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
            let scan = scan_arcade_with_resolver(storage_root, &mut report, family_resolver)?;
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
    parent_id: String,
    namespace: Option<RomNamespace>,
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
    let mut resolver = MachineFamilyResolver::for_storage_root(storage_root)?;
    scan_arcade_with_resolver(storage_root, report, &mut resolver)
}

fn scan_arcade_with_resolver(
    storage_root: &Path,
    report: &mut FastSourceSystemReport,
    resolver: &mut MachineFamilyResolver,
) -> Result<ArcadeScan, String> {
    let resolved_before = resolver
        .mame_matches
        .saturating_add(resolver.hbmame_matches);
    let candidates = scan_arcade_candidates(storage_root, report, resolver)?;
    let raw = candidates.len();
    let scan = collapse_arcade_candidates(candidates);
    report.family_raw = raw;
    report.family_resolved = resolver
        .mame_matches
        .saturating_add(resolver.hbmame_matches)
        .saturating_sub(resolved_before);
    report.family_visible = scan.games.len();
    report.family_variants = scan.variants.len();
    Ok(scan)
}

pub(crate) fn audit_arcade_candidates(storage_root: &Path) -> Vec<FastArcadeAuditCandidate> {
    let mut report = FastSourceSystemReport::default();
    let mut resolver = MachineFamilyResolver::for_storage_root(storage_root).unwrap_or_default();
    scan_arcade_candidates(storage_root, &mut report, &mut resolver)
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
    resolver: &mut MachineFamilyResolver,
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
    let updater_families = updater
        .values()
        .filter_map(|row| row.catalog_metadata.as_ref())
        .filter_map(|metadata| {
            let identity = normalize_machine_id(&metadata.identity_id);
            let family = normalize_machine_id(&metadata.family_id);
            (!identity.is_empty() && !family.is_empty()).then_some((identity, family))
        })
        .collect::<BTreeMap<_, _>>();
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
                parent_id: String::new(),
                namespace: primary_rom_namespace(&row.primary_rom),
            });
            continue;
        }
        updater_misses = updater_misses.saturating_add(1);
        let bytes = match read_bounded_file(&path, MAX_MRA_BYTES) {
            Ok(bytes) => bytes,
            Err(_) => {
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
        let namespace = primary_rom_namespace(&inspection.primary_rom);
        let identity_id = inspection
            .header
            .setname
            .as_deref()
            .map(normalize_machine_id)
            .filter(|identity| !identity.is_empty())
            .or_else(|| match &inspection.primary_rom {
                PrimaryRomRequirement::Archive { setname, .. } => {
                    Some(normalize_machine_id(setname)).filter(|identity| !identity.is_empty())
                }
                PrimaryRomRequirement::None | PrimaryRomRequirement::Ambiguous => None,
            })
            .unwrap_or_default();
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
        let family_id = inspection
            .catalog_metadata
            .as_ref()
            .map(|metadata| normalize_machine_id(&metadata.family_id))
            .unwrap_or_default();
        let family_id = if family_id.is_empty() {
            updater_families
                .get(&identity_id)
                .cloned()
                .unwrap_or_default();
        } else {
            family_id
        };
        let parent_id = inspection
            .header
            .parent
            .as_deref()
            .map(normalize_machine_id)
            .unwrap_or_default();
        games.push(ArcadeCandidate {
            game,
            identity_id,
            family_id,
            parent_id,
            namespace,
        });
    }
    let requests = games
        .iter()
        .filter(|candidate| candidate.family_id.is_empty() && !candidate.identity_id.is_empty())
        .map(|candidate| (candidate.identity_id.clone(), candidate.namespace.clone()))
        .collect::<Vec<_>>();
    let resolved = resolver.resolve_many(requests)?;
    for candidate in &mut games {
        if candidate.family_id.is_empty() {
            if let Some(Some(machine)) =
                resolved.get(&(candidate.identity_id.clone(), candidate.namespace.clone()))
            {
                candidate.family_id = machine.family.clone();
            }
            if candidate.family_id.is_empty() {
                candidate.family_id = candidate.parent_id.clone();
            }
            if candidate.family_id.is_empty() {
                candidate.family_id = candidate.identity_id.clone();
            }
        }
    }
    resolver.finish_log("arcade");
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
    let projection = project_machine_families(
        candidates
            .drain(..)
            .map(|candidate| MachineFamilyCandidate {
                game: candidate.game,
                identity_id: candidate.identity_id,
                family_id: candidate.family_id,
                relation: FastFiveVariantRelation::ArcadeVariant,
            })
            .collect(),
    );
    ArcadeScan {
        games: projection.games,
        variants: projection.variants,
    }
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
    collect_arcade_mras_at_depth(root, visited, output, 0)
}

fn collect_arcade_mras_at_depth(
    root: &Path,
    visited: &mut usize,
    output: &mut Vec<PathBuf>,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DISCOVERY_DEPTH {
        return Err(format!(
            "{} kind=directory-depth observed={} configured={} path={}",
            crate::catalog_progress::CATALOG_SAFETY_LIMIT_NONRETRYABLE,
            depth,
            MAX_DISCOVERY_DEPTH,
            root.display()
        ));
    }
    let Some(mut entries) = read_dir_entries_checked(root)? else {
        return Ok(());
    };
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    for entry in entries {
        *visited = visited.saturating_add(1);
        crate::catalog_progress::report_inner_progress_at(*visited);
        if *visited > MAX_DISCOVERY_ENTRIES {
            return Err(format!(
                "{} kind=entries observed={} configured={} path={}",
                crate::catalog_progress::CATALOG_SAFETY_LIMIT_NONRETRYABLE,
                *visited,
                MAX_DISCOVERY_ENTRIES,
                root.display()
            ));
        }
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
            collect_arcade_mras_at_depth(&path, visited, output, depth.saturating_add(1))?;
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
                crate::catalog_progress::report_inner_progress_at(report.files_visited);
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

fn primary_rom_namespace(requirement: &PrimaryRomRequirement) -> Option<RomNamespace> {
    match requirement {
        PrimaryRomRequirement::Archive { namespace, .. } => Some(namespace.clone()),
        PrimaryRomRequirement::None | PrimaryRomRequirement::Ambiguous => None,
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
            let bytes = read_bounded_file(path, MAX_COLLECTION_LISTING_BYTES as u64)?;
            let contents = String::from_utf8_lossy(&bytes);
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
                let Some(contents) =
                    crate::media_metadata::collection_listing_text_with_tool_result(
                        &found,
                        &listing,
                        Path::new("/media/fat/linux/7za"),
                        Duration::from_secs(10),
                    )
                    .map_err(|error| {
                        format!(
                            "read AmigaVision listing {} from {}: {error}",
                            entry_path,
                            found.path.display()
                        )
                    })?
                else {
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
    collect_matching_files_at_depth(root, visited, output, matches, 0)
}

fn collect_matching_files_at_depth(
    root: &Path,
    visited: &mut usize,
    output: &mut Vec<PathBuf>,
    matches: impl Fn(&Path) -> bool + Copy,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DISCOVERY_DEPTH {
        return Err(format!(
            "{} kind=directory-depth observed={} configured={} path={}",
            crate::catalog_progress::CATALOG_SAFETY_LIMIT_NONRETRYABLE,
            depth,
            MAX_DISCOVERY_DEPTH,
            root.display()
        ));
    }
    let Some(mut entries) = read_dir_entries_checked(root)? else {
        return Ok(());
    };
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    for entry in entries {
        *visited = visited.saturating_add(1);
        crate::catalog_progress::report_inner_progress_at(*visited);
        if *visited > MAX_DISCOVERY_ENTRIES {
            return Err(format!(
                "{} kind=entries observed={} configured={} path={}",
                crate::catalog_progress::CATALOG_SAFETY_LIMIT_NONRETRYABLE,
                *visited,
                MAX_DISCOVERY_ENTRIES,
                root.display()
            ));
        }
        let file_type = entry
            .file_type()
            .map_err(|error| format!("inspect {}: {error}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_matching_files_at_depth(
                &path,
                visited,
                output,
                matches,
                depth.saturating_add(1),
            )?;
        } else if file_type.is_file() && matches(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn read_dir_entries_checked(root: &Path) -> Result<Option<Vec<fs::DirEntry>>, String> {
    let mut last_error = None;
    for _ in 0..2 {
        last_error = None;
        match fs::read_dir(root) {
            Ok(entries) => {
                let mut collected = Vec::new();
                for entry in entries {
                    if collected.len() >= MAX_DIRECTORY_ENTRIES {
                        return Err(format!(
                            "{} kind=directory-entries observed={} configured={} path={}",
                            crate::catalog_progress::CATALOG_SAFETY_LIMIT_NONRETRYABLE,
                            collected.len().saturating_add(1),
                            MAX_DIRECTORY_ENTRIES,
                            root.display(),
                        ));
                    }
                    match entry {
                        Ok(entry) => {
                            collected.push(entry);
                            crate::catalog_progress::report_inner_progress_at(collected.len());
                        }
                        Err(error) => {
                            last_error = Some(error);
                            break;
                        }
                    }
                }
                if last_error.is_none() {
                    return Ok(Some(collected));
                }
            }
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

fn read_bounded_file(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("metadata {}: {error}", path.display()))?;
    if metadata.len() > maximum {
        return Err(format!(
            "{} kind=file-bytes observed={} configured={} path={}",
            crate::catalog_progress::CATALOG_SAFETY_LIMIT_NONRETRYABLE,
            metadata.len(),
            maximum,
            path.display(),
        ));
    }
    let file = fs::File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(metadata.len().try_into().unwrap_or(usize::MAX));
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes.len() as u64 > maximum {
        return Err(format!(
            "{} kind=file-bytes observed={} configured={} path={}",
            crate::catalog_progress::CATALOG_SAFETY_LIMIT_NONRETRYABLE,
            bytes.len(),
            maximum,
            path.display(),
        ));
    }
    Ok(bytes)
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

fn project_neogeo_system(
    system: &mut FastFiveSystem,
    resolver: &mut MachineFamilyResolver,
) -> Result<FamilyProjectionStats, String> {
    if system.system_id != "neogeo" || system.games.is_empty() {
        return Ok(FamilyProjectionStats::default());
    }
    let (projection, _, stats) = project_neogeo_games(system.games.drain(..), resolver)?;
    system.games = projection.games;
    system.variants = projection.variants;
    Ok(stats)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NeoGeoIdentityStrength {
    Strong,
    Weak,
}

#[derive(Clone, Debug)]
struct NeoGeoIdentity {
    id: String,
    strength: NeoGeoIdentityStrength,
}

#[derive(Clone, Debug)]
struct NeoGeoProjectionMetadata {
    identity_id: String,
    family_id: String,
    strength: NeoGeoIdentityStrength,
    source: Option<MachineSource>,
}

fn project_neogeo_games(
    games: impl IntoIterator<Item = SystemGame>,
    resolver: &mut MachineFamilyResolver,
) -> Result<
    (
        crate::machine_family_projection::MachineFamilyProjection,
        BTreeMap<String, NeoGeoProjectionMetadata>,
        FamilyProjectionStats,
    ),
    String,
> {
    let mut candidates = games
        .into_iter()
        .map(|game| {
            let identity = neogeo_identity(&game.launch_ref);
            MachineFamilyCandidate {
                game,
                identity_id: identity
                    .as_ref()
                    .map_or_else(String::new, |item| item.id.clone()),
                family_id: String::new(),
                relation: FastFiveVariantRelation::NeoGeoVariant,
            }
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by(|left, right| {
        left.game
            .launch_ref
            .cmp(&right.game.launch_ref)
            .then_with(|| left.game.stable_key.cmp(&right.game.stable_key))
    });
    candidates.dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);
    let raw = candidates.len();
    let strengths = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.game.launch_ref.clone(),
                neogeo_identity(&candidate.game.launch_ref)
                    .map_or(NeoGeoIdentityStrength::Weak, |item| item.strength),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let requests = candidates
        .iter()
        .filter(|candidate| !candidate.identity_id.is_empty())
        .map(|candidate| (candidate.identity_id.clone(), None))
        .collect::<Vec<_>>();
    let resolved = resolver.resolve_many(requests)?;
    let mut metadata = BTreeMap::new();
    for candidate in &mut candidates {
        let strength = strengths
            .get(&candidate.game.launch_ref)
            .copied()
            .unwrap_or(NeoGeoIdentityStrength::Weak);
        let mut source = None;
        if let Some(Some(machine)) = resolved.get(&(candidate.identity_id.clone(), None)) {
            candidate.family_id = machine.family.clone();
            candidate.game.preview_asset_key = machine.family.clone();
            source = Some(machine.source);
        }
        metadata.insert(
            candidate.game.launch_ref.clone(),
            NeoGeoProjectionMetadata {
                identity_id: candidate.identity_id.clone(),
                family_id: candidate.family_id.clone(),
                strength,
                source,
            },
        );
    }
    let projection = project_machine_families(candidates);
    resolver.finish_log("neogeo");
    let stats = FamilyProjectionStats {
        raw,
        resolved: metadata
            .values()
            .filter(|item| item.source.is_some())
            .count(),
        visible: projection.games.len(),
        variants: projection.variants.len(),
    };
    Ok((projection, metadata, stats))
}

#[derive(Clone, Debug)]
pub struct NeoGeoFamilyAuditReport {
    pub valid: bool,
    pub text: String,
}

/// Audit the installed Neo Geo source through the same scanner, resolver, and
/// contiguous family projector used by publication.
pub fn audit_installed_neogeo_families(
    storage_root: &Path,
) -> Result<NeoGeoFamilyAuditReport, String> {
    let mut resolver = MachineFamilyResolver::for_storage_root(storage_root)?;
    let installed = rebuild_installed_generic_system(storage_root, "neogeo")?;
    let Some((system, _report)) = installed else {
        return Ok(NeoGeoFamilyAuditReport {
            valid: true,
            text: "neogeo_family_summary_tsv\tvalid=1\traw_deduplicated=0\tinstalled_games=0\tvisible_games=0\tvariants=0\tfamilies=0\tmame=0\thbmame=0\tunresolved=0\tstrong_unresolved=0\traw_visible_variant_mismatch=0\tduplicate_visible_families=0\tmissing_variant_heads=0\n".to_string(),
        });
    };
    let (projection, metadata, stats) = project_neogeo_games(system.games, &mut resolver)?;
    let family_keys = projection
        .games
        .iter()
        .map(|game| game.stable_key.as_str())
        .collect::<BTreeSet<_>>();
    let mame = metadata
        .values()
        .filter(|item| item.source == Some(MachineSource::Mame))
        .count();
    let hbmame = metadata
        .values()
        .filter(|item| item.source == Some(MachineSource::Hbmame))
        .count();
    let unresolved = metadata
        .values()
        .filter(|item| item.source.is_none())
        .count();
    let strong_unresolved = metadata
        .values()
        .filter(|item| item.source.is_none() && item.strength == NeoGeoIdentityStrength::Strong)
        .count();
    let mut resolved_visible_families = BTreeMap::<&str, usize>::new();
    for game in &projection.games {
        if let Some(item) = metadata.get(&game.launch_ref)
            && !item.family_id.is_empty()
        {
            *resolved_visible_families
                .entry(item.family_id.as_str())
                .or_default() += 1;
        }
    }
    let duplicate_visible_families = resolved_visible_families
        .values()
        .filter(|count| **count > 1)
        .count();
    let missing_variant_heads = projection
        .variants
        .iter()
        .filter(|variant| !family_keys.contains(variant.family_stable_key.as_str()))
        .count();
    let raw_mismatch = stats.raw != stats.visible.saturating_add(stats.variants);
    let valid = !raw_mismatch
        && duplicate_visible_families == 0
        && missing_variant_heads == 0
        && strong_unresolved == 0;
    let mut rows = String::new();
    for game in &projection.games {
        let item = metadata.get(&game.launch_ref);
        let fallback = NeoGeoProjectionMetadata {
            identity_id: String::new(),
            family_id: String::new(),
            strength: NeoGeoIdentityStrength::Weak,
            source: None,
        };
        let item = item.unwrap_or(&fallback);
        rows.push_str(&format!(
            "neogeo_family_row_tsv\tstatus=visible\ttitle={}\tlaunch_ref={}\tidentity_id={}\tfamily_id={}\tidentity_strength={}\tfamily_source={}\tfamily_stable_key={}\n",
            sanitize_family_audit_field(&game.title),
            sanitize_family_audit_field(&game.launch_ref),
            sanitize_family_audit_field(&item.identity_id),
            sanitize_family_audit_field(&item.family_id),
            neogeo_strength_label(item.strength),
            machine_source_label(item.source),
            sanitize_family_audit_field(&game.stable_key),
        ));
    }
    for variant in &projection.variants {
        let item = metadata.get(&variant.game.launch_ref);
        let fallback = NeoGeoProjectionMetadata {
            identity_id: String::new(),
            family_id: String::new(),
            strength: NeoGeoIdentityStrength::Weak,
            source: None,
        };
        let item = item.unwrap_or(&fallback);
        rows.push_str(&format!(
            "neogeo_family_row_tsv\tstatus=variant\ttitle={}\tlaunch_ref={}\tidentity_id={}\tfamily_id={}\tidentity_strength={}\tfamily_source={}\tfamily_stable_key={}\n",
            sanitize_family_audit_field(&variant.game.title),
            sanitize_family_audit_field(&variant.game.launch_ref),
            sanitize_family_audit_field(&item.identity_id),
            sanitize_family_audit_field(&item.family_id),
            neogeo_strength_label(item.strength),
            machine_source_label(item.source),
            sanitize_family_audit_field(&variant.family_stable_key),
        ));
    }
    let summary = format!(
        "neogeo_family_summary_tsv\tvalid={}\traw_deduplicated={}\tinstalled_games={}\tvisible_games={}\tvariants={}\tfamilies={}\tmame={}\thbmame={}\tunresolved={}\tstrong_unresolved={}\traw_visible_variant_mismatch={}\tduplicate_visible_families={}\tmissing_variant_heads={}\n",
        if valid { 1 } else { 0 },
        stats.raw,
        stats.raw,
        stats.visible,
        stats.variants,
        family_keys.len(),
        mame,
        hbmame,
        unresolved,
        strong_unresolved,
        if raw_mismatch { 1 } else { 0 },
        duplicate_visible_families,
        missing_variant_heads,
    );
    Ok(NeoGeoFamilyAuditReport {
        valid,
        text: summary + &rows,
    })
}

fn sanitize_family_audit_field(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

fn neogeo_identity(launch_ref: &str) -> Option<NeoGeoIdentity> {
    let path = match crate::archive_member::decode_archive_member_ref(launch_ref) {
        Ok(Some(member)) => member.member_path,
        _ => launch_ref.to_string(),
    };
    let path = Path::new(&path);
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case("zip") && !extension.eq_ignore_ascii_case("neo") {
        return None;
    }
    if let Some(identity) =
        crate::media_metadata::parenthesized_setname(path.to_string_lossy().as_ref())
    {
        return Some(NeoGeoIdentity {
            id: identity,
            strength: NeoGeoIdentityStrength::Strong,
        });
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(normalize_machine_id)
        .filter(|identity| !identity.is_empty())
        .map(|id| NeoGeoIdentity {
            id,
            strength: if extension.eq_ignore_ascii_case("zip") {
                NeoGeoIdentityStrength::Strong
            } else {
                NeoGeoIdentityStrength::Weak
            },
        })
}

fn neogeo_strength_label(strength: NeoGeoIdentityStrength) -> &'static str {
    match strength {
        NeoGeoIdentityStrength::Strong => "strong",
        NeoGeoIdentityStrength::Weak => "weak",
    }
}

fn machine_source_label(source: Option<MachineSource>) -> &'static str {
    match source {
        Some(MachineSource::Mame) => "mame",
        Some(MachineSource::Hbmame) => "hbmame",
        None => "unresolved",
    }
}

fn enrich_fast_preview_identities(storage_root: &Path, systems: &mut [FastFiveSystem]) {
    let title_index = systems
        .iter()
        .any(|system| matches!(system.system_id.as_str(), "snes" | "saturn"))
        .then(|| load_fast_console_preview_title_index(storage_root))
        .unwrap_or_default();
    let mut visited = 0usize;
    for system in systems {
        for game in &mut system.games {
            visited = visited.saturating_add(1);
            crate::catalog_progress::report_inner_progress_at(visited);
            match system.system_id.as_str() {
                "neogeo" => {
                    if game.preview_asset_key.is_empty() {
                        game.preview_asset_key = Path::new(&game.launch_ref)
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .map(str::to_ascii_lowercase)
                            .unwrap_or_default();
                    }
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

fn fingerprint_systems<'a>(
    systems: impl IntoIterator<Item = &'a FastFiveSystem>,
) -> Result<(String, BTreeMap<String, String>), String> {
    let mut digest = Sha256::new();
    let mut row_fingerprints = BTreeMap::new();
    digest.update(b"mister-magik-independent-fast-sources-v3\0");
    for system in systems {
        let payload =
            crate::fast_catalog_refresh::encode_row_fingerprint_payload_for_system(system)?;
        let row_fingerprint = hex_lower(&Sha256::digest(&payload));
        digest.update(system.system_id.as_bytes());
        digest.update([0]);
        digest.update(row_fingerprint.as_bytes());
        digest.update([0]);
        row_fingerprints.insert(system.system_id.clone(), row_fingerprint);
    }
    Ok((hex_lower(&digest.finalize()), row_fingerprints))
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

fn normalize_machine_id(value: &str) -> String {
    let normalized = crate::library_db::normalize_id(value);
    (normalized != "unknown")
        .then_some(normalized)
        .unwrap_or_default()
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
                parent_id: String::new(),
                namespace: None,
            },
            ArcadeCandidate {
                game: parent,
                identity_id: "example".to_string(),
                family_id: "example".to_string(),
                parent_id: String::new(),
                namespace: None,
            },
            ArcadeCandidate {
                game: standalone,
                identity_id: String::new(),
                family_id: String::new(),
                parent_id: String::new(),
                namespace: None,
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
    fn arcade_updater_miss_uses_resolver_before_mra_parent_and_projects_four_candidates() {
        let root = crate::test_support::unique_temp_dir("fast-source-arcade-family-fallback");
        fs::create_dir_all(root.join("_Arcade/cores")).unwrap();
        fs::create_dir_all(root.join("games/mame")).unwrap();
        fs::write(
            root.join("_Arcade/cores/ArkanoidCore_20260830.rbf"),
            b"core",
        )
        .unwrap();
        fs::write(root.join("games/mame/arkanoid.zip"), b"rom").unwrap();
        let database = root.join("mister-magik/mame.sqlite3");
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        crate::test_support::write_mame_fixture_db(
            &database,
            &[
                ("arkanoid", None, "Arkanoid", None, None),
                (
                    "arkanoidj",
                    Some("arkanoid"),
                    "Arkanoid (Japan)",
                    None,
                    None,
                ),
                ("arkanoid2", Some("arkanoid"), "Arkanoid 2", None, None),
                ("arkanoid3", Some("arkanoid"), "Arkanoid 3", None, None),
            ],
        );
        for (name, setname) in [
            ("Arkanoid.mra", "arkanoid"),
            ("Arkanoid Japan.mra", "arkanoidj"),
            ("Arkanoid 2.mra", "arkanoid2"),
            ("Arkanoid 3.mra", "arkanoid3"),
        ] {
            fs::write(
                root.join("_Arcade").join(name),
                format!(
                    "<misterromdescription><name>{name}</name><setname>{setname}</setname><parent>mra-parent-{setname}</parent><rbf>ArkanoidCore</rbf><rom zip=\"arkanoid.zip\"><part>00</part></rom></misterromdescription>"
                ),
            )
            .unwrap();
        }
        let mut report = FastSourceSystemReport::default();
        let scan = scan_arcade(&root, &mut report).unwrap();
        assert_eq!(scan.games.len(), 1);
        assert_eq!(scan.variants.len(), 3);
        assert_eq!(report.family_resolved, 4);
        let _ = fs::remove_dir_all(root);
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
    fn amigavision_scan_rejects_oversized_installed_listing() {
        let root = crate::test_support::unique_temp_dir("fast-source-amiga-listing-limit");
        fs::create_dir_all(root.join("games/Amiga/listings")).unwrap();
        fs::write(root.join("games/Amiga/AmigaVision.hdf"), b"hdf").unwrap();
        let listing = fs::File::create(root.join("games/Amiga/listings/games.txt")).unwrap();
        listing
            .set_len(MAX_COLLECTION_LISTING_BYTES as u64 + 1)
            .unwrap();
        let mut report = FastSourceSystemReport::default();

        let error = scan_amiga(&root, &mut report).expect_err("oversized listing must fail closed");

        assert!(error.contains(crate::catalog_progress::CATALOG_SAFETY_LIMIT_NONRETRYABLE));
        assert!(error.contains("observed="));
        assert!(error.contains("configured="));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_scan_accepts_legacy_listing_bytes() {
        let root = crate::test_support::unique_temp_dir("fast-source-amiga-legacy-text");
        fs::create_dir_all(root.join("games/Amiga/listings")).unwrap();
        fs::write(root.join("games/Amiga/AmigaVision.hdf"), b"hdf").unwrap();
        fs::write(
            root.join("games/Amiga/listings/demos.txt"),
            b"State of the Art\nLegacy \xff title\n",
        )
        .unwrap();
        let mut report = FastSourceSystemReport::default();

        let games = scan_amiga(&root, &mut report).expect("lossy listing scan");

        assert!(
            games
                .iter()
                .any(|game| game.title == "Legacy \u{fffd} title")
        );
        let _ = fs::remove_dir_all(root);
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
        assert_eq!(FAST_SOURCE_ADAPTER_VERSION, 14);
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

    #[test]
    fn neogeo_family_audit_reports_installed_projection() {
        let root = crate::test_support::unique_temp_dir("fast-neogeo-family-audit");
        let database = root.join("mister-magik/mame.sqlite3");
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        crate::test_support::write_mame_fixture_db(
            &database,
            &[
                ("mslug3", None, "Metal Slug 3", None, None),
                (
                    "mslug3j",
                    Some("mslug3"),
                    "Metal Slug 3 (Japan)",
                    None,
                    None,
                ),
            ],
        );
        fs::create_dir_all(root.join("games/NEOGEO")).unwrap();
        fs::write(root.join("games/NEOGEO/Metal Slug 3 (mslug3).neo"), b"rom").unwrap();
        fs::write(root.join("games/NEOGEO/Metal Slug 3 (mslug3j).neo"), b"rom").unwrap();

        let report = audit_installed_neogeo_families(&root).unwrap();

        assert!(report.valid);
        assert!(report.text.contains(
            "neogeo_family_summary_tsv\tvalid=1\traw_deduplicated=2\tinstalled_games=2\tvisible_games=1\tvariants=1\tfamilies=1\tmame=2"
        ));
        assert!(
            report
                .text
                .contains("neogeo_family_row_tsv\tstatus=visible")
        );
        assert!(
            report
                .text
                .contains("neogeo_family_row_tsv\tstatus=variant")
        );
        assert!(report.text.contains("\tfamily_id=mslug3\t"));
    }

    #[test]
    fn neogeo_audit_distinguishes_strong_and_weak_unresolved_identities() {
        let root = crate::test_support::unique_temp_dir("fast-neogeo-family-strength");
        let directory = root.join("games/NEOGEO");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("unresolved.zip"), b"rom").unwrap();
        fs::write(directory.join("unresolved.neo"), b"rom").unwrap();

        let report = audit_installed_neogeo_families(&root).unwrap();

        assert!(!report.valid);
        assert!(report.text.contains("strong_unresolved=1"));
        assert!(report.text.contains("unresolved=2"));
        assert!(report.text.contains("identity_strength=strong"));
        assert!(report.text.contains("identity_strength=weak"));
        let _ = fs::remove_dir_all(root);
    }
}
