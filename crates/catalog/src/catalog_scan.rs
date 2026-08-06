// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library walking and archive candidate discovery.

use crate::catalog_discovery::{GameDirFact, GameDirHeader};
use crate::launch_profiles::{
    self, CatalogScanPlan, LaunchProfile, PayloadDisposition, ProfilePathClass,
};
use crate::library_db::{
    self, ArchiveFormat, ArchiveScanStatus, LibraryContainer, LibraryContainerEntry,
};
use crate::namespace_walk::{
    self, NamespaceEntry, NamespaceEntryKind, NamespaceSignatureCapture, NamespaceWalkStats,
};
use crate::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

const DISCOVERY_EVENT_BUFFER: usize = 8192;
/// A runtime directory normally contributes a small set of candidate records.
/// Keep that transient buffer bounded; the overflow path deliberately re-walks
/// just that directory after its facts have selected a profile, rather than
/// dropping a possible game.
const MAX_RUNTIME_DIRECTORY_BUFFERED_FILES: usize = 65_536;
const ZIP_CENTRAL_DIRECTORY_BUFFER_BYTES: usize = 64 * 1024;
const ZIP_CENTRAL_DIRECTORY_MAX_BUFFER_BYTES: u64 = 8 * 1024 * 1024;
const ZIP_SKIP_BUFFER_BYTES: usize = 4 * 1024;

pub(crate) struct FoundFile {
    pub(crate) path: PathBuf,
    pub(crate) ext: String,
    pub(crate) size: u64,
    pub(crate) mtime_secs: i64,
}

pub(crate) struct RuntimeDirectoryCandidates {
    pub(crate) header: GameDirHeader,
    pub(crate) facts: GameDirFact,
    pub(crate) files: Vec<FoundFile>,
    pub(crate) overflowed: bool,
}

/// Stable identity and ordering for one target in a planned library scan.
///
/// Target boundary events use this descriptor so consumers can associate all
/// discoveries between `TargetStart` and `TargetComplete` with exactly one
/// planned target without inferring boundaries from file paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ScanTargetDescriptor {
    pub(crate) ordinal: usize,
    pub(crate) path: PathBuf,
    pub(crate) kind: ScanTargetKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScanTargetKind {
    Static,
    Runtime,
    FactsOnly,
}

pub(crate) struct ArchiveScan {
    pub(crate) container: LibraryContainer,
    pub(crate) entries: Vec<LibraryContainerEntry>,
}

pub(crate) fn precount_discovery_candidates(roots: &[String]) -> (usize, usize, u64) {
    let started = Instant::now();
    let rx = discover_files_pipelined(roots.to_vec());
    let mut candidates = 0usize;
    let mut dirs = 0usize;
    while let Ok(event) = rx.recv() {
        match event {
            DiscoveryEvent::TargetStart(_) | DiscoveryEvent::TargetComplete(_) => {}
            DiscoveryEvent::File(_) => candidates += 1,
            DiscoveryEvent::GameDirFacts(_) => {}
            DiscoveryEvent::RuntimeDirectory(runtime) => candidates += runtime.files.len(),
            DiscoveryEvent::Done { dirs: count, .. } => {
                dirs = count;
                break;
            }
        }
    }
    (candidates, dirs, started.elapsed().as_micros() as u64)
}

pub(crate) fn classify_profile_path<'a>(
    profiles: &'a [LaunchProfile],
    path: &Path,
) -> Option<(&'a LaunchProfile, ProfilePathClass)> {
    let profile = profile_for_path(profiles, path)?;
    Some((profile, profile.classify_path(path)))
}

pub(crate) fn profile_for_path<'a>(
    profiles: &'a [LaunchProfile],
    path: &Path,
) -> Option<&'a LaunchProfile> {
    let mut previous_was_games = false;
    for component in path_components_str(path) {
        if previous_was_games
            && let Some(profile) = launch_profiles::profile_for_game_dir(profiles, component)
        {
            return Some(profile);
        }
        previous_was_games = component.eq_ignore_ascii_case("games");
    }

    profiles.iter().find(|profile| {
        path_components_str(path).any(|component| {
            profile
                .game_dirs
                .iter()
                .any(|dir| component.eq_ignore_ascii_case(dir))
        })
    })
}

fn path_components_str(path: &Path) -> impl Iterator<Item = &str> {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
}

pub(crate) enum DiscoveryEvent {
    TargetStart(ScanTargetDescriptor),
    File(FoundFile),
    GameDirFacts(GameDirFact),
    RuntimeDirectory(RuntimeDirectoryCandidates),
    TargetComplete(ScanTargetDescriptor),
    Done { dirs: usize, discover_us: u64 },
}

struct WalkTargetStats {
    dirs: usize,
    files: usize,
    candidates: usize,
    elapsed_us: u64,
    aborted: bool,
    namespace: NamespaceWalkStats,
}

/// Aggregate time spent handing producer events to the bounded discovery
/// channel. This deliberately measures the complete `SyncSender::send` call:
/// with the channel's fixed capacity, sustained time here is producer
/// backpressure rather than filesystem enumeration.
#[derive(Default)]
struct SyncSendStats {
    elapsed_us: u64,
    sends: usize,
    slow_sends: usize,
    max_us: u64,
}

impl SyncSendStats {
    fn send(&mut self, tx: &mpsc::SyncSender<DiscoveryEvent>, event: DiscoveryEvent) -> bool {
        let started = Instant::now();
        let sent = tx.send(event).is_ok();
        let elapsed_us = started.elapsed().as_micros() as u64;
        self.elapsed_us = self.elapsed_us.saturating_add(elapsed_us);
        self.sends = self.sends.saturating_add(1);
        self.slow_sends = self
            .slow_sends
            .saturating_add(usize::from(elapsed_us >= 1_000));
        self.max_us = self.max_us.max(elapsed_us);
        sent
    }

    fn add(&mut self, other: &Self) {
        self.elapsed_us = self.elapsed_us.saturating_add(other.elapsed_us);
        self.sends = self.sends.saturating_add(other.sends);
        self.slow_sends = self.slow_sends.saturating_add(other.slow_sends);
        self.max_us = self.max_us.max(other.max_us);
    }
}

pub(crate) fn discover_files_pipelined(roots: Vec<String>) -> mpsc::Receiver<DiscoveryEvent> {
    discover_files_pipelined_with_role(roots, None, RuntimeThreadRole::LibraryWalker)
}

pub(crate) fn discover_files_pipelined_foreground_with_plan(
    roots: Vec<String>,
    plan: CatalogScanPlan,
    excluded_targets: Vec<PathBuf>,
) -> mpsc::Receiver<DiscoveryEvent> {
    discover_files_pipelined_with_plan(
        roots,
        plan,
        excluded_targets,
        RuntimeThreadRole::LibraryWalkerForeground,
    )
}

pub(crate) fn discover_files_pipelined_with_plan(
    roots: Vec<String>,
    plan: CatalogScanPlan,
    excluded_targets: Vec<PathBuf>,
    role: RuntimeThreadRole,
) -> mpsc::Receiver<DiscoveryEvent> {
    let (tx, rx) = mpsc::sync_channel(DISCOVERY_EVENT_BUFFER);
    std::thread::Builder::new()
        .name("library-walker".to_string())
        .spawn(move || {
            apply_runtime_thread_policy(role);
            let _background_scope = (role == RuntimeThreadRole::LibraryWalker)
                .then(crate::cooperative_work::BackgroundScope::enter);
            let t = Instant::now();
            let walk_pmu = mister_magik_perf_events::sampled_span(crate::pmu_phase::WALK);
            let dirs = walk_index_candidates_with_plan(&roots, &plan, &excluded_targets, &tx);
            drop(walk_pmu);
            mister_magik_perf_events::submit_thread_profile("library-walker");
            let _ = tx.send(DiscoveryEvent::Done {
                dirs,
                discover_us: t.elapsed().as_micros() as u64,
            });
        })
        .expect("spawn library-walker");
    rx
}

fn discover_files_pipelined_with_role(
    roots: Vec<String>,
    profiles: Option<Vec<LaunchProfile>>,
    role: RuntimeThreadRole,
) -> mpsc::Receiver<DiscoveryEvent> {
    let (tx, rx) = mpsc::sync_channel(DISCOVERY_EVENT_BUFFER);
    std::thread::Builder::new()
        .name("library-walker".to_string())
        .spawn(move || {
            apply_runtime_thread_policy(role);
            let _background_scope = (role == RuntimeThreadRole::LibraryWalker)
                .then(crate::cooperative_work::BackgroundScope::enter);
            let t = Instant::now();
            let walk_pmu = mister_magik_perf_events::sampled_span(crate::pmu_phase::WALK);
            let dirs = discover_files_streaming(&roots, profiles, &tx);
            drop(walk_pmu);
            mister_magik_perf_events::submit_thread_profile("library-walker");
            let _ = tx.send(DiscoveryEvent::Done {
                dirs,
                discover_us: t.elapsed().as_micros() as u64,
            });
        })
        .expect("spawn library-walker");
    rx
}

fn discover_files_streaming(
    roots: &[String],
    profiles: Option<Vec<LaunchProfile>>,
    tx: &mpsc::SyncSender<DiscoveryEvent>,
) -> usize {
    walk_index_candidates(roots, profiles, Some(tx))
}

fn walk_index_candidates(
    roots: &[String],
    profiles: Option<Vec<LaunchProfile>>,
    tx: Option<&mpsc::SyncSender<DiscoveryEvent>>,
) -> usize {
    let profiles = match profiles {
        Some(profiles) => profiles,
        None => {
            let profiles_t = Instant::now();
            let profiles = launch_profiles::active_profiles_for_roots(roots);
            library_db::report_library_scan_timing(
                "active_profiles",
                profiles_t.elapsed().as_micros() as u64,
                format!("profiles={}", profiles.len()),
            );
            profiles
        }
    };
    let candidate_exts = source_index_extensions(&profiles);
    let targets = scan_targets_for_roots(roots, &profiles);
    library_db::report_library_scan_timing(
        "walk_targets",
        0,
        format!(
            "roots={} targets={} extensions={}",
            roots.len(),
            targets.len(),
            candidate_exts.len()
        ),
    );
    let mut dirs = 0usize;
    if let Some(tx) = tx {
        return walk_index_candidates_streaming(targets, &profiles, &candidate_exts, tx);
    }
    for target in targets {
        let stats = scan_target_candidates(&target, &profiles, &candidate_exts, |_| true);
        dirs += stats.dirs;
        report_walk_target(&target, &stats, 0, &SyncSendStats::default());
    }
    dirs
}

enum PlannedScanTarget {
    Static {
        path: PathBuf,
        game_dir_header: Option<GameDirHeader>,
    },
    Runtime(GameDirHeader),
    FactsOnly(GameDirHeader),
}

pub(crate) fn planned_scan_target_descriptors(
    roots: &[String],
    plan: &CatalogScanPlan,
    excluded_targets: &[PathBuf],
) -> Vec<ScanTargetDescriptor> {
    scan_targets_for_plan(roots, plan, plan.base_profiles(), excluded_targets)
        .iter()
        .enumerate()
        .map(|(ordinal, target)| target.descriptor(ordinal))
        .collect()
}

fn walk_index_candidates_with_plan(
    roots: &[String],
    plan: &CatalogScanPlan,
    excluded_targets: &[PathBuf],
    tx: &mpsc::SyncSender<DiscoveryEvent>,
) -> usize {
    let profiles = plan.base_profiles();
    let candidate_exts = source_index_extensions(profiles);
    let targets = scan_targets_for_plan(roots, plan, profiles, excluded_targets);
    library_db::report_library_scan_timing(
        "walk_targets",
        0,
        format!(
            "roots={} targets={} extensions={} runtime_dirs={}",
            roots.len(),
            targets.len(),
            candidate_exts.len(),
            plan.game_dir_headers().len(),
        ),
    );
    let mut dirs = 0usize;
    let mut producer_us = 0u64;
    let mut send_stats = SyncSendStats::default();
    for (ordinal, target) in targets.into_iter().enumerate() {
        let mut target_send_stats = SyncSendStats::default();
        let descriptor = target.descriptor(ordinal);
        if !target_send_stats.send(tx, DiscoveryEvent::TargetStart(descriptor.clone())) {
            break;
        }
        let stats = match target {
            PlannedScanTarget::Static {
                path,
                game_dir_header,
            } => {
                let (stats, facts) = scan_target_candidates_with_facts(
                    &path,
                    profiles,
                    &candidate_exts,
                    game_dir_header.as_ref(),
                    |file| target_send_stats.send(tx, DiscoveryEvent::File(file)),
                );
                let in_target_send_us = target_send_stats.elapsed_us;
                if let Some(facts) = facts
                    && !target_send_stats.send(tx, DiscoveryEvent::GameDirFacts(facts))
                {
                    break;
                }
                producer_us =
                    producer_us.saturating_add(stats.elapsed_us.saturating_sub(in_target_send_us));
                report_walk_target(&path, &stats, in_target_send_us, &target_send_stats);
                stats
            }
            PlannedScanTarget::Runtime(header) => {
                let (stats, candidates) = scan_runtime_target_candidates(&header, plan);
                if !target_send_stats.send(tx, DiscoveryEvent::RuntimeDirectory(candidates)) {
                    break;
                }
                producer_us = producer_us.saturating_add(stats.elapsed_us);
                report_walk_target(&header.path, &stats, 0, &target_send_stats);
                stats
            }
            PlannedScanTarget::FactsOnly(header) => {
                let (stats, facts) = scan_game_dir_facts_only(&header);
                if !target_send_stats.send(tx, DiscoveryEvent::GameDirFacts(facts)) {
                    break;
                }
                producer_us = producer_us.saturating_add(stats.elapsed_us);
                report_walk_target(&header.path, &stats, 0, &target_send_stats);
                stats
            }
        };
        dirs += stats.dirs;
        if stats.aborted {
            send_stats.add(&target_send_stats);
            break;
        }
        if !target_send_stats.send(tx, DiscoveryEvent::TargetComplete(descriptor)) {
            send_stats.add(&target_send_stats);
            break;
        }
        send_stats.add(&target_send_stats);
    }
    report_walker_pipeline_breakdown(producer_us, &send_stats);
    dirs
}

