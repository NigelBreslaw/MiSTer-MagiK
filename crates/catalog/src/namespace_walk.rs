// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Collection-independent namespace traversal backends.
//!
//! The established WalkDir backend streams entries directly to the caller.
//! Linux can optionally avoid repeatedly resolving full directory paths by
//! walking with `openat` and `getdents64`. The optional backend captures a
//! bounded target before publishing it, so any syscall or budget failure can
//! discard the partial capture and restart through the streaming backend.

use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NamespaceEntryKind {
    Directory,
    File,
    Other,
}

#[derive(Debug)]
pub(crate) struct NamespaceEntry {
    pub(crate) path: PathBuf,
    pub(crate) kind: NamespaceEntryKind,
    pub(crate) zip_signature: Option<(u64, i64)>,
    /// Directory metadata captured from an fd that the namespace backend had
    /// to open anyway. This is opt-in so deep production walks do not acquire
    /// one extra exFAT metadata operation per directory.
    pub(crate) directory_signature: Option<(u64, i64)>,
}

/// A complete, serial WalkDir capture of one subtree. Partial captures are
/// retained only for diagnostics; callers must check `complete` before using
/// the entries as a recovery result.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct NamespaceSubtreeSnapshot {
    pub(crate) entries: Vec<NamespaceEntry>,
    pub(crate) stats: NamespaceWalkStats,
    pub(crate) complete: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NamespaceSignatureCapture {
    None,
    Target,
    TargetAndDepthOneDirectories,
    #[cfg(feature = "builder")]
    AllDirectories,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum NamespaceRootPolicy {
    #[default]
    NoFollow,
    FollowSymlink,
}

impl NamespaceSignatureCapture {
    fn target(self) -> bool {
        match self {
            Self::None => false,
            Self::Target | Self::TargetAndDepthOneDirectories => true,
            #[cfg(feature = "builder")]
            Self::AllDirectories => true,
        }
    }

    #[cfg(target_os = "linux")]
    fn directory_at_depth(self, depth: usize) -> bool {
        match self {
            Self::TargetAndDepthOneDirectories => depth == 1,
            #[cfg(feature = "builder")]
            Self::AllDirectories => true,
            Self::None | Self::Target => false,
        }
    }
}

#[derive(Debug)]
pub(crate) struct NamespaceWalkStats {
    pub(crate) backend: &'static str,
    pub(crate) fallback_reason: Option<String>,
    /// Time spent opening the requested root directory, excluding signature
    /// probes and the first getdents call. Zero means the streaming backend
    /// did not have an fd-relative root open to attribute.
    pub(crate) root_open_us: u64,
    pub(crate) dir_opens: usize,
    pub(crate) read_calls: usize,
    pub(crate) read_bytes: u64,
    /// Number of entries whose getdents type was DT_UNKNOWN.
    pub(crate) type_stats: usize,
    pub(crate) stat_calls: usize,
    pub(crate) stat_us: u64,
    pub(crate) signature_stat_calls: usize,
    pub(crate) signature_stat_us: u64,
    /// The namespace walker does not canonicalize paths. Keep this explicit
    /// so attribution cannot silently confuse canonicalization with opening.
    pub(crate) canonicalization_count: usize,
    pub(crate) canonicalization_us: u64,
    pub(crate) captured_entries: usize,
    pub(crate) peak_buffered_entries: usize,
    pub(crate) peak_buffered_bytes: usize,
    pub(crate) buffer_allocations: usize,
    pub(crate) fallback_count: usize,
    pub(crate) restart_count: usize,
    pub(crate) errors: usize,
    pub(crate) first_entry_us: Option<u64>,
    pub(crate) final_entry_us: Option<u64>,
    pub(crate) target_signature: Option<(u64, i64)>,
}

#[derive(Debug)]
pub(crate) struct DirectorySignatureProbe {
    pub(crate) target_signature: Option<(u64, i64)>,
    pub(crate) child_signatures: Vec<Option<(u64, i64)>>,
}

#[cfg(feature = "builder")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct KnownPathMetadata {
    pub(crate) is_dir: bool,
    pub(crate) is_file: bool,
    pub(crate) size: u64,
    pub(crate) modified_ns: i128,
    pub(crate) changed_ns: i128,
    pub(crate) inode: u64,
}

/// Probe immediate children of one parent directory without repeating the
/// parent pathname lookup. The returned entries follow `std::fs::metadata`
/// semantics: symlinks are followed, and missing or invalid entries are
/// represented by `None`.
#[cfg(feature = "builder")]
pub(crate) fn probe_known_path_metadata(
    parent: &Path,
    child_paths: &[PathBuf],
) -> Vec<Option<KnownPathMetadata>> {
    #[cfg(target_os = "linux")]
    {
        linux::probe_known_path_metadata(parent, child_paths)
    }

    #[cfg(not(target_os = "linux"))]
    let _ = parent;

    #[cfg(not(target_os = "linux"))]
    child_paths
        .iter()
        .map(|path| std::fs::metadata(path).ok().map(known_path_metadata))
        .collect()
}

#[cfg(not(target_os = "linux"))]
#[cfg(feature = "builder")]
fn known_path_metadata(metadata: std::fs::Metadata) -> KnownPathMetadata {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    use std::time::UNIX_EPOCH;

    KnownPathMetadata {
        is_dir: metadata.is_dir(),
        is_file: metadata.is_file(),
        size: metadata.len(),
        modified_ns: metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |value| {
                i128::from(value.as_secs()) * 1_000_000_000 + i128::from(value.subsec_nanos())
            }),
        #[cfg(unix)]
        changed_ns: i128::from(metadata.ctime()) * 1_000_000_000
            + i128::from(metadata.ctime_nsec()),
        #[cfg(not(unix))]
        changed_ns: 0,
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(not(unix))]
        inode: 0,
    }
}

impl NamespaceWalkStats {
    pub(crate) fn add(&mut self, other: &Self) {
        if self.backend != other.backend {
            self.backend = "mixed";
        }
        if self.fallback_reason.is_none() {
            self.fallback_reason.clone_from(&other.fallback_reason);
        }
        self.root_open_us = self.root_open_us.saturating_add(other.root_open_us);
        self.dir_opens = self.dir_opens.saturating_add(other.dir_opens);
        self.read_calls = self.read_calls.saturating_add(other.read_calls);
        self.read_bytes = self.read_bytes.saturating_add(other.read_bytes);
        self.type_stats = self.type_stats.saturating_add(other.type_stats);
        self.stat_calls = self.stat_calls.saturating_add(other.stat_calls);
        self.stat_us = self.stat_us.saturating_add(other.stat_us);
        self.signature_stat_calls = self
            .signature_stat_calls
            .saturating_add(other.signature_stat_calls);
        self.signature_stat_us = self
            .signature_stat_us
            .saturating_add(other.signature_stat_us);
        self.canonicalization_count = self
            .canonicalization_count
            .saturating_add(other.canonicalization_count);
        self.canonicalization_us = self
            .canonicalization_us
            .saturating_add(other.canonicalization_us);
        self.captured_entries = self.captured_entries.saturating_add(other.captured_entries);
        self.peak_buffered_entries = self.peak_buffered_entries.max(other.peak_buffered_entries);
        self.peak_buffered_bytes = self.peak_buffered_bytes.max(other.peak_buffered_bytes);
        self.buffer_allocations = self
            .buffer_allocations
            .saturating_add(other.buffer_allocations);
        self.fallback_count = self.fallback_count.saturating_add(other.fallback_count);
        self.restart_count = self.restart_count.saturating_add(other.restart_count);
        self.errors = self.errors.saturating_add(other.errors);
        self.first_entry_us = match (self.first_entry_us, other.first_entry_us) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        self.final_entry_us = match (self.final_entry_us, other.final_entry_us) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        };
        // Aggregated stats no longer describe one target. Callers that need a
        // target signature consume it before combining subordinate walks.
        self.target_signature = None;
    }
}

/// Visit a target without ever requiring the caller to retain its full
/// namespace. `false` from the visitor stops immediately.
///
/// `auto` selects bounded fd-relative traversal on Linux and the established
/// streaming backend elsewhere. Any fd-relative failure or capture-budget
/// limit restarts the untouched target through streaming WalkDir.
pub(crate) fn visit(
    target: &Path,
    max_depth: Option<usize>,
    ignore: impl Fn(&Path) -> bool,
    visitor: impl FnMut(&NamespaceEntry) -> bool,
) -> NamespaceWalkStats {
    visit_with_signature_capture(
        target,
        max_depth,
        NamespaceSignatureCapture::None,
        ignore,
        visitor,
    )
}

pub(crate) fn visit_with_signature_capture(
    target: &Path,
    max_depth: Option<usize>,
    signature_capture: NamespaceSignatureCapture,
    ignore: impl Fn(&Path) -> bool,
    visitor: impl FnMut(&NamespaceEntry) -> bool,
) -> NamespaceWalkStats {
    visit_with_root_policy_and_signature_capture(
        target,
        max_depth,
        NamespaceRootPolicy::NoFollow,
        signature_capture,
        ignore,
        visitor,
    )
}

