// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::cpu_profile::CatalogBuildProfiler;
use crate::preview_state::SystemEntryPreviewPrelude;
use mister_magik_catalog::arcade_catalog::ArcadeCatalog;
use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const CATALOG_WORKER_CHILD_ENV: &str = "MISTER_CATALOG_WORKER_CHILD";
const CATALOG_WORKER_PROTOCOL_PREFIX: &str = "MISTER_CATALOG_EVENT ";
const CATALOG_WORKER_PROTOCOL_VERSION: u8 = 4;
const MAX_CATALOG_WORKER_PROTOCOL_LINE_BYTES: u64 = 256 * 1024;
const CATALOG_WORKER_EVENT_QUEUE_CAPACITY: usize = 16_384;
const CATALOG_WORKER_SNAPSHOT_DIRECTORY: &str = "/tmp/mister-magik/catalog-worker";

fn filesystem_available_bytes(path: &str) -> Option<u64> {
    let path = CString::new(path).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    (stats.f_bavail as u64).checked_mul(stats.f_frsize as u64)
}

fn report_catalog_filesystem_headroom(tx: &mpsc::Sender<CatalogWorkerMessage>, phase: &str) {
    let tmp_available_bytes = filesystem_available_bytes("/tmp/mister-magik");
    let media_available_bytes = filesystem_available_bytes("/media/fat");
    let format_bytes = |value: Option<u64>| {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    };
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "catalog_filesystem_headroom".to_string(),
        detail: format!(
            "phase={phase} tmp_available_bytes={} media_available_bytes={}",
            format_bytes(tmp_available_bytes),
            format_bytes(media_available_bytes),
        ),
    });
}

fn send_ready_catalog(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    catalog: ArcadeCatalog,
    load_us: u64,
    source: CatalogSource,
    durable_save_pending: bool,
    generation_fingerprint: Option<String>,
) {
    let publication_started = Instant::now();
    let publication_source = format!("{source:?}");
    let _ = tx.send(CatalogWorkerMessage::Ready {
        catalog,
        load_us,
        source,
        durable_save_pending,
        generation_fingerprint,
        publication_ack: None,
    });
    crate::ui_logln!(
        "catalog_publication_dispatched_tsv\tsource={}\telapsed_us={}\tdurable_save_pending={}",
        publication_source,
        publication_started.elapsed().as_micros(),
        durable_save_pending as u8,
    );
}

fn publish_strict_registry_seed_at(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    root: &str,
    storage: &Path,
) -> Result<(), String> {
    let load_started = Instant::now();
    match load_sharded_registry_seed_at(root, storage) {
        Ok(seed) => {
            let load_us = load_started.elapsed().as_micros() as u64;
            let fingerprint = seed.catalog_fingerprint.clone();
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_strict_registry_load".to_string(),
                detail: format!(
                    "status=ready load_us={load_us} generation={} systems={} resident_games={}",
                    seed.generation,
                    seed.catalog.systems.len(),
                    seed.catalog.games.len()
                ),
            });
            send_ready_catalog(
                tx,
                seed.catalog,
                load_us,
                CatalogSource::ShardedRegistry,
                false,
                Some(fingerprint),
            );
            Ok(())
        }
        Err(error) => {
            let detail = format!(
                "status={} load_us={} error={}",
                error.status,
                load_started.elapsed().as_micros(),
                error.to_string().replace('\t', " ")
            );
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_strict_registry_load".to_string(),
                detail,
            });
            Err(error.to_string())
        }
    }
}

fn publish_registry_ready_at(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    root: &str,
    storage: &Path,
) -> Result<(), String> {
    if std::env::var_os(CATALOG_WORKER_CHILD_ENV).is_none() {
        return publish_strict_registry_seed_at(tx, root, storage);
    }
    let manifest = mister_magik_catalog::shard_registry::read_latest_manifest_lazy(
        storage,
        mister_magik_catalog::shard_registry::production_registry_limits(),
    )
    .map_err(|error| format!("read published catalog manifest: {error}"))?;
    let fingerprint =
        mister_magik_catalog::fast_five_catalog::registry_fingerprint_for_manifest(&manifest);
    tx.send(CatalogWorkerMessage::PublishedRegistryReady {
        generation: manifest.generation,
        fingerprint,
    })
    .map_err(|_| "catalog worker receiver closed before registry publication".to_string())
}

pub(super) fn catalog_refresh_available() -> bool {
    true
}

pub(super) struct CatalogChildControl {
    child: Mutex<Option<Child>>,
    process_group: i32,
    handshake_seen: AtomicBool,
    reaped: AtomicBool,
}

impl CatalogChildControl {
    pub(super) fn reaped(&self) -> bool {
        self.reaped.load(Ordering::Acquire)
    }

    pub(super) fn terminate(&self) -> bool {
        #[cfg(unix)]
        let group_result = if self.process_group > 0 {
            // SAFETY: the process group id was created for this child with
            // setpgid(0, 0); a negative id targets only that group.
            unsafe { libc::kill(-self.process_group, libc::SIGKILL) }
        } else {
            -1
        };
        #[cfg(not(unix))]
        let group_result = -1;
        if group_result == 0 {
            return true;
        }
        let Ok(mut child) = self.child.try_lock() else {
            return false;
        };
        child.as_mut().is_some_and(|child| child.kill().is_ok())
    }

