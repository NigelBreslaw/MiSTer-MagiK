// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fast catalog construction for ordinary user-managed ROM directories.
//!
//! Unlike prepared-collection builders, this scanner assumes no fixed release
//! or directory manifest. It discovers launchable payloads from the installed
//! core launch profiles, walks arbitrary nesting, and reads ZIP directories
//! without extracting payload data.

use crate::catalog_discovery::{GameDirFact, GameDirHeader, GameDirSignature};
use crate::catalog_scan::{FoundFile, scan_zip_central_directory, should_ignore_path};
use crate::fast_five_catalog::{FastFiveSnapshot, FastFiveSystem, GENERIC_EXAMPLE_SYSTEM_IDS};
use crate::launch_profiles::{
    BorrowedProfilePathClass, CatalogScanPlan, IgnoreReason, IgnoreRule, LaunchProfile, MountKind,
    MountSpec, PayloadDisposition, PayloadRule, ProfilePathClass, ProfileSet, RuleProvenance,
};
use crate::namespace_walk::{
    self, NamespaceEntryKind, NamespaceSignatureCapture, NamespaceWalkStats,
};
use crate::system_shard::{SystemGame, SystemLaunchPlan};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize)]
pub struct GenericSystemScanReport {
    pub elapsed_us: u64,
    pub games: usize,
    pub systems: Vec<GenericSystemStats>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct GenericSystemStats {
    pub system_id: String,
    pub roots: usize,
    pub directories: usize,
    pub files: usize,
    pub candidate_files: usize,
    pub archive_members: usize,
    pub games: usize,
    pub ignored_files: usize,
    pub dependency_files: usize,
    pub unmatched_files: usize,
    pub namespace_backend: String,
    pub namespace_read_calls: usize,
    pub namespace_read_bytes: u64,
    pub namespace_type_stats: usize,
    pub inventory_roots: usize,
    pub inventory_entries: usize,
    pub archive_opens: usize,
    pub read_errors: usize,
    pub archive_errors: usize,
    pub elapsed_us: u64,
}

#[derive(Debug)]
struct GenericNamespaceInventory {
    fact: GameDirFact,
    entries: Vec<GenericInventoryEntry>,
    namespace: NamespaceWalkStats,
    watch: GenericSourceWatchObservations,
    continuation_roots: Vec<PathBuf>,
    elapsed_us: u64,
}

#[derive(Debug)]
struct GenericInventoryEntry {
    path: PathBuf,
    kind: NamespaceEntryKind,
    zip_signature: Option<(u64, i64)>,
}

#[derive(Debug, Default)]
struct GenericSystemAccumulator {
    profiles: Vec<LaunchProfile>,
    stats: GenericSystemStats,
    games: Vec<ScannedGame>,
    watch: GenericSourceWatchObservations,
}

type GenericSystemPlanDiscovery = (
    Vec<FastFiveSystem>,
    GenericSystemScanReport,
    Vec<LaunchProfile>,
    BTreeMap<String, GenericSourceWatchObservations>,
);

#[derive(Clone, Debug, Default)]
pub(crate) struct GenericSourceWatchObservations {
    pub(crate) roots: BTreeSet<String>,
    pub(crate) directories: Vec<GenericWatchedDirectoryObservation>,
    pub(crate) containers: Vec<PathBuf>,
    pub(crate) complete: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct GenericWatchedDirectoryObservation {
    pub(crate) path: PathBuf,
    pub(crate) modified_ns: i128,
    pub(crate) entry_fingerprint: String,
}

#[derive(Debug)]
pub(crate) struct PreparedExtensionInventory {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) files_visited: usize,
    pub(crate) watch: GenericSourceWatchObservations,
}

#[derive(Debug, Default)]
struct GenericDirectoryObservationBuilder {
    signature: Option<(u64, i64)>,
    entries: Vec<(String, u8)>,
}

/// Inventory a prepared collection whose owned payload lies below named
/// first-level directories. This keeps prepared row discovery and refresh
/// watch generation on the same serial filesystem traversal.
pub(crate) fn inventory_prepared_extension_under_named_roots(
    root: &Path,
    root_name_marker: &str,
    extension: &str,
) -> Result<PreparedExtensionInventory, String> {
    if !root.is_dir() {
        return Ok(PreparedExtensionInventory {
            files: Vec::new(),
            files_visited: 0,
            watch: GenericSourceWatchObservations::default(),
        });
    }
    let marker = root_name_marker.to_ascii_lowercase();
    let mut files = Vec::new();
    let mut files_visited = 0usize;
    let mut watch_complete = true;
    let mut watch_builders = BTreeMap::<PathBuf, GenericDirectoryObservationBuilder>::new();
    watch_builders.insert(
        root.to_path_buf(),
        GenericDirectoryObservationBuilder::default(),
    );
    let ignore = |path: &Path| {
        should_ignore_path(path)
            || (path.parent() == Some(root)
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.to_ascii_lowercase().contains(&marker)))
    };
    let namespace = namespace_walk::visit_with_signature_capture(
        root,
        None,
        NamespaceSignatureCapture::AllDirectories,
        ignore,
        |entry| {
            files_visited = files_visited.saturating_add(1);
            crate::catalog_progress::report_inner_progress_at(files_visited);
            if let (Some(parent), Some(name)) = (entry.path.parent(), entry.path.file_name()) {
                let kind = match entry.kind {
                    NamespaceEntryKind::Directory => b'd',
                    NamespaceEntryKind::File => b'f',
                    NamespaceEntryKind::Other => {
                        watch_complete = false;
                        b'o'
                    }
                };
                watch_builders
                    .entry(parent.to_path_buf())
                    .or_default()
                    .entries
                    .push((name.to_string_lossy().into_owned(), kind));
            }
            if entry.kind == NamespaceEntryKind::Directory {
                watch_builders
                    .entry(entry.path.clone())
                    .or_default()
                    .signature = entry.directory_signature;
            } else if entry.kind == NamespaceEntryKind::File
                && entry
                    .path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case(extension))
            {
                files.push(entry.path.clone());
            }
            true
        },
    );
    if namespace.errors > 0 {
        return Err(format!(
            "incomplete {} prepared inventory: {} directory errors",
            root.display(),
            namespace.errors
        ));
    }
    if let Some(root_builder) = watch_builders.get_mut(root) {
        root_builder.signature = namespace.target_signature;
    }
    let mut directories = Vec::with_capacity(watch_builders.len());
    for (path, mut builder) in watch_builders {
        let Some((_, modified_ns)) = builder.signature else {
            watch_complete = false;
            continue;
        };
        builder
            .entries
            .sort_by_cached_key(|(name, _)| (name.to_ascii_lowercase(), name.clone()));
        let mut digest = Sha256::new();
        for (name, kind) in builder.entries {
            digest.update([kind]);
            digest.update(name.as_bytes());
            digest.update([0]);
        }
        directories.push(GenericWatchedDirectoryObservation {
            path,
            modified_ns: i128::from(modified_ns),
            entry_fingerprint: hex_lower(&digest.finalize()),
        });
    }
    directories.sort_by(|left, right| left.path.cmp(&right.path));
    files.sort_by_cached_key(|path| path.to_string_lossy().to_ascii_lowercase());
    Ok(PreparedExtensionInventory {
        files,
        files_visited,
        watch: GenericSourceWatchObservations {
            roots: BTreeSet::from([root.to_string_lossy().into_owned()]),
            directories,
            containers: Vec::new(),
            complete: watch_complete,
        },
    })
}

pub const MEDIA_EXPERIMENT_SYSTEM_IDS: [&str; 3] = ["psx", "bbcmicro", "msx"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenericScanImplementation {
    Baseline,
    NamespaceBorrowed,
}

impl GenericScanImplementation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::NamespaceBorrowed => "namespace-borrowed",
        }
    }
}

