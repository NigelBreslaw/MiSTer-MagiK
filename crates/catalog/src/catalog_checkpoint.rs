// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cheap discovery checkpoint for warm catalog drift detection.
//!
//! The checkpoint records stable, low-cost catalog inputs. It is intentionally
//! not a full file manifest: warm validation should notice new systems/cores
//! without walking every game payload.

use crate::catalog_config::{CATALOG_BUILD_VERSION, SCHEMA_VERSION};
use crate::catalog_discovery;
use crate::catalog_scan::should_ignore_path;
use crate::core_audit::CatalogAuditRow;
use crate::launch_profiles::{
    self, CORE_LAUNCH_MANIFEST_VERSION, PROFILE_SET_VERSION, core_launch_manifest_fingerprint,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

const CHECKPOINT_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const CHECKPOINT_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogDiscoveryCheckpoint {
    lines: Vec<String>,
}

impl CatalogDiscoveryCheckpoint {
    pub fn from_lines(lines: Vec<String>) -> Self {
        Self { lines }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn fingerprint(&self) -> u64 {
        let mut hash = CHECKPOINT_HASH_OFFSET;
        for line in &self.lines {
            hash = fnv64_update(hash, line.as_bytes());
            hash = fnv64_update(hash, &[0xff]);
        }
        hash
    }

    pub fn fingerprint_hex(&self) -> String {
        format!("{:016x}", self.fingerprint())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CatalogDriftSummary {
    pub unchanged: bool,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub changed_roots: usize,
    pub changed_core_roots: usize,
    pub changed_cores: usize,
    pub changed_game_dirs: usize,
    pub changed_audit_rows: usize,
    pub changed_metadata: usize,
    pub changed_versions: usize,
    pub detail: String,
}

impl CatalogDriftSummary {
    pub fn unchanged() -> Self {
        Self {
            unchanged: true,
            detail: "checkpoint unchanged".to_string(),
            ..Self::default()
        }
    }

    pub fn from_checkpoints(
        stored: Option<&CatalogDiscoveryCheckpoint>,
        current: &CatalogDiscoveryCheckpoint,
    ) -> Self {
        let Some(stored) = stored else {
            return Self {
                unchanged: false,
                added_lines: current.lines.len(),
                detail: "checkpoint missing".to_string(),
                ..Self::default()
            };
        };
        if stored == current {
            return Self::unchanged();
        }

        let stored_set = stored.lines.iter().collect::<BTreeSet<_>>();
        let current_set = current.lines.iter().collect::<BTreeSet<_>>();
        let mut summary = Self {
            unchanged: false,
            added_lines: current_set.difference(&stored_set).count(),
            removed_lines: stored_set.difference(&current_set).count(),
            ..Self::default()
        };
        for line in current_set
            .difference(&stored_set)
            .chain(stored_set.difference(&current_set))
        {
            match line.split('\t').next().unwrap_or_default() {
                "schema" | "catalog-build" | "profile-set" | "core-launch-manifest" => {
                    summary.changed_versions += 1
                }
                "root" => summary.changed_roots += 1,
                "core-search-root" => summary.changed_core_roots += 1,
                "installed-core" => summary.changed_cores += 1,
                "game-dir" => summary.changed_game_dirs += 1,
                "core-audit-row" | "core-audit-summary" => summary.changed_audit_rows += 1,
                "mame-metadata"
                | "hbmame-metadata"
                | "runtime-metadata"
                | "runtime-metadata-shard" => summary.changed_metadata += 1,
                _ => {}
            }
        }
        summary.detail = format!(
            "checkpoint changed added={} removed={} versions={} roots={} core_roots={} cores={} game_dirs={} audit={} metadata={}",
            summary.added_lines,
            summary.removed_lines,
            summary.changed_versions,
            summary.changed_roots,
            summary.changed_core_roots,
            summary.changed_cores,
            summary.changed_game_dirs,
            summary.changed_audit_rows,
            summary.changed_metadata
        );
        summary
    }
}

#[cfg(test)]
pub(crate) fn compute_catalog_discovery_checkpoint(
    roots: &[String],
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
    audit_rows: &[CatalogAuditRow],
) -> CatalogDiscoveryCheckpoint {
    let installed_cores = catalog_discovery::installed_cores_for_roots(roots);
    let game_dirs = catalog_discovery::top_level_game_dirs_for_roots(roots);
    compute_catalog_discovery_checkpoint_from_facts(
        roots,
        mame_sqlite_path,
        hbmame_sqlite_path,
        audit_rows,
        &installed_cores,
        &game_dirs,
    )
}

pub(crate) fn compute_catalog_discovery_checkpoint_from_facts(
    roots: &[String],
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
    audit_rows: &[CatalogAuditRow],
    installed_cores: &[catalog_discovery::InstalledCore],
    game_dirs: &[catalog_discovery::GameDirFact],
) -> CatalogDiscoveryCheckpoint {
    let started = Instant::now();
    let mut lines = vec![
        format!("schema\t{SCHEMA_VERSION}"),
        format!("catalog-build\t{CATALOG_BUILD_VERSION}"),
        format!("profile-set\t{PROFILE_SET_VERSION}"),
        format!(
            "core-launch-manifest\t{}\t{:016x}",
            CORE_LAUNCH_MANIFEST_VERSION,
            core_launch_manifest_fingerprint()
        ),
        format!("roots\t{}", roots.len()),
    ];

    let root_t = Instant::now();
    for (idx, root) in roots.iter().enumerate() {
        append_path_signature(&mut lines, "root", idx, Path::new(root));
    }
    report_checkpoint_timing(
        "roots",
        root_t.elapsed().as_micros() as u64,
        format!("roots={}", roots.len()),
    );

    let core_t = Instant::now();
    append_core_summaries(&mut lines, roots, installed_cores);
    report_checkpoint_timing(
        "cores",
        core_t.elapsed().as_micros() as u64,
        format!("lines={}", lines.len()),
    );

    let game_dir_t = Instant::now();
    append_game_dir_summaries(&mut lines, roots, game_dirs);
    report_checkpoint_timing(
        "game_dirs",
        game_dir_t.elapsed().as_micros() as u64,
        format!("lines={}", lines.len()),
    );

    let audit_t = Instant::now();
    append_audit_summary(&mut lines, audit_rows);
    report_checkpoint_timing(
        "audit",
        audit_t.elapsed().as_micros() as u64,
        format!("rows={}", audit_rows.len()),
    );

    append_named_file_signature(&mut lines, "mame-metadata", mame_sqlite_path);
    append_named_file_signature(&mut lines, "hbmame-metadata", hbmame_sqlite_path);
    append_runtime_metadata_signature(&mut lines);
    report_checkpoint_timing(
        "compute_total",
        started.elapsed().as_micros() as u64,
        format!("lines={}", lines.len()),
    );
    CatalogDiscoveryCheckpoint { lines }
}

fn append_runtime_metadata_signature(lines: &mut Vec<String>) {
    let path = crate::catalog_config::default_runtime_metadata_path();
    let Ok(store) = crate::runtime_metadata::MetadataStore::open(&path) else {
        append_named_file_signature(lines, "runtime-metadata", &path);
        return;
    };
    lines.push(format!(
        "runtime-metadata\t{}\t{}\t{}",
        path.display(),
        store.status().file_len,
        store.status().shard_count
    ));
    for (id, digest) in store.shard_digests() {
        lines.push(format!(
            "runtime-metadata-shard\t{id}\t{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ));
    }
}

/// Revalidate the retained cold-scan checkpoint without walking every payload.
///
/// The cold scan retains the semantic payload shape and audit summary. Warm
/// validation enumerates only top-level system directories, refreshes their
/// compact metadata signatures, and reuses retained semantic fields only when
/// each path has an unambiguous, available signature. Any missing, added,
/// malformed, or unstatable directory produces a different checkpoint and a
/// conservative rebuild.
pub(crate) fn compute_catalog_discovery_checkpoint_probe(
    roots: &[String],
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
    installed_cores: &[catalog_discovery::InstalledCore],
    game_dir_headers: &[catalog_discovery::GameDirHeader],
    stored: &CatalogDiscoveryCheckpoint,
) -> CatalogDiscoveryCheckpoint {
    let mut ambiguous = false;
    let mut retained_shapes = HashMap::<String, bool>::new();
    let mut top_probes = HashSet::<String>::new();
    let mut child_probe_counts = HashMap::<String, usize>::new();
    let mut child_probe_paths = HashMap::<String, Vec<PathBuf>>::new();
    for line in stored.lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.first().copied().unwrap_or_default() {
            "game-dir" => {
                if fields.len() != 6 {
                    ambiguous = true;
                    continue;
                }
                let payloadish = match fields[5] {
                    "payloadish" => true,
                    "empty" => false,
                    _ => {
                        ambiguous = true;
                        continue;
                    }
                };
                let key = fields[2].to_ascii_lowercase();
                if retained_shapes.insert(key, payloadish).is_some() {
                    ambiguous = true;
                }
            }
            "game-dir-probe" => {
                if !valid_present_probe(&fields) {
                    ambiguous = true;
                    continue;
                }
                if !top_probes.insert(fields[1].to_ascii_lowercase()) {
                    ambiguous = true;
                }
            }
            "game-dir-child-probes" => {
                if fields.len() != 3 {
                    ambiguous = true;
                    continue;
                }
                let Ok(count) = fields[2].parse::<usize>() else {
                    ambiguous = true;
                    continue;
                };
                if child_probe_counts
                    .insert(fields[1].to_ascii_lowercase(), count)
                    .is_some()
                {
                    ambiguous = true;
                }
            }
            "game-dir-child-probe" => {
                if !valid_present_probe(&fields) {
                    ambiguous = true;
                    continue;
                }
                let path = PathBuf::from(fields[1]);
                let Some(parent) = path.parent() else {
                    ambiguous = true;
                    continue;
                };
                child_probe_paths
                    .entry(parent.to_string_lossy().to_ascii_lowercase())
                    .or_default()
                    .push(path);
            }
            _ => {}
        }
    }
    for (parent, count) in &child_probe_counts {
        if child_probe_paths.get(parent).map_or(0, Vec::len) != *count {
            ambiguous = true;
        }
    }

    let mut game_dirs = Vec::with_capacity(game_dir_headers.len());
    for header in game_dir_headers {
        let key = header.path.to_string_lossy().to_ascii_lowercase();
        let Some(payloadish) = retained_shapes.remove(&key) else {
            ambiguous = true;
            game_dirs.push(catalog_discovery::GameDirFact {
                name: header.name.clone(),
                path: header.path.clone(),
                signature: header.signature,
                has_payload_files: false,
                has_zip_files: false,
                direct_zip_paths: Vec::new(),
                nested_probe_signatures: Vec::new(),
                payload_extensions: BTreeSet::new(),
            });
            continue;
        };
        if !top_probes.remove(&key) || !child_probe_counts.contains_key(&key) {
            ambiguous = true;
        }
        let child_paths = child_probe_paths.remove(&key).unwrap_or_default();
        let probe = crate::namespace_walk::probe_directory_signatures(&header.path, &child_paths);
        let top_signature =
            catalog_discovery::GameDirSignature::from_namespace_signature(probe.target_signature);
        if matches!(
            top_signature,
            catalog_discovery::GameDirSignature::Unavailable
        ) {
            ambiguous = true;
        }
        let mut nested_probe_signatures = child_paths
            .into_iter()
            .zip(probe.child_signatures)
            .map(|(path, signature)| {
                let signature =
                    catalog_discovery::GameDirSignature::from_namespace_signature(signature);
                if matches!(signature, catalog_discovery::GameDirSignature::Unavailable) {
                    ambiguous = true;
                }
                (path, signature)
            })
            .collect::<Vec<_>>();
        nested_probe_signatures.sort_by_cached_key(|(path, _)| {
            (path.to_string_lossy().to_ascii_lowercase(), path.clone())
        });
        game_dirs.push(catalog_discovery::GameDirFact {
            name: header.name.clone(),
            path: header.path.clone(),
            signature: top_signature,
            has_payload_files: payloadish,
            has_zip_files: false,
            direct_zip_paths: Vec::new(),
            nested_probe_signatures,
            payload_extensions: BTreeSet::new(),
        });
    }
    if !retained_shapes.is_empty() || !top_probes.is_empty() || !child_probe_paths.is_empty() {
        ambiguous = true;
    }

    let mut current = compute_catalog_discovery_checkpoint_from_facts(
        roots,
        mame_sqlite_path,
        hbmame_sqlite_path,
        &[],
        installed_cores,
        &game_dirs,
    );
    let retained_audit = stored
        .lines()
        .iter()
        .filter(|line| {
            line.starts_with("core-audit-summary\t") || line.starts_with("core-audit-row\t")
        })
        .cloned()
        .collect::<Vec<_>>();
    if retained_audit.is_empty() {
        ambiguous = true;
    }
    current.lines.retain(|line| {
        !line.starts_with("core-audit-summary\t") && !line.starts_with("core-audit-row\t")
    });
    let audit_insert = current
        .lines
        .iter()
        .position(|line| {
            line.starts_with("mame-metadata\t") || line.starts_with("hbmame-metadata\t")
        })
        .unwrap_or(current.lines.len());
    current
        .lines
        .splice(audit_insert..audit_insert, retained_audit);
    if ambiguous {
        current
            .lines
            .push("checkpoint-probe\tambiguous".to_string());
    }
    current
}

fn valid_present_probe(fields: &[&str]) -> bool {
    fields.len() == 5
        && fields[2] == "present"
        && fields[3].parse::<u64>().is_ok()
        && fields[4].parse::<i64>().is_ok()
}

pub(crate) fn without_probe_lines(
    checkpoint: &CatalogDiscoveryCheckpoint,
) -> CatalogDiscoveryCheckpoint {
    CatalogDiscoveryCheckpoint::from_lines(
        checkpoint
            .lines()
            .iter()
            .filter(|line| {
                !line.starts_with("game-dir-probe\t")
                    && !line.starts_with("game-dir-child-probe")
                    && !line.starts_with("checkpoint-probe\t")
            })
            .cloned()
            .collect(),
    )
}

pub(crate) fn catalog_trace_detail_enabled() -> bool {
    std::env::var("MISTER_CATALOG_TRACE")
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("detail")
}

pub(crate) fn report_checkpoint_timing(stage: &str, us: u64, detail: impl std::fmt::Display) {
    crate::catalog_logln!("catalog_checkpoint_tsv\t{stage}\t{us}\t{detail}");
}

pub(crate) fn report_drift_summary(summary: &CatalogDriftSummary) {
    crate::catalog_logln!(
        "catalog_drift_tsv\tunchanged={}\tadded={}\tremoved={}\tversions={}\troots={}\tcore_roots={}\tcores={}\tgame_dirs={}\taudit={}\tmetadata={}\tdetail={}",
        summary.unchanged,
        summary.added_lines,
        summary.removed_lines,
        summary.changed_versions,
        summary.changed_roots,
        summary.changed_core_roots,
        summary.changed_cores,
        summary.changed_game_dirs,
        summary.changed_audit_rows,
        summary.changed_metadata,
        summary.detail
    );
}

fn append_core_summaries(
    lines: &mut Vec<String>,
    roots: &[String],
    installed_cores: &[catalog_discovery::InstalledCore],
) {
    let search_roots = catalog_discovery::core_search_roots(roots);
    lines.push(format!("core-search-roots\t{}", search_roots.len()));
    for (idx, search_root) in search_roots.iter().enumerate() {
        append_path_signature(lines, "core-search-root", idx, search_root);
    }
    let checkpoint_cores = installed_cores
        .iter()
        .filter(|core| !should_ignore_path(&core.path))
        .collect::<Vec<_>>();
    for (idx, core) in checkpoint_cores.iter().enumerate() {
        let status = if is_known_core(&core.core_id) {
            "known"
        } else {
            "unknown"
        };
        append_installed_core_signature(lines, idx, status, &core.core_id, &core.path);
    }
    lines.push(format!("installed-cores\t{}", checkpoint_cores.len()));
}

fn append_game_dir_summaries(
    lines: &mut Vec<String>,
    roots: &[String],
    game_dirs: &[catalog_discovery::GameDirFact],
) {
    let game_roots = catalog_discovery::game_roots(roots);
    lines.push(format!("game-roots\t{}", game_roots.len()));
    for (root_idx, game_root) in game_roots.iter().enumerate() {
        append_path_signature(lines, "game-root", root_idx, game_root);
    }
    let mut game_dirs = game_dirs.iter().collect::<Vec<_>>();
    game_dirs.sort_by_cached_key(|dir| {
        (
            dir.path.to_string_lossy().to_ascii_lowercase(),
            dir.path.to_string_lossy().into_owned(),
        )
    });
    let game_dir_count = game_dirs.len();
    for (idx, dir) in game_dirs.into_iter().enumerate() {
        let status = if launch_profiles::generic_manifest_profile_for_game_dir(&dir.name).is_some()
            || launch_profiles::builtin_profiles().iter().any(|profile| {
                profile
                    .game_dirs
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&dir.name))
            }) {
            "known"
        } else {
            "unknown"
        };
        let payload_shape = if dir.has_payloadish_files() {
            "payloadish"
        } else {
            "empty"
        };
        lines.push(format!(
            "game-dir\t{idx}\t{}\t{}\t{status}\t{payload_shape}",
            dir.path.display(),
            dir.name,
        ));
        append_game_dir_probe(lines, "game-dir-probe", &dir.path, dir.signature);
        lines.push(format!(
            "game-dir-child-probes\t{}\t{}",
            dir.path.display(),
            dir.nested_probe_signatures.len()
        ));
        for (child_path, signature) in &dir.nested_probe_signatures {
            append_game_dir_probe(lines, "game-dir-child-probe", child_path, *signature);
        }
    }
    lines.push(format!("game-dirs\t{game_dir_count}"));
}

fn append_game_dir_probe(
    lines: &mut Vec<String>,
    kind: &str,
    path: &Path,
    signature: catalog_discovery::GameDirSignature,
) {
    match signature {
        catalog_discovery::GameDirSignature::Present { len, mtime_nanos } => lines.push(format!(
            "{kind}\t{}\tpresent\t{len}\t{mtime_nanos}",
            path.display()
        )),
        catalog_discovery::GameDirSignature::Unavailable => {
            lines.push(format!("{kind}\t{}\tunavailable", path.display()))
        }
    }
}

fn append_audit_summary(lines: &mut Vec<String>, audit_rows: &[CatalogAuditRow]) {
    let mut counts = BTreeMap::<String, usize>::new();
    for row in audit_rows {
        *counts
            .entry(format!(
                "{}\t{}\t{}",
                row.catalog_status, row.source, row.reason
            ))
            .or_default() += 1;
    }
    lines.push(format!("core-audit-summary\t{}", counts.len()));
    for (idx, (key, count)) in counts.into_iter().enumerate() {
        lines.push(format!("core-audit-row\t{idx}\t{key}\t{count}"));
    }
}

fn append_path_signature(lines: &mut Vec<String>, kind: &str, idx: usize, path: &Path) {
    match std::fs::metadata(path) {
        Ok(meta) => lines.push(format!(
            "{kind}\t{idx}\t{}\t{}\t{}\t{}",
            path.display(),
            if meta.is_dir() { "dir" } else { "not-dir" },
            meta.len(),
            mtime_nanos(&meta)
        )),
        Err(_) => lines.push(format!("{kind}\t{idx}\t{}\tmissing", path.display())),
    }
}

fn append_installed_core_signature(
    lines: &mut Vec<String>,
    idx: usize,
    status: &str,
    core_id: &str,
    path: &Path,
) {
    match std::fs::metadata(path) {
        Ok(meta) => {
            lines.push(format!(
                "installed-core\t{idx}\t{status}\t{core_id}\t{}\t{}\t{}",
                path.display(),
                meta.len(),
                mtime_nanos(&meta)
            ));
            if catalog_trace_detail_enabled() {
                crate::catalog_logln!(
                    "catalog_profile_manifest_tsv\tcore_id={core_id}\tstatus={status}\tpath={}\tsize={}\tmtime_nanos={}",
                    path.display(),
                    meta.len(),
                    mtime_nanos(&meta)
                );
            }
        }
        Err(_) => lines.push(format!(
            "installed-core\t{idx}\t{status}\t{core_id}\t{}\tmissing",
            path.display()
        )),
    }
}

fn append_named_file_signature(lines: &mut Vec<String>, name: &str, path: &Path) {
    match std::fs::metadata(path) {
        Ok(meta) => lines.push(format!(
            "{name}\t{}\t{}\t{}",
            path.display(),
            meta.len(),
            mtime_nanos(&meta)
        )),
        Err(_) => lines.push(format!("{name}\t{}\tmissing", path.display())),
    }
}

fn is_known_core(core_id: &str) -> bool {
    launch_profiles::generic_manifest_profile_for_core(core_id).is_some()
        || launch_profiles::builtin_profiles()
            .iter()
            .any(|profile| profile.core_name.eq_ignore_ascii_case(core_id))
}

fn mtime_nanos(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn fnv64_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(CHECKPOINT_HASH_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn checkpoint_is_stable_for_unchanged_roots() {
        let root = unique_temp_dir("checkpoint-stable");
        std::fs::create_dir_all(root.join("games/NES")).expect("create nes dir");
        std::fs::write(root.join("games/NES/Game.nes"), b"rom").expect("write rom");
        set_mtime_for_test(&root.join("games"), 10, 0);
        let roots = vec![root.display().to_string()];

        let first = compute_catalog_discovery_checkpoint(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
        );
        let second = compute_catalog_discovery_checkpoint(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
        );

        assert_eq!(first, second);
        assert_eq!(first.fingerprint_hex(), second.fingerprint_hex());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn retained_game_dir_order_matches_live_canonical_order() {
        let root = unique_temp_dir("checkpoint-retained-order");
        for name in ["Saturn", "NES", "PSX"] {
            let dir = root.join("games").join(name);
            std::fs::create_dir_all(&dir).expect("create game dir");
            std::fs::write(dir.join("Game.rom"), b"rom").expect("write game");
        }
        let roots = vec![root.display().to_string()];
        let live_facts = catalog_discovery::top_level_game_dirs_for_roots(&roots);
        let mut retained_facts = live_facts.clone();
        retained_facts.reverse();

        let live = compute_catalog_discovery_checkpoint_from_facts(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
            &[],
            &live_facts,
        );
        let retained = compute_catalog_discovery_checkpoint_from_facts(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
            &[],
            &retained_facts,
        );

        assert_eq!(retained, live);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn known_core_addition_changes_checkpoint() {
        let root = unique_temp_dir("checkpoint-known-core");
        let roots = vec![root.display().to_string()];
        let first = compute_catalog_discovery_checkpoint(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
        );

        let console = root.join("_Console");
        std::fs::create_dir_all(&console).expect("create console");
        std::fs::write(console.join("ColecoVision_20260630.rbf"), b"core").expect("write core");
        let second = compute_catalog_discovery_checkpoint(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
        );

        let drift = CatalogDriftSummary::from_checkpoints(Some(&first), &second);
        assert!(!drift.unchanged);
        assert!(drift.changed_cores > 0 || drift.changed_core_roots > 0);
        assert!(
            second
                .lines()
                .iter()
                .any(|line| line.contains("installed-core") && line.contains("known"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_core_and_game_dir_are_checkpoint_inputs() {
        let root = unique_temp_dir("checkpoint-unknown-core");
        let roots = vec![root.display().to_string()];
        let first = compute_catalog_discovery_checkpoint(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
        );

        let console = root.join("_Console");
        let game_dir = root.join("games/ChannelF");
        std::fs::create_dir_all(&console).expect("create console");
        std::fs::create_dir_all(&game_dir).expect("create game dir");
        std::fs::write(console.join("ChannelF_20260630.rbf"), b"core").expect("write core");
        std::fs::write(game_dir.join("Alien.chf"), b"rom").expect("write payload");
        let second = compute_catalog_discovery_checkpoint(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
        );

        let drift = CatalogDriftSummary::from_checkpoints(Some(&first), &second);
        assert!(!drift.unchanged);
        assert!(drift.changed_cores > 0 || drift.changed_game_dirs > 0);
        assert!(
            second
                .lines()
                .iter()
                .any(|line| line.contains("installed-core") && line.contains("unknown"))
        );
        assert!(
            second
                .lines()
                .iter()
                .any(|line| line.contains("game-dir") && line.contains("payloadish"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn existing_payloadish_game_dir_metadata_churn_is_not_checkpoint_drift() {
        let root = unique_temp_dir("checkpoint-game-dir-metadata-churn");
        let game_dir = root.join("games/NES");
        std::fs::create_dir_all(&game_dir).expect("create game dir");
        std::fs::write(game_dir.join("Mario.nes"), b"rom").expect("write rom");
        let roots = vec![root.display().to_string()];

        let first = compute_catalog_discovery_checkpoint(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
        );
        std::fs::write(game_dir.join("Zelda.nes"), b"rom").expect("write second rom");
        set_mtime_for_test(&game_dir, 20, 0);
        let second = compute_catalog_discovery_checkpoint(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
        );

        assert_eq!(without_probe_lines(&first), without_probe_lines(&second));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ignored_media_dirs_do_not_become_game_dir_lines() {
        let root = unique_temp_dir("checkpoint-ignore-media");
        for dir in [
            "screenshots",
            "ScreenShot",
            "screenshot-magik",
            "BoxArt",
            "_organized",
        ] {
            let path = root.join("games").join(dir);
            std::fs::create_dir_all(&path).expect("create ignored dir");
            std::fs::write(path.join("Fake.nes"), b"media").expect("write fake media payload");
        }
        let roots = vec![root.display().to_string()];

        let checkpoint = compute_catalog_discovery_checkpoint(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
        );

        for dir in [
            "screenshots",
            "ScreenShot",
            "screenshot-magik",
            "BoxArt",
            "_organized",
        ] {
            assert!(
                !checkpoint
                    .lines()
                    .iter()
                    .any(|line| line.contains("game-dir") && line.contains(dir)),
                "{dir} should not be a checkpoint game-dir line"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn appledouble_core_sidecars_are_ignored() {
        let root = unique_temp_dir("checkpoint-appledouble-core");
        let console = root.join("_Console");
        std::fs::create_dir_all(&console).expect("create console");
        std::fs::write(console.join("._ColecoVision_20260630.rbf"), b"sidecar")
            .expect("write sidecar");
        let roots = vec![root.display().to_string()];

        let checkpoint = compute_catalog_discovery_checkpoint(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            &[],
        );

        assert!(
            !checkpoint
                .lines()
                .iter()
                .any(|line| line.contains("._ColecoVision"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[cfg(unix)]
    fn set_mtime_for_test(path: &Path, sec: i64, nsec: i64) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        #[repr(C)]
        struct Timespec {
            tv_sec: i64,
            tv_nsec: i64,
        }
        let c_path = CString::new(path.as_os_str().as_bytes()).expect("path CString");
        let times = [
            Timespec {
                tv_sec: sec,
                tv_nsec: nsec,
            },
            Timespec {
                tv_sec: sec,
                tv_nsec: nsec,
            },
        ];
        unsafe extern "C" {
            fn utimensat(
                dirfd: i32,
                pathname: *const i8,
                times: *const Timespec,
                flags: i32,
            ) -> i32;
        }
        let rc = unsafe { utimensat(-100, c_path.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0, "utimensat failed");
    }
}
