// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library walking and archive candidate discovery.

use crate::catalog_discovery::{GameDirFact, GameDirHeader};
use crate::launch_profiles::{
    self, CatalogScanPlan, LaunchProfile, PayloadDisposition, ProfilePathClass,
};
use crate::library_db;
#[cfg(any(test, feature = "builder"))]
use crate::library_db::LibraryContainerEntry;
use crate::namespace_walk::{
    self, NamespaceEntryKind, NamespaceRootPolicy, NamespaceSignatureCapture, NamespaceWalkStats,
};
use std::collections::{BTreeMap, HashSet};
#[cfg(any(test, feature = "builder"))]
use std::fs::File;
#[cfg(any(test, feature = "builder"))]
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// A runtime directory normally contributes a small set of candidate records.
/// Keep that transient buffer bounded; the overflow path deliberately re-walks
/// just that directory after its facts have selected a profile, rather than
/// dropping a possible game.
const MAX_RUNTIME_DIRECTORY_BUFFERED_FILES: usize = 65_536;
#[cfg(any(test, feature = "builder"))]
const ZIP_CENTRAL_DIRECTORY_BUFFER_BYTES: usize = 64 * 1024;
#[cfg(any(test, feature = "builder"))]
const ZIP_CENTRAL_DIRECTORY_MAX_BUFFER_BYTES: u64 = 8 * 1024 * 1024;
#[cfg(any(test, feature = "builder"))]
const ZIP_SKIP_BUFFER_BYTES: usize = 4 * 1024;

fn bounded_map_detail(values: &BTreeMap<String, usize>) -> String {
    if values.is_empty() {
        return "none".to_string();
    }
    values
        .iter()
        .take(8)
        .map(|(key, value)| format!("{}:{value}", key.replace([' ', '\t', '\n', ','], "_")))
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) struct FoundFile {
    pub(crate) path: PathBuf,
    pub(crate) ext: String,
}