impl PlannedScanTarget {
    fn descriptor(&self, ordinal: usize) -> ScanTargetDescriptor {
        let (path, kind) = match self {
            Self::Static { path, .. } => (path.clone(), ScanTargetKind::Static),
            Self::Runtime(header) => (header.path.clone(), ScanTargetKind::Runtime),
            Self::FactsOnly(header) => (header.path.clone(), ScanTargetKind::FactsOnly),
        };
        ScanTargetDescriptor {
            ordinal,
            path,
            kind,
        }
    }
}

fn scan_targets_for_plan(
    roots: &[String],
    plan: &CatalogScanPlan,
    profiles: &[LaunchProfile],
    excluded_targets: &[PathBuf],
) -> Vec<PlannedScanTarget> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for path in scan_targets_for_roots(roots, profiles) {
        if excluded_targets
            .iter()
            .any(|excluded| same_library_path(excluded, &path))
        {
            continue;
        }
        let key = path.display().to_string().to_ascii_lowercase();
        if seen.insert(key) {
            let game_dir_header = plan
                .all_game_dir_headers()
                .iter()
                .find(|header| same_library_path(&header.path, &path))
                .cloned();
            targets.push(PlannedScanTarget::Static {
                path,
                game_dir_header,
            });
        }
    }
    for header in plan.game_dir_headers() {
        let key = header.path.display().to_string().to_ascii_lowercase();
        if seen.insert(key) {
            targets.push(PlannedScanTarget::Runtime(header.clone()));
        }
    }
    for header in plan.all_game_dir_headers() {
        let key = header.path.display().to_string().to_ascii_lowercase();
        if seen.insert(key) {
            targets.push(PlannedScanTarget::FactsOnly(header.clone()));
        }
    }
    targets
}

fn same_library_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

fn walk_index_candidates_streaming(
    targets: Vec<PathBuf>,
    profiles: &[LaunchProfile],
    candidate_exts: &HashSet<String>,
    tx: &mpsc::SyncSender<DiscoveryEvent>,
) -> usize {
    let mut dirs = 0usize;
    let mut producer_us = 0u64;
    let mut send_stats = SyncSendStats::default();
    for target in targets {
        let mut target_send_stats = SyncSendStats::default();
        let stats = scan_target_candidates(&target, profiles, candidate_exts, |file| {
            target_send_stats.send(tx, DiscoveryEvent::File(file))
        });
        dirs += stats.dirs;
        producer_us = producer_us.saturating_add(
            stats
                .elapsed_us
                .saturating_sub(target_send_stats.elapsed_us),
        );
        report_walk_target(
            &target,
            &stats,
            target_send_stats.elapsed_us,
            &target_send_stats,
        );
        send_stats.add(&target_send_stats);
        if stats.aborted {
            break;
        }
    }
    report_walker_pipeline_breakdown(producer_us, &send_stats);
    dirs
}

fn scan_target_candidates(
    target: &Path,
    profiles: &[LaunchProfile],
    candidate_exts: &HashSet<String>,
    emit: impl FnMut(FoundFile) -> bool,
) -> WalkTargetStats {
    scan_target_candidates_with_facts(target, profiles, candidate_exts, None, emit).0
}

fn scan_target_candidates_with_facts(
    target: &Path,
    profiles: &[LaunchProfile],
    candidate_exts: &HashSet<String>,
    game_dir_header: Option<&GameDirHeader>,
    mut emit: impl FnMut(FoundFile) -> bool,
) -> (WalkTargetStats, Option<GameDirFact>) {
    let target_t = Instant::now();
    let mut dirs = 1usize;
    let mut files = 0usize;
    let mut candidates = 0usize;
    let mut aborted = false;
    let mut nested_directory_seen = false;
    let mut facts = game_dir_header.map(|header| GameDirFact {
        name: header.name.clone(),
        path: header.path.clone(),
        signature: header.signature,
        has_payload_files: false,
        has_zip_files: false,
        direct_zip_paths: Vec::new(),
        nested_probe_signatures: Vec::new(),
        payload_extensions: std::collections::BTreeSet::new(),
    });
    let signature_capture = if facts.is_some() {
        NamespaceSignatureCapture::Target
    } else {
        NamespaceSignatureCapture::None
    };
    let namespace_stats = namespace_walk::visit_with_signature_capture(
        target,
        None,
        signature_capture,
        should_ignore_path,
        |entry| {
            let p = entry.path.as_path();
            if entry.kind == NamespaceEntryKind::Directory {
                dirs += 1;
                if facts.is_some() {
                    nested_directory_seen = true;
                }
                return true;
            }
            if entry.kind != NamespaceEntryKind::File {
                return true;
            }
            files += 1;
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if let Some(facts) = facts.as_mut() {
                let depth = p
                    .strip_prefix(target)
                    .ok()
                    .map(|relative| relative.components().count())
                    .unwrap_or(usize::MAX);
                if depth <= 2 {
                    if ext.eq_ignore_ascii_case("zip") {
                        facts.has_zip_files = true;
                        if depth == 1 {
                            let relative = p.strip_prefix(target).unwrap_or(p);
                            facts.direct_zip_paths.push(facts.path.join(relative));
                        }
                    } else {
                        facts.has_payload_files = true;
                        if !ext.is_empty() {
                            facts.payload_extensions.insert(ext.clone());
                        }
                    }
                }
            }
            if !is_source_index_extension(candidate_exts, p, &ext) {
                return true;
            }
            if !is_index_candidate(profiles, p, &ext) {
                return true;
            }
            let (size, mtime_secs) = candidate_signature_for_namespace_entry(entry, &ext);
            let file = FoundFile {
                path: p.to_path_buf(),
                ext,
                size,
                mtime_secs,
            };
            candidates += 1;
            if !emit(file) {
                aborted = true;
                return false;
            }
            true
        },
    );
    let target_signature = namespace_stats.target_signature;
    (
        WalkTargetStats {
            dirs,
            files,
            candidates,
            elapsed_us: target_t.elapsed().as_micros() as u64,
            aborted,
            namespace: namespace_stats,
        },
        facts.map(|mut facts| {
            facts.signature = if nested_directory_seen && !facts.has_payloadish_files() {
                // Static targets do not retain one probe per nested directory:
                // doing so would add thousands of metadata operations to deep
                // production trees. An empty tree with existing children can
                // become payloadish without changing the top directory, so it
                // must take the exact warm fallback rather than authorize reuse.
                crate::catalog_discovery::GameDirSignature::Unavailable
            } else {
                crate::catalog_discovery::GameDirSignature::from_namespace_signature(
                    target_signature,
                )
            };
            facts
                .direct_zip_paths
                .sort_by_cached_key(|path| path.to_string_lossy().to_ascii_lowercase());
            facts
        }),
    )
}

fn scan_runtime_target_candidates(
    header: &GameDirHeader,
    plan: &CatalogScanPlan,
) -> (WalkTargetStats, RuntimeDirectoryCandidates) {
    let target_t = Instant::now();
    let mut dirs = 1usize;
    let mut files_seen = 0usize;
    let mut has_payload_files = false;
    let mut has_zip_files = false;
    let mut direct_zip_paths = Vec::new();
    let mut nested_probe_signatures = Vec::new();
    let mut payload_extensions = std::collections::BTreeSet::new();
    let mut shallow_files = Vec::new();
    let mut deep_roots = Vec::new();

    let shallow_namespace_stats = namespace_walk::visit_with_signature_capture(
        &header.path,
        Some(2),
        NamespaceSignatureCapture::TargetAndDepthOneDirectories,
        should_ignore_path,
        |entry| {
            let path = entry.path.as_path();
            if entry.kind == NamespaceEntryKind::Directory {
                dirs += 1;
                let depth = path
                    .strip_prefix(&header.path)
                    .ok()
                    .map(|relative| relative.components().count());
                if depth == Some(1) {
                    nested_probe_signatures.push((
                        path.to_path_buf(),
                        crate::catalog_discovery::GameDirSignature::from_namespace_signature(
                            entry.directory_signature,
                        ),
                    ));
                } else if depth == Some(2) {
                    deep_roots.push(path.to_path_buf());
                }
                return true;
            }
            if entry.kind != NamespaceEntryKind::File {
                return true;
            }
            files_seen += 1;
            let ext = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext.eq_ignore_ascii_case("zip") {
                has_zip_files = true;
                if path
                    .strip_prefix(&header.path)
                    .ok()
                    .is_some_and(|relative| relative.components().count() == 1)
                {
                    direct_zip_paths.push(path.to_path_buf());
                }
            } else {
                has_payload_files = true;
                if !ext.is_empty() {
                    payload_extensions.insert(ext.clone());
                }
            }
            let (size, mtime_secs) = candidate_signature_for_namespace_entry(entry, &ext);
            let file = FoundFile {
                path: path.to_path_buf(),
                ext,
                size,
                mtime_secs,
            };
            shallow_files.push(file);
            true
        },
    );
    let target_signature = shallow_namespace_stats.target_signature;
    let mut namespace_stats = shallow_namespace_stats;
    direct_zip_paths.sort_by_cached_key(|path| path.to_string_lossy().to_ascii_lowercase());
    nested_probe_signatures.sort_by_cached_key(|(path, _)| {
        (path.to_string_lossy().to_ascii_lowercase(), path.clone())
    });
    let facts = GameDirFact {
        name: header.name.clone(),
        path: header.path.clone(),
        signature: crate::catalog_discovery::GameDirSignature::from_namespace_signature(
            target_signature,
        ),
        has_payload_files,
        has_zip_files,
        direct_zip_paths,
        nested_probe_signatures,
        payload_extensions,
    };
    let Some(profile) = plan.profile_for_game_dir_facts(&facts) else {
        return (
            WalkTargetStats {
                dirs,
                files: files_seen,
                candidates: 0,
                elapsed_us: target_t.elapsed().as_micros() as u64,
                aborted: false,
                namespace: namespace_stats,
            },
            RuntimeDirectoryCandidates {
                header: header.clone(),
                facts,
                files: Vec::new(),
                overflowed: false,
            },
        );
    };

    let candidate_exts = source_index_extensions(std::slice::from_ref(&profile));
    let mut files = Vec::new();
    let mut overflowed = false;
    for file in shallow_files {
        push_runtime_candidate(&mut files, &mut overflowed, &candidate_exts, &profile, file);
    }
    for root in deep_roots {
        let deep_namespace_stats =
            namespace_walk::visit(&root, None, should_ignore_path, |entry| {
                let path = entry.path.as_path();
                if entry.kind == NamespaceEntryKind::Directory {
                    dirs += 1;
                    return true;
                }
                if entry.kind != NamespaceEntryKind::File {
                    return true;
                }
                files_seen += 1;
                let ext = path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let (size, mtime_secs) = candidate_signature_for_namespace_entry(entry, &ext);
                let file = FoundFile {
                    path: path.to_path_buf(),
                    ext,
                    size,
                    mtime_secs,
                };
                push_runtime_candidate(
                    &mut files,
                    &mut overflowed,
                    &candidate_exts,
                    &profile,
                    file,
                );
                true
            });
        namespace_stats.add(&deep_namespace_stats);
    }
    let stats = WalkTargetStats {
        dirs,
        files: files_seen,
        candidates: files.len(),
        elapsed_us: target_t.elapsed().as_micros() as u64,
        aborted: false,
        namespace: namespace_stats,
    };
    library_db::report_library_scan_timing(
        "runtime_buffer",
        0,
        format!(
            "path={} buffered={} limit={} overflowed={}",
            header.path.display(),
            files.len(),
            MAX_RUNTIME_DIRECTORY_BUFFERED_FILES,
            overflowed,
        ),
    );
    (
        stats,
        RuntimeDirectoryCandidates {
            header: header.clone(),
            facts,
            files,
            overflowed,
        },
    )
}

