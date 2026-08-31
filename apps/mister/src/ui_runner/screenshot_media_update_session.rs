// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_worker_intents::{LauncherWorkerUiIntent, MediaProgressDisplay};
use super::*;
use std::collections::BTreeSet;

const MEDIA_PROGRESS_DONE_HOLD: Duration = Duration::from_secs(2);
const MEDIA_INTERACTION_SETTLE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    EnsureWorker {
        mode: &'static str,
    },
    EnsureSystem {
        system_id: String,
    },
    FinishWorker,
    DropWorker,
    MarkWorkerUnavailable,
    ClearPreviewFailures,
    ApplyPreviewAvailability {
        system_id: String,
        games: Vec<mister_magik_catalog::system_shard::SystemGame>,
    },
    SetInteractionActive {
        active: bool,
        reason: &'static str,
    },
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
    pending_system_checks: BTreeSet<String>,
    dispatched_system_checks: BTreeSet<String>,
    worker_episode_active: bool,
    visible_system_id: Option<String>,
    progress_display: MediaProgressDisplay,
    progress_clear_at: Option<Instant>,
    interaction_block_until: Option<Instant>,
    last_gate: MediaInteractionGate,
    low_memory_paused: bool,
}

impl Default for ScreenshotMediaUpdateSession {
    fn default() -> Self {
        let pending_system_checks = BTreeSet::from(["arcade".to_string()]);
        Self {
            pending_system_checks,
            dispatched_system_checks: BTreeSet::new(),
            worker_episode_active: false,
            visible_system_id: None,
            progress_display: MediaProgressDisplay::default(),
            progress_clear_at: None,
            interaction_block_until: None,
            last_gate: MediaInteractionGate {
                active: true,
                reason: "startup",
            },
            low_memory_paused: false,
        }
    }
}

impl ScreenshotMediaUpdateSession {
    pub(super) fn request_catalog_seed(&mut self) {}

    pub(super) fn observe_system_entry(&mut self, system_id: Option<&str>) {
        if self.visible_system_id.as_deref() == system_id {
            return;
        }
        self.visible_system_id = system_id.map(str::to_string);
        if let Some(system_id) = system_id {
            self.pending_system_checks.insert(system_id.to_string());
        }
    }

