//! Library filesystem indexing and classification.
//!
//! This module owns the full-scan product contract: walk configured roots,
//! classify launcher/payload/archive/listing candidates, emit progress/events,
//! and return a complete `LibraryScan`.

use crate::catalog_config::SCHEMA_VERSION;
use crate::catalog_progress::{report_catalog_progress, CatalogProgress};
use crate::catalog_scan::{self, DiscoveryEvent};
use crate::core_audit;
use crate::game_discovery::{
    catalog_system_id_for_discovery, discovery_from_profile_archive_entry,
    discovery_from_profile_file, GameDiscovery,
};
use crate::launch_profiles::{self, PayloadDisposition, PayloadRule, ProfilePathClass};
use crate::library_db::{
    self, ArchiveFormat, BenchConfig, LibraryBootstrapSummary, LibraryPayloadFile, LibraryScan,
    LibraryScanEvent, ProgressCallback, ScanEventCallback,
};
use crate::media_metadata;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

const SCAN_PROGRESS_CANDIDATE_BATCH: usize = 50;
const BOOTSTRAP_PROGRESS_BATCH: usize = 50;
const SCREENSHOT_PACK_SYSTEM_IDS: &[&str] = &[
    "arcade",
    "neogeo",
    "nes",
    "snes",
    "n64",
    "sms",
    "megadrive",
    "saturn",
];

pub(crate) struct LibraryIndexer<'a> {
    cfg: &'a BenchConfig,
    priority: LibraryScanPriority,
}

impl<'a> LibraryIndexer<'a> {
    pub(crate) fn new(cfg: &'a BenchConfig) -> Self {
        Self {
            cfg,
            priority: LibraryScanPriority::Background,
        }
    }

    pub(crate) fn foreground(cfg: &'a BenchConfig) -> Self {
        Self {
            cfg,
            priority: LibraryScanPriority::Foreground,
        }
    }

    #[cfg(test)]
    pub(crate) fn scan(&self) -> LibraryScan {
        self.scan_with_progress_and_events(None, None)
    }