/// Run the ordinary recursive scanner against the five prepared-source trees.
///
/// This is an A/B benchmark baseline, not an alternate publication path. It
/// deliberately has no release manifest or precomputed file inventory.
pub fn scan_prepared_system_with_generic_walker(
    storage_root: &Path,
    system_id: &str,
) -> Result<(FastFiveSystem, GenericSystemStats), String> {
    let profiles = ProfileSet::all();
    let profile = match system_id {
        "arcade" => profiles
            .profiles()
            .iter()
            .find(|profile| profile.id == "mra")
            .cloned(),
        "amiga" => profiles
            .profiles()
            .iter()
            .find(|profile| profile.id == "amiga")
            .cloned(),
        "dos" => profiles
            .profiles()
            .iter()
            .find(|profile| profile.id == "dos")
            .cloned(),
        "x68000" => profiles
            .profiles()
            .iter()
            .find(|profile| profile.id == "neon68k")
            .cloned(),
        "c64" => Some(generic_c64_baseline_profile()),
        _ => return Err(format!("unsupported prepared generic baseline {system_id}")),
    }
    .ok_or_else(|| format!("generic baseline profile is missing for {system_id}"))?;
    let roots = match system_id {
        "arcade" => vec![storage_root.join("_Arcade")],
        "amiga" => vec![storage_root.join("games/Amiga")],
        "dos" => vec![storage_root.join("_DOS Games")],
        "x68000" => vec![
            storage_root.join("_Computer/_X68000 Games"),
            storage_root.join("_Computer/X68000 Games"),
        ],
        "c64" => vec![storage_root.join("games/C64")],
        _ => unreachable!(),
    };
    let started = Instant::now();
    let mut stats = GenericSystemStats {
        system_id: system_id.to_string(),
        ..GenericSystemStats::default()
    };
    let mut scanned = Vec::new();
    let mut visited_roots = BTreeSet::new();
    for candidate in roots {
        if !candidate.is_dir() {
            continue;
        }
        let root = candidate.canonicalize().unwrap_or(candidate);
        if visited_roots.insert(root.to_string_lossy().to_ascii_lowercase()) {
            stats.roots += 1;
            scan_directory(&root, &profile, &mut stats, &mut scanned);
        }
    }
    if system_id == "c64" {
        scanned.retain(|row| {
            row.game
                .launch_ref
                .to_ascii_lowercase()
                .contains("oneload64")
        });
    }
    scanned.sort_by_cached_key(|row| {
        (
            row.game.title.to_ascii_lowercase(),
            row.game.stable_key.clone(),
        )
    });
    scanned.dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);
    stats.games = scanned.len();
    stats.elapsed_us = started.elapsed().as_micros() as u64;
    Ok((
        FastFiveSystem {
            system_id: system_id.to_string(),
            display_title: display_title(system_id).to_string(),
            games: scanned.into_iter().map(|row| row.game).collect(),
            variants: Vec::new(),
        },
        stats,
    ))
}

pub(crate) fn generic_c64_baseline_profile() -> LaunchProfile {
    LaunchProfile {
        id: "generic-c64-baseline".to_string(),
        system_id: "c64".to_string(),
        category: "Computer".to_string(),
        title: "Commodore 64".to_string(),
        core_name: "C64".to_string(),
        core_path: Some("_Computer/C64".to_string()),
        game_dirs: vec!["C64".to_string()],
        payload_rules: vec![PayloadRule {
            extensions: vec!["crt".to_string()],
            mount: MountSpec::load_file(1),
            disposition: PayloadDisposition::Playable,
            provenance: RuleProvenance::conf_str(
                "Generic A/B baseline treats installed C64 cartridges as primary games",
            ),
        }],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::conf_str(
            "Generic A/B baseline for arbitrary C64 cartridge directories",
        ),
    }
}

pub fn rebuild_generic_system(
    storage_root: &Path,
    system_id: &str,
) -> Result<(FastFiveSystem, GenericSystemStats), String> {
    rebuild_installed_generic_system(storage_root, system_id)?
        .ok_or_else(|| format!("generic system {system_id} has no installed launchable content"))
}

pub fn rebuild_installed_generic_system(
    storage_root: &Path,
    system_id: &str,
) -> Result<Option<(FastFiveSystem, GenericSystemStats)>, String> {
    let roots = [storage_root.display().to_string()];
    let profiles = ProfileSet::try_for_roots(&roots)?
        .into_profiles()
        .into_iter()
        .filter(|profile| profile.system_id == system_id)
        .collect::<Vec<_>>();
    rebuild_generic_system_from_profiles(storage_root, system_id, &profiles)
}

/// Discover every installed profile-backed system without assuming a fixed
/// console or computer list.
///
/// Prepared collections are merged by the independent source layer after this
/// pass, so ordinary user-managed files remain the fallback for every system.
pub fn discover_generic_systems(
    storage_root: &Path,
) -> Result<(Vec<FastFiveSystem>, GenericSystemScanReport), String> {
    discover_generic_systems_excluding(storage_root, &[])
}

pub fn discover_generic_systems_excluding(
    storage_root: &Path,
    excluded_system_ids: &[&str],
) -> Result<(Vec<FastFiveSystem>, GenericSystemScanReport), String> {
    discover_generic_systems_excluding_with_progress(storage_root, excluded_system_ids, |_| {})
}

pub fn discover_generic_systems_excluding_with_progress(
    storage_root: &Path,
    excluded_system_ids: &[&str],
    mut system_complete: impl FnMut(&str),
) -> Result<(Vec<FastFiveSystem>, GenericSystemScanReport), String> {
    let roots = [storage_root.display().to_string()];
    let profiles = ProfileSet::try_for_roots(&roots)?.into_profiles();
    discover_generic_systems_from_profiles_excluding_with_progress(
        storage_root,
        &profiles,
        excluded_system_ids,
        |system| system_complete(&system.system_id),
    )
}

pub(crate) fn discover_generic_systems_from_profiles_excluding_with_progress(
    storage_root: &Path,
    profiles: &[LaunchProfile],
    excluded_system_ids: &[&str],
    mut system_complete: impl FnMut(&FastFiveSystem),
) -> Result<(Vec<FastFiveSystem>, GenericSystemScanReport), String> {
    let started = Instant::now();
    let mut grouped = BTreeMap::<String, Vec<LaunchProfile>>::new();
    for profile in profiles.iter().cloned() {
        if excluded_system_ids.contains(&profile.system_id.as_str()) {
            continue;
        }
        grouped
            .entry(profile.system_id.clone())
            .or_default()
            .push(profile);
    }
    let mut systems = Vec::new();
    let mut reports = Vec::new();
    for (system_id, profiles) in grouped {
        let system_started = Instant::now();
        if let Some((system, report)) =
            rebuild_generic_system_from_profiles(storage_root, &system_id, &profiles)?
        {
            crate::catalog_logln!(
                "fast_catalog_source_tsv\tadapter=generic\tsystem={}\telapsed_us={}\tfiles={}\tdirectories={}\tarchive_members={}\tgames={}\tread_errors={}\tarchive_errors={}",
                report.system_id,
                report.elapsed_us,
                report.files,
                report.directories,
                report.archive_members,
                report.games,
                report.read_errors,
                report.archive_errors,
            );
            system_complete(&system);
            systems.push(system);
            reports.push(report);
        } else {
            crate::catalog_logln!(
                "fast_catalog_source_tsv\tadapter=generic\tsystem={}\telapsed_us={}\tstatus=absent",
                system_id,
                system_started.elapsed().as_micros(),
            );
        }
    }
    systems.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    reports.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    Ok((
        systems,
        GenericSystemScanReport {
            elapsed_us: started.elapsed().as_micros() as u64,
            games: reports.iter().map(|system| system.games).sum(),
            systems: reports,
        },
    ))
}

