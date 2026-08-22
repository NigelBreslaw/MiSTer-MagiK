// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Collection-independent namespace traversal backends.
//!
//! The established WalkDir backend streams entries directly to the caller.
//! Linux can optionally avoid repeatedly resolving full directory paths by
//! walking with `openat` and `getdents64`. The optional backend captures a
//! bounded target before publishing it. A benchmark-selected variant instead
//! publishes entries as one reusable `getdents64` buffer is consumed; any
//! partial failure explicitly restarts the target before WalkDir fallback.

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NamespaceSignatureCapture {
    None,
    Target,
    TargetAndDepthOneDirectories,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum NamespaceRootPolicy {
    #[default]
    NoFollow,
    FollowSymlink,
}

impl NamespaceSignatureCapture {
    fn target(self) -> bool {
        matches!(self, Self::Target | Self::TargetAndDepthOneDirectories)
    }

    #[cfg(target_os = "linux")]
    fn depth_one_directories(self) -> bool {
        matches!(self, Self::TargetAndDepthOneDirectories)
    }
}

#[derive(Debug)]
pub(crate) struct NamespaceWalkStats {
    pub(crate) backend: &'static str,
    pub(crate) fallback_reason: Option<String>,
    pub(crate) dir_opens: usize,
    pub(crate) read_calls: usize,
    pub(crate) read_bytes: u64,
    pub(crate) type_stats: usize,
    pub(crate) captured_entries: usize,
    pub(crate) peak_buffered_entries: usize,
    pub(crate) peak_buffered_bytes: usize,
    pub(crate) buffer_allocations: usize,
    pub(crate) fallback_count: usize,
    pub(crate) restart_count: usize,
    pub(crate) first_entry_us: Option<u64>,
    pub(crate) final_entry_us: Option<u64>,
    pub(crate) target_signature: Option<(u64, i64)>,
}

#[derive(Debug)]
pub(crate) struct DirectorySignatureProbe {
    pub(crate) target_signature: Option<(u64, i64)>,
    pub(crate) child_signatures: Vec<Option<(u64, i64)>>,
}

pub(crate) enum NamespaceVisit<'a> {
    Entry(&'a NamespaceEntry),
    #[cfg_attr(
        not(target_os = "linux"),
        expect(dead_code, reason = "streaming restarts are Linux-only")
    )]
    Restart(&'a str),
}

impl NamespaceWalkStats {
    pub(crate) fn add(&mut self, other: &Self) {
        if self.backend != other.backend {
            self.backend = "mixed";
        }
        if self.fallback_reason.is_none() {
            self.fallback_reason.clone_from(&other.fallback_reason);
        }
        self.dir_opens = self.dir_opens.saturating_add(other.dir_opens);
        self.read_calls = self.read_calls.saturating_add(other.read_calls);
        self.read_bytes = self.read_bytes.saturating_add(other.read_bytes);
        self.type_stats = self.type_stats.saturating_add(other.type_stats);
        self.captured_entries = self.captured_entries.saturating_add(other.captured_entries);
        self.peak_buffered_entries = self.peak_buffered_entries.max(other.peak_buffered_entries);
        self.peak_buffered_bytes = self.peak_buffered_bytes.max(other.peak_buffered_bytes);
        self.buffer_allocations = self
            .buffer_allocations
            .saturating_add(other.buffer_allocations);
        self.fallback_count = self.fallback_count.saturating_add(other.fallback_count);
        self.restart_count = self.restart_count.saturating_add(other.restart_count);
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
#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn visit_with_root_policy_and_signature_capture(
    target: &Path,
    max_depth: Option<usize>,
    root_policy: NamespaceRootPolicy,
    signature_capture: NamespaceSignatureCapture,
    ignore: impl Fn(&Path) -> bool,
    mut visitor: impl FnMut(&NamespaceEntry) -> bool,
) -> NamespaceWalkStats {
    visit_restartable_with_root_policy_and_signature_capture(
        target,
        max_depth,
        root_policy,
        signature_capture,
        ignore,
        |event| match event {
            NamespaceVisit::Entry(entry) => visitor(entry),
            NamespaceVisit::Restart(_) => true,
        },
    )
}

pub(crate) fn visit_restartable_with_root_policy_and_signature_capture(
    target: &Path,
    max_depth: Option<usize>,
    root_policy: NamespaceRootPolicy,
    signature_capture: NamespaceSignatureCapture,
    ignore: impl Fn(&Path) -> bool,
    mut visitor: impl FnMut(NamespaceVisit<'_>) -> bool,
) -> NamespaceWalkStats {
    let requested =
        std::env::var("MISTER_LIBRARY_NAMESPACE_BACKEND").unwrap_or_else(|_| "auto".to_string());
    if requested == "walkdir" {
        let mut entry_visitor = |entry: &NamespaceEntry| visitor(NamespaceVisit::Entry(entry));
        return visit_walkdir(
            target,
            max_depth,
            root_policy,
            signature_capture,
            &ignore,
            &mut entry_visitor,
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
                    if !visitor(NamespaceVisit::Entry(entry)) {
                        break;
                    }
                }
                return capture.stats;
            }
            Err(reason) => {
                let mut entry_visitor =
                    |entry: &NamespaceEntry| visitor(NamespaceVisit::Entry(entry));
                return visit_walkdir(
                    target,
                    max_depth,
                    root_policy,
                    signature_capture,
                    &ignore,
                    &mut entry_visitor,
                    Some(reason),
                );
            }
        }
    }

