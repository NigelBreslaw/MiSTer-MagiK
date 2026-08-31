// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::preview_state::SystemEntryPreviewPrelude;
use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SystemEntryThreadSnapshot {
    cpu: i32,
    thread_cpu_us: u64,
    minor_page_faults: u64,
    major_page_faults: u64,
}

#[cfg(target_os = "linux")]
fn system_entry_thread_snapshot() -> SystemEntryThreadSnapshot {
    let mut cpu_time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: cpu_time points to writable timespec storage.
    let cpu_time_available =
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut cpu_time) } == 0;
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: usage points to writable rusage storage.
    let usage_available = unsafe { libc::getrusage(libc::RUSAGE_THREAD, usage.as_mut_ptr()) } == 0;
    // SAFETY: successful getrusage initialized usage; zero is valid for every integer field on
    // the failure path.
    let usage = unsafe { usage.assume_init() };
    // SAFETY: sched_getcpu has no pointer arguments or retained state.
    let cpu = unsafe { libc::sched_getcpu() };
    SystemEntryThreadSnapshot {
        cpu,
        thread_cpu_us: cpu_time_available
            .then(|| {
                u64::try_from(cpu_time.tv_sec)
                    .unwrap_or(0)
                    .saturating_mul(1_000_000)
                    .saturating_add(u64::try_from(cpu_time.tv_nsec).unwrap_or(0) / 1_000)
            })
            .unwrap_or(0),
        minor_page_faults: usage_available
            .then(|| u64::try_from(usage.ru_minflt).unwrap_or(0))
            .unwrap_or(0),
        major_page_faults: usage_available
            .then(|| u64::try_from(usage.ru_majflt).unwrap_or(0))
            .unwrap_or(0),
    }
}

#[cfg(not(target_os = "linux"))]
fn system_entry_thread_snapshot() -> SystemEntryThreadSnapshot {
    SystemEntryThreadSnapshot {
        cpu: -1,
        ..SystemEntryThreadSnapshot::default()
    }
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

pub(super) const CATALOG_MESSAGES_PER_FRAME: usize = 2;
pub(super) const MEDIA_MESSAGES_PER_FRAME: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CatalogPollScope {
    /// Drain every catalog-adjacent result while the launcher is idle.
    Idle,
    /// Keep the primary catalog control channel live during ordinary input and navigation.
    Interactive { system_entry_handoff: bool },
    /// Keep only catalog control and an optional foreground entry live while CPU1 is owned.
    Transition { system_entry_handoff: bool },
}

#[derive(Default)]
pub(super) struct CatalogJobEventBuf {
    events: Vec<CatalogWorkerMessage>,
}

impl CatalogJobEventBuf {
    pub(super) fn new() -> Self {
        Self {
            events: Vec::with_capacity(8),
        }
    }

    pub(super) fn clear(&mut self) {
        self.events.clear();
    }

    fn push(&mut self, event: CatalogWorkerMessage) {
        self.events.push(event);
    }

    pub(super) fn drain(&mut self) -> impl Iterator<Item = CatalogWorkerMessage> + '_ {
        self.events.drain(..)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.events.len()
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.events.capacity()
    }
}

#[derive(Default)]
pub(super) struct MediaJobEventBuf {
    events: Vec<MediaWorkerMessage>,
}

impl MediaJobEventBuf {
    pub(super) fn new() -> Self {
        Self {
            events: Vec::with_capacity(8),
        }
    }

    pub(super) fn clear(&mut self) {
        self.events.clear();
    }

    fn push(&mut self, event: MediaWorkerMessage) {
        self.events.push(event);
    }

    pub(super) fn drain(&mut self) -> impl Iterator<Item = MediaWorkerMessage> + '_ {
        self.events.drain(..)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.events.len()
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.events.capacity()
    }
}

enum CatalogJobState {
    Idle,
    Running(CatalogWorkerReceiver),
}

enum SearchQueryJobState {
    Idle,
    Running(mpsc::Receiver<CatalogWorkerMessage>),
}

enum SystemShardJobState {
    Idle,
    Running {
        system_id: String,
        generation: Option<String>,
        sequence: u64,
    },
}

struct SystemShardRequest {
    system_id: String,
    reason: &'static str,
    requested_at: Instant,
    base_catalog: ArcadeCatalog,
    base_catalog_version: usize,
    preview: Option<SystemEntryPreviewDispatch>,
}

pub(super) struct SystemEntryPreviewDispatch {
    pub(super) generation: u64,
    pub(super) requests: mister_magik_catalog::preview_worker::PreviewSelectedRequestHandle,
}

struct SystemEntryPrepareRequest {
    sequence: u64,
    generation: Option<String>,
    request: SystemShardRequest,
}

enum SystemEntryPrepareCommand {
    Prepare(SystemEntryPrepareRequest),
    RetireOutcome(SystemEntryPrepareOutcome),
    RetireCatalog(ArcadeCatalog),
    OpenGeneration {
        generation: Option<String>,
        reply: mpsc::SyncSender<Result<u64, String>>,
    },
}

struct PreparedSystemEntry {
    sequence: u64,
    generation: Option<String>,
    system_id: String,
    catalog: ArcadeCatalog,
    base_catalog_version: usize,
    game_count: usize,
    prepare_us: u64,
    profile: SystemEntryCatalogProfile,
    preview_prelude: Option<SystemEntryPreviewPrelude>,
}

struct FailedSystemEntry {
    sequence: u64,
    generation: Option<String>,
    system_id: String,
    error: String,
}

enum SystemEntryPrepareOutcome {
    Prepared(PreparedSystemEntry),
    Failed(FailedSystemEntry),
}

impl SystemEntryPrepareOutcome {
    fn sequence(&self) -> u64 {
        match self {
            Self::Prepared(entry) => entry.sequence,
            Self::Failed(failure) => failure.sequence,
        }
    }

    fn generation(&self) -> Option<&str> {
        match self {
            Self::Prepared(entry) => entry.generation.as_deref(),
            Self::Failed(failure) => failure.generation.as_deref(),
        }
    }

    fn into_message(self) -> CatalogWorkerMessage {
        match self {
            Self::Prepared(entry) => CatalogWorkerMessage::SystemShardReady {
                system_id: entry.system_id,
                catalog: entry.catalog,
                base_catalog_version: entry.base_catalog_version,
                game_count: entry.game_count,
                prepare_us: entry.prepare_us,
                profile: entry.profile,
                preview_prelude: entry.preview_prelude,
            },
            Self::Failed(failure) => CatalogWorkerMessage::SystemShardFailed {
                system_id: failure.system_id,
                error: failure.error,
            },
        }
    }
}

#[derive(Default)]
struct PreparedSystemEntryMailbox {
    newest: Mutex<Option<SystemEntryPrepareOutcome>>,
}

impl PreparedSystemEntryMailbox {
    fn publish(&self, outcome: SystemEntryPrepareOutcome) {
        let mut newest = self
            .newest
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if newest
            .as_ref()
            .is_some_and(|current| current.sequence() > outcome.sequence())
        {
            return;
        }
        *newest = Some(outcome);
    }

    fn try_take(&self) -> Option<SystemEntryPrepareOutcome> {
        match self.newest.try_lock() {
            Ok(mut newest) => newest.take(),
            Err(std::sync::TryLockError::WouldBlock) => None,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner().take(),
        }
    }
}

struct SystemEntryPrepareWorker {
    requests: mpsc::Sender<SystemEntryPrepareCommand>,
    results: Arc<PreparedSystemEntryMailbox>,
    liveness: mpsc::Receiver<()>,
}