/// Discover generic systems while sharing each runtime directory traversal
/// between profile resolution and row construction.
///
/// Known profile roots are streamed directly. Runtime-derived roots are
/// retained only for the duration of that root, resolved from the same entries,
/// and then classified without reopening the directory tree.
pub(crate) fn discover_generic_systems_from_plan_excluding_with_progress(
    storage_root: &Path,
    plan: &CatalogScanPlan,
    excluded_system_ids: &[&str],
    mut system_complete: impl FnMut(&FastFiveSystem),
) -> Result<GenericSystemPlanDiscovery, String> {
    let started = Instant::now();
    let mut known_profile_us = 0_u64;
    let mut runtime_inventory_us = 0_u64;
    let mut runtime_resolution_us = 0_u64;
    let mut continuation_us = 0_u64;
    let mut known_roots_considered = 0_usize;
    let mut known_roots_found = 0_usize;
    let mut runtime_headers = 0_usize;
    let mut runtime_resolved = 0_usize;
    let mut runtime_unresolved = 0_usize;
    let mut continuation_root_count = 0_usize;
    let mut profiles = plan.base_profiles().to_vec();
    let mut accumulators = BTreeMap::<String, GenericSystemAccumulator>::new();
    let mut visited_roots = BTreeSet::new();

    for profile in plan.base_profiles() {
        if excluded_system_ids.contains(&profile.system_id.as_str()) {
            continue;
        }
        for game_dir in &profile.game_dirs {
            known_roots_considered = known_roots_considered.saturating_add(1);
            let known_started = Instant::now();
            let candidate = storage_root.join("games").join(game_dir);
            if !candidate.is_dir() {
                known_profile_us =
                    known_profile_us.saturating_add(known_started.elapsed().as_micros() as u64);
                continue;
            }
            known_roots_found = known_roots_found.saturating_add(1);
            let root_key = candidate.to_string_lossy().to_ascii_lowercase();
            if !visited_roots.insert(root_key) {
                known_profile_us =
                    known_profile_us.saturating_add(known_started.elapsed().as_micros() as u64);
                continue;
            }
            let header = GameDirHeader {
                name: game_dir.clone(),
                signature: GameDirSignature::from_path(&candidate),
                path: candidate,
            };
            let inventory = collect_generic_namespace_inventory(&header, None)?;
            let accumulator = accumulator_for_profile(&mut accumulators, profile);
            accumulator.stats.roots = accumulator.stats.roots.saturating_add(1);
            apply_generic_namespace_inventory(
                inventory,
                profile,
                &mut accumulator.stats,
                &mut accumulator.games,
                &mut accumulator.watch,
                true,
            );
            known_profile_us =
                known_profile_us.saturating_add(known_started.elapsed().as_micros() as u64);
        }
    }

    for header in plan.game_dir_headers() {
        runtime_headers = runtime_headers.saturating_add(1);
        let inventory_started = Instant::now();
        let mut inventory = collect_generic_namespace_inventory(header, Some(2))?;
        runtime_inventory_us =
            runtime_inventory_us.saturating_add(inventory_started.elapsed().as_micros() as u64);
        let resolution_started = Instant::now();
        let Some(profile) = plan.profile_for_game_dir_facts(&inventory.fact) else {
            runtime_unresolved = runtime_unresolved.saturating_add(1);
            runtime_resolution_us = runtime_resolution_us
                .saturating_add(resolution_started.elapsed().as_micros() as u64);
            continue;
        };
        runtime_resolved = runtime_resolved.saturating_add(1);
        merge_resolved_profile(&mut profiles, profile.clone());
        runtime_resolution_us =
            runtime_resolution_us.saturating_add(resolution_started.elapsed().as_micros() as u64);
        if excluded_system_ids.contains(&profile.system_id.as_str()) {
            continue;
        }
        let root_key = inventory.fact.path.to_string_lossy().to_ascii_lowercase();
        if !visited_roots.insert(root_key) {
            continue;
        }
        let continuation_roots = std::mem::take(&mut inventory.continuation_roots);
        let accumulator = accumulator_for_profile(&mut accumulators, &profile);
        accumulator.stats.roots = accumulator.stats.roots.saturating_add(1);
        apply_generic_namespace_inventory(
            inventory,
            &profile,
            &mut accumulator.stats,
            &mut accumulator.games,
            &mut accumulator.watch,
            true,
        );
        for continuation_root in continuation_roots {
            continuation_root_count = continuation_root_count.saturating_add(1);
            let continuation_started = Instant::now();
            let continuation_header = GameDirHeader {
                name: header.name.clone(),
                signature: GameDirSignature::from_path(&continuation_root),
                path: continuation_root,
            };
            let mut continuation = collect_generic_namespace_inventory(&continuation_header, None)?;
            continuation.watch.roots.clear();
            apply_generic_namespace_inventory(
                continuation,
                &profile,
                &mut accumulator.stats,
                &mut accumulator.games,
                &mut accumulator.watch,
                false,
            );
            continuation_us =
                continuation_us.saturating_add(continuation_started.elapsed().as_micros() as u64);
        }
    }

    let finalization_started = Instant::now();
    let mut systems = Vec::new();
    let mut reports = Vec::new();
    let mut watch_observations = BTreeMap::new();
    for (system_id, mut accumulator) in accumulators {
        if accumulator.stats.roots == 0 {
            continue;
        }
        accumulator.games.sort_by_cached_key(|row| {
            (
                row.game.title.to_ascii_lowercase(),
                row.game.stable_key.clone(),
            )
        });
        accumulator
            .games
            .dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);
        accumulator.stats.games = accumulator.games.len();
        if accumulator.stats.read_errors > 0 {
            return Err(format!(
                "incomplete {system_id} scan: {} directory errors",
                accumulator.stats.read_errors
            ));
        }
        crate::catalog_logln!(
            "fast_catalog_source_tsv\tadapter=generic-one-pass\tsystem={}\telapsed_us={}\tfiles={}\tdirectories={}\tarchive_members={}\tgames={}\tread_errors={}\tarchive_errors={}",
            system_id,
            accumulator.stats.elapsed_us,
            accumulator.stats.files,
            accumulator.stats.directories,
            accumulator.stats.archive_members,
            accumulator.stats.games,
            accumulator.stats.read_errors,
            accumulator.stats.archive_errors,
        );
        let display_title = accumulator
            .profiles
            .iter()
            .map(|profile| profile.title.trim())
            .find(|title| !title.is_empty())
            .unwrap_or_else(|| display_title(&system_id))
            .to_string();
        let system = FastFiveSystem {
            system_id: system_id.clone(),
            display_title,
            games: accumulator.games.into_iter().map(|row| row.game).collect(),
            variants: Vec::new(),
        };
        system_complete(&system);
        systems.push(system);
        reports.push(accumulator.stats);
        watch_observations.insert(system_id, accumulator.watch);
    }
    let finalization_us = finalization_started.elapsed().as_micros() as u64;
    systems.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    reports.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    let elapsed_us = started.elapsed().as_micros() as u64;
    let accounted_us = known_profile_us
        .saturating_add(runtime_inventory_us)
        .saturating_add(runtime_resolution_us)
        .saturating_add(continuation_us)
        .saturating_add(finalization_us);
    crate::catalog_logln!(
        "fast_catalog_generic_phase_tsv\telapsed_us={}\tknown_profile_us={}\truntime_inventory_us={}\truntime_resolution_us={}\tcontinuation_us={}\tfinalization_us={}\tresidual_us={}\tknown_roots_considered={}\tknown_roots_found={}\truntime_headers={}\truntime_resolved={}\truntime_unresolved={}\tcontinuation_roots={}",
        elapsed_us,
        known_profile_us,
        runtime_inventory_us,
        runtime_resolution_us,
        continuation_us,
        finalization_us,
        elapsed_us.saturating_sub(accounted_us),
        known_roots_considered,
        known_roots_found,
        runtime_headers,
        runtime_resolved,
        runtime_unresolved,
        continuation_root_count,
    );
    Ok((
        systems,
        GenericSystemScanReport {
            elapsed_us,
            games: reports.iter().map(|system| system.games).sum(),
            systems: reports,
        },
        profiles,
        watch_observations,
    ))
}

