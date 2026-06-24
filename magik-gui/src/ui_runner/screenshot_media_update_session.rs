use super::launcher_worker_intents::{LauncherWorkerUiIntent, MediaProgressDisplay};
use super::*;

const MEDIA_PROGRESS_DONE_HOLD: Duration = Duration::from_secs(8);
const MEDIA_INTERACTION_SETTLE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct MediaInteractionGate {
    pub(super) active: bool,
    pub(super) reason: &'static str,
}

pub(super) struct ScreenshotMediaUpdateEvent {
    pub(super) name: String,
    pub(super) detail: String,
}

pub(super) enum ScreenshotMediaUpdateEffect {
    StartupEvent(ScreenshotMediaUpdateEvent),
    Ui(LauncherWorkerUiIntent),
    EnsureWorker { mode: &'static str },
    EnsureSystem { system_id: String },
    EnsureCatalogSystems,
    FinishWorker,
    DropWorker,
    MarkWorkerUnavailable,
    SetInteractionActive { active: bool, reason: &'static str },
}

#[derive(Default)]
pub(super) struct ScreenshotMediaUpdateEffects {
    effects: Vec<ScreenshotMediaUpdateEffect>,
}

impl ScreenshotMediaUpdateEffects {
    fn push(&mut self, effect: ScreenshotMediaUpdateEffect) {
        self.effects.push(effect);
    }

    fn event(&mut self, name: impl Into<String>, detail: impl Into<String>) {
        self.push(ScreenshotMediaUpdateEffect::StartupEvent(
            ScreenshotMediaUpdateEvent {
                name: name.into(),
                detail: detail.into(),
            },
        ));
    }

    fn ui(&mut self, intent: LauncherWorkerUiIntent) {
        self.push(ScreenshotMediaUpdateEffect::Ui(intent));
    }

    pub(super) fn into_effects(self) -> impl IntoIterator<Item = ScreenshotMediaUpdateEffect> {
        self.effects
    }
}

pub(super) struct ScreenshotMediaUpdateSession {
    catalog_seed_pending: bool,
    catalog_seed_defer_reason: Option<&'static str>,
    progress_display: MediaProgressDisplay,
    progress_clear_at: Option<Instant>,
    interaction_block_until: Option<Instant>,
    last_gate: MediaInteractionGate,
}

impl Default for ScreenshotMediaUpdateSession {
    fn default() -> Self {
        Self {
            catalog_seed_pending: false,
            catalog_seed_defer_reason: None,
            progress_display: MediaProgressDisplay::default(),
            progress_clear_at: None,
            interaction_block_until: None,
            last_gate: MediaInteractionGate {
                active: true,
                reason: "startup",
            },
        }
    }
}

impl ScreenshotMediaUpdateSession {
    pub(super) fn request_catalog_seed(&mut self) {
        self.catalog_seed_pending = true;
        self.catalog_seed_defer_reason = None;
    }

    pub(super) fn note_nav_change(
        &mut self,
        before: &LauncherBridgeKey,
        after: &LauncherBridgeKey,
        now: Instant,
    ) {
        if before.screen == Screen::Arcade || after.screen == Screen::Arcade {
            self.interaction_block_until = Some(now + MEDIA_INTERACTION_SETTLE);
        }
    }

    pub(super) fn current_gate(
        &self,
        first_visible_copy_done: bool,
        launch_handoff_active: bool,
        benchmark_interaction_active: bool,
        now: Instant,
    ) -> MediaInteractionGate {
        if !first_visible_copy_done {
            return MediaInteractionGate {
                active: true,
                reason: "startup",
            };
        }
        if launch_handoff_active {
            return MediaInteractionGate {
                active: true,
                reason: "launch-handoff",
            };
        }
        if benchmark_interaction_active {
            return MediaInteractionGate {
                active: true,
                reason: "benchmark",
            };
        }
        if self
            .interaction_block_until
            .is_some_and(|until| now < until)
        {
            return MediaInteractionGate {
                active: true,
                reason: "arcade-scroll",
            };
        }
        MediaInteractionGate {
            active: false,
            reason: "idle",
        }
    }

    pub(super) fn sync_gate(&mut self, gate: MediaInteractionGate) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        if self.last_gate == gate {
            return effects;
        }
        self.last_gate = gate;
        effects.event(
            "screenshot_media_interaction_gate",
            format!(
                "active={} reason={}",
                if gate.active { 1 } else { 0 },
                gate.reason
            ),
        );
        effects.push(ScreenshotMediaUpdateEffect::SetInteractionActive {
            active: gate.active,
            reason: gate.reason,
        });
        effects
    }

    pub(super) fn handle_catalog_system_discovered(
        &mut self,
        system_id: String,
        media_gate: Option<MediaInteractionGate>,
    ) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        effects.event("catalog_system_discovered", format!("system={system_id}"));
        let Some(media_gate) = media_gate else {
            self.catalog_seed_pending = true;
            effects.event(
                "screenshot_media_catalog_defer",
                format!("system={system_id} reason=media-gate-unavailable"),
            );
            return effects;
        };
        if media_gate.active {
            self.catalog_seed_pending = true;
            effects.event(
                "screenshot_media_catalog_defer",
                format!("system={system_id} reason={}", media_gate.reason),
            );
        } else {
            effects.push(ScreenshotMediaUpdateEffect::EnsureWorker {
                mode: "discovered-system",
            });
            effects.push(ScreenshotMediaUpdateEffect::EnsureSystem { system_id });
        }
        effects
    }

