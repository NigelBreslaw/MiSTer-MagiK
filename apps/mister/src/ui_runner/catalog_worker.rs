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
#[cfg(all(test, unix))]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
#[cfg(unix)]
use std::os::unix::io::RawFd;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const CATALOG_WORKER_CHILD_ENV: &str = "MISTER_CATALOG_WORKER_CHILD";
const CATALOG_WORKER_PROTOCOL_PREFIX: &str = "MISTER_CATALOG_EVENT ";
const CATALOG_WORKER_PROTOCOL_VERSION: u8 = 6;
const MAX_CATALOG_WORKER_PROTOCOL_LINE_BYTES: u64 = 256 * 1024;
const CATALOG_WORKER_EVENT_QUEUE_CAPACITY: usize = 16_384;
const MAX_CATALOG_WORKER_COLLECTION_CHUNK_ITEMS: usize = 512;
const MAX_CATALOG_WORKER_COLLECTION_ITEM_BYTES: usize = 256;
const MAX_CATALOG_WORKER_COLLECTION_ITEMS_TOTAL: usize = 65_536;
const CATALOG_WORKER_SNAPSHOT_DIRECTORY: &str = "/tmp/mister-magik/catalog-worker";
const CATALOG_WORKER_REGISTRY_SNAPSHOT_SUFFIX: &str = "registry-seed";
const CATALOG_WORKER_REGISTRY_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct RegistrySeedTransport {
    generation: u64,
    fingerprint: String,
    systems: Vec<RegistrySeedSystem>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct RegistrySeedSystem {
    system_id: String,
    display_title: String,
    games: u64,
}

impl RegistrySeedTransport {
    fn encode(&self) -> Result<Vec<u8>, String> {
        let bytes = serde_json::to_vec(self)
            .map_err(|error| format!("encode registry seed transport: {error}"))?;
        if bytes.len() > CATALOG_WORKER_REGISTRY_SNAPSHOT_BYTES {
            return Err("registry seed transport exceeds size limit".to_string());
        }
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > CATALOG_WORKER_REGISTRY_SNAPSHOT_BYTES {
            return Err("registry seed transport exceeds size limit".to_string());
        }
        serde_json::from_slice(bytes)
            .map_err(|error| format!("decode registry seed transport: {error}"))
    }

    fn from_seed(seed: &crate::launcher_runtime::catalog::ShardedCatalogSeed) -> Self {
        Self {
            generation: seed.generation,
            fingerprint: seed.catalog_fingerprint.clone(),
            systems: seed
                .catalog
                .systems
                .iter()
                .map(|system| RegistrySeedSystem {
                    system_id: system.id.clone(),
                    display_title: system.title.clone(),
                    games: system.count as u64,
                })
                .collect(),
        }
    }

    fn into_catalog(self, root: &str) -> ArcadeCatalog {
        let platform_kinds = self
            .systems
            .iter()
            .map(|system| {
                (
                    system.system_id.clone(),
                    mister_magik_catalog::catalog_classify::platform_kind_for_system(
                        &system.system_id,
                    ),
                )
            })
            .collect();
        let systems = self
            .systems
            .into_iter()
            .map(
                |system| mister_magik_catalog::arcade_catalog::GameSystemEntry {
                    id: system.system_id,
                    title: system.display_title,
                    count: system.games.try_into().unwrap_or(usize::MAX),
                },
            )
            .collect();
        ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
            root.into(),
            Vec::new(),
            systems,
            Vec::new(),
            platform_kinds,
        )
    }
}

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
            if std::env::var_os(CATALOG_WORKER_CHILD_ENV).is_some() {
                let transport = RegistrySeedTransport::from_seed(&seed);
                tx.send(CatalogWorkerMessage::PublishedRegistrySeed {
                    transport: Box::new(transport),
                })
                .map_err(|_| {
                    "catalog worker receiver closed before registry publication".to_string()
                })?;
                return Ok(());
            }
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
    publish_strict_registry_seed_at(tx, root, storage)
}

pub(super) fn catalog_refresh_available() -> bool {
    true
}

pub(super) struct CatalogChildControl {
    child: Mutex<Option<Child>>,
    process_group: i32,
    handshake_seen: AtomicBool,
    reaped: AtomicBool,
    watchdog_terminal: Arc<Mutex<Option<CatalogWorkerMessage>>>,
}

impl CatalogChildControl {
    pub(super) fn pid(&self) -> Option<u32> {
        self.process_group.try_into().ok()
    }

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

    pub(super) fn fail_and_terminate(&self, error: impl Into<String>) {
        let mut terminal = self
            .watchdog_terminal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if terminal.is_none() {
            *terminal = Some(CatalogWorkerMessage::PersistenceFailed {
                error: error.into(),
            });
        }
        drop(terminal);
        let _ = self.terminate();
    }