fn push_runtime_candidate(
    files: &mut Vec<FoundFile>,
    overflowed: &mut bool,
    candidate_exts: &HashSet<String>,
    profile: &LaunchProfile,
    file: FoundFile,
) {
    if *overflowed
        || !is_source_index_extension(candidate_exts, &file.path, &file.ext)
        || !is_index_candidate(std::slice::from_ref(profile), &file.path, &file.ext)
    {
        return;
    }
    if files.len() == MAX_RUNTIME_DIRECTORY_BUFFERED_FILES {
        *overflowed = true;
        files.clear();
        return;
    }
    files.push(file);
}

fn scan_game_dir_facts_only(header: &GameDirHeader) -> (WalkTargetStats, GameDirFact) {
    let target_t = Instant::now();
    let mut dirs = 1usize;
    let mut files = 0usize;
    let mut has_payload_files = false;
    let mut has_zip_files = false;
    let mut direct_zip_paths = Vec::new();
    let mut nested_probe_signatures = Vec::new();
    let mut payload_extensions = std::collections::BTreeSet::new();
    let namespace_stats = namespace_walk::visit_with_signature_capture(
        &header.path,
        Some(2),
        NamespaceSignatureCapture::TargetAndDepthOneDirectories,
        should_ignore_path,
        |entry| {
            let path = entry.path.as_path();
            if entry.kind == NamespaceEntryKind::Directory {
                dirs += 1;
                if path
                    .strip_prefix(&header.path)
                    .ok()
                    .is_some_and(|relative| relative.components().count() == 1)
                {
                    nested_probe_signatures.push((
                        path.to_path_buf(),
                        crate::catalog_discovery::GameDirSignature::from_namespace_signature(
                            entry.directory_signature,
                        ),
                    ));
                }
                return true;
            }
            if entry.kind != NamespaceEntryKind::File {
                return true;
            }
            files += 1;
            let ext = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext.eq_ignore_ascii_case("zip") {
                has_zip_files = true;
                if path
                    .strip_prefix(&header.path)
                    .ok()
                    .is_some_and(|relative| relative.components().count() == 1)
                {
                    direct_zip_paths.push(path.to_path_buf());
                }
            } else {
                has_payload_files = true;
                if !ext.is_empty() {
                    payload_extensions.insert(ext);
                }
            }
            true
        },
    );
    direct_zip_paths.sort_by_cached_key(|path| path.to_string_lossy().to_ascii_lowercase());
    nested_probe_signatures.sort_by_cached_key(|(path, _)| {
        (path.to_string_lossy().to_ascii_lowercase(), path.clone())
    });
    let target_signature = namespace_stats.target_signature;
    (
        WalkTargetStats {
            dirs,
            files,
            candidates: 0,
            elapsed_us: target_t.elapsed().as_micros() as u64,
            aborted: false,
            namespace: namespace_stats,
        },
        GameDirFact {
            name: header.name.clone(),
            path: header.path.clone(),
            signature: crate::catalog_discovery::GameDirSignature::from_namespace_signature(
                target_signature,
            ),
            has_payload_files,
            has_zip_files,
            direct_zip_paths,
            nested_probe_signatures,
            payload_extensions,
        },
    )
}

/// Correctness-preserving rare fallback for a runtime directory that exceeded
/// the in-RAM candidate bound. The normal path never calls this second walk.
pub(crate) fn collect_runtime_candidates_after_overflow(
    header: &GameDirHeader,
    profiles: &[LaunchProfile],
) -> Vec<FoundFile> {
    let candidate_exts = source_index_extensions(profiles);
    let mut files = Vec::new();
    let stats = scan_target_candidates(&header.path, profiles, &candidate_exts, |file| {
        files.push(file);
        true
    });
    report_walk_target(&header.path, &stats, 0, &SyncSendStats::default());
    files
}

fn report_walk_target(
    target: &Path,
    stats: &WalkTargetStats,
    in_target_send_us: u64,
    send: &SyncSendStats,
) {
    let producer_us = stats.elapsed_us.saturating_sub(in_target_send_us);
    library_db::report_library_scan_timing(
        "walk_target",
        stats.elapsed_us,
        format!(
            "path={} dirs={} files={} candidates={} producer_us={} sync_send_us={} sync_sends={} sync_slow_sends={} sync_send_max_us={} namespace_backend={} namespace_entries={} namespace_dir_opens={} namespace_reads={} namespace_bytes={} namespace_type_stats={} namespace_fallback={}",
            target.display(),
            stats.dirs,
            stats.files,
            stats.candidates,
            producer_us,
            send.elapsed_us,
            send.sends,
            send.slow_sends,
            send.max_us,
            stats.namespace.backend,
            stats.namespace.captured_entries,
            stats.namespace.dir_opens,
            stats.namespace.read_calls,
            stats.namespace.read_bytes,
            stats.namespace.type_stats,
            stats.namespace.fallback_reason.as_deref().unwrap_or("none"),
        ),
    );
}

fn report_walker_pipeline_breakdown(producer_us: u64, send: &SyncSendStats) {
    library_db::report_library_scan_timing(
        "walker_producer",
        producer_us,
        format!("sync_send_us={} sync_sends={}", send.elapsed_us, send.sends,),
    );
    library_db::report_library_scan_timing(
        "walker_sync_send",
        send.elapsed_us,
        format!(
            "sends={} slow_sends={} max_us={} channel_capacity={}",
            send.sends, send.slow_sends, send.max_us, DISCOVERY_EVENT_BUFFER,
        ),
    );
}

fn source_index_extensions(profiles: &[LaunchProfile]) -> HashSet<String> {
    let mut extensions = HashSet::new();
    for profile in profiles {
        for rule in &profile.payload_rules {
            insert_extensions(&mut extensions, &rule.extensions);
        }
        for rule in &profile.archive_entry_rules {
            insert_extensions(&mut extensions, &rule.extensions);
            extensions.insert("zip".to_string());
        }
        for rule in &profile.collection_rules {
            insert_extensions(&mut extensions, &rule.archive_extensions);
        }
        for rule in &profile.ignore_rules {
            insert_extensions(&mut extensions, &rule.extensions);
        }
    }
    extensions
}

fn insert_extensions(extensions: &mut HashSet<String>, values: &[String]) {
    for value in values {
        let value = value.trim().to_ascii_lowercase();
        if !value.is_empty() {
            extensions.insert(value);
        }
    }
}

fn is_source_index_extension(candidate_exts: &HashSet<String>, path: &Path, ext: &str) -> bool {
    candidate_exts.contains(ext) || crate::media_metadata::is_amigavision_listing_path(path)
}

fn scan_targets_for_roots(roots: &[String], profiles: &[LaunchProfile]) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    for root in roots {
        let path = Path::new(root);
        if !is_real_dir(path) {
            continue;
        }
        if is_direct_scan_root(path, profiles) {
            push_scan_target(&mut targets, path.to_path_buf());
            continue;
        }
        push_prepared_collection_targets(&mut targets, path);
        if path_name_eq(path, "games") {
            push_profile_game_dirs(&mut targets, path, profiles);
            continue;
        }

        for launcher_dir in ["_Arcade", "_Games", "_DOS Games", "_Console (autoboot)"] {
            push_scan_target(&mut targets, path.join(launcher_dir));
        }
        push_profile_game_dirs(&mut targets, &path.join("games"), profiles);
    }
    dedupe_existing_scan_targets(targets)
}

fn push_prepared_collection_targets(targets: &mut Vec<PathBuf>, configured_root: &Path) {
    let storage_root = if ["games", "_Arcade", "_Games", "_DOS Games", "_LLAPI"]
        .iter()
        .any(|name| path_name_eq(configured_root, name))
    {
        configured_root.parent().unwrap_or(configured_root)
    } else {
        configured_root
    };
    push_scan_target(targets, storage_root.join("_Computer").join("X68000 Games"));
}

fn push_profile_game_dirs(
    targets: &mut Vec<PathBuf>,
    games_dir: &Path,
    profiles: &[LaunchProfile],
) {
    for profile in profiles {
        if !profile_has_initial_catalog_candidates(profile) {
            continue;
        }
        for dir in &profile.game_dirs {
            if dir.starts_with('_') {
                continue;
            }
            push_scan_target(targets, games_dir.join(dir));
        }
    }
}

fn push_scan_target(targets: &mut Vec<PathBuf>, path: PathBuf) {
    if is_real_dir(&path) {
        targets.push(path);
    }
}

fn dedupe_existing_scan_targets(targets: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for target in targets {
        let key = target.display().to_string().to_ascii_lowercase();
        if seen.insert(key) {
            out.push(target);
        }
    }
    out
}

fn is_direct_scan_root(path: &Path, profiles: &[LaunchProfile]) -> bool {
    ["_Arcade", "_Games", "_DOS Games", "_Console (autoboot)"]
        .iter()
        .any(|name| path_name_eq(path, name))
        || profiles.iter().any(|profile| {
            profile_has_initial_catalog_candidates(profile)
                && profile
                    .game_dirs
                    .iter()
                    .any(|dir| !dir.starts_with('_') && path_name_eq(path, dir))
        })
}

fn profile_has_initial_catalog_candidates(profile: &LaunchProfile) -> bool {
    profile
        .payload_rules
        .iter()
        .any(|rule| rule.disposition == PayloadDisposition::Playable)
        || !profile.archive_entry_rules.is_empty()
        || !profile.collection_rules.is_empty()
}

fn path_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn candidate_signature_for_namespace_entry(entry: &NamespaceEntry, ext: &str) -> (u64, i64) {
    if ext.eq_ignore_ascii_case("zip")
        && let Some(signature) = entry.zip_signature
    {
        return signature;
    }
    (0, 0)
}

pub(crate) fn scan_archive_toc(
    file: &FoundFile,
    format: ArchiveFormat,
    profile: &LaunchProfile,
) -> ArchiveScan {
    let t = Instant::now();
    let (status, entries) = match format {
        ArchiveFormat::Zip => match scan_zip_central_directory(file, profile) {
            Ok(entries) => (ArchiveScanStatus::Ok, entries),
            Err(e) => (ArchiveScanStatus::Error(e), Vec::new()),
        },
        ArchiveFormat::SevenZip | ArchiveFormat::Lha | ArchiveFormat::Lzh | ArchiveFormat::Rar => {
            (ArchiveScanStatus::Unsupported, Vec::new())
        }
        ArchiveFormat::Chd => (ArchiveScanStatus::HeaderOnly, Vec::new()),
    };
    ArchiveScan {
        container: LibraryContainer {
            file_path: file.path.display().to_string(),
            format,
            size: file.size,
            mtime_secs: file.mtime_secs,
            entry_count: entries.len() as u32,
            scan_status: status,
            scan_us: t.elapsed().as_micros() as u64,
        },
        entries,
    }
}

pub(crate) fn scan_container_header(file: &FoundFile, format: ArchiveFormat) -> LibraryContainer {
    LibraryContainer {
        file_path: file.path.display().to_string(),
        format,
        size: file.size,
        mtime_secs: file.mtime_secs,
        entry_count: 0,
        scan_status: ArchiveScanStatus::HeaderOnly,
        scan_us: 0,
    }
}

pub(crate) fn scan_zip_central_directory(
    file: &FoundFile,
    profile: &LaunchProfile,
) -> Result<Vec<LibraryContainerEntry>, String> {
    let mut f = File::open(&file.path).map_err(|e| format!("open zip: {e}"))?;
    let len = f.metadata().map_err(|e| format!("stat zip: {e}"))?.len();
    if len < 22 {
        return Err("zip too small".to_string());
    }

    let tail_len = len.min(66_000) as usize;
    f.seek(SeekFrom::End(-(tail_len as i64)))
        .map_err(|e| format!("seek zip tail: {e}"))?;
    let mut tail = vec![0u8; tail_len];
    f.read_exact(&mut tail)
        .map_err(|e| format!("read zip tail: {e}"))?;
    let Some(eocd) = library_db::find_eocd(&tail) else {
        return Err("zip EOCD not found".to_string());
    };

    let mut cd_entries = library_db::le_u16(&tail[eocd + 10..eocd + 12]) as usize;
    let mut cd_size = library_db::le_u32(&tail[eocd + 12..eocd + 16]) as u64;
    let mut cd_offset = library_db::le_u32(&tail[eocd + 16..eocd + 20]) as u64;
    if cd_offset == u32::MAX as u64 || cd_size == u32::MAX as u64 || cd_entries == u16::MAX as usize
    {
        let zip64 = read_zip64_central_directory_location(&mut f, &tail, eocd)?;
        cd_entries = zip64.entries;
        cd_size = zip64.size;
        cd_offset = zip64.offset;
    }
    match cd_offset.checked_add(cd_size) {
        Some(end) if end <= len => {}
        _ => return Err("zip central directory outside file".to_string()),
    }
    let file_path = file.path.display().to_string();
    f.seek(SeekFrom::Start(cd_offset))
        .map_err(|e| format!("seek zip central directory: {e}"))?;
    if cd_size <= ZIP_CENTRAL_DIRECTORY_MAX_BUFFER_BYTES {
        let mut central_directory = vec![0u8; cd_size as usize];
        f.read_exact(&mut central_directory)
            .map_err(|e| format!("read zip central directory: {e}"))?;
        return scan_zip_central_directory_entries(
            &mut central_directory.as_slice(),
            cd_size,
            cd_entries,
            &file_path,
            profile,
        );
    }
    let mut central_directory =
        BufReader::with_capacity(ZIP_CENTRAL_DIRECTORY_BUFFER_BYTES, f.take(cd_size));
    scan_zip_central_directory_entries(
        &mut central_directory,
        cd_size,
        cd_entries,
        &file_path,
        profile,
    )
}