    #[cfg(target_os = "linux")]
    if requested == "fd-relative-stream" {
        match linux::visit_fd_relative_stream(
            target,
            max_depth,
            root_policy,
            signature_capture,
            &ignore,
            &mut |entry| visitor(NamespaceVisit::Entry(entry)),
        ) {
            Ok(stats) => return stats,
            Err(mut failure) => {
                failure.stats.fallback_reason = Some(failure.reason.clone());
                if !visitor(NamespaceVisit::Restart(&failure.reason)) {
                    return failure.stats;
                }
                let mut entry_visitor =
                    |entry: &NamespaceEntry| visitor(NamespaceVisit::Entry(entry));
                let mut fallback = visit_walkdir(
                    target,
                    max_depth,
                    root_policy,
                    signature_capture,
                    &ignore,
                    &mut entry_visitor,
                    Some(failure.reason),
                );
                fallback.add(&failure.stats);
                fallback.backend = "walkdir-fallback";
                return fallback;
            }
        }
    }

    if requested == "auto" {
        let mut entry_visitor = |entry: &NamespaceEntry| visitor(NamespaceVisit::Entry(entry));
        return visit_walkdir(
            target,
            max_depth,
            root_policy,
            signature_capture,
            &ignore,
            &mut entry_visitor,
            None,
        );
    }

    let reason = if requested == "fd-relative" || requested == "fd-relative-stream" {
        Some("fd-relative backend unsupported on this operating system".to_string())
    } else {
        Some(format!("unknown namespace backend {requested:?}"))
    };
    let mut entry_visitor = |entry: &NamespaceEntry| visitor(NamespaceVisit::Entry(entry));
    visit_walkdir(
        target,
        max_depth,
        root_policy,
        signature_capture,
        &ignore,
        &mut entry_visitor,
        reason,
    )
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
    for entry in builder
        .into_iter()
        .filter_entry(|entry| !ignore(entry.path()))
        .filter_map(Result::ok)
    {
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
        dir_opens: 0,
        read_calls: 0,
        read_bytes: 0,
        type_stats: 0,
        captured_entries: visited_entries,
        peak_buffered_entries: usize::from(visited_entries > 0),
        peak_buffered_bytes,
        buffer_allocations: 0,
        fallback_count: usize::from(restarted),
        restart_count: usize::from(restarted),
        first_entry_us,
        final_entry_us,
        target_signature,
    }
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
    use super::{
        DirectorySignatureProbe, NamespaceEntry, NamespaceEntryKind, NamespaceRootPolicy,
        NamespaceSignatureCapture, NamespaceWalkStats, is_zip_path, unix_timestamp_nanos,
    };
    use std::ffi::{CString, OsString};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    const GETDENTS_BUFFER_BYTES: usize = 128 * 1024;
    const DIRENT64_HEADER_BYTES: usize = 19;
    const MAX_CAPTURED_ENTRIES: usize = 65_536;
    const MAX_CAPTURED_PATH_BYTES: usize = 16 * 1024 * 1024;
    const MAX_OPEN_DIRECTORY_FDS: usize = 64;
    pub(super) const MAX_STREAM_PENDING_ENTRIES: usize = 8_192;
    pub(super) const MAX_STREAM_PENDING_BYTES: usize = 2 * 1024 * 1024;
    const INITIAL_STREAM_TASK_CAPACITY: usize = 512;

    pub(super) struct NamespaceCapture {
        pub(super) entries: Vec<NamespaceEntry>,
        pub(super) stats: NamespaceWalkStats,
    }