fn accumulator_for_profile<'a>(
    accumulators: &'a mut BTreeMap<String, GenericSystemAccumulator>,
    profile: &LaunchProfile,
) -> &'a mut GenericSystemAccumulator {
    let accumulator = accumulators
        .entry(profile.system_id.clone())
        .or_insert_with(|| GenericSystemAccumulator {
            stats: GenericSystemStats {
                system_id: profile.system_id.clone(),
                ..GenericSystemStats::default()
            },
            watch: GenericSourceWatchObservations {
                complete: true,
                ..GenericSourceWatchObservations::default()
            },
            ..GenericSystemAccumulator::default()
        });
    if !accumulator
        .profiles
        .iter()
        .any(|existing| existing.id == profile.id)
    {
        accumulator.profiles.push(profile.clone());
    }
    accumulator
}

fn merge_resolved_profile(profiles: &mut Vec<LaunchProfile>, profile: LaunchProfile) {
    if let Some(existing) = profiles
        .iter_mut()
        .find(|existing| existing.id == profile.id)
    {
        for game_dir in profile.game_dirs {
            if !existing
                .game_dirs
                .iter()
                .any(|existing_dir| existing_dir.eq_ignore_ascii_case(&game_dir))
            {
                existing.game_dirs.push(game_dir);
            }
        }
    } else {
        profiles.push(profile);
    }
}

fn collect_generic_namespace_inventory(
    header: &GameDirHeader,
    max_depth: Option<usize>,
) -> Result<GenericNamespaceInventory, String> {
    let started = Instant::now();
    let canonical_path = header
        .path
        .canonicalize()
        .unwrap_or_else(|_| header.path.clone());
    let header = GameDirHeader {
        name: header.name.clone(),
        signature: header.signature,
        path: canonical_path,
    };
    let mut entries = Vec::new();
    let mut has_payload_files = false;
    let mut has_zip_files = false;
    let mut direct_zip_paths = Vec::new();
    let mut nested_probe_signatures = Vec::new();
    let mut payload_extensions = BTreeSet::new();
    let mut watch_builders = BTreeMap::<PathBuf, GenericDirectoryObservationBuilder>::new();
    watch_builders.insert(
        header.path.clone(),
        GenericDirectoryObservationBuilder::default(),
    );
    let mut watch_containers = Vec::new();
    let mut continuation_roots = Vec::new();
    let mut watch_complete = true;
    let mut walked_entries = 0usize;
    let namespace_started = Instant::now();
    let namespace = namespace_walk::visit_with_signature_capture(
        &header.path,
        max_depth,
        NamespaceSignatureCapture::AllDirectories,
        should_ignore_path,
        |entry| {
            walked_entries = walked_entries.saturating_add(1);
            crate::catalog_progress::report_inner_progress_at(walked_entries);
            let depth = entry
                .path
                .strip_prefix(&header.path)
                .map(|relative| relative.components().count())
                .unwrap_or(usize::MAX);
            if depth <= 2 {
                match entry.kind {
                    NamespaceEntryKind::Directory if depth == 1 => {
                        nested_probe_signatures.push((
                            entry.path.clone(),
                            GameDirSignature::from_namespace_signature(entry.directory_signature),
                        ));
                    }
                    NamespaceEntryKind::File => {
                        if entry
                            .path
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
                        {
                            has_zip_files = true;
                            if depth == 1 {
                                direct_zip_paths.push(entry.path.clone());
                            }
                        } else {
                            has_payload_files = true;
                            if let Some(extension) = entry
                                .path
                                .extension()
                                .and_then(|extension| extension.to_str())
                            {
                                payload_extensions.insert(extension.to_ascii_lowercase());
                            }
                        }
                    }
                    _ => {}
                }
            }
            if max_depth == Some(2) && depth == 2 && entry.kind == NamespaceEntryKind::Directory {
                continuation_roots.push(entry.path.clone());
            }
            if let (Some(parent), Some(name)) = (entry.path.parent(), entry.path.file_name()) {
                let kind = match entry.kind {
                    NamespaceEntryKind::Directory => b'd',
                    NamespaceEntryKind::File => b'f',
                    NamespaceEntryKind::Other => {
                        watch_complete = false;
                        b'o'
                    }
                };
                watch_builders
                    .entry(parent.to_path_buf())
                    .or_default()
                    .entries
                    .push((name.to_string_lossy().into_owned(), kind));
            }
            if entry.kind == NamespaceEntryKind::Directory {
                watch_builders
                    .entry(entry.path.clone())
                    .or_default()
                    .signature = entry.directory_signature;
            } else if entry.kind == NamespaceEntryKind::File
                && entry
                    .path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
            {
                watch_containers.push(entry.path.clone());
            }
            entries.push(GenericInventoryEntry {
                path: entry.path.clone(),
                kind: entry.kind,
                zip_signature: entry.zip_signature,
            });
            true
        },
    );
    let namespace_us = namespace_started.elapsed().as_micros() as u64;
    if namespace.errors > 0 {
        return Err(format!(
            "incomplete {} inventory: {} directory errors",
            header.path.display(),
            namespace.errors
        ));
    }
    if let Some(root) = watch_builders.get_mut(&header.path) {
        root.signature = namespace.target_signature;
    }
    let mut watch_directories = Vec::with_capacity(watch_builders.len());
    for (path, mut builder) in watch_builders {
        let Some((_, modified_ns)) = builder.signature else {
            watch_complete = false;
            continue;
        };
        builder
            .entries
            .sort_by_cached_key(|(name, _)| (name.to_ascii_lowercase(), name.clone()));
        let mut digest = Sha256::new();
        for (name, kind) in builder.entries {
            digest.update([kind]);
            digest.update(name.as_bytes());
            digest.update([0]);
        }
        watch_directories.push(GenericWatchedDirectoryObservation {
            path,
            modified_ns: i128::from(modified_ns),
            entry_fingerprint: hex_lower(&digest.finalize()),
        });
    }
    watch_directories.sort_by(|left, right| left.path.cmp(&right.path));
    watch_containers.sort();
    watch_containers.dedup();
    continuation_roots.sort();
    continuation_roots.dedup();
    direct_zip_paths.sort_by_cached_key(|path| path.to_string_lossy().to_ascii_lowercase());
    nested_probe_signatures.sort_by_cached_key(|(path, _)| {
        (path.to_string_lossy().to_ascii_lowercase(), path.clone())
    });
    let total_us = started.elapsed().as_micros() as u64;
    crate::catalog_logln!(
        "fast_catalog_generic_inventory_tsv\tpath={}\tbackend={}\tentries={}\tnamespace_us={}\tpost_walk_us={}\ttotal_us={}",
        header.path.display(),
        namespace.backend,
        entries.len(),
        namespace_us,
        total_us.saturating_sub(namespace_us),
        total_us,
    );
    Ok(GenericNamespaceInventory {
        fact: GameDirFact {
            name: header.name.clone(),
            path: header.path.clone(),
            signature: header.signature,
            has_payload_files,
            has_zip_files,
            direct_zip_paths,
            nested_probe_signatures,
            payload_extensions,
        },
        entries,
        namespace,
        watch: GenericSourceWatchObservations {
            roots: BTreeSet::from([header.path.to_string_lossy().into_owned()]),
            directories: watch_directories,
            containers: watch_containers,
            complete: watch_complete,
        },
        continuation_roots,
        elapsed_us: total_us,
    })
}

