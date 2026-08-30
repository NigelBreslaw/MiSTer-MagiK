// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structured catalog progress phases and the legacy title/detail adapter.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};

const INNER_PROGRESS_BATCH: u64 = 4096;
static INNER_PROGRESS_UNITS: AtomicU64 = AtomicU64::new(0);

pub(crate) type ProgressCallback<'a> = Option<&'a mut dyn FnMut(&str, &str)>;
pub const CATALOG_SAFETY_LIMIT_NONRETRYABLE: &str = "catalog-safety-limit-nonretryable";

/// Record that a bounded batch of catalog work completed. The counter is
/// intentionally process-wide and monotonic so a supervising worker can poll
/// it without adding a callback or allocation to hot traversal loops.
pub(crate) fn report_inner_progress() {
    INNER_PROGRESS_UNITS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn report_inner_progress_at(count: usize) {
    if count.is_multiple_of(INNER_PROGRESS_BATCH as usize) {
        report_inner_progress();
    }
}

/// Return the monotonic inner-work counter for a supervising worker.
pub fn inner_progress_units() -> u64 {
    INNER_PROGRESS_UNITS.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogProgressPhase {
    FindingGames,
    ClassifyingLibrary,
    IndexingLibrary,
    SavingLibrary,
    LoadingLibrary,
    LibraryScanFailed,
    LibraryLoadFailed,
    Other,
}

impl CatalogProgressPhase {
    pub fn display_title(self) -> &'static str {
        match self {
            Self::FindingGames => "Finding games",
            Self::ClassifyingLibrary => "Classifying library",
            Self::IndexingLibrary => "Indexing library",
            Self::SavingLibrary => "Saving library",
            Self::LoadingLibrary => "Loading library",
            Self::LibraryScanFailed => "Library scan failed",
            Self::LibraryLoadFailed => "Library load failed",
            Self::Other => "",
        }
    }

    pub fn from_display_title(title: &str) -> Self {
        match title {
            "Finding games" => Self::FindingGames,
            "Classifying library" => Self::ClassifyingLibrary,
            "Indexing library" => Self::IndexingLibrary,
            "Saving library" => Self::SavingLibrary,
            "Loading library" => Self::LoadingLibrary,
            "Library scan failed" => Self::LibraryScanFailed,
            "Library load failed" => Self::LibraryLoadFailed,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogProgress {
    phase: CatalogProgressPhase,
    detail: CatalogProgressDetail,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CatalogProgressDetail {
    Static(&'static str),
    Owned(String),
    GamesFound(usize),
    IndexSummary { games: usize, archives: usize },
    SqliteImport { written: usize, total: usize },
    SqlitePublish { done: u64, total: u64 },
}

impl CatalogProgress {
    pub fn finding_games_found(count: usize) -> Self {
        Self {
            phase: CatalogProgressPhase::FindingGames,
            detail: CatalogProgressDetail::GamesFound(count),
        }
    }

    pub fn classifying_games_found(count: usize) -> Self {
        Self {
            phase: CatalogProgressPhase::ClassifyingLibrary,
            detail: CatalogProgressDetail::GamesFound(count),
        }
    }

    pub fn indexing_building_catalog() -> Self {
        Self {
            phase: CatalogProgressPhase::IndexingLibrary,
            detail: CatalogProgressDetail::Static("Building catalog..."),
        }
    }

    pub fn indexing_full_build() -> Self {
        Self {
            phase: CatalogProgressPhase::IndexingLibrary,
            detail: CatalogProgressDetail::Static("Full catalog build..."),
        }
    }

    pub fn indexing_write_summary(games: usize, archives: usize) -> Self {
        Self {
            phase: CatalogProgressPhase::IndexingLibrary,
            detail: CatalogProgressDetail::IndexSummary { games, archives },
        }
    }

    pub fn saving_before_opening_launcher() -> Self {
        Self {
            phase: CatalogProgressPhase::SavingLibrary,
            detail: CatalogProgressDetail::Static(
                "Writing catalog database before opening launcher...",
            ),
        }
    }

    pub fn saving_sqlite_import(written: usize, total: usize) -> Self {
        Self {
            phase: CatalogProgressPhase::SavingLibrary,
            detail: CatalogProgressDetail::SqliteImport { written, total },
        }
    }

    pub fn saving_sqlite_publish(done: u64, total: u64) -> Self {
        Self {
            phase: CatalogProgressPhase::SavingLibrary,
            detail: CatalogProgressDetail::SqlitePublish { done, total },
        }
    }

    pub fn saving_finalizing() -> Self {
        Self {
            phase: CatalogProgressPhase::SavingLibrary,
            detail: CatalogProgressDetail::Static("Finalizing catalog views and search indexes..."),
        }
    }

    pub fn loading_sqlite_catalog() -> Self {
        Self {
            phase: CatalogProgressPhase::LoadingLibrary,
            detail: CatalogProgressDetail::Static("Opening SQLite catalog..."),
        }
    }

    pub fn library_scan_failed(error: impl Into<String>) -> Self {
        Self {
            phase: CatalogProgressPhase::LibraryScanFailed,
            detail: CatalogProgressDetail::Owned(error.into()),
        }
    }

    pub fn library_load_failed(error: impl Into<String>) -> Self {
        Self {
            phase: CatalogProgressPhase::LibraryLoadFailed,
            detail: CatalogProgressDetail::Owned(error.into()),
        }
    }

    pub fn phase(&self) -> CatalogProgressPhase {
        self.phase
    }

    pub fn display(&self) -> CatalogProgressDisplay<'_> {
        let detail = match &self.detail {
            CatalogProgressDetail::Static(detail) => Cow::Borrowed(*detail),
            CatalogProgressDetail::Owned(detail) => Cow::Borrowed(detail.as_str()),
            CatalogProgressDetail::GamesFound(count) => Cow::Owned(format!("Games found: {count}")),
            CatalogProgressDetail::IndexSummary { games, archives } => {
                Cow::Owned(format!("Writing {games} games, {archives} archives..."))
            }
            CatalogProgressDetail::SqliteImport { written, total } => {
                Cow::Owned(format!("Writing {written} of {total} games into SQLite..."))
            }
            CatalogProgressDetail::SqlitePublish { done, total } => {
                Cow::Owned(format!("Saving {done} of {total} bytes to disk..."))
            }
        };
        CatalogProgressDisplay {
            phase: self.phase,
            title: self.phase.display_title(),
            detail,
        }
    }
}

pub struct CatalogProgressDisplay<'a> {
    phase: CatalogProgressPhase,
    title: &'static str,
    detail: Cow<'a, str>,
}

impl CatalogProgressDisplay<'_> {
    pub fn phase(&self) -> CatalogProgressPhase {
        self.phase
    }

    pub fn title(&self) -> &'static str {
        self.title
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub fn percent(&self) -> i32 {
        catalog_progress_percent(self.phase, self.detail())
    }
}

pub(crate) fn report_catalog_progress(progress: &mut ProgressCallback<'_>, event: CatalogProgress) {
    let display = event.display();
    if let Some(report) = progress.as_mut() {
        report(display.title(), display.detail());
    }
}

pub fn catalog_progress_percent_from_display(title: &str, detail: &str) -> i32 {
    catalog_progress_percent(CatalogProgressPhase::from_display_title(title), detail)
}

fn catalog_progress_percent(phase: CatalogProgressPhase, detail: &str) -> i32 {
    match phase {
        CatalogProgressPhase::LoadingLibrary => 100,
        CatalogProgressPhase::SavingLibrary => {
            if let Some(percent) = sqlite_save_percent(detail) {
                return percent;
            }
            if let Some(percent) = sqlite_import_percent(detail) {
                return percent;
            }
            if detail.starts_with("Finalizing ") {
                return 99;
            }
            90
        }
        CatalogProgressPhase::IndexingLibrary if detail.starts_with("Writing ") => 90,
        _ => -1,
    }
}

fn sqlite_import_percent(detail: &str) -> Option<i32> {
    let rest = detail.strip_prefix("Writing ")?;
    let mut parts = rest.split_whitespace();
    let written = parts.next()?.parse::<usize>().ok()?;
    if parts.next()? != "of" {
        return None;
    }
    let total = parts.next()?.parse::<usize>().ok()?;
    if total == 0 {
        return Some(90);
    }
    let percent = 90 + (written.min(total) * 9 / total) as i32;
    Some(percent.clamp(90, 99))
}

fn sqlite_save_percent(detail: &str) -> Option<i32> {
    let rest = detail.strip_prefix("Saving ")?;
    let mut parts = rest.split_whitespace();
    let written = parts.next()?.parse::<u64>().ok()?;
    if parts.next()? != "of" {
        return None;
    }
    let total = parts.next()?.parse::<u64>().ok()?;
    if parts.next()? != "bytes" {
        return None;
    }
    if total == 0 {
        return Some(100);
    }
    Some(((written.min(total) * 100) / total) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_progress_display_preserves_existing_text() {
        let progress = CatalogProgress::indexing_write_summary(10, 2);
        let display = progress.display();
        assert_eq!(display.phase(), CatalogProgressPhase::IndexingLibrary);
        assert_eq!(display.title(), "Indexing library");
        assert_eq!(display.detail(), "Writing 10 games, 2 archives...");
        assert_eq!(display.percent(), 90);

        let progress = CatalogProgress::saving_sqlite_import(50, 100);
        let display = progress.display();
        assert_eq!(display.title(), "Saving library");
        assert_eq!(display.detail(), "Writing 50 of 100 games into SQLite...");
        assert_eq!(display.percent(), 94);
    }

    #[test]
    fn catalog_progress_percent_tracks_sqlite_publish_progress() {
        assert_eq!(
            catalog_progress_percent_from_display(
                "Saving library",
                "Saving 0 of 1000 bytes to disk..."
            ),
            0
        );
        assert_eq!(
            catalog_progress_percent_from_display(
                "Saving library",
                "Saving 500 of 1000 bytes to disk..."
            ),
            50
        );
        assert_eq!(
            catalog_progress_percent_from_display(
                "Saving library",
                "Saving 1200 of 1000 bytes to disk..."
            ),
            100
        );
    }

    #[test]
    fn catalog_progress_display_covers_all_structured_phases() {
        let cases = [
            (
                CatalogProgress::classifying_games_found(7),
                CatalogProgressPhase::ClassifyingLibrary,
                "Classifying library",
                "Games found: 7",
                -1,
            ),
            (
                CatalogProgress::indexing_building_catalog(),
                CatalogProgressPhase::IndexingLibrary,
                "Indexing library",
                "Building catalog...",
                -1,
            ),
            (
                CatalogProgress::indexing_full_build(),
                CatalogProgressPhase::IndexingLibrary,
                "Indexing library",
                "Full catalog build...",
                -1,
            ),
            (
                CatalogProgress::saving_before_opening_launcher(),
                CatalogProgressPhase::SavingLibrary,
                "Saving library",
                "Writing catalog database before opening launcher...",
                90,
            ),
            (
                CatalogProgress::saving_finalizing(),
                CatalogProgressPhase::SavingLibrary,
                "Saving library",
                "Finalizing catalog views and search indexes...",
                99,
            ),
            (
                CatalogProgress::loading_sqlite_catalog(),
                CatalogProgressPhase::LoadingLibrary,
                "Loading library",
                "Opening SQLite catalog...",
                100,
            ),
            (
                CatalogProgress::library_scan_failed("scan failed"),
                CatalogProgressPhase::LibraryScanFailed,
                "Library scan failed",
                "scan failed",
                -1,
            ),
            (
                CatalogProgress::library_load_failed("load failed"),
                CatalogProgressPhase::LibraryLoadFailed,
                "Library load failed",
                "load failed",
                -1,
            ),
        ];

        for (progress, phase, title, detail, percent) in cases {
            let display = progress.display();
            assert_eq!(display.phase(), phase);
            assert_eq!(display.title(), title);
            assert_eq!(display.detail(), detail);
            assert_eq!(display.percent(), percent);
        }
    }

    #[test]
    fn catalog_progress_percent_rejects_malformed_legacy_details() {
        assert_eq!(
            catalog_progress_percent_from_display("Saving library", "Writing nope of 10 games"),
            90
        );
        assert_eq!(
            catalog_progress_percent_from_display("Saving library", "Writing 5 from 10 games"),
            90
        );
        assert_eq!(
            catalog_progress_percent_from_display("Saving library", "Writing 5 of 0 games"),
            90
        );
        assert_eq!(
            catalog_progress_percent_from_display("Saving library", "Saving 5 from 10 bytes"),
            90
        );
        assert_eq!(
            catalog_progress_percent_from_display("Saving library", "Saving 5 of 0 bytes"),
            100
        );
        assert_eq!(
            catalog_progress_percent_from_display("Mystery phase", "Saving 5 of 10 bytes"),
            -1
        );
        assert_eq!(
            catalog_progress_percent_from_display(
                "Indexing library",
                "Resolving playable games — 250 of 500 (still working: 3s)"
            ),
            -1
        );
    }

    #[test]
    fn legacy_callback_adapter_emits_display_text() {
        let mut messages = Vec::<(String, String)>::new();
        let mut callback = |title: &str, detail: &str| {
            messages.push((title.to_string(), detail.to_string()));
        };
        let mut progress: ProgressCallback<'_> = Some(&mut callback);

        report_catalog_progress(&mut progress, CatalogProgress::finding_games_found(50));

        assert_eq!(
            messages,
            vec![("Finding games".to_string(), "Games found: 50".to_string())]
        );
    }

    #[test]
    fn inner_progress_reports_only_completed_batches_and_is_monotonic() {
        let before = inner_progress_units();
        report_inner_progress_at(4095);
        assert!(inner_progress_units() >= before);
        report_inner_progress_at(4096);
        let after_first = inner_progress_units();
        assert!(after_first > before);
        report_inner_progress_at(8192);
        assert!(inner_progress_units() > after_first);
    }
}