    #[cfg(test)]
    pub(super) fn test_unreaped() -> Self {
        Self {
            child: Mutex::new(None),
            process_group: -1,
            handshake_seen: AtomicBool::new(true),
            reaped: AtomicBool::new(false),
            watchdog_terminal: Arc::new(Mutex::new(None)),
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
    #[serde(default)]
    collection: String,
    #[serde(default)]
    collection_index: u32,
    #[serde(default)]
    collection_chunks: u32,
    #[serde(default)]
    collection_items_total: u64,
    #[serde(default)]
    collection_items: Vec<String>,
    #[serde(default)]
    collection_checksum: String,
}

struct CatalogWorkerProtocolState {
    run_id: String,
    sequence: u64,
    heartbeat_phase: String,
    progress_epoch: u64,
    work_units: u64,
    plan: Option<CatalogWorkerCollectionAssembly>,
    manifest_rebuilt: Option<CatalogWorkerCollectionAssembly>,
    manifest_removed: Option<CatalogWorkerCollectionAssembly>,
    manifest_rebuilt_items: Option<Vec<String>>,
    manifest_removed_items: Option<Vec<String>>,
    manifest_generation: Option<u64>,
}

struct CatalogWorkerCollectionAssembly {
    chunks: u32,
    total_items: usize,
    next_index: u32,
    checksum: String,
    items: Vec<String>,
    generation: u64,
    all_published_systems: bool,
}

impl Default for CatalogWorkerProtocolState {
    fn default() -> Self {
        Self {
            run_id: String::new(),
            sequence: 0,
            heartbeat_phase: String::new(),
            progress_epoch: 0,
            work_units: 0,
            plan: None,
            manifest_rebuilt: None,
            manifest_removed: None,
            manifest_rebuilt_items: None,
            manifest_removed_items: None,
            manifest_generation: None,
        }
    }
}

fn catalog_worker_collection_checksum(items: &[String]) -> String {
    let mut digest = Sha256::new();
    for item in items {
        digest.update(item.as_bytes());
        digest.update([0]);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
        if event.kind == "collection-chunk"
            && (event.collection.is_empty()
                || event.collection_chunks == 0
                || event.collection_index >= event.collection_chunks
                || event.collection_items.len() > MAX_CATALOG_WORKER_COLLECTION_CHUNK_ITEMS
                || event.collection_items_total > MAX_CATALOG_WORKER_COLLECTION_ITEMS_TOTAL as u64
                || event.collection_checksum.len() != 64
                || !event
                    .collection_checksum
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || event
                    .collection_items
                    .iter()
                    .any(|item| item.len() > MAX_CATALOG_WORKER_COLLECTION_ITEM_BYTES))
        {
            return Err("invalid catalog worker collection chunk".to_string());
        }
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

    fn collect_collection(
        &mut self,
        event: &CatalogWorkerWireEvent,
    ) -> Result<Option<CatalogWorkerMessage>, String> {
        if event.kind != "collection-chunk" {
            if event.kind == "done"
                && (self.plan.is_some()
                    || self.manifest_rebuilt.is_some()
                    || self.manifest_removed.is_some()
                    || self.manifest_rebuilt_items.is_some()
                    || self.manifest_removed_items.is_some()
                    || self.manifest_generation.is_some())
            {
                return Err("catalog worker collection ended before all chunks".to_string());
            }
            return Ok(None);
        }
        let slot = match event.collection.as_str() {
            "plan" => &mut self.plan,
            "manifest-rebuilt" => &mut self.manifest_rebuilt,
            "manifest-removed" => &mut self.manifest_removed,
            _ => return Err("unknown catalog worker collection".to_string()),
        };
        if event.collection_index == 0 {
            if slot.is_some() {
                return Err("catalog worker collection restarted before completion".to_string());
            }
            *slot = Some(CatalogWorkerCollectionAssembly {
                chunks: event.collection_chunks,
                total_items: event.collection_items_total as usize,
                next_index: 0,
                checksum: event.collection_checksum.clone(),
                items: Vec::with_capacity(event.collection_items_total as usize),
                generation: event.generation,
                all_published_systems: event.all_published_systems,
            });
        }
        let assembly = slot
            .as_mut()
            .ok_or_else(|| "catalog worker collection chunk arrived out of order".to_string())?;
        if assembly.chunks != event.collection_chunks
            || assembly.total_items != event.collection_items_total as usize
            || assembly.checksum != event.collection_checksum
            || assembly.generation != event.generation
            || assembly.all_published_systems != event.all_published_systems
            || assembly.next_index != event.collection_index
        {
            return Err("catalog worker collection chunk metadata changed".to_string());
        }
        if assembly
            .items
            .len()
            .saturating_add(event.collection_items.len())
            > assembly.total_items
        {
            return Err("catalog worker collection contains too many items".to_string());
        }
        assembly
            .items
            .extend(event.collection_items.iter().cloned());
        assembly.next_index = assembly.next_index.saturating_add(1);
        if assembly.next_index != assembly.chunks {
            return Ok(None);
        }
        let assembly = slot.take().expect("collection assembly");
        if assembly.items.len() != assembly.total_items
            || catalog_worker_collection_checksum(&assembly.items) != assembly.checksum
        {
            return Err("catalog worker collection checksum or count differs".to_string());
        }
        match event.collection.as_str() {
            "plan" => Ok(Some(CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids: assembly.items,
                all_published_systems: assembly.all_published_systems,
            })),
            "manifest-rebuilt" => {
                if self
                    .manifest_generation
                    .is_some_and(|generation| generation != assembly.generation)
                {
                    return Err("catalog worker manifest generation changed".to_string());
                }
                self.manifest_generation = Some(assembly.generation);
                self.manifest_rebuilt_items = Some(assembly.items);
                match (
                    self.manifest_rebuilt_items.take(),
                    self.manifest_removed_items.take(),
                ) {
                    (Some(rebuilt), Some(removed)) => {
                        self.manifest_generation = None;
                        Ok(Some(CatalogWorkerMessage::ManifestPublished {
                            generation: assembly.generation,
                            rebuilt,
                            removed,
                        }))
                    }
                    (Some(rebuilt), None) => {
                        self.manifest_rebuilt_items = Some(rebuilt);
                        Ok(None)
                    }
                    (None, removed) => {
                        self.manifest_removed_items = removed;
                        Ok(None)
                    }
                }
            }
            "manifest-removed" => {
                if self
                    .manifest_generation
                    .is_some_and(|generation| generation != assembly.generation)
                {
                    return Err("catalog worker manifest generation changed".to_string());
                }
                self.manifest_generation = Some(assembly.generation);
                self.manifest_removed_items = Some(assembly.items);
                match (
                    self.manifest_rebuilt_items.take(),
                    self.manifest_removed_items.take(),
                ) {
                    (Some(rebuilt), Some(removed)) => {
                        self.manifest_generation = None;
                        Ok(Some(CatalogWorkerMessage::ManifestPublished {
                            generation: assembly.generation,
                            rebuilt,
                            removed,
                        }))
                    }
                    (rebuilt, Some(removed)) => {
                        self.manifest_rebuilt_items = rebuilt;
                        self.manifest_removed_items = Some(removed);
                        Ok(None)
                    }
                    (rebuilt, None) => {
                        self.manifest_rebuilt_items = rebuilt;
                        Ok(None)
                    }
                }
            }
            _ => unreachable!(),
        }
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
        if phase == self.phase && work_units <= self.work_units {
            return;
        }
        if self.phase != phase {
            self.phase = phase.to_string();
            self.progress_epoch = self.progress_epoch.saturating_add(1);
        }
        self.work_units = self.work_units.max(work_units);
        if self.work_units == 0 {
            self.work_units = 1;
        }
    }
}

fn write_worker_wire_event(writer: &mut impl Write, event: &CatalogWorkerWireEvent) -> bool {
    if serde_json::to_writer(&mut *writer, event).is_err() {
        return false;
    }
    writer
        .write_all(b"\n")
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
        if file_type.is_file() && entry.file_name().to_string_lossy().ends_with(".bin") {
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
    let bytes = mister_magik_catalog::fast_five_catalog::encode_fast_system_transport(system)?;
    write_catalog_worker_snapshot_at(
        root,
        run_id,
        "arcade-system",
        &bytes,
        mister_magik_catalog::fast_five_catalog::MAX_FAST_SYSTEM_TRANSPORT_BYTES,
    )
}

fn write_catalog_worker_snapshot_at(
    root: &Path,
    run_id: &str,
    suffix: &str,
    bytes: &[u8],
    max_bytes: usize,
) -> Result<(String, String), String> {
    cleanup_catalog_worker_snapshots(root)?;
    if bytes.len() > max_bytes {
        return Err(format!("catalog worker snapshot exceeds {max_bytes} bytes"));
    }
    std::fs::create_dir_all(root)
        .map_err(|error| format!("create catalog worker snapshot root: {error}"))?;
    let path = root.join(format!("{run_id}.{suffix}.bin"));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|error| format!("create catalog worker snapshot: {error}"))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("write catalog worker snapshot: {error}"));
    }
    if let Err(error) = std::fs::File::open(root).and_then(|directory| directory.sync_all()) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("sync catalog worker snapshot root: {error}"));
    }
    Ok((path.to_string_lossy().into_owned(), snapshot_sha256(&bytes)))
}