    pub(super) fn finish_worker_if_no_catalog_seed_pending(&self) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        if !self.catalog_seed_pending {
            effects.push(ScreenshotMediaUpdateEffect::FinishWorker);
        }
        effects
    }

    pub(super) fn finish_worker(&self) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        effects.push(ScreenshotMediaUpdateEffect::FinishWorker);
        effects
    }

    pub(super) fn apply_gate(
        &mut self,
        gate: MediaInteractionGate,
    ) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        if self.catalog_seed_pending && !gate.active {
            self.catalog_seed_pending = false;
            self.catalog_seed_defer_reason = None;
            effects.push(ScreenshotMediaUpdateEffect::EnsureCatalogSystems);
        } else if self.catalog_seed_pending && self.catalog_seed_defer_reason != Some(gate.reason) {
            self.catalog_seed_defer_reason = Some(gate.reason);
            effects.event(
                "screenshot_media_catalog_defer",
                format!("reason={}", gate.reason),
            );
        }
        effects
    }

    pub(super) fn clear_progress_if_due(&mut self, now: Instant) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        if self
            .progress_clear_at
            .is_some_and(|deadline| now >= deadline)
        {
            effects.ui(self.progress_display.clear_intent());
            self.progress_clear_at = None;
        }
        effects
    }

    pub(super) fn handle_worker_message(
        &mut self,
        message: MediaWorkerMessage,
        catalog_scan_visible: bool,
        now: Instant,
    ) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        match message {
            MediaWorkerMessage::Timing { name, detail } => {
                effects.event(name, detail);
            }
            MediaWorkerMessage::Progress(event) => {
                self.progress_clear_at = None;
                effects.event("screenshot_media_progress", event.log_detail());
                let intent = self.progress_display.progress_intent(&event);
                if event.system != "all" {
                    let standalone_visible =
                        !catalog_scan_visible && self.progress_display.has_visible_rows();
                    effects.event(
                        "screenshot_media_ui_visibility",
                        self.progress_display.visibility_log_detail(
                            &event.system,
                            catalog_scan_visible,
                            standalone_visible,
                        ),
                    );
                }
                if self.progress_display.all_requested_terminal() {
                    self.progress_clear_at = Some(now + MEDIA_PROGRESS_DONE_HOLD);
                }
                effects.ui(intent);
            }
            MediaWorkerMessage::CacheMetadata { scope, metadata } => {
                effects.event(
                    "screenshot_media_cache_metadata",
                    metadata.log_detail(&scope),
                );
            }
            MediaWorkerMessage::PackStatus {
                system,
                image_size,
                status,
                detail,
            } => {
                effects.event(
                    "screenshot_media_pack_status",
                    format!("system={system} image_size={image_size} status={status} {detail}"),
                );
            }
            MediaWorkerMessage::Failed { detail } => {
                effects.event("screenshot_media_update_failed", detail);
                self.progress_clear_at = None;
                effects.push(ScreenshotMediaUpdateEffect::MarkWorkerUnavailable);
                effects.ui(self.progress_display.clear_intent());
                effects.push(ScreenshotMediaUpdateEffect::DropWorker);
            }
            MediaWorkerMessage::Done { detail } => {
                effects.event("screenshot_media_update_done", detail);
                self.progress_clear_at = Some(now + MEDIA_PROGRESS_DONE_HOLD);
                effects.push(ScreenshotMediaUpdateEffect::DropWorker);
            }
        }
        effects
    }

    pub(super) fn shutdown_for_reset(&mut self) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        effects.push(ScreenshotMediaUpdateEffect::FinishWorker);
        effects.push(ScreenshotMediaUpdateEffect::MarkWorkerUnavailable);
        effects.push(ScreenshotMediaUpdateEffect::DropWorker);
        self.progress_clear_at = None;
        effects.ui(self.progress_display.clear_intent());
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect_names(effects: ScreenshotMediaUpdateEffects) -> Vec<&'static str> {
        effects
            .into_effects()
            .into_iter()
            .map(|effect| match effect {
                ScreenshotMediaUpdateEffect::StartupEvent(_) => "event",
                ScreenshotMediaUpdateEffect::Ui(_) => "ui",
                ScreenshotMediaUpdateEffect::EnsureWorker { .. } => "ensure-worker",
                ScreenshotMediaUpdateEffect::EnsureSystem { .. } => "ensure-system",
                ScreenshotMediaUpdateEffect::EnsureCatalogSystems => "ensure-catalog-systems",
                ScreenshotMediaUpdateEffect::FinishWorker => "finish-worker",
                ScreenshotMediaUpdateEffect::DropWorker => "drop-worker",
                ScreenshotMediaUpdateEffect::MarkWorkerUnavailable => "mark-unavailable",
                ScreenshotMediaUpdateEffect::SetInteractionActive { .. } => "set-interaction",
            })
            .collect()
    }

    #[test]
    fn catalog_seed_defers_until_media_gate_is_idle() {
        let mut session = ScreenshotMediaUpdateSession::default();
        session.request_catalog_seed();

        let deferred = session.apply_gate(MediaInteractionGate {
            active: true,
            reason: "startup",
        });
        assert_eq!(effect_names(deferred), vec!["event"]);
        assert!(session.catalog_seed_pending);

        let ready = session.apply_gate(MediaInteractionGate {
            active: false,
            reason: "idle",
        });
        assert_eq!(effect_names(ready), vec!["ensure-catalog-systems"]);
        assert!(!session.catalog_seed_pending);
    }

    #[test]
    fn discovered_system_starts_worker_only_when_gate_is_idle() {
        let mut session = ScreenshotMediaUpdateSession::default();

        let active = session.handle_catalog_system_discovered(
            "neogeo".to_string(),
            Some(MediaInteractionGate {
                active: true,
                reason: "startup",
            }),
        );
        assert_eq!(effect_names(active), vec!["event", "event"]);
        assert!(session.catalog_seed_pending);

        let idle = session.handle_catalog_system_discovered(
            "arcade".to_string(),
            Some(MediaInteractionGate {
                active: false,
                reason: "idle",
            }),
        );
        assert_eq!(
            effect_names(idle),
            vec!["event", "ensure-worker", "ensure-system"]
        );
    }

    #[test]
    fn failed_worker_message_clears_progress_and_marks_unavailable() {
        let mut session = ScreenshotMediaUpdateSession::default();
        let effects = session.handle_worker_message(
            MediaWorkerMessage::Failed {
                detail: "manifest fetch failed".to_string(),
            },
            false,
            Instant::now(),
        );

        assert_eq!(
            effect_names(effects),
            vec!["event", "mark-unavailable", "ui", "drop-worker"]
        );
        assert!(session.progress_clear_at.is_none());
    }

    #[test]
    fn progress_message_logs_visibility_and_terminal_clear() {
        let now = Instant::now();
        let mut session = ScreenshotMediaUpdateSession::default();
        let active = MediaProgressEvent {
            system: "neogeo".to_string(),
            image_size: "320x320".to_string(),
            variant: "identity".to_string(),
            phase: "download".to_string(),
            bytes_done: 128,
            bytes_total: 256,
            pack_index: 1,
            pack_count: 1,
            download_mbps: Some(4.0),
            detail: String::new(),
        };

        assert_eq!(
            effect_names(session.handle_worker_message(
                MediaWorkerMessage::Progress(active),
                false,
                now
            )),
            vec!["event", "event", "ui"]
        );
        assert!(session.progress_clear_at.is_none());

        let done = MediaProgressEvent {
            phase: "done".to_string(),
            bytes_done: 256,
            ..MediaProgressEvent {
                system: "neogeo".to_string(),
                image_size: "320x320".to_string(),
                variant: "identity".to_string(),
                phase: String::new(),
                bytes_done: 0,
                bytes_total: 256,
                pack_index: 1,
                pack_count: 1,
                download_mbps: None,
                detail: String::new(),
            }
        };
        assert_eq!(
            effect_names(session.handle_worker_message(
                MediaWorkerMessage::Progress(done),
                false,
                now
            )),
            vec!["event", "event", "ui"]
        );
        assert_eq!(
            session.progress_clear_at,
            Some(now + MEDIA_PROGRESS_DONE_HOLD)
        );
    }

    #[test]
    fn media_gate_priority_and_duplicate_sync_are_stable() {
        let now = Instant::now();
        let mut session = ScreenshotMediaUpdateSession::default();

        let startup = session.current_gate(false, true, true, now);
        assert!(startup.active);
        assert_eq!(startup.reason, "startup");

        assert!(effect_names(session.sync_gate(startup)).is_empty());

        let launch = session.current_gate(true, true, true, now);
        assert!(launch.active);
        assert_eq!(launch.reason, "launch-handoff");
        assert_eq!(
            effect_names(session.sync_gate(launch)),
            vec!["event", "set-interaction"]
        );

        let benchmark = session.current_gate(true, false, true, now);
        assert!(benchmark.active);
        assert_eq!(benchmark.reason, "benchmark");

        let mut before_nav = LauncherNav::new();
        before_nav.screen = Screen::Home;
        let before = LauncherBridgeKey::from_nav(&before_nav);
        let mut after_nav = LauncherNav::new();
        after_nav.screen = Screen::Arcade;
        let after = LauncherBridgeKey::from_nav(&after_nav);
        session.note_nav_change(&before, &after, now);
        let scroll = session.current_gate(true, false, false, now);
        assert!(scroll.active);
        assert_eq!(scroll.reason, "arcade-scroll");

        let idle = session.current_gate(true, false, false, now + MEDIA_INTERACTION_SETTLE);
        assert!(!idle.active);
        assert_eq!(idle.reason, "idle");
    }

    #[test]
    fn done_worker_message_schedules_terminal_clear() {
        let now = Instant::now();
        let mut session = ScreenshotMediaUpdateSession::default();
        let effects = session.handle_worker_message(
            MediaWorkerMessage::Done {
                detail: "packs=1 current=0 missing=1 stale=0 downloaded=1 failed=0".to_string(),
            },
            false,
            now,
        );

        assert_eq!(effect_names(effects), vec!["event", "drop-worker"]);
        assert_eq!(
            session.progress_clear_at,
            Some(now + MEDIA_PROGRESS_DONE_HOLD)
        );
        assert!(effect_names(session.clear_progress_if_due(now)).is_empty());
        assert_eq!(
            effect_names(session.clear_progress_if_due(now + MEDIA_PROGRESS_DONE_HOLD)),
            vec!["ui"]
        );
    }
}