    pub(super) struct StreamFailure {
        pub(super) reason: String,
        pub(super) stats: NamespaceWalkStats,
    }

    #[derive(Clone, Copy)]
    struct CaptureBudget {
        max_entries: usize,
        max_path_bytes: usize,
        max_open_fds: usize,
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
    ) -> Result<NamespaceCapture, String> {
        collect_fd_relative_with_budget(
            target,
            max_depth,
            root_policy,
            signature_capture,
            ignore,
            CaptureBudget::default(),
        )
    }

    pub(super) fn visit_fd_relative_stream(
        target: &Path,
        max_depth: Option<usize>,
        root_policy: NamespaceRootPolicy,
        signature_capture: NamespaceSignatureCapture,
        ignore: &dyn Fn(&Path) -> bool,
        visitor: &mut dyn FnMut(&NamespaceEntry) -> bool,
    ) -> Result<NamespaceWalkStats, StreamFailure> {
        visit_fd_relative_stream_with_failure(
            target,
            max_depth,
            root_policy,
            signature_capture,
            ignore,
            visitor,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn visit_fd_relative_stream_with_failure(
        target: &Path,
        max_depth: Option<usize>,
        root_policy: NamespaceRootPolicy,
        signature_capture: NamespaceSignatureCapture,
        ignore: &dyn Fn(&Path) -> bool,
        visitor: &mut dyn FnMut(&NamespaceEntry) -> bool,
        fail_after_entries: Option<usize>,
    ) -> Result<NamespaceWalkStats, StreamFailure> {
        let started = std::time::Instant::now();
        let mut stats = NamespaceWalkStats {
            backend: "fd-relative-stream",
            fallback_reason: None,
            dir_opens: 0,
            read_calls: 0,
            read_bytes: 0,
            type_stats: 0,
            captured_entries: 0,
            peak_buffered_entries: 0,
            peak_buffered_bytes: 0,
            buffer_allocations: 0,
            fallback_count: 0,
            restart_count: 0,
            first_entry_us: None,
            final_entry_us: None,
            target_signature: None,
        };
        if max_depth == Some(0) || ignore(target) {
            return Ok(stats);
        }
        let target_name = match c_string(target.as_os_str().as_bytes(), "target path") {
            Ok(target_name) => target_name,
            Err(reason) => return Err(StreamFailure { reason, stats }),
        };
        let no_follow = if root_policy == NamespaceRootPolicy::NoFollow {
            libc::O_NOFOLLOW
        } else {
            0
        };
        let raw_fd = unsafe {
            libc::open(
                target_name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | no_follow,
            )
        };
        if raw_fd < 0 {
            return Err(StreamFailure {
                reason: format!(
                    "open target {}: {}",
                    target.display(),
                    io::Error::last_os_error()
                ),
                stats,
            });
        }
        let root = Rc::new(unsafe { OwnedFd::from_raw_fd(raw_fd) });
        stats.dir_opens = 1;
        let target_signature_before = if signature_capture.target() {
            stat_fd(root.as_raw_fd()).ok().and_then(directory_signature)
        } else {
            None
        };
        let mut buffer = Vec::new();
        if let Err(error) = buffer.try_reserve_exact(GETDENTS_BUFFER_BYTES) {
            return Err(StreamFailure {
                reason: format!("stream allocation: getdents buffer: {error}"),
                stats,
            });
        }
        buffer.resize(GETDENTS_BUFFER_BYTES, 0u8);
        stats.buffer_allocations = 1;
        if let Err(reason) = stream_namespace(
            Rc::clone(&root),
            target.to_path_buf(),
            max_depth,
            signature_capture,
            ignore,
            visitor,
            &mut buffer,
            &mut stats,
            started,
            fail_after_entries,
        ) {
            return Err(StreamFailure { reason, stats });
        }
        if signature_capture.target() {
            let target_signature_after =
                stat_fd(root.as_raw_fd()).ok().and_then(directory_signature);
            stats.target_signature =
                super::stable_directory_signature(target_signature_before, target_signature_after);
        }
        Ok(stats)
    }

    struct PendingStreamEntry {
        parent: Rc<OwnedFd>,
        name: CString,
        path: PathBuf,
        d_type: u8,
        depth: usize,
        buffered_bytes: usize,
    }

    enum StreamTask {
        ReadDirectory {
            directory: Rc<OwnedFd>,
            path: PathBuf,
            depth: usize,
        },
        ProcessEntry(PendingStreamEntry),
        FinishDirectory {
            directory: Rc<OwnedFd>,
            entry: NamespaceEntry,
            signature_before: Option<(u64, i64)>,
            buffered_bytes: usize,
        },
    }

    #[allow(clippy::too_many_arguments)]
    fn stream_namespace(
        root: Rc<OwnedFd>,
        root_path: PathBuf,
        max_depth: Option<usize>,
        signature_capture: NamespaceSignatureCapture,
        ignore: &dyn Fn(&Path) -> bool,
        visitor: &mut dyn FnMut(&NamespaceEntry) -> bool,
        buffer: &mut [u8],
        stats: &mut NamespaceWalkStats,
        started: std::time::Instant,
        fail_after_entries: Option<usize>,
    ) -> Result<(), String> {
        let mut tasks = Vec::new();
        tasks
            .try_reserve_exact(INITIAL_STREAM_TASK_CAPACITY)
            .map_err(|error| format!("stream allocation: task stack: {error}"))?;
        let mut batch = Vec::new();
        batch
            .try_reserve_exact(INITIAL_STREAM_TASK_CAPACITY)
            .map_err(|error| format!("stream allocation: directory batch: {error}"))?;
        stats.buffer_allocations = stats.buffer_allocations.saturating_add(2);
        push_stream_task(
            &mut tasks,
            StreamTask::ReadDirectory {
                directory: root,
                path: root_path,
                depth: 0,
            },
            stats,
        )?;
        let mut pending_entries = 0usize;
        let mut pending_bytes = 0usize;

        while let Some(task) = tasks.pop() {
            crate::cooperative_work::checkpoint();
            match task {
                StreamTask::ReadDirectory {
                    directory,
                    path,
                    depth,
                } => {
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
                        return Err(format!(
                            "getdents64 {}: {}",
                            path.display(),
                            io::Error::last_os_error()
                        ));
                    }
                    if read == 0 {
                        continue;
                    }
                    let read = usize::try_from(read).map_err(|_| "negative getdents64 size")?;
                    stats.read_bytes = stats.read_bytes.saturating_add(read as u64);
                    batch.clear();
                    let mut batch_buffered_bytes = 0usize;
                    let mut offset = 0usize;
                    while offset < read {
                        if read - offset < DIRENT64_HEADER_BYTES {
                            return Err(format!(
                                "truncated getdents64 record in {}",
                                path.display()
                            ));
                        }
                        let record_len =
                            u16::from_ne_bytes([buffer[offset + 16], buffer[offset + 17]]) as usize;
                        if record_len <= DIRENT64_HEADER_BYTES || offset + record_len > read {
                            return Err(format!("invalid getdents64 record in {}", path.display()));
                        }
                        let record = &buffer[offset..offset + record_len];
                        offset += record_len;
                        let name_region = &record[DIRENT64_HEADER_BYTES..];
                        let name_len =
                            name_region
                                .iter()
                                .position(|byte| *byte == 0)
                                .ok_or_else(|| {
                                    format!("unterminated getdents64 name in {}", path.display())
                                })?;
                        let name = &name_region[..name_len];
                        if name.is_empty() || name == b"." || name == b".." {
                            continue;
                        }
                        let child_path = path.join(OsString::from_vec(name.to_vec()));
                        if ignore(&child_path) {
                            continue;
                        }
                        let buffered_bytes = child_path
                            .as_os_str()
                            .len()
                            .saturating_add(name.len())
                            .saturating_add(std::mem::size_of::<PendingStreamEntry>());
                        if pending_entries.saturating_add(batch.len()) >= MAX_STREAM_PENDING_ENTRIES
                            || pending_bytes
                                .saturating_add(batch_buffered_bytes)
                                .saturating_add(buffered_bytes)
                                > MAX_STREAM_PENDING_BYTES
                        {
                            return Err(format!(
                                "stream budget: pending namespace batch exceeds {} entries or {} bytes under {}",
                                MAX_STREAM_PENDING_ENTRIES,
                                MAX_STREAM_PENDING_BYTES,
                                path.display()
                            ));
                        }
                        let previous_capacity = batch.capacity();
                        batch.try_reserve(1).map_err(|error| {
                            format!("stream allocation: directory entry: {error}")
                        })?;
                        if batch.capacity() != previous_capacity {
                            stats.buffer_allocations = stats.buffer_allocations.saturating_add(1);
                        }
                        batch.push(PendingStreamEntry {
                            parent: Rc::clone(&directory),
                            name: c_string(name, "directory entry name")?,
                            path: child_path,
                            d_type: record[18],
                            depth: depth.saturating_add(1),
                            buffered_bytes,
                        });
                        batch_buffered_bytes = batch_buffered_bytes.saturating_add(buffered_bytes);
                    }
                    pending_entries = pending_entries.saturating_add(batch.len());
                    pending_bytes = pending_bytes.saturating_add(batch_buffered_bytes);
                    stats.peak_buffered_entries = stats.peak_buffered_entries.max(pending_entries);
                    stats.peak_buffered_bytes = stats.peak_buffered_bytes.max(pending_bytes);
                    push_stream_task(
                        &mut tasks,
                        StreamTask::ReadDirectory {
                            directory,
                            path,
                            depth,
                        },
                        stats,
                    )?;
                    for entry in batch.drain(..).rev() {
                        push_stream_task(&mut tasks, StreamTask::ProcessEntry(entry), stats)?;
                    }
                }
                StreamTask::ProcessEntry(pending) => {
                    pending_entries = pending_entries.saturating_sub(1);
                    pending_bytes = pending_bytes.saturating_sub(pending.buffered_bytes);
                    if fail_after_entries.is_some_and(|limit| stats.captured_entries >= limit) {
                        return Err(format!(
                            "injected streaming failure after {} entries",
                            stats.captured_entries
                        ));
                    }
                    let mut stat = None;
                    let kind = match pending.d_type {
                        libc::DT_DIR => NamespaceEntryKind::Directory,
                        libc::DT_REG => NamespaceEntryKind::File,
                        libc::DT_UNKNOWN => {
                            stats.type_stats = stats.type_stats.saturating_add(1);
                            let value = stat_entry(
                                pending.parent.as_raw_fd(),
                                &pending.name,
                                &pending.path,
                            )?;
                            let kind = kind_from_mode(value.st_mode);
                            stat = Some(value);
                            kind
                        }
                        _ => NamespaceEntryKind::Other,
                    };
                    let zip_signature = if kind == NamespaceEntryKind::File
                        && is_zip_path(&pending.path)
                    {
                        let value = match stat {
                            Some(value) => value,
                            None => stat_entry(
                                pending.parent.as_raw_fd(),
                                &pending.name,
                                &pending.path,
                            )?,
                        };
                        Some((
                            u64::try_from(value.st_size).unwrap_or(0),
                            #[allow(clippy::unnecessary_cast)]
                            unix_timestamp_nanos(value.st_mtime as i64, value.st_mtime_nsec as i64),
                        ))
                    } else {
                        None
                    };
                    let should_descend = kind == NamespaceEntryKind::Directory
                        && max_depth.is_none_or(|limit| pending.depth < limit);
                    let capture_directory_signature = kind == NamespaceEntryKind::Directory
                        && pending.depth == 1
                        && signature_capture.depth_one_directories();
                    let mut entry = NamespaceEntry {
                        path: pending.path.clone(),
                        kind,
                        zip_signature,
                        directory_signature: None,
                    };
                    if should_descend {
                        if pending.depth >= MAX_OPEN_DIRECTORY_FDS {
                            return Err(format!(
                                "stream budget: more than {MAX_OPEN_DIRECTORY_FDS} open directory fds under {}",
                                pending.path.display()
                            ));
                        }
                        let raw_child = unsafe {
                            libc::openat(
                                pending.parent.as_raw_fd(),
                                pending.name.as_ptr(),
                                libc::O_RDONLY
                                    | libc::O_DIRECTORY
                                    | libc::O_CLOEXEC
                                    | libc::O_NOFOLLOW,
                            )
                        };
                        if raw_child < 0 {
                            return Err(format!(
                                "openat directory {}: {}",
                                pending.path.display(),
                                io::Error::last_os_error()
                            ));
                        }
                        stats.dir_opens = stats.dir_opens.saturating_add(1);
                        let child = Rc::new(unsafe { OwnedFd::from_raw_fd(raw_child) });
                        if capture_directory_signature {
                            let signature_before = stat_fd(child.as_raw_fd())
                                .ok()
                                .and_then(directory_signature);
                            pending_entries = pending_entries.saturating_add(1);
                            pending_bytes = pending_bytes.saturating_add(pending.buffered_bytes);
                            stats.peak_buffered_entries =
                                stats.peak_buffered_entries.max(pending_entries);
                            stats.peak_buffered_bytes =
                                stats.peak_buffered_bytes.max(pending_bytes);
                            push_stream_task(
                                &mut tasks,
                                StreamTask::FinishDirectory {
                                    directory: Rc::clone(&child),
                                    entry,
                                    signature_before,
                                    buffered_bytes: pending.buffered_bytes,
                                },
                                stats,
                            )?;
                        } else if !publish_stream_entry(&entry, visitor, stats, started) {
                            return Ok(());
                        }
                        push_stream_task(
                            &mut tasks,
                            StreamTask::ReadDirectory {
                                directory: child,
                                path: pending.path,
                                depth: pending.depth,
                            },
                            stats,
                        )?;
                    } else {
                        if capture_directory_signature {
                            let value = match stat {
                                Some(value) => Some(value),
                                None => stat_entry(
                                    pending.parent.as_raw_fd(),
                                    &pending.name,
                                    &pending.path,
                                )
                                .ok(),
                            };
                            entry.directory_signature = value.and_then(directory_signature);
                        }
                        if !publish_stream_entry(&entry, visitor, stats, started) {
                            return Ok(());
                        }
                    }
                }
                StreamTask::FinishDirectory {
                    directory,
                    mut entry,
                    signature_before,
                    buffered_bytes,
                } => {
                    pending_entries = pending_entries.saturating_sub(1);
                    pending_bytes = pending_bytes.saturating_sub(buffered_bytes);
                    let signature_after = stat_fd(directory.as_raw_fd())
                        .ok()
                        .and_then(directory_signature);
                    entry.directory_signature =
                        super::stable_directory_signature(signature_before, signature_after);
                    if !publish_stream_entry(&entry, visitor, stats, started) {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    fn push_stream_task(
        tasks: &mut Vec<StreamTask>,
        task: StreamTask,
        stats: &mut NamespaceWalkStats,
    ) -> Result<(), String> {
        let previous_capacity = tasks.capacity();
        tasks
            .try_reserve(1)
            .map_err(|error| format!("stream allocation: task entry: {error}"))?;
        if tasks.capacity() != previous_capacity {
            stats.buffer_allocations = stats.buffer_allocations.saturating_add(1);
        }
        tasks.push(task);
        Ok(())
    }

    fn publish_stream_entry(
        entry: &NamespaceEntry,
        visitor: &mut dyn FnMut(&NamespaceEntry) -> bool,
        stats: &mut NamespaceWalkStats,
        started: std::time::Instant,
    ) -> bool {
        stats.captured_entries = stats.captured_entries.saturating_add(1);
        stats.peak_buffered_entries = 1;
        stats.peak_buffered_bytes = stats.peak_buffered_bytes.max(
            std::mem::size_of::<NamespaceEntry>().saturating_add(entry.path.as_os_str().len()),
        );
        let elapsed_us = started.elapsed().as_micros() as u64;
        stats.first_entry_us.get_or_insert(elapsed_us);
        stats.final_entry_us = Some(elapsed_us);
        visitor(entry)
    }

    pub(super) fn probe_directory_signatures(
        target: &Path,
        child_paths: &[std::path::PathBuf],
    ) -> DirectorySignatureProbe {
        let Ok(target_name) = c_string(target.as_os_str().as_bytes(), "target path") else {
            return unavailable_probe(child_paths.len());
        };
        let raw_fd = unsafe {
            libc::open(
                target_name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
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
    ) -> Result<NamespaceCapture, String> {
        if max_depth == Some(0) || ignore(target) {
            return Ok(NamespaceCapture {
                entries: Vec::new(),
                stats: NamespaceWalkStats {
                    backend: "fd-relative",
                    fallback_reason: None,
                    dir_opens: 0,
                    read_calls: 0,
                    read_bytes: 0,
                    type_stats: 0,
                    captured_entries: 0,
                    peak_buffered_entries: 0,
                    peak_buffered_bytes: 0,
                    buffer_allocations: 0,
                    fallback_count: 0,
                    restart_count: 0,
                    first_entry_us: None,
                    final_entry_us: None,
                    target_signature: None,
                },
            });
        }
        if budget.max_open_fds == 0 {
            return Err("capture budget: zero open directory fds".to_string());
        }
        let target_name = c_string(target.as_os_str().as_bytes(), "target path")?;
        let no_follow = if root_policy == NamespaceRootPolicy::NoFollow {
            libc::O_NOFOLLOW
        } else {
            0
        };
        let raw_fd = unsafe {
            libc::open(
                target_name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | no_follow,
            )
        };
        if raw_fd < 0 {
            return Err(format!(
                "open target {}: {}",
                target.display(),
                io::Error::last_os_error()
            ));
        }
        let root = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        let target_signature_before = if signature_capture.target() {
            stat_fd(root.as_raw_fd()).ok().and_then(directory_signature)
        } else {
            None
        };
        let mut entries = Vec::new();
        let mut captured_path_bytes = 0usize;
        let mut stats = NamespaceWalkStats {
            backend: "fd-relative",
            fallback_reason: None,
            dir_opens: 1,
            read_calls: 0,
            read_bytes: 0,
            type_stats: 0,
            captured_entries: 0,
            peak_buffered_entries: 0,
            peak_buffered_bytes: 0,
            buffer_allocations: 0,
            fallback_count: 0,
            restart_count: 0,
            first_entry_us: None,
            final_entry_us: None,
            target_signature: None,
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
        )?;
        if signature_capture.target() {
            let target_signature_after =
                stat_fd(root.as_raw_fd()).ok().and_then(directory_signature);
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
    ) -> Result<(), String> {
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(GETDENTS_BUFFER_BYTES)
            .map_err(|error| format!("capture allocation: getdents buffer: {error}"))?;
        buffer.resize(GETDENTS_BUFFER_BYTES, 0u8);
        stats.buffer_allocations = stats.buffer_allocations.saturating_add(1);
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
                return Err(format!(
                    "getdents64 {}: {}",
                    directory_path.display(),
                    io::Error::last_os_error()
                ));
            }
            if read == 0 {
                return Ok(());
            }
            let read = usize::try_from(read).map_err(|_| "negative getdents64 size")?;
            stats.read_bytes = stats.read_bytes.saturating_add(read as u64);
            let mut offset = 0usize;
            while offset < read {
                if entries.len().is_multiple_of(16) {
                    crate::cooperative_work::checkpoint();
                }
                if read - offset < DIRENT64_HEADER_BYTES {
                    return Err(format!(
                        "truncated getdents64 record in {}",
                        directory_path.display()
                    ));
                }
                let record_len =
                    u16::from_ne_bytes([buffer[offset + 16], buffer[offset + 17]]) as usize;
                if record_len <= DIRENT64_HEADER_BYTES || offset + record_len > read {
                    return Err(format!(
                        "invalid getdents64 record in {}",
                        directory_path.display()
                    ));
                }
                let record = &buffer[offset..offset + record_len];
                offset += record_len;
                let name_region = &record[DIRENT64_HEADER_BYTES..];
                let name_len = name_region
                    .iter()
                    .position(|byte| *byte == 0)
                    .ok_or_else(|| {
                        format!(
                            "unterminated getdents64 name in {}",
                            directory_path.display()
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
                    return Err(format!(
                        "capture budget: more than {} entries under {}",
                        budget.max_entries,
                        directory_path.display()
                    ));
                }
                let child_path_bytes = child_path.as_os_str().as_bytes().len();
                let next_path_bytes = captured_path_bytes
                    .checked_add(child_path_bytes)
                    .filter(|total| *total <= budget.max_path_bytes)
                    .ok_or_else(|| {
                        format!(
                            "capture budget: more than {} path bytes under {}",
                            budget.max_path_bytes,
                            directory_path.display()
                        )
                    })?;
                let previous_capacity = entries.capacity();
                entries
                    .try_reserve(1)
                    .map_err(|error| format!("capture allocation: namespace entry: {error}"))?;
                if entries.capacity() != previous_capacity {
                    stats.buffer_allocations = stats.buffer_allocations.saturating_add(1);
                }
                let child_name = c_string(name, "directory entry name")?;
                let entry_depth = depth.saturating_add(1);
                let mut stat = None;
                let kind = match record[18] {
                    libc::DT_DIR => NamespaceEntryKind::Directory,
                    libc::DT_REG => NamespaceEntryKind::File,
                    libc::DT_UNKNOWN => {
                        stats.type_stats = stats.type_stats.saturating_add(1);
                        let value = stat_entry(directory.as_raw_fd(), &child_name, &child_path)?;
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
                        None => stat_entry(directory.as_raw_fd(), &child_name, &child_path)?,
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
                    && entry_depth == 1
                    && signature_capture.depth_one_directories();
                let mut opened_child = None;
                let mut child_signature_before = None;
                if should_descend {
                    if entry_depth >= budget.max_open_fds {
                        return Err(format!(
                            "capture budget: more than {} open directory fds under {}",
                            budget.max_open_fds,
                            child_path.display()
                        ));
                    }
                    let raw_child = unsafe {
                        libc::openat(
                            directory.as_raw_fd(),
                            child_name.as_ptr(),
                            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                        )
                    };
                    if raw_child < 0 {
                        return Err(format!(
                            "openat directory {}: {}",
                            child_path.display(),
                            io::Error::last_os_error()
                        ));
                    }
                    stats.dir_opens = stats.dir_opens.saturating_add(1);
                    opened_child = Some(unsafe { OwnedFd::from_raw_fd(raw_child) });
                    if capture_directory_signature {
                        child_signature_before = opened_child
                            .as_ref()
                            .and_then(|child| stat_fd(child.as_raw_fd()).ok())
                            .and_then(directory_signature);
                    }
                }
                let captured_directory_signature = if capture_directory_signature && !should_descend
                {
                    let value = match stat {
                        Some(value) => Some(value),
                        None => stat_entry(directory.as_raw_fd(), &child_name, &child_path).ok(),
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
                    collect_directory(
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
                    )?;
                    if capture_directory_signature {
                        let child_signature_after = stat_fd(child.as_raw_fd())
                            .ok()
                            .and_then(directory_signature);
                        entries[entry_index].directory_signature =
                            super::stable_directory_signature(
                                child_signature_before,
                                child_signature_after,
                            );
                    }
                }
            }
        }
    }

    fn c_string(bytes: &[u8], description: &str) -> Result<CString, String> {
        CString::new(bytes).map_err(|_| format!("NUL in {description}"))
    }

    fn stat_entry(directory: RawFd, name: &CString, path: &Path) -> Result<libc::stat, String> {
        let mut value = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                directory,
                name.as_ptr(),
                value.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            return Err(format!(
                "fstatat {}: {}",
                path.display(),
                io::Error::last_os_error()
            ));
        }
        Ok(unsafe { value.assume_init() })
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
        )
    }

    #[cfg(test)]
    pub(super) fn visit_stream_with_failure_for_test(
        target: &Path,
        fail_after_entries: usize,
        visitor: &mut dyn FnMut(&NamespaceEntry) -> bool,
    ) -> Result<NamespaceWalkStats, StreamFailure> {
        visit_fd_relative_stream_with_failure(
            target,
            None,
            NamespaceRootPolicy::NoFollow,
            NamespaceSignatureCapture::None,
            &|_| false,
            visitor,
            Some(fail_after_entries),
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
    fn stream_snapshot(
        root: &Path,
        max_depth: Option<usize>,
        ignore: &dyn Fn(&Path) -> bool,
    ) -> (Snapshot, NamespaceWalkStats) {
        let mut entries = Vec::new();
        let stats = linux::visit_fd_relative_stream(
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
        )
        .unwrap();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        (entries, stats)
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
            let (streamed, stats) = stream_snapshot(&dir, max_depth, &ignore);
            assert_eq!(streamed, walkdir);
            assert!(stats.buffer_allocations <= 4);
            assert!(stats.peak_buffered_entries <= linux::MAX_STREAM_PENDING_ENTRIES);
            assert!(stats.peak_buffered_bytes <= linux::MAX_STREAM_PENDING_BYTES);
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
        .err()
        .expect("the deterministic entry budget must fail mid-target");
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

    #[cfg(target_os = "linux")]
    #[test]
    fn streaming_failure_exposes_partial_work_before_identical_retry() {
        let dir = unique_temp_dir("namespace-stream-restart");
        fs::create_dir_all(dir.join("nested/deep")).unwrap();
        fs::write(dir.join("one.rom"), b"one").unwrap();
        fs::write(dir.join("nested/two.rom"), b"two").unwrap();
        fs::write(dir.join("nested/deep/three.rom"), b"three").unwrap();

        let expected = walkdir_snapshot(&dir, None, &|_| false).0;
        let mut partial = Vec::new();
        let failure = linux::visit_stream_with_failure_for_test(&dir, 2, &mut |entry| {
            partial.push(entry.path.clone());
            true
        })
        .err()
        .expect("the deterministic streaming fault must interrupt the target");
        assert_eq!(partial.len(), 2);
        assert_eq!(failure.stats.backend, "fd-relative-stream");
        assert_eq!(failure.stats.buffer_allocations, 3);
        assert!(failure.stats.peak_buffered_entries <= linux::MAX_STREAM_PENDING_ENTRIES);
        assert!(failure.stats.peak_buffered_bytes <= linux::MAX_STREAM_PENDING_BYTES);

        let mut restarted = Vec::new();
        let retry = visit_walkdir(
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
            Some(failure.reason),
        );
        restarted.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(restarted, expected);
        assert_eq!(retry.fallback_count, 1);
        assert_eq!(retry.restart_count, 1);

        fs::remove_dir_all(dir).unwrap();
    }
}
