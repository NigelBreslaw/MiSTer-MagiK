// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_worker_intents::{
    CatalogCounterPhase, CatalogProgressUiIntent, CatalogWorkerUiContext, LauncherWorkerUiIntent,
    cached_catalog_validation_intent, catalog_plan_ready_intent, catalog_rebuild_started_intent,
    parse_games_found_detail,
};
use super::*;

pub(super) struct CatalogWorkerStart {
    pub(super) root: String,
    pub(super) request: CatalogWorkerRequest,
    pub(super) initial_cache: CatalogWorkerInitialCache,
    pub(super) execution_mode: CatalogExecutionMode,
}

struct DeferredCatalogWorker {
    root: String,
    request: CatalogWorkerRequest,
    initial_cache: CatalogWorkerInitialCache,
    execution_mode: CatalogExecutionMode,
    start_after: Option<Instant>,
}

pub(super) struct CatalogSessionEvent {
    pub(super) name: String,
    pub(super) detail: String,
}

pub(super) enum CatalogSessionEffect {
    StartupEvent(CatalogSessionEvent),
    UseCatalog {
        catalog: ArcadeCatalog,
        load_us: u64,
        source: CatalogSource,
        durable: bool,
        generation_fingerprint: Option<String>,
        publication_ack: Option<mpsc::Sender<()>>,
    },
    MarkCatalogDurable {
        generation_fingerprint: Option<String>,
    },
    ConfirmCatalogSeed,
    DiscardPartialCatalog,
    ApplySearchResult {
        request: launcher::ArcadeSearchRequest,
        result: mister_magik_catalog::persisted_search::PersistedCollectionSearchResult,
    },
    FailSearchRequest {
        request: launcher::ArcadeSearchRequest,
        error: String,
    },
    SyncCatalogBridge,
    CatalogBuildStarted,
    CatalogPlanReady {
        system_ids: Vec<String>,
        all_published_systems: bool,
    },
    CatalogSystemDiscovered {
        system_id: String,
    },
    CatalogSystemScanning {
        system_id: String,
    },
    CatalogSystemPrepared {
        system_id: String,
        generation: u64,
    },
    CatalogManifestPublished {
        generation: u64,
        rebuilt: Vec<String>,
        removed: Vec<String>,
    },
    CatalogSystemUpdateFailed {
        system_id: String,
    },
    CatalogSystemHydrationFailed {
        system_id: String,
    },
    PersistCatalogFailure {
        detail: String,
        mode: CatalogRecoveryMode,
        has_stale_catalog: bool,
        system_id: Option<String>,
    },
    CatalogBuildFinished,
    Ui(LauncherWorkerUiIntent),
    FinishMediaWorker,
    FinishMediaWorkerIfNoCatalogSeedPending,
    CatalogValidationFinished,
    RequestMediaCatalogSeed,
    MediaSystemDiscovered {
        system_id: String,
        media_gate: Option<MediaInteractionGate>,
    },
    ApplySystemShard {
        system_id: String,
        catalog: ArcadeCatalog,
        base_catalog_version: usize,
        game_count: usize,
        prepare_us: u64,
        profile: SystemEntryCatalogProfile,
    },
    RequestLibraryRebuildOnNextBoot,
    Confirm(launcher::ConfirmAction),
    Lifecycle(LauncherLifecycleInput),
    StartCatalogWorker(CatalogWorkerStart),
}

#[derive(Default)]
pub(super) struct CatalogSessionEffects {
    effects: Vec<CatalogSessionEffect>,
}

impl CatalogSessionEffects {
    fn push(&mut self, effect: CatalogSessionEffect) {
        self.effects.push(effect);
    }

    fn event(&mut self, name: impl Into<String>, detail: impl Into<String>) {
        self.push(CatalogSessionEffect::StartupEvent(CatalogSessionEvent {
            name: name.into(),
            detail: detail.into(),
        }));
    }

    fn ui(&mut self, intent: LauncherWorkerUiIntent) {
        self.push(CatalogSessionEffect::Ui(intent));
    }

    pub(super) fn into_effects(self) -> impl IntoIterator<Item = CatalogSessionEffect> {
        self.effects
    }
}

pub(super) struct CatalogWorkerMessageContext {
    pub(super) catalog_ready: bool,
    pub(super) catalog_partial: bool,
    pub(super) screen: Screen,
    pub(super) media_gate: Option<MediaInteractionGate>,
}

pub(super) struct LauncherCatalogSession {
    foreground_update: bool,
    refresh_done: bool,
    refresh_failed: bool,
    summary_only: bool,
    persisted_summary_seen: bool,
    deferred_worker: Option<DeferredCatalogWorker>,
    games_found_counter: GamesFoundCounter,
    bootstrap_counter_climb_logged: bool,
    bootstrap_counter_sustained_climb_logged: bool,
    full_scan_counter_climb_logged: bool,
}

impl LauncherCatalogSession {
    pub(super) fn new(foreground_update: bool) -> Self {
        Self {
            foreground_update,
            refresh_done: false,
            refresh_failed: false,
            summary_only: false,
            persisted_summary_seen: false,
            deferred_worker: None,
            games_found_counter: GamesFoundCounter::default(),
            bootstrap_counter_climb_logged: false,
            bootstrap_counter_sustained_climb_logged: false,
            full_scan_counter_climb_logged: false,
        }
    }

    pub(super) fn foreground_update(&self) -> bool {
        self.foreground_update
    }

    pub(super) fn refresh_done(&self) -> bool {
        self.refresh_done
    }

    pub(super) fn mark_refresh_done(&mut self) {
        self.refresh_done = true;
    }

    pub(super) fn note_summary_seed_ready(&mut self) {
        self.summary_only = true;
    }

    pub(super) fn note_cached_catalog_ready(&mut self) {
        self.summary_only = false;
    }

    pub(super) fn defer_catalog_worker(
        &mut self,
        root: String,
        request: CatalogWorkerRequest,
        initial_cache: CatalogWorkerInitialCache,
        execution_mode: CatalogExecutionMode,
    ) {
        self.deferred_worker = Some(DeferredCatalogWorker {
            root,
            request,
            initial_cache,
            execution_mode,
            start_after: None,
        });
    }

    pub(super) fn maybe_start_deferred_worker(
        &mut self,
        worker_running: bool,
        first_visible_copy_done: bool,
        background_work_allowed: bool,
        loop_start: Instant,
        delay: Duration,
        catalog_builder_available: impl FnOnce() -> bool,
    ) -> Option<CatalogWorkerStart> {
        if self.refresh_done || worker_running {
            return None;
        }
        let deferred = self.deferred_worker.as_mut()?;
        if !first_visible_copy_done {
            return None;
        }
        if !background_work_allowed {
            return None;
        }
        let start_after = *deferred
            .start_after
            .get_or_insert_with(|| loop_start + delay);
        if loop_start < start_after {
            return None;
        }
        if deferred.request == CatalogWorkerRequest::CheckStamp && !catalog_builder_available() {
            deferred.start_after = Some(loop_start + Duration::from_secs(1));
            return None;
        }
        let deferred = self.deferred_worker.take()?;
        Some(CatalogWorkerStart {
            root: deferred.root,
            request: deferred.request,
            initial_cache: deferred.initial_cache,
            execution_mode: deferred.execution_mode,
        })
    }