    #[cfg(test)]
    pub(super) fn test_unreaped() -> Self {
        Self {
            child: Mutex::new(None),
            process_group: -1,
            handshake_seen: AtomicBool::new(true),
            reaped: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    pub(super) fn mark_reaped_for_test(&self) {
        self.reaped.store(true, Ordering::Release);
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
struct CatalogWorkerWireEvent {
    version: u8,
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    system_id: String,
    #[serde(default)]
    system_ids: Vec<String>,
    #[serde(default)]
    all_published_systems: bool,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    rebuilt: Vec<String>,
    #[serde(default)]
    removed: Vec<String>,
    #[serde(default)]
    elapsed_us: u64,
    #[serde(default)]
    source: String,
    #[serde(default)]
    durable_save_pending: bool,
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    run_id: String,
    #[serde(default)]
    phase: String,
    #[serde(default)]
    sequence: u64,
    #[serde(default)]
    progress_epoch: u64,
    #[serde(default)]
    work_units: u64,
    #[serde(default)]
    snapshot_path: String,
    #[serde(default)]
    snapshot_sha256: String,
}

#[derive(Default)]
struct CatalogWorkerProtocolState {
    run_id: String,
    sequence: u64,
    heartbeat_phase: String,
    progress_epoch: u64,
    work_units: u64,
}

impl CatalogWorkerProtocolState {
    fn validate(&mut self, event: &CatalogWorkerWireEvent) -> Result<bool, String> {
        if event.version != CATALOG_WORKER_PROTOCOL_VERSION {
            return Err(format!(
                "unsupported catalog worker protocol version {}",
                event.version
            ));
        }
        if event.kind == "handshake" {
            if !self.run_id.is_empty() || event.run_id.is_empty() || event.sequence != 0 {
                return Err("invalid catalog worker handshake".to_string());
            }
            self.run_id.clone_from(&event.run_id);
            return Ok(true);
        }
        if self.run_id.is_empty() {
            return Err("catalog worker event arrived before handshake".to_string());
        }
        if event.run_id != self.run_id {
            return Err("catalog worker run id changed".to_string());
        }
        if event.sequence <= self.sequence {
            return Err("catalog worker sequence did not advance".to_string());
        }
        self.sequence = event.sequence;
        if event.kind != "heartbeat" {
            return Ok(false);
        }
        if event.phase.is_empty()
            || event.progress_epoch < self.progress_epoch
            || event.work_units < self.work_units
        {
            return Err("catalog worker heartbeat progress regressed".to_string());
        }
        if !self.heartbeat_phase.is_empty()
            && ((event.phase == self.heartbeat_phase
                && event.progress_epoch != self.progress_epoch)
                || (event.phase != self.heartbeat_phase
                    && event.progress_epoch <= self.progress_epoch))
        {
            return Err("catalog worker heartbeat phase transition is invalid".to_string());
        }
        self.heartbeat_phase.clone_from(&event.phase);
        self.progress_epoch = event.progress_epoch;
        self.work_units = event.work_units;
        Ok(false)
    }
}

#[derive(Clone)]
struct CatalogHeartbeatProgress {
    phase: String,
    progress_epoch: u64,
    work_units: u64,
}

impl CatalogHeartbeatProgress {
    fn new() -> Self {
        Self {
            phase: "starting".to_string(),
            progress_epoch: 0,
            work_units: 0,
        }
    }

    fn advance(&mut self, phase: &str, work_units: u64) {
        if work_units <= self.work_units {
            return;
        }
        if self.phase != phase {
            self.phase = phase.to_string();
            self.progress_epoch = self.progress_epoch.saturating_add(1);
        }
        self.work_units = work_units;
    }
}

fn write_worker_wire_event(writer: &mut impl Write, event: &CatalogWorkerWireEvent) -> bool {
    serde_json::to_writer(&mut *writer, event)
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .is_ok()
}

fn heartbeat_interval_elapsed(stop: &mpsc::Receiver<()>, interval: std::time::Duration) -> bool {
    matches!(
        stop.recv_timeout(interval),
        Err(mpsc::RecvTimeoutError::Timeout)
    )
}

fn snapshot_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn cleanup_catalog_worker_snapshots(root: &Path) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(root)
        .map_err(|error| format!("inspect catalog worker snapshot root: {error}"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err("catalog worker snapshot root is not a directory".to_string());
    }
    for (index, entry) in std::fs::read_dir(root)
        .map_err(|error| format!("read catalog worker snapshot root: {error}"))?
        .enumerate()
    {
        if index >= 1024 {
            return Err("too many stale catalog worker snapshots".to_string());
        }
        let entry = entry.map_err(|error| format!("read catalog worker snapshot: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("inspect catalog worker snapshot: {error}"))?;
        if file_type.is_file()
            && entry
                .file_name()
                .to_string_lossy()
                .ends_with(".arcade-system.bin")
        {
            std::fs::remove_file(entry.path())
                .map_err(|error| format!("remove stale catalog worker snapshot: {error}"))?;
        }
    }
    Ok(())
}

fn write_arcade_system_snapshot(
    run_id: &str,
    system: &mister_magik_catalog::fast_five_catalog::FastFiveSystem,
) -> Result<(String, String), String> {
    write_arcade_system_snapshot_at(Path::new(CATALOG_WORKER_SNAPSHOT_DIRECTORY), run_id, system)
}

fn write_arcade_system_snapshot_at(
    root: &Path,
    run_id: &str,
    system: &mister_magik_catalog::fast_five_catalog::FastFiveSystem,
) -> Result<(String, String), String> {
    cleanup_catalog_worker_snapshots(root)?;
    let bytes = mister_magik_catalog::fast_five_catalog::encode_fast_system_transport(system)?;
    std::fs::create_dir_all(root)
        .map_err(|error| format!("create catalog worker snapshot root: {error}"))?;
    let path = root.join(format!("{run_id}.arcade-system.bin"));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|error| format!("create catalog worker snapshot: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write catalog worker snapshot: {error}"))?;
    std::fs::File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("sync catalog worker snapshot root: {error}"))?;
    Ok((path.to_string_lossy().into_owned(), snapshot_sha256(&bytes)))
}

fn load_arcade_system_snapshot(
    arcade_root: &str,
    event: &CatalogWorkerWireEvent,
) -> Result<ArcadeCatalog, String> {
    load_arcade_system_snapshot_at(
        Path::new(CATALOG_WORKER_SNAPSHOT_DIRECTORY),
        arcade_root,
        event,
    )
}

fn load_arcade_system_snapshot_at(
    snapshot_root: &Path,
    arcade_root: &str,
    event: &CatalogWorkerWireEvent,
) -> Result<ArcadeCatalog, String> {
    let expected = snapshot_root.join(format!("{}.arcade-system.bin", event.run_id));
    if Path::new(&event.snapshot_path) != expected {
        return Err("catalog worker snapshot path does not match its run".to_string());
    }
    let metadata = std::fs::symlink_metadata(&expected)
        .map_err(|error| format!("inspect catalog worker snapshot: {error}"))?;
    if !metadata.file_type().is_file()
        || metadata.len()
            > mister_magik_catalog::fast_five_catalog::MAX_FAST_SYSTEM_TRANSPORT_BYTES as u64
    {
        return Err("catalog worker snapshot is not a bounded regular file".to_string());
    }
    let mut bytes = Vec::with_capacity(metadata.len().try_into().unwrap_or(0));
    std::fs::File::open(&expected)
        .map_err(|error| format!("open catalog worker snapshot: {error}"))?
        .take(mister_magik_catalog::fast_five_catalog::MAX_FAST_SYSTEM_TRANSPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read catalog worker snapshot: {error}"))?;
    let _ = std::fs::remove_file(&expected);
    if bytes.len() > mister_magik_catalog::fast_five_catalog::MAX_FAST_SYSTEM_TRANSPORT_BYTES
        || snapshot_sha256(&bytes) != event.snapshot_sha256
    {
        return Err("catalog worker snapshot checksum or size differs".to_string());
    }
    let system = mister_magik_catalog::fast_five_catalog::decode_fast_system_transport(&bytes)?;
    if system.system_id != "arcade" {
        return Err("catalog worker snapshot is not Arcade".to_string());
    }
    Ok(
        mister_magik_catalog::fast_catalog_sources::launcher_catalog_for_fast_system(
            Path::new(arcade_root),
            &system,
        ),
    )
}

fn worker_wire_event(message: &CatalogWorkerMessage) -> CatalogWorkerWireEvent {
    let mut event = CatalogWorkerWireEvent {
        version: CATALOG_WORKER_PROTOCOL_VERSION,
        kind: String::new(),
        name: String::new(),
        detail: String::new(),
        error: String::new(),
        system_id: String::new(),
        system_ids: Vec::new(),
        all_published_systems: false,
        generation: 0,
        rebuilt: Vec::new(),
        removed: Vec::new(),
        elapsed_us: 0,
        source: String::new(),
        durable_save_pending: false,
        fingerprint: String::new(),
        run_id: String::new(),
        phase: String::new(),
        sequence: 0,
        progress_epoch: 0,
        work_units: 0,
        snapshot_path: String::new(),
        snapshot_sha256: String::new(),
    };
    match message {
        CatalogWorkerMessage::Timing { name, detail } => {
            event.kind = "timing".to_string();
            event.name = name.clone();
            event.detail = detail.clone();
        }
        CatalogWorkerMessage::LoadFailed { error } => {
            event.kind = "load-failed".to_string();
            event.error = error.clone();
        }
        CatalogWorkerMessage::ReconciliationPlanReady {
            system_ids,
            all_published_systems,
        } => {
            event.kind = "plan-ready".to_string();
            event.system_ids = system_ids.clone();
            event.all_published_systems = *all_published_systems;
        }
        CatalogWorkerMessage::SystemScanning { system_id } => {
            event.kind = "system-scanning".to_string();
            event.system_id = system_id.clone();
        }
        CatalogWorkerMessage::SystemPrepared {
            system_id,
            generation,
        } => {
            event.kind = "system-prepared".to_string();
            event.system_id = system_id.clone();
            event.generation = *generation;
        }
        CatalogWorkerMessage::SystemRemoved { system_id } => {
            event.kind = "system-removed".to_string();
            event.system_id = system_id.clone();
        }
        CatalogWorkerMessage::SystemUpdateFailed { system_id, error } => {
            event.kind = "system-update-failed".to_string();
            event.system_id = system_id.clone();
            event.error = error.clone();
        }
        CatalogWorkerMessage::ManifestPublished {
            generation,
            rebuilt,
            removed,
        } => {
            event.kind = "manifest-published".to_string();
            event.generation = *generation;
            event.rebuilt = rebuilt.clone();
            event.removed = removed.clone();
        }
        CatalogWorkerMessage::BuildCompleted { elapsed_us } => {
            event.kind = "build-completed".to_string();
            event.elapsed_us = *elapsed_us;
        }
        CatalogWorkerMessage::HydrationDoneNeedsValidation { root } => {
            event.kind = "hydration-done".to_string();
            event.detail = root.clone();
        }
        CatalogWorkerMessage::Ready {
            source,
            durable_save_pending,
            generation_fingerprint,
            ..
        } => {
            event.kind = "ready".to_string();
            event.source = source.label().to_string();
            event.durable_save_pending = *durable_save_pending;
            event.fingerprint = generation_fingerprint.clone().unwrap_or_default();
        }
        CatalogWorkerMessage::PublishedRegistryReady {
            generation,
            fingerprint,
        } => {
            event.kind = "ready".to_string();
            event.source = CatalogSource::ShardedRegistry.label().to_string();
            event.generation = *generation;
            event.fingerprint.clone_from(fingerprint);
        }
        CatalogWorkerMessage::ArcadeBootstrapReady { .. } => {
            unreachable!("Arcade bootstrap must use the bounded snapshot transport")
        }
        CatalogWorkerMessage::PersistenceFailed { error } => {
            event.kind = "persistence-failed".to_string();
            event.error = error.clone();
        }
        CatalogWorkerMessage::Done => event.kind = "done".to_string(),
        CatalogWorkerMessage::Heartbeat {
            run_id,
            phase,
            sequence,
            progress_epoch,
            work_units,
        } => {
            event.kind = "heartbeat".to_string();
            event.run_id = run_id.clone();
            event.phase = phase.clone();
            event.sequence = *sequence;
            event.progress_epoch = *progress_epoch;
            event.work_units = *work_units;
        }
        CatalogWorkerMessage::Progress { .. } => {
            unreachable!("internal catalog progress must not cross the worker protocol")
        }
        CatalogWorkerMessage::SystemShardReady { .. }
        | CatalogWorkerMessage::SystemShardFailed { .. }
        | CatalogWorkerMessage::SearchQueryReady { .. }
        | CatalogWorkerMessage::SearchQueryFailed { .. } => {
            event.kind = "unsupported".to_string();
        }
    }
    event
}

fn blank_worker_wire_event(kind: &str) -> CatalogWorkerWireEvent {
    CatalogWorkerWireEvent {
        version: CATALOG_WORKER_PROTOCOL_VERSION,
        kind: kind.to_string(),
        name: String::new(),
        detail: String::new(),
        error: String::new(),
        system_id: String::new(),
        system_ids: Vec::new(),
        all_published_systems: false,
        generation: 0,
        rebuilt: Vec::new(),
        removed: Vec::new(),
        elapsed_us: 0,
        source: String::new(),
        durable_save_pending: false,
        fingerprint: String::new(),
        run_id: String::new(),
        phase: String::new(),
        sequence: 0,
        progress_epoch: 0,
        work_units: 0,
        snapshot_path: String::new(),
        snapshot_sha256: String::new(),
    }
}

pub(super) enum CatalogWorkerReceiver {
    Direct(mpsc::Receiver<CatalogWorkerMessage>),
    Process {
        events: mpsc::Receiver<CatalogWorkerMessage>,
        terminal: mpsc::Receiver<CatalogWorkerMessage>,
        pending_terminal: Mutex<Option<CatalogWorkerMessage>>,
    },
}

impl CatalogWorkerReceiver {
    pub(super) fn try_recv(&self) -> Result<CatalogWorkerMessage, mpsc::TryRecvError> {
        match self {
            Self::Direct(receiver) => receiver.try_recv(),
            Self::Process {
                events,
                terminal,
                pending_terminal,
            } => {
                let mut pending = pending_terminal
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if let Some(message) = pending.as_ref()
                    && !matches!(message, CatalogWorkerMessage::Done)
                {
                    return Ok(pending.take().expect("pending terminal message"));
                }
                if pending.is_none()
                    && let Ok(message) = terminal.try_recv()
                {
                    if !matches!(message, CatalogWorkerMessage::Done) {
                        return Ok(message);
                    }
                    *pending = Some(message);
                }
                match events.try_recv() {
                    Ok(message) => Ok(message),
                    Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {
                        pending.take().map(Ok).unwrap_or_else(|| events.try_recv())
                    }
                }
            }
        }
    }
}

impl From<mpsc::Receiver<CatalogWorkerMessage>> for CatalogWorkerReceiver {
    fn from(receiver: mpsc::Receiver<CatalogWorkerMessage>) -> Self {
        Self::Direct(receiver)
    }
}

fn read_catalog_worker_protocol_line(reader: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut bytes = Vec::new();
    let read = reader
        .take(MAX_CATALOG_WORKER_PROTOCOL_LINE_BYTES + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| format!("read catalog worker protocol: {error}"))?;
    if read == 0 {
        return Ok(None);
    }
    if read as u64 > MAX_CATALOG_WORKER_PROTOCOL_LINE_BYTES {
        return Err("catalog worker protocol line exceeds size limit".to_string());
    }
    if bytes.pop() != Some(b'\n') {
        return Err("catalog worker protocol ended with an incomplete line".to_string());
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "catalog worker protocol line is not UTF-8".to_string())
}

fn prepare_catalog_worker_protocol_output() -> Result<Box<dyn Write + Send>, String> {
    #[cfg(unix)]
    {
        // Preserve the piped stdout exclusively for protocol records, then
        // redirect ordinary stdout logging to the inherited diagnostic stream.
        let protocol_fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
        if protocol_fd < 0 {
            return Err(format!(
                "duplicate catalog worker protocol stream: {}",
                std::io::Error::last_os_error()
            ));
        }
        if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
            unsafe { libc::close(protocol_fd) };
            return Err(format!(
                "redirect catalog worker diagnostics: {}",
                std::io::Error::last_os_error()
            ));
        }
        let output = unsafe { std::fs::File::from_raw_fd(protocol_fd) };
        return Ok(Box::new(output));
    }
    #[cfg(not(unix))]
    {
        Ok(Box::new(std::io::stdout()))
    }
}

pub(super) fn start_library_catalog_worker(
    root: String,
    request: CatalogWorkerRequest,
    initial_cache: CatalogWorkerInitialCache,
    execution_mode: CatalogExecutionMode,
    catalog_paths: mister_magik_catalog::device_layout::CatalogPaths,
    archive_cache: mister_magik_catalog::catalog_config::ArchiveCacheConfig,
) -> (CatalogWorkerReceiver, Option<Arc<CatalogChildControl>>) {
    if request != CatalogWorkerRequest::LoadOnly
        && request != CatalogWorkerRequest::StrictLoad
        && std::env::var_os(CATALOG_WORKER_CHILD_ENV).is_none()
    {
        return start_library_catalog_worker_process(
            root,
            request,
            initial_cache,
            execution_mode,
            catalog_paths,
        );
    }
    (
        start_library_catalog_worker_in_process(
            root,
            request,
            initial_cache,
            execution_mode,
            catalog_paths,
            archive_cache,
        )
        .into(),
        None,
    )
}

fn start_library_catalog_worker_in_process(
    root: String,
    request: CatalogWorkerRequest,
    initial_cache: CatalogWorkerInitialCache,
    execution_mode: CatalogExecutionMode,
    catalog_paths: mister_magik_catalog::device_layout::CatalogPaths,
    _archive_cache: mister_magik_catalog::catalog_config::ArchiveCacheConfig,
) -> mpsc::Receiver<CatalogWorkerMessage> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("catalog-refresh".to_string())
        .spawn(move || {
            apply_runtime_thread_policy(execution_mode.thread_role());
            if request == CatalogWorkerRequest::StrictLoad {
                match publish_strict_registry_seed_at(
                    &tx,
                    &root,
                    catalog_paths.sharded_catalog_dir(),
                ) {
                    Ok(()) => {
                        let _ = tx.send(CatalogWorkerMessage::Done);
                    }
                    Err(error) => {
                        let _ = tx.send(CatalogWorkerMessage::LoadFailed { error });
                    }
                }
                return;
            }
            let _mutation_lease =
                match mister_magik_catalog::catalog_lease::CatalogMutationLease::acquire_default() {
                    Ok(lease) => lease,
                    Err(error) => {
                        let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                            error: error.to_string(),
                        });
                        return;
                    }
                };
            if let Err(error) =
                mister_magik_catalog::fast_catalog_refresh::cleanup_refresh_temporary_files(
                    catalog_paths.sharded_catalog_dir(),
                )
            {
                let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                    error: format!("catalog temporary recovery failed: {error}"),
                });
                return;
            }
            let cache_state = match initial_cache {
                CatalogWorkerInitialCache::AlreadyLoadedReady => CatalogCacheState::Ready,
                _ => CatalogCacheState::Missing,
            };
            let plan = catalog_worker_plan(cache_state, request);
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_refresh_decision".to_string(),
                detail: format!(
                    "cache_state={} request={} plan={} execution_mode={}",
                    cache_state.label(),
                    request.label(),
                    plan.label(),
                    execution_mode.label()
                ),
            });
            if plan == CatalogWorkerPlan::LoadOnly {
                let _ = tx.send(CatalogWorkerMessage::Done);
                return;
            }
            run_fast_catalog_refresh_in_process(
                &root,
                plan,
                catalog_paths.sharded_catalog_dir(),
                &tx,
                &_mutation_lease,
            );
        })
        .expect("spawn catalog-refresh");
    rx
}