fn scan_zip_central_directory_entries(
    mut central_directory: &mut impl Read,
    cd_size: u64,
    cd_entries: usize,
    file_path: &str,
    profile: &LaunchProfile,
) -> Result<Vec<LibraryContainerEntry>, String> {
    let mut entries = Vec::new();
    let mut remaining = cd_size;
    let mut scanned = 0usize;
    while remaining >= 46 && scanned < cd_entries {
        let entry_offset = cd_size - remaining;
        let mut header = [0u8; 46];
        central_directory
            .read_exact(&mut header)
            .map_err(|e| format!("read zip central directory header: {e}"))?;
        remaining -= 46;
        if library_db::le_u32(&header[0..4]) != 0x0201_4b50 {
            return Err(format!("bad central directory signature at {entry_offset}"));
        }
        scanned += 1;
        let compression_method = library_db::le_u16(&header[10..12]);
        let crc32 = library_db::le_u32(&header[16..20]);
        let compressed_32 = library_db::le_u32(&header[20..24]);
        let uncompressed_32 = library_db::le_u32(&header[24..28]);
        let name_len = library_db::le_u16(&header[28..30]) as u64;
        let extra_len = library_db::le_u16(&header[30..32]) as u64;
        let comment_len = library_db::le_u16(&header[32..34]) as u64;
        let local_header_offset_32 = library_db::le_u32(&header[42..46]);
        let trailing_len = extra_len + comment_len;
        if name_len + trailing_len > remaining {
            return Err("zip entry name outside central directory".to_string());
        }
        let mut name_buf = vec![0u8; name_len as usize];
        central_directory
            .read_exact(&mut name_buf)
            .map_err(|e| format!("read zip entry name: {e}"))?;
        remaining -= name_len;
        let mut extra = vec![0u8; extra_len as usize];
        central_directory
            .read_exact(&mut extra)
            .map_err(|e| format!("read zip entry extra data: {e}"))?;
        remaining -= extra_len;
        if comment_len > 0 {
            discard_zip_bytes(&mut central_directory, comment_len)
                .map_err(|e| format!("skip zip entry comment: {e}"))?;
            remaining -= comment_len;
        }
        let (compressed, uncompressed, local_header_offset) = decode_zip64_member_metadata(
            compressed_32,
            uncompressed_32,
            local_header_offset_32,
            &extra,
        )?;
        let name = String::from_utf8_lossy(&name_buf).into_owned();
        if !name.ends_with('/')
            && !should_ignore_path(Path::new(&name))
            && let Some(rule) = profile.classify_archive_entry(Path::new(&name))
        {
            let launch_ref = crate::archive_member::encode_archive_member_ref(
                &crate::archive_member::ArchiveMemberRef {
                    archive_path: file_path.to_string(),
                    member_path: name.clone(),
                    local_header_offset,
                    compression_method,
                    compressed_size: compressed,
                    uncompressed_size: uncompressed,
                    crc32,
                },
            )?;
            entries.push(LibraryContainerEntry {
                file_path: file_path.to_string(),
                entry_path: name.clone(),
                normalized_title: library_db::normalize_title(&name),
                profile_id: profile.id.to_string(),
                rule,
                compressed_size: Some(compressed),
                uncompressed_size: Some(uncompressed),
                crc32: Some(crc32),
                launchable: true,
                launch_ref,
            });
        }
    }
    Ok(entries)
}

fn decode_zip64_member_metadata(
    compressed: u32,
    uncompressed: u32,
    local_header_offset: u32,
    extra: &[u8],
) -> Result<(u64, u64, u64), String> {
    if compressed != u32::MAX && uncompressed != u32::MAX && local_header_offset != u32::MAX {
        return Ok((
            compressed as u64,
            uncompressed as u64,
            local_header_offset as u64,
        ));
    }

    let mut cursor = 0usize;
    while cursor < extra.len() {
        if extra.len() - cursor < 4 {
            return Err("truncated zip extra field header".to_string());
        }
        let field_id = library_db::le_u16(&extra[cursor..cursor + 2]);
        let field_len = library_db::le_u16(&extra[cursor + 2..cursor + 4]) as usize;
        cursor += 4;
        let end = cursor
            .checked_add(field_len)
            .filter(|end| *end <= extra.len())
            .ok_or_else(|| "zip extra field outside entry metadata".to_string())?;
        if field_id == 0x0001 {
            let field = &extra[cursor..end];
            let mut field_cursor = 0usize;
            let mut next_u64 = || {
                let value_end = field_cursor
                    .checked_add(8)
                    .filter(|value_end| *value_end <= field.len())
                    .ok_or_else(|| "truncated ZIP64 member metadata".to_string())?;
                let value = library_db::le_u64(&field[field_cursor..value_end]);
                field_cursor = value_end;
                Ok::<u64, String>(value)
            };
            let uncompressed = if uncompressed == u32::MAX {
                next_u64()?
            } else {
                uncompressed as u64
            };
            let compressed = if compressed == u32::MAX {
                next_u64()?
            } else {
                compressed as u64
            };
            let local_header_offset = if local_header_offset == u32::MAX {
                next_u64()?
            } else {
                local_header_offset as u64
            };
            return Ok((compressed, uncompressed, local_header_offset));
        }
        cursor = end;
    }
    Err("missing ZIP64 member metadata".to_string())
}

fn discard_zip_bytes(reader: &mut impl Read, mut len: u64) -> Result<(), std::io::Error> {
    let mut scratch = [0u8; ZIP_SKIP_BUFFER_BYTES];
    while len > 0 {
        let read_len = len.min(scratch.len() as u64) as usize;
        reader.read_exact(&mut scratch[..read_len])?;
        len -= read_len as u64;
    }
    Ok(())
}

struct ZipCentralDirectoryLocation {
    entries: usize,
    size: u64,
    offset: u64,
}

fn read_zip64_central_directory_location(
    f: &mut File,
    tail: &[u8],
    eocd: usize,
) -> Result<ZipCentralDirectoryLocation, String> {
    let locator = tail[..eocd]
        .windows(4)
        .rposition(|bytes| bytes == [0x50, 0x4b, 0x06, 0x07])
        .ok_or_else(|| "zip64 EOCD locator not found".to_string())?;
    if locator + 20 > tail.len() {
        return Err("zip64 EOCD locator truncated".to_string());
    }
    let zip64_eocd_offset = library_db::le_u64(&tail[locator + 8..locator + 16]);
    f.seek(SeekFrom::Start(zip64_eocd_offset))
        .map_err(|e| format!("seek zip64 EOCD: {e}"))?;
    let mut record = [0u8; 56];
    f.read_exact(&mut record)
        .map_err(|e| format!("read zip64 EOCD: {e}"))?;
    if library_db::le_u32(&record[0..4]) != 0x0606_4b50 {
        return Err("zip64 EOCD signature not found".to_string());
    }
    let entries = usize::try_from(library_db::le_u64(&record[32..40]))
        .map_err(|_| "zip64 entry count too large to index".to_string())?;
    Ok(ZipCentralDirectoryLocation {
        entries,
        size: library_db::le_u64(&record[40..48]),
        offset: library_db::le_u64(&record[48..56]),
    })
}

pub(crate) fn is_index_candidate(profiles: &[LaunchProfile], path: &Path, _ext: &str) -> bool {
    matches!(
        crate::catalog_scan::classify_profile_path(profiles, path),
        Some((
            _,
            ProfilePathClass::Payload { .. }
                | ProfilePathClass::Collection { .. }
                | ProfilePathClass::Ignored { .. }
        ))
    ) || is_archive_entry_container_candidate(profiles, path)
        || crate::media_metadata::is_amigavision_listing_path(path)
}

pub(crate) fn is_archive_entry_container_candidate(
    profiles: &[LaunchProfile],
    path: &Path,
) -> bool {
    if !path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    {
        return false;
    }
    profile_for_path(profiles, path).is_some_and(|profile| !profile.archive_entry_rules.is_empty())
}

/// Returns true for catalog-irrelevant paths that should be pruned before
/// candidate classification, including macOS metadata sidecars and hidden dirs.
pub(crate) fn should_ignore_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_ascii_lowercase();
    if path_str.contains("/.____padding_file/") || path_str.contains("/__macosx/") {
        return true;
    }
    if is_arcade_non_game_tree(path) {
        return true;
    }
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        is_hidden_path_component(&s)
            || s == ".____padding_file"
            || s.eq_ignore_ascii_case("images")
            || s.eq_ignore_ascii_case("manuals")
            || s.eq_ignore_ascii_case("screenshot")
            || s.eq_ignore_ascii_case("screenshots")
            || s.eq_ignore_ascii_case("screenshot-magik")
            || s.eq_ignore_ascii_case("__macosx")
            || s.eq_ignore_ascii_case("_organized")
            || s.eq_ignore_ascii_case("boxart")
    })
}

fn is_hidden_path_component(component: &str) -> bool {
    component.len() > 1 && component.starts_with('.')
}