impl SystemEntryPrepareWorker {
    fn start(catalog_paths: mister_magik_catalog::device_layout::CatalogPaths) -> Option<Self> {
        let (request_tx, request_rx) = mpsc::channel::<SystemEntryPrepareCommand>();
        let results = Arc::new(PreparedSystemEntryMailbox::default());
        let worker_results = Arc::clone(&results);
        let (liveness_tx, liveness_rx) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("system-entry-prepare".to_string())
            .spawn(move || {
                let _liveness = liveness_tx;
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::SystemEntryPrepare,
                );
                let mut warmed_generation = None;
                while let Ok(command) = request_rx.recv() {
                    match command {
                        SystemEntryPrepareCommand::Prepare(work) => {
                            let _lease = mister_magik_catalog::work_coordinator::foreground(
                                "system-entry-prepare",
                            );
                            let reader = warmed_generation
                                .as_ref()
                                .filter(|(generation, _)| {
                                    warmed_generation_matches(generation, &work.generation)
                                })
                                .map(|(_, reader)| reader);
                            let message = match reader {
                                Some(reader) => prepare_system_shard(work.request, reader),
                                None => CatalogWorkerMessage::SystemShardFailed {
                                    system_id: work.request.system_id,
                                    error: "system-entry NavPack reader is not open for the active generation"
                                        .to_string(),
                                },
                            };
                            worker_results.publish(system_entry_prepare_outcome(
                                work.sequence,
                                work.generation,
                                message,
                            ));
                        }
                        SystemEntryPrepareCommand::RetireOutcome(outcome) => drop(outcome),
                        SystemEntryPrepareCommand::RetireCatalog(catalog) => drop(catalog),
                        SystemEntryPrepareCommand::OpenGeneration { generation, reply } => {
                            let started = Instant::now();
                            let result = mister_magik_catalog::lazy_sharded_reader::LazyShardedCatalogReader::open(
                                catalog_paths.sharded_catalog_dir(),
                                mister_magik_catalog::shard_registry::production_registry_limits(),
                            )
                            .map_err(|error| error.to_string())
                            .map(|reader| {
                                warmed_generation = Some((generation, reader));
                                started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
                            });
                            let _ = reply.send(result);
                        }
                    }
                }
            })
            .ok()?;
        Some(Self {
            requests: request_tx,
            results,
            liveness: liveness_rx,
        })
    }
}

fn warmed_generation_matches(warmed: &Option<String>, requested: &Option<String>) -> bool {
    warmed.is_some() && warmed == requested
}

fn system_entry_prepare_outcome(
    sequence: u64,
    generation: Option<String>,
    message: CatalogWorkerMessage,
) -> SystemEntryPrepareOutcome {
    match message {
        CatalogWorkerMessage::SystemShardReady {
            system_id,
            catalog,
            base_catalog_version,
            game_count,
            prepare_us,
            profile,
            preview_prelude,
        } => SystemEntryPrepareOutcome::Prepared(PreparedSystemEntry {
            sequence,
            generation,
            system_id,
            catalog,
            base_catalog_version,
            game_count,
            prepare_us,
            profile,
            preview_prelude,
        }),
        CatalogWorkerMessage::SystemShardFailed { system_id, error } => {
            SystemEntryPrepareOutcome::Failed(FailedSystemEntry {
                sequence,
                generation,
                system_id,
                error,
            })
        }
        _ => unreachable!("system-entry preparation produced a non-terminal message"),
    }
}

fn publish_prepared_system_collection(
    base_catalog: &ArcadeCatalog,
    requested_collection_id: &str,
    artifact_system_id: &str,
    collection: Arc<arcade_catalog::SystemCollection>,
) -> ArcadeCatalog {
    let mut catalog = base_catalog.with_system_collection_for_id(
        requested_collection_id.to_string(),
        Arc::clone(&collection),
    );
    if artifact_system_id == "arcade" {
        for alias in ["arcade", arcade_catalog::MENU_ARCADE_SYSTEM_ID] {
            if alias != requested_collection_id {
                catalog = catalog.with_system_collection_for_id(alias, Arc::clone(&collection));
            }
        }
    }
    catalog
}

fn prepare_system_shard(
    request: SystemShardRequest,
    reader: &mister_magik_catalog::lazy_sharded_reader::LazyShardedCatalogReader,
) -> CatalogWorkerMessage {
    let load_started = Instant::now();
    let worker_system_id = request.system_id;
    let base_catalog = request.base_catalog;
    let base_catalog_version = request.base_catalog_version;
    let preview_dispatch = request.preview;
    let artifact_system_id = worker_system_id
        .strip_prefix("menu:")
        .unwrap_or(&worker_system_id)
        .to_string();
    let execution_started = system_entry_thread_snapshot();
    crate::allocation_metrics::begin();
    let result: Result<
        (
            arcade_catalog::SystemCollection,
            usize,
            mister_magik_catalog::lazy_sharded_reader::LazySystemOpenTiming,
        ),
        mister_magik_catalog::sharded_catalog::CatalogError,
    > = (|| {
        let parsed = mister_magik_catalog::catalog_classify::SystemId::parse(&artifact_system_id)
            .map_err(|error| {
            mister_magik_catalog::sharded_catalog::CatalogError::new(
                "open-system",
                error.to_string(),
            )
        })?;
        let descriptor_started = Instant::now();
        let descriptor_pmu =
            mister_magik_perf_events::sampled_span("system-entry-descriptor-lookup");
        let generation = reader.active_system_generation(&parsed)?;
        drop(descriptor_pmu);
        let descriptor_lookup_us = elapsed_us(descriptor_started);
        let (collection, navpack) = if let Some(navpack_path) = generation.navpack_path {
            let navpack_pmu = mister_magik_perf_events::sampled_span("system-entry-navpack-open");
            let opened = arcade_catalog::SystemCollection::open_navpack(
                artifact_system_id.as_str(),
                &navpack_path,
                generation.navpack_bytes,
                generation.generation,
                generation.games,
                base_catalog.platform_kind(&artifact_system_id),
            )
            .map_err(|error| {
                mister_magik_catalog::sharded_catalog::CatalogError::new("open-system", error)
            })?;
            drop(navpack_pmu);
            opened
        } else {
            (
                arcade_catalog::SystemCollection::new(
                    artifact_system_id.as_str(),
                    Vec::new(),
                    Vec::new(),
                    base_catalog.platform_kind(&artifact_system_id),
                ),
                Default::default(),
            )
        };
        Ok((
            collection,
            generation.games,
            mister_magik_catalog::lazy_sharded_reader::LazySystemOpenTiming {
                descriptor_lookup_us,
                navpack: Some(navpack),
                ..Default::default()
            },
        ))
    })();
    let navpack_open_us = load_started.elapsed().as_micros();
    let message = match result {
        Ok((collection, game_count, open_timing)) => {
            let prepare_started = Instant::now();
            let preview_prelude = preview_dispatch.and_then(|dispatch| {
                let game = collection.game_at(0).filter(|game| {
                    game.has_preview
                        && !game.preview_archive_path.is_empty()
                        && !game.preview_asset_key.is_empty()
                })?;
                dispatch
                    .requests
                    .publish_reserved(
                        dispatch.generation,
                        game.title.to_string(),
                        game.preview_archive_path.to_string(),
                        game.preview_asset_key.to_string(),
                    )
                    .then(|| SystemEntryPreviewPrelude {
                        generation: dispatch.generation,
                        title: game.title.to_string(),
                        preview_archive_path: game.preview_archive_path.to_string(),
                        preview_asset_key: game.preview_asset_key.to_string(),
                    })
            });
            let replacement_started = Instant::now();
            let replacement_pmu =
                mister_magik_perf_events::sampled_span("system-entry-catalog-replacement");
            let catalog = publish_prepared_system_collection(
                &base_catalog,
                &worker_system_id,
                &artifact_system_id,
                Arc::new(collection),
            );
            drop(replacement_pmu);
            let catalog_replacement_us = elapsed_us(replacement_started);
            let allocations = crate::allocation_metrics::finish();
            let execution_finished = system_entry_thread_snapshot();
            mister_magik_perf_events::submit_thread_profile("system-entry-catalog");
            CatalogWorkerMessage::SystemShardReady {
                system_id: worker_system_id.clone(),
                catalog,
                base_catalog_version,
                game_count,
                prepare_us: elapsed_us(prepare_started),
                profile: SystemEntryCatalogProfile {
                    open: open_timing,
                    catalog_replacement_us,
                    total_wall_us: elapsed_us(load_started),
                    thread_cpu_us: execution_finished
                        .thread_cpu_us
                        .saturating_sub(execution_started.thread_cpu_us),
                    cpu_start: execution_started.cpu,
                    cpu_end: execution_finished.cpu,
                    minor_page_faults: execution_finished
                        .minor_page_faults
                        .saturating_sub(execution_started.minor_page_faults),
                    major_page_faults: execution_finished
                        .major_page_faults
                        .saturating_sub(execution_started.major_page_faults),
                    allocations: allocations.allocations,
                    allocated_bytes: allocations.bytes,
                },
                preview_prelude,
            }
        }
        Err(error) => {
            let _ = crate::allocation_metrics::finish();
            mister_magik_perf_events::submit_thread_profile("system-entry-catalog");
            CatalogWorkerMessage::SystemShardFailed {
                system_id: worker_system_id.clone(),
                error: error.to_string(),
            }
        }
    };
    crate::ui_logln!(
        "catalog_system_shard_load_finish system={} status={} load_us={}",
        worker_system_id,
        if matches!(&message, CatalogWorkerMessage::SystemShardReady { .. }) {
            "ready"
        } else {
            "failed"
        },
        load_started.elapsed().as_micros()
    );
    crate::ui_logln!(
        "catalog_system_navpack_open system={} open_us={}",
        worker_system_id,
        navpack_open_us
    );
    message
}