    pub(super) fn note_nav_change(
        &mut self,
        before: &LauncherProjectionKey,
        after: &LauncherProjectionKey,
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
        suppress_arcade_scroll_gate: bool,
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
        if !suppress_arcade_scroll_gate
            && self
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
        self.low_memory_paused = gate.active && gate.reason == "low-memory";
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
        _media_gate: Option<MediaInteractionGate>,
    ) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        effects.event(
            "screenshot_media_catalog_discovery_ignored",
            format!("system={system_id} policy=entry-only"),
        );
        effects
    }

    pub(super) fn finish_worker_if_no_catalog_seed_pending(&self) -> ScreenshotMediaUpdateEffects {
        ScreenshotMediaUpdateEffects::default()
    }

    pub(super) fn finish_worker(&self) -> ScreenshotMediaUpdateEffects {
        ScreenshotMediaUpdateEffects::default()
    }

    pub(super) fn apply_gate(
        &mut self,
        gate: MediaInteractionGate,
    ) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        if self.worker_episode_active || gate.reason == "low-memory" {
            return effects;
        }
        let undispatched = self
            .pending_system_checks
            .difference(&self.dispatched_system_checks)
            .cloned()
            .collect::<Vec<_>>();
        if !undispatched.is_empty() {
            self.worker_episode_active = true;
            effects.push(ScreenshotMediaUpdateEffect::EnsureWorker {
                mode: "fresh-manifest-check",
            });
            effects.push(ScreenshotMediaUpdateEffect::SetInteractionActive {
                active: gate.active,
                reason: gate.reason,
            });
        }
        for system_id in undispatched {
            self.dispatched_system_checks.insert(system_id.clone());
            effects.push(ScreenshotMediaUpdateEffect::EnsureSystem { system_id });
        }
        effects
    }

    pub(super) fn pause_for_low_memory(
        &mut self,
        _retain_worker_for_benchmark: bool,
    ) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        self.progress_clear_at = None;
        effects.event("screenshot_media_low_memory_pause", "reason=low-memory");
        effects.ui(self.progress_display.clear_intent());
        effects.push(ScreenshotMediaUpdateEffect::SetInteractionActive {
            active: true,
            reason: "low-memory",
        });
        self.low_memory_paused = true;
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
                effects.event("screenshot_media_progress", event.log_detail());
                let intent = self.progress_display.progress_intent(&event);
                let display_changed = !matches!(intent, LauncherWorkerUiIntent::None);
                if display_changed {
                    self.progress_clear_at = None;
                }
                if !self.low_memory_paused
                    && event.system != "all"
                    && self.progress_display.has_visible_rows()
                {
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
                if display_changed && self.progress_display.all_requested_terminal() {
                    self.progress_clear_at = Some(now + MEDIA_PROGRESS_DONE_HOLD);
                }
                if display_changed && !self.low_memory_paused {
                    effects.ui(intent);
                }
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
                if matches!(status.as_str(), "current" | "downloaded") {
                    effects.push(ScreenshotMediaUpdateEffect::ClearPreviewFailures);
                }
                if matches!(
                    status.as_str(),
                    "current" | "downloaded" | "failed" | "unavailable" | "checked"
                ) {
                    self.pending_system_checks.remove(&system);
                    self.dispatched_system_checks.remove(&system);
                }
            }
            MediaWorkerMessage::PreviewAvailabilityUpdated { outcome } => {
                effects.event(
                    "screenshot_media_catalog_updated",
                    format!(
                        "system={} previous_generation={} generation={} candidates={} available={} changed={} existing_identity_rows={} derived_identity_rows={} ambiguous_identity_rows={} resolver_status={:?}",
                        outcome.system_id,
                        outcome.previous_generation,
                        outcome.generation,
                        outcome.candidate_rows,
                        outcome.available_rows,
                        outcome.changed_rows,
                        outcome.existing_identity_rows,
                        outcome.derived_identity_rows,
                        outcome.ambiguous_identity_rows,
                        outcome.resolver_status
                    ),
                );
                effects.push(ScreenshotMediaUpdateEffect::ApplyPreviewAvailability {
                    system_id: outcome.system_id.as_str().to_string(),
                    games: outcome.games,
                });
            }
            MediaWorkerMessage::PreviewAvailabilityFailed { system, detail } => {
                effects.event(
                    "screenshot_media_catalog_update_failed",
                    format!("system={system} error={detail}"),
                );
            }
            MediaWorkerMessage::Failed { detail } => {
                effects.event("screenshot_media_update_failed", detail);
                self.progress_clear_at = None;
                effects.push(ScreenshotMediaUpdateEffect::MarkWorkerUnavailable);
                effects.ui(self.progress_display.clear_intent());
                effects.push(ScreenshotMediaUpdateEffect::DropWorker);
                self.worker_episode_active = false;
                self.dispatched_system_checks.clear();
            }
            MediaWorkerMessage::Done { detail } => {
                effects.event("screenshot_media_update_done", detail);
                if self.progress_display.has_visible_rows() {
                    self.progress_clear_at = Some(now + MEDIA_PROGRESS_DONE_HOLD);
                }
                effects.push(ScreenshotMediaUpdateEffect::DropWorker);
                self.worker_episode_active = false;
                self.dispatched_system_checks.clear();
            }
        }
        effects
    }

    pub(super) fn shutdown_for_reset(&mut self) -> ScreenshotMediaUpdateEffects {
        let mut effects = ScreenshotMediaUpdateEffects::default();
        effects.push(ScreenshotMediaUpdateEffect::FinishWorker);
        effects.push(ScreenshotMediaUpdateEffect::MarkWorkerUnavailable);
        effects.push(ScreenshotMediaUpdateEffect::DropWorker);
        self.worker_episode_active = false;
        self.dispatched_system_checks.clear();
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
                ScreenshotMediaUpdateEffect::FinishWorker => "finish-worker",
                ScreenshotMediaUpdateEffect::DropWorker => "drop-worker",
                ScreenshotMediaUpdateEffect::MarkWorkerUnavailable => "mark-unavailable",
                ScreenshotMediaUpdateEffect::ClearPreviewFailures => "clear-preview-failures",
                ScreenshotMediaUpdateEffect::ApplyPreviewAvailability { .. } => {
                    "apply-preview-availability"
                }
                ScreenshotMediaUpdateEffect::SetInteractionActive { .. } => "set-interaction",
            })
            .collect()
    }

    fn media_progress_event(system: &str, phase: &str, pack_count: usize) -> MediaProgressEvent {
        MediaProgressEvent {
            system: system.to_string(),
            image_size: "320x320".to_string(),
            variant: "identity".to_string(),
            phase: phase.to_string(),
            bytes_done: 128,
            bytes_total: 256,
            pack_index: 1,
            pack_count,
            download_mbps: None,
            detail: String::new(),
        }
    }

    #[test]
    fn startup_arcade_check_starts_even_while_the_media_gate_is_active() {
        let mut session = ScreenshotMediaUpdateSession::default();

        let startup = session.apply_gate(MediaInteractionGate {
            active: true,
            reason: "startup",
        });
        assert_eq!(
            effect_names(startup),
            vec!["ensure-worker", "set-interaction", "ensure-system"]
        );
        assert!(session.pending_system_checks.contains("arcade"));
        assert!(session.dispatched_system_checks.contains("arcade"));
    }

    #[test]
    fn catalog_discovery_never_queues_a_screenshot_pack() {
        let mut session = ScreenshotMediaUpdateSession::default();

        let effects = session.handle_catalog_system_discovered(
            "neogeo".to_string(),
            Some(MediaInteractionGate {
                active: true,
                reason: "catalog-build",
            }),
        );
        assert_eq!(effect_names(effects), vec!["event"]);
        assert!(!session.pending_system_checks.contains("neogeo"));
    }

    #[test]
    fn entering_a_system_queues_one_fresh_manifest_check_per_visit() {
        let mut session = ScreenshotMediaUpdateSession::default();
        session.pending_system_checks.clear();

        session.observe_system_entry(Some("snes"));
        let first = session.apply_gate(MediaInteractionGate {
            active: false,
            reason: "idle",
        });
        assert_eq!(
            effect_names(first),
            vec!["ensure-worker", "set-interaction", "ensure-system"]
        );
        session.observe_system_entry(Some("snes"));
        assert!(effect_names(session.apply_gate(session.last_gate)).is_empty());

        let _ = session.handle_worker_message(
            MediaWorkerMessage::PackStatus {
                system: "snes".to_string(),
                image_size: "256x224".to_string(),
                status: "current".to_string(),
                detail: String::new(),
            },
            false,
            Instant::now(),
        );
        session.observe_system_entry(None);
        session.observe_system_entry(Some("snes"));
        assert!(effect_names(session.apply_gate(session.last_gate)).is_empty());
        let _ = session.handle_worker_message(
            MediaWorkerMessage::Done {
                detail: "checked=1".to_string(),
            },
            false,
            Instant::now(),
        );
        assert_eq!(
            effect_names(session.apply_gate(session.last_gate)),
            vec!["ensure-worker", "set-interaction", "ensure-system"]
        );
    }

    #[test]
    fn transient_pack_failure_is_retried_with_a_new_worker_and_manifest() {
        let mut session = ScreenshotMediaUpdateSession::default();
        let gate = MediaInteractionGate {
            active: false,
            reason: "idle",
        };
        let _ = session.apply_gate(gate);
        let _ = session.handle_worker_message(
            MediaWorkerMessage::PackStatus {
                system: "arcade".to_string(),
                image_size: "320x320".to_string(),
                status: "retryable-failed".to_string(),
                detail: "curl exited with status 6".to_string(),
            },
            false,
            Instant::now(),
        );
        assert!(session.pending_system_checks.contains("arcade"));

        let done = session.handle_worker_message(
            MediaWorkerMessage::Done {
                detail: "failed=1".to_string(),
            },
            false,
            Instant::now(),
        );
        assert_eq!(effect_names(done), vec!["event", "drop-worker"]);
        assert_eq!(
            effect_names(session.apply_gate(gate)),
            vec!["ensure-worker", "set-interaction", "ensure-system"]
        );
    }

    #[test]
    fn a_later_system_entry_waits_for_a_new_manifest_episode() {
        let mut session = ScreenshotMediaUpdateSession::default();
        let gate = MediaInteractionGate {
            active: false,
            reason: "idle",
        };
        let _ = session.apply_gate(gate);

        session.observe_system_entry(Some("snes"));
        assert!(effect_names(session.apply_gate(gate)).is_empty());

        let _ = session.handle_worker_message(
            MediaWorkerMessage::PackStatus {
                system: "arcade".to_string(),
                image_size: "320x320".to_string(),
                status: "current".to_string(),
                detail: String::new(),
            },
            false,
            Instant::now(),
        );
        let _ = session.handle_worker_message(
            MediaWorkerMessage::Done {
                detail: "current=1".to_string(),
            },
            false,
            Instant::now(),
        );
        assert_eq!(
            effect_names(session.apply_gate(gate)),
            vec!["ensure-worker", "set-interaction", "ensure-system"]
        );
        assert!(session.dispatched_system_checks.contains("snes"));
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
    fn current_or_downloaded_pack_clears_failed_preview_paths() {
        let mut session = ScreenshotMediaUpdateSession::default();

        let current = session.handle_worker_message(
            MediaWorkerMessage::PackStatus {
                system: "arcade".to_string(),
                image_size: "320x320".to_string(),
                status: "current".to_string(),
                detail: "local_path=/media/fat/mister-magik/assets/arcade.mmlz4b".to_string(),
            },
            false,
            Instant::now(),
        );
        assert_eq!(
            effect_names(current),
            vec!["event", "clear-preview-failures"]
        );

        let downloaded = session.handle_worker_message(
            MediaWorkerMessage::PackStatus {
                system: "arcade".to_string(),
                image_size: "320x320".to_string(),
                status: "downloaded".to_string(),
                detail: "local_path=/media/fat/mister-magik/assets/arcade.mmlz4b".to_string(),
            },
            false,
            Instant::now(),
        );
        assert_eq!(
            effect_names(downloaded),
            vec!["event", "clear-preview-failures"]
        );
    }

    #[test]
    fn catalog_reconciliation_messages_are_logged_without_changing_worker_health() {
        let mut session = ScreenshotMediaUpdateSession::default();
        let updated = session.handle_worker_message(
            MediaWorkerMessage::PreviewAvailabilityUpdated {
                outcome: mister_magik_catalog::preview_availability::PreviewAvailabilityReconciliationOutcome {
                    system_id: mister_magik_catalog::catalog_classify::SystemId::parse("arcade")
                        .unwrap(),
                    previous_generation: 1,
                    generation: 2,
                    candidate_rows: 2,
                    available_rows: 1,
                    changed_rows: 1,
                    existing_identity_rows: 2,
                    derived_identity_rows: 0,
                    ambiguous_identity_rows: 0,
                    resolver_status:
                        mister_magik_catalog::preview_availability::PreviewIdentityResolverStatus::NotNeeded,
                    games: Vec::new(),
                },
            },
            false,
            Instant::now(),
        );
        assert_eq!(
            effect_names(updated),
            vec!["event", "apply-preview-availability"]
        );

        let failed = session.handle_worker_message(
            MediaWorkerMessage::PreviewAvailabilityFailed {
                system: "arcade".to_string(),
                detail: "publish failed".to_string(),
            },
            false,
            Instant::now(),
        );
        assert_eq!(effect_names(failed), vec!["event"]);
    }

    #[test]
    fn progress_message_logs_visibility_and_terminal_clear() {
        let now = Instant::now();
        let mut session = ScreenshotMediaUpdateSession::default();
        let active = MediaProgressEvent {
            download_mbps: Some(4.0),
            ..media_progress_event("neogeo", "download", 1)
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
            phase: "download_done".to_string(),
            bytes_done: 256,
            ..media_progress_event("neogeo", "", 1)
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
    fn check_only_progress_never_shows_media_popup() {
        let now = Instant::now();
        let mut session = ScreenshotMediaUpdateSession::default();

        assert_eq!(
            effect_names(session.handle_worker_message(
                MediaWorkerMessage::Progress(media_progress_event("arcade", "check-only", 1)),
                false,
                now,
            )),
            vec!["event"]
        );
        assert!(session.progress_clear_at.is_none());

        assert_eq!(
            effect_names(session.handle_worker_message(
                MediaWorkerMessage::Progress(media_progress_event("arcade", "skipped-current", 1)),
                false,
                now,
            )),
            vec!["event"]
        );
        assert!(session.progress_clear_at.is_none());
    }

    #[test]
    fn save_after_download_done_does_not_extend_media_popup() {
        let now = Instant::now();
        let mut session = ScreenshotMediaUpdateSession::default();

        let _ = session.handle_worker_message(
            MediaWorkerMessage::Progress(media_progress_event("arcade", "download", 1)),
            false,
            now,
        );
        let _ = session.handle_worker_message(
            MediaWorkerMessage::Progress(media_progress_event("arcade", "download_done", 1)),
            false,
            now + Duration::from_millis(100),
        );
        let clear_at = session.progress_clear_at;

        assert_eq!(
            effect_names(session.handle_worker_message(
                MediaWorkerMessage::Progress(media_progress_event("arcade", "save", 1)),
                false,
                now + Duration::from_millis(500),
            )),
            vec!["event", "event"]
        );
        assert_eq!(session.progress_clear_at, clear_at);
    }

    #[test]
    fn media_gate_priority_and_duplicate_sync_are_stable() {
        let now = Instant::now();
        let mut session = ScreenshotMediaUpdateSession::default();

        let startup = session.current_gate(false, true, true, false, now);
        assert!(startup.active);
        assert_eq!(startup.reason, "startup");

        assert!(effect_names(session.sync_gate(startup)).is_empty());

        let launch = session.current_gate(true, true, true, false, now);
        assert!(launch.active);
        assert_eq!(launch.reason, "launch-handoff");
        assert_eq!(
            effect_names(session.sync_gate(launch)),
            vec!["event", "set-interaction"]
        );

        let benchmark = session.current_gate(true, false, true, false, now);
        assert!(benchmark.active);
        assert_eq!(benchmark.reason, "benchmark");

        let mut before_nav = LauncherNav::new();
        before_nav.screen = Screen::Home;
        let before = LauncherProjectionKey::from_nav(&before_nav);
        let mut after_nav = LauncherNav::new();
        after_nav.screen = Screen::Arcade;
        let after = LauncherProjectionKey::from_nav(&after_nav);
        session.note_nav_change(&before, &after, now);
        let scroll = session.current_gate(true, false, false, false, now);
        assert!(scroll.active);
        assert_eq!(scroll.reason, "arcade-scroll");

        let idle = session.current_gate(true, false, false, false, now + MEDIA_INTERACTION_SETTLE);
        assert!(!idle.active);
        assert_eq!(idle.reason, "idle");

        let contention = session.current_gate(true, false, false, true, now);
        assert!(!contention.active);
        assert_eq!(contention.reason, "idle");
    }

    #[test]
    fn done_worker_message_without_download_does_not_show_media_popup() {
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
        assert!(session.progress_clear_at.is_none());
        assert!(effect_names(session.clear_progress_if_due(now)).is_empty());
        assert!(
            effect_names(session.clear_progress_if_due(now + MEDIA_PROGRESS_DONE_HOLD)).is_empty()
        );
    }

    #[test]
    fn low_memory_pause_retains_worker_drains_progress_and_accepts_done() {
        let now = Instant::now();
        let mut session = ScreenshotMediaUpdateSession::default();

        assert_eq!(
            effect_names(session.pause_for_low_memory(true)),
            vec!["event", "ui", "set-interaction"]
        );
        assert_eq!(
            effect_names(session.handle_worker_message(
                MediaWorkerMessage::Progress(media_progress_event("arcade", "download", 1)),
                false,
                now,
            )),
            vec!["event"]
        );
        assert_eq!(
            effect_names(session.handle_worker_message(
                MediaWorkerMessage::Done {
                    detail: "packs=1 current=0 missing=1 stale=0 downloaded=1 failed=0".to_string(),
                },
                false,
                now,
            )),
            vec!["event", "drop-worker"]
        );
    }

    #[test]
    fn production_low_memory_pause_keeps_the_single_retry_worker() {
        let mut session = ScreenshotMediaUpdateSession::default();

        assert_eq!(
            effect_names(session.pause_for_low_memory(false)),
            vec!["event", "ui", "set-interaction"]
        );
    }
}