fn apply_generic_namespace_inventory(
    inventory: GenericNamespaceInventory,
    profile: &LaunchProfile,
    stats: &mut GenericSystemStats,
    games: &mut Vec<ScannedGame>,
    watch: &mut GenericSourceWatchObservations,
    counts_as_root: bool,
) {
    let classify_started = Instant::now();
    merge_watch_observations(watch, &inventory.watch);
    stats.elapsed_us = stats.elapsed_us.saturating_add(inventory.elapsed_us);
    if counts_as_root {
        stats.inventory_roots = stats.inventory_roots.saturating_add(1);
    }
    stats.inventory_entries = stats
        .inventory_entries
        .saturating_add(inventory.entries.len());
    stats.directories = stats.directories.saturating_add(1);
    merge_namespace_stats(stats, &inventory.namespace);
    for (position, entry) in inventory.entries.into_iter().enumerate() {
        crate::catalog_progress::report_inner_progress_at(position.saturating_add(1));
        if entry.kind == NamespaceEntryKind::Directory {
            stats.directories = stats.directories.saturating_add(1);
            continue;
        }
        if entry.kind != NamespaceEntryKind::File {
            continue;
        }
        stats.files = stats.files.saturating_add(1);
        match profile.classify_path_borrowed(&entry.path) {
            BorrowedProfilePathClass::Payload { rule }
                if rule.disposition == PayloadDisposition::Playable =>
            {
                stats.candidate_files = stats.candidate_files.saturating_add(1);
                games.push(direct_game(profile, &entry.path, rule));
            }
            BorrowedProfilePathClass::NotMatched
                if entry
                    .path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
                    && !profile.archive_entry_rules.is_empty() =>
            {
                stats.candidate_files = stats.candidate_files.saturating_add(1);
                scan_archive_with_signature(
                    profile,
                    &entry.path,
                    entry.zip_signature,
                    stats,
                    games,
                );
            }
            BorrowedProfilePathClass::Ignored { reason } => {
                stats.ignored_files = stats.ignored_files.saturating_add(1);
                if reason == IgnoreReason::CueTrack {
                    stats.dependency_files = stats.dependency_files.saturating_add(1);
                }
            }
            BorrowedProfilePathClass::Payload { .. } => {
                stats.dependency_files = stats.dependency_files.saturating_add(1);
            }
            _ => stats.unmatched_files = stats.unmatched_files.saturating_add(1),
        }
    }
    stats.elapsed_us = stats
        .elapsed_us
        .saturating_add(classify_started.elapsed().as_micros() as u64);
}

fn merge_watch_observations(
    target: &mut GenericSourceWatchObservations,
    source: &GenericSourceWatchObservations,
) {
    target.complete &= source.complete;
    target.roots.extend(source.roots.iter().cloned());
    let mut directories = target
        .directories
        .drain(..)
        .map(|directory| (directory.path.clone(), directory))
        .collect::<BTreeMap<_, _>>();
    directories.extend(
        source
            .directories
            .iter()
            .cloned()
            .map(|directory| (directory.path.clone(), directory)),
    );
    target.directories = directories.into_values().collect();
    target.containers.extend(source.containers.iter().cloned());
    target.containers.sort();
    target.containers.dedup();
}

fn merge_namespace_stats(stats: &mut GenericSystemStats, namespace: &NamespaceWalkStats) {
    stats.namespace_backend = if stats.namespace_backend.is_empty() {
        namespace.backend.to_string()
    } else if stats.namespace_backend == namespace.backend {
        stats.namespace_backend.clone()
    } else {
        "mixed".to_string()
    };
    stats.namespace_read_calls = stats
        .namespace_read_calls
        .saturating_add(namespace.read_calls);
    stats.namespace_read_bytes = stats
        .namespace_read_bytes
        .saturating_add(namespace.read_bytes);
    stats.namespace_type_stats = stats
        .namespace_type_stats
        .saturating_add(namespace.type_stats);
    stats.read_errors = stats.read_errors.saturating_add(namespace.errors);
}

pub fn discover_generic_system_ids(storage_root: &Path) -> BTreeSet<String> {
    let roots = [storage_root.display().to_string()];
    ProfileSet::for_roots(&roots)
        .into_profiles()
        .into_iter()
        .filter(|profile| {
            profile
                .game_dirs
                .iter()
                .any(|game_dir| storage_root.join("games").join(game_dir).is_dir())
        })
        .map(|profile| profile.system_id)
        .collect()
}

fn rebuild_generic_system_from_profiles(
    storage_root: &Path,
    system_id: &str,
    profiles: &[LaunchProfile],
) -> Result<Option<(FastFiveSystem, GenericSystemStats)>, String> {
    if profiles.is_empty() {
        return Ok(None);
    }
    let started = Instant::now();
    let mut stats = GenericSystemStats {
        system_id: system_id.to_string(),
        ..GenericSystemStats::default()
    };
    let mut scanned = Vec::new();
    let mut visited_roots = BTreeSet::new();
    for profile in profiles {
        for game_dir in &profile.game_dirs {
            let candidate = storage_root.join("games").join(game_dir);
            if !candidate.is_dir() {
                continue;
            }
            let root = candidate.canonicalize().unwrap_or(candidate);
            if visited_roots.insert(root.to_string_lossy().to_ascii_lowercase()) {
                stats.roots += 1;
                scan_namespace_borrowed(&root, profile, &mut stats, &mut scanned);
            }
        }
    }
    if stats.roots == 0 {
        return Ok(None);
    }
    scanned.sort_by(|left, right| {
        left.game
            .title
            .to_ascii_lowercase()
            .cmp(&right.game.title.to_ascii_lowercase())
            .then_with(|| left.game.stable_key.cmp(&right.game.stable_key))
    });
    scanned.dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);
    stats.games = scanned.len();
    stats.elapsed_us = started.elapsed().as_micros() as u64;
    if stats.read_errors > 0 {
        return Err(format!(
            "incomplete {system_id} scan: {} directory errors",
            stats.read_errors
        ));
    }
    Ok(Some((
        FastFiveSystem {
            system_id: system_id.to_string(),
            display_title: profiles
                .iter()
                .map(|profile| profile.title.trim())
                .find(|title| !title.is_empty())
                .unwrap_or_else(|| display_title(system_id))
                .to_string(),
            games: scanned.into_iter().map(|row| row.game).collect(),
            variants: Vec::new(),
        },
        stats,
    )))
}

pub fn scan_media_experiment_system(
    storage_root: &Path,
    system_id: &str,
    implementation: GenericScanImplementation,
) -> Result<(FastFiveSystem, GenericSystemStats), String> {
    if !MEDIA_EXPERIMENT_SYSTEM_IDS.contains(&system_id) {
        return Err(format!("unsupported media experiment system {system_id}"));
    }
    let profiles = focused_media_profiles()?;
    let profile = profiles
        .iter()
        .find(|profile| profile.system_id == system_id)
        .ok_or_else(|| format!("no media experiment profile found for {system_id}"))?;
    if !core_is_installed(storage_root, profile) {
        return Err(format!(
            "no installed core found for media system {system_id}"
        ));
    }

    let started = Instant::now();
    let mut stats = GenericSystemStats {
        system_id: system_id.to_string(),
        ..GenericSystemStats::default()
    };
    let mut scanned = Vec::new();
    let mut visited_roots = BTreeSet::new();
    for game_dir in &profile.game_dirs {
        let candidate = storage_root.join("games").join(game_dir);
        if !candidate.is_dir() {
            continue;
        }
        let root = candidate.canonicalize().unwrap_or(candidate);
        if visited_roots.insert(root.to_string_lossy().to_ascii_lowercase()) {
            stats.roots += 1;
            match implementation {
                GenericScanImplementation::Baseline => {
                    scan_directory(&root, profile, &mut stats, &mut scanned)
                }
                GenericScanImplementation::NamespaceBorrowed => {
                    scan_namespace_borrowed(&root, profile, &mut stats, &mut scanned)
                }
            }
        }
    }
    if stats.roots == 0 {
        return Err(format!("media system {system_id} has no game directory"));
    }

    scanned.sort_by(|left, right| {
        left.game
            .title
            .to_ascii_lowercase()
            .cmp(&right.game.title.to_ascii_lowercase())
            .then_with(|| left.game.stable_key.cmp(&right.game.stable_key))
    });
    scanned.dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);
    stats.games = scanned.len();
    stats.elapsed_us = started.elapsed().as_micros() as u64;
    Ok((
        FastFiveSystem {
            system_id: system_id.to_string(),
            display_title: display_title(system_id).to_string(),
            games: scanned.into_iter().map(|row| row.game).collect(),
            variants: Vec::new(),
        },
        stats,
    ))
}

#[derive(Debug)]
struct ScannedGame {
    game: SystemGame,
    signature: String,
}