enum MediaJobState {
    Idle,
    Running(MediaWorkerHandle),
    Unavailable,
}

pub(super) struct LauncherScheduler {
    catalog_paths: mister_magik_catalog::device_layout::CatalogPaths,
    archive_cache: mister_magik_catalog::catalog_config::ArchiveCacheConfig,
    media_config: Result<MediaWorkerConfig, String>,
    catalog: CatalogJobState,
    catalog_child_control: Option<Arc<CatalogChildControl>>,
    catalog_stop_requested: bool,
    catalog_progress_work_units: u64,
    catalog_progress: crate::catalog_progress_report::CatalogProgressMonitor,
    search_query: SearchQueryJobState,
    pending_search_query: Option<launcher::ArcadeSearchRequest>,
    search_catalog: Arc<
        Mutex<
            Option<(
                usize,
                mister_magik_catalog::persisted_search::PersistedSearchCatalog,
            )>,
        >,
    >,
    system_shard: SystemShardJobState,
    system_shard_attempted: BTreeSet<String>,
    system_shard_queue: VecDeque<SystemShardRequest>,
    system_shard_generation: Option<String>,
    next_system_entry_sequence: u64,
    system_entry_prepare: Option<SystemEntryPrepareWorker>,
    media: MediaJobState,
    launch_handoff: LaunchHandoffSession,
}

impl LauncherScheduler {
    pub(super) fn new(launch_handoff_bench_enabled: bool) -> Self {
        let paths = mister_magik_catalog::device_layout::CatalogPaths::capture_process();
        let archive_cache =
            mister_magik_catalog::catalog_config::ArchiveCacheConfig::capture_process(&paths);
        Self::with_catalog_config(launch_handoff_bench_enabled, paths, archive_cache)
    }

    pub(super) fn with_catalog_config(
        launch_handoff_bench_enabled: bool,
        catalog_paths: mister_magik_catalog::device_layout::CatalogPaths,
        archive_cache: mister_magik_catalog::catalog_config::ArchiveCacheConfig,
    ) -> Self {
        let media_config = MediaWorkerConfig::capture_process(&catalog_paths);
        Self::with_runtime_config(
            launch_handoff_bench_enabled,
            catalog_paths,
            archive_cache,
            media_config,
            None,
        )
    }

    pub(super) fn with_runtime_config(
        launch_handoff_bench_enabled: bool,
        catalog_paths: mister_magik_catalog::device_layout::CatalogPaths,
        archive_cache: mister_magik_catalog::catalog_config::ArchiveCacheConfig,
        media_config: Result<MediaWorkerConfig, String>,
        launch_return_pmu_handoff_out: Option<String>,
    ) -> Self {
        let now = Instant::now();
        Self {
            catalog_paths: catalog_paths.clone(),
            archive_cache,
            media_config,
            catalog: CatalogJobState::Idle,
            catalog_child_control: None,
            catalog_stop_requested: false,
            catalog_progress_work_units: 0,
            catalog_progress: crate::catalog_progress_report::CatalogProgressMonitor::new(now),
            search_query: SearchQueryJobState::Idle,
            pending_search_query: None,
            search_catalog: Arc::new(Mutex::new(None)),
            system_shard: SystemShardJobState::Idle,
            system_shard_attempted: BTreeSet::new(),
            system_shard_queue: VecDeque::new(),
            system_shard_generation: None,
            next_system_entry_sequence: 1,
            system_entry_prepare: SystemEntryPrepareWorker::start(catalog_paths),
            media: MediaJobState::Idle,
            launch_handoff: LaunchHandoffSession::from_env(
                launch_handoff_bench_enabled,
                launch_return_pmu_handoff_out.as_deref(),
            ),
        }
    }

    pub(super) fn catalog_worker_running(&self) -> bool {
        matches!(self.catalog, CatalogJobState::Running(_))
    }

    pub(super) fn catalog_worker_available(&self) -> bool {
        !self.catalog_worker_running()
            && self
                .catalog_child_control
                .as_ref()
                .is_none_or(|control| control.reaped())
    }

    pub(super) fn catalog_messages_running(&self) -> bool {
        self.catalog_worker_running()
            || matches!(self.search_query, SearchQueryJobState::Running(_))
            || matches!(self.system_shard, SystemShardJobState::Running { .. })
    }

    pub(super) fn system_shard_loading(&self, system_id: &str) -> bool {
        matches!(
            &self.system_shard,
            SystemShardJobState::Running {
                system_id: active,
                ..
            } if active == system_id
        )
    }

    pub(super) fn system_entry_prepare_active(&self) -> bool {
        matches!(self.system_shard, SystemShardJobState::Running { .. })
    }

    pub(super) fn system_shard_attempted(&self, system_id: &str) -> bool {
        self.system_shard_attempted.contains(system_id)
    }

    pub(super) fn open_system_entry_reader(&self) -> Result<u64, String> {
        let worker = self
            .system_entry_prepare
            .as_ref()
            .ok_or_else(|| "system-entry prepare worker is unavailable".to_string())?;
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        worker
            .requests
            .send(SystemEntryPrepareCommand::OpenGeneration {
                generation: self.system_shard_generation.clone(),
                reply: reply_tx,
            })
            .map_err(|_| "system-entry prepare worker disconnected".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "system-entry reader open timed out".to_string())?
    }

    pub(super) fn request_system_shard(
        &mut self,
        system_id: String,
        reason: &'static str,
        base_catalog: ArcadeCatalog,
        base_catalog_version: usize,
        now: Instant,
    ) -> bool {
        self.request_system_shard_with_preview(
            system_id,
            reason,
            base_catalog,
            base_catalog_version,
            now,
            None,
        )
    }

    pub(super) fn request_system_shard_with_preview(
        &mut self,
        system_id: String,
        reason: &'static str,
        base_catalog: ArcadeCatalog,
        base_catalog_version: usize,
        now: Instant,
        preview: Option<SystemEntryPreviewDispatch>,
    ) -> bool {
        if self.system_shard_generation.is_none() {
            return false;
        }
        if self.system_shard_attempted.contains(&system_id) {
            return false;
        }
        self.system_shard_attempted.insert(system_id.clone());
        self.system_shard_queue.push_back(SystemShardRequest {
            system_id,
            reason,
            requested_at: now,
            base_catalog,
            base_catalog_version,
            preview,
        });
        self.start_next_system_shard_load();
        true
    }

    pub(super) fn set_system_shard_generation(&mut self, generation: Option<&str>) -> bool {
        if self.system_shard_generation.as_deref() == generation {
            return false;
        }
        self.system_shard_generation = generation.map(str::to_string);
        self.system_shard_attempted.clear();
        self.system_shard_queue.clear();
        true
    }

    pub(super) fn retry_system_shard(
        &mut self,
        system_id: String,
        reason: &'static str,
        base_catalog: ArcadeCatalog,
        base_catalog_version: usize,
        now: Instant,
    ) -> bool {
        self.retry_system_shard_with_preview(
            system_id,
            reason,
            base_catalog,
            base_catalog_version,
            now,
            None,
        )
    }

    pub(super) fn retry_system_shard_with_preview(
        &mut self,
        system_id: String,
        reason: &'static str,
        base_catalog: ArcadeCatalog,
        base_catalog_version: usize,
        now: Instant,
        preview: Option<SystemEntryPreviewDispatch>,
    ) -> bool {
        if self.system_shard_loading(&system_id) {
            return false;
        }
        self.system_shard_attempted.remove(&system_id);
        self.system_shard_queue
            .retain(|request| request.system_id != system_id);
        self.request_system_shard_with_preview(
            system_id,
            reason,
            base_catalog,
            base_catalog_version,
            now,
            preview,
        )
    }

