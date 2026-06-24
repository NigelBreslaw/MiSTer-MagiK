use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) enum LauncherWorkerUiIntent {
    None,
    CatalogScan(CatalogScanBridgeStatus),
    ClearCatalogScan,
    HideCatalogBackgroundScan,
    MediaProgress {
        progresses: ModelRc<slint_ui::launcher::ScreenshotPackProgress>,
        summary: String,
    },
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
        LauncherWorkerUiIntent::HideCatalogBackgroundScan => {
            status_presenter.sync_catalog_background_scan_visible(false);
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
        true,
        false,
        catalog_scan_message(foreground_update),
        "Indexing library",
        "Rebuilding catalog with latest games...",
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

    fn apply(&mut self, event: &MediaProgressEvent) -> bool {
        if event.system == "all" {
            return false;
        }
        if event.pack_count > 0 {
            self.requested_count = self.requested_count.max(event.pack_count);
        }
        if event.phase == "failed" {
            self.failed.insert(event.system.clone());
            self.active
                .insert(event.system.clone(), media_progress_display_row(event));
            return true;
        }
        if media_progress_terminal_phase(&event.phase) {
            self.done.insert(event.system.clone());
            self.active
                .insert(event.system.clone(), media_progress_display_row(event));
            return true;
        }
        self.active
            .insert(event.system.clone(), media_progress_display_row(event));
        true
    }

    fn clear(&mut self) {
        self.active.clear();
        self.done.clear();
        self.failed.clear();
        self.requested_count = 0;
    }

    fn model(&self) -> ModelRc<slint_ui::launcher::ScreenshotPackProgress> {
        let rows = self
            .active
            .values()
            .take(3)
            .map(|row| slint_ui::launcher::ScreenshotPackProgress {
                system: row.system.clone().into(),
                image_size: row.image_size.clone().into(),
                phase: row.phase.clone().into(),
                percent: row.percent,
                bytes_label: row.bytes_label.clone().into(),
                pack_position: row.pack_position.clone().into(),
            })
            .collect::<Vec<_>>();
        ModelRc::new(VecModel::from(rows))
    }

    pub(super) fn summary(&self) -> String {
        let active = self
            .active
            .keys()
            .filter(|system| !self.done.contains(*system) && !self.failed.contains(*system))
            .count();
        let done = self.done.len();
        let failed = self.failed.len();
        let total = self.requested_count.max(active + done + failed);
        if total == 0 {
            return String::new();
        }
        if failed > 0 {
            format!("screenshots {active} active · {done}/{total} done · {failed} failed")
        } else {
            format!("screenshots {active} active · {done}/{total} done")
        }
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

fn media_progress_terminal_phase(phase: &str) -> bool {
    matches!(phase, "done" | "skipped-current" | "check-only")
}

fn media_progress_percent(phase: &str, done: u64, total: u64) -> i32 {
    match phase {
        "check" | "download_start" => 0,
        "download" => scaled_progress(done, total, 0, 50),
        "download_done" => 50,
        "verify" => 60,
        "save" => scaled_progress(done, total, 60, 40),
        "sync" | "rename" | "parent-sync" | "done" | "skipped-current" | "check-only" => 100,
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
        let _ = display.progress_intent(&media_progress_event("neogeo", "save", 128, 1024, 2, 2));

        assert_eq!(display.active.len(), 2);
        assert_eq!(display.active["arcade"].percent, 25);
        assert_eq!(display.active["arcade"].phase, "download");
        assert_eq!(display.active["arcade"].bytes_label, "");
        assert_eq!(display.active["neogeo"].percent, 65);
        assert_eq!(display.active["neogeo"].phase, "saving");
        assert_eq!(display.summary(), "screenshots 2 active · 0/2 done");
    }

    #[test]
    fn media_progress_display_removes_terminal_rows() {
        let mut display = MediaProgressDisplay::default();
        let _ =
            display.progress_intent(&media_progress_event("arcade", "download", 128, 1024, 1, 2));
        let _ =
            display.progress_intent(&media_progress_event("neogeo", "download", 128, 1024, 2, 2));

        let _ = display.progress_intent(&media_progress_event("arcade", "done", 1024, 1024, 1, 2));
        let _ = display.progress_intent(&media_progress_event("neogeo", "failed", 128, 1024, 2, 2));

        assert_eq!(display.active.len(), 2);
        assert!(display.done.contains("arcade"));
        assert!(display.failed.contains("neogeo"));
        assert_eq!(display.active["arcade"].phase, "downloaded");
        assert_eq!(display.active["arcade"].percent, 100);
        assert_eq!(display.active["neogeo"].phase, "failed");
        assert_eq!(
            display.summary(),
            "screenshots 0 active · 1/2 done · 1 failed"
        );
    }

    #[test]
    fn media_progress_percent_reserves_ranges_for_download_verify_and_save() {
        assert_eq!(media_progress_percent("download", 512, 1024), 25);
        assert_eq!(media_progress_percent("download_done", 1024, 1024), 50);
        assert_eq!(media_progress_percent("verify", 1024, 1024), 60);
        assert_eq!(media_progress_percent("save", 512, 1024), 80);
        assert_eq!(media_progress_percent("sync", 1024, 1024), 100);
        assert_eq!(media_progress_percent("done", 1024, 1024), 100);
    }
}