fn load_arcade_system_snapshot_at(
    snapshot_root: &Path,
    arcade_root: &str,
    event: &CatalogWorkerWireEvent,
) -> Result<ArcadeCatalog, String> {
    let bytes = load_catalog_worker_snapshot_at(
        snapshot_root,
        &event.run_id,
        "arcade-system",
        &event.snapshot_path,
        &event.snapshot_sha256,
        mister_magik_catalog::fast_five_catalog::MAX_FAST_SYSTEM_TRANSPORT_BYTES,
    )?;
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

fn write_registry_seed_snapshot(
    run_id: &str,
    transport: &RegistrySeedTransport,
) -> Result<(String, String), String> {
    let bytes = transport.encode()?;
    write_catalog_worker_snapshot_at(
        Path::new(CATALOG_WORKER_SNAPSHOT_DIRECTORY),
        run_id,
        CATALOG_WORKER_REGISTRY_SNAPSHOT_SUFFIX,
        &bytes,
        CATALOG_WORKER_REGISTRY_SNAPSHOT_BYTES,
    )
}

fn load_registry_seed_snapshot_at(
    snapshot_root: &Path,
    event: &CatalogWorkerWireEvent,
    root: &str,
) -> Result<(ArcadeCatalog, u64, String), String> {
    let bytes = load_catalog_worker_snapshot_at(
        snapshot_root,
        &event.run_id,
        CATALOG_WORKER_REGISTRY_SNAPSHOT_SUFFIX,
        &event.snapshot_path,
        &event.snapshot_sha256,
        CATALOG_WORKER_REGISTRY_SNAPSHOT_BYTES,
    )?;
    let transport = RegistrySeedTransport::decode(&bytes)?;
    if transport.generation != event.generation || transport.fingerprint != event.fingerprint {
        return Err("registry seed transport identity differs from protocol event".to_string());
    }
    let generation = transport.generation;
    let fingerprint = transport.fingerprint.clone();
    Ok((transport.into_catalog(root), generation, fingerprint))
}

fn load_catalog_worker_snapshot_at(
    snapshot_root: &Path,
    run_id: &str,
    suffix: &str,
    snapshot_path: &str,
    expected_sha256: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let expected = snapshot_root.join(format!("{run_id}.{suffix}.bin"));
    if Path::new(snapshot_path) != expected {
        return Err("catalog worker snapshot path does not match its run".to_string());
    }
    let metadata = std::fs::symlink_metadata(&expected)
        .map_err(|error| format!("inspect catalog worker snapshot: {error}"))?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes as u64 {
        return Err("catalog worker snapshot is not a bounded regular file".to_string());
    }
    let mut bytes = Vec::with_capacity(metadata.len().try_into().unwrap_or(0));
    std::fs::File::open(&expected)
        .map_err(|error| format!("open catalog worker snapshot: {error}"))?
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read catalog worker snapshot: {error}"))?;
    let _ = std::fs::remove_file(&expected);
    if bytes.len() > max_bytes || snapshot_sha256(&bytes) != expected_sha256 {
        return Err("catalog worker snapshot checksum or size differs".to_string());
    }
    Ok(bytes)
}

fn discard_catalog_worker_snapshot_at(
    snapshot_root: &Path,
    run_id: &str,
    suffix: &str,
    snapshot_path: &str,
) {
    let expected = snapshot_root.join(format!("{run_id}.{suffix}.bin"));
    if Path::new(snapshot_path) == expected {
        let _ = std::fs::remove_file(expected);
    }
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
        collection: String::new(),
        collection_index: 0,
        collection_chunks: 0,
        collection_items_total: 0,
        collection_items: Vec::new(),
        collection_checksum: String::new(),
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
        CatalogWorkerMessage::ReconciliationPlanReady { .. } => {
            unreachable!("reconciliation plans must use bounded collection chunks")
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
        CatalogWorkerMessage::ManifestPublished { .. } => {
            unreachable!("manifest publications must use bounded collection chunks")
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
        CatalogWorkerMessage::PublishedRegistrySeed { .. } => {
            unreachable!("registry seed must use the bounded snapshot transport")
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

fn catalog_worker_collection_chunk_events(
    collection: &str,
    items: &[String],
    generation: u64,
    all_published_systems: bool,
) -> Result<Vec<CatalogWorkerWireEvent>, String> {
    if items.len() > MAX_CATALOG_WORKER_COLLECTION_ITEMS_TOTAL {
        return Err("catalog worker collection exceeds item limit".to_string());
    }
    if items
        .iter()
        .any(|item| item.len() > MAX_CATALOG_WORKER_COLLECTION_ITEM_BYTES)
    {
        return Err("catalog worker collection item exceeds size limit".to_string());
    }
    let chunk_count = items
        .len()
        .div_ceil(MAX_CATALOG_WORKER_COLLECTION_CHUNK_ITEMS)
        .max(1);
    let checksum = catalog_worker_collection_checksum(items);
    let mut events = Vec::with_capacity(chunk_count);
    let mut append_chunk = |index: usize, chunk: &[String]| -> Result<(), String> {
        let mut event = blank_worker_wire_event("collection-chunk");
        event.collection = collection.to_string();
        event.collection_index = index as u32;
        event.collection_chunks = chunk_count as u32;
        event.collection_items_total = items.len() as u64;
        event.collection_items = chunk.to_vec();
        event.collection_checksum = checksum.clone();
        event.generation = generation;
        event.all_published_systems = all_published_systems;
        let encoded_bytes = serde_json::to_vec(&event)
            .map_err(|error| format!("encode catalog worker collection chunk: {error}"))?;
        if encoded_bytes.len() + CATALOG_WORKER_PROTOCOL_PREFIX.len() + 1
            > MAX_CATALOG_WORKER_PROTOCOL_LINE_BYTES as usize
        {
            return Err("catalog worker collection chunk exceeds line size limit".to_string());
        }
        events.push(event);
        Ok(())
    };
    if items.is_empty() {
        append_chunk(0, &[])?;
    } else {
        for (index, chunk) in items
            .chunks(MAX_CATALOG_WORKER_COLLECTION_CHUNK_ITEMS)
            .enumerate()
        {
            append_chunk(index, chunk)?;
        }
    }
    Ok(events)
}

fn worker_wire_events(
    message: &CatalogWorkerMessage,
) -> Result<Vec<CatalogWorkerWireEvent>, String> {
    match message {
        CatalogWorkerMessage::ReconciliationPlanReady {
            system_ids,
            all_published_systems,
        } => catalog_worker_collection_chunk_events("plan", system_ids, 0, *all_published_systems),
        CatalogWorkerMessage::ManifestPublished {
            generation,
            rebuilt,
            removed,
        } => {
            let mut events = catalog_worker_collection_chunk_events(
                "manifest-rebuilt",
                rebuilt,
                *generation,
                false,
            )?;
            events.extend(catalog_worker_collection_chunk_events(
                "manifest-removed",
                removed,
                *generation,
                false,
            )?);
            Ok(events)
        }
        CatalogWorkerMessage::PublishedRegistrySeed { .. }
        | CatalogWorkerMessage::ArcadeBootstrapReady { .. } => {
            unreachable!("bounded snapshot messages have dedicated wire handling")
        }
        _ => Ok(vec![worker_wire_event(message)]),
    }
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
        collection: String::new(),
        collection_index: 0,
        collection_chunks: 0,
        collection_items_total: 0,
        collection_items: Vec::new(),
        collection_checksum: String::new(),
    }
}

pub(super) enum CatalogWorkerReceiver {
    Direct(mpsc::Receiver<CatalogWorkerMessage>),
    Process {
        events: mpsc::Receiver<CatalogWorkerMessage>,
        terminal: mpsc::Receiver<CatalogWorkerMessage>,
        pending_terminal: Mutex<Option<CatalogWorkerMessage>>,
        watchdog_terminal: Arc<Mutex<Option<CatalogWorkerMessage>>>,
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
                watchdog_terminal,
            } => {
                if let Some(message) = watchdog_terminal
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take()
                {
                    return Ok(message);
                }
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
        let protocol_fd = duplicate_protocol_fd(libc::STDOUT_FILENO)?;
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

#[cfg(unix)]
fn duplicate_protocol_fd(fd: RawFd) -> Result<RawFd, String> {
    let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated < 0 {
        return Err(format!(
            "duplicate catalog worker protocol stream: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(duplicated)
}

pub(super) fn start_library_catalog_worker(
    root: String,
    request: CatalogWorkerRequest,
    initial_cache: CatalogWorkerInitialCache,
    execution_mode: CatalogExecutionMode,
    catalog_paths: mister_magik_catalog::device_layout::CatalogPaths,
    archive_cache: mister_magik_catalog::catalog_config::ArchiveCacheConfig,
) -> (CatalogWorkerReceiver, Option<Arc<CatalogChildControl>>) {
    if should_supervise_catalog_worker(
        request,
        std::env::var_os(CATALOG_WORKER_CHILD_ENV).is_some(),
    ) {
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
            None,
        )
        .into(),
        None,
    )
}

fn should_supervise_catalog_worker(
    _request: CatalogWorkerRequest,
    running_inside_child: bool,
) -> bool {
    !running_inside_child
}

fn start_library_catalog_worker_in_process(
    root: String,
    request: CatalogWorkerRequest,
    initial_cache: CatalogWorkerInitialCache,
    execution_mode: CatalogExecutionMode,
    catalog_paths: mister_magik_catalog::device_layout::CatalogPaths,
    _archive_cache: mister_magik_catalog::catalog_config::ArchiveCacheConfig,
    bootstrap_run_id: Option<String>,
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
                mister_magik_catalog::fast_catalog_refresh::cleanup_refresh_temporary_files_with_lease(
                    catalog_paths.sharded_catalog_dir(),
                    &_mutation_lease,
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
                bootstrap_run_id.as_deref(),
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
    let watchdog_terminal = Arc::new(Mutex::new(None));
    let receiver = CatalogWorkerReceiver::Process {
        events: event_rx,
        terminal: terminal_rx,
        pending_terminal: Mutex::new(None),
        watchdog_terminal: Arc::clone(&watchdog_terminal),
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
        watchdog_terminal,
    });
    let reaper_control = Arc::clone(&control);
    std::thread::Builder::new()
        .name("catalog-worker-reaper".to_string())
        .spawn(move || {
            let mut child = reaper_control
                .child
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            if let Some(child) = child.as_mut() {
                let _ = child.wait();
            }
            reaper_control.reaped.store(true, Ordering::Release);
        })
        .expect("spawn catalog-worker-reaper");
    let handshake_control = Arc::clone(&control);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(5));
        if !handshake_control.handshake_seen.load(Ordering::Acquire) && !handshake_control.reaped()
        {
            handshake_control.fail_and_terminate(
                "catalog worker handshake timed out before the child became observable",
            );
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
                            name: "catalog_worker_handshake_v6".to_string(),
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
                let collection_message = match protocol_state.collect_collection(&event) {
                    Ok(message) => message,
                    Err(error) => {
                        terminal_message = Some(CatalogWorkerMessage::PersistenceFailed { error });
                        protocol_failed = true;
                        break;
                    }
                };
                if event.kind == "collection-chunk" && collection_message.is_none() {
                    continue;
                }
                let decoded_message = collection_message.map(Ok).unwrap_or_else(|| {
                    catalog_worker_message_from_wire(event, &reader_root, &reader_catalog_root)
                });
                match decoded_message {
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
            }
            if let Some(message) = terminal_message {
                if let CatalogWorkerMessage::PersistenceFailed { error } = message {
                    reader_control.fail_and_terminate(error);
                } else {
                    let _ = terminal_tx.try_send(message);
                }
            } else if !terminal {
                reader_control
                    .fail_and_terminate("catalog worker child exited without a terminal event");
            }
        })
        .expect("spawn catalog worker protocol reader");
    (receiver, Some(control))
}

fn catalog_worker_message_from_wire(
    event: CatalogWorkerWireEvent,
    root: &str,
    _catalog_root: &Path,
) -> Result<Option<CatalogWorkerMessage>, String> {
    catalog_worker_message_from_wire_at(
        Path::new(CATALOG_WORKER_SNAPSHOT_DIRECTORY),
        event,
        root,
        _catalog_root,
    )
}

fn catalog_worker_message_from_wire_at(
    snapshot_root: &Path,
    event: CatalogWorkerWireEvent,
    root: &str,
    _catalog_root: &Path,
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
        "ready" if event.source == "sharded-registry-snapshot" => {
            let started = Instant::now();
            let (catalog, _generation, fingerprint) =
                load_registry_seed_snapshot_at(snapshot_root, &event, root)?;
            CatalogWorkerMessage::Ready {
                catalog,
                load_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                source: CatalogSource::ShardedRegistry,
                durable_save_pending: event.durable_save_pending,
                generation_fingerprint: Some(fingerprint),
                publication_ack: None,
            }
        }
        "ready" if event.source == "navigation-projection" => {
            let started = Instant::now();
            match load_arcade_system_snapshot_at(snapshot_root, root, &event) {
                Ok(catalog) => CatalogWorkerMessage::Ready {
                    catalog,
                    load_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                    source: CatalogSource::NavigationProjection,
                    durable_save_pending: true,
                    generation_fingerprint: None,
                    publication_ack: None,
                },
                Err(error) => {
                    discard_catalog_worker_snapshot_at(
                        snapshot_root,
                        &event.run_id,
                        "arcade-system",
                        &event.snapshot_path,
                    );
                    CatalogWorkerMessage::Timing {
                        name: "catalog_arcade_bootstrap_skipped".to_string(),
                        detail: format!("error={error}"),
                    }
                }
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
    let writer = Arc::new(Mutex::new(std::io::BufWriter::new(protocol_output)));
    let (heartbeat_stop, heartbeat_stop_rx) = mpsc::sync_channel(1);
    let heartbeat_run_id = mister_magik_catalog::catalog_lease::CatalogRunId::new();
    let wire_run_id = heartbeat_run_id.as_str().to_string();
    let progress = Arc::new(Mutex::new(CatalogHeartbeatProgress::new()));
    let wire_sequence = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let inner_progress_baseline = mister_magik_catalog::catalog_progress::inner_progress_units();
    {
        let mut output = writer.lock().unwrap_or_else(|error| error.into_inner());
        let mut event = blank_worker_wire_event("handshake");
        event.run_id.clone_from(&wire_run_id);
        event.detail = format!("operation={}", request.label());
        let _ = output.write_all(CATALOG_WORKER_PROTOCOL_PREFIX.as_bytes());
        let _ = write_worker_wire_event(&mut *output, &event);
    }
    let rx = start_library_catalog_worker_in_process(
        root,
        request,
        initial_cache,
        execution_mode,
        paths,
        archive_cache,
        Some(wire_run_id.clone()),
    );
    let heartbeat_writer = Arc::clone(&writer);
    let heartbeat_progress = Arc::clone(&progress);
    let heartbeat_sequence = Arc::clone(&wire_sequence);
    let heartbeat_wire_run_id = wire_run_id.clone();
    let heartbeat = std::thread::spawn(move || {
        let mut inner_progress_reported = 0_u64;
        while heartbeat_interval_elapsed(&heartbeat_stop_rx, std::time::Duration::from_secs(10)) {
            let inner_progress = mister_magik_catalog::catalog_progress::inner_progress_units()
                .saturating_sub(inner_progress_baseline);
            let snapshot = {
                let mut progress = heartbeat_progress
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if inner_progress > inner_progress_reported {
                    let delta = inner_progress - inner_progress_reported;
                    progress.advance("inner-work", progress.work_units.saturating_add(delta));
                    inner_progress_reported = inner_progress;
                }
                progress.clone()
            };
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
                collection: String::new(),
                collection_index: 0,
                collection_chunks: 0,
                collection_items_total: 0,
                collection_items: Vec::new(),
                collection_checksum: String::new(),
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
        let events = match &message {
            CatalogWorkerMessage::PublishedRegistrySeed { transport } => {
                match write_registry_seed_snapshot(&wire_run_id, transport) {
                    Ok((snapshot_path, snapshot_sha256)) => {
                        let mut event = blank_worker_wire_event("ready");
                        event.source = "sharded-registry-snapshot".to_string();
                        event.generation = transport.generation;
                        event.fingerprint = transport.fingerprint.clone();
                        event.snapshot_path = snapshot_path;
                        event.snapshot_sha256 = snapshot_sha256;
                        Ok(vec![event])
                    }
                    Err(error) => {
                        let mut event = blank_worker_wire_event("persistence-failed");
                        event.error = format!("write registry seed snapshot: {error}");
                        terminal = true;
                        Ok(vec![event])
                    }
                }
            }
            CatalogWorkerMessage::ArcadeBootstrapReady {
                snapshot_path,
                snapshot_sha256,
                load_us,
            } => {
                let mut event = blank_worker_wire_event("ready");
                event.source = CatalogSource::NavigationProjection.label().to_string();
                event.durable_save_pending = true;
                event.elapsed_us = *load_us;
                event.snapshot_path = snapshot_path.clone();
                event.snapshot_sha256 = snapshot_sha256.clone();
                Ok(vec![event])
            }
            _ => worker_wire_events(&message),
        };
        let events = match events {
            Ok(events) => events,
            Err(error) => {
                terminal = true;
                let mut event = blank_worker_wire_event("persistence-failed");
                event.error = error;
                vec![event]
            }
        };
        let mut output = writer.lock().unwrap_or_else(|error| error.into_inner());
        for mut event in events {
            event.run_id.clone_from(&wire_run_id);
            event.sequence = wire_sequence.fetch_add(1, Ordering::Relaxed);
            if output
                .write_all(CATALOG_WORKER_PROTOCOL_PREFIX.as_bytes())
                .is_err()
                || !write_worker_wire_event(&mut *output, &event)
            {
                terminal = true;
                break;
            }
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
            collection: String::new(),
            collection_index: 0,
            collection_chunks: 0,
            collection_items_total: 0,
            collection_items: Vec::new(),
            collection_checksum: String::new(),
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
    bootstrap_run_id: Option<&str>,
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
        run_fast_catalog_fresh_build(
            root,
            catalog_root,
            tx,
            catalog_profile,
            mutation_lease,
            bootstrap_run_id,
        );
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
    bootstrap_run_id: Option<&str>,
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
                if let Some(run_id) = bootstrap_run_id {
                    match write_arcade_system_snapshot(run_id, system) {
                        Ok((snapshot_path, snapshot_sha256)) => {
                            let _ = tx.send(CatalogWorkerMessage::ArcadeBootstrapReady {
                                snapshot_path,
                                snapshot_sha256,
                                load_us,
                            });
                        }
                        Err(error) => {
                            let _ = tx.send(CatalogWorkerMessage::Timing {
                                name: "catalog_arcade_bootstrap_skipped".to_string(),
                                detail: format!("error={error}"),
                            });
                        }
                    }
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
        snapshot_path: String,
        snapshot_sha256: String,
        load_us: u64,
    },
    PublishedRegistrySeed {
        transport: Box<RegistrySeedTransport>,
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
    fn every_catalog_request_is_supervised_outside_the_child() {
        let requests = [
            CatalogWorkerRequest::LoadOnly,
            CatalogWorkerRequest::StrictLoad,
            CatalogWorkerRequest::CheckStamp,
            CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
            CatalogWorkerRequest::RECONCILE_ALL_SYSTEMS,
            CatalogWorkerRequest::FreshBuild,
        ];
        for request in requests {
            assert!(should_supervise_catalog_worker(request, false));
            assert!(!should_supervise_catalog_worker(request, true));
        }
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
            collection: String::new(),
            collection_index: 0,
            collection_chunks: 0,
            collection_items_total: 0,
            collection_items: Vec::new(),
            collection_checksum: String::new(),
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
    fn heartbeat_progress_remains_monotonic_across_inner_work_checkpoints() {
        let mut progress = CatalogHeartbeatProgress::new();
        progress.advance("systems", 4);
        progress.advance("inner-work", 1);
        assert_eq!(progress.phase, "inner-work");
        assert!(progress.work_units >= 4);
        let inner_units = progress.work_units;
        progress.advance("artifacts", 1);
        assert_eq!(progress.phase, "artifacts");
        assert!(progress.work_units >= inner_units);
        assert!(progress.progress_epoch >= 2);
    }

    #[test]
    fn protocol_collection_chunks_reassemble_large_plan_without_oversized_lines() {
        let system_ids = (0..4096)
            .map(|index| format!("system-{index:04}"))
            .collect::<Vec<_>>();
        let message = CatalogWorkerMessage::ReconciliationPlanReady {
            system_ids: system_ids.clone(),
            all_published_systems: true,
        };
        let events = worker_wire_events(&message).unwrap();
        assert!(events.len() > 1);
        assert!(events.iter().all(|event| {
            serde_json::to_vec(event).unwrap().len() + CATALOG_WORKER_PROTOCOL_PREFIX.len() + 1
                <= MAX_CATALOG_WORKER_PROTOCOL_LINE_BYTES as usize
        }));

        let mut state = CatalogWorkerProtocolState::default();
        let mut handshake = blank_worker_wire_event("handshake");
        handshake.run_id = "run-chunks".to_string();
        assert_eq!(state.validate(&handshake), Ok(true));
        let mut result = None;
        for (sequence, mut event) in events.into_iter().enumerate() {
            event.run_id = "run-chunks".to_string();
            event.sequence = sequence as u64 + 1;
            assert_eq!(state.validate(&event), Ok(false));
            result = state.collect_collection(&event).unwrap().or(result);
        }
        assert!(matches!(
            result,
            Some(CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids: actual,
                all_published_systems: true,
            }) if actual == system_ids
        ));
    }

    #[test]
    fn protocol_collection_chunks_reassemble_manifest_and_reject_reordering() {
        let message = CatalogWorkerMessage::ManifestPublished {
            generation: 17,
            rebuilt: vec!["arcade".to_string(), "amiga".to_string()],
            removed: vec!["dos".to_string()],
        };
        let events = worker_wire_events(&message).unwrap();
        let mut state = CatalogWorkerProtocolState::default();
        let mut handshake = blank_worker_wire_event("handshake");
        handshake.run_id = "run-manifest".to_string();
        assert_eq!(state.validate(&handshake), Ok(true));
        let mut result = None;
        for (sequence, mut event) in events.into_iter().enumerate() {
            event.run_id = "run-manifest".to_string();
            event.sequence = sequence as u64 + 1;
            assert_eq!(state.validate(&event), Ok(false));
            result = state.collect_collection(&event).unwrap().or(result);
        }
        assert!(matches!(
            result,
            Some(CatalogWorkerMessage::ManifestPublished {
                generation: 17,
                rebuilt,
                removed,
            }) if rebuilt == vec!["arcade", "amiga"] && removed == vec!["dos"]
        ));

        let mut malformed = worker_wire_events(&CatalogWorkerMessage::ReconciliationPlanReady {
            system_ids: vec!["one".to_string(), "two".to_string()],
            all_published_systems: false,
        })
        .unwrap();
        malformed[0].run_id = "run-bad".to_string();
        malformed[0].sequence = 1;
        let mut malformed_state = CatalogWorkerProtocolState::default();
        let mut bad_handshake = blank_worker_wire_event("handshake");
        bad_handshake.run_id = "run-bad".to_string();
        assert_eq!(malformed_state.validate(&bad_handshake), Ok(true));
        malformed[0].collection_checksum = "0".repeat(64);
        assert_eq!(malformed_state.validate(&malformed[0]), Ok(false));
        assert!(malformed_state.collect_collection(&malformed[0]).is_err());
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
    fn corrupt_arcade_bootstrap_snapshot_is_skipped_without_failing_the_build() {
        let snapshot_root = std::env::temp_dir().join(format!(
            "mister-magik-catalog-worker-corrupt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let run_id = "run-corrupt";
        let bytes = b"not-a-fast-system";
        let (path, checksum) = write_catalog_worker_snapshot_at(
            &snapshot_root,
            run_id,
            "arcade-system",
            bytes,
            mister_magik_catalog::fast_five_catalog::MAX_FAST_SYSTEM_TRANSPORT_BYTES,
        )
        .unwrap();
        let mut event = blank_worker_wire_event("ready");
        event.run_id = run_id.to_string();
        event.source = CatalogSource::NavigationProjection.label().to_string();
        event.snapshot_path = path.clone();
        event.snapshot_sha256 = checksum;

        let message = catalog_worker_message_from_wire_at(
            &snapshot_root,
            event,
            "/media/fat/_Arcade",
            Path::new("/tmp/catalog-fast-v1"),
        )
        .unwrap()
        .expect("bootstrap skip event");

        assert!(matches!(
            message,
            CatalogWorkerMessage::Timing { name, .. }
                if name == "catalog_arcade_bootstrap_skipped"
        ));
        assert!(!Path::new(&path).exists());
        std::fs::remove_dir_all(snapshot_root).unwrap();
    }

    #[test]
    fn registry_seed_transport_round_trips_without_catalog_storage_reads() {
        let transport = RegistrySeedTransport {
            generation: 41,
            fingerprint: "fingerprint-41".to_string(),
            systems: vec![
                RegistrySeedSystem {
                    system_id: "arcade".to_string(),
                    display_title: "Arcade".to_string(),
                    games: 120,
                },
                RegistrySeedSystem {
                    system_id: "computer".to_string(),
                    display_title: "Computer".to_string(),
                    games: 7,
                },
            ],
        };

        let decoded = RegistrySeedTransport::decode(&transport.encode().unwrap()).unwrap();
        let catalog = decoded.into_catalog("/unavailable/catalog");

        assert_eq!(catalog.systems.len(), 2);
        assert_eq!(catalog.systems[0].id, "arcade");
        assert_eq!(catalog.systems[0].count, 120);
        assert_eq!(catalog.systems[1].id, "computer");
        assert_eq!(catalog.systems[1].count, 7);
        assert!(catalog.games.is_empty());
        assert_eq!(catalog.root, Path::new("/unavailable/catalog"));
    }

    #[test]
    fn registry_seed_snapshot_is_bounded_verified_and_consumed() {
        let snapshot_root = std::env::temp_dir().join(format!(
            "mister-magik-catalog-registry-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let run_id = "run-registry-test";
        let transport = RegistrySeedTransport {
            generation: 9,
            fingerprint: "registry-fingerprint".to_string(),
            systems: vec![RegistrySeedSystem {
                system_id: "arcade".to_string(),
                display_title: "Arcade".to_string(),
                games: 3,
            }],
        };
        let bytes = transport.encode().unwrap();
        let (path, checksum) = write_catalog_worker_snapshot_at(
            &snapshot_root,
            run_id,
            CATALOG_WORKER_REGISTRY_SNAPSHOT_SUFFIX,
            &bytes,
            CATALOG_WORKER_REGISTRY_SNAPSHOT_BYTES,
        )
        .unwrap();
        let mut event = blank_worker_wire_event("ready");
        event.run_id = run_id.to_string();
        event.generation = transport.generation;
        event.fingerprint = transport.fingerprint.clone();
        event.snapshot_path = path.clone();
        event.snapshot_sha256 = checksum;

        let (catalog, generation, fingerprint) =
            load_registry_seed_snapshot_at(&snapshot_root, &event, "/unavailable/catalog").unwrap();

        assert_eq!(generation, 9);
        assert_eq!(fingerprint, "registry-fingerprint");
        assert_eq!(catalog.systems[0].count, 3);
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
            watchdog_terminal: Arc::new(Mutex::new(None)),
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
            watchdog_terminal: Arc::new(Mutex::new(None)),
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
            watchdog_terminal: Arc::new(Mutex::new(None)),
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

    #[cfg(unix)]
    #[test]
    fn duplicated_protocol_fd_is_close_on_exec() {
        let source = std::fs::File::open("/dev/null").expect("open protocol source");
        let duplicated = duplicate_protocol_fd(source.as_raw_fd());
        let duplicated = duplicated.expect("duplicate protocol source");
        let flags = unsafe { libc::fcntl(duplicated, libc::F_GETFD) };
        assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
        unsafe { libc::close(duplicated) };
    }

    #[cfg(unix)]
    #[test]
    fn duplicated_protocol_fd_does_not_cross_exec() {
        let source = std::fs::File::open("/dev/null").expect("open protocol source");
        let duplicated =
            duplicate_protocol_fd(source.as_raw_fd()).expect("duplicate protocol source");
        let fd = duplicated.to_string();
        let status = Command::new("sh")
            .arg("-c")
            .arg("eval 'printf x >&$1'")
            .arg("catalog-fd-check")
            .arg(&fd)
            .status()
            .expect("exec fd-check helper");
        unsafe { libc::close(duplicated) };
        assert!(!status.success());
    }

    #[test]
    fn watchdog_failure_is_visible_while_protocol_reader_is_blocked() {
        let (_, event_rx) = mpsc::sync_channel(1);
        let (_, terminal_rx) = mpsc::sync_channel(1);
        let watchdog_terminal = Arc::new(Mutex::new(None));
        let receiver = CatalogWorkerReceiver::Process {
            events: event_rx,
            terminal: terminal_rx,
            pending_terminal: Mutex::new(None),
            watchdog_terminal: Arc::clone(&watchdog_terminal),
        };
        let control = CatalogChildControl {
            child: Mutex::new(None),
            process_group: -1,
            handshake_seen: AtomicBool::new(false),
            reaped: AtomicBool::new(false),
            watchdog_terminal,
        };

        control.fail_and_terminate("watchdog test failure");

        assert!(matches!(
            receiver.try_recv(),
            Ok(CatalogWorkerMessage::PersistenceFailed { error })
                if error == "watchdog test failure"
        ));
    }
}