/// Replace the four ordinary-filesystem examples in a base snapshot.
///
/// The source snapshot remains independent of the legacy whole-card catalog.
/// Launch profiles are reused only as the core/media contract.
pub fn add_generic_example_systems(
    storage_root: &Path,
    mut snapshot: FastFiveSnapshot,
) -> Result<(FastFiveSnapshot, GenericSystemScanReport), String> {
    snapshot.validate()?;
    let started = Instant::now();
    let profiles = focused_profiles()?;
    let mut systems = Vec::with_capacity(GENERIC_EXAMPLE_SYSTEM_IDS.len());
    let mut all_signatures = Vec::new();
    let mut reports = Vec::with_capacity(GENERIC_EXAMPLE_SYSTEM_IDS.len());

    for system_id in GENERIC_EXAMPLE_SYSTEM_IDS {
        let profile = profiles
            .iter()
            .find(|profile| profile.system_id == system_id)
            .ok_or_else(|| format!("no focused launch profile found for {system_id}"))?;
        if !core_is_installed(storage_root, profile) {
            return Err(format!(
                "no installed launch profile found for generic system {system_id}"
            ));
        }
        let system_started = Instant::now();
        let mut stats = GenericSystemStats {
            system_id: system_id.to_string(),
            ..GenericSystemStats::default()
        };
        let mut scanned = Vec::new();
        let mut visited_roots = BTreeSet::new();
        for game_dir in &profile.game_dirs {
            let candidate = storage_root.join("games").join(game_dir);
            if !candidate.is_dir() {
                continue;
            }
            let root = candidate.canonicalize().unwrap_or(candidate);
            if visited_roots.insert(root.to_string_lossy().to_ascii_lowercase()) {
                stats.roots += 1;
                scan_directory(&root, profile, &mut stats, &mut scanned);
            }
        }
        if stats.roots == 0 {
            return Err(format!(
                "generic system {system_id} has an installed profile but no game directory"
            ));
        }

        scanned.sort_by(|left, right| {
            left.game
                .title
                .to_ascii_lowercase()
                .cmp(&right.game.title.to_ascii_lowercase())
                .then_with(|| left.game.stable_key.cmp(&right.game.stable_key))
        });
        scanned.dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);
        stats.games = scanned.len();
        stats.elapsed_us = system_started.elapsed().as_micros() as u64;
        all_signatures.extend(scanned.iter().map(|row| row.signature.clone()));
        systems.push(FastFiveSystem {
            system_id: system_id.to_string(),
            display_title: display_title(system_id).to_string(),
            games: scanned.into_iter().map(|row| row.game).collect(),
            variants: Vec::new(),
        });
        reports.push(stats);
    }

    snapshot
        .systems
        .retain(|system| !GENERIC_EXAMPLE_SYSTEM_IDS.contains(&system.system_id.as_str()));
    snapshot.systems.extend(systems);
    snapshot
        .systems
        .sort_by(|left, right| left.system_id.cmp(&right.system_id));
    all_signatures.sort();
    let mut fingerprint = Sha256::new();
    fingerprint.update(b"mister-magik-generic-system-scan-v1\0");
    fingerprint.update(snapshot.source_fingerprint.as_bytes());
    for signature in all_signatures {
        fingerprint.update([0]);
        fingerprint.update(signature.as_bytes());
    }
    snapshot.source_fingerprint = hex_lower(&fingerprint.finalize());
    snapshot.validate()?;
    let report = GenericSystemScanReport {
        elapsed_us: started.elapsed().as_micros() as u64,
        games: reports.iter().map(|system| system.games).sum(),
        systems: reports,
    };
    Ok((snapshot, report))
}

fn focused_profiles() -> Result<Vec<LaunchProfile>, String> {
    let all = ProfileSet::all();
    let mut profiles = Vec::with_capacity(GENERIC_EXAMPLE_SYSTEM_IDS.len());
    for (system_id, game_dir) in [("neogeo", "NEOGEO"), ("saturn", "Saturn"), ("snes", "SNES")] {
        let mut profile = all
            .profiles()
            .iter()
            .find(|profile| {
                profile.system_id == system_id
                    && profile
                        .game_dirs
                        .iter()
                        .any(|dir| dir.eq_ignore_ascii_case(game_dir))
            })
            .cloned()
            .ok_or_else(|| format!("built-in launch profile is missing for {system_id}"))?;
        if system_id == "snes" {
            profile.game_dirs.extend(
                ["Satellaview", "SGB2", "SNES-Sinden"]
                    .into_iter()
                    .map(str::to_string),
            );
        }
        profiles.push(profile);
    }
    let spectrum_rule = PayloadRule {
        extensions: ["sna", "szx", "tap", "tzx", "z80"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        mount: MountSpec::load_file(1),
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::conf_str(
            "Focused ZX Spectrum profile uses the established runtime payload contract",
        ),
    };
    profiles.push(LaunchProfile {
        id: "runtime-zx-spectrum".to_string(),
        system_id: "zx-spectrum".to_string(),
        category: "Computer".to_string(),
        title: "ZX Spectrum".to_string(),
        core_name: "ZX-Spectrum".to_string(),
        core_path: Some("_Computer/ZX-Spectrum".to_string()),
        game_dirs: vec!["Spectrum".to_string()],
        payload_rules: vec![spectrum_rule.clone()],
        archive_entry_rules: vec![spectrum_rule],
        collection_rules: Vec::new(),
        ignore_rules: vec![IgnoreRule {
            file_names: vec!["boot.rom".to_string()],
            extensions: Vec::new(),
            reason: IgnoreReason::Bios,
            provenance: RuleProvenance::magik("boot.rom is Spectrum firmware, not a game"),
        }],
        provenance: RuleProvenance::conf_str(
            "Focused generic scanner profile for the ZX-Spectrum core",
        ),
    });
    Ok(profiles)
}

fn focused_media_profiles() -> Result<Vec<LaunchProfile>, String> {
    let all = ProfileSet::all();
    let psx = all
        .profiles()
        .iter()
        .find(|profile| profile.id == "psx")
        .cloned()
        .ok_or_else(|| "built-in PSX launch profile is missing".to_string())?;
    let bbc_rule = PayloadRule {
        extensions: ["adl", "dsd", "sdd", "ssd", "uef"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        mount: MountSpec::load_file(1),
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::conf_str(
            "BBC Micro runtime profile accepts maintained disk and tape payload formats",
        ),
    };
    let msx_rule = PayloadRule {
        extensions: ["rom", "mx1", "mx2"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        mount: MountSpec::load_file(1),
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::conf_str(
            "Focused MSX experiment covers cartridge payloads without filesystem snapshots",
        ),
    };
    Ok(vec![
        psx,
        LaunchProfile {
            id: "focused-bbcmicro".to_string(),
            system_id: "bbcmicro".to_string(),
            category: "Computer".to_string(),
            title: "BBC Micro".to_string(),
            core_name: "BBCMicro".to_string(),
            core_path: Some("_Computer/BBCMicro".to_string()),
            game_dirs: vec!["BBCMicro".to_string()],
            payload_rules: vec![bbc_rule.clone()],
            archive_entry_rules: vec![bbc_rule],
            collection_rules: Vec::new(),
            ignore_rules: Vec::new(),
            provenance: RuleProvenance::conf_str(
                "Focused generic BBC Micro media experiment profile",
            ),
        },
        LaunchProfile {
            id: "focused-msx".to_string(),
            system_id: "msx".to_string(),
            category: "Computer".to_string(),
            title: "MSX".to_string(),
            core_name: "MSX".to_string(),
            core_path: Some("_Computer/MSX".to_string()),
            game_dirs: vec!["MSX".to_string()],
            payload_rules: vec![msx_rule.clone()],
            archive_entry_rules: vec![msx_rule],
            collection_rules: Vec::new(),
            ignore_rules: Vec::new(),
            provenance: RuleProvenance::conf_str(
                "Focused generic MSX cartridge experiment profile",
            ),
        },
    ])
}

fn core_is_installed(storage_root: &Path, profile: &LaunchProfile) -> bool {
    let Some(core_path) = profile.core_path.as_deref() else {
        return false;
    };
    let relative = Path::new(core_path);
    let Some(parent) = relative.parent() else {
        return false;
    };
    let Some(expected) = relative.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Ok(entries) = fs::read_dir(storage_root.join(parent)) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
            && path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| {
                    stem.eq_ignore_ascii_case(expected)
                        || stem
                            .get(..expected.len())
                            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(expected))
                            && stem.as_bytes().get(expected.len()) == Some(&b'_')
                })
    })
}