    pub(super) fn handle_worker_message(
        &mut self,
        context: CatalogWorkerMessageContext,
        message: CatalogWorkerMessage,
        now: Instant,
    ) -> CatalogSessionEffects {
        let mut effects = CatalogSessionEffects::default();
        match message {
            CatalogWorkerMessage::Timing { name, detail } => {
                effects.event(name, detail);
            }
            CatalogWorkerMessage::Progress {
                title,
                detail,
                percent,
                ..
            } => {
                self.handle_progress(context, title, detail, percent, now, &mut effects);
            }
            CatalogWorkerMessage::LoadFailed { error } => {
                let has_stale_catalog = context.catalog_ready && !context.catalog_partial;
                let transient = catalog_failure_is_transient(&error);
                let mode = if catalog_failure_is_format_upgrade(&error) {
                    CatalogRecoveryMode::UpgradeRequired
                } else {
                    CatalogRecoveryMode::LoadFailure { transient }
                };
                self.refresh_done = true;
                self.foreground_update = false;
                self.refresh_failed = true;
                self.deferred_worker = None;
                self.games_found_counter.reset();
                effects.push(CatalogSessionEffect::FinishMediaWorker);
                effects.push(CatalogSessionEffect::CatalogValidationFinished);
                effects.push(CatalogSessionEffect::CatalogBuildFinished);
                effects.event("library_load_failed", error.clone());
                effects.push(CatalogSessionEffect::PersistCatalogFailure {
                    detail: error.clone(),
                    mode,
                    has_stale_catalog,
                    system_id: None,
                });
                effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
                if context.catalog_partial {
                    effects.push(CatalogSessionEffect::DiscardPartialCatalog);
                }
                effects.push(CatalogSessionEffect::Lifecycle(
                    LauncherLifecycleInput::CatalogRecoveryRequired {
                        error,
                        has_stale_catalog,
                        mode,
                    },
                ));
            }
            CatalogWorkerMessage::FreshCleanupStarted => {
                effects.push(CatalogSessionEffect::CatalogBuildStarted);
                effects.event("library_fresh_cleanup_started", "lock=acquired");
                effects.push(CatalogSessionEffect::Lifecycle(
                    LauncherLifecycleInput::CatalogFreshCleanupStarted,
                ));
            }
            CatalogWorkerMessage::FreshCleanupCompleted { removed } => {
                effects.event(
                    "library_fresh_cleanup_completed",
                    format!("removed={removed}"),
                );
                effects.push(CatalogSessionEffect::Lifecycle(
                    LauncherLifecycleInput::CatalogFreshCleanupCompleted,
                ));
                effects.ui(catalog_rebuild_started_intent(true));
            }
            CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids,
                all_published_systems,
            } => {
                effects.ui(catalog_plan_ready_intent(
                    system_ids.len(),
                    all_published_systems,
                ));
                effects.push(CatalogSessionEffect::CatalogPlanReady {
                    system_ids,
                    all_published_systems,
                });
            }
            CatalogWorkerMessage::SystemDiscovered { system_id } => {
                effects.push(CatalogSessionEffect::CatalogSystemDiscovered {
                    system_id: system_id.clone(),
                });
                effects.push(CatalogSessionEffect::MediaSystemDiscovered {
                    system_id,
                    media_gate: context.media_gate,
                });
            }
            CatalogWorkerMessage::SystemScanning { system_id } => {
                effects.push(CatalogSessionEffect::CatalogSystemScanning { system_id });
            }
            CatalogWorkerMessage::SystemPrepared {
                system_id,
                generation,
            } => {
                effects.push(CatalogSessionEffect::CatalogSystemPrepared {
                    system_id,
                    generation,
                });
            }
            CatalogWorkerMessage::SystemUpdateFailed { system_id, error } => {
                effects.push(CatalogSessionEffect::CatalogSystemUpdateFailed {
                    system_id: system_id.clone(),
                });
                effects.event(
                    "catalog_system_update_failed",
                    format!("system={system_id} error={error}"),
                );
            }
            CatalogWorkerMessage::ManifestPublished {
                generation,
                rebuilt,
                removed,
            } => {
                effects.push(CatalogSessionEffect::CatalogManifestPublished {
                    generation,
                    rebuilt,
                    removed,
                });
            }
            CatalogWorkerMessage::SystemShardReady {
                system_id,
                catalog,
                base_catalog_version,
                game_count,
                prepare_us,
                profile,
            } => effects.push(CatalogSessionEffect::ApplySystemShard {
                system_id,
                catalog,
                base_catalog_version,
                game_count,
                prepare_us,
                profile,
            }),
            CatalogWorkerMessage::SystemShardFailed { system_id, error } => {
                effects.push(CatalogSessionEffect::CatalogSystemHydrationFailed {
                    system_id: system_id.clone(),
                });
                effects.event(
                    "catalog_system_shard_failed",
                    format!("system={system_id} error={error}"),
                );
                effects.push(CatalogSessionEffect::PersistCatalogFailure {
                    detail: error,
                    mode: CatalogRecoveryMode::LoadFailure { transient: false },
                    has_stale_catalog: context.catalog_ready && !context.catalog_partial,
                    system_id: Some(system_id),
                });
            }
            CatalogWorkerMessage::SearchQueryReady { request, result } => {
                effects.push(CatalogSessionEffect::ApplySearchResult { request, result });
            }
            CatalogWorkerMessage::SearchQueryFailed { request, error } => {
                effects.push(CatalogSessionEffect::FailSearchRequest { request, error });
            }
            CatalogWorkerMessage::HydrationDoneNeedsValidation { root } => {
                self.refresh_done = false;
                self.defer_catalog_worker(
                    root,
                    CatalogWorkerRequest::CheckStamp,
                    CatalogWorkerInitialCache::AlreadyLoadedReady,
                    CatalogExecutionMode::BackgroundInteractive,
                );
                effects.event(
                    "catalog_validation_deferred_after_hydration",
                    "reason=interactive_idle_gate",
                );
            }
            CatalogWorkerMessage::Ready {
                catalog,
                summary,
                load_us,
                source,
                durable_save_pending,
                generation_fingerprint,
                publication_ack,
            } => {
                self.handle_ready(
                    context.catalog_ready,
                    catalog,
                    summary,
                    load_us,
                    source,
                    durable_save_pending,
                    generation_fingerprint,
                    publication_ack,
                    &mut effects,
                );
            }
            CatalogWorkerMessage::Persisted {
                summary,
                completed_build_seconds,
                generation_fingerprint,
            } => {
                self.persisted_summary_seen = true;
                self.refresh_done = true;
                self.foreground_update = false;
                self.refresh_failed = false;
                effects.push(CatalogSessionEffect::FinishMediaWorkerIfNoCatalogSeedPending);
                effects.push(CatalogSessionEffect::CatalogValidationFinished);
                effects.push(CatalogSessionEffect::CatalogBuildFinished);
                effects.event("library_db_saved", format_library_refresh_summary(&summary));
                effects.push(CatalogSessionEffect::MarkCatalogDurable {
                    generation_fingerprint,
                });
                let seconds = completed_build_seconds.unwrap_or_else(|| {
                    mister_magik_catalog::catalog_build_record::rounded_seconds(
                        Duration::from_micros(summary.scan_us.saturating_add(summary.import_us)),
                    )
                });
                effects.ui(LauncherWorkerUiIntent::InfoDatabaseBuild(
                    mister_magik_catalog::catalog_build_record::format_duration(seconds),
                ));
                push_catalog_coverage_diagnostic(&summary, &mut effects);
                effects.ui(LauncherWorkerUiIntent::HideCatalogBackgroundScan);
            }
            CatalogWorkerMessage::PersistenceFailed { error } => {
                let transient = catalog_failure_is_transient(&error);
                self.refresh_done = true;
                self.foreground_update = false;
                self.refresh_failed = true;
                effects.push(CatalogSessionEffect::FinishMediaWorker);
                effects.push(CatalogSessionEffect::CatalogValidationFinished);
                effects.push(CatalogSessionEffect::CatalogBuildFinished);
                effects.event("library_db_save_failed", error.clone());
                effects.push(CatalogSessionEffect::PersistCatalogFailure {
                    detail: error.clone(),
                    mode: CatalogRecoveryMode::PersistenceFailure { transient },
                    has_stale_catalog: context.catalog_ready && !context.catalog_partial,
                    system_id: None,
                });
                effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
                effects.push(CatalogSessionEffect::Lifecycle(
                    LauncherLifecycleInput::CatalogRecoveryRequired {
                        error,
                        has_stale_catalog: context.catalog_ready && !context.catalog_partial,
                        mode: CatalogRecoveryMode::PersistenceFailure { transient },
                    },
                ));
                self.games_found_counter.reset();
            }
            CatalogWorkerMessage::Unchanged { summary } => {
                self.refresh_done = true;
                self.foreground_update = false;
                self.refresh_failed = false;
                effects.push(CatalogSessionEffect::ConfirmCatalogSeed);
                effects.push(CatalogSessionEffect::FinishMediaWorker);
                effects.push(CatalogSessionEffect::CatalogValidationFinished);
                effects.push(CatalogSessionEffect::CatalogBuildFinished);
                effects.event(
                    "library_db_unchanged",
                    format_library_refresh_summary(&summary),
                );
                effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
                self.games_found_counter.reset();
            }
            CatalogWorkerMessage::Changed { detail, reason } => {
                self.refresh_done = true;
                self.foreground_update = false;
                self.refresh_failed = false;
                effects.push(CatalogSessionEffect::FinishMediaWorker);
                effects.push(CatalogSessionEffect::CatalogValidationFinished);
                effects.event("library_changed_detected", detail.clone());
                let mode = match reason {
                    mister_magik_catalog::builder_protocol::CatalogChangeReason::InputsChanged => {
                        CatalogRecoveryMode::InputsChanged
                    }
                    mister_magik_catalog::builder_protocol::CatalogChangeReason::ProjectionUpgrade {
                        ..
                    } => CatalogRecoveryMode::UpgradeRequired,
                    mister_magik_catalog::builder_protocol::CatalogChangeReason::RepairRequired => {
                        CatalogRecoveryMode::RepairRequired
                    }
                };
                if mode == CatalogRecoveryMode::RepairRequired {
                    effects.push(CatalogSessionEffect::PersistCatalogFailure {
                        detail: detail.clone(),
                        mode,
                        has_stale_catalog: context.catalog_ready && !context.catalog_partial,
                        system_id: None,
                    });
                }
                effects.push(CatalogSessionEffect::Lifecycle(
                    LauncherLifecycleInput::CatalogRecoveryRequired {
                        error: detail,
                        has_stale_catalog: context.catalog_ready && !context.catalog_partial,
                        mode,
                    },
                ));
                effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
                self.games_found_counter.reset();
            }
            CatalogWorkerMessage::Done => {
                self.refresh_done = true;
                self.foreground_update = false;
                self.refresh_failed = false;
                effects.push(CatalogSessionEffect::FinishMediaWorker);
                effects.push(CatalogSessionEffect::CatalogValidationFinished);
                effects.push(CatalogSessionEffect::CatalogBuildFinished);
                if context.catalog_ready {
                    effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
                    self.games_found_counter.reset();
                }
            }
        }
        effects
    }

    pub(super) fn continue_with_stale_library(&mut self) -> CatalogSessionEffects {
        let mut effects = CatalogSessionEffects::default();
        effects.push(CatalogSessionEffect::RequestLibraryRebuildOnNextBoot);
        self.refresh_done = true;
        self.foreground_update = false;
        self.deferred_worker = None;
        self.refresh_failed = false;
        self.games_found_counter.reset();
        effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
        effects
    }

    pub(super) fn rebuild_library(&mut self, root: String) -> CatalogSessionEffects {
        let mut effects = CatalogSessionEffects::default();
        effects.event("library_rebuild_requested", "source=dialog");
        self.refresh_done = false;
        self.foreground_update = false;
        self.deferred_worker = None;
        self.refresh_failed = false;
        self.reset_counter_metrics();
        self.games_found_counter.reset();
        effects.push(CatalogSessionEffect::StartCatalogWorker(
            CatalogWorkerStart {
                root,
                request: CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                initial_cache: CatalogWorkerInitialCache::AlreadyLoadedReady,
                execution_mode: CatalogExecutionMode::BackgroundInteractive,
            },
        ));
        effects.ui(catalog_rebuild_started_intent(self.foreground_update));
        effects
    }

    pub(super) fn rebuild_database(&mut self, root: String) -> CatalogSessionEffects {
        let mut effects = CatalogSessionEffects::default();
        effects.event(
            "database_rebuild_requested",
            "source=settings scope=all-systems",
        );
        self.refresh_done = false;
        self.foreground_update = false;
        self.deferred_worker = None;
        self.refresh_failed = false;
        self.reset_counter_metrics();
        self.games_found_counter.reset();
        effects.push(CatalogSessionEffect::CatalogPlanReady {
            system_ids: Vec::new(),
            all_published_systems: true,
        });
        effects.ui(catalog_plan_ready_intent(0, true));
        effects.push(CatalogSessionEffect::StartCatalogWorker(
            CatalogWorkerStart {
                root,
                request: CatalogWorkerRequest::RECONCILE_ALL_SYSTEMS,
                initial_cache: CatalogWorkerInitialCache::AlreadyLoadedReady,
                execution_mode: CatalogExecutionMode::BackgroundInteractive,
            },
        ));
        effects
    }

    pub(super) fn qualification_fresh_rebuild(&mut self, root: String) -> CatalogSessionEffects {
        let mut effects = CatalogSessionEffects::default();
        effects.event(
            "library_fresh_rebuild_requested",
            "source=latch-v5-qualification",
        );
        self.refresh_done = false;
        self.foreground_update = true;
        self.deferred_worker = None;
        self.refresh_failed = false;
        self.reset_counter_metrics();
        self.games_found_counter.reset();
        effects.push(CatalogSessionEffect::StartCatalogWorker(
            CatalogWorkerStart {
                root,
                request: CatalogWorkerRequest::FreshBuild,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            },
        ));
        effects.ui(catalog_rebuild_started_intent(self.foreground_update));
        effects
    }

    fn handle_progress(
        &mut self,
        context: CatalogWorkerMessageContext,
        title: String,
        detail: String,
        percent: i32,
        _now: Instant,
        effects: &mut CatalogSessionEffects,
    ) {
        let intent = CatalogProgressUiIntent::from_worker_progress(
            CatalogWorkerUiContext {
                catalog_ready: context.catalog_ready,
                screen: context.screen,
                foreground_update: self.foreground_update,
            },
            title,
            detail,
            percent,
        );
        if intent.failed {
            self.refresh_failed = true;
        }
        if let Some(counter_target) = intent
            .counter_target
            .filter(|target| counter_climb_target_is_meaningful(target.target))
        {
            let target = counter_target.target;
            let visible_counter_before = self.games_found_counter.displayed;
            if counter_target.phase == CatalogCounterPhase::Bootstrap
                && !self.bootstrap_counter_climb_logged
            {
                self.bootstrap_counter_climb_logged = true;
                effects.event("bootstrap_counter_climb", format!("target={target}"));
            }
            if counter_target.phase == CatalogCounterPhase::Bootstrap
                && !self.bootstrap_counter_sustained_climb_logged
                && counter_climb_target_is_sustained(target)
            {
                self.bootstrap_counter_sustained_climb_logged = true;
                effects.event(
                    "bootstrap_counter_sustained_climb",
                    format!("target={target}"),
                );
            }
            if counter_target.phase == CatalogCounterPhase::FullScan
                && !self.full_scan_counter_climb_logged
                && counter_climb_target_overtakes_visible(target, visible_counter_before)
            {
                self.full_scan_counter_climb_logged = true;
                effects.event("full_scan_counter_climb", format!("target={target}"));
            }
        }
        let detail = self
            .games_found_counter
            .progress_detail(&intent.title, &intent.detail);
        effects.ui(intent.ui_with_detail(detail));
    }

    fn handle_ready(
        &mut self,
        catalog_ready: bool,
        ready_catalog: ArcadeCatalog,
        summary: Option<library_db::LibraryRefreshSummary>,
        load_us: u64,
        source: CatalogSource,
        durable_save_pending: bool,
        generation_fingerprint: Option<String>,
        publication_ack: Option<mpsc::Sender<()>>,
        effects: &mut CatalogSessionEffects,
    ) {
        let cached_before_refresh = summary.is_none() && !durable_save_pending;
        let duplicate_cached_catalog = !self.summary_only
            && duplicate_cached_catalog_ready(catalog_ready, cached_before_refresh);
        let validation_already_finished = self.refresh_done;
        self.refresh_done =
            validation_already_finished || (!cached_before_refresh && !durable_save_pending);
        let catalog_len = ready_catalog.len();
        if !duplicate_cached_catalog {
            self.summary_only = false;
            effects.push(CatalogSessionEffect::RequestMediaCatalogSeed);
            effects.push(CatalogSessionEffect::UseCatalog {
                catalog: ready_catalog,
                load_us,
                source,
                durable: !durable_save_pending,
                generation_fingerprint,
                publication_ack: publication_ack.clone(),
            });
            effects.event(
                "library_ready",
                format!("games={catalog_len} load_us={load_us}"),
            );
        }
        if let Some(summary) = summary {
            effects.push(CatalogSessionEffect::FinishMediaWorkerIfNoCatalogSeedPending);
            self.foreground_update = false;
            self.refresh_failed = false;
            let event = if summary.skipped {
                "library_db_unchanged"
            } else {
                "library_db_saved"
            };
            if !self.persisted_summary_seen {
                effects.event(event, format_library_refresh_summary(&summary));
            }
            if !summary.skipped {
                push_catalog_coverage_diagnostic(&summary, effects);
            }
        }
        if duplicate_cached_catalog {
            if let Some(publication_ack) = publication_ack {
                let _ = publication_ack.send(());
            }
            if self.refresh_failed || self.foreground_update {
                self.refresh_done = true;
                self.foreground_update = false;
                effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
                self.games_found_counter.reset();
                effects.push(CatalogSessionEffect::Lifecycle(
                    LauncherLifecycleInput::CatalogRecoveryRequired {
                        error: "The catalog update did not complete.".to_string(),
                        has_stale_catalog: true,
                        mode: CatalogRecoveryMode::PersistenceFailure { transient: false },
                    },
                ));
                effects.event(
                    "library_rebuild_fallback_catalog_ready",
                    format!("games={catalog_len} load_us={load_us}"),
                );
            }
            return;
        }
        self.games_found_counter.reset();
        if cached_before_refresh && !validation_already_finished {
            effects.ui(cached_catalog_validation_intent(
                self.foreground_update,
                catalog_len,
            ));
        } else if durable_save_pending {
            self.foreground_update = false;
            effects.ui(LauncherWorkerUiIntent::ShowCatalogBackgroundScan);
        } else {
            effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
        }
        effects.push(CatalogSessionEffect::SyncCatalogBridge);
    }

    fn reset_counter_metrics(&mut self) {
        self.bootstrap_counter_climb_logged = false;
        self.bootstrap_counter_sustained_climb_logged = false;
        self.full_scan_counter_climb_logged = false;
    }
}