fn start_library_catalog_worker_process(
    root: String,
    request: CatalogWorkerRequest,
    initial_cache: CatalogWorkerInitialCache,
    execution_mode: CatalogExecutionMode,
    catalog_paths: mister_magik_catalog::device_layout::CatalogPaths,
) -> (CatalogWorkerReceiver, Option<Arc<CatalogChildControl>>) {
    let (event_tx, event_rx) = mpsc::sync_channel(CATALOG_WORKER_EVENT_QUEUE_CAPACITY);
    let (terminal_tx, terminal_rx) = mpsc::sync_channel(1);
    let receiver = CatalogWorkerReceiver::Process {
        events: event_rx,
        terminal: terminal_rx,
        pending_terminal: Mutex::new(None),
    };
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            let _ = terminal_tx.send(CatalogWorkerMessage::PersistenceFailed {
                error: format!("locate catalog worker executable: {error}"),
            });
            return (receiver, None);
        }
    };
    let mut command = Command::new(executable);
    command
        .arg(crate::command_args::CATALOG_WORKER_COMMAND)
        .arg(request.label())
        .arg(match initial_cache {
            CatalogWorkerInitialCache::AlreadyLoadedReady => "ready",
            CatalogWorkerInitialCache::AlreadyProbedMissing => "missing",
        })
        .arg(execution_mode.label())
        .arg(&root)
        .env(CATALOG_WORKER_CHILD_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(unix)]
    {
        // Put the worker and any archive helpers it starts into a private
        // process group so watchdog cancellation cannot strand descendants.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = terminal_tx.send(CatalogWorkerMessage::PersistenceFailed {
                error: format!("spawn catalog worker child: {error}"),
            });
            return (receiver, None);
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = terminal_tx.send(CatalogWorkerMessage::PersistenceFailed {
                error: "catalog worker child has no protocol stream".to_string(),
            });
            return (receiver, None);
        }
    };
    let process_group = child.id().try_into().unwrap_or(-1);
    let control = Arc::new(CatalogChildControl {
        child: Mutex::new(Some(child)),
        process_group,
        handshake_seen: AtomicBool::new(false),
        reaped: AtomicBool::new(false),
    });
    let handshake_control = Arc::clone(&control);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(5));
        if !handshake_control.handshake_seen.load(Ordering::Acquire) && !handshake_control.reaped()
        {
            let _ = handshake_control.terminate();
        }
    });
    let reader_control = Arc::clone(&control);
    let reader_root = root.clone();
    let reader_catalog_root = catalog_paths.sharded_catalog_dir().to_path_buf();
    std::thread::Builder::new()
        .name("catalog-worker-protocol".to_string())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut terminal = false;
            let mut terminal_message = None;
            let mut protocol_failed = false;
            let mut protocol_state = CatalogWorkerProtocolState::default();
            loop {
                let line = match read_catalog_worker_protocol_line(&mut reader) {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(error) => {
                        terminal_message = Some(CatalogWorkerMessage::PersistenceFailed { error });
                        protocol_failed = true;
                        break;
                    }
                };
                let Some(payload) = line.strip_prefix(CATALOG_WORKER_PROTOCOL_PREFIX) else {
                    continue;
                };
                let event = match serde_json::from_str::<CatalogWorkerWireEvent>(payload) {
                    Ok(event) => event,
                    Err(error) => {
                        terminal_message = Some(CatalogWorkerMessage::PersistenceFailed {
                            error: format!("decode catalog worker protocol: {error}"),
                        });
                        protocol_failed = true;
                        break;
                    }
                };
                match protocol_state.validate(&event) {
                    Ok(true) => {
                        reader_control.handshake_seen.store(true, Ordering::Release);
                        let _ = event_tx.try_send(CatalogWorkerMessage::Timing {
                            name: "catalog_worker_handshake_v4".to_string(),
                            detail: format!("run_id={}", event.run_id),
                        });
                        continue;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        terminal_message = Some(CatalogWorkerMessage::PersistenceFailed { error });
                        protocol_failed = true;
                        break;
                    }
                }
                match catalog_worker_message_from_wire(event, &reader_root, &reader_catalog_root) {
                    Ok(Some(message)) => {
                        terminal = matches!(
                            message,
                            CatalogWorkerMessage::Done
                                | CatalogWorkerMessage::LoadFailed { .. }
                                | CatalogWorkerMessage::PersistenceFailed { .. }
                        );
                        if terminal {
                            terminal_message = Some(message);
                            break;
                        }
                        match event_tx.try_send(message) {
                            Ok(()) => {}
                            Err(mpsc::TrySendError::Full(
                                CatalogWorkerMessage::Heartbeat { .. }
                                | CatalogWorkerMessage::Timing { .. },
                            )) => {}
                            Err(mpsc::TrySendError::Full(_)) => {
                                terminal_message = Some(CatalogWorkerMessage::PersistenceFailed {
                                    error:
                                        "catalog worker event queue exceeded its bounded capacity"
                                            .to_string(),
                                });
                                protocol_failed = true;
                                break;
                            }
                            Err(mpsc::TrySendError::Disconnected(_)) => break,
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        terminal_message = Some(CatalogWorkerMessage::PersistenceFailed { error });
                        protocol_failed = true;
                        break;
                    }
                }
            }
            if !terminal && terminal_message.is_none() {
                terminal_message = Some(CatalogWorkerMessage::PersistenceFailed {
                    error: "catalog worker protocol ended without a terminal event".to_string(),
                });
                protocol_failed = true;
            }
            if protocol_failed {
                let _ = reader_control.terminate();
                if let Some(message) = terminal_message.take() {
                    let _ = terminal_tx.try_send(message);
                }
            }
            let mut child = reader_control
                .child
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            let child_status = child.as_mut().and_then(|child| child.wait().ok());
            reader_control.reaped.store(true, Ordering::Release);
            if let Some(message) = terminal_message {
                let _ = terminal_tx.try_send(message);
            } else if !terminal {
                let detail = child_status
                    .map(|status| format!("catalog worker child exited with {status}"))
                    .unwrap_or_else(|| {
                        "catalog worker child exited without a terminal event".to_string()
                    });
                let _ =
                    terminal_tx.try_send(CatalogWorkerMessage::PersistenceFailed { error: detail });
            }
        })
        .expect("spawn catalog worker protocol reader");
    (receiver, Some(control))
}