fn scan_directory(
    root: &Path,
    profile: &LaunchProfile,
    stats: &mut GenericSystemStats,
    games: &mut Vec<ScannedGame>,
) {
    if stats.namespace_backend.is_empty() {
        stats.namespace_backend = "std-read-dir".to_string();
    }
    stats.directories += 1;
    let mut entries = match fs::read_dir(root) {
        Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
        Err(_) => {
            stats.read_errors += 1;
            return;
        }
    };
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    for entry in entries {
        let work_units = stats.files.saturating_add(stats.directories);
        crate::catalog_progress::report_inner_progress_at(work_units);
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                stats.read_errors += 1;
                continue;
            }
        };
        if file_type.is_symlink() {
            continue;
        }
        if should_ignore_path(&path) {
            continue;
        }
        if file_type.is_dir() {
            scan_directory(&path, profile, stats, games);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        stats.files += 1;
        match profile.classify_path(&path) {
            ProfilePathClass::Payload { rule }
                if rule.disposition == PayloadDisposition::Playable =>
            {
                stats.candidate_files += 1;
                games.push(direct_game(profile, &path, &rule));
            }
            ProfilePathClass::NotMatched
                if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
                    && !profile.archive_entry_rules.is_empty() =>
            {
                stats.candidate_files += 1;
                scan_archive(profile, &path, stats, games);
            }
            ProfilePathClass::Ignored { reason, .. } => {
                stats.ignored_files += 1;
                if reason == IgnoreReason::CueTrack {
                    stats.dependency_files += 1;
                }
            }
            ProfilePathClass::Payload { .. } => stats.dependency_files += 1,
            _ => stats.unmatched_files += 1,
        }
    }
}

fn scan_namespace_borrowed(
    root: &Path,
    profile: &LaunchProfile,
    stats: &mut GenericSystemStats,
    games: &mut Vec<ScannedGame>,
) {
    stats.directories += 1;
    crate::catalog_progress::report_inner_progress_at(stats.directories);
    let namespace = namespace_walk::visit(root, None, should_ignore_path, |entry| {
        if entry.kind == NamespaceEntryKind::Directory {
            stats.directories += 1;
            crate::catalog_progress::report_inner_progress_at(stats.directories);
            return true;
        }
        if entry.kind != NamespaceEntryKind::File {
            return true;
        }
        let path = entry.path.as_path();
        stats.files += 1;
        crate::catalog_progress::report_inner_progress_at(stats.files);
        match profile.classify_path_borrowed(path) {
            BorrowedProfilePathClass::Payload { rule }
                if rule.disposition == PayloadDisposition::Playable =>
            {
                stats.candidate_files += 1;
                games.push(direct_game(profile, path, rule));
            }
            BorrowedProfilePathClass::NotMatched
                if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
                    && !profile.archive_entry_rules.is_empty() =>
            {
                stats.candidate_files += 1;
                scan_archive(profile, path, stats, games);
            }
            BorrowedProfilePathClass::Ignored { reason } => {
                stats.ignored_files += 1;
                if reason == IgnoreReason::CueTrack {
                    stats.dependency_files += 1;
                }
            }
            BorrowedProfilePathClass::Payload { .. } => stats.dependency_files += 1,
            _ => stats.unmatched_files += 1,
        }
        true
    });
    stats.namespace_backend = namespace.backend.to_string();
    stats.namespace_read_calls = stats
        .namespace_read_calls
        .saturating_add(namespace.read_calls);
    stats.namespace_read_bytes = stats
        .namespace_read_bytes
        .saturating_add(namespace.read_bytes);
    stats.namespace_type_stats = stats
        .namespace_type_stats
        .saturating_add(namespace.type_stats);
    stats.read_errors = stats.read_errors.saturating_add(namespace.errors);
}

fn direct_game(profile: &LaunchProfile, path: &Path, rule: &PayloadRule) -> ScannedGame {
    let launch_ref = path.to_string_lossy().into_owned();
    let signature = format!("{}\u{1f}{}", profile.system_id, launch_ref);
    ScannedGame {
        game: system_game(profile, path, &launch_ref, rule),
        signature,
    }
}

fn scan_archive(
    profile: &LaunchProfile,
    path: &Path,
    stats: &mut GenericSystemStats,
    games: &mut Vec<ScannedGame>,
) {
    let signature = fs::metadata(path)
        .ok()
        .map(|metadata| (metadata.len(), mtime_secs(&metadata)));
    scan_archive_with_signature(profile, path, signature, stats, games);
}

fn scan_archive_with_signature(
    profile: &LaunchProfile,
    path: &Path,
    signature: Option<(u64, i64)>,
    stats: &mut GenericSystemStats,
    games: &mut Vec<ScannedGame>,
) {
    stats.archive_opens = stats.archive_opens.saturating_add(1);
    let Some((size, mtime_secs)) = signature else {
        stats.read_errors += 1;
        return;
    };
    let found = FoundFile {
        path: path.to_path_buf(),
        ext: "zip".to_string(),
        size,
        mtime_secs,
    };
    match scan_zip_central_directory(&found, profile) {
        Ok(entries) => {
            stats.archive_members += entries.len();
            for (position, entry) in entries.into_iter().enumerate() {
                crate::catalog_progress::report_inner_progress_at(position.saturating_add(1));
                let member_path = PathBuf::from(&entry.entry_path);
                let signature = format!("{}\u{1f}{}", profile.system_id, entry.launch_ref);
                games.push(ScannedGame {
                    game: system_game(profile, &member_path, &entry.launch_ref, &entry.rule),
                    signature,
                });
            }
        }
        Err(_) => stats.archive_errors += 1,
    }
}

fn system_game(
    profile: &LaunchProfile,
    title_path: &Path,
    launch_ref: &str,
    rule: &PayloadRule,
) -> SystemGame {
    let title = display_name(title_path);
    let normalized_title = title.to_ascii_lowercase();
    let core_path = profile
        .core_path
        .clone()
        .unwrap_or_else(|| profile.core_name.clone());
    SystemGame {
        stable_key: format!(
            "{}\u{1f}{}\u{1f}{}",
            profile.system_id, normalized_title, launch_ref
        ),
        title: title.clone(),
        launch_ref: launch_ref.to_string(),
        preview_archive_path: String::new(),
        preview_asset_key: String::new(),
        has_preview: false,
        year: None,
        manufacturer: String::new(),
        category: profile.category.clone(),
        players: None,
        control: String::new(),
        is_new: false,
        launch_plan: Some(SystemLaunchPlan {
            launch_ref: launch_ref.to_string(),
            title,
            system_id: profile.system_id.clone(),
            core_path,
            payload_path: launch_ref.to_string(),
            mount_kind: mount_kind(rule.mount.kind).to_string(),
            mount_index: rule.mount.index,
            delay_secs: rule.mount.delay_secs,
        }),
    }
}

fn display_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("Unknown")
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_title(system_id: &str) -> &'static str {
    match system_id {
        "bbcmicro" => "BBC Micro",
        "msx" => "MSX",
        "neogeo" => "Neo Geo",
        "psx" => "PlayStation",
        "saturn" => "Sega Saturn",
        "snes" => "Super Nintendo",
        "zx-spectrum" => "ZX Spectrum",
        _ => "Games",
    }
}

fn mount_kind(kind: MountKind) -> &'static str {
    match kind {
        MountKind::Launcher => "launcher",
        MountKind::LoadFile => "load-file",
        MountKind::MountImage => "mount-image",
        MountKind::Core => "core",
    }
}

