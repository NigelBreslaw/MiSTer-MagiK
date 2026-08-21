// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Catalog build orchestration and progress events.

use crate::arcade_catalog;
use crate::catalog_progress::{CatalogProgress, report_catalog_progress};
use crate::catalog_stamp;
use crate::game_discovery::{
    covered_payload_paths, preferred_playable_discovery_indices_by_key, unique_discovery_count,
};
use crate::library_db::{
    BenchConfig, LibraryCatalogLoad, LibraryRamScanArtifact, LibraryRefreshCatalog,
    LibraryRefreshSummary, LibraryScan, LibraryScanArtifact, LibraryScanStats, ProgressCallback,
    ScanEventCallback,
};
use crate::library_indexer::LibraryIndexer;
use crate::sqlite_catalog;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

pub(crate) struct CatalogRefreshPipeline<'a> {
    cfg: &'a BenchConfig,
    archive_reader: crate::catalog_config::ArchiveReaderConfig,
    sqlite_build_dir_override: Option<PathBuf>,
    arcade_updater_index: Option<PathBuf>,
    enforce_arcade_rom_presence: bool,
}

impl<'a> CatalogRefreshPipeline<'a> {
    pub(crate) fn new(cfg: &'a BenchConfig) -> Self {
        Self {
            cfg,
            archive_reader: crate::catalog_config::ArchiveReaderConfig::default(),
            sqlite_build_dir_override: None,
            arcade_updater_index: None,
            enforce_arcade_rom_presence: true,
        }
    }

    pub(crate) fn with_archive_cache(
        cfg: &'a BenchConfig,
        archive_cache: &crate::catalog_config::ArchiveCacheConfig,
    ) -> Self {
        Self {
            cfg,
            archive_reader: archive_cache.archive_reader_config().clone(),
            sqlite_build_dir_override: archive_cache
                .sqlite_build_dir_override()
                .map(Path::to_path_buf),
            arcade_updater_index: None,
            enforce_arcade_rom_presence: true,
        }
    }

    #[cfg(feature = "builder")]
    pub(crate) fn with_arcade_updater_index(mut self, path: &Path) -> Self {
        self.arcade_updater_index = Some(path.to_path_buf());
        self
    }

    #[cfg(feature = "builder")]
    pub(crate) fn with_arcade_rom_presence(mut self, enforce: bool) -> Self {
        self.enforce_arcade_rom_presence = enforce;
        self
    }