fn catalog_worker_message_from_wire(
    event: CatalogWorkerWireEvent,
    root: &str,
    catalog_root: &Path,
) -> Result<Option<CatalogWorkerMessage>, String> {
    if event.version != CATALOG_WORKER_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported catalog worker protocol version {}",
            event.version
        ));
    }
    let message = match event.kind.as_str() {
        "heartbeat" => CatalogWorkerMessage::Heartbeat {
            run_id: event.run_id,
            phase: event.phase,
            sequence: event.sequence,
            progress_epoch: event.progress_epoch,
            work_units: event.work_units,
        },
        "timing" => CatalogWorkerMessage::Timing {
            name: event.name,
            detail: event.detail,
        },
        "load-failed" => CatalogWorkerMessage::LoadFailed { error: event.error },
        "plan-ready" => CatalogWorkerMessage::ReconciliationPlanReady {
            system_ids: event.system_ids,
            all_published_systems: event.all_published_systems,
        },
        "system-scanning" => CatalogWorkerMessage::SystemScanning {
            system_id: event.system_id,
        },
        "system-prepared" => CatalogWorkerMessage::SystemPrepared {
            system_id: event.system_id,
            generation: event.generation,
        },
        "system-removed" => CatalogWorkerMessage::SystemRemoved {
            system_id: event.system_id,
        },
        "system-update-failed" => CatalogWorkerMessage::SystemUpdateFailed {
            system_id: event.system_id,
            error: event.error,
        },
        "manifest-published" => CatalogWorkerMessage::ManifestPublished {
            generation: event.generation,
            rebuilt: event.rebuilt,
            removed: event.removed,
        },
        "build-completed" => CatalogWorkerMessage::BuildCompleted {
            elapsed_us: event.elapsed_us,
        },
        "hydration-done" => {
            CatalogWorkerMessage::HydrationDoneNeedsValidation { root: event.detail }
        }
        "persistence-failed" => CatalogWorkerMessage::PersistenceFailed { error: event.error },
        "done" => CatalogWorkerMessage::Done,
        "ready" if event.source == "sharded-registry" => {
            let started = Instant::now();
            let seed = load_sharded_registry_seed_at(root, catalog_root)
                .map_err(|error| format!("load published catalog from child: {error}"))?;
            if seed.generation != event.generation {
                return Err(format!(
                    "published catalog generation changed: expected {}, loaded {}",
                    event.generation, seed.generation
                ));
            }
            if seed.catalog_fingerprint != event.fingerprint {
                return Err("published catalog fingerprint changed before loading".to_string());
            }
            CatalogWorkerMessage::Ready {
                catalog: seed.catalog,
                load_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                source: CatalogSource::ShardedRegistry,
                durable_save_pending: event.durable_save_pending,
                generation_fingerprint: (!event.fingerprint.is_empty())
                    .then_some(event.fingerprint),
                publication_ack: None,
            }
        }
        "ready" if event.source == "navigation-projection" => {
            let started = Instant::now();
            let catalog = load_arcade_system_snapshot(root, &event)?;
            CatalogWorkerMessage::Ready {
                catalog,
                load_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                source: CatalogSource::NavigationProjection,
                durable_save_pending: true,
                generation_fingerprint: None,
                publication_ack: None,
            }
        }
        "ready" => {
            return Err(format!(
                "unknown catalog worker ready source {:?}",
                event.source
            ));
        }
        _ => {
            return Err(format!(
                "unknown catalog worker event kind {:?}",
                event.kind
            ));
        }
    };
    Ok(Some(message))
}