    fn start_next_system_shard_load(&mut self) {
        if matches!(self.system_shard, SystemShardJobState::Running { .. }) {
            return;
        }
        let Some(request) = self.system_shard_queue.pop_front() else {
            return;
        };
        let system_id = request.system_id.clone();
        crate::ui_logln!(
            "catalog_system_shard_load_start system={} reason={} queue_wait_us={}",
            system_id,
            request.reason,
            request.requested_at.elapsed().as_micros()
        );
        let generation = self.system_shard_generation.clone();
        let sequence = self.next_system_entry_sequence;
        self.next_system_entry_sequence = self.next_system_entry_sequence.wrapping_add(1).max(1);
        self.system_shard = SystemShardJobState::Running {
            system_id,
            generation: generation.clone(),
            sequence,
        };
        let dispatched = self.system_entry_prepare.as_ref().is_some_and(|worker| {
            worker
                .requests
                .send(SystemEntryPrepareCommand::Prepare(
                    SystemEntryPrepareRequest {
                        sequence,
                        generation,
                        request,
                    },
                ))
                .is_ok()
        });
        if !dispatched {
            self.system_shard = SystemShardJobState::Idle;
            self.start_next_system_shard_load();
        }
    }

    pub(super) fn retire_catalog(&self, catalog: ArcadeCatalog) {
        if let Some(worker) = &self.system_entry_prepare {
            if let Err(error) = worker
                .requests
                .send(SystemEntryPrepareCommand::RetireCatalog(catalog))
            {
                drop(error.0);
            }
        }
    }

    pub(super) fn request_arcade_search(&mut self, request: launcher::ArcadeSearchRequest) {
        self.pending_search_query = Some(request);
        self.start_next_arcade_search();
    }