/// Visit a target while transferring fd-relative entries to the caller.
///
/// The Linux backend already owns each captured `PathBuf`; consuming the
/// capture avoids cloning that path once more merely to hand it to an
/// inventory builder. The WalkDir fallback remains exact and clones only on
/// that slow path.
#[allow(dead_code)]
pub(crate) fn visit_owned_with_signature_capture(
    target: &Path,
    max_depth: Option<usize>,
    signature_capture: NamespaceSignatureCapture,
    ignore: impl Fn(&Path) -> bool,
    mut visitor: impl FnMut(NamespaceEntry) -> bool,
) -> NamespaceWalkStats {
    let requested =
        std::env::var("MISTER_LIBRARY_NAMESPACE_BACKEND").unwrap_or_else(|_| "auto".to_string());
    if requested == "walkdir" {
        return visit_walkdir_owned(
            target,
            max_depth,
            NamespaceRootPolicy::NoFollow,
            signature_capture,
            &ignore,
            &mut visitor,
            None,
        );
    }

    #[cfg(target_os = "linux")]
    if requested == "fd-relative" || requested == "auto" {
        let visit_started = Instant::now();
        match linux::collect_fd_relative(
            target,
            max_depth,
            NamespaceRootPolicy::NoFollow,
            signature_capture,
            &ignore,
        ) {
            Ok(mut capture) => {
                for entry in capture.entries.drain(..) {
                    let elapsed_us = visit_started.elapsed().as_micros() as u64;
                    capture.stats.first_entry_us.get_or_insert(elapsed_us);
                    capture.stats.final_entry_us = Some(elapsed_us);
                    if !visitor(entry) {
                        break;
                    }
                }
                return capture.stats;
            }
            Err(reason) => {
                let fd_attempt_us = visit_started.elapsed().as_micros() as u64;
                let fallback_started = Instant::now();
                let stats = visit_walkdir_owned(
                    target,
                    max_depth,
                    NamespaceRootPolicy::NoFollow,
                    signature_capture,
                    &ignore,
                    &mut visitor,
                    Some(reason.to_string()),
                );
                report_namespace_fallback(target, &reason, fd_attempt_us, fallback_started, &stats);
                return stats;
            }
        }
    }

    let reason = if requested == "auto" {
        None
    } else if requested == "fd-relative" {
        Some("fd-relative backend unsupported on this operating system".to_string())
    } else {
        Some(format!("unknown namespace backend {requested:?}"))
    };
    visit_walkdir_owned(
        target,
        max_depth,
        NamespaceRootPolicy::NoFollow,
        signature_capture,
        &ignore,
        &mut visitor,
        reason,
    )
}

pub(crate) fn visit_with_root_policy_and_signature_capture(
    target: &Path,
    max_depth: Option<usize>,
    root_policy: NamespaceRootPolicy,
    signature_capture: NamespaceSignatureCapture,
    ignore: impl Fn(&Path) -> bool,
    mut visitor: impl FnMut(&NamespaceEntry) -> bool,
) -> NamespaceWalkStats {
    let requested =
        std::env::var("MISTER_LIBRARY_NAMESPACE_BACKEND").unwrap_or_else(|_| "auto".to_string());
    if requested == "walkdir" {
        return visit_walkdir(
            target,
            max_depth,
            root_policy,
            signature_capture,
            &ignore,
            &mut visitor,
            None,
        );
    }

    #[cfg(target_os = "linux")]
    if requested == "fd-relative" || requested == "auto" {
        let visit_started = Instant::now();
        match linux::collect_fd_relative(target, max_depth, root_policy, signature_capture, &ignore)
        {
            Ok(mut capture) => {
                for entry in &capture.entries {
                    let elapsed_us = visit_started.elapsed().as_micros() as u64;
                    capture.stats.first_entry_us.get_or_insert(elapsed_us);
                    capture.stats.final_entry_us = Some(elapsed_us);
                    if !visitor(entry) {
                        break;
                    }
                }
                return capture.stats;
            }
            Err(reason) => {
                let fd_attempt_us = visit_started.elapsed().as_micros() as u64;
                let fallback_started = Instant::now();
                let stats = visit_walkdir(
                    target,
                    max_depth,
                    root_policy,
                    signature_capture,
                    &ignore,
                    &mut visitor,
                    Some(reason.to_string()),
                );
                report_namespace_fallback(target, &reason, fd_attempt_us, fallback_started, &stats);
                return stats;
            }
        }
    }

    if requested == "auto" {
        return visit_walkdir(
            target,
            max_depth,
            root_policy,
            signature_capture,
            &ignore,
            &mut visitor,
            None,
        );
    }

    let reason = if requested == "fd-relative" {
        Some("fd-relative backend unsupported on this operating system".to_string())
    } else {
        Some(format!("unknown namespace backend {requested:?}"))
    };
    visit_walkdir(
        target,
        max_depth,
        root_policy,
        signature_capture,
        &ignore,
        &mut visitor,
        reason,
    )
}

#[cfg(target_os = "linux")]
fn report_namespace_fallback(
    target: &Path,
    failure: &linux::FdCaptureFailure,
    fd_attempt_us: u64,
    fallback_started: Instant,
    stats: &NamespaceWalkStats,
) {
    let fallback_us = fallback_started.elapsed().as_micros() as u64;
    crate::catalog_logln!(
        "namespace_walk_fallback_tsv\ttarget={}\tscope=whole-root\trestart_count=1\toperation={}\tfailure_path={}\tdepth={}\terrno={}\tfd_attempt_us={}\tfallback_us={}\tfallback_entries={}\tfallback_errors={}",
        target.display(),
        failure.operation(),
        failure.path().display(),
        failure.depth(),
        failure
            .errno()
            .map_or_else(|| "none".to_string(), |errno| errno.to_string()),
        fd_attempt_us,
        fallback_us,
        stats.captured_entries,
        stats.errors,
    );
}

#[cfg(target_os = "linux")]
fn report_namespace_subtree_recovery(
    failure: &linux::FdCaptureFailure,
    snapshot: &NamespaceSubtreeSnapshot,
    snapshot_us: u64,
    recovered: bool,
) {
    crate::catalog_logln!(
        "namespace_walk_subtree_recovery_tsv\tscope=subtree\trecovered={}\tattempts=1\toperation={}\tfailure_path={}\tdepth={}\terrno={}\tsnapshot_us={}\tsnapshot_entries={}\tsnapshot_errors={}",
        usize::from(recovered),
        failure.operation(),
        failure.path().display(),
        failure.depth(),
        failure
            .errno()
            .map_or_else(|| "none".to_string(), |errno| errno.to_string()),
        snapshot_us,
        snapshot.entries.len(),
        snapshot.stats.errors,
    );
}

/// Probe a directory and a known set of its immediate child directories.
/// Linux resolves every child relative to one open parent fd, avoiding the
/// repeated full-path lookups that are especially costly on exFAT/FUSE.
pub(crate) fn probe_directory_signatures(
    target: &Path,
    child_paths: &[PathBuf],
) -> DirectorySignatureProbe {
    #[cfg(target_os = "linux")]
    {
        linux::probe_directory_signatures(target, child_paths)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let target_before = std::fs::symlink_metadata(target)
            .ok()
            .filter(|metadata| metadata.is_dir())
            .and_then(|metadata| metadata_signature(metadata.len(), metadata.modified().ok()));
        let child_before = child_paths
            .iter()
            .map(|path| {
                std::fs::symlink_metadata(path)
                    .ok()
                    .filter(|metadata| metadata.is_dir())
                    .and_then(|metadata| {
                        metadata_signature(metadata.len(), metadata.modified().ok())
                    })
            })
            .collect::<Vec<_>>();
        let child_after = child_paths
            .iter()
            .map(|path| {
                std::fs::symlink_metadata(path)
                    .ok()
                    .filter(|metadata| metadata.is_dir())
                    .and_then(|metadata| {
                        metadata_signature(metadata.len(), metadata.modified().ok())
                    })
            })
            .collect::<Vec<_>>();
        let target_after = std::fs::symlink_metadata(target)
            .ok()
            .filter(|metadata| metadata.is_dir())
            .and_then(|metadata| metadata_signature(metadata.len(), metadata.modified().ok()));
        DirectorySignatureProbe {
            target_signature: stable_directory_signature(target_before, target_after),
            child_signatures: child_before
                .into_iter()
                .zip(child_after)
                .map(|(before, after)| stable_directory_signature(before, after))
                .collect(),
        }
    }
}

fn stable_directory_signature(
    before: Option<(u64, i64)>,
    after: Option<(u64, i64)>,
) -> Option<(u64, i64)> {
    before.filter(|signature| Some(*signature) == after)
}

/// Capture one subtree with the established streaming backend. This helper is
/// intentionally synchronous: it is used only as a bounded recovery step for
/// the currently active namespace walk, never as a second SD-card worker.
#[allow(dead_code)]
pub(crate) fn snapshot_walkdir_subtree(
    target: &Path,
    max_depth: Option<usize>,
    ignore: &dyn Fn(&Path) -> bool,
    max_entries: usize,
    max_path_bytes: usize,
) -> NamespaceSubtreeSnapshot {
    let mut entries = Vec::new();
    let mut captured_path_bytes = 0usize;
    let mut over_budget = false;
    let stats = visit_walkdir_owned(
        target,
        max_depth,
        NamespaceRootPolicy::NoFollow,
        NamespaceSignatureCapture::Target,
        ignore,
        &mut |entry| {
            let entry_path_bytes = entry.path.as_os_str().len();
            let Some(next_path_bytes) = captured_path_bytes.checked_add(entry_path_bytes) else {
                over_budget = true;
                return false;
            };
            if entries.len() >= max_entries || next_path_bytes > max_path_bytes {
                over_budget = true;
                return false;
            }
            captured_path_bytes = next_path_bytes;
            entries.push(entry);
            true
        },
        None,
    );
    let complete = !over_budget && stats.errors == 0 && stats.target_signature.is_some();
    NamespaceSubtreeSnapshot {
        entries,
        stats,
        complete,
    }
}