    pub(crate) fn scan_with_progress_and_events(
        &self,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryScan {
        scan_library_with_progress_and_events(
            self.cfg,
            self.priority,
            CoverageAuditMode::Inline,
            progress,
            scan_events,
        )
    }

    pub(crate) fn scan_without_coverage_audit_with_progress_and_events(
        &self,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryScan {
        scan_library_with_progress_and_events(
            self.cfg,
            self.priority,
            CoverageAuditMode::Deferred,
            progress,
            scan_events,
        )
    }

    pub(crate) fn bootstrap_progress(
        &self,
        progress: ProgressCallback<'_>,
    ) -> LibraryBootstrapSummary {
        bootstrap_library_progress(self.cfg, progress)
    }
}

#[derive(Clone, Copy)]
enum LibraryScanPriority {
    Background,
    Foreground,
}

#[derive(Clone, Copy)]
enum CoverageAuditMode {
    Inline,
    Deferred,
}

#[derive(Default)]
struct ScanTimingStats {
    profile_match_us: u64,
    profile_match_count: usize,
    file_discovery_us: u64,
    file_discovery_count: usize,
    archive_toc_us: u64,
    archive_toc_count: usize,
    installed_collection_us: u64,
    installed_collection_count: usize,
    collection_listing_us: u64,
    collection_listing_count: usize,
}

fn scan_library_with_progress_and_events(
    cfg: &BenchConfig,
    priority: LibraryScanPriority,
    audit_mode: CoverageAuditMode,
    mut progress: ProgressCallback<'_>,
    mut scan_events: ScanEventCallback<'_>,
) -> LibraryScan {
    let discover_t = Instant::now();
    let rx = match priority {
        LibraryScanPriority::Background => {
            catalog_scan::discover_files_pipelined(cfg.roots.clone())
        }
        LibraryScanPriority::Foreground => {
            catalog_scan::discover_files_pipelined_foreground(cfg.roots.clone())
        }
    };
    let profiles = launch_profiles::active_profiles_for_roots(&cfg.roots);
    let mut discover_us = 0;

    let mut normal_files = Vec::new();
    let mut containers = Vec::new();
    let mut entries = Vec::new();
    let mut ignored_files = 0usize;
    let mut discoveries = Vec::new();
    let classify_t = Instant::now();
    let mut timing = ScanTimingStats::default();
    let mut idx = 0usize;
    let mut first_discovery_reported = false;
    let mut discovered_systems = BTreeSet::new();
    while let Ok(event) = rx.recv() {
        let f = match event {
            DiscoveryEvent::File(file) => file,
            DiscoveryEvent::Done {
                discover_us: us, ..
            } => {
                discover_us = us;
                break;
            }
        };
        if idx == 0 {
            library_db::report_library_scan_timing(
                "first_candidate",
                classify_t.elapsed().as_micros() as u64,
                format!("path={}", f.path.display()),
            );
        }
        idx += 1;
        let discoveries_before = discoveries.len();
        let profile_match_t = Instant::now();
        let profile_match = catalog_scan::classify_profile_path(&profiles, &f.path);
        timing.profile_match_us += profile_match_t.elapsed().as_micros() as u64;
        timing.profile_match_count += 1;
        match profile_match {
            Some((
                profile,
                ProfilePathClass::Payload {
                    rule:
                        payload_rule @ PayloadRule {
                            disposition: PayloadDisposition::Playable,
                            ..
                        },
                },
            )) => {
                if media_metadata::is_amigavision_save_media_path(&f.path) {
                    ignored_files += 1;
                    continue;
                }
                let installed_t = Instant::now();
                let installed =
                    media_metadata::installed_amigavision_discoveries_from_hdf(&f, profile);
                timing.installed_collection_us += installed_t.elapsed().as_micros() as u64;
                timing.installed_collection_count += 1;
                if let Some(installed) = installed {
                    ignored_files += 1;
                    discoveries.extend(installed);
                    continue;
                }
                let mut has_archive_entries = false;
                if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
                    let archive_t = Instant::now();
                    let scan = catalog_scan::scan_archive_toc(&f, format, profile);
                    timing.archive_toc_us += archive_t.elapsed().as_micros() as u64;
                    timing.archive_toc_count += 1;
                    has_archive_entries = !scan.entries.is_empty();
                    for entry in scan.entries {
                        discoveries.push(discovery_from_profile_archive_entry(
                            &entry,
                            profile,
                            &entry.rule,
                        ));
                        entries.push(entry);
                    }
                    containers.push(scan.container);
                }
                if has_archive_entries {
                    continue;
                }
                normal_files.push(LibraryPayloadFile {
                    path: f.path.display().to_string(),
                });
                let discovery_t = Instant::now();
                discoveries.push(discovery_from_profile_file(
                    &f,
                    profile,
                    &payload_rule,
                    &profiles,
                ));
                timing.file_discovery_us += discovery_t.elapsed().as_micros() as u64;
                timing.file_discovery_count += 1;
            }
            Some((
                _,
                ProfilePathClass::Payload {
                    rule:
                        PayloadRule {
                            disposition: PayloadDisposition::AttachedMedia,
                            ..
                        },
                },
            )) => {
                normal_files.push(LibraryPayloadFile {
                    path: f.path.display().to_string(),
                });
                ignored_files += 1;
            }
            Some((profile, ProfilePathClass::Collection { rule })) => {
                if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
                    containers.push(catalog_scan::scan_container_header(&f, format));
                }
                let collection_t = Instant::now();
                discoveries.extend(media_metadata::collection_discoveries_from_container(
                    &f, profile, &rule,
                ));
                timing.collection_listing_us += collection_t.elapsed().as_micros() as u64;
                timing.collection_listing_count += 1;
            }
            Some((_profile, ProfilePathClass::Ignored { .. })) => {
                ignored_files += 1;
            }
            Some((profile, ProfilePathClass::NotMatched))
                if catalog_scan::is_archive_entry_container_candidate(&profiles, &f.path) =>
            {
                if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
                    let archive_t = Instant::now();
                    let scan = catalog_scan::scan_archive_toc(&f, format, profile);
                    timing.archive_toc_us += archive_t.elapsed().as_micros() as u64;
                    timing.archive_toc_count += 1;
                    for entry in scan.entries {
                        discoveries.push(discovery_from_profile_archive_entry(
                            &entry,
                            profile,
                            &entry.rule,
                        ));
                        entries.push(entry);
                    }
                    containers.push(scan.container);
                }
            }
            Some((_, ProfilePathClass::NotMatched)) | None => {}
        }
        report_new_discovered_systems(
            &discoveries[discoveries_before..],
            &mut discovered_systems,
            &mut scan_events,
        );
        if discoveries.len() > discoveries_before && !first_discovery_reported {
            first_discovery_reported = true;
            library_db::report_library_scan_timing(
                "first_discovery",
                classify_t.elapsed().as_micros() as u64,
                format!(
                    "candidate={} discoveries={} path={}",
                    idx,
                    discoveries.len(),
                    f.path.display()
                ),
            );
        }
        if idx.is_multiple_of(SCAN_PROGRESS_CANDIDATE_BATCH) {
            report_catalog_progress(
                &mut progress,
                CatalogProgress::classifying_games_found(discoveries.len()),
            );
        }
    }
    if discover_us == 0 {
        discover_us = discover_t.elapsed().as_micros() as u64;
    }
    library_db::report_library_scan_timing("walk", discover_us, format!("candidates={idx}"));
    library_db::report_library_scan_timing(
        "profile_match",
        timing.profile_match_us,
        format!("calls={}", timing.profile_match_count),
    );
    library_db::report_library_scan_timing(
        "installed_collection",
        timing.installed_collection_us,
        format!("calls={}", timing.installed_collection_count),
    );
    library_db::report_library_scan_timing(
        "archive_toc",
        timing.archive_toc_us,
        format!("containers={}", timing.archive_toc_count),
    );
    library_db::report_library_scan_timing(
        "collection_listings",
        timing.collection_listing_us,
        format!("collections={}", timing.collection_listing_count),
    );
    library_db::report_library_scan_timing(
        "file_discovery",
        timing.file_discovery_us,
        format!("files={}", timing.file_discovery_count),
    );
    library_db::report_library_scan_timing(
        "classify_total",
        classify_t.elapsed().as_micros() as u64,
        format!(
            "discoveries={} normal_files={} containers={} entries={}",
            discoveries.len(),
            normal_files.len(),
            containers.len(),
            entries.len()
        ),
    );
    let audit_rows = match audit_mode {
        CoverageAuditMode::Inline => {
            let audit_t = Instant::now();
            let audit_rows = core_audit::audit_catalog_coverage(&cfg.roots, &profiles);
            library_db::report_library_scan_timing(
                "coverage_audit",
                audit_t.elapsed().as_micros() as u64,
                format!("rows={}", audit_rows.len()),
            );
            audit_rows
        }
        CoverageAuditMode::Deferred => Vec::new(),
    };
    LibraryScan {
        version: SCHEMA_VERSION,
        scanned_at_unix: library_db::unix_now_secs(),
        roots: cfg.roots.clone(),
        profiles,
        normal_files,
        containers,
        entries,
        audit_rows,
        ignored_files,
        discoveries,
        discover_us,
        classify_us: classify_t.elapsed().as_micros() as u64,
    }
}