pub(crate) fn run_catalog_worker_child(args: &[String]) {
    let protocol_output = match prepare_catalog_worker_protocol_output() {
        Ok(output) => output,
        Err(error) => {
            crate::ui_errln!("catalog worker child: {error}");
            std::process::exit(2);
        }
    };
    let request = match args
        .get(2)
        .map(String::as_str)
        .and_then(parse_catalog_worker_request)
    {
        Some(request) => request,
        None => {
            crate::ui_errln!("catalog worker child: invalid request");
            std::process::exit(2);
        }
    };
    let initial_cache = match args.get(3).map(String::as_str) {
        Some("ready") => CatalogWorkerInitialCache::AlreadyLoadedReady,
        Some("missing") => CatalogWorkerInitialCache::AlreadyProbedMissing,
        _ => {
            crate::ui_errln!("catalog worker child: invalid cache state");
            std::process::exit(2);
        }
    };
    let execution_mode = match args.get(4).map(String::as_str) {
        Some("foreground_exclusive") => CatalogExecutionMode::ForegroundExclusive,
        Some("background_interactive") => CatalogExecutionMode::BackgroundInteractive,
        _ => {
            crate::ui_errln!("catalog worker child: invalid execution mode");
            std::process::exit(2);
        }
    };
    let root = args
        .get(5)
        .cloned()
        .unwrap_or_else(|| "/media/fat/_Arcade".to_string());
    let paths = mister_magik_catalog::device_layout::CatalogPaths::capture_process();
    let archive_cache =
        mister_magik_catalog::catalog_config::ArchiveCacheConfig::capture_process(&paths);
    let rx = start_library_catalog_worker_in_process(
        root,
        request,
        initial_cache,
        execution_mode,
        paths,
        archive_cache,
    );
    let writer = Arc::new(Mutex::new(std::io::BufWriter::new(protocol_output)));
    let (heartbeat_stop, heartbeat_stop_rx) = mpsc::sync_channel(1);
    let heartbeat_run_id = mister_magik_catalog::catalog_lease::CatalogRunId::new();
    let wire_run_id = heartbeat_run_id.as_str().to_string();
    let progress = Arc::new(Mutex::new(CatalogHeartbeatProgress::new()));
    let wire_sequence = Arc::new(std::sync::atomic::AtomicU64::new(1));
    {
        let mut output = writer.lock().unwrap_or_else(|error| error.into_inner());
        let mut event = blank_worker_wire_event("handshake");
        event.run_id.clone_from(&wire_run_id);
        event.detail = format!("operation={}", request.label());
        let _ = output.write_all(CATALOG_WORKER_PROTOCOL_PREFIX.as_bytes());
        let _ = write_worker_wire_event(&mut *output, &event);
    }
    let heartbeat_writer = Arc::clone(&writer);
    let heartbeat_progress = Arc::clone(&progress);
    let heartbeat_sequence = Arc::clone(&wire_sequence);
    let heartbeat_wire_run_id = wire_run_id.clone();
    let heartbeat = std::thread::spawn(move || {
        while heartbeat_interval_elapsed(&heartbeat_stop_rx, std::time::Duration::from_secs(10)) {
            let snapshot = heartbeat_progress
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            let mut event = CatalogWorkerWireEvent {
                version: CATALOG_WORKER_PROTOCOL_VERSION,
                kind: "heartbeat".to_string(),
                name: String::new(),
                detail: String::new(),
                error: String::new(),
                system_id: String::new(),
                system_ids: Vec::new(),
                all_published_systems: false,
                generation: 0,
                rebuilt: Vec::new(),
                removed: Vec::new(),
                elapsed_us: 0,
                source: String::new(),
                durable_save_pending: false,
                fingerprint: String::new(),
                run_id: heartbeat_wire_run_id.clone(),
                phase: snapshot.phase,
                sequence: 0,
                progress_epoch: snapshot.progress_epoch,
                work_units: snapshot.work_units,
                snapshot_path: String::new(),
                snapshot_sha256: String::new(),
            };
            let mut writer = heartbeat_writer
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            event.sequence = heartbeat_sequence.fetch_add(1, Ordering::Relaxed);
            let _ = writer.write_all(CATALOG_WORKER_PROTOCOL_PREFIX.as_bytes());
            let _ = write_worker_wire_event(&mut *writer, &event);
        }
    });
    let mut terminal = false;
    while let Ok(message) = rx.recv() {
        if let CatalogWorkerMessage::Progress { phase, work_units } = &message {
            progress
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .advance(phase, *work_units);
            continue;
        }
        let mut event = match &message {
            CatalogWorkerMessage::ArcadeBootstrapReady { system, load_us } => {
                match write_arcade_system_snapshot(&wire_run_id, system) {
                    Ok((snapshot_path, snapshot_sha256)) => {
                        let mut event = blank_worker_wire_event("ready");
                        event.source = CatalogSource::NavigationProjection.label().to_string();
                        event.durable_save_pending = true;
                        event.elapsed_us = *load_us;
                        event.snapshot_path = snapshot_path;
                        event.snapshot_sha256 = snapshot_sha256;
                        event
                    }
                    Err(error) => {
                        let mut event = blank_worker_wire_event("persistence-failed");
                        event.error = format!("write Arcade bootstrap snapshot: {error}");
                        terminal = true;
                        event
                    }
                }
            }
            _ => worker_wire_event(&message),
        };
        let mut output = writer.lock().unwrap_or_else(|error| error.into_inner());
        event.run_id.clone_from(&wire_run_id);
        event.sequence = wire_sequence.fetch_add(1, Ordering::Relaxed);
        if output
            .write_all(CATALOG_WORKER_PROTOCOL_PREFIX.as_bytes())
            .is_err()
            || !write_worker_wire_event(&mut *output, &event)
        {
            break;
        }
        terminal = terminal
            || matches!(
                message,
                CatalogWorkerMessage::Done
                    | CatalogWorkerMessage::LoadFailed { .. }
                    | CatalogWorkerMessage::PersistenceFailed { .. }
            );
        if terminal {
            break;
        }
    }
    let _ = heartbeat_stop.send(());
    let _ = heartbeat.join();
    if !terminal {
        let mut output = writer.lock().unwrap_or_else(|error| error.into_inner());
        let event = CatalogWorkerWireEvent {
            version: CATALOG_WORKER_PROTOCOL_VERSION,
            kind: "persistence-failed".to_string(),
            name: String::new(),
            detail: String::new(),
            error: "catalog worker channel closed without a terminal event".to_string(),
            system_id: String::new(),
            system_ids: Vec::new(),
            all_published_systems: false,
            generation: 0,
            rebuilt: Vec::new(),
            removed: Vec::new(),
            elapsed_us: 0,
            source: String::new(),
            durable_save_pending: false,
            fingerprint: String::new(),
            run_id: wire_run_id,
            phase: String::new(),
            sequence: wire_sequence.fetch_add(1, Ordering::Relaxed),
            progress_epoch: 0,
            work_units: 0,
            snapshot_path: String::new(),
            snapshot_sha256: String::new(),
        };
        let _ = output.write_all(CATALOG_WORKER_PROTOCOL_PREFIX.as_bytes());
        let _ = write_worker_wire_event(&mut *output, &event);
    }
}

fn parse_catalog_worker_request(label: &str) -> Option<CatalogWorkerRequest> {
    Some(match label {
        "load_only" => CatalogWorkerRequest::LoadOnly,
        "strict_load" => CatalogWorkerRequest::StrictLoad,
        "check_stamp" => CatalogWorkerRequest::CheckStamp,
        "reconcile_changed_inputs" => CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
        "reconcile_all_systems" => CatalogWorkerRequest::RECONCILE_ALL_SYSTEMS,
        "fresh_build" => CatalogWorkerRequest::FreshBuild,
        _ => return None,
    })
}