fn visit_walkdir(
    target: &Path,
    max_depth: Option<usize>,
    root_policy: NamespaceRootPolicy,
    signature_capture: NamespaceSignatureCapture,
    ignore: &dyn Fn(&Path) -> bool,
    visitor: &mut dyn FnMut(&NamespaceEntry) -> bool,
    fallback_reason: Option<String>,
) -> NamespaceWalkStats {
    let visit_started = Instant::now();
    let mut builder = walkdir::WalkDir::new(target)
        .follow_links(false)
        .follow_root_links(root_policy == NamespaceRootPolicy::FollowSymlink);
    if let Some(max_depth) = max_depth {
        builder = builder.max_depth(max_depth);
    }
    let mut visited_entries = 0usize;
    let mut peak_buffered_bytes = 0usize;
    let mut first_entry_us = None;
    let mut final_entry_us = None;
    let mut target_signature_before = None;
    let mut errors = 0usize;
    for entry in builder
        .into_iter()
        .filter_entry(|entry| !ignore(entry.path()))
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                errors = errors.saturating_add(1);
                continue;
            }
        };
        if visited_entries.is_multiple_of(16) {
            crate::cooperative_work::checkpoint();
        }
        let path = entry.path();
        if path == target {
            if signature_capture.target() {
                target_signature_before = entry.metadata().ok().and_then(|metadata| {
                    metadata_signature(metadata.len(), metadata.modified().ok())
                });
            }
            continue;
        }
        let kind = if entry.file_type().is_dir() {
            NamespaceEntryKind::Directory
        } else if entry.file_type().is_file() {
            NamespaceEntryKind::File
        } else {
            NamespaceEntryKind::Other
        };
        let zip_signature = if kind == NamespaceEntryKind::File && is_zip_path(path) {
            entry
                .metadata()
                .ok()
                .map(|metadata| (metadata.len(), crate::library_db::mtime_secs(&metadata)))
        } else {
            None
        };
        // WalkDir streams an entry before visiting its children, so it cannot
        // bracket a child-directory signature without buffering the target.
        // Leave child signatures unavailable on this conservative fallback;
        // the warm validator will run the exact path instead.
        let directory_signature = None;
        let visitor_entry = NamespaceEntry {
            path: path.to_path_buf(),
            kind,
            zip_signature,
            directory_signature,
        };
        visited_entries = visited_entries.saturating_add(1);
        peak_buffered_bytes = peak_buffered_bytes.max(
            std::mem::size_of::<NamespaceEntry>()
                .saturating_add(visitor_entry.path.as_os_str().len()),
        );
        let elapsed_us = visit_started.elapsed().as_micros() as u64;
        first_entry_us.get_or_insert(elapsed_us);
        final_entry_us = Some(elapsed_us);
        if !visitor(&visitor_entry) {
            break;
        }
    }
    let target_signature = if signature_capture.target() {
        let after = match root_policy {
            NamespaceRootPolicy::NoFollow => std::fs::symlink_metadata(target),
            NamespaceRootPolicy::FollowSymlink => std::fs::metadata(target),
        }
        .ok()
        .filter(|metadata| metadata.is_dir())
        .and_then(|metadata| metadata_signature(metadata.len(), metadata.modified().ok()));
        stable_directory_signature(target_signature_before, after)
    } else {
        None
    };
    let restarted = fallback_reason.is_some();
    NamespaceWalkStats {
        backend: if restarted {
            "walkdir-fallback"
        } else {
            "walkdir"
        },
        fallback_reason,
        root_open_us: 0,
        dir_opens: 0,
        read_calls: 0,
        read_bytes: 0,
        type_stats: 0,
        stat_calls: 0,
        stat_us: 0,
        signature_stat_calls: 0,
        signature_stat_us: 0,
        canonicalization_count: 0,
        canonicalization_us: 0,
        captured_entries: visited_entries,
        peak_buffered_entries: usize::from(visited_entries > 0),
        peak_buffered_bytes,
        buffer_allocations: 0,
        fallback_count: usize::from(restarted),
        restart_count: usize::from(restarted),
        errors,
        first_entry_us,
        final_entry_us,
        target_signature,
    }
}

#[allow(dead_code)]
fn visit_walkdir_owned(
    target: &Path,
    max_depth: Option<usize>,
    root_policy: NamespaceRootPolicy,
    signature_capture: NamespaceSignatureCapture,
    ignore: &dyn Fn(&Path) -> bool,
    visitor: &mut dyn FnMut(NamespaceEntry) -> bool,
    fallback_reason: Option<String>,
) -> NamespaceWalkStats {
    let mut borrowed = |entry: &NamespaceEntry| {
        visitor(NamespaceEntry {
            path: entry.path.clone(),
            kind: entry.kind,
            zip_signature: entry.zip_signature,
            directory_signature: entry.directory_signature,
        })
    };
    visit_walkdir(
        target,
        max_depth,
        root_policy,
        signature_capture,
        ignore,
        &mut borrowed,
        fallback_reason,
    )
}

fn metadata_signature(len: u64, modified: Option<std::time::SystemTime>) -> Option<(u64, i64)> {
    let mtime_nanos = modified?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos()
        .min(i64::MAX as u128) as i64;
    Some((len, mtime_nanos))
}

fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

#[cfg(any(target_os = "linux", test))]
fn unix_timestamp_nanos(seconds: i64, nanos: i64) -> i64 {
    if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
        return 0;
    }
    i128::from(seconds)
        .saturating_mul(1_000_000_000)
        .saturating_add(i128::from(nanos))
        .min(i128::from(i64::MAX)) as i64
}