fn mtime_secs(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs() as i64)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_five_catalog::{FAST_FIVE_SNAPSHOT_SCHEMA, FAST_FIVE_SYSTEM_IDS};

    #[test]
    fn one_pass_runtime_inventory_matches_two_pass_rows() {
        let root = crate::test_support::unique_temp_dir("generic-one-pass-parity");
        let core = root.join("_Console/MyBeta_20260828.rbf");
        fs::create_dir_all(core.parent().expect("core parent")).expect("create core parent");
        fs::write(core, b"core").expect("write core");
        for relative in [
            "games/MyBeta/Publisher/First Game.rom",
            "games/MyBeta/Publisher/Nested/Second Game.rom",
            "games/MyBeta/.metadata/Hidden Game.rom",
        ] {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("game parent")).expect("create game parent");
            fs::write(path, b"rom").expect("write game");
        }
        let roots = [root.display().to_string()];
        let profiles = ProfileSet::try_for_roots(&roots)
            .expect("two-pass profiles")
            .into_profiles();
        let (two_pass, _) = discover_generic_systems_from_profiles_excluding_with_progress(
            &root,
            &profiles,
            &[],
            |_| {},
        )
        .expect("two-pass scan");
        let plan = CatalogScanPlan::try_for_roots(&roots).expect("one-pass plan");
        let (one_pass, report, resolved, observations) =
            discover_generic_systems_from_plan_excluding_with_progress(&root, &plan, &[], |_| {})
                .expect("one-pass scan");

        assert_eq!(one_pass, two_pass);
        assert_eq!(
            resolved
                .iter()
                .filter(|profile| profile.system_id == "mybeta")
                .count(),
            1
        );
        let beta = report
            .systems
            .iter()
            .find(|system| system.system_id == "mybeta")
            .expect("runtime report");
        assert_eq!(beta.inventory_roots, 1);
        assert!(beta.inventory_entries >= 4);
        let watch = observations.get("mybeta").expect("runtime watch");
        assert_eq!(watch.roots.len(), 1);
        #[cfg(target_os = "linux")]
        {
            assert!(watch.complete);
            assert!(watch.directories.len() >= 3);
        }
        #[cfg(not(target_os = "linux"))]
        assert!(!watch.complete);
    }

    #[test]
    fn scans_nested_user_collections_without_a_release_manifest() {
        let root = crate::test_support::unique_temp_dir("generic-system-catalog");
        for core in [
            "_Console/SNES_20260826.rbf",
            "_Console/Saturn_20260826.rbf",
            "_Console/NeoGeo_20260826.rbf",
            "_Computer/ZX-Spectrum_20260826.rbf",
        ] {
            let path = root.join(core);
            fs::create_dir_all(path.parent().expect("core parent")).expect("create core parent");
            fs::write(path, b"core").expect("write core");
        }
        let files = [
            ("games/SNES/Publisher/Super Game.sfc", b"rom".as_slice()),
            (
                "games/SNES/Publisher/._Super Game.sfc",
                b"sidecar".as_slice(),
            ),
            ("games/SNES/.metadata/Hidden Game.sfc", b"hidden".as_slice()),
            ("games/Saturn/Disc Game.chd", b"disc".as_slice()),
            ("games/Saturn/Disc Game.bin", b"track".as_slice()),
            ("games/NEOGEO/Arcade Game.neo", b"rom".as_slice()),
            ("games/Spectrum/Tape Game.tzx", b"tape".as_slice()),
        ];
        for (relative, bytes) in files {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("game parent")).expect("create game parent");
            fs::write(path, bytes).expect("write game");
        }
        let base = FastFiveSnapshot {
            schema: FAST_FIVE_SNAPSHOT_SCHEMA.to_string(),
            source_fingerprint: "0".repeat(64),
            systems: FAST_FIVE_SYSTEM_IDS
                .into_iter()
                .map(|system_id| FastFiveSystem {
                    system_id: system_id.to_string(),
                    display_title: system_id.to_string(),
                    games: Vec::new(),
                    variants: Vec::new(),
                })
                .collect(),
        };

        let (snapshot, report) = add_generic_example_systems(&root, base).expect("scan");

        assert_eq!(snapshot.systems.len(), 9);
        assert_eq!(report.games, 4);
        for system_id in GENERIC_EXAMPLE_SYSTEM_IDS {
            let system = snapshot
                .systems
                .iter()
                .find(|system| system.system_id == system_id)
                .expect("generic system");
            assert_eq!(system.games.len(), 1, "{system_id}");
            assert!(system.games[0].launch_plan.is_some());
        }
        let saturn = snapshot
            .systems
            .iter()
            .find(|system| system.system_id == "saturn")
            .expect("saturn");
        assert!(
            saturn
                .games
                .iter()
                .all(|game| !game.launch_ref.ends_with(".bin"))
        );
    }

    #[test]
    fn media_experiment_implementations_have_exact_launch_parity() {
        let root = crate::test_support::unique_temp_dir("generic-media-experiment");
        for core in [
            "_Console/PSX_20260826.rbf",
            "_Computer/BBCMicro_20260826.rbf",
            "_Computer/MSX_20260826.rbf",
        ] {
            let path = root.join(core);
            fs::create_dir_all(path.parent().expect("core parent")).expect("create core parent");
            fs::write(path, b"core").expect("write core");
        }
        for (relative, bytes) in [
            ("games/PSX/Disc Game.chd", b"disc".as_slice()),
            ("games/PSX/._Disc Game.chd", b"sidecar".as_slice()),
            ("games/PSX/.metadata/Hidden Disc.chd", b"hidden".as_slice()),
            ("games/PSX/Disc Game.bin", b"track".as_slice()),
            ("games/PSX/boot.rom", b"bios".as_slice()),
            ("games/PSX/Disc Game.sbi", b"sidecar".as_slice()),
            ("games/BBCMicro/Elite.ssd", b"disk".as_slice()),
            ("games/BBCMicro/Exile.uef", b"tape".as_slice()),
            ("games/MSX/Metal Gear.rom", b"rom".as_slice()),
        ] {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("game parent")).expect("create game parent");
            fs::write(path, bytes).expect("write game");
        }

        for system_id in MEDIA_EXPERIMENT_SYSTEM_IDS {
            let (baseline, baseline_stats) =
                scan_media_experiment_system(&root, system_id, GenericScanImplementation::Baseline)
                    .expect("baseline scan");
            let (optimized, optimized_stats) = scan_media_experiment_system(
                &root,
                system_id,
                GenericScanImplementation::NamespaceBorrowed,
            )
            .expect("optimized scan");
            assert_eq!(
                baseline
                    .games
                    .iter()
                    .map(|game| &game.launch_ref)
                    .collect::<Vec<_>>(),
                optimized
                    .games
                    .iter()
                    .map(|game| &game.launch_ref)
                    .collect::<Vec<_>>(),
                "{system_id}"
            );
            assert_eq!(baseline_stats.games, optimized_stats.games, "{system_id}");
            assert_eq!(
                baseline_stats.ignored_files, optimized_stats.ignored_files,
                "{system_id}"
            );
        }
        let (_, psx_stats) = scan_media_experiment_system(
            &root,
            "psx",
            GenericScanImplementation::NamespaceBorrowed,
        )
        .expect("PSX scan");
        assert_eq!(psx_stats.games, 1);
        assert_eq!(psx_stats.ignored_files, 3);
        assert_eq!(psx_stats.dependency_files, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn generic_scanners_share_the_complete_production_hidden_name_policy() {
        for path in [
            "/media/fat/games/SNES/._Game.sfc",
            "/media/fat/games/SNES/.DS_Store.sfc",
            "/media/fat/games/SNES/.metadata/Game.sfc",
            "/media/fat/games/SNES/__MACOSX/Game.sfc",
            "/media/fat/games/SNES/.____padding_file/Game.sfc",
            "/media/fat/games/SNES/screenshots/Game.sfc",
            "/media/fat/games/SNES/boxart/Game.sfc",
        ] {
            assert!(should_ignore_path(Path::new(path)), "{path}");
        }
        assert!(!should_ignore_path(Path::new(
            "/media/fat/games/SNES/Real Game.sfc"
        )));
    }
}