pub(crate) struct RuntimeDirectoryCandidates {
    pub(crate) facts: GameDirFact,
    pub(crate) files: Vec<FoundFile>,
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

struct WalkTargetStats {
    dirs: usize,
    files: usize,
    candidates: usize,
    elapsed_us: u64,
    namespace: NamespaceWalkStats,
}

enum PlannedScanTarget {
    Static {
        path: PathBuf,
        game_dir_header: Option<GameDirHeader>,
    },
    Runtime(GameDirHeader),
    FactsOnly(GameDirHeader),
}

/// Inventory the targets selected by the production scan planner without
/// classifying games or publishing catalog state.
pub fn catalog_corpus_inventory_tsv(roots: &[String]) -> String {
    let started = Instant::now();
    let plan = CatalogScanPlan::for_roots(roots);
    let profiles = plan.base_profiles();
    let candidate_exts = source_index_extensions(profiles);
    let targets = scan_targets_for_plan(roots, &plan, profiles, &[]);
    let target_count = targets.len();
    let mut rows = Vec::with_capacity(target_count);

    for (ordinal, target) in targets.into_iter().enumerate() {
        let descriptor = target.descriptor(ordinal);
        let mut extensions = BTreeMap::<String, usize>::new();
        let (stats, has_archives, profile) = match target {
            PlannedScanTarget::Static {
                path,
                game_dir_header,
            } => {
                let (stats, facts) = scan_target_candidates_with_facts(
                    &path,
                    profiles,
                    &candidate_exts,
                    game_dir_header.as_ref(),
                    &[],
                    |file| {
                        *extensions.entry(file.ext).or_default() += 1;
                        true
                    },
                );
                let has_archives = facts.as_ref().is_some_and(|facts| facts.has_zip_files)
                    || extensions.keys().any(|ext| is_archive_extension(ext));
                (
                    stats,
                    has_archives,
                    profile_for_path(profiles, &descriptor.path).cloned(),
                )
            }
            PlannedScanTarget::Runtime(header) => {
                let (stats, candidates) = scan_runtime_target_candidates(&header, &plan, &[]);
                for file in &candidates.files {
                    *extensions.entry(file.ext.clone()).or_default() += 1;
                }
                let has_archives = candidates.facts.has_zip_files
                    || extensions.keys().any(|ext| is_archive_extension(ext));
                let profile = plan.profile_for_game_dir_facts(&candidates.facts);
                (stats, has_archives, profile)
            }
            PlannedScanTarget::FactsOnly(header) => {
                let (stats, facts) = scan_game_dir_facts_only(&header);
                let profile = plan.profile_for_game_dir_facts(&facts);
                for ext in facts.payload_extensions {
                    extensions.entry(ext).or_default();
                }
                let has_archives = facts.has_zip_files;
                (stats, has_archives, profile)
            }
        };
        let mechanisms = corpus_mechanisms(
            descriptor.kind,
            &descriptor.path,
            profile.as_ref(),
            &extensions,
            has_archives,
            stats.dirs,
        );
        rows.push(format!(
            "catalog_corpus_target_tsv\tordinal={ordinal}\tkind={}\tsystem={}\tprofile={}\tpath={}\tdirs={}\tfiles={}\tcandidates={}\telapsed_us={}\tmechanisms={}\textensions={}\tnamespace_backend={}\tnamespace_dir_opens={}\tnamespace_reads={}\tnamespace_bytes={}\tnamespace_fallback={}",
            scan_target_kind_label(descriptor.kind),
            profile
                .as_ref()
                .map_or("unknown", |profile| profile.system_id.as_str()),
            profile
                .as_ref()
                .map_or("unknown", |profile| profile.id.as_str()),
            tsv_value(&descriptor.path.display().to_string()),
            stats.dirs,
            stats.files,
            stats.candidates,
            stats.elapsed_us,
            mechanisms.join(","),
            bounded_map_detail(&extensions),
            stats.namespace.backend,
            stats.namespace.dir_opens,
            stats.namespace.read_calls,
            stats.namespace.read_bytes,
            tsv_value(stats.namespace.fallback_reason.as_deref().unwrap_or("none")),
        ));
    }

    let mut out = format!(
        "catalog_corpus_inventory_tsv\tschema=mister-magik-catalog-corpus-inventory-v1\troots={}\tprofiles={}\ttargets={}\telapsed_us={}\n",
        roots.len(),
        profiles.len(),
        target_count,
        started.elapsed().as_micros(),
    );
    for row in rows {
        out.push_str(&row);
        out.push('\n');
    }
    out
}

fn scan_target_kind_label(kind: ScanTargetKind) -> &'static str {
    match kind {
        ScanTargetKind::Static => "static",
        ScanTargetKind::Runtime => "runtime",
        ScanTargetKind::FactsOnly => "facts-only",
    }
}

fn corpus_mechanisms(
    kind: ScanTargetKind,
    path: &Path,
    profile: Option<&LaunchProfile>,
    extensions: &BTreeMap<String, usize>,
    has_archives: bool,
    dirs: usize,
) -> Vec<&'static str> {
    let mut mechanisms = vec![scan_target_kind_label(kind)];
    if extensions.contains_key("mra") {
        mechanisms.push("mra");
    }
    if extensions.contains_key("mgl") {
        mechanisms.push("mgl");
    }
    if has_archives {
        mechanisms.push("archive");
    }
    if profile.is_some_and(|profile| !profile.collection_rules.is_empty())
        || path
            .components()
            .any(|part| part.as_os_str().to_string_lossy().contains("X68000 Games"))
    {
        mechanisms.push("prepared-collection");
    }
    mechanisms.push(if dirs > 1 { "nested" } else { "flat" });
    mechanisms
}

fn is_archive_extension(extension: &str) -> bool {
    matches!(extension, "zip" | "7z" | "lha" | "lzh" | "rar")
}