#[cfg(target_os = "linux")]
mod linux {
    #[cfg(feature = "builder")]
    use super::KnownPathMetadata;
    use super::{
        DirectorySignatureProbe, NamespaceEntry, NamespaceEntryKind, NamespaceRootPolicy,
        NamespaceSignatureCapture, NamespaceWalkStats, is_zip_path, unix_timestamp_nanos,
    };
    use std::ffi::{CString, OsString};
    use std::fmt;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};

    const GETDENTS_INITIAL_BUFFER_BYTES: usize = 8 * 1024;
    const GETDENTS_MAX_BUFFER_BYTES: usize = 128 * 1024;
    const DIRENT64_HEADER_BYTES: usize = 19;
    const MAX_CAPTURED_ENTRIES: usize = 65_536;
    const MAX_CAPTURED_PATH_BYTES: usize = 16 * 1024 * 1024;
    const MAX_OPEN_DIRECTORY_FDS: usize = 64;

    #[derive(Debug)]
    pub(super) struct NamespaceCapture {
        pub(super) entries: Vec<NamespaceEntry>,
        pub(super) stats: NamespaceWalkStats,
    }

    #[derive(Clone, Copy)]
    struct CaptureBudget {
        max_entries: usize,
        max_path_bytes: usize,
        max_open_fds: usize,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FdCaptureFailureOperation {
        RootOpen,
        RootRead,
        ChildOpen,
        ChildRead,
        EntryStat,
        MalformedDirent,
        Budget,
        Allocation,
        NameEncoding,
    }

    #[derive(Clone, Copy, Debug)]
    struct FaultInjection {
        operation: FdCaptureFailureOperation,
        depth: usize,
    }

    impl FdCaptureFailureOperation {
        fn is_nested_read(self) -> bool {
            self == Self::ChildRead
        }
    }

    #[allow(dead_code)]
    #[derive(Debug)]
    pub(super) struct FdCaptureFailure {
        operation: FdCaptureFailureOperation,
        path: PathBuf,
        depth: usize,
        errno: Option<i32>,
        context: String,
    }

    #[allow(dead_code)]
    impl FdCaptureFailure {
        fn new(
            operation: FdCaptureFailureOperation,
            path: &Path,
            depth: usize,
            errno: Option<i32>,
            context: String,
        ) -> Self {
            Self {
                operation,
                path: path.to_path_buf(),
                depth,
                errno,
                context,
            }
        }

        fn io(
            operation: FdCaptureFailureOperation,
            path: &Path,
            depth: usize,
            error: &io::Error,
            context: String,
        ) -> Self {
            Self::new(operation, path, depth, error.raw_os_error(), context)
        }

        pub(super) fn operation(&self) -> &'static str {
            operation_name(self.operation)
        }

        pub(super) fn path(&self) -> &Path {
            &self.path
        }

        pub(super) fn depth(&self) -> usize {
            self.depth
        }

        pub(super) fn errno(&self) -> Option<i32> {
            self.errno
        }

        pub(super) fn is_recoverable_nested_enoent(&self) -> bool {
            self.operation.is_nested_read() && self.depth > 0 && self.errno == Some(libc::ENOENT)
        }
    }

    impl fmt::Display for FdCaptureFailure {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.context)
        }
    }

    fn operation_name(operation: FdCaptureFailureOperation) -> &'static str {
        match operation {
            FdCaptureFailureOperation::RootOpen => "root-open",
            FdCaptureFailureOperation::RootRead => "root-read",
            FdCaptureFailureOperation::ChildOpen => "child-open",
            FdCaptureFailureOperation::ChildRead => "child-read",
            FdCaptureFailureOperation::EntryStat => "entry-stat",
            FdCaptureFailureOperation::MalformedDirent => "malformed-dirent",
            FdCaptureFailureOperation::Budget => "budget",
            FdCaptureFailureOperation::Allocation => "allocation",
            FdCaptureFailureOperation::NameEncoding => "name-encoding",
        }
    }

    fn injected_failure(
        fault: Option<FaultInjection>,
        operation: FdCaptureFailureOperation,
        path: &Path,
        depth: usize,
    ) -> Option<FdCaptureFailure> {
        let fault = fault.filter(|fault| fault.operation == operation && fault.depth == depth)?;
        let errno = if fault.operation.is_nested_read() {
            libc::ENOENT
        } else {
            libc::EIO
        };
        Some(FdCaptureFailure::new(
            operation,
            path,
            depth,
            Some(errno),
            format!(
                "injected {} failure at {}",
                operation_name(operation),
                path.display()
            ),
        ))
    }

    impl Default for CaptureBudget {
        fn default() -> Self {
            Self {
                max_entries: MAX_CAPTURED_ENTRIES,
                max_path_bytes: MAX_CAPTURED_PATH_BYTES,
                max_open_fds: MAX_OPEN_DIRECTORY_FDS,
            }
        }
    }

    pub(super) fn collect_fd_relative(
        target: &Path,
        max_depth: Option<usize>,
        root_policy: NamespaceRootPolicy,
        signature_capture: NamespaceSignatureCapture,
        ignore: &dyn Fn(&Path) -> bool,
    ) -> Result<NamespaceCapture, FdCaptureFailure> {
        collect_fd_relative_with_budget(
            target,
            max_depth,
            root_policy,
            signature_capture,
            ignore,
            CaptureBudget::default(),
            None,
        )
    }

    pub(super) fn probe_directory_signatures(
        target: &Path,
        child_paths: &[std::path::PathBuf],
    ) -> DirectorySignatureProbe {
        let Ok(target_name) = c_string(target.as_os_str().as_bytes(), "target path") else {
            return unavailable_probe(child_paths.len());
        };
        let raw_fd = open_retry(
            target_name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
        .ok()
        .unwrap_or(-1);
        if raw_fd < 0 {
            return unavailable_probe(child_paths.len());
        }
        let target_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        let target_before = stat_fd(target_fd.as_raw_fd())
            .ok()
            .and_then(directory_signature);
        let child_before = child_paths
            .iter()
            .map(|path| child_directory_signature(target_fd.as_raw_fd(), path))
            .collect::<Vec<_>>();
        let child_after = child_paths
            .iter()
            .map(|path| child_directory_signature(target_fd.as_raw_fd(), path))
            .collect::<Vec<_>>();
        let target_after = stat_fd(target_fd.as_raw_fd())
            .ok()
            .and_then(directory_signature);
        DirectorySignatureProbe {
            target_signature: super::stable_directory_signature(target_before, target_after),
            child_signatures: child_before
                .into_iter()
                .zip(child_after)
                .map(|(before, after)| super::stable_directory_signature(before, after))
                .collect(),
        }
    }

    #[cfg(feature = "builder")]
    pub(super) fn probe_known_path_metadata(
        parent: &Path,
        child_paths: &[PathBuf],
    ) -> Vec<Option<KnownPathMetadata>> {
        use std::os::unix::ffi::OsStrExt;

        let Ok(parent_name) = c_string(parent.as_os_str().as_bytes(), "parent path") else {
            return vec![None; child_paths.len()];
        };
        let raw_fd = open_retry(
            parent_name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
        .ok()
        .unwrap_or(-1);
        if raw_fd < 0 {
            return vec![None; child_paths.len()];
        }
        let parent_fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw_fd) };
        child_paths
            .iter()
            .map(|path| {
                let name = path.file_name()?.as_bytes();
                let name = c_string(name, "child path").ok()?;
                let value = stat_entry_following_symlinks(parent_fd.as_raw_fd(), &name).ok()?;
                Some(known_path_metadata_from_stat(value))
            })
            .collect()
    }

    fn child_directory_signature(parent_fd: RawFd, path: &Path) -> Option<(u64, i64)> {
        let name = path.file_name()?;
        let name = c_string(name.as_bytes(), "directory entry name").ok()?;
        let value = stat_entry(parent_fd, &name, path).ok()?;
        (kind_from_mode(value.st_mode) == NamespaceEntryKind::Directory)
            .then(|| directory_signature(value))
            .flatten()
    }

    fn unavailable_probe(child_count: usize) -> DirectorySignatureProbe {
        DirectorySignatureProbe {
            target_signature: None,
            child_signatures: vec![None; child_count],
        }
    }

    fn collect_fd_relative_with_budget(
        target: &Path,
        max_depth: Option<usize>,
        root_policy: NamespaceRootPolicy,
        signature_capture: NamespaceSignatureCapture,
        ignore: &dyn Fn(&Path) -> bool,
        budget: CaptureBudget,
        fault: Option<FaultInjection>,
    ) -> Result<NamespaceCapture, FdCaptureFailure> {
        collect_fd_relative_with_budget_mode(
            target,
            max_depth,
            root_policy,
            signature_capture,
            ignore,
            budget,
            fault,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_fd_relative_with_budget_mode(
        target: &Path,
        max_depth: Option<usize>,
        root_policy: NamespaceRootPolicy,
        signature_capture: NamespaceSignatureCapture,
        ignore: &dyn Fn(&Path) -> bool,
        budget: CaptureBudget,
        fault: Option<FaultInjection>,
        recover_nested_enoent: bool,
    ) -> Result<NamespaceCapture, FdCaptureFailure> {
        if max_depth == Some(0) || ignore(target) {
            return Ok(NamespaceCapture {
                entries: Vec::new(),
                stats: NamespaceWalkStats {
                    backend: "fd-relative",
                    fallback_reason: None,
                    root_open_us: 0,
                    dir_opens: 0,
                    read_calls: 0,
                    read_bytes: 0,
                    type_stats: 0,
                    stat_calls: 0,
                    stat_us: 0,
                    signature_stat_calls: 0,
                    signature_stat_us: 0,
                    canonicalization_count: 0,
                    canonicalization_us: 0,
                    captured_entries: 0,
                    peak_buffered_entries: 0,
                    peak_buffered_bytes: 0,
                    buffer_allocations: 0,
                    fallback_count: 0,
                    restart_count: 0,
                    errors: 0,
                    first_entry_us: None,
                    final_entry_us: None,
                    target_signature: None,
                },
            });
        }
        if budget.max_open_fds == 0 {
            return Err(FdCaptureFailure::new(
                FdCaptureFailureOperation::Budget,
                target,
                0,
                None,
                "capture budget: zero open directory fds".to_string(),
            ));
        }
        let target_name =
            c_string(target.as_os_str().as_bytes(), "target path").map_err(|context| {
                FdCaptureFailure::new(
                    FdCaptureFailureOperation::NameEncoding,
                    target,
                    0,
                    None,
                    context,
                )
            })?;
        let no_follow = if root_policy == NamespaceRootPolicy::NoFollow {
            libc::O_NOFOLLOW
        } else {
            0
        };
        let root_open_started = std::time::Instant::now();
        let raw_fd = match open_retry(
            target_name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | no_follow,
        ) {
            Ok(fd) => fd,
            Err(error) => {
                return Err(FdCaptureFailure::io(
                    FdCaptureFailureOperation::RootOpen,
                    target,
                    0,
                    &error,
                    format!("open target {}: {error}", target.display()),
                ));
            }
        };
        let root_open_us = root_open_started.elapsed().as_micros() as u64;
        let root = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        if let Some(failure) =
            injected_failure(fault, FdCaptureFailureOperation::RootRead, target, 0)
        {
            return Err(failure);
        }
        let mut entries = Vec::new();
        let mut captured_path_bytes = 0usize;
        let mut stats = NamespaceWalkStats {
            backend: "fd-relative",
            fallback_reason: None,
            root_open_us,
            dir_opens: 1,
            read_calls: 0,
            read_bytes: 0,
            type_stats: 0,
            stat_calls: 0,
            stat_us: 0,
            signature_stat_calls: 0,
            signature_stat_us: 0,
            canonicalization_count: 0,
            canonicalization_us: 0,
            captured_entries: 0,
            peak_buffered_entries: 0,
            peak_buffered_bytes: 0,
            buffer_allocations: 0,
            fallback_count: 0,
            restart_count: 0,
            errors: 0,
            first_entry_us: None,
            final_entry_us: None,
            target_signature: None,
        };
        let target_signature_before = if signature_capture.target() {
            stat_fd_timed(&mut stats, root.as_raw_fd(), true)
                .ok()
                .and_then(directory_signature)
        } else {
            None
        };
        collect_directory(
            &root,
            target,
            0,
            max_depth,
            signature_capture,
            ignore,
            &mut entries,
            &mut captured_path_bytes,
            &mut stats,
            budget,
            fault,
            recover_nested_enoent,
        )?;
        if signature_capture.target() {
            let target_signature_after = stat_fd_timed(&mut stats, root.as_raw_fd(), true)
                .ok()
                .and_then(directory_signature);
            stats.target_signature =
                super::stable_directory_signature(target_signature_before, target_signature_after);
        }
        stats.captured_entries = entries.len();
        Ok(NamespaceCapture { entries, stats })
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_directory(
        directory: &OwnedFd,
        directory_path: &Path,
        depth: usize,
        max_depth: Option<usize>,
        signature_capture: NamespaceSignatureCapture,
        ignore: &dyn Fn(&Path) -> bool,
        entries: &mut Vec<NamespaceEntry>,
        captured_path_bytes: &mut usize,
        stats: &mut NamespaceWalkStats,
        budget: CaptureBudget,
        fault: Option<FaultInjection>,
        recover_nested_enoent: bool,
    ) -> Result<(), FdCaptureFailure> {
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(GETDENTS_INITIAL_BUFFER_BYTES)
            .map_err(|error| {
                FdCaptureFailure::new(
                    FdCaptureFailureOperation::Allocation,
                    directory_path,
                    depth,
                    None,
                    format!("capture allocation: getdents buffer: {error}"),
                )
            })?;
        buffer.resize(GETDENTS_INITIAL_BUFFER_BYTES, 0u8);
        stats.buffer_allocations = stats.buffer_allocations.saturating_add(1);
        if let Some(failure) = injected_failure(
            fault,
            FdCaptureFailureOperation::ChildRead,
            directory_path,
            depth,
        ) {
            return Err(failure);
        }
        loop {
            crate::cooperative_work::checkpoint();
            let read = unsafe {
                libc::syscall(
                    libc::SYS_getdents64,
                    directory.as_raw_fd(),
                    buffer.as_mut_ptr(),
                    buffer.len(),
                )
            };
            stats.read_calls = stats.read_calls.saturating_add(1);
            if read < 0 {
                if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    // Profilers such as pprof deliver SIGPROF asynchronously.
                    // Retry the directory read so an interrupted syscall does
                    // not force an otherwise healthy fd-relative walk onto the
                    // slower full-path fallback backend.
                    continue;
                }
                let error = io::Error::last_os_error();
                let operation = if depth == 0 {
                    FdCaptureFailureOperation::RootRead
                } else {
                    FdCaptureFailureOperation::ChildRead
                };
                return Err(FdCaptureFailure::io(
                    operation,
                    directory_path,
                    depth,
                    &error,
                    format!("getdents64 {}: {error}", directory_path.display()),
                ));
            }
            if read == 0 {
                return Ok(());
            }
            let read = usize::try_from(read).map_err(|_| {
                FdCaptureFailure::new(
                    FdCaptureFailureOperation::MalformedDirent,
                    directory_path,
                    depth,
                    None,
                    "negative getdents64 size".to_string(),
                )
            })?;
            stats.read_bytes = stats.read_bytes.saturating_add(read as u64);
            let mut offset = 0usize;
            while offset < read {
                if entries.len().is_multiple_of(16) {
                    crate::cooperative_work::checkpoint();
                }
                if read - offset < DIRENT64_HEADER_BYTES {
                    return Err(FdCaptureFailure::new(
                        FdCaptureFailureOperation::MalformedDirent,
                        directory_path,
                        depth,
                        None,
                        format!(
                            "truncated getdents64 record in {}",
                            directory_path.display()
                        ),
                    ));
                }
                let record_len =
                    u16::from_ne_bytes([buffer[offset + 16], buffer[offset + 17]]) as usize;
                if record_len <= DIRENT64_HEADER_BYTES || offset + record_len > read {
                    return Err(FdCaptureFailure::new(
                        FdCaptureFailureOperation::MalformedDirent,
                        directory_path,
                        depth,
                        None,
                        format!("invalid getdents64 record in {}", directory_path.display()),
                    ));
                }
                let record = &buffer[offset..offset + record_len];
                offset += record_len;
                let name_region = &record[DIRENT64_HEADER_BYTES..];
                let name_len = name_region
                    .iter()
                    .position(|byte| *byte == 0)
                    .ok_or_else(|| {
                        FdCaptureFailure::new(
                            FdCaptureFailureOperation::MalformedDirent,
                            directory_path,
                            depth,
                            None,
                            format!(
                                "unterminated getdents64 name in {}",
                                directory_path.display()
                            ),
                        )
                    })?;
                let name = &name_region[..name_len];
                if name.is_empty() || name == b"." || name == b".." {
                    continue;
                }
                let child_path = directory_path.join(OsString::from_vec(name.to_vec()));
                if ignore(&child_path) {
                    continue;
                }
                if entries.len() >= budget.max_entries {
                    return Err(FdCaptureFailure::new(
                        FdCaptureFailureOperation::Budget,
                        directory_path,
                        depth,
                        None,
                        format!(
                            "capture budget: more than {} entries under {}",
                            budget.max_entries,
                            directory_path.display()
                        ),
                    ));
                }
                let child_path_bytes = child_path.as_os_str().as_bytes().len();
                let next_path_bytes = captured_path_bytes
                    .checked_add(child_path_bytes)
                    .filter(|total| *total <= budget.max_path_bytes)
                    .ok_or_else(|| {
                        FdCaptureFailure::new(
                            FdCaptureFailureOperation::Budget,
                            directory_path,
                            depth,
                            None,
                            format!(
                                "capture budget: more than {} path bytes under {}",
                                budget.max_path_bytes,
                                directory_path.display()
                            ),
                        )
                    })?;
                let previous_capacity = entries.capacity();
                entries.try_reserve(1).map_err(|error| {
                    FdCaptureFailure::new(
                        FdCaptureFailureOperation::Allocation,
                        directory_path,
                        depth,
                        None,
                        format!("capture allocation: namespace entry: {error}"),
                    )
                })?;
                if entries.capacity() != previous_capacity {
                    stats.buffer_allocations = stats.buffer_allocations.saturating_add(1);
                }
                let entry_depth = depth.saturating_add(1);
                let child_name = c_string(name, "directory entry name").map_err(|context| {
                    FdCaptureFailure::new(
                        FdCaptureFailureOperation::NameEncoding,
                        &child_path,
                        entry_depth,
                        None,
                        context,
                    )
                })?;
                let mut stat = None;
                let kind = match record[18] {
                    libc::DT_DIR => NamespaceEntryKind::Directory,
                    libc::DT_REG => NamespaceEntryKind::File,
                    libc::DT_UNKNOWN => {
                        stats.type_stats = stats.type_stats.saturating_add(1);
                        let value = stat_entry_capture_timed(
                            stats,
                            false,
                            directory.as_raw_fd(),
                            &child_name,
                            &child_path,
                            entry_depth,
                        )?;
                        let kind = kind_from_mode(value.st_mode);
                        stat = Some(value);
                        kind
                    }
                    _ => NamespaceEntryKind::Other,
                };
                let zip_signature = if kind == NamespaceEntryKind::File && is_zip_path(&child_path)
                {
                    let value = match stat {
                        Some(value) => value,
                        None => stat_entry_capture_timed(
                            stats,
                            false,
                            directory.as_raw_fd(),
                            &child_name,
                            &child_path,
                            entry_depth,
                        )?,
                    };
                    Some((
                        u64::try_from(value.st_size).unwrap_or(0),
                        #[allow(clippy::unnecessary_cast)]
                        // libc field widths vary by Unix target.
                        unix_timestamp_nanos(value.st_mtime as i64, value.st_mtime_nsec as i64),
                    ))
                } else {
                    None
                };
                let should_descend = kind == NamespaceEntryKind::Directory
                    && max_depth.is_none_or(|limit| entry_depth < limit);
                let capture_directory_signature = kind == NamespaceEntryKind::Directory
                    && signature_capture.directory_at_depth(entry_depth);
                let mut opened_child = None;
                let mut child_signature_before = None;
                if should_descend {
                    if entry_depth >= budget.max_open_fds {
                        return Err(FdCaptureFailure::new(
                            FdCaptureFailureOperation::Budget,
                            &child_path,
                            entry_depth,
                            None,
                            format!(
                                "capture budget: more than {} open directory fds under {}",
                                budget.max_open_fds,
                                child_path.display()
                            ),
                        ));
                    }
                    let raw_child = match openat_retry(
                        directory.as_raw_fd(),
                        child_name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                    ) {
                        Ok(fd) => fd,
                        Err(error) => {
                            return Err(FdCaptureFailure::io(
                                FdCaptureFailureOperation::ChildOpen,
                                &child_path,
                                entry_depth,
                                &error,
                                format!("openat directory {}: {error}", child_path.display()),
                            ));
                        }
                    };
                    stats.dir_opens = stats.dir_opens.saturating_add(1);
                    opened_child = Some(unsafe { OwnedFd::from_raw_fd(raw_child) });
                    if capture_directory_signature {
                        child_signature_before = opened_child
                            .as_ref()
                            .and_then(|child| stat_fd_timed(stats, child.as_raw_fd(), true).ok())
                            .and_then(directory_signature);
                    }
                }
                let captured_directory_signature = if capture_directory_signature && !should_descend
                {
                    let value = match stat {
                        Some(value) => Some(value),
                        None => stat_entry_capture_timed(
                            stats,
                            true,
                            directory.as_raw_fd(),
                            &child_name,
                            &child_path,
                            entry_depth,
                        )
                        .ok(),
                    };
                    value.and_then(directory_signature)
                } else {
                    None
                };
                let entry_index = entries.len();
                entries.push(NamespaceEntry {
                    path: child_path.clone(),
                    kind,
                    zip_signature,
                    directory_signature: captured_directory_signature,
                });
                *captured_path_bytes = next_path_bytes;
                stats.peak_buffered_entries = stats.peak_buffered_entries.max(entries.len());
                stats.peak_buffered_bytes = stats.peak_buffered_bytes.max(
                    entries
                        .capacity()
                        .saturating_mul(std::mem::size_of::<NamespaceEntry>())
                        .saturating_add(*captured_path_bytes),
                );
                if should_descend {
                    let child = opened_child.expect("descended directory must be open");
                    let child_entries_start = entries.len();
                    let child_path_bytes_before = *captured_path_bytes;
                    match collect_directory(
                        &child,
                        &child_path,
                        entry_depth,
                        max_depth,
                        signature_capture,
                        ignore,
                        entries,
                        captured_path_bytes,
                        stats,
                        budget,
                        fault,
                        recover_nested_enoent,
                    ) {
                        Ok(()) => {
                            if capture_directory_signature {
                                let child_signature_after =
                                    stat_fd_timed(stats, child.as_raw_fd(), true)
                                        .ok()
                                        .and_then(directory_signature);
                                entries[entry_index].directory_signature =
                                    super::stable_directory_signature(
                                        child_signature_before,
                                        child_signature_after,
                                    );
                            }
                        }
                        Err(failure)
                            if recover_nested_enoent
                                && failure.is_recoverable_nested_enoent()
                                && failure.path() == child_path.as_path() =>
                        {
                            let snapshot_started = std::time::Instant::now();
                            let remaining_depth =
                                max_depth.map(|limit| limit.saturating_sub(entry_depth));
                            let snapshot = super::snapshot_walkdir_subtree(
                                &child_path,
                                remaining_depth,
                                ignore,
                                budget.max_entries.saturating_sub(child_entries_start),
                                budget
                                    .max_path_bytes
                                    .saturating_sub(child_path_bytes_before),
                            );
                            let snapshot_us = snapshot_started.elapsed().as_micros() as u64;
                            let snapshot_path_bytes =
                                snapshot.entries.iter().fold(0usize, |total, entry| {
                                    total.saturating_add(entry.path.as_os_str().len())
                                });
                            let snapshot_fits_budget = snapshot.entries.len()
                                <= budget.max_entries.saturating_sub(child_entries_start)
                                && snapshot_path_bytes
                                    <= budget
                                        .max_path_bytes
                                        .saturating_sub(child_path_bytes_before);
                            let recovered = snapshot.complete && snapshot_fits_budget;
                            super::report_namespace_subtree_recovery(
                                &failure,
                                &snapshot,
                                snapshot_us,
                                recovered,
                            );
                            if !recovered {
                                return Err(failure);
                            }
                            let snapshot_target_signature = snapshot.stats.target_signature;
                            entries.truncate(child_entries_start);
                            *captured_path_bytes = child_path_bytes_before;
                            stats.add(&snapshot.stats);
                            // The fd walk restarted this subtree through the
                            // streaming backend after an ENOENT race. Count
                            // that recovery explicitly even though the
                            // overall capture remained complete.
                            stats.fallback_count = stats.fallback_count.saturating_add(1);
                            stats.restart_count = stats.restart_count.saturating_add(1);
                            if stats.fallback_reason.is_none() {
                                stats.fallback_reason = Some("nested-enoent".to_string());
                            }
                            *captured_path_bytes =
                                (*captured_path_bytes).saturating_add(snapshot_path_bytes);
                            entries.extend(snapshot.entries);
                            stats.peak_buffered_entries =
                                stats.peak_buffered_entries.max(entries.len());
                            stats.peak_buffered_bytes = stats.peak_buffered_bytes.max(
                                entries
                                    .capacity()
                                    .saturating_mul(std::mem::size_of::<NamespaceEntry>())
                                    .saturating_add(*captured_path_bytes),
                            );
                            if capture_directory_signature {
                                entries[entry_index].directory_signature =
                                    super::stable_directory_signature(
                                        child_signature_before,
                                        snapshot_target_signature,
                                    );
                            }
                        }
                        Err(failure) => return Err(failure),
                    }
                }
            }
            if read.saturating_add(512) >= buffer.len() && buffer.len() < GETDENTS_MAX_BUFFER_BYTES
            {
                let additional = GETDENTS_MAX_BUFFER_BYTES.saturating_sub(buffer.len());
                buffer.try_reserve_exact(additional).map_err(|error| {
                    FdCaptureFailure::new(
                        FdCaptureFailureOperation::Allocation,
                        directory_path,
                        depth,
                        None,
                        format!("capture allocation: grow getdents buffer: {error}"),
                    )
                })?;
                buffer.resize(GETDENTS_MAX_BUFFER_BYTES, 0u8);
                stats.buffer_allocations = stats.buffer_allocations.saturating_add(1);
            }
        }
    }

    fn c_string(bytes: &[u8], description: &str) -> Result<CString, String> {
        CString::new(bytes).map_err(|_| format!("NUL in {description}"))
    }

    fn stat_entry(directory: RawFd, name: &CString, path: &Path) -> Result<libc::stat, String> {
        loop {
            let mut value = std::mem::MaybeUninit::<libc::stat>::uninit();
            let result = unsafe {
                libc::fstatat(
                    directory,
                    name.as_ptr(),
                    value.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result == 0 {
                return Ok(unsafe { value.assume_init() });
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(format!("fstatat {}: {error}", path.display()));
        }
    }

    fn stat_entry_capture(
        directory: RawFd,
        name: &CString,
        path: &Path,
        depth: usize,
    ) -> Result<libc::stat, FdCaptureFailure> {
        loop {
            let mut value = std::mem::MaybeUninit::<libc::stat>::uninit();
            let result = unsafe {
                libc::fstatat(
                    directory,
                    name.as_ptr(),
                    value.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result == 0 {
                return Ok(unsafe { value.assume_init() });
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(FdCaptureFailure::io(
                FdCaptureFailureOperation::EntryStat,
                path,
                depth,
                &error,
                format!("fstatat {}: {error}", path.display()),
            ));
        }
    }

    fn stat_entry_capture_timed(
        stats: &mut NamespaceWalkStats,
        signature: bool,
        directory: RawFd,
        name: &CString,
        path: &Path,
        depth: usize,
    ) -> Result<libc::stat, FdCaptureFailure> {
        let started = std::time::Instant::now();
        let result = stat_entry_capture(directory, name, path, depth);
        record_stat_duration(stats, signature, started);
        result
    }

    fn stat_fd_timed(
        stats: &mut NamespaceWalkStats,
        fd: RawFd,
        signature: bool,
    ) -> Result<libc::stat, io::Error> {
        let started = std::time::Instant::now();
        let result = stat_fd(fd);
        record_stat_duration(stats, signature, started);
        result
    }

    fn record_stat_duration(
        stats: &mut NamespaceWalkStats,
        signature: bool,
        started: std::time::Instant,
    ) {
        let elapsed_us = started.elapsed().as_micros() as u64;
        stats.stat_calls = stats.stat_calls.saturating_add(1);
        stats.stat_us = stats.stat_us.saturating_add(elapsed_us);
        if signature {
            stats.signature_stat_calls = stats.signature_stat_calls.saturating_add(1);
            stats.signature_stat_us = stats.signature_stat_us.saturating_add(elapsed_us);
        }
    }

    #[cfg(feature = "builder")]
    fn stat_entry_following_symlinks(
        directory: RawFd,
        name: &CString,
    ) -> Result<libc::stat, io::Error> {
        loop {
            let mut value = std::mem::MaybeUninit::<libc::stat>::uninit();
            let result = unsafe { libc::fstatat(directory, name.as_ptr(), value.as_mut_ptr(), 0) };
            if result == 0 {
                return Ok(unsafe { value.assume_init() });
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(error);
        }
    }

    fn open_retry(path: *const libc::c_char, flags: libc::c_int) -> io::Result<RawFd> {
        loop {
            let fd = unsafe { libc::open(path, flags) };
            if fd >= 0 {
                return Ok(fd);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(error);
        }
    }

    fn openat_retry(
        directory: RawFd,
        path: *const libc::c_char,
        flags: libc::c_int,
    ) -> io::Result<RawFd> {
        loop {
            let fd = unsafe { libc::openat(directory, path, flags) };
            if fd >= 0 {
                return Ok(fd);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(error);
        }
    }

    #[cfg(feature = "builder")]
    fn known_path_metadata_from_stat(value: libc::stat) -> KnownPathMetadata {
        #[cfg(target_pointer_width = "32")]
        let inode = u64::from(value.st_ino);
        #[cfg(target_pointer_width = "64")]
        let inode = value.st_ino;
        KnownPathMetadata {
            is_dir: kind_from_mode(value.st_mode) == NamespaceEntryKind::Directory,
            is_file: kind_from_mode(value.st_mode) == NamespaceEntryKind::File,
            size: u64::try_from(value.st_size).unwrap_or(0),
            modified_ns: i128::from(value.st_mtime) * 1_000_000_000
                + i128::from(value.st_mtime_nsec),
            changed_ns: i128::from(value.st_ctime) * 1_000_000_000
                + i128::from(value.st_ctime_nsec),
            inode,
        }
    }

    fn stat_fd(fd: RawFd) -> Result<libc::stat, io::Error> {
        let mut value = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe { libc::fstat(fd, value.as_mut_ptr()) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { value.assume_init() })
    }

    fn directory_signature(value: libc::stat) -> Option<(u64, i64)> {
        #[allow(clippy::unnecessary_cast)]
        let seconds = value.st_mtime as i64;
        #[allow(clippy::unnecessary_cast)]
        let nanos = value.st_mtime_nsec as i64;
        if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
            return None;
        }
        Some((
            u64::try_from(value.st_size).unwrap_or(0),
            unix_timestamp_nanos(seconds, nanos),
        ))
    }

    fn kind_from_mode(mode: libc::mode_t) -> NamespaceEntryKind {
        match mode & libc::S_IFMT {
            libc::S_IFDIR => NamespaceEntryKind::Directory,
            libc::S_IFREG => NamespaceEntryKind::File,
            _ => NamespaceEntryKind::Other,
        }
    }

    #[cfg(test)]
    pub(super) fn collect_with_budget_for_test(
        target: &Path,
        max_depth: Option<usize>,
        signature_capture: NamespaceSignatureCapture,
        ignore: &dyn Fn(&Path) -> bool,
        max_entries: usize,
        max_path_bytes: usize,
        max_open_fds: usize,
    ) -> Result<NamespaceCapture, String> {
        collect_fd_relative_with_budget(
            target,
            max_depth,
            NamespaceRootPolicy::NoFollow,
            signature_capture,
            ignore,
            CaptureBudget {
                max_entries,
                max_path_bytes,
                max_open_fds,
            },
            None,
        )
        .map_err(|error| error.to_string())
    }

    #[cfg(test)]
    pub(super) fn collect_with_fault_for_test(
        target: &Path,
        max_depth: Option<usize>,
        signature_capture: NamespaceSignatureCapture,
        ignore: &dyn Fn(&Path) -> bool,
        operation: &'static str,
        depth: usize,
    ) -> Result<NamespaceCapture, FdCaptureFailure> {
        let operation = match operation {
            "root-open" => FdCaptureFailureOperation::RootOpen,
            "root-read" => FdCaptureFailureOperation::RootRead,
            "child-open" => FdCaptureFailureOperation::ChildOpen,
            "child-read" => FdCaptureFailureOperation::ChildRead,
            "entry-stat" => FdCaptureFailureOperation::EntryStat,
            "malformed-dirent" => FdCaptureFailureOperation::MalformedDirent,
            "budget" => FdCaptureFailureOperation::Budget,
            "allocation" => FdCaptureFailureOperation::Allocation,
            "name-encoding" => FdCaptureFailureOperation::NameEncoding,
            _ => panic!("unknown fd capture fault operation {operation}"),
        };
        collect_fd_relative_with_budget_mode(
            target,
            max_depth,
            NamespaceRootPolicy::NoFollow,
            signature_capture,
            ignore,
            CaptureBudget::default(),
            Some(FaultInjection { operation, depth }),
            false,
        )
    }

    #[cfg(test)]
    pub(super) fn collect_with_fault_recovery_for_test(
        target: &Path,
        max_depth: Option<usize>,
        signature_capture: NamespaceSignatureCapture,
        ignore: &dyn Fn(&Path) -> bool,
        operation: &'static str,
        depth: usize,
    ) -> Result<NamespaceCapture, FdCaptureFailure> {
        let operation = match operation {
            "root-open" => FdCaptureFailureOperation::RootOpen,
            "root-read" => FdCaptureFailureOperation::RootRead,
            "child-open" => FdCaptureFailureOperation::ChildOpen,
            "child-read" => FdCaptureFailureOperation::ChildRead,
            "entry-stat" => FdCaptureFailureOperation::EntryStat,
            "malformed-dirent" => FdCaptureFailureOperation::MalformedDirent,
            "budget" => FdCaptureFailureOperation::Budget,
            "allocation" => FdCaptureFailureOperation::Allocation,
            "name-encoding" => FdCaptureFailureOperation::NameEncoding,
            _ => panic!("unknown fd capture fault operation {operation}"),
        };
        collect_fd_relative_with_budget(
            target,
            max_depth,
            NamespaceRootPolicy::NoFollow,
            signature_capture,
            ignore,
            CaptureBudget::default(),
            Some(FaultInjection { operation, depth }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_temp_dir;
    use std::fs;

    type SnapshotEntry = (PathBuf, NamespaceEntryKind, Option<(u64, i64)>);
    type Snapshot = Vec<SnapshotEntry>;

    fn walkdir_snapshot(
        root: &Path,
        max_depth: Option<usize>,
        ignore: &dyn Fn(&Path) -> bool,
    ) -> (Snapshot, NamespaceWalkStats) {
        let mut entries = Vec::new();
        let stats = visit_walkdir(
            root,
            max_depth,
            NamespaceRootPolicy::NoFollow,
            NamespaceSignatureCapture::None,
            ignore,
            &mut |entry| {
                entries.push((
                    entry.path.strip_prefix(root).unwrap().to_path_buf(),
                    entry.kind,
                    entry.zip_signature,
                ));
                true
            },
            None,
        );
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        (entries, stats)
    }

    #[test]
    fn timestamp_nanos_matches_metadata_contract_edges() {
        assert_eq!(
            unix_timestamp_nanos(1_700_000_123, 456_789_012),
            1_700_000_123_456_789_012
        );
        assert_eq!(unix_timestamp_nanos(-1, 999_999_999), 0);
        assert_eq!(unix_timestamp_nanos(1, -1), 0);
        assert_eq!(unix_timestamp_nanos(1, 1_000_000_000), 0);
        assert_eq!(unix_timestamp_nanos(i64::MAX, 0), i64::MAX);
    }

    #[cfg(unix)]
    #[test]
    fn walkdir_root_symlink_is_not_followed() {
        use std::os::unix::fs::symlink;

        let dir = unique_temp_dir("namespace-root-symlink");
        let outside = dir.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("sentinel.rom"), b"sentinel").unwrap();
        let link = dir.join("link");
        symlink(&outside, &link).unwrap();

        let (entries, _) = walkdir_snapshot(&link, None, &|_| false);
        assert!(entries.is_empty());

        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn root_only_policy_follows_root_but_not_nested_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = unique_temp_dir("namespace-root-only-symlink");
        let launchers = dir.join("launchers");
        let duplicate = dir.join("duplicate");
        fs::create_dir_all(&launchers).unwrap();
        fs::create_dir_all(&duplicate).unwrap();
        fs::write(launchers.join("Game.mgl"), b"mgl").unwrap();
        fs::write(duplicate.join("Duplicate.mgl"), b"mgl").unwrap();
        symlink(&duplicate, launchers.join("Collection")).unwrap();
        let root = dir.join("root");
        symlink(&launchers, &root).unwrap();

        let mut paths = Vec::new();
        visit_with_root_policy_and_signature_capture(
            &root,
            None,
            NamespaceRootPolicy::FollowSymlink,
            NamespaceSignatureCapture::None,
            |_| false,
            |entry| {
                paths.push(entry.path.clone());
                true
            },
        );

        assert!(paths.contains(&root.join("Game.mgl")));
        assert!(paths.contains(&root.join("Collection")));
        assert!(!paths.contains(&root.join("Collection/Duplicate.mgl")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn walkdir_depth_and_ignore_are_streamed_exactly() {
        let dir = unique_temp_dir("namespace-walkdir-depth");
        fs::create_dir_all(dir.join("dir/deep")).unwrap();
        fs::create_dir_all(dir.join("ignored")).unwrap();
        fs::write(dir.join("one.rom"), b"one").unwrap();
        fs::write(dir.join("dir/two.rom"), b"two").unwrap();
        fs::write(dir.join("dir/deep/three.rom"), b"three").unwrap();
        fs::write(dir.join("ignored/hidden.rom"), b"hidden").unwrap();
        let ignore = |path: &Path| path.file_name().is_some_and(|name| name == "ignored");

        let (depth_zero, _) = walkdir_snapshot(&dir, Some(0), &ignore);
        assert!(depth_zero.is_empty());
        let (depth_one, _) = walkdir_snapshot(&dir, Some(1), &ignore);
        assert!(
            depth_one
                .iter()
                .any(|entry| entry.0 == Path::new("one.rom"))
        );
        assert!(
            !depth_one
                .iter()
                .any(|entry| entry.0 == Path::new("dir/two.rom"))
        );
        let (depth_two, _) = walkdir_snapshot(&dir, Some(2), &ignore);
        assert!(
            depth_two
                .iter()
                .any(|entry| entry.0 == Path::new("dir/two.rom"))
        );
        assert!(
            !depth_two
                .iter()
                .any(|entry| entry.0 == Path::new("dir/deep/three.rom"))
        );
        let (unbounded, _) = walkdir_snapshot(&dir, None, &ignore);
        assert!(
            unbounded
                .iter()
                .any(|entry| entry.0 == Path::new("dir/deep/three.rom"))
        );
        assert!(!unbounded.iter().any(|entry| entry.0.starts_with("ignored")));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn walkdir_subtree_snapshot_publishes_only_a_complete_directory() {
        let dir = unique_temp_dir("namespace-walkdir-snapshot");
        fs::create_dir_all(dir.join("nested/deep")).unwrap();
        fs::write(dir.join("one.rom"), b"one").unwrap();
        fs::write(dir.join("nested/two.rom"), b"two").unwrap();

        let snapshot = snapshot_walkdir_subtree(&dir, None, &|_| false, usize::MAX, usize::MAX);
        assert!(snapshot.complete);
        assert!(snapshot.stats.target_signature.is_some());
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| entry.path == dir.join("nested/two.rom"))
        );

        let missing = snapshot_walkdir_subtree(
            &dir.join("missing"),
            None,
            &|_| false,
            usize::MAX,
            usize::MAX,
        );
        assert!(!missing.complete);
        assert!(missing.entries.is_empty());
        assert!(missing.stats.errors > 0);

        let file = dir.join("one.rom");
        let file_snapshot =
            snapshot_walkdir_subtree(&file, None, &|_| false, usize::MAX, usize::MAX);
        assert!(!file_snapshot.complete);
        assert!(file_snapshot.entries.is_empty());

        let bounded = snapshot_walkdir_subtree(&dir, None, &|_| false, 1, usize::MAX);
        assert!(!bounded.complete);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn directory_signature_capture_is_opt_in_and_depth_bounded() {
        let dir = unique_temp_dir("namespace-signature-depth");
        fs::create_dir_all(dir.join("direct/deep")).unwrap();
        fs::write(dir.join("direct/deep/game.rom"), b"game").unwrap();

        let mut default_direct = None;
        let default = visit(
            &dir,
            Some(2),
            |_| false,
            |entry| {
                if entry.path == dir.join("direct") {
                    default_direct = entry.directory_signature;
                }
                true
            },
        );
        assert_eq!(default.target_signature, None);
        assert_eq!(default_direct, None);

        let mut direct = None;
        let mut deep = None;
        let captured = visit_with_signature_capture(
            &dir,
            Some(2),
            NamespaceSignatureCapture::TargetAndDepthOneDirectories,
            |_| false,
            |entry| {
                if entry.path == dir.join("direct") {
                    direct = entry.directory_signature;
                } else if entry.path == dir.join("direct/deep") {
                    deep = entry.directory_signature;
                }
                true
            },
        );
        assert!(captured.target_signature.is_some());
        #[cfg(target_os = "linux")]
        assert!(direct.is_some());
        #[cfg(not(target_os = "linux"))]
        assert_eq!(direct, None);
        assert_eq!(deep, None);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mutation_between_signature_brackets_forces_exact_fallback() {
        let before = Some((4096, 1_700_000_000_000_000_000));
        let after = Some((4096, 1_700_000_000_000_000_001));

        assert_eq!(stable_directory_signature(before, before), before);
        assert_eq!(stable_directory_signature(before, after), None);
        assert_eq!(stable_directory_signature(before, None), None);
        assert_eq!(stable_directory_signature(None, after), None);
    }

    #[test]
    fn batched_directory_probe_rejects_missing_and_non_directories() {
        let dir = unique_temp_dir("namespace-signature-probe");
        let child = dir.join("child");
        let file = dir.join("file.rom");
        let missing = dir.join("missing");
        fs::create_dir_all(&child).unwrap();
        fs::write(&file, b"file").unwrap();

        let probe = probe_directory_signatures(&dir, &[child.clone(), file, missing]);

        assert!(probe.target_signature.is_some());
        assert!(probe.child_signatures[0].is_some());
        assert_eq!(probe.child_signatures[1], None);
        assert_eq!(probe.child_signatures[2], None);

        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(feature = "builder")]
    #[test]
    fn known_path_metadata_batches_directory_and_file_children() {
        let dir = unique_temp_dir("namespace-known-path-probe");
        let child = dir.join("child");
        let file = dir.join("file.rom");
        let missing = dir.join("missing");
        fs::create_dir_all(&child).unwrap();
        fs::write(&file, b"file").unwrap();

        let observations =
            probe_known_path_metadata(&dir, &[child.clone(), file.clone(), missing.clone()]);

        assert_eq!(observations.len(), 3);
        assert!(observations[0].is_some_and(|value| value.is_dir));
        assert!(observations[1].is_some_and(|value| value.is_file && value.size == 4));
        assert_eq!(observations[2], None);

        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn fd_snapshot(
        root: &Path,
        max_depth: Option<usize>,
        ignore: &dyn Fn(&Path) -> bool,
    ) -> Snapshot {
        let mut entries = linux::collect_fd_relative(
            root,
            max_depth,
            NamespaceRootPolicy::NoFollow,
            NamespaceSignatureCapture::None,
            ignore,
        )
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| {
            (
                entry.path.strip_prefix(root).unwrap().to_path_buf(),
                entry.kind,
                entry.zip_signature,
            )
        })
        .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fd_relative_matches_walkdir_and_zip_signature() {
        use crate::test_support::{set_file_mtime_for_test, write_stored_zip};
        use std::os::unix::ffi::OsStringExt;

        let dir = unique_temp_dir("namespace-fd-parity");
        fs::create_dir_all(dir.join("dir/deep")).unwrap();
        fs::create_dir_all(dir.join("ignored")).unwrap();
        fs::write(dir.join("one.rom"), b"one").unwrap();
        fs::write(dir.join("dir/two.rom"), b"two").unwrap();
        fs::write(dir.join("dir/deep/three.rom"), b"three").unwrap();
        fs::write(dir.join("ignored/hidden.rom"), b"hidden").unwrap();
        let zip = dir.join(std::ffi::OsString::from_vec(b"odd\x80.ZIP".to_vec()));
        write_stored_zip(&zip, &[("game.rom", b"game")]);
        set_file_mtime_for_test(&zip, 1_700_000_123, 456_789_012);
        let ignore = |path: &Path| path.file_name().is_some_and(|name| name == "ignored");

        for max_depth in [Some(0), Some(1), Some(2), None] {
            let (walkdir, _) = walkdir_snapshot(&dir, max_depth, &ignore);
            assert_eq!(fd_snapshot(&dir, max_depth, &ignore), walkdir);
        }
        let expected = fs::metadata(&zip).unwrap();
        let signature = fd_snapshot(&dir, None, &ignore)
            .into_iter()
            .find_map(|entry| (entry.0.as_os_str() == zip.file_name().unwrap()).then_some(entry.2))
            .flatten()
            .unwrap();
        assert_eq!(
            signature,
            (expected.len(), crate::library_db::mtime_secs(&expected))
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fd_capture_budget_fails_before_publishing() {
        let dir = unique_temp_dir("namespace-fd-budget");
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("one.rom"), b"one").unwrap();
        fs::write(dir.join("nested/two.rom"), b"two").unwrap();

        let error = linux::collect_with_budget_for_test(
            &dir,
            None,
            NamespaceSignatureCapture::None,
            &|_| false,
            1,
            usize::MAX,
            usize::MAX,
        )
        .err()
        .unwrap();
        assert!(error.starts_with("capture budget:"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fd_capture_failure_preserves_nested_read_context() {
        let dir = unique_temp_dir("namespace-fd-failure-context");
        fs::create_dir_all(dir.join("nested")).unwrap();

        let error = linux::collect_with_fault_for_test(
            &dir,
            None,
            NamespaceSignatureCapture::None,
            &|_| false,
            "child-read",
            1,
        )
        .expect_err("the test-only fault injector must fail the nested read");

        assert_eq!(error.operation(), "child-read");
        assert_eq!(error.path(), dir.join("nested").as_path());
        assert_eq!(error.depth(), 1);
        assert_eq!(error.errno(), Some(libc::ENOENT));
        assert!(error.is_recoverable_nested_enoent());

        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recoverable_nested_enoent_replaces_only_the_affected_subtree() {
        let dir = unique_temp_dir("namespace-fd-subtree-recovery");
        fs::create_dir_all(dir.join("nested/deep")).unwrap();
        fs::create_dir_all(dir.join("sibling")).unwrap();
        fs::write(dir.join("one.rom"), b"one").unwrap();
        fs::write(dir.join("nested/two.rom"), b"two").unwrap();
        fs::write(dir.join("nested/deep/three.rom"), b"three").unwrap();
        fs::write(dir.join("sibling/four.rom"), b"four").unwrap();

        let capture = linux::collect_with_fault_recovery_for_test(
            &dir,
            None,
            NamespaceSignatureCapture::None,
            &|_| false,
            "child-read",
            1,
        )
        .expect("nested ENOENT should recover through the subtree snapshot");
        assert_eq!(capture.stats.captured_entries, capture.entries.len());
        let mut recovered = capture
            .entries
            .into_iter()
            .map(|entry| {
                (
                    entry.path.strip_prefix(&dir).unwrap().to_path_buf(),
                    entry.kind,
                    entry.zip_signature,
                )
            })
            .collect::<Vec<_>>();
        recovered.sort_by(|left, right| left.0.cmp(&right.0));

        assert_eq!(recovered, walkdir_snapshot(&dir, None, &|_| false).0);
        assert_eq!(capture.stats.backend, "mixed");
        assert_eq!(capture.stats.fallback_count, 1);
        assert_eq!(capture.stats.restart_count, 1);
        assert_eq!(
            capture.stats.fallback_reason.as_deref(),
            Some("nested-enoent")
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fd_relative_edge_matrix_preserves_parity_and_attribution() {
        use crate::test_support::write_stored_zip;
        use std::os::unix::fs::symlink;

        let dir = unique_temp_dir("namespace-fd-edge-matrix");
        fs::create_dir_all(dir.join("deep/one/two")).unwrap();
        fs::create_dir_all(dir.join("mixed/child")).unwrap();
        fs::create_dir_all(dir.join("Case")).unwrap();
        fs::create_dir_all(dir.join("case")).unwrap();
        fs::create_dir_all(dir.join("ignored")).unwrap();
        fs::create_dir_all(dir.join("empty")).unwrap();
        fs::create_dir_all(dir.join("symlink-target")).unwrap();
        fs::write(dir.join("flat.rom"), b"flat").unwrap();
        fs::write(dir.join("deep/one/two/deep.rom"), b"deep").unwrap();
        fs::write(dir.join("mixed/mixed.rom"), b"mixed").unwrap();
        fs::write(dir.join("mixed/child/nested.rom"), b"nested").unwrap();
        fs::write(dir.join("Case/Upper.rom"), b"case").unwrap();
        fs::write(dir.join("case/lower.rom"), b"case").unwrap();
        fs::write(dir.join("ignored/ignored.rom"), b"ignored").unwrap();
        fs::write(dir.join("symlink-target/hidden.rom"), b"target").unwrap();
        write_stored_zip(&dir.join("archive.ZIP"), &[("inside.rom", b"zip")]);
        symlink(dir.join("symlink-target"), dir.join("symlink")).expect("create nested symlink");
        let ignore = |path: &Path| path.file_name().is_some_and(|name| name == "ignored");

        let expected = walkdir_snapshot(&dir, None, &ignore).0;
        let capture = linux::collect_fd_relative(
            &dir,
            None,
            NamespaceRootPolicy::NoFollow,
            NamespaceSignatureCapture::TargetAndDepthOneDirectories,
            &ignore,
        )
        .expect("edge matrix capture");
        let mut actual = capture
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.path.strip_prefix(&dir).unwrap().to_path_buf(),
                    entry.kind,
                    entry.zip_signature,
                )
            })
            .collect::<Vec<_>>();
        actual.sort_by(|left, right| left.0.cmp(&right.0));

        assert_eq!(actual, expected);
        assert_eq!(capture.stats.captured_entries, actual.len());
        assert!(capture.stats.read_calls > 0);
        assert!(capture.stats.read_bytes > 0);
        assert!(capture.stats.dir_opens >= 1);
        assert!(capture.stats.stat_calls >= capture.stats.signature_stat_calls);
        assert!(capture.stats.signature_stat_calls > 0);
        assert!(capture.stats.signature_stat_us <= capture.stats.stat_us);
        assert_eq!(capture.stats.canonicalization_count, 0);

        let missing = linux::collect_fd_relative(
            &dir.join("ENOENT"),
            None,
            NamespaceRootPolicy::NoFollow,
            NamespaceSignatureCapture::None,
            &|_| false,
        )
        .expect_err("missing root must retain the typed ENOENT failure");
        assert_eq!(missing.operation(), "root-open");
        assert_eq!(missing.errno(), Some(libc::ENOENT));

        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn incomplete_exact_subtree_recovery_does_not_escalate_to_an_ancestor() {
        use std::cell::Cell;

        let dir = unique_temp_dir("namespace-fd-incomplete-subtree-recovery");
        let deep = dir.join("nested/deep");
        fs::create_dir_all(deep.join("leaf")).unwrap();
        fs::create_dir_all(dir.join("sibling")).unwrap();
        fs::write(deep.join("game.rom"), b"game").unwrap();
        fs::write(dir.join("sibling/other.rom"), b"other").unwrap();

        let deep_ignore_calls = Cell::new(0usize);
        let ignore = |path: &Path| {
            if path == deep {
                let call = deep_ignore_calls.get();
                deep_ignore_calls.set(call.saturating_add(1));
                call > 0
            } else {
                false
            }
        };
        let error = linux::collect_with_fault_recovery_for_test(
            &dir,
            None,
            NamespaceSignatureCapture::None,
            &ignore,
            "child-read",
            2,
        )
        .expect_err("an incomplete exact snapshot must retain the typed failure");

        assert_eq!(error.path(), deep.as_path());
        assert_eq!(error.depth(), 2);
        assert!(error.is_recoverable_nested_enoent());
        assert!(deep_ignore_calls.get() >= 2);

        let mut fallback_entries = Vec::new();
        let fallback_stats = visit_walkdir(
            &dir,
            None,
            NamespaceRootPolicy::NoFollow,
            NamespaceSignatureCapture::None,
            &ignore,
            &mut |entry| {
                fallback_entries.push((
                    entry.path.strip_prefix(&dir).unwrap().to_path_buf(),
                    entry.kind,
                    entry.zip_signature,
                ));
                true
            },
            Some(error.to_string()),
        );
        fallback_entries.sort_by(|left, right| left.0.cmp(&right.0));

        assert_eq!(fallback_entries, walkdir_snapshot(&dir, None, &ignore).0);
        assert_eq!(fallback_stats.backend, "walkdir-fallback");
        assert_eq!(fallback_stats.fallback_count, 1);
        assert_eq!(fallback_stats.restart_count, 1);
        assert!(fallback_stats.fallback_reason.is_some());

        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn injected_mid_target_failure_restarts_with_identical_output() {
        let dir = unique_temp_dir("namespace-fd-restart");
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("one.rom"), b"one").unwrap();
        fs::write(dir.join("nested/two.rom"), b"two").unwrap();

        let injected = linux::collect_with_budget_for_test(
            &dir,
            None,
            NamespaceSignatureCapture::None,
            &|_| false,
            1,
            usize::MAX,
            usize::MAX,
        )
        .expect_err("the deterministic entry budget must fail mid-target");
        let expected = walkdir_snapshot(&dir, None, &|_| false).0;
        let mut restarted = Vec::new();
        let stats = visit_walkdir(
            &dir,
            None,
            NamespaceRootPolicy::NoFollow,
            NamespaceSignatureCapture::None,
            &|_| false,
            &mut |entry| {
                restarted.push((
                    entry.path.strip_prefix(&dir).unwrap().to_path_buf(),
                    entry.kind,
                    entry.zip_signature,
                ));
                true
            },
            Some(injected),
        );
        restarted.sort_by(|left, right| left.0.cmp(&right.0));

        assert_eq!(restarted, expected);
        assert_eq!(stats.backend, "walkdir-fallback");
        assert_eq!(stats.fallback_count, 1);
        assert_eq!(stats.restart_count, 1);

        fs::remove_dir_all(dir).unwrap();
    }
}