fn catalog_failure_is_transient(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    ![
        "unsupported",
        "schema",
        "corrupt",
        "checksum",
        "identity",
        "exceeds configured",
        "does not match",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

fn catalog_failure_is_format_upgrade(error: &str) -> bool {
    let (expected, actual) = crate::catalog_failure_report::schema_versions(error);
    match (
        expected.and_then(|value| value.parse::<u64>().ok()),
        actual.and_then(|value| value.parse::<u64>().ok()),
    ) {
        (Some(expected), Some(actual)) => actual < expected,
        _ => false,
    }
}

pub(super) fn consume_library_rebuild_marker(worker_enabled: bool, start: Instant) -> bool {
    if !worker_enabled {
        return false;
    }
    match launcher::consume_library_rebuild_on_next_boot() {
        Ok(pending) => {
            if pending {
                print_startup_event(start, "library_rebuild_marker_consumed", "pending=1");
            }
            pending
        }
        Err(e) => {
            crate::ui_errln!("failed to consume library rebuild marker: {e}");
            print_startup_event(start, "library_rebuild_marker_consume_failed", e);
            false
        }
    }
}

fn format_library_refresh_summary(summary: &library_db::LibraryRefreshSummary) -> String {
    format!(
        "bytes={} scan_us={} discover_us={} classify_us={} import_us={} discoveries={} normal_files={} containers={} entries={} audit_rows={}",
        summary.bytes,
        summary.scan_us,
        summary.discover_us,
        summary.classify_us,
        summary.import_us,
        summary.discoveries,
        summary.normal_files,
        summary.containers,
        summary.entries,
        summary.audit_rows
    )
}

fn push_catalog_coverage_diagnostic(
    summary: &library_db::LibraryRefreshSummary,
    effects: &mut CatalogSessionEffects,
) {
    if summary.audit_rows == 0 {
        return;
    }
    crate::ui_errln!(
        "catalog coverage audit: rows={} (query catalog_audit for details)",
        summary.audit_rows
    );
    effects.event(
        "catalog_coverage_audit",
        format!("rows={}", summary.audit_rows),
    );
}

fn duplicate_cached_catalog_ready(catalog_ready: bool, cached_before_refresh: bool) -> bool {
    catalog_ready && cached_before_refresh
}

#[derive(Debug, Default)]
pub(super) struct GamesFoundCounter {
    displayed: usize,
}

impl GamesFoundCounter {
    fn progress_detail(&mut self, title: &str, detail: &str) -> Option<String> {
        let phase = CatalogCounterPhase::for_title(title);
        let target = phase.and_then(|_| parse_games_found_detail(detail));
        let Some(target) = target else {
            self.reset();
            return None;
        };
        self.displayed = self.displayed.max(target);
        Some(format_games_found(self.displayed))
    }

    fn reset(&mut self) {
        self.displayed = 0;
    }
}

fn format_games_found(count: usize) -> String {
    format!("Games found: {count}")
}

pub(super) fn counter_climb_target_is_meaningful(target: usize) -> bool {
    target >= 50
}

pub(super) fn counter_climb_target_is_sustained(target: usize) -> bool {
    target >= 500
}

pub(super) fn counter_climb_target_overtakes_visible(target: usize, displayed: usize) -> bool {
    target > displayed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{arcade_catalog, arcade_game, arcade_system};

    fn catalog_with_games(count: usize) -> ArcadeCatalog {
        let games = (0..count)
            .map(|index| {
                arcade_game(format!("Game {index}"))
                    .path(format!("/media/fat/_Arcade/game-{index}.mra"))
                    .build()
            })
            .collect();
        arcade_catalog(games, vec![arcade_system("arcade", count)])
    }

    fn effect_names(effects: CatalogSessionEffects) -> Vec<&'static str> {
        effects
            .into_effects()
            .into_iter()
            .filter(|effect| {
                !matches!(
                    effect,
                    CatalogSessionEffect::CatalogBuildStarted
                        | CatalogSessionEffect::CatalogPlanReady { .. }
                        | CatalogSessionEffect::CatalogSystemDiscovered { .. }
                        | CatalogSessionEffect::CatalogSystemScanning { .. }
                        | CatalogSessionEffect::CatalogSystemPrepared { .. }
                        | CatalogSessionEffect::CatalogManifestPublished { .. }
                        | CatalogSessionEffect::CatalogSystemUpdateFailed { .. }
                        | CatalogSessionEffect::CatalogSystemHydrationFailed { .. }
                        | CatalogSessionEffect::PersistCatalogFailure { .. }
                        | CatalogSessionEffect::CatalogBuildFinished
                        | CatalogSessionEffect::ApplySystemShard { .. }
                )
            })
            .map(|effect| match effect {
                CatalogSessionEffect::StartupEvent(_) => "event",
                CatalogSessionEffect::UseCatalog { .. } => "catalog",
                CatalogSessionEffect::MarkCatalogDurable { .. } => "mark-durable",
                CatalogSessionEffect::ConfirmCatalogSeed => "confirm-seed",
                CatalogSessionEffect::DiscardPartialCatalog => "discard-partial",
                CatalogSessionEffect::ApplySearchResult { .. } => "search-result",
                CatalogSessionEffect::FailSearchRequest { .. } => "search-failed",
                CatalogSessionEffect::SyncCatalogBridge => "sync",
                CatalogSessionEffect::CatalogBuildStarted
                | CatalogSessionEffect::CatalogPlanReady { .. }
                | CatalogSessionEffect::CatalogSystemDiscovered { .. }
                | CatalogSessionEffect::CatalogSystemScanning { .. }
                | CatalogSessionEffect::CatalogSystemPrepared { .. }
                | CatalogSessionEffect::CatalogManifestPublished { .. }
                | CatalogSessionEffect::CatalogSystemUpdateFailed { .. }
                | CatalogSessionEffect::CatalogSystemHydrationFailed { .. }
                | CatalogSessionEffect::PersistCatalogFailure { .. }
                | CatalogSessionEffect::CatalogBuildFinished
                | CatalogSessionEffect::ApplySystemShard { .. } => {
                    unreachable!("presentation effects filtered above")
                }
                CatalogSessionEffect::Ui(_) => "ui",
                CatalogSessionEffect::FinishMediaWorker => "finish-media",
                CatalogSessionEffect::FinishMediaWorkerIfNoCatalogSeedPending => {
                    "finish-media-if-no-seed"
                }
                CatalogSessionEffect::CatalogValidationFinished => "catalog-validation-finished",
                CatalogSessionEffect::RequestMediaCatalogSeed => "request-media-seed",
                CatalogSessionEffect::MediaSystemDiscovered { .. } => "media-system-discovered",
                CatalogSessionEffect::RequestLibraryRebuildOnNextBoot => "request-rebuild-marker",
                CatalogSessionEffect::Confirm(_) => "confirm",
                CatalogSessionEffect::Lifecycle(_) => "lifecycle",
                CatalogSessionEffect::StartCatalogWorker(_) => "start-worker",
            })
            .collect()
    }

    fn effect_and_ui_names(
        effects: CatalogSessionEffects,
    ) -> (Vec<&'static str>, Vec<&'static str>) {
        let mut effect_names = Vec::new();
        let mut ui_names = Vec::new();
        for effect in effects.into_effects() {
            if matches!(
                effect,
                CatalogSessionEffect::CatalogBuildStarted
                    | CatalogSessionEffect::CatalogPlanReady { .. }
                    | CatalogSessionEffect::CatalogSystemDiscovered { .. }
                    | CatalogSessionEffect::CatalogSystemScanning { .. }
                    | CatalogSessionEffect::CatalogSystemPrepared { .. }
                    | CatalogSessionEffect::CatalogManifestPublished { .. }
                    | CatalogSessionEffect::CatalogSystemUpdateFailed { .. }
                    | CatalogSessionEffect::CatalogSystemHydrationFailed { .. }
                    | CatalogSessionEffect::PersistCatalogFailure { .. }
                    | CatalogSessionEffect::CatalogBuildFinished
                    | CatalogSessionEffect::ApplySystemShard { .. }
            ) {
                continue;
            }
            match effect {
                CatalogSessionEffect::StartupEvent(_) => effect_names.push("event"),
                CatalogSessionEffect::UseCatalog { .. } => effect_names.push("catalog"),
                CatalogSessionEffect::MarkCatalogDurable { .. } => {
                    effect_names.push("mark-durable")
                }
                CatalogSessionEffect::ConfirmCatalogSeed => effect_names.push("confirm-seed"),
                CatalogSessionEffect::DiscardPartialCatalog => effect_names.push("discard-partial"),
                CatalogSessionEffect::ApplySearchResult { .. } => {
                    effect_names.push("search-result")
                }
                CatalogSessionEffect::FailSearchRequest { .. } => {
                    effect_names.push("search-failed")
                }
                CatalogSessionEffect::SyncCatalogBridge => effect_names.push("sync"),
                CatalogSessionEffect::CatalogBuildStarted
                | CatalogSessionEffect::CatalogPlanReady { .. }
                | CatalogSessionEffect::CatalogSystemDiscovered { .. }
                | CatalogSessionEffect::CatalogSystemScanning { .. }
                | CatalogSessionEffect::CatalogSystemPrepared { .. }
                | CatalogSessionEffect::CatalogManifestPublished { .. }
                | CatalogSessionEffect::CatalogSystemUpdateFailed { .. }
                | CatalogSessionEffect::CatalogSystemHydrationFailed { .. }
                | CatalogSessionEffect::PersistCatalogFailure { .. }
                | CatalogSessionEffect::CatalogBuildFinished
                | CatalogSessionEffect::ApplySystemShard { .. } => {
                    unreachable!("presentation effects filtered above")
                }
                CatalogSessionEffect::Ui(intent) => {
                    effect_names.push("ui");
                    ui_names.push(match intent {
                        LauncherWorkerUiIntent::CatalogScan(_) => "catalog-scan",
                        LauncherWorkerUiIntent::ClearCatalogScan => "clear-catalog-scan",
                        LauncherWorkerUiIntent::ShowCatalogBackgroundScan => "show-background-scan",
                        LauncherWorkerUiIntent::HideCatalogBackgroundScan => "hide-background-scan",
                        LauncherWorkerUiIntent::InfoDatabaseBuild(_) => "info-database-build",
                        LauncherWorkerUiIntent::MediaProgress { .. } => "media-progress",
                        LauncherWorkerUiIntent::None => "none",
                    });
                }
                CatalogSessionEffect::FinishMediaWorker => effect_names.push("finish-media"),
                CatalogSessionEffect::FinishMediaWorkerIfNoCatalogSeedPending => {
                    effect_names.push("finish-media-if-no-seed")
                }
                CatalogSessionEffect::CatalogValidationFinished => {
                    effect_names.push("catalog-validation-finished")
                }
                CatalogSessionEffect::RequestMediaCatalogSeed => {
                    effect_names.push("request-media-seed")
                }
                CatalogSessionEffect::MediaSystemDiscovered { .. } => {
                    effect_names.push("media-system-discovered")
                }
                CatalogSessionEffect::RequestLibraryRebuildOnNextBoot => {
                    effect_names.push("request-rebuild-marker")
                }
                CatalogSessionEffect::Confirm(_) => effect_names.push("confirm"),
                CatalogSessionEffect::Lifecycle(_) => effect_names.push("lifecycle"),
                CatalogSessionEffect::StartCatalogWorker(_) => effect_names.push("start-worker"),
            }
        }
        (effect_names, ui_names)
    }

    fn catalog_scan_statuses(effects: CatalogSessionEffects) -> Vec<CatalogScanBridgeStatus> {
        effects
            .into_effects()
            .into_iter()
            .filter_map(|effect| match effect {
                CatalogSessionEffect::Ui(LauncherWorkerUiIntent::CatalogScan(status)) => {
                    Some(status)
                }
                _ => None,
            })
            .collect()
    }

    fn database_build_values(effects: CatalogSessionEffects) -> Vec<String> {
        effects
            .into_effects()
            .into_iter()
            .filter_map(|effect| match effect {
                CatalogSessionEffect::Ui(LauncherWorkerUiIntent::InfoDatabaseBuild(value)) => {
                    Some(value)
                }
                _ => None,
            })
            .collect()
    }

    fn refresh_summary() -> library_db::LibraryRefreshSummary {
        library_db::LibraryRefreshSummary {
            skipped: false,
            scan_us: 10,
            discover_us: 11,
            classify_us: 12,
            import_us: 13,
            bytes: 14,
            normal_files: 15,
            containers: 16,
            entries: 17,
            audit_rows: 0,
            discoveries: 18,
        }
    }

    #[test]
    fn persisted_catalog_displays_builder_duration_in_whole_seconds() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(true);
        let values = database_build_values(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Persisted {
                summary: refresh_summary(),
                completed_build_seconds: Some(119),
                generation_fingerprint: None,
            },
            now,
        ));

        assert_eq!(values, vec!["119 seconds"]);
    }

    #[test]
    fn progressive_presentation_effects_follow_worker_event_sequence() {
        let now = Instant::now();
        let context = || CatalogWorkerMessageContext {
            catalog_ready: false,
            catalog_partial: false,
            screen: Screen::Home,
            media_gate: None,
        };
        let mut session = LauncherCatalogSession::new(false);

        let started = session.handle_worker_message(
            context(),
            CatalogWorkerMessage::FreshCleanupStarted,
            now,
        );
        assert!(
            started
                .into_effects()
                .into_iter()
                .any(|effect| matches!(effect, CatalogSessionEffect::CatalogBuildStarted))
        );

        let discovered = session.handle_worker_message(
            context(),
            CatalogWorkerMessage::SystemDiscovered {
                system_id: "snes".to_string(),
            },
            now,
        );
        assert!(discovered.into_effects().into_iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::CatalogSystemDiscovered { system_id } if system_id == "snes"
        )));

        let ready = session.handle_worker_message(
            context(),
            CatalogWorkerMessage::SystemShardReady {
                system_id: "snes".to_string(),
                catalog: empty_arcade_catalog("/tmp"),
                base_catalog_version: 7,
                game_count: 0,
                prepare_us: 42,
                profile: SystemEntryCatalogProfile::default(),
            },
            now,
        );
        let ready_effects = ready.into_effects().into_iter().collect::<Vec<_>>();
        assert!(ready_effects.iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::ApplySystemShard { system_id, .. } if system_id == "snes"
        )));
        assert!(!ready_effects.iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::CatalogSystemUpdateFailed { .. }
                | CatalogSessionEffect::CatalogSystemHydrationFailed { .. }
        )));

        let finished = session.handle_worker_message(
            context(),
            CatalogWorkerMessage::Persisted {
                summary: refresh_summary(),
                completed_build_seconds: Some(1),
                generation_fingerprint: None,
            },
            now,
        );
        assert!(
            finished
                .into_effects()
                .into_iter()
                .any(|effect| matches!(effect, CatalogSessionEffect::CatalogBuildFinished))
        );
    }

    #[test]
    fn update_and_hydration_failures_emit_distinct_state_effects() {
        let now = Instant::now();
        let context = || CatalogWorkerMessageContext {
            catalog_ready: true,
            catalog_partial: false,
            screen: Screen::Home,
            media_gate: None,
        };
        let mut session = LauncherCatalogSession::new(false);

        let update_effects = session
            .handle_worker_message(
                context(),
                CatalogWorkerMessage::SystemUpdateFailed {
                    system_id: "snes".to_string(),
                    error: "update failed".to_string(),
                },
                now,
            )
            .into_effects()
            .into_iter()
            .collect::<Vec<_>>();
        assert!(update_effects.iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::CatalogSystemUpdateFailed { system_id }
                if system_id == "snes"
        )));
        assert!(!update_effects.iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::CatalogSystemHydrationFailed { .. }
        )));

        let hydration_effects = session
            .handle_worker_message(
                context(),
                CatalogWorkerMessage::SystemShardFailed {
                    system_id: "snes".to_string(),
                    error: "load failed".to_string(),
                },
                now,
            )
            .into_effects()
            .into_iter()
            .collect::<Vec<_>>();
        assert!(hydration_effects.iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::CatalogSystemHydrationFailed { system_id }
                if system_id == "snes"
        )));
        assert!(!hydration_effects.iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::CatalogSystemUpdateFailed { .. }
        )));
    }

    #[test]
    fn ready_catalog_replaces_cache_and_requests_media_seed() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                summary: None,
                load_us: 42,
                source: CatalogSource::FullSqlite,
                durable_save_pending: false,
                generation_fingerprint: None,
                publication_ack: None,
            },
            now,
        );

        assert_eq!(
            effect_names(effects),
            vec!["request-media-seed", "catalog", "event", "ui", "sync"]
        );
        assert!(!session.refresh_done());
    }

    #[test]
    fn hydrated_catalog_defers_validation_to_a_fresh_idle_gated_worker() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Arcade,
                media_gate: None,
            },
            CatalogWorkerMessage::HydrationDoneNeedsValidation {
                root: "/media/fat".into(),
            },
            now,
        );

        assert_eq!(effect_names(effects), vec!["event"]);
        assert!(!session.refresh_done());
        assert!(
            session
                .maybe_start_deferred_worker(false, true, false, now, Duration::ZERO, || true)
                .is_none()
        );
        let worker = session
            .maybe_start_deferred_worker(false, true, true, now, Duration::ZERO, || true)
            .expect("validation worker after idle gate opens");
        assert_eq!(worker.root, "/media/fat");
        assert_eq!(worker.request, CatalogWorkerRequest::CheckStamp);
        assert_eq!(
            worker.initial_cache,
            CatalogWorkerInitialCache::AlreadyLoadedReady
        );
        assert_eq!(
            worker.execution_mode,
            CatalogExecutionMode::BackgroundInteractive
        );
    }

    #[test]
    fn early_ready_keeps_refresh_open_until_persisted() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(false);
        let (ready_effects, ready_ui) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                summary: None,
                load_us: 42,
                source: CatalogSource::FreshBuild,
                durable_save_pending: true,
                generation_fingerprint: None,
                publication_ack: None,
            },
            now,
        ));

        assert_eq!(
            ready_effects,
            vec!["request-media-seed", "catalog", "event", "ui", "sync"]
        );
        assert_eq!(ready_ui, vec!["show-background-scan"]);
        assert!(!session.refresh_done());

        let (persisted_effects, persisted_ui) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Persisted {
                summary: refresh_summary(),
                completed_build_seconds: Some(119),
                generation_fingerprint: None,
            },
            now,
        ));

        assert_eq!(
            persisted_effects,
            vec![
                "finish-media-if-no-seed",
                "catalog-validation-finished",
                "event",
                "mark-durable",
                "ui",
                "ui"
            ]
        );
        assert_eq!(
            persisted_ui,
            vec!["info-database-build", "hide-background-scan"]
        );
        assert!(session.refresh_done());
        assert!(!session.foreground_update());
        assert!(!session.refresh_failed);
    }

    #[test]
    fn foreground_rebuild_ready_moves_persistence_to_background_scan() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(true);
        let (ready_effects, ready_ui) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(4),
                summary: None,
                load_us: 42,
                source: CatalogSource::FreshBuild,
                durable_save_pending: true,
                generation_fingerprint: None,
                publication_ack: None,
            },
            now,
        ));

        assert_eq!(
            ready_effects,
            vec!["request-media-seed", "catalog", "event", "ui", "sync"]
        );
        assert_eq!(ready_ui, vec!["show-background-scan"]);
        assert!(!session.refresh_done());
        assert!(!session.foreground_update());

        let (_, progress_ui) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Progress {
                title: "Saving library".to_string(),
                detail: "Writing catalog database before opening launcher...".to_string(),
                percent: -1,
                metadata: None,
            },
            now,
        ));

        assert_eq!(progress_ui, vec!["catalog-scan"]);
        assert!(!session.foreground_update());

        let (persisted_effects, persisted_ui) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Persisted {
                summary: refresh_summary(),
                completed_build_seconds: Some(119),
                generation_fingerprint: None,
            },
            now,
        ));

        assert_eq!(
            persisted_effects,
            vec![
                "finish-media-if-no-seed",
                "catalog-validation-finished",
                "event",
                "mark-durable",
                "ui",
                "ui"
            ]
        );
        assert_eq!(
            persisted_ui,
            vec!["info-database-build", "hide-background-scan"]
        );
        assert!(session.refresh_done());
        assert!(!session.foreground_update());
        assert!(!session.refresh_failed);
    }

    #[test]
    fn persisted_catalog_with_audit_rows_logs_coverage_diagnostic_without_prompt() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(true);
        let mut summary = refresh_summary();
        summary.audit_rows = 2;

        let effects = effect_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Persisted {
                summary,
                completed_build_seconds: Some(119),
                generation_fingerprint: None,
            },
            now,
        ));

        assert_eq!(
            effects,
            vec![
                "finish-media-if-no-seed",
                "catalog-validation-finished",
                "event",
                "mark-durable",
                "ui",
                "event",
                "ui"
            ]
        );
    }

    #[test]
    fn ready_catalog_with_saved_audit_rows_logs_coverage_diagnostic_without_prompt() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(true);
        let mut summary = refresh_summary();
        summary.audit_rows = 2;

        let effects = effect_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                summary: Some(summary),
                load_us: 42,
                source: CatalogSource::FreshBuild,
                durable_save_pending: false,
                generation_fingerprint: None,
                publication_ack: None,
            },
            now,
        ));

        assert_eq!(
            effects,
            vec![
                "request-media-seed",
                "catalog",
                "event",
                "finish-media-if-no-seed",
                "event",
                "event",
                "ui",
                "sync"
            ]
        );
    }

    #[test]
    fn late_cached_ready_after_terminal_validation_does_not_reopen_refresh() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(false);
        session.note_summary_seed_ready();

        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: true,
                screen: Screen::Arcade,
                media_gate: None,
            },
            CatalogWorkerMessage::Unchanged {
                summary: refresh_summary(),
            },
            now,
        );
        assert!(
            effects
                .into_effects()
                .into_iter()
                .any(|effect| matches!(effect, CatalogSessionEffect::ConfirmCatalogSeed))
        );
        assert!(session.refresh_done());

        let (effects, ui_effects) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Arcade,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                summary: None,
                load_us: 42,
                source: CatalogSource::FullSqlite,
                durable_save_pending: false,
                generation_fingerprint: None,
                publication_ack: None,
            },
            now,
        ));

        assert_eq!(
            effects,
            vec!["request-media-seed", "catalog", "event", "ui", "sync"]
        );
        assert_eq!(ui_effects, vec!["clear-catalog-scan"]);
        assert!(session.refresh_done());
    }

    #[test]
    fn duplicate_cached_ready_after_failed_foreground_rebuild_prompts_fallback() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(true);
        session.note_cached_catalog_ready();
        let _ = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Progress {
                title: "Library scan failed".to_string(),
                detail: "disk unavailable".to_string(),
                percent: -1,
                metadata: None,
            },
            now,
        );

        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                summary: None,
                load_us: 42,
                source: CatalogSource::FullSqlite,
                durable_save_pending: false,
                generation_fingerprint: None,
                publication_ack: None,
            },
            now,
        );

        assert_eq!(effect_names(effects), vec!["ui", "lifecycle", "event"]);
        assert!(session.refresh_done());
        assert!(!session.foreground_update());
    }

    #[test]
    fn survivability_failed_foreground_rebuild_with_cached_catalog_prompts_fallback() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(true);
        session.note_cached_catalog_ready();
        let _ = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Progress {
                title: "Library load failed".to_string(),
                detail: "sqlite projection corrupt".to_string(),
                percent: -1,
                metadata: None,
            },
            now,
        );

        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                summary: None,
                load_us: 42,
                source: CatalogSource::FullSqlite,
                durable_save_pending: false,
                generation_fingerprint: None,
                publication_ack: None,
            },
            now,
        );

        assert_eq!(effect_names(effects), vec!["ui", "lifecycle", "event"]);
        assert!(session.refresh_done());
        assert!(!session.foreground_update());
    }

    #[test]
    fn persistence_failure_replaces_finalizing_progress_with_error_state() {
        let mut session = LauncherCatalogSession::new(true);
        let (effects, ui_effects) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::PersistenceFailed {
                error: "read launcher catalog row".to_string(),
            },
            Instant::now(),
        ));

        assert_eq!(
            effects,
            vec![
                "finish-media",
                "catalog-validation-finished",
                "event",
                "ui",
                "lifecycle"
            ]
        );
        assert_eq!(ui_effects, vec!["clear-catalog-scan"]);
        assert!(session.refresh_done());
        assert!(!session.foreground_update());
        assert!(session.refresh_failed);
    }

    #[test]
    fn survivability_first_boot_persistence_failure_keeps_ram_catalog_and_reports_error() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(true);
        let ready_effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(4),
                summary: None,
                load_us: 42,
                source: CatalogSource::FreshBuild,
                durable_save_pending: true,
                generation_fingerprint: None,
                publication_ack: None,
            },
            now,
        );

        assert_eq!(
            effect_names(ready_effects),
            vec!["request-media-seed", "catalog", "event", "ui", "sync"]
        );
        assert!(!session.refresh_done());

        let (effects, ui_effects) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::PersistenceFailed {
                error: "insert profile: UNIQUE constraint failed".to_string(),
            },
            now,
        ));

        assert!(effects.contains(&"lifecycle"));
        assert_eq!(ui_effects, vec!["clear-catalog-scan"]);
        assert!(session.refresh_done());
        assert!(session.refresh_failed);
    }

    #[test]
    fn persistence_failure_after_early_ready_keeps_session_catalog_available() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(true);
        let _ = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(4),
                summary: None,
                load_us: 42,
                source: CatalogSource::FreshBuild,
                durable_save_pending: true,
                generation_fingerprint: None,
                publication_ack: None,
            },
            now,
        );
        assert!(!session.refresh_done());

        let (effects, ui_effects) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::PersistenceFailed {
                error: "publish sqlite catalog".to_string(),
            },
            now,
        ));

        assert_eq!(
            effects,
            vec![
                "finish-media",
                "catalog-validation-finished",
                "event",
                "ui",
                "lifecycle"
            ]
        );
        assert_eq!(ui_effects, vec!["clear-catalog-scan"]);
        assert!(session.refresh_done());
        assert!(!session.foreground_update());
        assert!(session.refresh_failed);
    }

    #[test]
    fn changed_catalog_opens_stale_library_dialog() {
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Changed {
                detail: "Catalog stamp changed; rebuild required.".to_string(),
                reason: mister_magik_catalog::builder_protocol::CatalogChangeReason::InputsChanged,
            },
            Instant::now(),
        );

        assert_eq!(
            effect_names(effects),
            vec![
                "finish-media",
                "catalog-validation-finished",
                "event",
                "lifecycle",
                "ui"
            ]
        );
        assert!(session.refresh_done());
        assert!(!session.foreground_update());
    }

    #[test]
    fn stale_library_continue_requests_rebuild_marker_without_rebuilding_now() {
        let mut session = LauncherCatalogSession::new(false);

        assert_eq!(
            effect_names(session.continue_with_stale_library()),
            vec!["request-rebuild-marker", "ui"]
        );
        assert!(session.refresh_done());
        assert!(!session.foreground_update());
    }

    #[test]
    fn rebuild_library_starts_background_warm_rebuild() {
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.rebuild_library("/media/fat/_Arcade".to_string());

        assert!(!session.refresh_done());
        assert!(!session.foreground_update());
        let worker = effects
            .effects
            .iter()
            .find_map(|effect| match effect {
                CatalogSessionEffect::StartCatalogWorker(worker) => Some(worker),
                _ => None,
            })
            .expect("catalog worker");
        assert_eq!(
            worker.request,
            CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS
        );
        assert_eq!(
            worker.execution_mode,
            CatalogExecutionMode::BackgroundInteractive
        );
        assert_eq!(effect_names(effects), vec!["event", "start-worker", "ui"]);
    }

    #[test]
    fn settings_database_rebuild_marks_all_systems_and_stays_background() {
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.rebuild_database("/media/fat/_Arcade".to_string());

        assert!(!session.refresh_done());
        assert!(!session.foreground_update());
        assert!(effects.effects.iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::CatalogPlanReady {
                system_ids,
                all_published_systems: true,
            } if system_ids.is_empty()
        )));
        let worker = effects
            .effects
            .iter()
            .find_map(|effect| match effect {
                CatalogSessionEffect::StartCatalogWorker(worker) => Some(worker),
                _ => None,
            })
            .expect("catalog worker");
        assert_eq!(worker.request, CatalogWorkerRequest::RECONCILE_ALL_SYSTEMS);
        assert_eq!(
            worker.initial_cache,
            CatalogWorkerInitialCache::AlreadyLoadedReady
        );
        assert_eq!(
            worker.execution_mode,
            CatalogExecutionMode::BackgroundInteractive
        );
        assert_eq!(effect_names(effects), vec!["event", "ui", "start-worker"]);
    }

    #[test]
    fn qualification_rebuild_is_a_fresh_foreground_generation() {
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.qualification_fresh_rebuild("/media/fat/_Arcade".to_string());
        let worker = effects
            .effects
            .iter()
            .find_map(|effect| match effect {
                CatalogSessionEffect::StartCatalogWorker(worker) => Some(worker),
                _ => None,
            })
            .expect("catalog worker");
        assert_eq!(worker.request, CatalogWorkerRequest::FreshBuild);
        assert_eq!(
            worker.execution_mode,
            CatalogExecutionMode::ForegroundExclusive
        );
    }

    #[test]
    fn deferred_catalog_worker_waits_for_first_visible_copy_and_delay() {
        let mut session = LauncherCatalogSession::new(false);
        let now = Instant::now();
        let delay = Duration::from_millis(50);

        session.defer_catalog_worker(
            "/media/fat/_Arcade".to_string(),
            CatalogWorkerRequest::CheckStamp,
            CatalogWorkerInitialCache::ProbeSqlite,
            CatalogExecutionMode::BackgroundInteractive,
        );

        assert!(
            session
                .maybe_start_deferred_worker(false, false, true, now + delay, delay, || true)
                .is_none()
        );
        assert!(
            session
                .maybe_start_deferred_worker(
                    false,
                    true,
                    true,
                    now + Duration::from_millis(20),
                    delay,
                    || true,
                )
                .is_none()
        );

        let worker = session
            .maybe_start_deferred_worker(
                false,
                true,
                true,
                now + Duration::from_millis(70),
                delay,
                || true,
            )
            .expect("deferred worker");

        assert_eq!(worker.root, "/media/fat/_Arcade");
        assert_eq!(worker.request, CatalogWorkerRequest::CheckStamp);
        assert_eq!(worker.initial_cache, CatalogWorkerInitialCache::ProbeSqlite);
        assert_eq!(
            worker.execution_mode,
            CatalogExecutionMode::BackgroundInteractive
        );
        assert!(
            session
                .maybe_start_deferred_worker(
                    false,
                    true,
                    true,
                    now + Duration::from_millis(200),
                    delay,
                    || true,
                )
                .is_none()
        );
    }

    #[test]
    fn deferred_catalog_worker_waits_until_background_work_is_allowed() {
        let mut session = LauncherCatalogSession::new(false);
        let now = Instant::now();
        let delay = Duration::from_millis(50);

        session.defer_catalog_worker(
            "/media/fat/_Arcade".to_string(),
            CatalogWorkerRequest::CheckStamp,
            CatalogWorkerInitialCache::ProbeSqlite,
            CatalogExecutionMode::BackgroundInteractive,
        );

        assert!(
            session
                .maybe_start_deferred_worker(
                    false,
                    true,
                    false,
                    now + Duration::from_millis(70),
                    delay,
                    || true,
                )
                .is_none()
        );
        assert!(
            session
                .maybe_start_deferred_worker(
                    false,
                    true,
                    true,
                    now + Duration::from_millis(80),
                    delay,
                    || true,
                )
                .is_none()
        );

        let worker = session
            .maybe_start_deferred_worker(
                false,
                true,
                true,
                now + Duration::from_millis(140),
                delay,
                || true,
            )
            .expect("deferred worker");

        assert_eq!(worker.root, "/media/fat/_Arcade");
        assert_eq!(worker.request, CatalogWorkerRequest::CheckStamp);
        assert_eq!(worker.initial_cache, CatalogWorkerInitialCache::ProbeSqlite);
        assert!(
            session
                .maybe_start_deferred_worker(
                    false,
                    true,
                    true,
                    now + Duration::from_millis(200),
                    delay,
                    || true,
                )
                .is_none()
        );
    }

    #[test]
    fn deferred_stamp_check_waits_while_standalone_builder_holds_lock() {
        let mut session = LauncherCatalogSession::new(false);
        let now = Instant::now();
        let delay = Duration::from_millis(50);
        session.defer_catalog_worker(
            "/media/fat/_Arcade".to_string(),
            CatalogWorkerRequest::CheckStamp,
            CatalogWorkerInitialCache::AlreadyLoadedReady,
            CatalogExecutionMode::BackgroundInteractive,
        );

        assert!(
            session
                .maybe_start_deferred_worker(false, true, true, now, delay, || {
                    panic!("lock must not be probed before validation delay")
                })
                .is_none()
        );
        assert!(
            session
                .maybe_start_deferred_worker(
                    false,
                    true,
                    true,
                    now + Duration::from_millis(60),
                    delay,
                    || false,
                )
                .is_none()
        );
        assert!(
            session
                .maybe_start_deferred_worker(
                    false,
                    true,
                    true,
                    now + Duration::from_millis(900),
                    delay,
                    || panic!("lock must not be reprobed before retry deadline"),
                )
                .is_none()
        );
        let worker = session
            .maybe_start_deferred_worker(
                false,
                true,
                true,
                now + Duration::from_millis(1100),
                delay,
                || true,
            )
            .expect("deferred check after builder exits");
        assert_eq!(worker.request, CatalogWorkerRequest::CheckStamp);
    }

    #[test]
    pub(super) fn games_found_counter_reports_real_worker_counts() {
        let mut counter = GamesFoundCounter::default();

        assert_eq!(
            counter.progress_detail("Classifying library", "Games found: 250"),
            Some("Games found: 250".to_string())
        );
        assert_eq!(
            counter.progress_detail("Classifying library", "Games found: 500"),
            Some("Games found: 500".to_string())
        );
    }

    #[test]
    pub(super) fn counter_climb_metric_waits_for_meaningful_target() {
        assert!(!counter_climb_target_is_meaningful(1));
        assert!(!counter_climb_target_is_meaningful(49));
        assert!(counter_climb_target_is_meaningful(50));
        assert!(counter_climb_target_is_meaningful(250));
        assert!(!counter_climb_target_is_sustained(499));
        assert!(counter_climb_target_is_sustained(500));
    }

    #[test]
    pub(super) fn games_found_counter_accepts_bootstrap_title() {
        let mut counter = GamesFoundCounter::default();

        assert_eq!(
            counter.progress_detail("Finding games", "Games found: 50"),
            Some("Games found: 50".to_string())
        );
    }

    #[test]
    pub(super) fn games_found_counter_does_not_drop_when_full_scan_starts_lower() {
        let mut counter = GamesFoundCounter::default();

        counter.progress_detail("Finding games", "Games found: 911");
        assert_eq!(
            counter.progress_detail("Classifying library", "Games found: 50"),
            Some("Games found: 911".to_string())
        );
        assert_eq!(counter.displayed, 911);

        assert_eq!(
            counter.progress_detail("Classifying library", "Games found: 1200"),
            Some("Games found: 1200".to_string())
        );
        assert_eq!(counter.displayed, 1200);
    }

    #[test]
    pub(super) fn full_scan_counter_takeover_requires_visible_overtake() {
        assert!(!counter_climb_target_overtakes_visible(50, 650));
        assert!(!counter_climb_target_overtakes_visible(650, 650));
        assert!(counter_climb_target_overtakes_visible(700, 650));
    }

    #[test]
    pub(super) fn games_found_counter_ignores_other_scan_phases() {
        let mut counter = GamesFoundCounter::default();

        counter.progress_detail("Classifying library", "Games found: 100");
        assert_eq!(
            counter.progress_detail("Saving library", "Writing 0 of 100 games into SQLite..."),
            None
        );
        assert_eq!(counter.displayed, 0);
    }

    #[test]
    pub(super) fn duplicate_cached_catalog_ready_is_skipped_after_sync_load() {
        assert!(duplicate_cached_catalog_ready(true, true));
        assert!(!duplicate_cached_catalog_ready(false, true));
        assert!(!duplicate_cached_catalog_ready(true, false));
    }

    #[test]
    fn partial_catalog_load_failure_is_not_offered_as_stale_catalog() {
        let mut session = LauncherCatalogSession::new(true);
        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: true,
                screen: Screen::Arcade,
                media_gate: None,
            },
            CatalogWorkerMessage::LoadFailed {
                error: "disconnected".to_string(),
            },
            Instant::now(),
        );

        let mut discarded = false;
        let mut stale = None;
        for effect in effects.into_effects() {
            match effect {
                CatalogSessionEffect::DiscardPartialCatalog => discarded = true,
                CatalogSessionEffect::Lifecycle(
                    LauncherLifecycleInput::CatalogRecoveryRequired {
                        has_stale_catalog, ..
                    },
                ) => stale = Some(has_stale_catalog),
                _ => {}
            }
        }
        assert!(discarded);
        assert_eq!(stale, Some(false));
    }

    #[test]
    fn older_shard_schema_is_classified_as_a_format_upgrade() {
        assert!(catalog_failure_is_format_upgrade(
            "unsupported shard schema version for snes generation 1: expected 3, found 2"
        ));
        assert!(!catalog_failure_is_format_upgrade(
            "unsupported shard schema version for snes generation 1: expected 3, found 4"
        ));
        assert!(!catalog_failure_is_format_upgrade("disk full"));
    }
}
