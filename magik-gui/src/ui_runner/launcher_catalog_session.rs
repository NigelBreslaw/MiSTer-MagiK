use super::launcher_worker_intents::{
    cached_catalog_validation_intent, catalog_persistence_failed_intent,
    catalog_rebuild_started_intent, parse_games_found_detail, CatalogCounterPhase,
    CatalogProgressUiIntent, CatalogWorkerUiContext, LauncherWorkerUiIntent,
};
use super::*;

pub(super) struct CatalogWorkerStart {
    pub(super) root: String,
    pub(super) request: CatalogWorkerRequest,
    pub(super) initial_cache: CatalogWorkerInitialCache,
}

struct DeferredCatalogWorker {
    root: String,
    request: CatalogWorkerRequest,
    initial_cache: CatalogWorkerInitialCache,
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
    },
    SyncCatalogBridge,
    Ui(LauncherWorkerUiIntent),
    FinishMediaWorker,
    FinishMediaWorkerIfNoCatalogSeedPending,
    CatalogValidationFinished,
    RequestMediaCatalogSeed,
    MediaSystemDiscovered {
        system_id: String,
        media_gate: Option<MediaInteractionGate>,
    },
    RequestLibraryRebuildOnNextBoot,
    Confirm(launcher::ConfirmAction),
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
    ) {
        self.deferred_worker = Some(DeferredCatalogWorker {
            root,
            request,
            initial_cache,
            start_after: None,
        });
    }

    pub(super) fn maybe_start_deferred_worker(
        &mut self,
        worker_running: bool,
        first_visible_copy_done: bool,
        loop_start: Instant,
        delay: Duration,
    ) -> Option<CatalogWorkerStart> {
        if self.refresh_done || worker_running {
            return None;
        }
        let deferred = self.deferred_worker.as_mut()?;
        if !first_visible_copy_done {
            return None;
        }
        let start_after = *deferred
            .start_after
            .get_or_insert_with(|| loop_start + delay);
        if loop_start < start_after {
            return None;
        }
        let deferred = self.deferred_worker.take()?;
        Some(CatalogWorkerStart {
            root: deferred.root,
            request: deferred.request,
            initial_cache: deferred.initial_cache,
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
            } => {
                self.handle_progress(context, title, detail, percent, now, &mut effects);
            }
            CatalogWorkerMessage::SystemDiscovered { system_id } => {
                effects.push(CatalogSessionEffect::MediaSystemDiscovered {
                    system_id,
                    media_gate: context.media_gate,
                });
            }
            CatalogWorkerMessage::Ready {
                catalog,
                summary,
                load_us,
                source,
                durable_save_pending,
            } => {
                self.handle_ready(
                    context.catalog_ready,
                    catalog,
                    summary,
                    load_us,
                    source,
                    durable_save_pending,
                    &mut effects,
                );
            }
            CatalogWorkerMessage::Persisted { summary } => {
                self.persisted_summary_seen = true;
                self.refresh_done = true;
                self.foreground_update = false;
                self.refresh_failed = false;
                effects.push(CatalogSessionEffect::FinishMediaWorkerIfNoCatalogSeedPending);
                effects.push(CatalogSessionEffect::CatalogValidationFinished);
                effects.event("library_db_saved", format_library_refresh_summary(&summary));
                push_catalog_coverage_diagnostic(&summary, &mut effects);
                effects.ui(LauncherWorkerUiIntent::HideCatalogBackgroundScan);
            }
            CatalogWorkerMessage::PersistenceFailed { error } => {
                self.refresh_done = true;
                self.foreground_update = false;
                self.refresh_failed = true;
                effects.push(CatalogSessionEffect::FinishMediaWorker);
                effects.push(CatalogSessionEffect::CatalogValidationFinished);
                effects.event("library_db_save_failed", error.clone());
                effects.ui(catalog_persistence_failed_intent(error));
                self.games_found_counter.reset();
            }
            CatalogWorkerMessage::Unchanged { summary } => {
                self.refresh_done = true;
                self.foreground_update = false;
                self.refresh_failed = false;
                effects.push(CatalogSessionEffect::FinishMediaWorker);
                effects.push(CatalogSessionEffect::CatalogValidationFinished);
                effects.event(
                    "library_db_unchanged",
                    format_library_refresh_summary(&summary),
                );
                effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
                self.games_found_counter.reset();
            }
            CatalogWorkerMessage::Changed { detail } => {
                self.refresh_done = true;
                self.foreground_update = false;
                self.refresh_failed = false;
                effects.push(CatalogSessionEffect::FinishMediaWorker);
                effects.push(CatalogSessionEffect::CatalogValidationFinished);
                effects.event("library_changed_detected", detail);
                effects.push(CatalogSessionEffect::Confirm(
                    launcher::ConfirmAction::LibraryChanged,
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
        self.foreground_update = true;
        self.deferred_worker = None;
        self.refresh_failed = false;
        self.reset_counter_metrics();
        self.games_found_counter.reset();
        effects.push(CatalogSessionEffect::StartCatalogWorker(
            CatalogWorkerStart {
                root,
                request: CatalogWorkerRequest::ForceBuild,
                initial_cache: CatalogWorkerInitialCache::AlreadyLoadedReady,
            },
        ));
        effects.ui(catalog_rebuild_started_intent(self.foreground_update));
        effects
    }

    pub(super) fn tick_games_found_counter(&mut self, now: Instant) -> Option<String> {
        self.games_found_counter.tick(now)
    }

    fn handle_progress(
        &mut self,
        context: CatalogWorkerMessageContext,
        title: String,
        detail: String,
        percent: i32,
        now: Instant,
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
            .progress_detail(&intent.title, &intent.detail, now);
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
            if self.refresh_failed || self.foreground_update {
                self.refresh_done = true;
                self.foreground_update = false;
                effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
                self.games_found_counter.reset();
                effects.push(CatalogSessionEffect::Confirm(
                    launcher::ConfirmAction::LibraryUpdateFailed,
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
            effects.ui(LauncherWorkerUiIntent::ClearCatalogScan);
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
    target: usize,
    active: bool,
    last_tick: Option<Instant>,
    phase: Option<CatalogCounterPhase>,
}

impl GamesFoundCounter {
    fn progress_detail(&mut self, title: &str, detail: &str, now: Instant) -> Option<String> {
        let phase = CatalogCounterPhase::for_title(title);
        let target = phase.and_then(|_| parse_games_found_detail(detail));
        let Some(target) = target else {
            self.reset();
            return None;
        };
        let phase = phase.expect("phase exists when target parses");
        if phase == CatalogCounterPhase::FullScan && target <= self.displayed {
            return Some(format_games_found(self.displayed));
        }
        if !self.active || target < self.displayed {
            self.displayed = self.displayed.min(target);
            self.last_tick = Some(now);
        }
        self.target = target;
        self.active = true;
        self.phase = Some(phase);
        Some(format_games_found(self.displayed))
    }

    fn tick(&mut self, now: Instant) -> Option<String> {
        if !self.active || self.displayed >= self.target {
            self.last_tick = Some(now);
            return None;
        }
        let elapsed = self
            .last_tick
            .map(|last| now.duration_since(last))
            .unwrap_or(Duration::from_millis(66));
        let step = games_found_count_step(
            self.displayed,
            self.target,
            elapsed,
            self.phase.unwrap_or(CatalogCounterPhase::FullScan),
        );
        if step == 0 {
            return None;
        }
        self.displayed = self.displayed.saturating_add(step).min(self.target);
        self.last_tick = Some(now);
        Some(format_games_found(self.displayed))
    }

    fn reset(&mut self) {
        self.displayed = 0;
        self.target = 0;
        self.active = false;
        self.last_tick = None;
        self.phase = None;
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

fn games_found_count_step(
    displayed: usize,
    target: usize,
    elapsed: Duration,
    phase: CatalogCounterPhase,
) -> usize {
    if target <= displayed {
        return 0;
    }
    let lag = target - displayed;
    let elapsed_ms = elapsed.as_millis().max(1) as usize;
    if phase == CatalogCounterPhase::Bootstrap {
        let bootstrap_games_per_second = 55usize;
        return ((bootstrap_games_per_second * elapsed_ms).div_ceil(1000)).clamp(1, lag);
    }
    let catchup_ms = 450usize;
    ((lag * elapsed_ms).div_ceil(catchup_ms)).clamp(1, lag)
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
            .map(|effect| match effect {
                CatalogSessionEffect::StartupEvent(_) => "event",
                CatalogSessionEffect::UseCatalog { .. } => "catalog",
                CatalogSessionEffect::SyncCatalogBridge => "sync",
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
            match effect {
                CatalogSessionEffect::StartupEvent(_) => effect_names.push("event"),
                CatalogSessionEffect::UseCatalog { .. } => effect_names.push("catalog"),
                CatalogSessionEffect::SyncCatalogBridge => effect_names.push("sync"),
                CatalogSessionEffect::Ui(intent) => {
                    effect_names.push("ui");
                    ui_names.push(match intent {
                        LauncherWorkerUiIntent::CatalogScan(_) => "catalog-scan",
                        LauncherWorkerUiIntent::ClearCatalogScan => "clear-catalog-scan",
                        LauncherWorkerUiIntent::HideCatalogBackgroundScan => "hide-background-scan",
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
                CatalogSessionEffect::StartCatalogWorker(_) => effect_names.push("start-worker"),
            }
        }
        (effect_names, ui_names)
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
    fn ready_catalog_replaces_cache_and_requests_media_seed() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                summary: None,
                load_us: 42,
                source: CatalogSource::FullSqlite,
                durable_save_pending: false,
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
    fn early_ready_keeps_refresh_open_until_persisted() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(false);
        let (ready_effects, ready_ui) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                summary: None,
                load_us: 42,
                source: CatalogSource::FreshBuild,
                durable_save_pending: true,
            },
            now,
        ));

        assert_eq!(
            ready_effects,
            vec!["request-media-seed", "catalog", "event", "ui", "sync"]
        );
        assert_eq!(ready_ui, vec!["clear-catalog-scan"]);
        assert!(!session.refresh_done());

        let (persisted_effects, persisted_ui) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Persisted {
                summary: refresh_summary(),
            },
            now,
        ));

        assert_eq!(
            persisted_effects,
            vec![
                "finish-media-if-no-seed",
                "catalog-validation-finished",
                "event",
                "ui"
            ]
        );
        assert_eq!(persisted_ui, vec!["hide-background-scan"]);
        assert!(session.refresh_done());
        assert!(!session.foreground_update());
        assert!(!session.refresh_failed);
    }

    #[test]
    fn foreground_rebuild_ready_clears_blocking_scan_until_persisted() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(true);
        let (ready_effects, ready_ui) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(4),
                summary: None,
                load_us: 42,
                source: CatalogSource::FreshBuild,
                durable_save_pending: true,
            },
            now,
        ));

        assert_eq!(
            ready_effects,
            vec!["request-media-seed", "catalog", "event", "ui", "sync"]
        );
        assert_eq!(ready_ui, vec!["clear-catalog-scan"]);
        assert!(!session.refresh_done());
        assert!(!session.foreground_update());

        let (_, progress_ui) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Progress {
                title: "Saving library".to_string(),
                detail: "Writing catalog database before opening launcher...".to_string(),
                percent: -1,
            },
            now,
        ));

        assert_eq!(progress_ui, vec!["catalog-scan"]);
        assert!(!session.foreground_update());

        let (persisted_effects, persisted_ui) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Persisted {
                summary: refresh_summary(),
            },
            now,
        ));

        assert_eq!(
            persisted_effects,
            vec![
                "finish-media-if-no-seed",
                "catalog-validation-finished",
                "event",
                "ui"
            ]
        );
        assert_eq!(persisted_ui, vec!["hide-background-scan"]);
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
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Persisted { summary },
            now,
        ));

        assert_eq!(
            effects,
            vec![
                "finish-media-if-no-seed",
                "catalog-validation-finished",
                "event",
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
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                summary: Some(summary),
                load_us: 42,
                source: CatalogSource::FreshBuild,
                durable_save_pending: false,
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

        let _ = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                screen: Screen::Arcade,
                media_gate: None,
            },
            CatalogWorkerMessage::Unchanged {
                summary: refresh_summary(),
            },
            now,
        );
        assert!(session.refresh_done());

        let (effects, ui_effects) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                screen: Screen::Arcade,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                summary: None,
                load_us: 42,
                source: CatalogSource::FullSqlite,
                durable_save_pending: false,
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
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Progress {
                title: "Library scan failed".to_string(),
                detail: "disk unavailable".to_string(),
                percent: -1,
            },
            now,
        );

        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(3),
                summary: None,
                load_us: 42,
                source: CatalogSource::FullSqlite,
                durable_save_pending: false,
            },
            now,
        );

        assert_eq!(effect_names(effects), vec!["ui", "confirm", "event"]);
        assert!(session.refresh_done());
        assert!(!session.foreground_update());
    }

    #[test]
    fn persistence_failure_replaces_finalizing_progress_with_error_state() {
        let mut session = LauncherCatalogSession::new(true);
        let (effects, ui_effects) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
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
            vec!["finish-media", "catalog-validation-finished", "event", "ui"]
        );
        assert_eq!(ui_effects, vec!["catalog-scan"]);
        assert!(session.refresh_done());
        assert!(!session.foreground_update());
        assert!(session.refresh_failed);
    }

    #[test]
    fn persistence_failure_after_early_ready_keeps_session_catalog_available() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(true);
        let _ = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_with_games(4),
                summary: None,
                load_us: 42,
                source: CatalogSource::FreshBuild,
                durable_save_pending: true,
            },
            now,
        );
        assert!(!session.refresh_done());

        let (effects, ui_effects) = effect_and_ui_names(session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
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
            vec!["finish-media", "catalog-validation-finished", "event", "ui"]
        );
        assert_eq!(ui_effects, vec!["catalog-scan"]);
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
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Changed {
                detail: "Catalog stamp changed; rebuild required.".to_string(),
            },
            Instant::now(),
        );

        assert_eq!(
            effect_names(effects),
            vec![
                "finish-media",
                "catalog-validation-finished",
                "event",
                "confirm",
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
    fn rebuild_library_starts_foreground_force_build() {
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.rebuild_library("/media/fat/_Arcade".to_string());

        assert_eq!(effect_names(effects), vec!["event", "start-worker", "ui"]);
        assert!(!session.refresh_done());
        assert!(session.foreground_update());
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
        );

        assert!(session
            .maybe_start_deferred_worker(false, false, now + delay, delay)
            .is_none());
        assert!(session
            .maybe_start_deferred_worker(false, true, now + Duration::from_millis(20), delay)
            .is_none());

        let worker = session
            .maybe_start_deferred_worker(false, true, now + Duration::from_millis(70), delay)
            .expect("deferred worker");

        assert_eq!(worker.root, "/media/fat/_Arcade");
        assert_eq!(worker.request, CatalogWorkerRequest::CheckStamp);
        assert_eq!(worker.initial_cache, CatalogWorkerInitialCache::ProbeSqlite);
        assert!(session
            .maybe_start_deferred_worker(false, true, now + Duration::from_millis(200), delay)
            .is_none());
    }

    #[test]
    pub(super) fn games_found_counter_eases_toward_real_scan_count() {
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        assert_eq!(
            counter.progress_detail("Classifying library", "Games found: 250", now),
            Some("Games found: 0".to_string())
        );
        assert_eq!(
            counter.tick(now + Duration::from_millis(66)),
            Some("Games found: 37".to_string())
        );
        assert_eq!(
            counter.progress_detail(
                "Classifying library",
                "Games found: 500",
                now + Duration::from_millis(132)
            ),
            Some("Games found: 37".to_string())
        );
        let next = counter
            .tick(now + Duration::from_millis(198))
            .expect("counter should move after the target increases");
        let first_tick_count = parse_games_found_detail(&next).expect("parse counter detail");
        assert!(first_tick_count > 37);
        assert!(first_tick_count < 500);

        let next = counter
            .tick(now + Duration::from_millis(264))
            .expect("counter should keep moving");
        let count = parse_games_found_detail(&next).expect("parse counter detail");
        assert!(count > first_tick_count);
        assert!(count < 500);
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
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        assert_eq!(
            counter.progress_detail("Finding games", "Games found: 50", now),
            Some("Games found: 0".to_string())
        );
        assert_eq!(
            counter.tick(now + Duration::from_millis(66)),
            Some("Games found: 4".to_string())
        );
    }

    #[test]
    pub(super) fn games_found_counter_uses_real_bootstrap_target() {
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        assert_eq!(
            counter.progress_detail("Finding games", "Games found: 911", now),
            Some("Games found: 0".to_string())
        );
        assert_eq!(counter.target, 911);
        for frame in 1..=20 {
            counter.tick(now + Duration::from_millis(frame * 66));
        }

        assert!(counter.displayed > 50);
        assert!(counter.displayed < 125);
    }

    #[test]
    pub(super) fn games_found_counter_does_not_drop_when_full_scan_starts_lower() {
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        counter.progress_detail("Finding games", "Games found: 911", now);
        counter.displayed = 650;
        counter.target = 911;
        assert_eq!(
            counter.progress_detail("Classifying library", "Games found: 50", now),
            Some("Games found: 650".to_string())
        );
        assert_eq!(counter.displayed, 650);
        assert_eq!(counter.target, 911);

        assert_eq!(
            counter.progress_detail("Classifying library", "Games found: 700", now),
            Some("Games found: 650".to_string())
        );
        assert_eq!(counter.target, 700);
        assert_eq!(counter.phase, Some(CatalogCounterPhase::FullScan));
    }

    #[test]
    pub(super) fn full_scan_counter_takeover_requires_visible_overtake() {
        assert!(!counter_climb_target_overtakes_visible(50, 650));
        assert!(!counter_climb_target_overtakes_visible(650, 650));
        assert!(counter_climb_target_overtakes_visible(700, 650));
    }

    #[test]
    pub(super) fn games_found_counter_catches_large_lag_without_overshoot() {
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        counter.progress_detail("Classifying library", "Games found: 1000", now);
        for frame in 1..20 {
            counter.tick(now + Duration::from_millis(frame * 66));
        }

        assert!(counter.displayed > 900);
        assert!(counter.displayed <= 1000);
    }

    #[test]
    pub(super) fn games_found_counter_ignores_other_scan_phases() {
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        counter.progress_detail("Classifying library", "Games found: 100", now);
        assert_eq!(
            counter.progress_detail(
                "Saving library",
                "Writing 0 of 100 games into SQLite...",
                now
            ),
            None
        );
        assert!(!counter.active);
    }

    #[test]
    pub(super) fn duplicate_cached_catalog_ready_is_skipped_after_sync_load() {
        assert!(duplicate_cached_catalog_ready(true, true));
        assert!(!duplicate_cached_catalog_ready(false, true));
        assert!(!duplicate_cached_catalog_ready(true, false));
    }
}