fn report_new_discovered_systems(
    discoveries: &[GameDiscovery],
    discovered_systems: &mut BTreeSet<String>,
    scan_events: &mut ScanEventCallback<'_>,
) {
    let Some(report) = scan_events.as_mut() else {
        return;
    };
    for discovery in discoveries {
        let system_id = catalog_system_id_for_discovery(discovery);
        if !screenshot_pack_system_supported(&system_id) {
            continue;
        }
        if discovered_systems.insert(system_id.clone()) {
            report(LibraryScanEvent::SystemDiscovered { system_id });
        }
    }
}

fn screenshot_pack_system_supported(system_id: &str) -> bool {
    SCREENSHOT_PACK_SYSTEM_IDS.contains(&system_id)
}

fn bootstrap_library_progress(
    cfg: &BenchConfig,
    mut progress: ProgressCallback<'_>,
) -> LibraryBootstrapSummary {
    let started = Instant::now();
    let mut launchers = 0usize;
    for target in bootstrap_launcher_targets(&cfg.roots) {
        scan_bootstrap_launcher_target(&target, &mut launchers, &mut progress);
    }
    LibraryBootstrapSummary {
        launchers,
        scan_us: started.elapsed().as_micros() as u64,
    }
}

fn bootstrap_launcher_targets(roots: &[String]) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    for root in roots {
        let path = Path::new(root);
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("_Arcade"))
        {
            targets.push(path.to_path_buf());
        } else {
            targets.push(path.join("_Arcade"));
        }
    }
    targets
}

fn scan_bootstrap_launcher_target(
    target: &Path,
    launchers: &mut usize,
    progress: &mut ProgressCallback<'_>,
) {
    let Ok(entries) = std::fs::read_dir(target) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !is_bootstrap_launcher_path(&path) {
            continue;
        }
        *launchers += 1;
        if launchers.is_multiple_of(BOOTSTRAP_PROGRESS_BATCH) {
            report_catalog_progress(progress, CatalogProgress::finding_games_found(*launchers));
        }
    }
}

fn is_bootstrap_launcher_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name.len() > 1 && name.starts_with('.') {
        return false;
    }
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("mra" | "mgl")
    )
}
