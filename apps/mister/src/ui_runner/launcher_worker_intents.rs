// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) enum LauncherWorkerUiIntent {
    None,
    CatalogScan(CatalogScanBridgeStatus),
    ClearCatalogScan,
    ShowCatalogBackgroundScan,
    HideCatalogBackgroundScan,
    InfoDatabaseBuild(String),
    MediaProgress {
        progresses: ModelRc<slint_ui::launcher::ScreenshotPackProgress>,
        summary: String,
    },
}

impl LauncherWorkerUiIntent {
    pub(super) fn is_catalog_presentation(&self) -> bool {
        matches!(
            self,
            Self::CatalogScan(_)
                | Self::ClearCatalogScan
                | Self::ShowCatalogBackgroundScan
                | Self::HideCatalogBackgroundScan
        )
    }
}

pub(super) fn apply_launcher_worker_ui_intent(
    app: &slint_ui::launcher::Launcher,
    intent: LauncherWorkerUiIntent,
    full_bridge_dirty: &mut bool,
) {
    if sync_launcher_worker_ui_intent(app, intent) {
        *full_bridge_dirty = true;
    }
}

pub(super) fn sync_launcher_worker_ui_intent(
    app: &slint_ui::launcher::Launcher,
    intent: LauncherWorkerUiIntent,
) -> bool {
    let intent = match intent {
        LauncherWorkerUiIntent::None => return false,
        intent => intent,
    };
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let status_presenter = LauncherStatusPresenter::new(&bridge);
    match intent {
        LauncherWorkerUiIntent::None => unreachable!("handled before bridge lookup"),
        LauncherWorkerUiIntent::CatalogScan(status) => {
            status_presenter.sync_catalog_scan(status);
        }
        LauncherWorkerUiIntent::ClearCatalogScan => {
            status_presenter.clear_catalog_scan();
        }
        LauncherWorkerUiIntent::ShowCatalogBackgroundScan => {
            status_presenter.clear_catalog_scan();
            status_presenter.sync_catalog_background_scan_visible(true);
        }
        LauncherWorkerUiIntent::HideCatalogBackgroundScan => {
            status_presenter.sync_catalog_background_scan_visible(false);
        }
        LauncherWorkerUiIntent::InfoDatabaseBuild(value) => {
            bridge.set_info_database_build(value.into());
        }
        LauncherWorkerUiIntent::MediaProgress {
            progresses,
            summary,
        } => {
            status_presenter.sync_media_progresses(progresses, summary);
        }
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CatalogWorkerUiContext {
    pub catalog_ready: bool,
    pub screen: Screen,
    pub foreground_update: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CatalogProgressUiIntent {
    pub title: String,
    pub detail: String,
    pub failed: bool,
    pub counter_target: Option<CatalogCounterTarget>,
    visible: bool,
    background_visible: bool,
    message: &'static str,
    percent: i32,
}

impl CatalogProgressUiIntent {
    pub(super) fn from_worker_progress(
        context: CatalogWorkerUiContext,
        title: String,
        detail: String,
        percent: i32,
    ) -> Self {
        let visible = catalog_scan_progress_visible(
            context.catalog_ready,
            context.screen,
            &title,
            context.foreground_update,
        );
        let background_visible =
            catalog_background_scan_progress_visible(context.catalog_ready, visible, &title);
        let counter_target = CatalogCounterPhase::for_title(&title).and_then(|phase| {
            parse_games_found_detail(&detail).map(|target| CatalogCounterTarget { phase, target })
        });
        Self {
            failed: catalog_progress_title_is_failure(&title),
            title,
            detail,
            counter_target,
            visible,
            background_visible,
            message: catalog_scan_message(context.foreground_update),
            percent,
        }
    }

    pub(super) fn ui_with_detail(self, detail: Option<String>) -> LauncherWorkerUiIntent {
        LauncherWorkerUiIntent::CatalogScan(CatalogScanBridgeStatus::new(
            self.visible,
            self.background_visible,
            self.message,
            self.title,
            detail.unwrap_or(self.detail),
            self.percent,
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CatalogCounterTarget {
    pub phase: CatalogCounterPhase,
    pub target: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogCounterPhase {
    Bootstrap,
    FullScan,
}

impl CatalogCounterPhase {
    pub(super) fn for_title(title: &str) -> Option<Self> {
        match title {
            "Finding games" => Some(Self::Bootstrap),
            "Classifying library" => Some(Self::FullScan),
            _ => None,
        }
    }
}

pub(super) fn cached_catalog_validation_intent(
    foreground_update: bool,
    games: usize,
) -> LauncherWorkerUiIntent {
    LauncherWorkerUiIntent::CatalogScan(CatalogScanBridgeStatus::new(
        false,
        false,
        catalog_scan_message(foreground_update),
        "Validating library",
        format!("Using cached {games} games while checking for changes"),
        -1,
    ))
}

pub(super) fn catalog_rebuild_started_intent(foreground_update: bool) -> LauncherWorkerUiIntent {
    LauncherWorkerUiIntent::CatalogScan(CatalogScanBridgeStatus::new(
        foreground_update,
        !foreground_update,
        catalog_scan_message(foreground_update),
        if foreground_update {
            "Indexing library"
        } else {
            "Checking library"
        },
        if foreground_update {
            "Rebuilding catalog with latest games..."
        } else {
            "Comparing library changes..."
        },
        -1,
    ))
}

pub(super) fn catalog_plan_ready_intent(
    system_count: usize,
    all_published_systems: bool,
) -> LauncherWorkerUiIntent {
    LauncherWorkerUiIntent::CatalogScan(CatalogScanBridgeStatus::new(
        false,
        true,
        UPDATING_LIBRARY_SCAN_MESSAGE,
        "Updating library",
        if all_published_systems {
            "Updating all library systems".to_string()
        } else {
            format!(
                "Updating {system_count} system{}",
                if system_count == 1 { "" } else { "s" }
            )
        },
        -1,
    ))
}

pub(super) fn catalog_persistence_failed_intent(
    error: impl Into<String>,
) -> LauncherWorkerUiIntent {
    LauncherWorkerUiIntent::CatalogScan(CatalogScanBridgeStatus::new(
        true,
        false,
        FIRST_LIBRARY_SCAN_MESSAGE,
        "Library load failed",
        error.into(),
        -1,
    ))
}

pub(super) fn catalog_scan_message(foreground_update: bool) -> &'static str {
    if foreground_update {
        UPDATING_LIBRARY_SCAN_MESSAGE
    } else {
        FIRST_LIBRARY_SCAN_MESSAGE
    }
}

pub(super) fn catalog_scan_progress_visible(
    catalog_ready: bool,
    screen: Screen,
    title: &str,
    foreground_update: bool,
) -> bool {
    if catalog_progress_title_is_failure(title) {
        return true;
    }
    if foreground_update {
        return true;
    }
    if !catalog_ready {
        return screen == Screen::Home || screen == Screen::Arcade || title == "Indexing library";
    }
    false
}

pub(super) fn catalog_background_scan_progress_visible(
    catalog_ready: bool,
    full_scan_visible: bool,
    title: &str,
) -> bool {
    catalog_ready && !full_scan_visible && !catalog_progress_title_is_failure(title)
}

fn catalog_progress_title_is_failure(title: &str) -> bool {
    matches!(title, "Library scan failed" | "Library load failed")
}

pub(super) fn parse_games_found_detail(detail: &str) -> Option<usize> {
    detail.strip_prefix("Games found: ")?.trim().parse().ok()
}

#[derive(Default)]
pub(super) struct MediaProgressDisplay {
    pub(super) active: BTreeMap<String, MediaProgressDisplayRow>,
    pub(super) downloading: BTreeSet<String>,
    pub(super) done: BTreeSet<String>,
    pub(super) failed: BTreeSet<String>,
    requested_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MediaProgressDisplayRow {
    pub(super) system: String,
    pub(super) image_size: String,
    pub(super) phase: String,
    pub(super) percent: i32,
    pub(super) bytes_label: String,
    pub(super) pack_position: String,
}

impl MediaProgressDisplay {
    pub(super) fn progress_intent(&mut self, event: &MediaProgressEvent) -> LauncherWorkerUiIntent {
        if !self.apply(event) {
            return LauncherWorkerUiIntent::None;
        }
        self.sync_intent()
    }

    pub(super) fn clear_intent(&mut self) -> LauncherWorkerUiIntent {
        self.clear();
        self.sync_intent()
    }

    fn sync_intent(&self) -> LauncherWorkerUiIntent {
        LauncherWorkerUiIntent::MediaProgress {
            progresses: self.model(),
            summary: self.summary(),
        }
    }

    pub(super) fn visibility_log_detail(
        &self,
        system: &str,
        catalog_scan_visible: bool,
        standalone_visible: bool,
    ) -> String {
        let visible_systems = self
            .active
            .values()
            .take(3)
            .map(|row| row.system.as_str())
            .collect::<Vec<_>>();
        let row_index = visible_systems
            .iter()
            .position(|visible| *visible == system)
            .map(|index| index as isize)
            .unwrap_or(-1);
        let row_seen = row_index >= 0;
        let rendered = row_seen && (catalog_scan_visible || standalone_visible);
        let row = self.active.get(system);
        let phase = row
            .map(|row| log_token(&row.phase))
            .unwrap_or_else(|| "-".to_string());
        let percent = row.map(|row| row.percent).unwrap_or(-1);
        let (active, done, failed, total) = self.counts();
        format!(
            "system={} row_seen={} row_index={} rendered={} catalog_scan_visible={} standalone_visible={} active_rows={} visible_count={} visible_systems={} phase={} percent={} summary_active={} summary_done={} summary_failed={} summary_total={}",
            log_token(system),
            if row_seen { 1 } else { 0 },
            row_index,
            if rendered { 1 } else { 0 },
            if catalog_scan_visible { 1 } else { 0 },
            if standalone_visible { 1 } else { 0 },
            self.active.len(),
            visible_systems.len(),
            if visible_systems.is_empty() {
                "-".to_string()
            } else {
                visible_systems.join(",")
            },
            phase,
            percent,
            active,
            done,
            failed,
            total
        )
    }

    fn apply(&mut self, event: &MediaProgressEvent) -> bool {
        if event.system == "all" {
            return false;
        }
        if media_progress_download_active_event(event) && event.pack_count > 0 {
            self.requested_count = self.requested_count.max(event.pack_count);
        }
        if event.phase == "failed" {
            if !self.downloading.remove(&event.system) {
                return false;
            }
            let row = self.media_progress_display_row(event);
            self.failed.insert(event.system.clone());
            self.active.insert(event.system.clone(), row);
            return true;
        }
        if media_progress_download_done_event(event) {
            if !self.downloading.remove(&event.system) {
                return false;
            }
            let row = self.media_progress_display_row(event);
            self.done.insert(event.system.clone());
            self.active.insert(event.system.clone(), row);
            return true;
        }
        if !media_progress_download_active_event(event) {
            return false;
        }
        let row = self.media_progress_display_row(event);
        self.done.remove(&event.system);
        self.failed.remove(&event.system);
        self.downloading.insert(event.system.clone());
        self.active.insert(event.system.clone(), row);
        true
    }

    fn media_progress_display_row(&self, event: &MediaProgressEvent) -> MediaProgressDisplayRow {
        let mut row = media_progress_display_row(event);
        if let Some(previous) = self.active.get(&event.system) {
            row.percent = row.percent.max(previous.percent);
        }
        row
    }

    fn clear(&mut self) {
        self.active.clear();
        self.downloading.clear();
        self.done.clear();
        self.failed.clear();
        self.requested_count = 0;
    }

    fn model(&self) -> ModelRc<slint_ui::launcher::ScreenshotPackProgress> {
        let allocation_started = Instant::now();
        let rows = self
            .active
            .values()
            .take(3)
            .map(|row| slint_ui::launcher::ScreenshotPackProgress {
                system: mister_magik_catalog::catalog_classify::system_title(&row.system).into(),
                image_size: row.image_size.clone().into(),
                phase: row.phase.clone().into(),
                percent: row.percent,
                bytes_label: row.bytes_label.clone().into(),
                pack_position: row.pack_position.clone().into(),
            })
            .collect::<Vec<_>>();
        crate::launcher_presentation::bridge_churn_record_row_allocations(rows.len() as u64);
        crate::launcher_presentation::bridge_churn_record_shared_strings(
            rows.len().saturating_mul(6) as u64,
        );
        crate::launcher_presentation::bridge_churn_record_model_allocation_us(
            allocation_started.elapsed().as_micros(),
        );
        ModelRc::new(VecModel::from(rows))
    }

    pub(super) fn summary(&self) -> String {
        let (active, done, failed, total) = self.counts();
        if total == 0 {
            return String::new();
        }
        if failed > 0 {
            format!("screenshots {active} active · {done}/{total} done · {failed} failed")
        } else {
            format!("screenshots {active} active · {done}/{total} done")
        }
    }

    pub(super) fn has_visible_rows(&self) -> bool {
        !self.active.is_empty()
    }

    pub(super) fn all_requested_terminal(&self) -> bool {
        let (active, _done, _failed, total) = self.counts();
        total > 0 && active == 0
    }

    fn counts(&self) -> (usize, usize, usize, usize) {
        let active = self.downloading.len();
        let done = self.done.len();
        let failed = self.failed.len();
        let total = self.requested_count.max(active + done + failed);
        (active, done, failed, total)
    }
}

fn log_token(value: &str) -> String {
    let token = value
        .chars()
        .map(|ch| if ch.is_ascii_whitespace() { '_' } else { ch })
        .collect::<String>();
    if token.is_empty() {
        "-".to_string()
    } else {
        token
    }
}

fn media_progress_display_row(event: &MediaProgressEvent) -> MediaProgressDisplayRow {
    MediaProgressDisplayRow {
        system: event.system.clone(),
        image_size: event.image_size.clone(),
        phase: media_progress_phase_label(&event.phase),
        percent: media_progress_percent(&event.phase, event.bytes_done, event.bytes_total),
        bytes_label: String::new(),
        pack_position: if event.pack_index > 0 && event.pack_count > 0 {
            format!("{}/{}", event.pack_index, event.pack_count)
        } else {
            String::new()
        },
    }
}

fn media_progress_download_active_event(event: &MediaProgressEvent) -> bool {
    event.variant == "identity" && matches!(event.phase.as_str(), "download_start" | "download")
}

fn media_progress_download_done_event(event: &MediaProgressEvent) -> bool {
    event.variant == "identity" && event.phase == "download_done"
}

fn media_progress_percent(phase: &str, done: u64, total: u64) -> i32 {
    match phase {
        "check" | "download_start" => 0,
        "download" => scaled_progress(done, total, 0, 100),
        "download_done" | "verify" | "save" | "sync" | "rename" | "parent-sync" | "done"
        | "skipped-current" | "check-only" => 100,
        "failed" => scaled_progress(done, total, 0, 100),
        _ => scaled_progress(done, total, 0, 100),
    }
}

fn scaled_progress(done: u64, total: u64, offset: i32, span: i32) -> i32 {
    if total == 0 {
        return offset;
    }
    let phase_percent = done
        .min(total)
        .saturating_mul(span as u64)
        .checked_div(total)
        .map(|value| value as i32)
        .unwrap_or(0);
    (offset + phase_percent).clamp(0, 100)
}

fn media_progress_phase_label(phase: &str) -> String {
    match phase {
        "download_start" | "download" | "download_done" => "download".to_string(),
        "verify" => "verify".to_string(),
        "save" | "sync" | "rename" | "parent-sync" => "saving".to_string(),
        "done" => "downloaded".to_string(),
        "skipped-current" => "current".to_string(),
        "check-only" => "checked".to_string(),
        other => other.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media_progress_event(
        system: &str,
        phase: &str,
        bytes_done: u64,
        bytes_total: u64,
        pack_index: usize,
        pack_count: usize,
    ) -> MediaProgressEvent {
        MediaProgressEvent {
            system: system.to_string(),
            image_size: "320x320".to_string(),
            variant: "identity".to_string(),
            phase: phase.to_string(),
            bytes_done,
            bytes_total,
            pack_index,
            pack_count,
            download_mbps: None,
            detail: String::new(),
        }
    }

    fn media_progress_variant_event(
        system: &str,
        variant: &str,
        phase: &str,
        bytes_done: u64,
        bytes_total: u64,
        pack_index: usize,
        pack_count: usize,
    ) -> MediaProgressEvent {
        MediaProgressEvent {
            variant: variant.to_string(),
            ..media_progress_event(
                system,
                phase,
                bytes_done,
                bytes_total,
                pack_index,
                pack_count,
            )
        }
    }

    #[test]
    fn catalog_progress_intent_routes_ready_validation_to_background() {
        let intent = CatalogProgressUiIntent::from_worker_progress(
            CatalogWorkerUiContext {
                catalog_ready: true,
                screen: Screen::Home,
                foreground_update: false,
            },
            "Validating library".to_string(),
            "Checking stamp".to_string(),
            -1,
        );

        assert!(!intent.visible);
        assert!(intent.background_visible);
        assert!(!intent.failed);
    }

    #[test]
    fn catalog_progress_intent_marks_failures_foreground() {
        let intent = CatalogProgressUiIntent::from_worker_progress(
            CatalogWorkerUiContext {
                catalog_ready: true,
                screen: Screen::Arcade,
                foreground_update: false,
            },
            "Library scan failed".to_string(),
            "disk read failed".to_string(),
            -1,
        );

        assert!(intent.visible);
        assert!(!intent.background_visible);
        assert!(intent.failed);
    }

    #[test]
    fn survivability_persistence_failure_intent_is_visible_and_specific() {
        let LauncherWorkerUiIntent::CatalogScan(status) =
            catalog_persistence_failed_intent("insert profile: UNIQUE constraint failed")
        else {
            panic!("expected catalog scan intent");
        };

        assert!(status.visible());
        assert!(!status.background_visible());
        assert_eq!(status.title(), "Library load failed");
        assert_eq!(status.detail(), "insert profile: UNIQUE constraint failed");
    }

    #[test]
    fn survivability_catalog_failures_never_hide_in_background_progress() {
        let scan_failed = CatalogProgressUiIntent::from_worker_progress(
            CatalogWorkerUiContext {
                catalog_ready: true,
                screen: Screen::Home,
                foreground_update: false,
            },
            "Library scan failed".to_string(),
            "unsupported filesystem entry".to_string(),
            -1,
        );
        let load_failed = CatalogProgressUiIntent::from_worker_progress(
            CatalogWorkerUiContext {
                catalog_ready: true,
                screen: Screen::Arcade,
                foreground_update: false,
            },
            "Library load failed".to_string(),
            "sqlite projection corrupt".to_string(),
            -1,
        );

        assert!(scan_failed.visible);
        assert!(!scan_failed.background_visible);
        assert!(scan_failed.failed);
        assert!(load_failed.visible);
        assert!(!load_failed.background_visible);
        assert!(load_failed.failed);
    }

    #[test]
    fn catalog_progress_intent_extracts_counter_targets() {
        let intent = CatalogProgressUiIntent::from_worker_progress(
            CatalogWorkerUiContext {
                catalog_ready: false,
                screen: Screen::Home,
                foreground_update: false,
            },
            "Classifying library".to_string(),
            "Games found: 250".to_string(),
            45,
        );

        assert_eq!(
            intent.counter_target,
            Some(CatalogCounterTarget {
                phase: CatalogCounterPhase::FullScan,
                target: 250
            })
        );
    }

    #[test]
    fn media_progress_display_tracks_active_rows_and_summary() {
        let mut display = MediaProgressDisplay::default();

        let _ =
            display.progress_intent(&media_progress_event("arcade", "download", 512, 1024, 1, 2));
        let _ =
            display.progress_intent(&media_progress_event("neogeo", "download", 128, 1024, 2, 2));

        assert_eq!(display.active.len(), 2);
        assert_eq!(display.downloading.len(), 2);
        assert_eq!(display.active["arcade"].percent, 50);
        assert_eq!(display.active["arcade"].phase, "download");
        assert_eq!(display.active["arcade"].bytes_label, "");
        assert_eq!(display.active["neogeo"].percent, 12);
        assert_eq!(display.active["neogeo"].phase, "download");
        assert_eq!(display.summary(), "screenshots 2 active · 0/2 done");
    }

    #[test]
    fn media_progress_model_uses_launch_tile_system_titles() {
        for (system, expected_title) in [
            ("neogeo", "NeoGeo"),
            ("n64", "Nintendo 64"),
            ("sms", "Sega Master System"),
            ("megadrive", "Mega Drive"),
            ("atarilynx", "Atari Lynx"),
        ] {
            let mut display = MediaProgressDisplay::default();
            let _ =
                display.progress_intent(&media_progress_event(system, "download", 128, 1024, 1, 1));

            let row = display.model().row_data(0).expect("progress row");
            assert_eq!(row.system.as_str(), expected_title, "{system}");
        }
    }

    #[test]
    fn media_progress_display_removes_terminal_rows() {
        let mut display = MediaProgressDisplay::default();
        let _ =
            display.progress_intent(&media_progress_event("arcade", "download", 128, 1024, 1, 2));
        let _ =
            display.progress_intent(&media_progress_event("neogeo", "download", 128, 1024, 2, 2));

        let _ = display.progress_intent(&media_progress_event(
            "arcade",
            "download_done",
            1024,
            1024,
            1,
            2,
        ));
        let _ = display.progress_intent(&media_progress_event("neogeo", "failed", 128, 1024, 2, 2));

        assert_eq!(display.active.len(), 2);
        assert!(display.downloading.is_empty());
        assert!(display.done.contains("arcade"));
        assert!(display.failed.contains("neogeo"));
        assert_eq!(display.active["arcade"].phase, "download");
        assert_eq!(display.active["arcade"].percent, 100);
        assert_eq!(display.active["neogeo"].phase, "failed");
        assert_eq!(
            display.summary(),
            "screenshots 0 active · 1/2 done · 1 failed"
        );
    }

    #[test]
    fn media_progress_display_reports_visible_row_truth() {
        let mut display = MediaProgressDisplay::default();
        for (idx, system) in ["arcade", "megadrive", "n64", "neogeo"]
            .into_iter()
            .enumerate()
        {
            let _ = display.progress_intent(&media_progress_event(
                system,
                "download",
                128,
                1024,
                idx + 1,
                4,
            ));
        }

        let arcade = display.visibility_log_detail("arcade", true, false);
        assert!(arcade.contains("system=arcade"));
        assert!(arcade.contains("row_seen=1"));
        assert!(arcade.contains("rendered=1"));
        assert!(arcade.contains("visible_systems=arcade,megadrive,n64"));

        let neogeo = display.visibility_log_detail("neogeo", true, false);
        assert!(neogeo.contains("system=neogeo"));
        assert!(neogeo.contains("row_seen=0"));
        assert!(neogeo.contains("rendered=0"));
        assert!(neogeo.contains("row_index=-1"));
    }

    #[test]
    fn media_progress_display_reports_standalone_rendered_rows() {
        let mut display = MediaProgressDisplay::default();
        let _ =
            display.progress_intent(&media_progress_event("neogeo", "download", 128, 1024, 1, 1));

        let detail = display.visibility_log_detail("neogeo", false, true);
        assert!(detail.contains("row_seen=1"));
        assert!(detail.contains("rendered=1"));
        assert!(detail.contains("catalog_scan_visible=0"));
        assert!(detail.contains("standalone_visible=1"));

        let _ = display.progress_intent(&media_progress_event(
            "neogeo",
            "download_done",
            1024,
            1024,
            1,
            1,
        ));
        assert!(display.all_requested_terminal());
    }

    #[test]
    fn media_progress_percent_uses_download_for_full_streamed_pack_flow() {
        assert_eq!(media_progress_percent("download", 512, 1024), 50);
        assert_eq!(media_progress_percent("download_done", 1024, 1024), 100);
        assert_eq!(media_progress_percent("verify", 1024, 1024), 100);
        assert_eq!(media_progress_percent("save", 512, 1024), 100);
        assert_eq!(media_progress_percent("sync", 1024, 1024), 100);
        assert_eq!(media_progress_percent("done", 1024, 1024), 100);
    }

    #[test]
    fn media_progress_display_does_not_reset_for_index_sidecar_after_pack_download() {
        let mut display = MediaProgressDisplay::default();

        let _ = display.progress_intent(&media_progress_variant_event(
            "arcade", "identity", "download", 512, 1024, 1, 1,
        ));
        assert_eq!(display.active["arcade"].percent, 50);

        let _ = display.progress_intent(&media_progress_variant_event(
            "arcade",
            "identity",
            "download_done",
            1024,
            1024,
            1,
            1,
        ));
        assert_eq!(display.active["arcade"].percent, 100);

        let _ = display.progress_intent(&media_progress_variant_event(
            "arcade",
            "index",
            "download_start",
            0,
            1024,
            1,
            1,
        ));
        assert_eq!(display.active["arcade"].percent, 100);

        let _ = display.progress_intent(&media_progress_variant_event(
            "arcade", "index", "download", 64, 128, 1, 1,
        ));
        assert_eq!(display.active["arcade"].percent, 100);
    }
}