    fn configure_arcade_updater_index(&self, indexer: LibraryIndexer<'a>) -> LibraryIndexer<'a> {
        let indexer = match self.arcade_updater_index.as_deref() {
            Some(path) => indexer.with_arcade_updater_index(path),
            None => indexer,
        };
        indexer.with_arcade_rom_presence(self.enforce_arcade_rom_presence)
    }

    pub(crate) fn rebuild_with_events(
        &self,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> Result<LibraryRefreshSummary, String> {
        self.rebuild_with_catalog(arcade_catalog::DEFAULT_ARCADE_ROOT, progress, scan_events)
            .map(|refresh| refresh.summary)
    }

    pub(crate) fn rebuild_with_catalog(
        &self,
        root: impl AsRef<Path>,
        mut progress: ProgressCallback<'_>,
        mut scan_events: ScanEventCallback<'_>,
    ) -> Result<LibraryRefreshCatalog, String> {
        let scan_t = Instant::now();
        report_catalog_progress(&mut progress, CatalogProgress::indexing_full_build());
        let artifact = match (progress.as_mut(), scan_events.as_mut()) {
            (Some(report), Some(events)) => {
                self.scan_artifact_with_events(Some(&mut **report), Some(&mut **events))
            }
            (Some(report), None) => self.scan_artifact_with_events(Some(&mut **report), None),
            (None, Some(events)) => self.scan_artifact_with_events(None, Some(&mut **events)),
            (None, None) => self.scan_artifact_with_events(None, None),
        };
        let scan_us = scan_t.elapsed().as_micros() as u64;
        report_catalog_progress(
            &mut progress,
            CatalogProgress::indexing_write_summary(
                artifact.stats.discoveries,
                artifact.stats.containers,
            ),
        );
        let mut refresh = self.save_artifact_with_catalog(artifact, root, progress)?;
        refresh.summary.scan_us = scan_us;
        Ok(refresh)
    }

    pub(crate) fn scan_artifact(&self, progress: ProgressCallback<'_>) -> LibraryScanArtifact {
        self.scan_artifact_with_events(progress, None)
    }

    pub(crate) fn scan_artifact_with_events(
        &self,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryScanArtifact {
        self.scan_artifact_with_events_using(
            LibraryIndexer::with_archive_reader(self.cfg, self.archive_reader.clone()),
            progress,
            scan_events,
        )
    }

    pub(crate) fn scan_artifact_foreground_with_events(
        &self,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryScanArtifact {
        self.scan_artifact_with_events_using(
            LibraryIndexer::foreground_with_archive_reader(self.cfg, self.archive_reader.clone()),
            progress,
            scan_events,
        )
    }

    pub(crate) fn scan_ram_artifact_foreground_with_events(
        &self,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryRamScanArtifact {
        self.scan_ram_artifact_with_events_using(
            self.configure_arcade_updater_index(LibraryIndexer::foreground_with_archive_reader(
                self.cfg,
                self.archive_reader.clone(),
            )),
            progress,
            scan_events,
        )
    }

    pub(crate) fn scan_ram_artifact_foreground_with_events_and_durable_resume(
        &self,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
        durable_resume: bool,
    ) -> LibraryRamScanArtifact {
        self.scan_ram_artifact_with_events_using(
            LibraryIndexer::foreground_with_archive_reader(self.cfg, self.archive_reader.clone())
                .with_durable_resume(durable_resume),
            progress,
            scan_events,
        )
    }

    pub(crate) fn scan_ram_artifact_with_events_and_durable_resume(
        &self,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
        durable_resume: bool,
    ) -> LibraryRamScanArtifact {
        self.scan_ram_artifact_with_events_using(
            self.configure_arcade_updater_index(
                LibraryIndexer::with_archive_reader(self.cfg, self.archive_reader.clone())
                    .with_durable_resume(durable_resume),
            ),
            progress,
            scan_events,
        )
    }

    pub(crate) fn scan_ram_artifact_with_reused_prefix(
        &self,
        reused: LibraryRamScanArtifact,
        excluded_targets: Vec<PathBuf>,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryRamScanArtifact {
        self.scan_ram_artifact_with_reused_prefix_using(
            LibraryIndexer::with_archive_reader(self.cfg, self.archive_reader.clone()),
            reused,
            excluded_targets,
            progress,
            scan_events,
        )
    }

    pub(crate) fn scan_ram_artifact_foreground_with_reused_prefix(
        &self,
        reused: LibraryRamScanArtifact,
        excluded_targets: Vec<PathBuf>,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryRamScanArtifact {
        self.scan_ram_artifact_with_reused_prefix_using(
            LibraryIndexer::foreground_with_archive_reader(self.cfg, self.archive_reader.clone()),
            reused,
            excluded_targets,
            progress,
            scan_events,
        )
    }

    fn scan_ram_artifact_with_reused_prefix_using(
        &self,
        indexer: LibraryIndexer<'_>,
        reused: LibraryRamScanArtifact,
        excluded_targets: Vec<PathBuf>,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryRamScanArtifact {
        let scan_t = Instant::now();
        let mut scan = indexer.scan_without_coverage_audit_with_progress_events_and_exclusions(
            progress,
            scan_events,
            excluded_targets,
        );
        let reused_scan_us = reused.stats.scan_us;
        merge_reused_scan(&mut scan, reused.scan);
        let covered_payloads = covered_payload_paths(&scan.discoveries);
        let preferred_discoveries =
            preferred_playable_discovery_indices_by_key(&scan.discoveries, &covered_payloads);
        let mut stats = scan_stats_with_discovery_count(&scan, scan_t, preferred_discoveries.len());
        stats.scan_us = stats.scan_us.saturating_add(reused_scan_us);
        crate::library_db::apply_library_path_map_to_ram_artifact(LibraryRamScanArtifact {
            scan,
            stats,
            preferred_discoveries,
        })
    }

    fn scan_ram_artifact_with_events_using(
        &self,
        indexer: LibraryIndexer<'_>,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryRamScanArtifact {
        let scan_t = Instant::now();
        let scan =
            indexer.scan_without_coverage_audit_with_progress_and_events(progress, scan_events);
        let covered_payloads = covered_payload_paths(&scan.discoveries);
        let preferred_discoveries =
            preferred_playable_discovery_indices_by_key(&scan.discoveries, &covered_payloads);
        let stats = scan_stats_with_discovery_count(&scan, scan_t, preferred_discoveries.len());
        crate::library_db::apply_library_path_map_to_ram_artifact(LibraryRamScanArtifact {
            scan,
            stats,
            preferred_discoveries,
        })
    }

    fn scan_artifact_with_events_using(
        &self,
        indexer: LibraryIndexer<'_>,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryScanArtifact {
        let scan_t = Instant::now();
        let scan = indexer.scan_with_progress_and_events(progress, scan_events);
        let stamp = catalog_stamp::compute_default_catalog_stamp_with_audit(
            &self.cfg.roots,
            &scan.audit_rows,
        );
        let stats = scan_stats(&scan, scan_t);
        let artifact = LibraryScanArtifact { scan, stats, stamp };
        crate::library_db::apply_library_path_map(artifact)
    }

    pub(crate) fn save_artifact(
        &self,
        artifact: LibraryScanArtifact,
        progress: ProgressCallback<'_>,
    ) -> Result<LibraryRefreshSummary, String> {
        self.save_artifact_with_catalog_and_bench_iteration(
            artifact,
            arcade_catalog::DEFAULT_ARCADE_ROOT,
            progress,
            None,
        )
        .map(|refresh| refresh.summary)
    }

    pub(crate) fn save_artifact_for_bench(
        &self,
        artifact: LibraryScanArtifact,
        progress: ProgressCallback<'_>,
        iteration: usize,
    ) -> Result<LibraryRefreshSummary, String> {
        self.save_artifact_with_catalog_and_bench_iteration(
            artifact,
            arcade_catalog::DEFAULT_ARCADE_ROOT,
            progress,
            Some(iteration),
        )
        .map(|refresh| refresh.summary)
    }

    pub(crate) fn save_artifact_with_catalog(
        &self,
        artifact: LibraryScanArtifact,
        root: impl AsRef<Path>,
        progress: ProgressCallback<'_>,
    ) -> Result<LibraryRefreshCatalog, String> {
        self.save_artifact_with_catalog_and_bench_iteration(artifact, root, progress, None)
    }

    fn save_artifact_with_catalog_and_bench_iteration(
        &self,
        artifact: LibraryScanArtifact,
        root: impl AsRef<Path>,
        progress: ProgressCallback<'_>,
        bench_iteration: Option<usize>,
    ) -> Result<LibraryRefreshCatalog, String> {
        let root = root.as_ref();
        let import_t = Instant::now();
        let catalog_t = Instant::now();
        let catalog = artifact.catalog(root);
        let catalog_us = catalog_t.elapsed().as_micros() as u64;
        sqlite_catalog::report_library_import_timing(
            "precompute_catalog",
            catalog_t,
            format!("rows={}", catalog.len()),
        );
        let build_plan = sqlite_catalog::sqlite_build_temp_plan_for(
            &self.cfg.sqlite_path,
            self.sqlite_build_dir_override.as_deref(),
        );
        let bytes = sqlite_catalog::save_sqlite_scan_with_progress_and_stamp_and_projections_with_bench_iteration(
                &self.cfg.sqlite_path,
                &artifact.scan,
                artifact.stamp(),
                root,
                progress,
                bench_iteration,
                build_plan,
            )?;
        let import_us = import_t.elapsed().as_micros() as u64;
        Ok(LibraryRefreshCatalog {
            summary: LibraryRefreshSummary {
                skipped: false,
                scan_us: artifact.stats.scan_us,
                discover_us: artifact.stats.discover_us,
                classify_us: artifact.stats.classify_us,
                import_us,
                bytes,
                normal_files: artifact.stats.normal_files,
                containers: artifact.stats.containers,
                entries: artifact.stats.entries,
                audit_rows: artifact.stats.audit_rows,
                discoveries: artifact.stats.discoveries,
            },
            catalog: LibraryCatalogLoad::from_precomputed(catalog, catalog_us),
        })
    }
}

fn merge_reused_scan(scan: &mut LibraryScan, mut reused: LibraryScan) {
    reused.normal_files.append(&mut scan.normal_files);
    reused.containers.append(&mut scan.containers);
    reused.entries.append(&mut scan.entries);
    reused.discoveries.append(&mut scan.discoveries);
    scan.normal_files = reused.normal_files;
    scan.containers = reused.containers;
    scan.entries = reused.entries;
    scan.discoveries = reused.discoveries;
    scan.ignored_files = scan.ignored_files.saturating_add(reused.ignored_files);
    scan.discover_us = scan.discover_us.saturating_add(reused.discover_us);
    scan.classify_us = scan.classify_us.saturating_add(reused.classify_us);
}

fn scan_stats(scan: &LibraryScan, scan_t: Instant) -> LibraryScanStats {
    scan_stats_with_discovery_count(scan, scan_t, unique_discovery_count(&scan.discoveries))
}

fn scan_stats_with_discovery_count(
    scan: &LibraryScan,
    scan_t: Instant,
    discoveries: usize,
) -> LibraryScanStats {
    LibraryScanStats {
        scan_us: scan_t.elapsed().as_micros() as u64,
        discover_us: scan.discover_us,
        classify_us: scan.classify_us,
        normal_files: scan.normal_files.len(),
        containers: scan.containers.len(),
        entries: scan.entries.len(),
        audit_rows: scan.audit_rows.len(),
        discoveries,
    }
}