fn is_arcade_non_game_tree(path: &Path) -> bool {
    let mut previous_was_arcade = false;
    for component in path.components().filter_map(|c| c.as_os_str().to_str()) {
        if previous_was_arcade
            && (component.eq_ignore_ascii_case("media") || component.eq_ignore_ascii_case("cores"))
        {
            return true;
        }
        previous_was_arcade = component.eq_ignore_ascii_case("_Arcade");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_discovery::DiscoverySourceKind;
    use crate::launch_profiles::{self, ProfilePathClass};
    use crate::library_db::{BenchConfig, mtime_secs, scan_library};
    use crate::sqlite_catalog::{load_arcade_catalog_from_sqlite_at, save_sqlite_scan};
    use crate::test_support::*;
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn background_scanner_pauses_and_resumes_without_losing_discoveries() {
        let _test_lock = crate::cooperative_work::TEST_LOCK.lock().unwrap();
        let root = unique_temp_dir("background-scan-gate");
        let arcade = root.join("_Arcade");
        std::fs::create_dir_all(&arcade).unwrap();
        for index in 0..40 {
            std::fs::write(
                arcade.join(format!("Game {index}.mra")),
                format!("<misterromdescription><name>Game {index}</name><setname>game-{index}</setname></misterromdescription>"),
            )
            .unwrap();
        }
        crate::cooperative_work::set_background_allowed(false);
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker_root = root.clone();
        let worker = std::thread::spawn(move || {
            let _scope = crate::cooperative_work::BackgroundScope::enter();
            started_tx.send(()).unwrap();
            let cfg = BenchConfig {
                roots: vec![worker_root.display().to_string()],
                sqlite_path: worker_root.join("library.sqlite3"),
            };
            done_tx.send(scan_library(&cfg).discoveries.len()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(30)).is_err());
        crate::cooperative_work::set_background_allowed(true);
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(3)).unwrap(), 40);
        worker.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_ignored_support_files_do_not_become_payloads() {
        let profiles = launch_profiles::builtin_profiles();

        assert!(matches!(
            classify_profile_path(&profiles, Path::new("/media/fat/games/Saturn/boot.rom")),
            Some((profile, ProfilePathClass::Ignored { reason: launch_profiles::IgnoreReason::Bios, .. }))
                if profile.id == "saturn"
        ));
        assert!(matches!(
            classify_profile_path(&profiles, Path::new("/media/fat/games/AO486/boot1.rom")),
            Some((profile, ProfilePathClass::Ignored { reason: launch_profiles::IgnoreReason::Bios, .. }))
                if profile.id == "ao486"
        ));
    }

    #[test]
    fn atari_2600_shared_core_scan_keeps_distinct_system_and_games() {
        let root = unique_temp_dir("atari2600-shared-core-scan");
        install_test_console_core(&root, "Atari7800");
        std::fs::create_dir_all(root.join("games/Atari2600")).expect("create games");
        std::fs::write(
            root.join("_Console/Atari 2600.mgl"),
            r#"<mistergamedescription><rbf>_Console/Atari7800</rbf><setname>Atari2600</setname></mistergamedescription>"#,
        )
        .expect("write descriptor");
        std::fs::write(root.join("games/Atari2600/Adventure.a26"), b"rom").expect("write game");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        let profile = scan
            .profiles
            .iter()
            .find(|profile| profile.system_id == "atari2600")
            .expect("Atari 2600 profile");
        assert_eq!(
            profile.core_path.as_deref(),
            Some("_Console/Atari7800_20260630")
        );
        assert!(scan.discoveries.iter().any(|discovery| {
            discovery.platform_id == "atari2600" && discovery.title == "Adventure"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rbf_cores_are_not_profile_candidates() {
        let profiles = launch_profiles::builtin_profiles();

        assert!(
            classify_profile_path(
                &profiles,
                Path::new("/media/fat/_Computer/AcornAtom_20251001.rbf")
            )
            .is_none()
        );
        assert!(
            classify_profile_path(
                &profiles,
                Path::new("/media/fat/_LLAPI/NES_LLAPI_20251206.rbf")
            )
            .is_none()
        );
    }

    #[test]
    fn survivability_wild_sd_card_builds_limited_usable_catalog_and_audit() {
        let root = WildSdCardFixture::new("survivability-wild-sd")
            .install_console_core("Gameboy")
            .install_console_core("ColecoVision")
            .install_console_core("SMS")
            .install_console_core("NeoGeo")
            .write_arcade_mra("Puck Man.mra", "Puck Man", "puckman")
            .write_game("Gameboy", "Tetris.gb", b"gb")
            .write_game("Gameboy-Sinden", "Camera.gb", b"gb")
            .write_game("Coleco", "Smurf Rescue.col", b"col")
            .write_game_zip("SMS", "Packed.zip", &[("Hang On.sms", b"sms")])
            .write_game("Loose", "Zaxxon.sg", b"sg")
            .write_game("NotInstalledCore", "Mystery.nes", b"nes")
            .write_game("Saturn", "boot.rom", b"bios")
            .write_game(
                "NeoGeo-CD",
                "Metal Slug.cue",
                b"FILE \"Metal Slug.bin\" BINARY",
            )
            .write_game("NeoGeo-CD", "Metal Slug.bin", b"track")
            .write_game("NeoGeo-CD", "neocd.rom", b"bios")
            .build();
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);
        let profile_ids = scan
            .profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>();
        let unique_profile_ids = profile_ids.iter().copied().collect::<BTreeSet<_>>();

        assert_eq!(
            profile_ids.len(),
            unique_profile_ids.len(),
            "{profile_ids:?}"
        );
        assert!(
            scan.discoveries
                .iter()
                .any(|discovery| discovery.platform_id == "gameboy" && discovery.title == "Tetris")
        );
        assert!(
            scan.discoveries
                .iter()
                .any(|discovery| discovery.platform_id == "gameboy" && discovery.title == "Camera")
        );
        assert!(scan.discoveries.iter().any(|discovery| {
            discovery.platform_id == "colecovision" && discovery.title == "Smurf Rescue"
        }));
        assert!(
            scan.discoveries
                .iter()
                .any(|discovery| discovery.platform_id == "sms" && discovery.title == "Hang On")
        );
        assert!(scan.discoveries.iter().any(|discovery| {
            discovery.platform_id == "neogeo-cd" && discovery.title == "Metal Slug"
        }));
        assert!(!scan.discoveries.iter().any(|discovery| {
            discovery.title.contains("boot")
                || discovery.title.contains("neocd")
                || discovery.title.contains("Metal Slug.bin")
        }));
        assert!(scan.audit_rows.iter().any(|row| {
            row.expected_game_dir == "games/Loose"
                && row.catalog_status == "uncataloged"
                && row.reason == "ambiguous-alias"
        }));
        assert!(scan.audit_rows.iter().any(|row| {
            row.expected_game_dir == "games/NotInstalledCore"
                && row.catalog_status == "uncataloged"
                && row.reason == "no-installed-core"
        }));

        save_sqlite_scan(&db, &scan).expect("save limited but usable sqlite catalog");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");
        let conn = library_db::open_sqlite_read_only(&db).expect("open sqlite");
        let duplicate_profiles: i64 = conn
            .query_row(
                "SELECT count(*) FROM (
                    SELECT profile_id FROM profiles GROUP BY profile_id HAVING count(*) > 1
                )",
                [],
                |row| row.get(0),
            )
            .expect("query duplicate profiles");
        let launcher_rows_without_games: i64 = conn
            .query_row(
                "SELECT count(*) FROM launcher_catalog
                 LEFT JOIN launch_targets ON launch_targets.launch_id = launcher_catalog.launch_id
                 LEFT JOIN games ON games.game_key_id = launch_targets.game_key_id
                 WHERE launch_targets.launch_id IS NULL OR games.game_id IS NULL",
                [],
                |row| row.get(0),
            )
            .expect("query launcher consistency");
        let audit_rows: i64 = conn
            .query_row("SELECT count(*) FROM catalog_audit", [], |row| row.get(0))
            .expect("query audit rows");

        assert_eq!(duplicate_profiles, 0);
        assert_eq!(launcher_rows_without_games, 0);
        assert!(audit_rows >= 2);
        assert_eq!(loaded.catalog.system_game_count("gameboy"), 2);
        assert_eq!(loaded.catalog.system_game_count("colecovision"), 1);
        assert_eq!(loaded.catalog.system_game_count("sms"), 1);
        assert_eq!(loaded.catalog.system_game_count("neogeo-cd"), 1);
        assert!(loaded.catalog.system_game_count("arcade") >= 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_for_path_prefers_directory_after_games_component() {
        let profiles = launch_profiles::builtin_profiles();
        let profile = profile_for_path(
            &profiles,
            Path::new("/media/fat/collections/NES/games/NeoGeo/mslug3.neo"),
        )
        .expect("profile");

        assert_eq!(profile.id, "neogeo");
    }

    #[test]
    fn planned_target_matches_game_dir_header_case_insensitively() {
        assert!(same_library_path(
            Path::new("/media/fat/games/NeoGeo"),
            Path::new("/media/fat/games/NEOGEO")
        ));
        assert!(!same_library_path(
            Path::new("/media/fat/games/NeoGeo"),
            Path::new("/media/fat/games/NeoGeo-CD")
        ));
    }

    #[test]
    fn menu_mgl_launchers_are_not_profile_candidates() {
        let profiles = launch_profiles::builtin_profiles();

        assert!(
            classify_profile_path(&profiles, Path::new("/media/fat/_Computer/Amiga.mgl")).is_none()
        );
        assert!(
            classify_profile_path(&profiles, Path::new("/media/fat/_Console/Game Gear.mgl"))
                .is_none()
        );
    }

    #[test]
    fn target_streaming_emits_candidates_in_target_order_without_gaps() {
        let root = unique_temp_dir("target-streaming-order");
        let nes = root.join("games/NES");
        let snes = root.join("games/SNES");
        let gba = root.join("games/GBA");
        let gbc = root.join("games/GBC");
        std::fs::create_dir_all(&nes).expect("create nes dir");
        std::fs::create_dir_all(&snes).expect("create snes dir");
        std::fs::create_dir_all(&gba).expect("create gba dir");
        std::fs::create_dir_all(&gbc).expect("create gbc dir");
        let paths = [
            nes.join("01-first.nes"),
            snes.join("02-second.sfc"),
            gba.join("03-third.gba"),
            gbc.join("04-fourth.gbc"),
        ];
        for path in &paths {
            std::fs::write(path, "rom").expect("write candidate");
        }
        let profiles = launch_profiles::builtin_profiles();
        let candidate_exts = source_index_extensions(&profiles);
        let targets = vec![nes, snes, gba, gbc];
        let (tx, rx) = std::sync::mpsc::sync_channel(DISCOVERY_EVENT_BUFFER);

        let dirs = walk_index_candidates_streaming(targets, &profiles, &candidate_exts, &tx);
        drop(tx);
        let found = rx
            .try_iter()
            .map(|event| match event {
                DiscoveryEvent::TargetStart(_) | DiscoveryEvent::TargetComplete(_) => {
                    unreachable!("direct walk does not emit planned target boundaries")
                }
                DiscoveryEvent::File(file) => file.path,
                DiscoveryEvent::GameDirFacts(_) => {
                    unreachable!("direct walk does not collect game-dir facts")
                }
                DiscoveryEvent::RuntimeDirectory(_) => {
                    unreachable!("direct walk does not buffer runtime directories")
                }
                DiscoveryEvent::Done { .. } => unreachable!("direct walk does not send done"),
            })
            .collect::<Vec<_>>();

        assert_eq!(dirs, 4);
        assert_eq!(found, paths);
        let unique = found.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), paths.len());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn target_streaming_aborts_when_downstream_closes() {
        let root = unique_temp_dir("target-streaming-abort");
        let nes = root.join("games/NES");
        let snes = root.join("games/SNES");
        std::fs::create_dir_all(&nes).expect("create nes dir");
        std::fs::create_dir_all(&snes).expect("create snes dir");
        std::fs::write(nes.join("01-first.nes"), "rom").expect("write first candidate");
        std::fs::write(snes.join("02-second.sfc"), "rom").expect("write second candidate");
        let profiles = launch_profiles::builtin_profiles();
        let candidate_exts = source_index_extensions(&profiles);
        let targets = vec![nes, snes];
        let (tx, rx) = std::sync::mpsc::sync_channel(0);
        drop(rx);

        let dirs = walk_index_candidates_streaming(targets, &profiles, &candidate_exts, &tx);

        assert_eq!(dirs, 1);
    }

    #[test]
    fn planned_scan_brackets_each_target_with_stable_descriptors() {
        let root = unique_temp_dir("planned-target-boundaries");
        let arcade = root.join("_Arcade");
        std::fs::create_dir_all(&arcade).expect("create arcade dir");
        std::fs::write(arcade.join("game.mra"), "<misterromdescription/>")
            .expect("write arcade launcher");

        let roots = vec![root.display().to_string()];
        let plan = CatalogScanPlan::for_roots(&roots);
        let (tx, rx) = std::sync::mpsc::sync_channel(DISCOVERY_EVENT_BUFFER);
        let dirs = walk_index_candidates_with_plan(&roots, &plan, &[], &tx);
        drop(tx);

        let mut open = None;
        let mut completed = Vec::new();
        for event in rx.try_iter() {
            match event {
                DiscoveryEvent::TargetStart(descriptor) => {
                    assert!(
                        open.replace(descriptor).is_none(),
                        "targets must not overlap"
                    );
                }
                DiscoveryEvent::TargetComplete(descriptor) => {
                    let started = open.take().expect("target completion must have a start");
                    assert_eq!(descriptor, started);
                    completed.push(descriptor);
                }
                DiscoveryEvent::File(_)
                | DiscoveryEvent::GameDirFacts(_)
                | DiscoveryEvent::RuntimeDirectory(_) => {
                    assert!(open.is_some(), "target payload must be bracketed");
                }
                DiscoveryEvent::Done { .. } => unreachable!("direct planned walk has no done"),
            }
        }

        assert!(open.is_none(), "last target must be complete");
        assert!(!completed.is_empty());
        assert_eq!(
            completed
                .iter()
                .map(|target| target.ordinal)
                .collect::<Vec<_>>(),
            (0..completed.len()).collect::<Vec<_>>()
        );
        assert!(
            completed
                .iter()
                .any(|target| { target.path == arcade && target.kind == ScanTargetKind::Static })
        );
        assert!(dirs >= 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn neogeo_zip_entries_generate_virtual_launches_and_system() {
        let root = unique_temp_dir("neogeo-zip-entries");
        let neogeo_dir = root.join("games/NEOGEO");
        std::fs::create_dir_all(&neogeo_dir).expect("create neogeo dir");
        let zip_path = neogeo_dir.join("Neo Geo Mister FGPA Ultra Pack.zip");
        write_stored_zip(
            &zip_path,
            &[
                (
                    "Neo Geo Mister FGPA Ultra Pack/ World A-Z/Neo Bomberman (neobombe).neo",
                    b"neo",
                ),
                ("Neo Geo Mister FGPA Ultra Pack/readme.txt", b"ignore"),
            ],
        );
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.containers.len(), 1);
        assert_eq!(scan.entries.len(), 1);
        assert!(scan.normal_files.is_empty());
        let discovery = scan
            .discoveries
            .iter()
            .find(|d| d.source_kind == DiscoverySourceKind::ArchiveEntry)
            .expect("archive entry discovery");
        assert_eq!(discovery.platform_id, "neogeo");
        let member = crate::archive_member::decode_archive_member_ref(&discovery.launch_ref)
            .expect("decode archive launch ref")
            .expect("explicit archive member");
        assert_eq!(member.archive_path, zip_path.display().to_string());
        assert_eq!(
            member.member_path,
            "Neo Geo Mister FGPA Ultra Pack/ World A-Z/Neo Bomberman (neobombe).neo"
        );

        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(loaded.rows, 1);
        assert_eq!(loaded.catalog.games[0].system_id.as_ref(), "neogeo");
        assert!(loaded.catalog.games[0].mra_path.starts_with("magik-plan:"));
        assert!(
            loaded
                .catalog
                .systems
                .iter()
                .any(|system| system.id == "neogeo" && system.count == 1)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn zip_central_directory_scans_entries_with_extra_and_comment_padding() {
        let root = unique_temp_dir("zip-central-padding");
        std::fs::create_dir_all(&root).expect("create temp root");
        let zip_path = root.join("games.zip");
        write_stored_zip_with_central_metadata(
            &zip_path,
            &[("World A-Z/Neo Bomberman (neobombe).neo", b"neo".as_slice())],
            b"extra",
            b"comment",
        );
        let meta = std::fs::metadata(&zip_path).expect("stat zip");
        let file = FoundFile {
            path: zip_path.clone(),
            ext: "zip".to_string(),
            size: meta.len(),
            mtime_secs: mtime_secs(&meta),
        };
        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "neogeo")
            .expect("neogeo profile");

        let entries = scan_zip_central_directory(&file, profile).expect("scan zip");

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].entry_path,
            "World A-Z/Neo Bomberman (neobombe).neo"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn zip_central_directory_decodes_zip64_member_metadata() {
        let root = unique_temp_dir("zip64-member");
        std::fs::create_dir_all(&root).expect("create temp root");
        let zip_path = root.join("games.zip");
        write_stored_zip64_member(&zip_path, "World A-Z/Neo Bomberman (neobombe).neo", b"neo");
        let meta = std::fs::metadata(&zip_path).expect("stat zip");
        let file = FoundFile {
            path: zip_path.clone(),
            ext: "zip".to_string(),
            size: meta.len(),
            mtime_secs: mtime_secs(&meta),
        };
        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "neogeo")
            .expect("neogeo profile");

        let entries = scan_zip_central_directory(&file, profile).expect("scan zip");
        let member = crate::archive_member::decode_archive_member_ref(&entries[0].launch_ref)
            .expect("decode launch ref")
            .expect("archive member");

        assert_eq!(member.compressed_size, 3);
        assert_eq!(member.uncompressed_size, 3);
        assert_eq!(member.local_header_offset, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn zip_central_directory_skips_large_metadata_padding() {
        let root = unique_temp_dir("zip-central-large-padding");
        std::fs::create_dir_all(&root).expect("create temp root");
        let zip_path = root.join("games.zip");
        let extra = vec![0x5a; ZIP_SKIP_BUFFER_BYTES + 17];
        let comment = vec![0xa5; ZIP_SKIP_BUFFER_BYTES + 31];
        write_stored_zip_with_central_metadata(
            &zip_path,
            &[
                (
                    "World A-Z/2020 Super Baseball (2020bb).neo",
                    b"neo".as_slice(),
                ),
                ("World A-Z/Neo Bomberman (neobombe).neo", b"neo".as_slice()),
            ],
            &extra,
            &comment,
        );
        let meta = std::fs::metadata(&zip_path).expect("stat zip");
        let file = FoundFile {
            path: zip_path.clone(),
            ext: "zip".to_string(),
            size: meta.len(),
            mtime_secs: mtime_secs(&meta),
        };
        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "neogeo")
            .expect("neogeo profile");

        let entries = scan_zip_central_directory(&file, profile).expect("scan zip");

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[1].entry_path,
            "World A-Z/Neo Bomberman (neobombe).neo"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_ignores_gamelists_and_screenshot_media_dirs() {
        let root = unique_temp_dir("ignore-screenshot-media");
        install_test_console_core(&root, "NES");
        let nes_dir = root.join("games/NES");
        let screenshot_dir = nes_dir.join("screenshot");
        std::fs::create_dir_all(&screenshot_dir).expect("create screenshot dir");
        std::fs::write(nes_dir.join("Mario.nes"), "rom").expect("write rom");
        std::fs::write(
            nes_dir.join("gamelist.xml"),
            "<game><path>./Mario.nes</path><image>./screenshot/Mario.png</image></game>",
        )
        .expect("write gamelist");
        std::fs::write(screenshot_dir.join("Not A Game.nes"), "media").expect("write media");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 1);
        assert_eq!(scan.discoveries.len(), 1);
        assert_eq!(
            scan.normal_files[0].path,
            nes_dir.join("Mario.nes").display().to_string()
        );
        assert!(
            scan.discoveries
                .iter()
                .all(|discovery| !discovery.launch_ref.contains("gamelist.xml")
                    && !discovery.launch_ref.contains("Not A Game.nes"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_prunes_hidden_files_and_dirs_before_candidate_work() {
        let root = unique_temp_dir("ignore-hidden-files");
        let arcade_dir = root.join("_Arcade");
        let hidden_dir = arcade_dir.join(".metadata-cache");
        std::fs::create_dir_all(&hidden_dir).expect("create hidden dir");
        std::fs::write(
            arcade_dir.join("Real Game.mra"),
            "<misterromdescription><name>Real Game</name><setname>realgame</setname></misterromdescription>",
        )
        .expect("write real mra");
        std::fs::write(
            arcade_dir.join("._Puck Man (JP, Set 1).mra"),
            "<misterromdescription><name>AppleDouble Sidecar</name></misterromdescription>",
        )
        .expect("write apple sidecar");
        std::fs::write(
            arcade_dir.join(".DS_Store.mra"),
            "<misterromdescription><name>Finder Metadata</name></misterromdescription>",
        )
        .expect("write ds store candidate");
        std::fs::write(
            hidden_dir.join("Hidden Game.mra"),
            "<misterromdescription><name>Hidden Game</name></misterromdescription>",
        )
        .expect("write hidden game");
        let profiles = launch_profiles::builtin_profiles();
        let candidate_exts = source_index_extensions(&profiles);
        let mut found = Vec::new();

        let stats = scan_target_candidates(&arcade_dir, &profiles, &candidate_exts, |file| {
            found.push(file.path);
            true
        });

        assert_eq!(stats.files, 1);
        assert_eq!(stats.candidates, 1);
        assert_eq!(found, vec![arcade_dir.join("Real Game.mra")]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_ignores_hidden_zip_entries() {
        let root = unique_temp_dir("ignore-hidden-zip-entries");
        let neogeo_dir = root.join("games/NEOGEO");
        std::fs::create_dir_all(&neogeo_dir).expect("create neogeo dir");
        let zip_path = neogeo_dir.join("NeoGeo Additions.zip");
        write_stored_zip(
            &zip_path,
            &[
                ("Visible/Real Game.neo", b"neo"),
                ("__MACOSX/Visible/Real Game.neo", b"metadata"),
                ("__MACOSX/Visible/._Real Game.neo", b"sidecar"),
                ("Visible/._Real Game.neo", b"sidecar"),
                ("Visible/.DS_Store.neo", b"metadata"),
                (".metadata-cache/Hidden Game.neo", b"hidden"),
            ],
        );
        let meta = std::fs::metadata(&zip_path).expect("stat zip");
        let file = FoundFile {
            path: zip_path.clone(),
            ext: "zip".to_string(),
            size: meta.len(),
            mtime_secs: mtime_secs(&meta),
        };
        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "neogeo")
            .expect("neogeo profile");

        let entries = scan_zip_central_directory(&file, profile).expect("scan zip");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry_path, "Visible/Real Game.neo");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_adds_exact_runtime_game_dirs_without_walking_unmatched_dirs() {
        let root = unique_temp_dir("target-runtime-game-dirs");
        install_test_console_core(&root, "Gameboy");
        let gameboy_dir = root.join("games/Gameboy");
        let unrelated_dir = root.join("games/NotACoreProfile");
        std::fs::create_dir_all(&gameboy_dir).expect("create gameboy dir");
        std::fs::create_dir_all(&unrelated_dir).expect("create unrelated dir");
        std::fs::write(gameboy_dir.join("Tetris.gb"), "rom").expect("write gameboy rom");
        std::fs::write(unrelated_dir.join("Ghost.nope"), "rom").expect("write unrelated rom");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 1);
        assert_eq!(
            scan.normal_files[0].path,
            gameboy_dir.join("Tetris.gb").display().to_string()
        );
        assert!(
            scan.profiles
                .iter()
                .any(|profile| profile.id == "runtime-gameboy")
        );
        assert!(
            scan.discoveries
                .iter()
                .any(|discovery| discovery.platform_id == "gameboy" && discovery.title == "Tetris")
        );
        assert!(
            scan.discoveries
                .iter()
                .all(|discovery| !discovery.launch_ref.contains("Ghost.nope"))
        );
        assert!(scan.audit_rows.iter().any(|row| {
            row.expected_game_dir == "games/NotACoreProfile"
                && row.catalog_status == "uncataloged"
                && row.reason == "no-installed-core"
        }));

        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let conn = library_db::open_sqlite_read_only(&db).expect("open sqlite");
        let systems: i64 = conn
            .query_row(
                "SELECT count(*) FROM systems WHERE system_id='gameboy'",
                [],
                |row| row.get(0),
            )
            .expect("query systems");
        let games: i64 = conn
            .query_row(
                "SELECT count(*) FROM games WHERE system_id='gameboy'",
                [],
                |row| row.get(0),
            )
            .expect("query games");
        let launcher_rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM launcher_catalog WHERE system_id='gameboy'",
                [],
                |row| row.get(0),
            )
            .expect("query launcher");
        assert_eq!((systems, games, launcher_rows), (0, 1, 0));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_derives_exact_runtime_extensions_for_unmanifested_cores() {
        let root = unique_temp_dir("target-runtime-derived-extensions");
        std::fs::create_dir_all(root.join("_Computer")).expect("create computer dir");
        std::fs::write(root.join("_Computer/BBCMicro_20260630.rbf"), b"rbf")
            .expect("write bbc micro core");
        let bbc_dir = root.join("games/BBCMicro");
        std::fs::create_dir_all(&bbc_dir).expect("create bbc micro dir");
        std::fs::write(bbc_dir.join("Elite.ssd"), "disk").expect("write disk");
        std::fs::write(bbc_dir.join("metadata.xml"), "xml").expect("write metadata");
        std::fs::write(bbc_dir.join("cover.jpg"), "jpg").expect("write image");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 1);
        assert_eq!(
            scan.normal_files[0].path,
            bbc_dir.join("Elite.ssd").display().to_string()
        );
        assert!(scan.profiles.iter().any(|profile| {
            profile.id == "runtime-bbcmicro"
                && profile.system_id == "bbcmicro"
                && profile.game_dirs == vec!["BBCMicro".to_string()]
        }));
        assert!(
            scan.discoveries
                .iter()
                .any(|discovery| discovery.platform_id == "bbcmicro" && discovery.title == "Elite")
        );
        assert!(
            scan.discoveries
                .iter()
                .all(|discovery| !discovery.launch_ref.ends_with("metadata.xml"))
        );

        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let conn = library_db::open_sqlite_read_only(&db).expect("open sqlite");
        let systems: i64 = conn
            .query_row(
                "SELECT count(*) FROM systems WHERE system_id='bbcmicro'",
                [],
                |row| row.get(0),
            )
            .expect("query systems");
        let games: i64 = conn
            .query_row(
                "SELECT count(*) FROM games WHERE system_id='bbcmicro'",
                [],
                |row| row.get(0),
            )
            .expect("query games");
        let launcher_rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM launcher_catalog WHERE system_id='bbcmicro'",
                [],
                |row| row.get(0),
            )
            .expect("query launcher");
        assert_eq!((systems, games, launcher_rows), (0, 1, 0));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_does_not_create_empty_runtime_system_rows() {
        let root = unique_temp_dir("target-runtime-empty");
        install_test_console_core(&root, "Gameboy");
        std::fs::create_dir_all(root.join("games/Gameboy")).expect("create empty gameboy dir");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert!(scan.normal_files.is_empty());
        assert!(scan.discoveries.is_empty());

        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let conn = library_db::open_sqlite_read_only(&db).expect("open sqlite");
        let systems: i64 = conn
            .query_row(
                "SELECT count(*) FROM systems WHERE system_id='gameboy'",
                [],
                |row| row.get(0),
            )
            .expect("query systems");
        let games: i64 = conn
            .query_row(
                "SELECT count(*) FROM games WHERE system_id='gameboy'",
                [],
                |row| row.get(0),
            )
            .expect("query games");
        let launcher_rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM launcher_catalog WHERE system_id='gameboy'",
                [],
                |row| row.get(0),
            )
            .expect("query launcher");
        assert_eq!((systems, games, launcher_rows), (0, 0, 0));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_catalogs_unique_extension_alias_game_dirs() {
        let root = unique_temp_dir("target-runtime-coleco-alias");
        install_test_console_core(&root, "ColecoVision");
        let coleco_dir = root.join("games/Coleco");
        std::fs::create_dir_all(&coleco_dir).expect("create coleco alias dir");
        std::fs::write(coleco_dir.join("Smurf Rescue.col"), "rom").expect("write coleco rom");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 1);
        assert!(scan.profiles.iter().any(|profile| {
            profile.id == "runtime-colecovision" && profile.game_dirs == vec!["Coleco".to_string()]
        }));
        assert!(scan.discoveries.iter().any(|discovery| {
            discovery.platform_id == "colecovision" && discovery.title == "Smurf Rescue"
        }));

        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let conn = library_db::open_sqlite_read_only(&db).expect("open sqlite");
        let systems: i64 = conn
            .query_row(
                "SELECT count(*) FROM systems WHERE system_id='colecovision'",
                [],
                |row| row.get(0),
            )
            .expect("query systems");
        let games: i64 = conn
            .query_row(
                "SELECT count(*) FROM games WHERE system_id='colecovision'",
                [],
                |row| row.get(0),
            )
            .expect("query games");
        let launcher_rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM launcher_catalog WHERE system_id='colecovision'",
                [],
                |row| row.get(0),
            )
            .expect("query launcher");
        assert_eq!((systems, games, launcher_rows), (1, 1, 1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_catalogs_spectrum_alias_without_shared_rom_ambiguity() {
        let root = unique_temp_dir("target-runtime-spectrum-alias");
        std::fs::create_dir_all(root.join("_Computer")).expect("create computer dir");
        install_test_console_core(&root, "ColecoVision");
        install_test_console_core(&root, "Intellivision");
        let spectrum_dir = root.join("games/Spectrum");
        std::fs::create_dir_all(&spectrum_dir).expect("create spectrum dir");
        std::fs::write(root.join("_Computer/ZX-Spectrum_20260630.rbf"), b"rbf")
            .expect("write spectrum core");
        std::fs::write(spectrum_dir.join("Jet Set Willy.tzx"), "tape")
            .expect("write spectrum tape");
        std::fs::write(spectrum_dir.join("support.rom"), "bios").expect("write support rom");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert!(scan.profiles.iter().any(|profile| {
            profile.id == "runtime-zx-spectrum"
                && profile.system_id == "zx-spectrum"
                && profile.game_dirs == vec!["Spectrum".to_string()]
        }));
        assert!(scan.discoveries.iter().any(|discovery| {
            discovery.platform_id == "zx-spectrum" && discovery.title == "Jet Set Willy"
        }));
        assert!(
            scan.discoveries
                .iter()
                .all(|discovery| !discovery.launch_ref.ends_with("support.rom"))
        );

        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let conn = library_db::open_sqlite_read_only(&db).expect("open sqlite");
        let systems: i64 = conn
            .query_row(
                "SELECT count(*) FROM systems WHERE system_id='zx-spectrum'",
                [],
                |row| row.get(0),
            )
            .expect("query systems");
        let games: i64 = conn
            .query_row(
                "SELECT count(*) FROM games WHERE system_id='zx-spectrum'",
                [],
                |row| row.get(0),
            )
            .expect("query games");
        let launcher_rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM launcher_catalog WHERE system_id='zx-spectrum'",
                [],
                |row| row.get(0),
            )
            .expect("query launcher");
        let launch_target: (String, i64, i64) = conn
            .query_row(
                "SELECT launch_targets.mount_kind,
                        launch_targets.mount_index,
                        launch_targets.delay_secs
                 FROM launch_targets
                 JOIN games ON games.game_key_id=launch_targets.game_key_id
                 WHERE games.system_id='zx-spectrum'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("query spectrum launch target");
        assert_eq!((systems, games, launcher_rows), (0, 1, 0));
        assert_eq!(launch_target, ("load-file".to_string(), 1, 1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_preserves_runtime_profile_mounts_when_profile_id_is_system_id() {
        let root = unique_temp_dir("target-runtime-profile-mount");
        install_test_console_core(&root, "Intellivision");
        let game_dir = root.join("games/Intellivision");
        std::fs::create_dir_all(&game_dir).expect("create intellivision dir");
        std::fs::write(game_dir.join("Armor Battle.int"), "rom").expect("write intellivision rom");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert!(scan.profiles.iter().any(|profile| {
            profile.id == "runtime-intellivision"
                && profile.system_id == "intellivision"
                && profile.game_dirs == vec!["Intellivision".to_string()]
        }));

        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let conn = library_db::open_sqlite_read_only(&db).expect("open sqlite");
        let launch_target: (String, i64, i64) = conn
            .query_row(
                "SELECT launch_targets.mount_kind,
                        launch_targets.mount_index,
                        launch_targets.delay_secs
                 FROM launch_targets
                 JOIN games ON games.game_key_id=launch_targets.game_key_id
                 WHERE games.system_id='intellivision'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("query intellivision launch target");

        assert_eq!(launch_target, ("load-file".to_string(), 1, 1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_keeps_ambiguous_extension_aliases_audited_only() {
        let root = unique_temp_dir("target-runtime-ambiguous-alias");
        install_test_console_core(&root, "ColecoVision");
        install_test_console_core(&root, "SMS");
        let loose_dir = root.join("games/Loose");
        std::fs::create_dir_all(&loose_dir).expect("create loose dir");
        std::fs::write(loose_dir.join("Zaxxon.sg"), "rom").expect("write sg rom");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert!(scan.normal_files.is_empty());
        assert!(scan.discoveries.is_empty());
        assert!(scan.audit_rows.iter().any(|row| {
            row.expected_game_dir == "games/Loose"
                && row.catalog_status == "uncataloged"
                && row.reason == "ambiguous-alias"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_uses_numeric_core_alias_and_boot_rom_evidence_for_pc8801() {
        let root = unique_temp_dir("target-runtime-pc8801-alias");
        install_test_console_core(&root, "PC88");
        let game_dir = root.join("games/PC8801");
        std::fs::create_dir_all(&game_dir).expect("create PC8801 dir");
        std::fs::write(game_dir.join("boot.rom"), "firmware").expect("write boot ROM");
        std::fs::write(game_dir.join("Ys.7z"), "archive").expect("write PC8801 game");
        std::fs::write(game_dir.join("Thexder.7z"), "archive").expect("write PC8801 game");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        let pc88_games = scan
            .discoveries
            .iter()
            .filter(|discovery| discovery.platform_id == "pc88")
            .map(|discovery| discovery.title.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(pc88_games, BTreeSet::from(["Thexder", "Ys"]));
        assert!(
            scan.discoveries
                .iter()
                .all(|discovery| discovery.title != "boot")
        );
        assert!(scan.audit_rows.iter().any(|row| {
            row.expected_game_dir == "games/PC8801"
                && row.catalog_status == "cataloged"
                && row.core_id == "PC88"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cartridge_zip_entries_index_as_games() {
        let root = unique_temp_dir("sms-loose-vs-zip");
        install_test_console_core(&root, "SMS");
        let sms_dir = root.join("games/SMS");
        std::fs::create_dir_all(&sms_dir).expect("create sms dir");
        std::fs::write(sms_dir.join("Loose.sms"), "rom").expect("write sms rom");
        write_stored_zip(&sms_dir.join("Packed.zip"), &[("Packed.sms", b"rom")]);
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 1);
        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.discoveries.len(), 2);
        assert!(
            scan.discoveries
                .iter()
                .all(|discovery| discovery.platform_id == "sms")
        );
        assert!(scan.audit_rows.iter().all(|row| {
            row.expected_game_dir != "games/SMS" || !row.reason.contains("zip-archive-not-indexed")
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn installed_manifest_core_catalogs_loose_colecovision_games() {
        let root = unique_temp_dir("colecovision-loose-visible");
        install_test_console_core(&root, "ColecoVision");
        let coleco_dir = root.join("games/ColecoVision");
        std::fs::create_dir_all(&coleco_dir).expect("create colecovision dir");
        std::fs::write(coleco_dir.join("Mouse Trap.col"), "rom").expect("write coleco rom");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 1);
        assert!(
            scan.profiles
                .iter()
                .any(|profile| profile.id == "colecovision")
        );
        assert!(scan.discoveries.iter().any(|discovery| {
            discovery.platform_id == "colecovision" && discovery.title == "Mouse Trap"
        }));
        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");
        assert!(
            loaded
                .catalog
                .systems
                .iter()
                .any(|system| system.id == "colecovision" && system.count == 1)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn installed_manifest_core_catalogs_colecovision_zip_entries() {
        let root = unique_temp_dir("colecovision-zip-visible");
        install_test_console_core(&root, "ColecoVision");
        let coleco_dir = root.join("games/ColecoVision");
        std::fs::create_dir_all(&coleco_dir).expect("create colecovision dir");
        write_stored_zip(
            &coleco_dir.join("Additions.zip"),
            &[("Venture (USA).col", b"rom"), ("readme.txt", b"ignore")],
        );
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 0);
        assert_eq!(scan.entries.len(), 1);
        assert!(scan.discoveries.iter().any(|discovery| {
            discovery.source_kind == DiscoverySourceKind::ArchiveEntry
                && discovery.platform_id == "colecovision"
                && discovery.title.starts_with("Venture")
        }));
        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");
        assert!(
            loaded
                .catalog
                .systems
                .iter()
                .any(|system| system.id == "colecovision" && system.count == 1)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn installed_manifest_core_with_empty_game_dir_audits_cataloged_zero_games() {
        let root = unique_temp_dir("colecovision-empty-cataloged");
        install_test_console_core(&root, "ColecoVision");
        std::fs::create_dir_all(root.join("games/ColecoVision"))
            .expect("create empty colecovision dir");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert!(scan.discoveries.is_empty());
        assert!(scan.audit_rows.iter().any(|row| {
            row.core_id == "ColecoVision"
                && row.expected_game_dir == "games/ColecoVision"
                && row.catalog_status == "cataloged"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_game_dir_without_installed_core_is_support_only_not_launchable() {
        let root = unique_temp_dir("colecovision-folder-no-core");
        let coleco_dir = root.join("games/ColecoVision");
        std::fs::create_dir_all(&coleco_dir).expect("create colecovision dir");
        std::fs::write(coleco_dir.join("Mouse Trap.col"), "rom").expect("write coleco rom");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert!(scan.normal_files.is_empty());
        assert!(scan.discoveries.is_empty());
        assert!(
            scan.profiles
                .iter()
                .all(|profile| profile.id != "colecovision")
        );
        assert!(scan.audit_rows.iter().any(|row| {
            row.core_id == "ColecoVision"
                && row.expected_game_dir == "games/ColecoVision"
                && row.catalog_status == "support-only"
                && row.reason == "no-installed-core"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn wonderswan_zip_entries_generate_visible_system() {
        let root = unique_temp_dir("wonderswan-zip-entries");
        install_test_console_core(&root, "WonderSwan");
        let ws_dir = root.join("games/WonderSwan");
        std::fs::create_dir_all(&ws_dir).expect("create wonderswan dir");
        write_stored_zip(
            &ws_dir.join("Packed WonderSwan Games.zip"),
            &[("Gunpey (Japan).ws", b"rom")],
        );
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.entries.len(), 1);
        assert!(scan.discoveries.iter().any(|discovery| {
            discovery.platform_id == "wonderswan" && discovery.title.contains("Gunpey")
        }));
        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");
        assert!(
            loaded
                .catalog
                .systems
                .iter()
                .any(|system| system.id == "wonderswan" && system.count == 1)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn primary_scan_retains_direct_zip_paths_for_deferred_audit() {
        let root = unique_temp_dir("scan-retained-direct-zip-audit");
        let psx_dir = root.join("games/PSX");
        let nested_dir = psx_dir.join("Nested");
        let direct_zip = psx_dir.join("Packed PSX Games.zip");
        let nested_zip = nested_dir.join("Nested PSX Games.zip");
        std::fs::create_dir_all(&nested_dir).expect("create psx dirs");
        write_stored_zip(&direct_zip, &[("Game.cue", b"cue")]);
        write_stored_zip(&nested_zip, &[("Nested.cue", b"cue")]);
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);
        let facts = scan
            .game_dir_facts
            .iter()
            .find(|facts| facts.name == "PSX")
            .expect("PSX facts");

        assert!(facts.has_zip_files);
        assert_eq!(facts.direct_zip_paths, vec![direct_zip.clone()]);
        assert!(scan.audit_rows.iter().any(|row| {
            row.reason == format!("zip-archive-not-indexed:{}", direct_zip.display())
        }));
        assert!(
            !scan
                .audit_rows
                .iter()
                .any(|row| row.reason.contains("Nested PSX Games.zip"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn empty_static_target_with_nested_directory_forces_exact_warm_fallback() {
        let root = unique_temp_dir("scan-empty-static-nested-probe");
        std::fs::create_dir_all(root.join("games/NES/Incoming")).expect("create empty nested dir");
        install_test_console_core(&root, "NES");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);
        let facts = scan
            .game_dir_facts
            .iter()
            .find(|facts| facts.name == "NES")
            .expect("NES facts");

        assert!(!facts.has_payloadish_files());
        assert_eq!(
            facts.signature,
            crate::catalog_discovery::GameDirSignature::Unavailable
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_skips_attached_media_only_targets_but_keeps_dos_mgl_launchers() {
        let root = unique_temp_dir("skip-attached-media-target");
        install_test_console_core(&root, "NES");
        let dos_dir = root.join("_DOS Games");
        let ao486_dir = root.join("games/AO486");
        let nes_dir = root.join("games/NES");
        std::fs::create_dir_all(&dos_dir).expect("create dos dir");
        std::fs::create_dir_all(&ao486_dir).expect("create ao486 dir");
        std::fs::create_dir_all(&nes_dir).expect("create nes dir");
        let dos_mgl = dos_dir.join("Doom.mgl");
        std::fs::write(
            &dos_mgl,
            r#"<mistergamelist><rbf>AO486</rbf><file delay="1" type="s">../games/AO486/Doom.vhd</file></mistergamelist>"#,
        )
        .expect("write dos mgl");
        let raw_media = ao486_dir.join("Doom.vhd");
        std::fs::write(&raw_media, "disk").expect("write raw ao486 media");
        let nes_rom = nes_dir.join("Mario.nes");
        std::fs::write(&nes_rom, "rom").expect("write nes rom");
        let profiles = launch_profiles::builtin_profiles();
        let targets = scan_targets_for_roots(&[root.display().to_string()], &profiles);

        assert!(targets.iter().any(|target| target == &dos_dir));
        assert!(targets.iter().any(|target| target == &nes_dir));
        assert!(!targets.iter().any(|target| target == &ao486_dir));

        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };
        let scan = scan_library(&cfg);

        assert!(scan.discoveries.iter().any(|discovery| discovery.launch_ref
            == dos_mgl.display().to_string()
            && discovery.platform_id == "dos"));
        assert!(
            scan.discoveries
                .iter()
                .any(|discovery| discovery.launch_ref == nes_rom.display().to_string())
        );
        assert!(
            !scan
                .discoveries
                .iter()
                .any(|discovery| discovery.launch_ref == raw_media.display().to_string())
        );
        assert!(
            !scan
                .normal_files
                .iter()
                .any(|file| file.path == raw_media.display().to_string())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_arcade_root_does_not_pull_in_prepared_computer_collection() {
        let root = unique_temp_dir("explicit-arcade-target");
        let arcade = root.join("_Arcade");
        let prepared = root.join("_Computer/X68000 Games");
        std::fs::create_dir_all(&arcade).expect("create Arcade");
        std::fs::create_dir_all(&prepared).expect("create prepared collection");

        let profiles = launch_profiles::builtin_profiles();
        let targets = scan_targets_for_roots(&[arcade.display().to_string()], &profiles);

        assert_eq!(targets, vec![arcade]);
        assert!(!targets.contains(&prepared));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_discovers_preinstalled_x68000_game_mgls_under_computer() {
        let root = unique_temp_dir("neon68k-mgl-scan");
        let launcher_dir = root.join("_Computer/X68000 Games/Minor Bugs");
        let payload_dir = root.join("games/X68000");
        std::fs::create_dir_all(&launcher_dir).expect("create launcher dir");
        std::fs::create_dir_all(&payload_dir).expect("create payload dir");
        std::fs::write(payload_dir.join("Akumajou.hdf"), b"hdf").expect("write HDF");
        let mgl = launcher_dir.join("Akumajou Dracula.mgl");
        std::fs::write(
            &mgl,
            r#"<mistergamedescription><rbf>X68000</rbf><setname>Akumajou</setname><file path="../../../games/X68000/Akumajou.hdf"/></mistergamedescription>"#,
        )
        .expect("write MGL");
        let cfg = BenchConfig {
            roots: vec![root.join("games").display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);
        let discovery = scan
            .discoveries
            .iter()
            .find(|discovery| discovery.launch_ref == mgl.display().to_string())
            .expect("discover X68000 MGL");

        assert_eq!(discovery.platform_id, "x68000");
        assert_eq!(discovery.setname.as_deref(), Some("Akumajou"));
        assert_eq!(discovery.genre.as_deref(), Some("Neon68K / Minor Bugs"));
        assert_eq!(
            discovery.prepared.map(|value| value.collection_id),
            Some(crate::prepared_collections::PreparedCollectionId::Neon68k)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_marks_only_primary_oneload64_crts_as_prepared() {
        let root = unique_temp_dir("oneload64-scan");
        std::fs::create_dir_all(root.join("_Computer")).expect("create computer dir");
        std::fs::write(root.join("_Computer/C64_20260630.rbf"), b"rbf").expect("write core");
        let install = root.join("games/C64/OneLoad64 Games Collection v4");
        let multi = install.join("MultiLoad64");
        let dumps = install.join("Dumps");
        let alternatives = install.join("AlternativeFormats");
        for path in [&multi, &dumps, &alternatives] {
            std::fs::create_dir_all(path).expect("create collection dir");
        }
        let primary = install.join("Impossible Mission.crt");
        let multiload = multi.join("Summer Games.crt");
        let dump = dumps.join("Dump.crt");
        for path in [&primary, &multiload, &dump] {
            std::fs::write(path, b"crt").expect("write CRT");
        }
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);
        let prepared_paths = scan
            .discoveries
            .iter()
            .filter(|discovery| {
                discovery.prepared.is_some_and(|prepared| {
                    prepared.collection_id
                        == crate::prepared_collections::PreparedCollectionId::OneLoad64
                })
            })
            .map(|discovery| discovery.launch_ref.as_str())
            .collect::<Vec<_>>();

        assert!(prepared_paths.contains(&primary.to_str().expect("primary path")));
        assert!(prepared_paths.contains(&multiload.to_str().expect("multiload path")));
        assert!(!prepared_paths.contains(&dump.to_str().expect("dump path")));
        save_sqlite_scan(&cfg.sqlite_path, &scan).expect("save catalog");
        let conn = rusqlite::Connection::open(&cfg.sqlite_path).expect("open catalog");
        let prepared_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM prepared_launch_rows WHERE collection_id='oneload64'",
                [],
                |row| row.get(0),
            )
            .expect("count prepared launches");
        assert_eq!(prepared_count, 2);
        let generic_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM launch_provenance WHERE launch_quality='generic'",
                [],
                |row| row.get(0),
            )
            .expect("count generic launches");
        assert_eq!(generic_count, 1);
        let excluded_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM prepared_launch_diagnostic_rows WHERE collection_id='oneload64' AND status='excluded'",
                [],
                |row| row.get(0),
            )
            .expect("count excluded collection files");
        assert_eq!(excluded_count, 1);
        let loaded =
            load_arcade_catalog_from_sqlite_at(&root, &cfg.sqlite_path).expect("load catalog");
        let game = loaded
            .catalog
            .games
            .iter()
            .find(|game| game.title.as_ref() == "Impossible Mission")
            .expect("find primary game");
        let target = loaded.catalog.launch_target_for_ref(&game.mra_path);
        let crate::arcade_catalog::LaunchTarget::Structured(plan) = target else {
            panic!("expected structured OneLoad64 plan");
        };
        assert_eq!(plan.mount_kind.as_ref(), "load-file");
        assert_eq!(plan.mount_index, 1);
        assert_eq!(plan.payload_path.as_ref(), primary.display().to_string());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_prepared_mgl_stays_generic_and_records_diagnostic() {
        let root = unique_temp_dir("invalid-prepared-mgl");
        let dos = root.join("_DOS Games");
        std::fs::create_dir_all(&dos).expect("create DOS dir");
        let mgl = dos.join("Broken Game.mgl");
        std::fs::write(
            &mgl,
            r#"<mistergamedescription><rbf>Minimig</rbf><file path="missing.vhd"/><reset/></mistergamedescription>"#,
        )
        .expect("write invalid MGL");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);
        let discovery = scan
            .discoveries
            .iter()
            .find(|discovery| discovery.launch_ref == mgl.display().to_string())
            .expect("retain generic MGL discovery");
        assert!(discovery.prepared.is_none());
        save_sqlite_scan(&cfg.sqlite_path, &scan).expect("save catalog");
        let conn = rusqlite::Connection::open(&cfg.sqlite_path).expect("open catalog");
        let diagnostic: (String, String, String) = conn
            .query_row(
                "SELECT collection_id,status,reason FROM prepared_launch_diagnostic_rows",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read prepared launch diagnostic");
        assert_eq!(diagnostic.0, "0mhz");
        assert_eq!(diagnostic.1, "invalid");
        assert!(diagnostic.2.contains("expected AO486"));
        let loaded = load_arcade_catalog_from_sqlite_at(&root, &cfg.sqlite_path)
            .expect("load generic fallback");
        assert!(
            loaded
                .catalog
                .games
                .iter()
                .any(|game| game.mra_path.as_ref() == mgl.display().to_string())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_prunes_arcade_media_and_cores_but_keeps_arcade_game_mras() {
        let root = unique_temp_dir("target-arcade-game-dirs");
        let arcade_dir = root.join("_Arcade");
        let media_dir = arcade_dir.join("media");
        let cores_dir = arcade_dir.join("cores");
        let alternatives_dir = arcade_dir.join("_alternatives/_Alt");
        std::fs::create_dir_all(&media_dir).expect("create media dir");
        std::fs::create_dir_all(&cores_dir).expect("create cores dir");
        std::fs::create_dir_all(&alternatives_dir).expect("create alternatives dir");
        std::fs::write(
            arcade_dir.join("Real Game.mra"),
            "<misterromdescription><name>Real Game</name><setname>realgame</setname></misterromdescription>",
        )
        .expect("write real mra");
        std::fs::write(
            alternatives_dir.join("Alt Game.mra"),
            "<misterromdescription><name>Alt Game</name><setname>altgame</setname></misterromdescription>",
        )
        .expect("write alt mra");
        std::fs::write(
            arcade_dir.join("NeoGeo Pocket.mra"),
            "<misterromdescription><name>NeoGeo Pocket</name><setname>ngp</setname><rbf>jtngp</rbf></misterromdescription>",
        )
        .expect("write system launcher mra");
        std::fs::write(
            alternatives_dir.join("NeoGeo Pocket Color.mra"),
            "<misterromdescription><name>NeoGeo Pocket Color</name><setname>ngpc</setname><rbf>jtngp</rbf></misterromdescription>",
        )
        .expect("write alternative system launcher mra");
        std::fs::write(
            media_dir.join("Fake Screenshot.mra"),
            "<misterromdescription><name>Fake Screenshot</name></misterromdescription>",
        )
        .expect("write media fake");
        std::fs::write(cores_dir.join("Core.rbf"), "core").expect("write rbf");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 2);
        let titles = scan
            .discoveries
            .iter()
            .map(|discovery| discovery.title.as_str())
            .collect::<Vec<_>>();
        assert!(titles.contains(&"Real Game"));
        assert!(titles.contains(&"Alt Game"));
        assert!(!titles.contains(&"Fake Screenshot"));
        assert!(!titles.contains(&"NeoGeo Pocket"));
        assert!(!titles.contains(&"NeoGeo Pocket Color"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_does_not_follow_symlinked_game_dirs() {
        let root = unique_temp_dir("ignore-symlinked-game-dir");
        install_test_console_core(&root, "NES");
        let outside = unique_temp_dir("symlink-target-games");
        let games_dir = root.join("games");
        let linked_nes = games_dir.join("NES");
        std::fs::create_dir_all(&games_dir).expect("create games dir");
        std::fs::create_dir_all(&outside).expect("create symlink target");
        std::fs::write(outside.join("Mario.nes"), "rom").expect("write outside rom");
        std::os::unix::fs::symlink(&outside, &linked_nes).expect("create linked game dir");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert!(scan.normal_files.is_empty());
        assert!(scan.discoveries.is_empty());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn scanner_does_not_follow_symlinked_game_files() {
        let root = unique_temp_dir("ignore-symlinked-game-file");
        install_test_console_core(&root, "NES");
        let outside = unique_temp_dir("symlink-target-file");
        let nes_dir = root.join("games/NES");
        std::fs::create_dir_all(&nes_dir).expect("create nes dir");
        std::fs::create_dir_all(&outside).expect("create symlink target dir");
        let outside_rom = outside.join("Mario.nes");
        std::fs::write(&outside_rom, "rom").expect("write outside rom");
        std::os::unix::fs::symlink(&outside_rom, nes_dir.join("Mario.nes"))
            .expect("create linked game file");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert!(scan.normal_files.is_empty());
        assert!(scan.discoveries.is_empty());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn scanner_ignores_organized_alias_dirs() {
        let root = unique_temp_dir("ignore-organized-aliases");
        let arcade_dir = root.join("_Arcade");
        let organized_dir = arcade_dir.join("_Organized/_1 A-E");
        std::fs::create_dir_all(&organized_dir).expect("create organized dir");
        std::fs::write(
            arcade_dir.join("Diamond Run.mra"),
            "<misterromdescription><name>Diamond Run</name><setname>diamond</setname></misterromdescription>",
        )
        .expect("write source mra");
        std::fs::write(
            organized_dir.join("Diamond Run.mra"),
            "<misterromdescription><name>Diamond Run Alias</name><setname>diamond-alias</setname></misterromdescription>",
        )
        .expect("write organized alias mra");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 1);
        assert_eq!(scan.discoveries.len(), 1);
        assert_eq!(scan.discoveries[0].title, "Diamond Run");
        assert_eq!(
            scan.normal_files[0].path,
            arcade_dir.join("Diamond Run.mra").display().to_string()
        );
        assert!(
            scan.discoveries
                .iter()
                .all(|discovery| !discovery.launch_ref.contains("_Organized"))
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
