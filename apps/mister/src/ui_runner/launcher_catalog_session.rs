// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_worker_intents::{
    LauncherWorkerUiIntent, cached_catalog_validation_intent, catalog_build_status_intent,
    catalog_rebuild_started_intent, catalog_system_discovering_intent,
    catalog_system_update_preparing_intent, catalog_system_update_progress_intent,
};
use super::*;
use crate::preview_state::SystemEntryPreviewPrelude;
use std::collections::BTreeSet;

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
    CatalogPlanReady {
        system_ids: Vec<String>,
        all_published_systems: bool,
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
    CatalogValidationFinished,
    RequestMediaCatalogSeed,
    ApplySystemShard {
        system_id: String,
        catalog: ArcadeCatalog,
        base_catalog_version: usize,
        game_count: usize,
        prepare_us: u64,
        profile: SystemEntryCatalogProfile,
        preview_prelude: Option<SystemEntryPreviewPrelude>,
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
}

pub(super) struct LauncherCatalogSession {
    foreground_update: bool,
    refresh_done: bool,
    refresh_failed: bool,
    summary_only: bool,
    deferred_worker: Option<DeferredCatalogWorker>,
    system_update_total: Option<usize>,
    completed_system_updates: BTreeSet<String>,
    displayed_system_updates: usize,
    catalog_seed_partial: bool,
}

impl LauncherCatalogSession {
    pub(super) fn new(foreground_update: bool) -> Self {
        Self {
            foreground_update,
            refresh_done: false,
            refresh_failed: false,
            summary_only: false,
            deferred_worker: None,
            system_update_total: None,
            completed_system_updates: BTreeSet::new(),
            displayed_system_updates: 0,
            catalog_seed_partial: false,
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
        self.catalog_seed_partial = true;
    }

    pub(super) fn note_cached_catalog_ready(&mut self) {
        self.summary_only = false;
        self.catalog_seed_partial = false;
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
        _now: Instant,
    ) -> CatalogSessionEffects {
        let mut effects = CatalogSessionEffects::default();
        match message {
            CatalogWorkerMessage::Progress { phase, work_units } => {
                if phase == "systems" {
                    let completed = work_units
                        .saturating_sub(1)
                        .try_into()
                        .unwrap_or(usize::MAX);
                    self.note_system_update_progress(completed, &mut effects);
                }
            }
            CatalogWorkerMessage::Heartbeat { .. } => {}
            CatalogWorkerMessage::Timing { name, detail } => {
                effects.event(name, detail);
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
            CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids,
                all_published_systems,
            } => {
                self.system_update_total = Some(system_ids.len());
                self.completed_system_updates.clear();
                self.displayed_system_updates = 0;
                effects.ui(catalog_system_update_progress_intent(0, system_ids.len()));
                effects.push(CatalogSessionEffect::CatalogPlanReady {
                    system_ids,
                    all_published_systems,
                });
            }
            CatalogWorkerMessage::SystemDiscovering { title } => {
                effects.ui(catalog_system_discovering_intent(&title));
            }
            CatalogWorkerMessage::BuildStatus { title } => {
                effects.ui(catalog_build_status_intent(title));
            }
            CatalogWorkerMessage::SystemScanning { system_id } => {
                effects.push(CatalogSessionEffect::CatalogSystemScanning { system_id });
            }
            CatalogWorkerMessage::SystemPrepared {
                system_id,
                generation,
            } => {
                self.note_system_update_terminal(&system_id, &mut effects);
                effects.push(CatalogSessionEffect::CatalogSystemPrepared {
                    system_id,
                    generation,
                });
            }
            CatalogWorkerMessage::SystemRemoved { system_id } => {
                self.note_system_update_terminal(&system_id, &mut effects);
            }
            CatalogWorkerMessage::SystemUpdateFailed { system_id, error } => {
                self.note_system_update_terminal(&system_id, &mut effects);
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
            CatalogWorkerMessage::BuildCompleted { elapsed_us } => {
                effects.ui(LauncherWorkerUiIntent::InfoDatabaseBuild(
                    mister_magik_catalog::fast_catalog_refresh::format_build_elapsed(elapsed_us),
                ));
            }
            CatalogWorkerMessage::SystemShardReady {
                system_id,
                catalog,
                base_catalog_version,
                game_count,
                prepare_us,
                profile,
                preview_prelude,
            } => effects.push(CatalogSessionEffect::ApplySystemShard {
                system_id,
                catalog,
                base_catalog_version,
                game_count,
                prepare_us,
                profile,
                preview_prelude,
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
                load_us,
                source,
                durable_save_pending,
                generation_fingerprint,
                publication_ack,
            } => {
                self.handle_ready(
                    context.catalog_ready,
                    context.catalog_partial || self.catalog_seed_partial,
                    catalog,
                    load_us,
                    source,
                    durable_save_pending,
                    generation_fingerprint,
                    publication_ack,
                    &mut effects,
                );
            }
            CatalogWorkerMessage::ArcadeBootstrapReady { .. }
            | CatalogWorkerMessage::PublishedRegistrySeed { .. } => {
                unreachable!("internal catalog publication crossed the child protocol")
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
        self.system_update_total = None;
        self.completed_system_updates.clear();
        self.displayed_system_updates = 0;
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
        self.system_update_total = None;
        self.completed_system_updates.clear();
        self.displayed_system_updates = 0;
        effects.push(CatalogSessionEffect::StartCatalogWorker(
            CatalogWorkerStart {
                root,
                request: CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                initial_cache: CatalogWorkerInitialCache::AlreadyLoadedReady,
                execution_mode: CatalogExecutionMode::BackgroundInteractive,
            },
        ));
        effects.ui(catalog_system_update_preparing_intent());
        effects
    }

    pub(super) fn rebuild_database(
        &mut self,
        root: String,
        worker_available: bool,
    ) -> CatalogSessionEffects {
        let mut effects = CatalogSessionEffects::default();
        if !worker_available {
            effects.event(
                "database_rebuild_rejected",
                "source=settings reason=catalog-worker-busy",
            );
            effects.push(CatalogSessionEffect::Confirm(
                launcher::ConfirmAction::DatabaseRebuildUnavailable,
            ));
            return effects;
        }
        effects.event(
            "database_rebuild_requested",
            "source=settings scope=all-systems",
        );
        self.refresh_done = false;
        self.foreground_update = false;
        self.deferred_worker = None;
        self.refresh_failed = false;
        self.system_update_total = None;
        self.completed_system_updates.clear();
        self.displayed_system_updates = 0;
        effects.push(CatalogSessionEffect::CatalogPlanReady {
            system_ids: Vec::new(),
            all_published_systems: true,
        });
        effects.ui(catalog_system_update_preparing_intent());
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
        self.system_update_total = None;
        self.completed_system_updates.clear();
        self.displayed_system_updates = 0;
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

    fn note_system_update_terminal(
        &mut self,
        system_id: &str,
        effects: &mut CatalogSessionEffects,
    ) {
        if self.system_update_total.is_none() {
            return;
        }
        if self.completed_system_updates.insert(system_id.to_string()) {
            self.note_system_update_progress(self.completed_system_updates.len(), effects);
        }
    }

    fn note_system_update_progress(
        &mut self,
        completed: usize,
        effects: &mut CatalogSessionEffects,
    ) {
        let Some(total) = self.system_update_total else {
            return;
        };
        let completed = completed.min(total);
        if completed > self.displayed_system_updates {
            self.displayed_system_updates = completed;
            effects.ui(catalog_system_update_progress_intent(completed, total));
        }
    }

    fn handle_ready(
        &mut self,
        catalog_ready: bool,
        catalog_partial: bool,
        ready_catalog: ArcadeCatalog,
        load_us: u64,
        source: CatalogSource,
        durable_save_pending: bool,
        generation_fingerprint: Option<String>,
        publication_ack: Option<mpsc::Sender<()>>,
        effects: &mut CatalogSessionEffects,
    ) {
        let cached_before_refresh = !durable_save_pending;
        let duplicate_cached_catalog = !self.summary_only
            && duplicate_cached_catalog_ready(
                catalog_ready,
                catalog_partial,
                cached_before_refresh,
            );
        let validation_already_finished = self.refresh_done;
        self.refresh_done =
            validation_already_finished || (!cached_before_refresh && !durable_save_pending);
        let catalog_len = ready_catalog.len();
        if !duplicate_cached_catalog {
            self.summary_only = false;
            self.catalog_seed_partial = matches!(
                source,
                CatalogSource::ReturnCapsule
                    | CatalogSource::SummaryProjection
                    | CatalogSource::NavigationProjection
            );
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
        if duplicate_cached_catalog {
            if let Some(publication_ack) = publication_ack {
                let _ = publication_ack.send(());
            }
            if self.refresh_failed || self.foreground_update {
                self.refresh_done = true;
                self.foreground_update = false;
                effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
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
}

fn catalog_failure_is_transient(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    if error.contains(mister_magik_catalog::catalog_progress::CATALOG_SAFETY_LIMIT_NONRETRYABLE) {
        return false;
    }
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

fn duplicate_cached_catalog_ready(
    catalog_ready: bool,
    catalog_partial: bool,
    cached_before_refresh: bool,
) -> bool {
    catalog_ready && !catalog_partial && cached_before_refresh
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
                    CatalogSessionEffect::CatalogPlanReady { .. }
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
                CatalogSessionEffect::DiscardPartialCatalog => "discard-partial",
                CatalogSessionEffect::ApplySearchResult { .. } => "search-result",
                CatalogSessionEffect::FailSearchRequest { .. } => "search-failed",
                CatalogSessionEffect::SyncCatalogBridge => "sync",
                CatalogSessionEffect::CatalogPlanReady { .. }
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
                CatalogSessionEffect::CatalogValidationFinished => "catalog-validation-finished",
                CatalogSessionEffect::RequestMediaCatalogSeed => "request-media-seed",
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
                CatalogSessionEffect::CatalogPlanReady { .. }
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
                CatalogSessionEffect::DiscardPartialCatalog => effect_names.push("discard-partial"),
                CatalogSessionEffect::ApplySearchResult { .. } => {
                    effect_names.push("search-result")
                }
                CatalogSessionEffect::FailSearchRequest { .. } => {
                    effect_names.push("search-failed")
                }
                CatalogSessionEffect::SyncCatalogBridge => effect_names.push("sync"),
                CatalogSessionEffect::CatalogPlanReady { .. }
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
                CatalogSessionEffect::CatalogValidationFinished => {
                    effect_names.push("catalog-validation-finished")
                }
                CatalogSessionEffect::RequestMediaCatalogSeed => {
                    effect_names.push("request-media-seed")
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

    #[test]
    fn completed_catalog_build_displays_elapsed_time() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(true);
        let values = database_build_values(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
            },
            CatalogWorkerMessage::BuildCompleted {
                elapsed_us: 119_000_000,
            },
            now,
        ));

        assert_eq!(values, vec!["119 seconds"]);
    }

    #[test]
    fn rebuild_progress_counts_unique_terminal_system_events() {
        let now = Instant::now();
        let context = || CatalogWorkerMessageContext {
            catalog_ready: true,
            catalog_partial: false,
        };
        let mut session = LauncherCatalogSession::new(false);

        let planned = catalog_scan_statuses(session.handle_worker_message(
            context(),
            CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids: vec!["neogeo".into(), "snes".into(), "zx-spectrum".into()],
                all_published_systems: false,
            },
            now,
        ));
        assert_eq!(planned[0].title(), "Updating systems 0/3");

        let live_first = catalog_scan_statuses(session.handle_worker_message(
            context(),
            CatalogWorkerMessage::Progress {
                phase: "systems".into(),
                work_units: 2,
            },
            now,
        ));
        assert_eq!(live_first[0].title(), "Updating systems 1/3");

        let live_second = catalog_scan_statuses(session.handle_worker_message(
            context(),
            CatalogWorkerMessage::Progress {
                phase: "systems".into(),
                work_units: 3,
            },
            now,
        ));
        assert_eq!(live_second[0].title(), "Updating systems 2/3");

        let live_complete = catalog_scan_statuses(session.handle_worker_message(
            context(),
            CatalogWorkerMessage::Progress {
                phase: "systems".into(),
                work_units: 4,
            },
            now,
        ));
        assert_eq!(live_complete[0].title(), "Updating systems 3/3");

        let prepared = catalog_scan_statuses(session.handle_worker_message(
            context(),
            CatalogWorkerMessage::SystemPrepared {
                system_id: "snes".into(),
                generation: 1,
            },
            now,
        ));
        assert!(prepared.is_empty());

        let duplicate = catalog_scan_statuses(session.handle_worker_message(
            context(),
            CatalogWorkerMessage::SystemPrepared {
                system_id: "snes".into(),
                generation: 1,
            },
            now,
        ));
        assert!(duplicate.is_empty());

        let failed = catalog_scan_statuses(session.handle_worker_message(
            context(),
            CatalogWorkerMessage::SystemUpdateFailed {
                system_id: "neogeo".into(),
                error: "bad archive".into(),
            },
            now,
        ));
        assert!(failed.is_empty());

        let done = session.handle_worker_message(context(), CatalogWorkerMessage::Done, now);
        assert!(done.into_effects().into_iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::Ui(LauncherWorkerUiIntent::ClearCatalogScan)
        )));
    }

    #[test]
    fn rebuild_progress_terminal_fallback_counts_removed_and_failed_systems_once() {
        let now = Instant::now();
        let context = || CatalogWorkerMessageContext {
            catalog_ready: true,
            catalog_partial: false,
        };
        let mut session = LauncherCatalogSession::new(false);

        let planned = catalog_scan_statuses(session.handle_worker_message(
            context(),
            CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids: vec!["neogeo".into(), "snes".into(), "zx-spectrum".into()],
                all_published_systems: false,
            },
            now,
        ));
        assert_eq!(planned[0].title(), "Updating systems 0/3");

        let removed = catalog_scan_statuses(session.handle_worker_message(
            context(),
            CatalogWorkerMessage::SystemRemoved {
                system_id: "neogeo".into(),
            },
            now,
        ));
        assert_eq!(removed[0].title(), "Updating systems 1/3");

        let failed = catalog_scan_statuses(session.handle_worker_message(
            context(),
            CatalogWorkerMessage::SystemUpdateFailed {
                system_id: "snes".into(),
                error: "bad archive".into(),
            },
            now,
        ));
        assert_eq!(failed[0].title(), "Updating systems 2/3");

        let prepared = catalog_scan_statuses(session.handle_worker_message(
            context(),
            CatalogWorkerMessage::SystemPrepared {
                system_id: "zx-spectrum".into(),
                generation: 1,
            },
            now,
        ));
        assert_eq!(prepared[0].title(), "Updating systems 3/3");

        assert!(
            catalog_scan_statuses(session.handle_worker_message(
                context(),
                CatalogWorkerMessage::SystemRemoved {
                    system_id: "neogeo".into(),
                },
                now,
            ))
            .is_empty()
        );
        assert!(
            catalog_scan_statuses(session.handle_worker_message(
                context(),
                CatalogWorkerMessage::SystemUpdateFailed {
                    system_id: "snes".into(),
                    error: "bad archive".into(),
                },
                now,
            ))
            .is_empty()
        );
    }

    #[test]
    fn system_discovery_updates_the_background_status() {
        let mut session = LauncherCatalogSession::new(false);
        let statuses = catalog_scan_statuses(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
            },
            CatalogWorkerMessage::SystemDiscovering {
                title: "Super Nintendo".to_string(),
            },
            Instant::now(),
        ));

        assert_eq!(statuses[0].title(), "Discovering Super Nintendo");
        assert!(statuses[0].background_visible());
    }

    #[test]
    fn catalog_build_status_updates_the_background_status() {
        let mut session = LauncherCatalogSession::new(false);
        let context = || CatalogWorkerMessageContext {
            catalog_ready: true,
            catalog_partial: false,
        };

        for title in [
            "Saving system 1/90",
            "Saving system 90/90",
            "Saving catalog metadata…",
            "Finishing catalog…",
        ] {
            let statuses = catalog_scan_statuses(session.handle_worker_message(
                context(),
                CatalogWorkerMessage::BuildStatus {
                    title: title.to_string(),
                },
                Instant::now(),
            ));

            assert_eq!(statuses[0].title(), title);
            assert!(statuses[0].background_visible());
        }
    }

    #[test]
    fn update_and_hydration_failures_emit_distinct_state_effects() {
        let now = Instant::now();
        let context = || CatalogWorkerMessageContext {
            catalog_ready: true,
            catalog_partial: false,
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
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
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
    fn persistence_failure_replaces_finalizing_progress_with_error_state() {
        let mut session = LauncherCatalogSession::new(true);
        let (effects, ui_effects) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
                catalog_partial: false,
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
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(4),
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
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(4),
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
        assert!(effects.effects.iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::Ui(LauncherWorkerUiIntent::CatalogScan(status))
                if status.title() == "Discovering systems"
        )));
        assert_eq!(effect_names(effects), vec!["event", "start-worker", "ui"]);
    }

    #[test]
    fn settings_database_rebuild_marks_all_systems_and_stays_background() {
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.rebuild_database("/media/fat/_Arcade".to_string(), true);

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
        assert!(effects.effects.iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::Ui(LauncherWorkerUiIntent::CatalogScan(status))
                if status.title() == "Discovering systems"
        )));
        assert_eq!(effect_names(effects), vec!["event", "ui", "start-worker"]);
    }

    #[test]
    fn settings_database_rebuild_busy_only_shows_acknowledgement_dialog() {
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.rebuild_database("/media/fat/_Arcade".to_string(), false);
        let mut effects = effects.into_effects().into_iter();
        assert!(matches!(
            effects.next(),
            Some(CatalogSessionEffect::StartupEvent(CatalogSessionEvent { name, detail }))
                if name == "database_rebuild_rejected"
                    && detail == "source=settings reason=catalog-worker-busy"
        ));
        assert!(matches!(
            effects.next(),
            Some(CatalogSessionEffect::Confirm(
                launcher::ConfirmAction::DatabaseRebuildUnavailable
            ))
        ));
        assert!(effects.next().is_none());
        assert!(!session.refresh_done());
        assert!(!session.foreground_update());
        assert!(session.deferred_worker.is_none());
        assert!(session.system_update_total.is_none());
        assert!(session.completed_system_updates.is_empty());
        assert_eq!(session.displayed_system_updates, 0);
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
            CatalogWorkerInitialCache::AlreadyProbedMissing,
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
        assert_eq!(
            worker.initial_cache,
            CatalogWorkerInitialCache::AlreadyProbedMissing
        );
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
            CatalogWorkerInitialCache::AlreadyProbedMissing,
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
        assert_eq!(
            worker.initial_cache,
            CatalogWorkerInitialCache::AlreadyProbedMissing
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
    pub(super) fn duplicate_cached_catalog_ready_is_skipped_after_sync_load() {
        assert!(duplicate_cached_catalog_ready(true, false, true));
        assert!(!duplicate_cached_catalog_ready(false, false, true));
        assert!(!duplicate_cached_catalog_ready(true, true, true));
        assert!(!duplicate_cached_catalog_ready(true, false, false));
    }

    #[test]
    fn authoritative_registry_replaces_arcade_bootstrap_before_duplicate_suppression() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(false);
        let context = |catalog_ready| CatalogWorkerMessageContext {
            catalog_ready,
            catalog_partial: false,
        };

        let bootstrap = session.handle_worker_message(
            context(false),
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(1),
                load_us: 1,
                source: CatalogSource::NavigationProjection,
                durable_save_pending: true,
                generation_fingerprint: None,
                publication_ack: None,
            },
            now,
        );
        assert!(bootstrap.into_effects().into_iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::UseCatalog {
                source: CatalogSource::NavigationProjection,
                ..
            }
        )));

        let registry = session.handle_worker_message(
            context(true),
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                load_us: 2,
                source: CatalogSource::ShardedRegistry,
                durable_save_pending: false,
                generation_fingerprint: Some("generation-1".to_string()),
                publication_ack: None,
            },
            now,
        );
        assert!(registry.into_effects().into_iter().any(|effect| matches!(
            effect,
            CatalogSessionEffect::UseCatalog {
                source: CatalogSource::ShardedRegistry,
                ..
            }
        )));

        let duplicate = session.handle_worker_message(
            context(true),
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                load_us: 3,
                source: CatalogSource::ShardedRegistry,
                durable_save_pending: false,
                generation_fingerprint: Some("generation-1".to_string()),
                publication_ack: None,
            },
            now,
        );
        assert!(
            !duplicate
                .into_effects()
                .into_iter()
                .any(|effect| matches!(effect, CatalogSessionEffect::UseCatalog { .. }))
        );
    }

    #[test]
    fn partial_catalog_load_failure_is_not_offered_as_stale_catalog() {
        let mut session = LauncherCatalogSession::new(true);
        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: true,
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

    #[test]
    fn safety_limit_failures_are_not_transient_or_auto_retried() {
        let error = format!(
            "{} kind=entries observed=4000001 configured=4000000 path=/media/fat/_Arcade",
            mister_magik_catalog::catalog_progress::CATALOG_SAFETY_LIMIT_NONRETRYABLE
        );
        assert!(!catalog_failure_is_transient(&error));
        assert!(catalog_failure_is_transient("disk full"));
    }
}