fn run_fast_catalog_refresh_in_process(
    root: &str,
    plan: CatalogWorkerPlan,
    catalog_root: &Path,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    mutation_lease: &mister_magik_catalog::catalog_lease::CatalogMutationLease,
) {
    use mister_magik_catalog::fast_catalog_refresh::{
        FastCatalogRefreshRequest, FastCatalogSystemOutcome, FastSourceCheckStatus,
    };

    let mut catalog_profile = CatalogBuildProfiler::capture_process();
    let profile_operation = match plan {
        CatalogWorkerPlan::InitialBuild | CatalogWorkerPlan::FreshBuild => "fresh",
        CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS => "rebuild-all",
        _ => "refresh",
    };
    catalog_profile.arm(profile_operation);
    if matches!(
        plan,
        CatalogWorkerPlan::InitialBuild | CatalogWorkerPlan::FreshBuild
    ) {
        run_fast_catalog_fresh_build(root, catalog_root, tx, catalog_profile, mutation_lease);
        return;
    }
    let request = if plan == CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS {
        FastCatalogRefreshRequest::RebuildAll
    } else {
        FastCatalogRefreshRequest::Update
    };
    let storage_root = PathBuf::from("/media/fat");
    report_catalog_filesystem_headroom(tx, "begin");
    let planned = match mister_magik_catalog::fast_catalog_refresh::plan_fast_refresh(
        &storage_root,
        catalog_root,
        request,
    ) {
        Ok(planned) => planned,
        Err(error) => {
            report_catalog_filesystem_headroom(tx, "planning-error");
            catalog_profile.fail("planning-failed");
            let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                error: format!("fast catalog refresh planning failed: {error}"),
            });
            return;
        }
    };
    let system_ids = planned
        .checks
        .iter()
        .map(|check| check.system_id.clone())
        .collect::<Vec<_>>();
    let _ = tx.send(CatalogWorkerMessage::ReconciliationPlanReady {
        system_ids,
        all_published_systems: false,
    });
    let _ = tx.send(CatalogWorkerMessage::Progress {
        phase: "planning".to_string(),
        work_units: 1,
    });
    for check in &planned.checks {
        if check.status != FastSourceCheckStatus::Unchanged {
            let _ = tx.send(CatalogWorkerMessage::SystemScanning {
                system_id: check.system_id.clone(),
            });
        }
    }
    let report = match mister_magik_catalog::fast_catalog_refresh::execute_planned_fast_refresh_with_lease_and_progress(
            &storage_root,
            catalog_root,
            request,
            planned,
            mutation_lease,
            |phase, work_units| {
                let _ = tx.send(CatalogWorkerMessage::Progress {
                    phase: phase.to_string(),
                    work_units: work_units.saturating_add(1),
                });
            },
        ) {
            Ok(report) => report,
            Err(error) => {
                report_catalog_filesystem_headroom(tx, "refresh-error");
                catalog_profile.fail("refresh-failed");
                let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                    error: format!("fast catalog refresh failed: {error}"),
                });
                return;
            }
        };
    let mut rebuilt = Vec::new();
    let mut removed = Vec::new();
    for system in &report.system_reports {
        match system.outcome {
            FastCatalogSystemOutcome::Unchanged => {
                let _ = tx.send(CatalogWorkerMessage::SystemPrepared {
                    system_id: system.system_id.clone(),
                    generation: report.catalog_generation,
                });
            }
            FastCatalogSystemOutcome::Updated => {
                rebuilt.push(system.system_id.clone());
                let _ = tx.send(CatalogWorkerMessage::SystemPrepared {
                    system_id: system.system_id.clone(),
                    generation: report.catalog_generation,
                });
            }
            FastCatalogSystemOutcome::Removed => {
                removed.push(system.system_id.clone());
                let _ = tx.send(CatalogWorkerMessage::SystemRemoved {
                    system_id: system.system_id.clone(),
                });
            }
            FastCatalogSystemOutcome::FailedRetained => {
                let _ = tx.send(CatalogWorkerMessage::SystemUpdateFailed {
                    system_id: system.system_id.clone(),
                    error: system.detail.clone(),
                });
            }
        }
    }
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "fast_catalog_refresh".to_string(),
        detail: format!(
            "elapsed_us={} planning_us={} manifest_read_us={} active_read_us={} system_discovery_us={} checks_us={} watch_read_us={} metadata_probe_us={} metadata_parents={} metadata_paths={} source_rebuild_us={} artifact_publish_us={} snapshot_publish_us={} systems={} unchanged={} updated={} failed_retained={} artifact_systems_written={}",
            report.elapsed_us,
            report.planning_us,
            report.plan.manifest_read_us,
            report.plan.active_read_us,
            report.plan.system_discovery_us,
            report.plan.checks_us,
            report.plan.watch_read_us,
            report.plan.metadata_probe_us,
            report.plan.metadata_parents,
            report.plan.metadata_paths,
            report.source_rebuild_us,
            report.artifact_publish_us,
            report.snapshot_publish_us,
            report.systems,
            report.unchanged,
            report.updated,
            report.failed_retained,
            report.artifact_systems_written,
        ),
    });
    if !rebuilt.is_empty() || !removed.is_empty() {
        let _ = tx.send(CatalogWorkerMessage::ManifestPublished {
            generation: report.catalog_generation,
            rebuilt,
            removed,
        });
        if let Err(error) = publish_registry_ready_at(tx, root, catalog_root) {
            catalog_profile.fail("registry-reload-failed");
            let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                error: format!("fast catalog registry reload failed: {error}"),
            });
            return;
        }
    }
    report_catalog_filesystem_headroom(tx, "complete");
    if report.artifact_systems_written == 0 {
        catalog_profile.unchanged();
    } else {
        catalog_profile.persisted();
    }
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "catalog_100_percent_complete".to_string(),
        detail: format!(
            "generation={} systems={} unchanged={} updated={} artifacts_written={}",
            report.catalog_generation,
            report.systems,
            report.unchanged,
            report.updated,
            report.artifact_systems_written,
        ),
    });
    let _ = tx.send(CatalogWorkerMessage::Done);
}