    fn start_next_arcade_search(&mut self) {
        if matches!(self.search_query, SearchQueryJobState::Running(_)) {
            return;
        }
        let Some(request) = self.pending_search_query.take() else {
            return;
        };
        let worker_request = request.clone();
        let search_catalog = Arc::clone(&self.search_catalog);
        let storage = self.catalog_paths.sharded_catalog_dir().to_path_buf();
        let (tx, rx) = mpsc::channel();
        self.search_query = SearchQueryJobState::Running(rx);
        crate::ui_logln!(
            "search_query_tsv\tspawn\t{}\t{}",
            request.catalog_version,
            request.query
        );
        if std::thread::Builder::new()
            .name("catalog-search-query".to_string())
            .spawn(move || {
                let started = Instant::now();
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::CatalogWorker,
                );
                let result = cached_search_catalog(
                    &search_catalog,
                    worker_request.catalog_version,
                    &storage,
                )
                .and_then(|catalog| {
                    catalog.search(&worker_request.system_ids, &worker_request.query)
                });
                let message = match result {
                    Ok(result) => {
                        crate::ui_logln!(
                            "search_query_tsv\tfinish\t{}\t{}\t{}\t{}",
                            worker_request.catalog_version,
                            worker_request.query,
                            started.elapsed().as_micros(),
                            result.timing.total_us,
                        );
                        CatalogWorkerMessage::SearchQueryReady {
                            request: worker_request,
                            result,
                        }
                    }
                    Err(error) => CatalogWorkerMessage::SearchQueryFailed {
                        request: worker_request,
                        error: error.to_string(),
                    },
                };
                let _ = tx.send(message);
            })
            .is_err()
        {
            self.search_query = SearchQueryJobState::Idle;
            self.start_next_arcade_search();
        }
    }

    pub(super) fn start_catalog_worker(
        &mut self,
        root: String,
        request: CatalogWorkerRequest,
        initial_cache: CatalogWorkerInitialCache,
        execution_mode: CatalogExecutionMode,
    ) -> bool {
        if self.catalog_worker_running() {
            return false;
        }
        if let Some(control) = self.catalog_child_control.as_ref() {
            if !control.reaped() {
                return false;
            }
            self.catalog_child_control = None;
        }
        let evidence = self.catalog_progress.start(
            root.clone(),
            request.label(),
            execution_mode.label(),
            Instant::now(),
        );
        self.enqueue_catalog_progress(evidence);
        let (catalog_receiver, child_control) = start_library_catalog_worker(
            root,
            request,
            initial_cache,
            execution_mode,
            self.catalog_paths.clone(),
            self.archive_cache.clone(),
        );
        self.catalog_progress
            .note_worker_process(child_control.as_ref().and_then(|control| control.pid()));
        self.catalog_child_control = child_control;
        self.catalog_stop_requested = false;
        self.catalog_progress_work_units = 0;
        self.catalog = CatalogJobState::Running(catalog_receiver);
        true
    }

    pub(super) fn poll_catalog(
        &mut self,
        out: &mut CatalogJobEventBuf,
        scope: CatalogPollScope,
    ) -> bool {
        out.clear();
        if matches!(
            scope,
            CatalogPollScope::Interactive {
                system_entry_handoff: true
            } | CatalogPollScope::Transition {
                system_entry_handoff: true
            }
        ) {
            self.poll_system_entry_into(out);
        }

        let mut disconnected = false;
        while out.events.len() < CATALOG_MESSAGES_PER_FRAME {
            let received = match &self.catalog {
                CatalogJobState::Running(rx) => rx.try_recv(),
                CatalogJobState::Idle => break,
            };
            match received {
                Ok(message) => {
                    self.record_catalog_progress_message(&message, Instant::now());
                    if matches!(
                        message,
                        CatalogWorkerMessage::Done
                            | CatalogWorkerMessage::LoadFailed { .. }
                            | CatalogWorkerMessage::PersistenceFailed { .. }
                    ) {
                        self.catalog = CatalogJobState::Idle;
                        if self
                            .catalog_child_control
                            .as_ref()
                            .is_some_and(|control| control.reaped())
                        {
                            self.catalog_child_control = None;
                        }
                        self.catalog_stop_requested = false;
                    }
                    out.push(message);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if disconnected {
            if !self.catalog_worker_running() {
                // A terminal message may have transitioned the state to idle
                // just before the protocol reader dropped its sender.
            } else if let Some(control) = self.catalog_child_control.as_ref()
                && !control.reaped()
            {
                // Keep the job in the running state until the protocol reader
                // reaps the child and emits a terminal event. This prevents a
                // replacement builder from overlapping an unobserved child.
                let _ = control.terminate();
            } else {
                self.catalog = CatalogJobState::Idle;
                self.catalog_child_control = None;
                self.catalog_stop_requested = false;
                self.finish_catalog_progress(
                    "disconnected",
                    "catalog worker channel disconnected without a terminal message",
                );
                out.push(CatalogWorkerMessage::LoadFailed {
                    error: "catalog worker disconnected without a terminal message".to_string(),
                });
            }
        }
        if scope != CatalogPollScope::Idle {
            return disconnected;
        }

        let mut search_query_terminal = false;
        if let SearchQueryJobState::Running(rx) = &self.search_query {
            while out.events.len() < CATALOG_MESSAGES_PER_FRAME {
                match rx.try_recv() {
                    Ok(message) => {
                        search_query_terminal = matches!(
                            message,
                            CatalogWorkerMessage::SearchQueryReady { .. }
                                | CatalogWorkerMessage::SearchQueryFailed { .. }
                        );
                        out.push(message);
                        if search_query_terminal {
                            break;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        search_query_terminal = true;
                        break;
                    }
                }
            }
        }
        if search_query_terminal {
            self.search_query = SearchQueryJobState::Idle;
            self.start_next_arcade_search();
        }
        self.poll_system_entry_into(out);
        disconnected
    }

    fn poll_system_entry_into(&mut self, out: &mut CatalogJobEventBuf) {
        let mut shard_terminal = false;
        if matches!(self.system_shard, SystemShardJobState::Running { .. })
            && out.events.len() < CATALOG_MESSAGES_PER_FRAME
            && let Some(worker) = self.system_entry_prepare.as_ref()
        {
            if let Some(outcome) = worker.results.try_take() {
                shard_terminal = true;
                let outcome_is_current = matches!(
                    &self.system_shard,
                    SystemShardJobState::Running {
                        generation,
                        sequence,
                        ..
                    } if *sequence == outcome.sequence()
                        && generation.as_deref() == outcome.generation()
                        && generation == &self.system_shard_generation
                );
                if outcome_is_current {
                    out.push(outcome.into_message());
                } else if let Err(error) = worker
                    .requests
                    .send(SystemEntryPrepareCommand::RetireOutcome(outcome))
                {
                    drop(error.0);
                }
            } else if matches!(
                worker.liveness.try_recv(),
                Err(mpsc::TryRecvError::Disconnected)
            ) {
                shard_terminal = true;
                self.system_entry_prepare = None;
            }
        }
        if shard_terminal {
            self.system_shard = SystemShardJobState::Idle;
            self.start_next_system_shard_load();
        }
    }

    pub(super) fn tick_catalog_progress(&mut self, background_work_allowed: bool, now: Instant) {
        if let Some(evidence) =
            self.catalog_progress
                .tick(self.catalog_worker_running(), background_work_allowed, now)
        {
            self.enqueue_catalog_progress(evidence);
        }
        if self
            .catalog_progress
            .active_stalled(self.catalog_worker_running(), background_work_allowed)
            && !self.catalog_stop_requested
        {
            if let Some(control) = self.catalog_child_control.as_ref() {
                control.fail_and_terminate(
                    "catalog worker watchdog terminated a job after 120 seconds without validated progress",
                );
                self.catalog_stop_requested = true;
                self.catalog_progress.note_stall_cause(
                    "watchdog terminated the child after 120 seconds without validated progress",
                );
                self.note_catalog_progress(
                    "watchdog-stop",
                    "stalled",
                    "child terminated after 120 seconds without validated progress",
                    -1,
                    now,
                );
            }
        }
    }

    fn record_catalog_progress_message(&mut self, message: &CatalogWorkerMessage, now: Instant) {
        match message {
            CatalogWorkerMessage::Progress { .. } => {}
            CatalogWorkerMessage::Heartbeat {
                run_id,
                phase,
                progress_epoch,
                work_units,
                ..
            } => {
                self.catalog_progress
                    .note_heartbeat(run_id, phase, *progress_epoch, *work_units);
                if *work_units > self.catalog_progress_work_units {
                    self.catalog_progress_work_units = *work_units;
                    self.note_validated_catalog_progress(
                        "heartbeat",
                        phase,
                        &format!("work_units={work_units}"),
                        -1,
                        now,
                    );
                }
            }
            CatalogWorkerMessage::Timing { name, detail } => {
                if name == "catalog_worker_handshake_v6"
                    && let Some(run_id) = detail.strip_prefix("run_id=")
                {
                    self.catalog_progress.note_worker_run(run_id);
                }
                self.note_catalog_progress("timing", name, detail, -1, now);
            }
            CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids,
                all_published_systems,
            } => {
                self.note_catalog_progress(
                    "plan-ready",
                    "reconciling-systems",
                    &format!(
                        "systems={} all_published_systems={}",
                        system_ids.len(),
                        u8::from(*all_published_systems)
                    ),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::SystemDiscovering { title } => {
                self.note_catalog_progress(
                    "system-discovering",
                    "discovering-systems",
                    title,
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::BuildStatus { title } => {
                self.note_catalog_progress("build-status", "saving-catalog", title, -1, now);
            }
            CatalogWorkerMessage::SystemScanning { system_id } => {
                self.note_catalog_progress(
                    "system-scanning",
                    "reconciling-systems",
                    system_id,
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::SystemPrepared {
                system_id,
                generation,
            } => {
                self.note_catalog_progress(
                    "system-prepared",
                    "publishing-systems",
                    &format!("system={system_id} generation={generation}"),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::SystemRemoved { system_id } => {
                self.note_catalog_progress(
                    "system-removed",
                    "publishing-systems",
                    system_id,
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::SystemUpdateFailed { system_id, error } => {
                self.note_catalog_progress(
                    "system-update-failed",
                    "reconciling-systems",
                    &format!("system={system_id} error={error}"),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::ManifestPublished {
                generation,
                rebuilt,
                removed,
            } => {
                self.note_catalog_progress(
                    "manifest-published",
                    "publishing-systems",
                    &format!(
                        "generation={generation} rebuilt={} removed={}",
                        rebuilt.len(),
                        removed.len()
                    ),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::BuildCompleted { elapsed_us } => {
                self.note_catalog_progress(
                    "build-completed",
                    "catalog-ready",
                    &format!("elapsed_us={elapsed_us}"),
                    100,
                    now,
                );
            }
            CatalogWorkerMessage::SystemShardReady {
                system_id,
                game_count,
                prepare_us,
                ..
            } => {
                self.note_catalog_progress(
                    "system-ready",
                    "publishing-systems",
                    &format!("system={system_id} games={game_count} prepare_us={prepare_us}"),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::SystemShardFailed { system_id, error } => {
                self.note_catalog_progress(
                    "system-failed",
                    "publishing-systems",
                    &format!("system={system_id} error={error}"),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::Ready {
                catalog,
                durable_save_pending,
                ..
            } => {
                self.note_catalog_progress(
                    "catalog-ready",
                    "catalog-ready",
                    &format!(
                        "games={} durable_save_pending={}",
                        catalog.len(),
                        u8::from(*durable_save_pending)
                    ),
                    100,
                    now,
                );
            }
            CatalogWorkerMessage::ArcadeBootstrapReady { .. }
            | CatalogWorkerMessage::PublishedRegistrySeed { .. } => {
                unreachable!("internal catalog publication crossed the child protocol")
            }
            CatalogWorkerMessage::HydrationDoneNeedsValidation { root } => {
                self.note_catalog_progress("hydration-ready", "validation-deferred", root, -1, now);
            }
            CatalogWorkerMessage::Done => {
                self.finish_catalog_progress_at("completed", "worker completed", now);
            }
            CatalogWorkerMessage::LoadFailed { error } => {
                self.finish_catalog_progress_at("failed", error, now);
            }
            CatalogWorkerMessage::PersistenceFailed { error } => {
                self.finish_catalog_progress_at("failed", error, now);
            }
            CatalogWorkerMessage::SearchQueryReady { .. }
            | CatalogWorkerMessage::SearchQueryFailed { .. } => {}
        }
    }

    fn note_catalog_progress(
        &mut self,
        activity_kind: &str,
        phase: &str,
        detail: &str,
        percent: i32,
        now: Instant,
    ) {
        self.catalog_progress
            .note_observation(activity_kind, phase, detail, percent, now);
    }

    fn note_validated_catalog_progress(
        &mut self,
        activity_kind: &str,
        phase: &str,
        detail: &str,
        percent: i32,
        now: Instant,
    ) {
        if let Some(evidence) =
            self.catalog_progress
                .note_activity(activity_kind, phase, detail, percent, now)
        {
            self.enqueue_catalog_progress(evidence);
        }
    }

    fn finish_catalog_progress(&mut self, state: &str, detail: &str) {
        self.finish_catalog_progress_at(state, detail, Instant::now());
    }

    fn finish_catalog_progress_at(&mut self, state: &str, detail: &str, now: Instant) {
        let episode_id = self.catalog_progress.episode_id().map(str::to_string);
        if let Some(evidence) = self.catalog_progress.finish(state, detail, now)
            && let Some(episode_id) = episode_id
        {
            emit_catalog_progress(episode_id, evidence);
        }
    }

    fn enqueue_catalog_progress(
        &self,
        evidence: crate::catalog_progress_report::CatalogProgressEvidence,
    ) {
        if let Some(episode_id) = self.catalog_progress.episode_id() {
            emit_catalog_progress(episode_id.to_string(), evidence);
        }
    }

    pub(super) fn media_worker_running(&self) -> bool {
        matches!(self.media, MediaJobState::Running(_))
    }

    pub(super) fn media_worker_unavailable(&self) -> bool {
        matches!(self.media, MediaJobState::Unavailable)
    }

    pub(super) fn ensure_media_worker_started(&mut self, start: Instant, mode: &str) {
        if !matches!(self.media, MediaJobState::Idle) {
            return;
        }
        match start_screenshot_media_worker_with_captured(self.media_config.clone()) {
            Some(handle) => {
                self.media = MediaJobState::Running(handle);
                print_startup_event(
                    start,
                    "screenshot_media_worker_start",
                    format!("mode={mode}"),
                );
            }
            None => {
                self.media = MediaJobState::Unavailable;
                print_startup_event(
                    start,
                    "screenshot_media_worker_skip",
                    format!("mode={mode}"),
                );
            }
        }
    }

    pub(super) fn ensure_media_system(&self, system_id: &str) {
        if let MediaJobState::Running(handle) = &self.media {
            handle.ensure_system(system_id);
        }
    }

    pub(super) fn finish_media_worker(&self) {
        if let MediaJobState::Running(handle) = &self.media {
            handle.finish();
        }
    }

    pub(super) fn drop_media_worker(&mut self) {
        self.media = MediaJobState::Idle;
    }

    pub(super) fn mark_media_worker_unavailable(&mut self) {
        self.media = MediaJobState::Unavailable;
    }

    pub(super) fn set_media_interaction_active(&self, active: bool, reason: &str) {
        if let MediaJobState::Running(handle) = &self.media {
            handle.set_interaction_active(active, reason);
        }
    }

    pub(super) fn poll_media(&mut self, out: &mut MediaJobEventBuf) {
        out.clear();
        if let MediaJobState::Running(handle) = &self.media {
            for _ in 0..MEDIA_MESSAGES_PER_FRAME {
                let Some(message) = handle.try_recv() else {
                    break;
                };
                out.push(message);
            }
        }
    }

    pub(super) fn record_loading_frame(&mut self, loop_start: Instant) {
        self.launch_handoff.record_loading_frame(loop_start);
    }

    pub(super) fn launch_loading_title(&self) -> &str {
        self.launch_handoff.loading_title()
    }

    pub(super) fn visible_loading_title<'a>(&'a self, fallback: &'a str) -> &'a str {
        self.launch_handoff.visible_loading_title(fallback)
    }

    pub(super) fn recover_stale_launch_transport(&mut self, lifecycle_launch_active: bool) -> bool {
        self.launch_handoff
            .recover_stale_transport(lifecycle_launch_active)
    }

    pub(super) fn has_pending_launch(&self) -> bool {
        self.launch_handoff.has_pending_launch()
    }

    pub(super) fn launch_benchmark_enabled(&self) -> bool {
        self.launch_handoff.benchmark_enabled()
    }

    pub(super) fn should_request_benchmark_launch(&self) -> bool {
        self.launch_handoff.should_request_benchmark_launch()
    }

    pub(super) fn begin_launch(
        &mut self,
        nav: &LauncherNav,
        catalog: &ArcadeCatalog,
        durable_catalog_fingerprint: Option<&str>,
        launch_ref: &str,
        now: Instant,
    ) -> bool {
        self.launch_handoff
            .begin_launch(nav, catalog, durable_catalog_fingerprint, launch_ref, now)
    }

    pub(super) fn complete_loading_frame(&mut self, loading_presented: Instant) {
        self.launch_handoff
            .complete_loading_frame(loading_presented);
    }

    pub(super) fn poll_launch_completion(
        &mut self,
        result_received: Instant,
    ) -> Option<LaunchHandoffCompletion> {
        self.launch_handoff.poll_completion(result_received)
    }

    pub(super) fn stop_spawned_mister_for_recovery(&mut self) -> bool {
        self.launch_handoff.stop_spawned_mister_for_recovery()
    }

    pub(super) fn finish_launch_failure_recovery(&mut self, recovery_presented: Instant) {
        self.launch_handoff
            .finish_failure_recovery(recovery_presented);
    }

    pub(super) fn launch_runtime_action(&self, now: Instant) -> Option<LaunchHandoffRuntimeAction> {
        self.launch_handoff.runtime_action(now)
    }
}

fn cached_search_catalog(
    cache: &Mutex<
        Option<(
            usize,
            mister_magik_catalog::persisted_search::PersistedSearchCatalog,
        )>,
    >,
    catalog_version: usize,
    storage: &std::path::Path,
) -> Result<
    mister_magik_catalog::persisted_search::PersistedSearchCatalog,
    mister_magik_catalog::persisted_search::PersistedSearchError,
> {
    versioned_cache_value(cache, catalog_version, || {
        mister_magik_catalog::persisted_search::PersistedSearchCatalog::open(
            storage,
            mister_magik_catalog::shard_registry::production_registry_limits(),
        )
    })
}

fn versioned_cache_value<T: Clone, E>(
    cache: &Mutex<Option<(usize, T)>>,
    version: usize,
    load: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached_version, value)) = cache.as_ref()
        && *cached_version == version
    {
        return Ok(value.clone());
    }
    let value = load()?;
    *cache = Some((version, value.clone()));
    Ok(value)
}

#[cfg(not(test))]
fn emit_catalog_progress(
    episode_id: String,
    evidence: crate::catalog_progress_report::CatalogProgressEvidence,
) {
    crate::catalog_progress_report::enqueue(episode_id, evidence);
}

#[cfg(test)]
fn emit_catalog_progress(
    _episode_id: String,
    _evidence: crate::catalog_progress_report::CatalogProgressEvidence,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_cache_reuses_and_invalidates_values() {
        let cache = Mutex::new(None);
        let first = versioned_cache_value::<_, ()>(&cache, 7, || Ok("first".to_string())).unwrap();
        let reused = versioned_cache_value::<_, ()>(&cache, 7, || {
            panic!("matching versions must not reload")
        })
        .unwrap();
        let replaced =
            versioned_cache_value::<_, ()>(&cache, 8, || Ok("second".to_string())).unwrap();

        assert_eq!(first, "first");
        assert_eq!(reused, "first");
        assert_eq!(replaced, "second");
    }

    #[test]
    fn event_buffers_are_reused_without_shrinking() {
        let mut catalog = CatalogJobEventBuf::new();
        let mut media = MediaJobEventBuf::new();
        let catalog_capacity = catalog.capacity();
        let media_capacity = media.capacity();

        catalog.clear();
        media.clear();

        assert_eq!(catalog.capacity(), catalog_capacity);
        assert_eq!(media.capacity(), media_capacity);
    }

    #[test]
    fn prepared_system_entry_mailbox_keeps_only_the_newest_sequence() {
        let mailbox = PreparedSystemEntryMailbox::default();
        for (sequence, error) in [(4, "four"), (6, "six"), (5, "five")] {
            mailbox.publish(SystemEntryPrepareOutcome::Failed(FailedSystemEntry {
                sequence,
                generation: Some("generation-a".to_string()),
                system_id: "c64".to_string(),
                error: error.to_string(),
            }));
        }

        let outcome = mailbox.try_take().expect("newest outcome");
        assert_eq!(outcome.sequence(), 6);
        assert!(matches!(
            outcome,
            SystemEntryPrepareOutcome::Failed(FailedSystemEntry { error, .. }) if error == "six"
        ));
        assert!(mailbox.try_take().is_none());
    }

    #[test]
    fn prepared_system_entry_mailbox_never_blocks_cpu1() {
        let mailbox = PreparedSystemEntryMailbox::default();
        mailbox.publish(SystemEntryPrepareOutcome::Failed(FailedSystemEntry {
            sequence: 4,
            generation: Some("generation-a".to_string()),
            system_id: "c64".to_string(),
            error: "ready".to_string(),
        }));
        let guard = mailbox.newest.lock().unwrap();

        assert!(mailbox.try_take().is_none());

        drop(guard);
        assert_eq!(mailbox.try_take().expect("deferred outcome").sequence(), 4);
    }

    #[test]
    fn warmed_generation_map_is_used_only_for_the_exact_catalog_generation() {
        let generation_a = Some("generation-a".to_string());
        let generation_b = Some("generation-b".to_string());

        assert!(warmed_generation_matches(&generation_a, &generation_a));
        assert!(!warmed_generation_matches(&generation_a, &generation_b));
        assert!(!warmed_generation_matches(&generation_a, &None));
        assert!(!warmed_generation_matches(&None, &None));
    }

    #[test]
    fn scheduler_starts_with_idle_system_entry_worker() {
        let scheduler = LauncherScheduler::new(false);

        assert!(!scheduler.catalog_worker_running());
        assert!(scheduler.catalog_worker_available());
        assert!(scheduler.system_entry_prepare.is_some());
        assert!(!scheduler.media_worker_running());
        assert!(!scheduler.media_worker_unavailable());
        assert!(!scheduler.launch_benchmark_enabled());
    }

    #[test]
    fn starting_catalog_worker_does_not_detach_existing_worker() {
        let (_tx, rx) = mpsc::channel();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.catalog = CatalogJobState::Running(rx.into());

        assert!(!scheduler.catalog_worker_available());
        assert!(!scheduler.start_catalog_worker(
            "/tmp/catalog-test".to_string(),
            CatalogWorkerRequest::LoadOnly,
            CatalogWorkerInitialCache::AlreadyProbedMissing,
            CatalogExecutionMode::BackgroundInteractive,
        ));
        assert!(scheduler.catalog_worker_running());
    }

    #[test]
    fn system_shard_requests_deduplicate_without_replacing_the_original_request() {
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "active".to_string(),
            generation: Some("generation-a".to_string()),
            sequence: 1,
        };
        let now = Instant::now();

        assert!(scheduler.request_system_shard(
            "c64".to_string(),
            "menu",
            empty_arcade_catalog("/tmp"),
            1,
            now
        ));
        assert!(!scheduler.request_system_shard(
            "c64".to_string(),
            "open",
            empty_arcade_catalog("/tmp"),
            2,
            now
        ));

        assert_eq!(scheduler.system_shard_queue.len(), 1);
        assert_eq!(scheduler.system_shard_queue[0].system_id, "c64");
        assert_eq!(scheduler.system_shard_queue[0].reason, "menu");
        assert_eq!(scheduler.system_shard_queue[0].base_catalog_version, 1);
    }

    #[test]
    fn system_shard_requests_remain_fifo() {
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "active".to_string(),
            generation: Some("generation-a".to_string()),
            sequence: 1,
        };
        let now = Instant::now();
        assert!(scheduler.request_system_shard(
            "acornatom".to_string(),
            "menu",
            empty_arcade_catalog("/tmp"),
            1,
            now
        ));
        assert!(scheduler.request_system_shard(
            "c64".to_string(),
            "open",
            empty_arcade_catalog("/tmp"),
            1,
            now
        ));

        let next = scheduler
            .system_shard_queue
            .front()
            .expect("queued request");
        assert_eq!(next.system_id, "acornatom");
    }

    #[test]
    fn system_shard_generation_change_clears_attempts_and_queue() {
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "active".to_string(),
            generation: Some("generation-a".to_string()),
            sequence: 1,
        };
        assert!(scheduler.request_system_shard(
            "c64".to_string(),
            "menu",
            empty_arcade_catalog("/tmp"),
            1,
            Instant::now()
        ));

        let _ = scheduler.set_system_shard_generation(Some("generation-b"));

        assert!(scheduler.system_shard_queue.is_empty());
        assert!(!scheduler.system_shard_attempted("c64"));
    }

    #[test]
    fn explicit_retry_requeues_a_failed_attempt() {
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "active".to_string(),
            generation: Some("generation-a".to_string()),
            sequence: 1,
        };
        scheduler.system_shard_attempted.insert("c64".to_string());

        assert!(scheduler.retry_system_shard(
            "c64".to_string(),
            "explicit-retry",
            empty_arcade_catalog("/tmp"),
            1,
            Instant::now()
        ));
        assert_eq!(scheduler.system_shard_queue.len(), 1);
    }

    #[test]
    fn system_shard_request_requires_authoritative_generation() {
        let mut scheduler = LauncherScheduler::new(false);

        assert!(!scheduler.request_system_shard(
            "c64".to_string(),
            "open",
            empty_arcade_catalog("/tmp"),
            1,
            Instant::now()
        ));
        assert!(scheduler.system_shard_queue.is_empty());
        assert!(!scheduler.system_shard_attempted("c64"));
    }

    #[test]
    fn arcade_publication_binds_direct_and_home_collection_aliases() {
        for requested in ["arcade", arcade_catalog::MENU_ARCADE_SYSTEM_ID] {
            let game = arcade_catalog::ArcadeGameEntry {
                title: "Fixture".into(),
                mra_path: "/media/fat/_Arcade/Fixture.mra".into(),
                preview_archive_path: "/media/fat/preview/arcade.zip".into(),
                preview_asset_key: "Fixture".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                players: None,
                control: "".into(),
                is_new: false,
            };
            let collection = Arc::new(arcade_catalog::SystemCollection::new(
                "arcade",
                vec![game],
                Vec::new(),
                arcade_catalog::PlatformKind::Arcade,
            ));

            let catalog = publish_prepared_system_collection(
                &empty_arcade_catalog("/tmp"),
                requested,
                "arcade",
                collection,
            );

            assert_eq!(catalog.system_game_count("arcade"), 1);
            assert_eq!(
                catalog.system_game_count(arcade_catalog::MENU_ARCADE_SYSTEM_ID),
                1
            );
            assert_eq!(
                catalog
                    .system_game_at(arcade_catalog::MENU_ARCADE_SYSTEM_ID, 0)
                    .unwrap()
                    .system_id
                    .as_ref(),
                "arcade"
            );
        }
    }

    #[test]
    fn stale_generation_shard_completion_is_discarded() {
        let results = Arc::new(PreparedSystemEntryMailbox::default());
        let (requests, request_rx) = mpsc::channel();
        let (_liveness_tx, liveness) = mpsc::channel();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_entry_prepare = Some(SystemEntryPrepareWorker {
            requests,
            results: Arc::clone(&results),
            liveness,
        });
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "c64".to_string(),
            generation: Some("generation-a".to_string()),
            sequence: 7,
        };
        let _ = scheduler.set_system_shard_generation(Some("generation-b"));
        results.publish(SystemEntryPrepareOutcome::Failed(FailedSystemEntry {
            sequence: 7,
            generation: Some("generation-a".to_string()),
            system_id: "c64".to_string(),
            error: "stale".to_string(),
        }));
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(&mut events, CatalogPollScope::Idle);

        assert!(events.events.is_empty());
        assert!(!scheduler.system_shard_loading("c64"));
        assert!(matches!(
            request_rx.try_recv(),
            Ok(SystemEntryPrepareCommand::RetireOutcome(_))
        ));
    }

    #[test]
    fn terminal_failure_becomes_idle_before_same_frame_retry() {
        let results = Arc::new(PreparedSystemEntryMailbox::default());
        let (requests, _request_rx) = mpsc::channel();
        let (_liveness_tx, liveness) = mpsc::channel();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_entry_prepare = Some(SystemEntryPrepareWorker {
            requests,
            results: Arc::clone(&results),
            liveness,
        });
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard_attempted.insert("c64".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "c64".to_string(),
            generation: Some("generation-a".to_string()),
            sequence: 9,
        };
        results.publish(SystemEntryPrepareOutcome::Failed(FailedSystemEntry {
            sequence: 9,
            generation: Some("generation-a".to_string()),
            system_id: "c64".to_string(),
            error: "temporary".to_string(),
        }));
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(&mut events, CatalogPollScope::Idle);

        assert!(!scheduler.system_shard_loading("c64"));
        assert!(scheduler.retry_system_shard(
            "c64".to_string(),
            "explicit-retry",
            empty_arcade_catalog("/tmp"),
            1,
            Instant::now()
        ));
    }

    #[test]
    fn catalog_poll_is_budgeted_per_frame() {
        let (tx, rx) = mpsc::channel();
        for idx in 0..3 {
            tx.send(CatalogWorkerMessage::Timing {
                name: format!("event-{idx}"),
                detail: String::new(),
            })
            .unwrap();
        }
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.catalog = CatalogJobState::Running(rx.into());
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(&mut events, CatalogPollScope::Idle);
        assert_eq!(events.len(), CATALOG_MESSAGES_PER_FRAME);

        scheduler.poll_catalog(&mut events, CatalogPollScope::Idle);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn interactive_catalog_poll_drains_primary_sequence_without_search_results() {
        let (catalog_tx, catalog_rx) = mpsc::channel();
        catalog_tx
            .send(CatalogWorkerMessage::Timing {
                name: "progress".to_string(),
                detail: String::new(),
            })
            .unwrap();
        catalog_tx
            .send(CatalogWorkerMessage::Ready {
                catalog: empty_arcade_catalog("/tmp"),
                load_us: 1,
                source: CatalogSource::FreshBuild,
                durable_save_pending: true,
                generation_fingerprint: None,
                publication_ack: None,
            })
            .unwrap();
        catalog_tx.send(CatalogWorkerMessage::Done).unwrap();
        let (search_tx, search_rx) = mpsc::channel();
        search_tx
            .send(CatalogWorkerMessage::SearchQueryFailed {
                request: launcher::ArcadeSearchRequest {
                    request_id: 1,
                    catalog_version: 1,
                    collection_id: "arcade".to_string(),
                    system_ids: vec!["arcade".to_string()],
                    query: "fixture".to_string(),
                },
                error: "deferred".to_string(),
            })
            .unwrap();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.catalog = CatalogJobState::Running(catalog_rx.into());
        scheduler.search_query = SearchQueryJobState::Running(search_rx);
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(
            &mut events,
            CatalogPollScope::Interactive {
                system_entry_handoff: false,
            },
        );
        assert!(matches!(
            events.events.as_slice(),
            [
                CatalogWorkerMessage::Timing { name, .. },
                CatalogWorkerMessage::Ready { .. }
            ] if name == "progress"
        ));
        assert!(matches!(
            scheduler.search_query,
            SearchQueryJobState::Running(_)
        ));

        scheduler.poll_catalog(
            &mut events,
            CatalogPollScope::Interactive {
                system_entry_handoff: false,
            },
        );
        assert!(matches!(
            events.events.as_slice(),
            [CatalogWorkerMessage::Done]
        ));
        assert!(matches!(
            scheduler.search_query,
            SearchQueryJobState::Running(_)
        ));
    }

    #[test]
    fn interactive_catalog_poll_prioritizes_foreground_system_entry() {
        let (catalog_tx, catalog_rx) = mpsc::channel();
        catalog_tx
            .send(CatalogWorkerMessage::Timing {
                name: "primary".to_string(),
                detail: String::new(),
            })
            .unwrap();
        let results = Arc::new(PreparedSystemEntryMailbox::default());
        let (requests, _request_rx) = mpsc::channel();
        let (_liveness_tx, liveness) = mpsc::channel();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.catalog = CatalogJobState::Running(catalog_rx.into());
        scheduler.system_entry_prepare = Some(SystemEntryPrepareWorker {
            requests,
            results: Arc::clone(&results),
            liveness,
        });
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "c64".to_string(),
            generation: Some("generation-a".to_string()),
            sequence: 12,
        };
        results.publish(SystemEntryPrepareOutcome::Failed(FailedSystemEntry {
            sequence: 12,
            generation: Some("generation-a".to_string()),
            system_id: "c64".to_string(),
            error: "terminal".to_string(),
        }));
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(
            &mut events,
            CatalogPollScope::Interactive {
                system_entry_handoff: true,
            },
        );

        assert!(matches!(
            events.events.as_slice(),
            [
                CatalogWorkerMessage::SystemShardFailed { system_id, .. },
                CatalogWorkerMessage::Timing { name, .. }
            ] if system_id == "c64" && name == "primary"
        ));
    }

    #[test]
    fn transition_poll_prioritizes_entry_then_drains_catalog_control() {
        let (catalog_tx, catalog_rx) = mpsc::channel();
        catalog_tx
            .send(CatalogWorkerMessage::Timing {
                name: "background".to_string(),
                detail: String::new(),
            })
            .unwrap();
        let results = Arc::new(PreparedSystemEntryMailbox::default());
        let (requests, _request_rx) = mpsc::channel();
        let (_liveness_tx, liveness) = mpsc::channel();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.catalog = CatalogJobState::Running(catalog_rx.into());
        scheduler.system_entry_prepare = Some(SystemEntryPrepareWorker {
            requests,
            results: Arc::clone(&results),
            liveness,
        });
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "c64".to_string(),
            generation: Some("generation-a".to_string()),
            sequence: 11,
        };
        results.publish(SystemEntryPrepareOutcome::Failed(FailedSystemEntry {
            sequence: 11,
            generation: Some("generation-a".to_string()),
            system_id: "c64".to_string(),
            error: "terminal".to_string(),
        }));
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(
            &mut events,
            CatalogPollScope::Transition {
                system_entry_handoff: true,
            },
        );

        assert_eq!(events.len(), CATALOG_MESSAGES_PER_FRAME);
        assert!(matches!(
            events.events.first(),
            Some(CatalogWorkerMessage::SystemShardFailed { system_id, .. }) if system_id == "c64"
        ));
        assert!(matches!(
            events.events.as_slice(),
            [
                CatalogWorkerMessage::SystemShardFailed { system_id, .. },
                CatalogWorkerMessage::Timing { name, .. }
            ] if system_id == "c64" && name == "background"
        ));
        assert!(matches!(
            catalog_tx.send(CatalogWorkerMessage::Done),
            Ok(())
        ));
    }

    #[test]
    fn catalog_poll_releases_disconnected_worker_after_buffer_is_drained() {
        let (tx, rx) = mpsc::channel();
        tx.send(CatalogWorkerMessage::Done).unwrap();
        drop(tx);
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.catalog = CatalogJobState::Running(rx.into());
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(&mut events, CatalogPollScope::Idle);
        assert_eq!(events.len(), 1);
        assert!(!scheduler.catalog_worker_running());
    }

    #[test]
    fn catalog_poll_surfaces_unexpected_disconnect_as_terminal_failure() {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.catalog = CatalogJobState::Running(rx.into());
        let mut events = CatalogJobEventBuf::new();

        assert!(scheduler.poll_catalog(&mut events, CatalogPollScope::Idle));
        assert!(matches!(
            events.events.as_slice(),
            [CatalogWorkerMessage::LoadFailed { error }]
                if error == "catalog worker disconnected without a terminal message"
        ));
        assert!(!scheduler.catalog_worker_running());
    }

    #[test]
    fn terminal_failure_retains_retry_gate_until_child_is_reaped() {
        let (tx, rx) = mpsc::channel();
        tx.send(CatalogWorkerMessage::PersistenceFailed {
            error: "protocol failed".to_string(),
        })
        .unwrap();
        let control = Arc::new(CatalogChildControl::test_unreaped());
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.catalog = CatalogJobState::Running(rx.into());
        scheduler.catalog_child_control = Some(Arc::clone(&control));
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(&mut events, CatalogPollScope::Idle);

        assert!(!scheduler.catalog_worker_running());
        assert!(scheduler.catalog_child_control.is_some());
        assert!(!scheduler.catalog_worker_available());
        assert!(!scheduler.start_catalog_worker(
            "/tmp/catalog-test".to_string(),
            CatalogWorkerRequest::LoadOnly,
            CatalogWorkerInitialCache::AlreadyProbedMissing,
            CatalogExecutionMode::BackgroundInteractive,
        ));
        control.mark_reaped_for_test();
        assert!(scheduler.catalog_worker_available());
    }

    #[test]
    fn advancing_heartbeats_keep_watchdog_live_past_stall_threshold() {
        let start = Instant::now();
        let (_tx, rx) = mpsc::channel();
        let control = Arc::new(CatalogChildControl::test_unreaped());
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.catalog = CatalogJobState::Running(rx.into());
        scheduler.catalog_child_control = Some(Arc::clone(&control));
        let _ = scheduler.catalog_progress.start(
            "/tmp/catalog-test".to_string(),
            "fresh_build",
            "background_interactive",
            start,
        );

        for (sequence, elapsed_secs) in [(1, 90), (2, 180), (3, 270)] {
            let now = start + Duration::from_secs(elapsed_secs);
            scheduler.record_catalog_progress_message(
                &CatalogWorkerMessage::Heartbeat {
                    run_id: "watchdog-test".to_string(),
                    phase: "artifacts".to_string(),
                    sequence,
                    progress_epoch: 1,
                    work_units: sequence,
                },
                now,
            );
            scheduler.tick_catalog_progress(true, now);
            assert!(!scheduler.catalog_stop_requested);
        }

        scheduler.tick_catalog_progress(true, start + Duration::from_secs(359));
        assert!(!scheduler.catalog_stop_requested);
        assert_eq!(scheduler.catalog_progress_work_units, 3);
        control.mark_reaped_for_test();
    }
}