fn tsv_value(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], "_")
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
        if excluded_targets
            .iter()
            .any(|excluded| same_library_path(excluded, &header.path))
        {
            continue;
        }
        let key = header.path.display().to_string().to_ascii_lowercase();
        if seen.insert(key) {
            targets.push(PlannedScanTarget::Runtime(header.clone()));
        }
    }
    for header in plan.all_game_dir_headers() {
        if excluded_targets
            .iter()
            .any(|excluded| same_library_path(excluded, &header.path))
        {
            continue;
        }
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

fn scan_target_candidates_with_facts(
    target: &Path,
    profiles: &[LaunchProfile],
    candidate_exts: &HashSet<String>,
    game_dir_header: Option<&GameDirHeader>,
    pruned_paths: &[PathBuf],
    mut emit: impl FnMut(FoundFile) -> bool,
) -> (WalkTargetStats, Option<GameDirFact>) {
    let target_t = Instant::now();
    let mut dirs = 1usize;
    let mut files = 0usize;
    let mut candidates = 0usize;
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
    let root_policy =
        if crate::prepared_collections::is_followable_neon68k_launcher_root_symlink(target) {
            NamespaceRootPolicy::FollowSymlink
        } else {
            NamespaceRootPolicy::NoFollow
        };
    let namespace_stats = namespace_walk::visit_with_root_policy_and_signature_capture(
        target,
        None,
        root_policy,
        signature_capture,
        |path| {
            should_ignore_path(path)
                || pruned_paths.iter().any(|root| path.starts_with(root))
                || crate::prepared_collections::neon68k_duplicate_alias_path(target, path)
        },
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
            let file = FoundFile {
                path: p.to_path_buf(),
                ext,
            };
            candidates += 1;
            if !emit(file) {
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
    pruned_paths: &[PathBuf],
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
        |path| should_ignore_path(path) || pruned_paths.iter().any(|root| path.starts_with(root)),
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
            let file = FoundFile {
                path: path.to_path_buf(),
                ext,
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
                namespace: namespace_stats,
            },
            RuntimeDirectoryCandidates {
                facts,
                files: Vec::new(),
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
        let deep_namespace_stats = namespace_walk::visit(
            &root,
            None,
            |path| {
                should_ignore_path(path)
                    || pruned_paths
                        .iter()
                        .any(|excluded| path.starts_with(excluded))
            },
            |entry| {
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
                let file = FoundFile {
                    path: path.to_path_buf(),
                    ext,
                };
                push_runtime_candidate(
                    &mut files,
                    &mut overflowed,
                    &candidate_exts,
                    &profile,
                    file,
                );
                true
            },
        );
        namespace_stats.add(&deep_namespace_stats);
    }
    let stats = WalkTargetStats {
        dirs,
        files: files_seen,
        candidates: files.len(),
        elapsed_us: target_t.elapsed().as_micros() as u64,
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
    (stats, RuntimeDirectoryCandidates { facts, files })
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
        if !is_real_dir(path)
            && !crate::prepared_collections::is_followable_neon68k_launcher_root_symlink(path)
        {
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
    for root in
        crate::prepared_collections::neon68k_launcher_roots_for_library_root(configured_root)
    {
        push_scan_target(targets, root);
    }
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
    if is_real_dir(&path)
        || crate::prepared_collections::is_followable_neon68k_launcher_root_symlink(&path)
    {
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

#[cfg(any(test, feature = "builder"))]
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

#[cfg(any(test, feature = "builder"))]
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
        crate::catalog_progress::report_inner_progress_at(scanned.saturating_add(1));
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

#[cfg(any(test, feature = "builder"))]
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

#[cfg(any(test, feature = "builder"))]
fn discard_zip_bytes(reader: &mut impl Read, mut len: u64) -> Result<(), std::io::Error> {
    let mut scratch = [0u8; ZIP_SKIP_BUFFER_BYTES];
    while len > 0 {
        let read_len = len.min(scratch.len() as u64) as usize;
        reader.read_exact(&mut scratch[..read_len])?;
        len -= read_len as u64;
    }
    Ok(())
}

#[cfg(any(test, feature = "builder"))]
struct ZipCentralDirectoryLocation {
    entries: usize,
    size: u64,
    offset: u64,
}

#[cfg(any(test, feature = "builder"))]
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
    use crate::launch_profiles::{self, ProfilePathClass};
    use crate::test_support::*;
    use std::path::Path;

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
    fn corpus_inventory_reports_real_routes_and_candidate_mechanisms() {
        let root = unique_temp_dir("catalog-corpus-inventory");
        let arcade = root.join("_Arcade");
        let snes = root.join("games/SNES/Nested");
        let c64 = root.join("games/C64");
        std::fs::create_dir_all(&arcade).expect("create arcade dir");
        std::fs::create_dir_all(&snes).expect("create snes dir");
        std::fs::create_dir_all(root.join("_Computer")).expect("create computer dir");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(&c64).expect("create c64 dir");
        std::fs::write(root.join("_Computer/C64.rbf"), "core").expect("write c64 core");
        std::fs::write(root.join("_Console/SNES.rbf"), "core").expect("write snes core");
        std::fs::write(
            arcade.join("Inventory Game.mra"),
            "<misterromdescription />",
        )
        .expect("write mra");
        std::fs::write(snes.join("Inventory Game.sfc"), "rom").expect("write rom");
        std::fs::write(c64.join("Inventory Game.d64"), "disk").expect("write c64 disk");

        let report = catalog_corpus_inventory_tsv(&[root.display().to_string()]);

        assert!(report.contains(
            "catalog_corpus_inventory_tsv\tschema=mister-magik-catalog-corpus-inventory-v1"
        ));
        assert!(report.contains("system=arcade"));
        assert!(report.contains("mechanisms=static,mra,flat"));
        assert!(report.contains("system=snes"));
        assert!(report.contains("extensions=sfc:1"));
        assert!(report.contains("mechanisms=static,nested"));
        assert!(report.contains("system=c64"));
        assert!(report.contains("extensions=d64:1"));
        let _ = std::fs::remove_dir_all(root);
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
    #[cfg(feature = "builder")]
    fn subtree_pruning_keeps_personal_c64_content_visible() {
        let root = unique_temp_dir("oneload-pruning");
        let c64 = root.join("games/C64");
        let oneload = c64.join("OneLoad64-Games-Collection-v5");
        let personal = c64.join("Personal");
        std::fs::create_dir_all(&oneload).unwrap();
        std::fs::create_dir_all(&personal).unwrap();
        std::fs::write(oneload.join("Bundled.crt"), b"bundle").unwrap();
        std::fs::write(personal.join("Homebrew.crt"), b"personal").unwrap();
        let profiles = vec![crate::generic_system_catalog::generic_c64_baseline_profile()];
        let candidate_exts = source_index_extensions(&profiles);
        let mut paths = Vec::new();

        let _ = scan_target_candidates_with_facts(
            &c64,
            &profiles,
            &candidate_exts,
            None,
            std::slice::from_ref(&oneload),
            |file| {
                paths.push(file.path);
                true
            },
        );

        assert_eq!(paths, vec![personal.join("Homebrew.crt")]);
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
        let file = FoundFile {
            path: zip_path.clone(),
            ext: "zip".to_string(),
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
        let file = FoundFile {
            path: zip_path.clone(),
            ext: "zip".to_string(),
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
        let file = FoundFile {
            path: zip_path.clone(),
            ext: "zip".to_string(),
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

        let (stats, _) = scan_target_candidates_with_facts(
            &arcade_dir,
            &profiles,
            &candidate_exts,
            None,
            &[],
            |file| {
                found.push(file.path);
                true
            },
        );

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
        let file = FoundFile {
            path: zip_path.clone(),
            ext: "zip".to_string(),
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
    fn explicit_arcade_root_does_not_pull_in_prepared_computer_collection() {
        let root = unique_temp_dir("explicit-arcade-target");
        let arcade = root.join("_Arcade");
        let prepared = root.join("_Computer/_X68000 Games");
        std::fs::create_dir_all(&arcade).expect("create Arcade");
        std::fs::create_dir_all(&prepared).expect("create prepared collection");

        let profiles = launch_profiles::builtin_profiles();
        let targets = scan_targets_for_roots(&[arcade.display().to_string()], &profiles);

        assert_eq!(targets, vec![arcade]);
        assert!(!targets.contains(&prepared));
        let _ = std::fs::remove_dir_all(root);
    }
}