fn run_fast_catalog_fresh_build(
    root: &str,
    catalog_root: &Path,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    mut catalog_profile: CatalogBuildProfiler,
    mutation_lease: &mister_magik_catalog::catalog_lease::CatalogMutationLease,
) {
    let storage_root = PathBuf::from("/media/fat");
    report_catalog_filesystem_headroom(tx, "fresh-begin");
    let mut planned_system_ids = Vec::new();
    let mut completed_system_ids = std::collections::BTreeSet::new();
    let progress_units = std::cell::Cell::new(0u64);
    let report = match mister_magik_catalog::fast_catalog_refresh::build_fresh_catalog_with_lease(
        &storage_root,
        catalog_root,
        mutation_lease,
        |system_ids| {
            planned_system_ids = system_ids.to_vec();
            let _ = tx.send(CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids: system_ids.to_vec(),
                all_published_systems: true,
            });
            progress_units.set(progress_units.get().saturating_add(1));
            let _ = tx.send(CatalogWorkerMessage::Progress {
                phase: "planning".to_string(),
                work_units: progress_units.get(),
            });
            for system_id in system_ids {
                if system_id == "arcade" {
                    continue;
                }
                let _ = tx.send(CatalogWorkerMessage::SystemScanning {
                    system_id: system_id.clone(),
                });
            }
            if system_ids.iter().any(|system_id| system_id == "arcade") {
                let _ = tx.send(CatalogWorkerMessage::SystemPrepared {
                    system_id: "arcade".to_string(),
                    generation: 0,
                });
            }
        },
        |system| {
            if completed_system_ids.insert(system.system_id.clone()) {
                let _ = tx.send(CatalogWorkerMessage::SystemPrepared {
                    system_id: system.system_id.clone(),
                    generation: 0,
                });
                progress_units.set(progress_units.get().saturating_add(1));
                let _ = tx.send(CatalogWorkerMessage::Progress {
                    phase: "systems".to_string(),
                    work_units: progress_units.get(),
                });
            }
            if system.system_id == "arcade" && !system.games.is_empty() {
                let load_us = 0;
                let _ = tx.send(CatalogWorkerMessage::Timing {
                    name: "catalog_arcade_bootstrap_ready".to_string(),
                    detail: format!("games={} load_us={load_us}", system.games.len()),
                });
                if std::env::var_os(CATALOG_WORKER_CHILD_ENV).is_some() {
                    let _ = tx.send(CatalogWorkerMessage::ArcadeBootstrapReady {
                        system: Box::new(system.clone()),
                        load_us,
                    });
                } else {
                    let started = Instant::now();
                    let catalog = mister_magik_catalog::fast_catalog_sources::launcher_catalog_for_fast_system(
                        Path::new(root),
                        system,
                    );
                    send_ready_catalog(
                        tx,
                        catalog,
                        started.elapsed().as_micros() as u64,
                        CatalogSource::NavigationProjection,
                        true,
                        None,
                    );
                }
            }
        },
    ) {
        Ok(report) => report,
        Err(error) => {
            report_catalog_filesystem_headroom(tx, "fresh-error");
            catalog_profile.fail("fresh-build-failed");
            let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                error: format!("catalog build failed: {error}"),
            });
            return;
        }
    };
    for system_id in &planned_system_ids {
        if completed_system_ids.insert(system_id.clone()) {
            let _ = tx.send(CatalogWorkerMessage::SystemPrepared {
                system_id: system_id.clone(),
                generation: report.publication.generation,
            });
            progress_units.set(progress_units.get().saturating_add(1));
            let _ = tx.send(CatalogWorkerMessage::Progress {
                phase: "systems".to_string(),
                work_units: progress_units.get(),
            });
        }
    }
    progress_units.set(progress_units.get().saturating_add(1));
    let _ = tx.send(CatalogWorkerMessage::Progress {
        phase: "artifacts".to_string(),
        work_units: progress_units.get(),
    });
    let _ = tx.send(CatalogWorkerMessage::ManifestPublished {
        generation: report.publication.generation,
        rebuilt: report.system_ids.clone(),
        removed: Vec::new(),
    });
    report_catalog_filesystem_headroom(tx, "fresh-complete");
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "catalog_fresh_build".to_string(),
        detail: format!(
            "elapsed_us={} source_us={} publish_us={} capture_us={} refresh_state_publish_us={} systems={} games={} copied_bytes={}",
            report.elapsed_us,
            report.source.elapsed_us,
            report.publication.elapsed_us,
            report.capture.elapsed_us,
            report.refresh_state_publish.elapsed_us,
            report.publication.systems,
            report.publication.games,
            report.publication.copied_bytes,
        ),
    });
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "catalog_100_percent_complete".to_string(),
        detail: format!(
            "generation={} systems={} games={} artifacts_written={}",
            report.publication.generation,
            report.publication.systems,
            report.publication.games,
            report.publication.systems,
        ),
    });
    let _ = tx.send(CatalogWorkerMessage::BuildCompleted {
        elapsed_us: report.elapsed_us,
    });
    if let Err(error) = publish_registry_ready_at(tx, root, catalog_root) {
        catalog_profile.fail("registry-reload-failed");
        let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
            error: format!("catalog registry load failed: {error}"),
        });
        return;
    }
    catalog_profile.persisted();
    let _ = tx.send(CatalogWorkerMessage::Done);
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogReconcileScope {
    ChangedInputs,
    AllSystems,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogWorkerRequest {
    LoadOnly,
    StrictLoad,
    CheckStamp,
    Reconcile { scope: CatalogReconcileScope },
    FreshBuild,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogExecutionMode {
    ForegroundExclusive,
    BackgroundInteractive,
}

impl CatalogExecutionMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ForegroundExclusive => "foreground_exclusive",
            Self::BackgroundInteractive => "background_interactive",
        }
    }

    fn thread_role(self) -> RuntimeThreadRole {
        match self {
            Self::ForegroundExclusive => RuntimeThreadRole::CatalogForeground,
            Self::BackgroundInteractive => RuntimeThreadRole::CatalogWorker,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogWorkerInitialCache {
    AlreadyLoadedReady,
    AlreadyProbedMissing,
}

impl CatalogWorkerRequest {
    pub(super) const RECONCILE_CHANGED_INPUTS: Self = Self::Reconcile {
        scope: CatalogReconcileScope::ChangedInputs,
    };
    pub(super) const RECONCILE_ALL_SYSTEMS: Self = Self::Reconcile {
        scope: CatalogReconcileScope::AllSystems,
    };

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::LoadOnly => "load_only",
            Self::StrictLoad => "strict_load",
            Self::CheckStamp => "check_stamp",
            Self::Reconcile {
                scope: CatalogReconcileScope::ChangedInputs,
            } => "reconcile_changed_inputs",
            Self::Reconcile {
                scope: CatalogReconcileScope::AllSystems,
            } => "reconcile_all_systems",
            Self::FreshBuild => "fresh_build",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogCacheState {
    Ready,
    Missing,
}

impl CatalogCacheState {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Missing => "missing",
        }
    }

    fn has_usable_catalog(self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogWorkerPlan {
    LoadOnly,
    CheckStamp,
    InitialBuild,
    Reconcile { scope: CatalogReconcileScope },
    FreshBuild,
}

impl CatalogWorkerPlan {
    const RECONCILE_CHANGED_INPUTS: Self = Self::Reconcile {
        scope: CatalogReconcileScope::ChangedInputs,
    };
    const RECONCILE_ALL_SYSTEMS: Self = Self::Reconcile {
        scope: CatalogReconcileScope::AllSystems,
    };

    fn label(self) -> &'static str {
        match self {
            Self::LoadOnly => "load_only",
            Self::CheckStamp => "check_stamp",
            Self::InitialBuild => "initial_build",
            Self::Reconcile {
                scope: CatalogReconcileScope::ChangedInputs,
            } => "reconcile_changed_inputs",
            Self::Reconcile {
                scope: CatalogReconcileScope::AllSystems,
            } => "reconcile_all_systems",
            Self::FreshBuild => "fresh_build",
        }
    }
}

fn catalog_worker_plan(
    cache_state: CatalogCacheState,
    request: CatalogWorkerRequest,
) -> CatalogWorkerPlan {
    match request {
        CatalogWorkerRequest::StrictLoad => return CatalogWorkerPlan::LoadOnly,
        CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS => {
            return CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS;
        }
        CatalogWorkerRequest::FreshBuild => return CatalogWorkerPlan::FreshBuild,
        _ => {}
    }
    match cache_state {
        CatalogCacheState::Ready => match request {
            CatalogWorkerRequest::LoadOnly => CatalogWorkerPlan::LoadOnly,
            CatalogWorkerRequest::StrictLoad => CatalogWorkerPlan::LoadOnly,
            CatalogWorkerRequest::CheckStamp => CatalogWorkerPlan::CheckStamp,
            CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS => {
                CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS
            }
            CatalogWorkerRequest::RECONCILE_ALL_SYSTEMS => CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS,
            CatalogWorkerRequest::FreshBuild => CatalogWorkerPlan::FreshBuild,
        },
        CatalogCacheState::Missing => CatalogWorkerPlan::InitialBuild,
    }
}

pub(super) enum CatalogWorkerMessage {
    Progress {
        phase: String,
        work_units: u64,
    },
    Heartbeat {
        run_id: String,
        phase: String,
        sequence: u64,
        progress_epoch: u64,
        work_units: u64,
    },
    Timing {
        name: String,
        detail: String,
    },
    LoadFailed {
        error: String,
    },
    ReconciliationPlanReady {
        system_ids: Vec<String>,
        all_published_systems: bool,
    },
    SystemScanning {
        system_id: String,
    },
    SystemPrepared {
        system_id: String,
        generation: u64,
    },
    SystemRemoved {
        system_id: String,
    },
    SystemUpdateFailed {
        system_id: String,
        error: String,
    },
    ManifestPublished {
        generation: u64,
        rebuilt: Vec<String>,
        removed: Vec<String>,
    },
    BuildCompleted {
        elapsed_us: u64,
    },
    SystemShardReady {
        system_id: String,
        catalog: ArcadeCatalog,
        base_catalog_version: usize,
        game_count: usize,
        prepare_us: u64,
        profile: SystemEntryCatalogProfile,
        preview_prelude: Option<SystemEntryPreviewPrelude>,
    },
    SystemShardFailed {
        system_id: String,
        error: String,
    },
    SearchQueryReady {
        request: launcher::ArcadeSearchRequest,
        result: mister_magik_catalog::persisted_search::PersistedCollectionSearchResult,
    },
    SearchQueryFailed {
        request: launcher::ArcadeSearchRequest,
        error: String,
    },
    HydrationDoneNeedsValidation {
        root: String,
    },
    Ready {
        catalog: ArcadeCatalog,
        load_us: u64,
        source: CatalogSource,
        durable_save_pending: bool,
        generation_fingerprint: Option<String>,
        publication_ack: Option<mpsc::Sender<()>>,
    },
    ArcadeBootstrapReady {
        system: Box<mister_magik_catalog::fast_five_catalog::FastFiveSystem>,
        load_us: u64,
    },
    PublishedRegistryReady {
        generation: u64,
        fingerprint: String,
    },
    PersistenceFailed {
        error: String,
    },
    Done,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(super) struct SystemEntryCatalogProfile {
    pub(super) open: mister_magik_catalog::lazy_sharded_reader::LazySystemOpenTiming,
    pub(super) catalog_replacement_us: u64,
    pub(super) total_wall_us: u64,
    pub(super) thread_cpu_us: u64,
    pub(super) cpu_start: i32,
    pub(super) cpu_end: i32,
    pub(super) minor_page_faults: u64,
    pub(super) major_page_faults: u64,
    pub(super) allocations: u64,
    pub(super) allocated_bytes: u64,
}

pub(super) fn print_startup_event(start: Instant, name: &str, detail: impl std::fmt::Display) {
    let elapsed_us = start.elapsed().as_micros();
    let elapsed_ms = elapsed_us / 1_000;
    let detail = detail.to_string();
    boot_analytics::event(
        name,
        format!("since_run_ui_us={elapsed_us} since_run_ui_ms={elapsed_ms} {detail}"),
    );
    crate::ui_logln!("startup_timing\t{name}\t{elapsed_us}us\t{detail}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_builds_missing_catalogs_and_updates_ready_catalogs() {
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Missing, CatalogWorkerRequest::LoadOnly),
            CatalogWorkerPlan::InitialBuild
        );
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Ready, CatalogWorkerRequest::LoadOnly),
            CatalogWorkerPlan::LoadOnly
        );
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Ready, CatalogWorkerRequest::CheckStamp),
            CatalogWorkerPlan::CheckStamp
        );
        assert_eq!(
            catalog_worker_plan(
                CatalogCacheState::Ready,
                CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
            ),
            CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS
        );
        assert_eq!(
            catalog_worker_plan(
                CatalogCacheState::Ready,
                CatalogWorkerRequest::RECONCILE_ALL_SYSTEMS,
            ),
            CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS
        );
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Ready, CatalogWorkerRequest::FreshBuild),
            CatalogWorkerPlan::FreshBuild
        );
    }

    #[test]
    fn refresh_has_no_external_builder_lock() {
        assert!(catalog_refresh_available());
    }

    #[test]
    fn heartbeat_protocol_is_decodable_without_counting_as_progress() {
        let event = CatalogWorkerWireEvent {
            version: CATALOG_WORKER_PROTOCOL_VERSION,
            kind: "heartbeat".to_string(),
            name: String::new(),
            detail: String::new(),
            error: String::new(),
            system_id: String::new(),
            system_ids: Vec::new(),
            all_published_systems: false,
            generation: 0,
            rebuilt: Vec::new(),
            removed: Vec::new(),
            elapsed_us: 0,
            source: String::new(),
            durable_save_pending: false,
            fingerprint: String::new(),
            run_id: "run-1".to_string(),
            phase: "scan".to_string(),
            sequence: 4,
            progress_epoch: 2,
            work_units: 99,
            snapshot_path: String::new(),
            snapshot_sha256: String::new(),
        };
        let encoded = serde_json::to_string(&event).unwrap();
        let decoded: CatalogWorkerWireEvent = serde_json::from_str(&encoded).unwrap();
        let message = catalog_worker_message_from_wire(
            decoded,
            "/media/fat/_Arcade",
            Path::new("/tmp/catalog-fast-v1"),
        )
        .unwrap()
        .expect("heartbeat event");
        assert!(matches!(message, CatalogWorkerMessage::Heartbeat { .. }));
    }

    #[test]
    fn arcade_bootstrap_snapshot_is_bounded_verified_and_consumed() {
        let snapshot_root = std::env::temp_dir().join(format!(
            "mister-magik-catalog-worker-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let run_id = "run-test";
        let system = mister_magik_catalog::fast_five_catalog::FastFiveSystem {
            system_id: "arcade".to_string(),
            display_title: "Arcade".to_string(),
            games: vec![mister_magik_catalog::system_shard::SystemGame {
                stable_key: "arcade\u{1f}game".to_string(),
                title: "Game".to_string(),
                launch_ref: "/media/fat/_Arcade/Game.mra".to_string(),
                ..Default::default()
            }],
            variants: Vec::new(),
        };
        let (path, checksum) =
            write_arcade_system_snapshot_at(&snapshot_root, run_id, &system).unwrap();
        let mut event = blank_worker_wire_event("ready");
        event.run_id = run_id.to_string();
        event.snapshot_path = path.clone();
        event.snapshot_sha256 = checksum;

        let catalog =
            load_arcade_system_snapshot_at(&snapshot_root, "/media/fat/_Arcade", &event).unwrap();

        assert_eq!(catalog.len(), 1);
        assert!(!Path::new(&path).exists());
        std::fs::remove_dir_all(snapshot_root).unwrap();
    }

    #[test]
    fn protocol_requires_one_run_and_monotonic_progress() {
        let mut state = CatalogWorkerProtocolState::default();
        let mut handshake = blank_worker_wire_event("handshake");
        handshake.run_id = "run-1".to_string();
        assert_eq!(state.validate(&handshake), Ok(true));
        assert!(state.validate(&handshake).is_err());

        let mut heartbeat = blank_worker_wire_event("heartbeat");
        heartbeat.run_id = "run-1".to_string();
        heartbeat.sequence = 1;
        heartbeat.phase = "systems".to_string();
        heartbeat.progress_epoch = 1;
        heartbeat.work_units = 4;
        assert_eq!(state.validate(&heartbeat), Ok(false));

        heartbeat.sequence = 2;
        heartbeat.work_units = 4;
        assert_eq!(state.validate(&heartbeat), Ok(false));

        heartbeat.sequence = 3;
        heartbeat.work_units = 3;
        assert!(state.validate(&heartbeat).is_err());

        let mut wrong_run = blank_worker_wire_event("done");
        wrong_run.run_id = "run-2".to_string();
        wrong_run.sequence = 4;
        assert!(state.validate(&wrong_run).is_err());
    }

    #[test]
    fn heartbeat_stop_interrupts_interval_wait() {
        let (stop, stop_rx) = mpsc::sync_channel(1);
        let started = Instant::now();
        let waiter = std::thread::spawn(move || {
            heartbeat_interval_elapsed(&stop_rx, std::time::Duration::from_secs(10))
        });

        stop.send(()).unwrap();

        assert!(!waiter.join().unwrap());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn protocol_reader_rejects_oversized_and_incomplete_lines() {
        let mut oversized = vec![b'x'; MAX_CATALOG_WORKER_PROTOCOL_LINE_BYTES as usize];
        oversized.push(b'\n');
        assert!(read_catalog_worker_protocol_line(&mut std::io::Cursor::new(oversized)).is_err());
        assert!(
            read_catalog_worker_protocol_line(&mut std::io::Cursor::new(b"incomplete")).is_err()
        );
    }

    #[test]
    fn process_receiver_preserves_success_order_and_prioritizes_failure() {
        let (event_tx, event_rx) = mpsc::sync_channel(2);
        let (terminal_tx, terminal_rx) = mpsc::sync_channel(1);
        let receiver = CatalogWorkerReceiver::Process {
            events: event_rx,
            terminal: terminal_rx,
            pending_terminal: Mutex::new(None),
        };
        event_tx
            .send(CatalogWorkerMessage::Timing {
                name: "before-done".to_string(),
                detail: String::new(),
            })
            .unwrap();
        terminal_tx.send(CatalogWorkerMessage::Done).unwrap();
        assert!(matches!(
            receiver.try_recv(),
            Ok(CatalogWorkerMessage::Timing { .. })
        ));
        assert!(matches!(
            receiver.try_recv(),
            Ok(CatalogWorkerMessage::Done)
        ));

        let (event_tx, event_rx) = mpsc::sync_channel(1);
        let (terminal_tx, terminal_rx) = mpsc::sync_channel(1);
        let receiver = CatalogWorkerReceiver::Process {
            events: event_rx,
            terminal: terminal_rx,
            pending_terminal: Mutex::new(None),
        };
        event_tx.send(CatalogWorkerMessage::Done).unwrap();
        terminal_tx
            .send(CatalogWorkerMessage::PersistenceFailed {
                error: "failed".to_string(),
            })
            .unwrap();
        assert!(matches!(
            receiver.try_recv(),
            Ok(CatalogWorkerMessage::PersistenceFailed { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn child_control_terminates_without_owning_the_child_handle() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("exec sleep 30");
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().expect("spawn isolated child");
        let process_group = child.id().try_into().unwrap();
        let control = CatalogChildControl {
            child: Mutex::new(Some(child)),
            process_group,
            handshake_seen: AtomicBool::new(true),
            reaped: AtomicBool::new(false),
        };
        let mut owned_child = control.child.lock().unwrap().take().unwrap();

        let terminated = control.terminate();
        if !terminated {
            let _ = owned_child.kill();
        }
        let status = owned_child.wait().expect("reap isolated child");
        control.reaped.store(true, Ordering::Release);

        assert!(terminated);
        assert!(!status.success());
        assert!(control.reaped());
    }
}
