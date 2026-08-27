// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_framebuffer_scenes::{Rgb565Pixel, SceneGeometry};
use mister_magik_screenshot_parade::{ScreenshotParade, ScreenshotParadeConfig};
use serde::Serialize;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const SCREENSHOT_FRAMES: u64 = 180;
const SCREENSHOT_WIDTH: usize = 960;
const SCREENSHOT_HEIGHT: usize = 600;
const SCREENSHOT_SEED: u64 = 0x4d61_6769_4b50_4d55;
const CATALOG_PROFILE_ROOT: &str = "/tmp/mister-magik/pmu-catalog-benchmark";

pub fn run(args: &[String]) {
    let result = match args {
        [workload] if workload == "probe" => Ok(crate::pmu_probe::probe()),
        [workload] if workload == "screensaver" => profile_screensaver(),
        [workload] if workload == "search" => profile_search(),
        [workload] if workload == "catalog" => profile_catalog(),
        _ => Err("usage: pmu-profile <probe|screensaver|search|catalog>".to_owned()),
    };
    match result {
        Ok(summary) => crate::ui_logln!("{summary}"),
        Err(error) => {
            crate::ui_errln!("PMU profile failed: {error}");
            std::process::exit(1);
        }
    }
}

fn profile_screensaver() -> Result<Value, String> {
    let _ = mister_magik_perf_events::take_thread_profile();
    let archive_path = mister_magik_catalog::media_identity::default_screenshot_asset_dir()
        .join("arcade-screenshots-320x320.mmlz4b");
    let archive =
        mister_magik_catalog::preview_worker::ResidentPreviewArchive::open(&archive_path)?;
    let geometry = SceneGeometry::new(SCREENSHOT_WIDTH, SCREENSHOT_HEIGHT, SCREENSHOT_WIDTH)
        .map_err(|error| error.to_string())?;
    let mut parade = ScreenshotParade::new_offline_prepared(
        archive,
        ScreenshotParadeConfig {
            geometry,
            seed: SCREENSHOT_SEED,
            worker_start: None,
            preparation_slack: None,
        },
    )?;
    let mut pixels = vec![Rgb565Pixel(0); geometry.len()];
    let started = Instant::now();
    let mut last_stats = None;
    for tick in 0..SCREENSHOT_FRAMES {
        last_stats = Some(parade.render_at_presentation_tick(&mut pixels, tick)?);
    }
    let elapsed_us = started.elapsed().as_micros();
    let profile = mister_magik_screenshot_parade::take_render_pmu_profile();
    let stats = last_stats.ok_or("screensaver PMU workload rendered no frames")?;
    Ok(json!({
        "schema": "mister-magik-pmu-workload-v1",
        "workload": "screensaver",
        "status": profile_status(&profile),
        "configuration": {
            "frames": SCREENSHOT_FRAMES,
            "width": SCREENSHOT_WIDTH,
            "height": SCREENSHOT_HEIGHT,
            "seed": SCREENSHOT_SEED,
            "archive_path": archive_path,
        },
        "elapsed_us": elapsed_us,
        "result": {
            "active_cards": stats.active_cards,
            "cards_drawn": stats.cards_drawn,
            "cards_culled": stats.cards_culled,
            "phase_bank_resident_bytes": stats.phase_bank_resident_bytes,
        },
        "profile": profile,
    }))
}

fn profile_search() -> Result<Value, String> {
    let _ = mister_magik_perf_events::take_thread_profile();
    let started = Instant::now();
    let benchmark = crate::search_bench::benchmark(
        mister_magik_catalog::catalog_config::default_sharded_catalog_path(),
    )?;
    let elapsed_us = started.elapsed().as_micros();
    let profile = mister_magik_perf_events::take_thread_profile();
    Ok(json!({
        "schema": "mister-magik-pmu-workload-v1",
        "workload": "search",
        "status": profile_status(&profile),
        "configuration": {
            "benchmark_schema": benchmark.get("schema").and_then(Value::as_str),
            "warmup_iterations": benchmark.get("warmup_iterations"),
            "measured_iterations": benchmark.get("measured_iterations"),
        },
        "elapsed_us": elapsed_us,
        "result": benchmark,
        "profile": profile,
    }))
}

#[derive(Serialize)]
struct CatalogOperationReport {
    operation: &'static str,
    status: &'static str,
    elapsed_us: u128,
    peak_rss_kib: Option<u64>,
    timings: Vec<(String, u64)>,
    rebuilt_systems: Vec<String>,
    removed_systems: Vec<String>,
    manifest_generation: u64,
    manifest_systems: Vec<String>,
    manifest_games: u64,
    profile: mister_magik_perf_events::ProcessProfileBatch,
    aggregate: CatalogPmuAggregate,
}

#[derive(Default, Serialize)]
struct CatalogPmuAggregate {
    cycles: u64,
    instructions: u64,
    speculative_instructions: u64,
    neon_instructions: u64,
    neon_clock_cycles: u64,
    data_dependent_stall_cycles: u64,
    l1d_accesses: u64,
    l1d_refills: u64,
    branches: u64,
    branch_mispredicts: u64,
    instructions_per_cycle: f64,
    l1d_refill_ratio: f64,
    branch_mispredict_ratio: f64,
    speculative_instructions_per_cycle: f64,
    neon_instruction_share: f64,
    neon_instructions_per_active_cycle: f64,
    neon_clock_duty: f64,
    data_dependent_stall_ratio: f64,
}

struct CatalogProfileRoot {
    path: PathBuf,
}

impl CatalogProfileRoot {
    fn prepare(path: &Path) -> Result<Self, String> {
        validate_catalog_profile_root(path)?;
        if path.exists() {
            let mut entries = fs::read_dir(path)
                .map_err(|error| format!("inspect catalog PMU root {}: {error}", path.display()))?;
            if entries
                .next()
                .transpose()
                .map_err(|error| {
                    format!("inspect catalog PMU root entry {}: {error}", path.display())
                })?
                .is_some()
            {
                return Err(format!(
                    "catalog PMU root must be empty before use: {}",
                    path.display()
                ));
            }
        } else {
            fs::create_dir_all(path)
                .map_err(|error| format!("create catalog PMU root {}: {error}", path.display()))?;
        }
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for CatalogProfileRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Copy)]
enum CatalogProfileOperation {
    Fresh,
    Update,
    RebuildAll,
}

fn profile_catalog() -> Result<Value, String> {
    let root = PathBuf::from(CATALOG_PROFILE_ROOT);
    let _root_guard = CatalogProfileRoot::prepare(&root)?;
    let fixture_root = root.join("fixture");
    let catalog_root = root.join("catalog-fast-v1");
    let fixture = mister_magik_catalog::synthetic_fixture::generate_synthetic_fixture(
        &fixture_root,
        &mister_magik_catalog::synthetic_fixture::SyntheticFixtureSpec {
            arcade_games: 0,
            small_system_games: 1,
            large_system_games: 0,
            large_system_depth: 1,
        },
    )
    .map_err(|error| format!("create catalog PMU fixture: {error}"))?;

    let fresh = profile_catalog_operation(
        "fresh-build",
        CatalogProfileOperation::Fresh,
        &fixture_root,
        &catalog_root,
    )?;
    mister_magik_catalog::synthetic_fixture::add_synthetic_snes_game(&fixture_root, 1)
        .map_err(|error| format!("append catalog PMU fixture game: {error}"))?;
    let rebuild = profile_catalog_operation(
        "update",
        CatalogProfileOperation::Update,
        &fixture_root,
        &catalog_root,
    )?;
    validate_incremental_catalog_profile(&fresh, &rebuild)?;
    let rebuild_all = profile_catalog_operation(
        "rebuild-all",
        CatalogProfileOperation::RebuildAll,
        &fixture_root,
        &catalog_root,
    )?;
    validate_rebuild_all_catalog_profile(&rebuild, &rebuild_all)?;

    let status = if [&fresh, &rebuild, &rebuild_all]
        .into_iter()
        .all(|operation| operation.status == "ok")
    {
        "ok"
    } else {
        "failed"
    };
    Ok(json!({
        "schema": "mister-magik-pmu-workload-v1",
        "workload": "catalog",
        "status": status,
        "configuration": {
            "root": CATALOG_PROFILE_ROOT,
            "storage_root": fixture_root,
            "fixture": fixture,
            "operations": ["fresh-build", "update", "rebuild-all"],
        },
        "operations": [fresh, rebuild, rebuild_all],
        "validation": {
            "isolated": true,
            "incremental_rebuilt_systems": ["snes"],
            "incremental_game_delta": 1,
            "rebuild_all_preserved_counts": true,
        },
    }))
}

fn profile_catalog_operation(
    operation_label: &'static str,
    operation: CatalogProfileOperation,
    storage_root: &Path,
    catalog_root: &Path,
) -> Result<CatalogOperationReport, String> {
    use mister_magik_catalog::fast_catalog_refresh::{
        FastCatalogRefreshRequest, FastCatalogSystemOutcome,
    };

    mister_magik_perf_events::clear_process_profiles();
    let started = Instant::now();
    let (timings, mut rebuilt_systems, mut removed_systems) = match operation {
        CatalogProfileOperation::Fresh => {
            let report = mister_magik_catalog::fast_catalog_refresh::build_fresh_catalog(
                storage_root,
                catalog_root,
            )?;
            (
                vec![
                    ("source".to_string(), report.source.elapsed_us),
                    (
                        "artifact-publish".to_string(),
                        report.publication.elapsed_us,
                    ),
                    ("snapshot-capture".to_string(), report.capture.elapsed_us),
                ],
                report.system_ids,
                Vec::new(),
            )
        }
        CatalogProfileOperation::Update | CatalogProfileOperation::RebuildAll => {
            let request = if matches!(operation, CatalogProfileOperation::Update) {
                FastCatalogRefreshRequest::Update
            } else {
                FastCatalogRefreshRequest::RebuildAll
            };
            let report = mister_magik_catalog::fast_catalog_refresh::execute_fast_refresh(
                storage_root,
                catalog_root,
                request,
            )?;
            let rebuilt = report
                .system_reports
                .iter()
                .filter(|system| system.outcome == FastCatalogSystemOutcome::Updated)
                .map(|system| system.system_id.clone())
                .collect();
            let removed = report
                .system_reports
                .iter()
                .filter(|system| system.outcome == FastCatalogSystemOutcome::Removed)
                .map(|system| system.system_id.clone())
                .collect();
            (
                vec![
                    ("planning".to_string(), report.planning_us),
                    ("source".to_string(), report.source_rebuild_us),
                    ("artifact-publish".to_string(), report.artifact_publish_us),
                    ("snapshot-publish".to_string(), report.snapshot_publish_us),
                ],
                rebuilt,
                removed,
            )
        }
    };
    mister_magik_perf_events::submit_thread_profile("catalog-refresh");
    let elapsed_us = started.elapsed().as_micros();
    let profile = mister_magik_perf_events::take_process_profiles();

    rebuilt_systems.sort();
    removed_systems.sort();
    let manifest = mister_magik_catalog::shard_registry::read_latest_manifest(
        catalog_root,
        mister_magik_catalog::shard_registry::production_registry_limits(),
    )
    .map_err(|error| format!("validate {operation_label} catalog manifest: {error}"))?;
    let mut manifest_systems = manifest
        .systems
        .iter()
        .map(|system| system.system_id.as_str().to_owned())
        .collect::<Vec<_>>();
    manifest_systems.sort();
    let manifest_games = manifest
        .systems
        .iter()
        .try_fold(0_u64, |total, system| {
            total.checked_add(system.active.games)
        })
        .ok_or_else(|| format!("{operation_label} manifest game count overflow"))?;
    let status = process_profile_status(&profile);
    let aggregate = aggregate_process_profile(&profile);
    Ok(CatalogOperationReport {
        operation: operation_label,
        status,
        elapsed_us,
        peak_rss_kib: peak_rss_kib(),
        timings,
        rebuilt_systems,
        removed_systems,
        manifest_generation: manifest.generation,
        manifest_systems,
        manifest_games,
        profile,
        aggregate,
    })
}

fn validate_incremental_catalog_profile(
    fresh: &CatalogOperationReport,
    rebuild: &CatalogOperationReport,
) -> Result<(), String> {
    if rebuild.rebuilt_systems != ["snes"] {
        return Err(format!(
            "incremental catalog PMU rebuild changed {:?}, expected only snes",
            rebuild.rebuilt_systems
        ));
    }
    if rebuild.manifest_games != fresh.manifest_games.saturating_add(1) {
        return Err(format!(
            "incremental catalog PMU game count changed from {} to {}, expected one game",
            fresh.manifest_games, rebuild.manifest_games
        ));
    }
    Ok(())
}

fn validate_rebuild_all_catalog_profile(
    rebuild: &CatalogOperationReport,
    rebuild_all: &CatalogOperationReport,
) -> Result<(), String> {
    if rebuild_all.rebuilt_systems != rebuild.manifest_systems {
        return Err(format!(
            "catalog PMU rebuild-all rebuilt {:?}, expected {:?}",
            rebuild_all.rebuilt_systems, rebuild.manifest_systems
        ));
    }
    if rebuild_all.manifest_systems != rebuild.manifest_systems
        || rebuild_all.manifest_games != rebuild.manifest_games
    {
        return Err("catalog PMU rebuild-all changed the published catalog contents".to_owned());
    }
    Ok(())
}

fn validate_catalog_profile_root(path: &Path) -> Result<(), String> {
    if path != Path::new(CATALOG_PROFILE_ROOT)
        || !path.is_absolute()
        || path.parent() != Some(Path::new("/tmp/mister-magik"))
    {
        return Err(format!(
            "unsafe catalog PMU root {}; expected {CATALOG_PROFILE_ROOT}",
            path.display()
        ));
    }
    Ok(())
}

fn process_profile_status(batch: &mister_magik_perf_events::ProcessProfileBatch) -> &'static str {
    if batch.dropped_profiles == 0
        && !batch.profiles.is_empty()
        && batch.profiles.iter().all(|entry| {
            entry.profile.enabled
                && entry.profile.failure.is_none()
                && entry.profile.dropped_spans == 0
                && !entry.profile.records.is_empty()
        })
    {
        "ok"
    } else {
        "failed"
    }
}
fn aggregate_process_profile(
    batch: &mister_magik_perf_events::ProcessProfileBatch,
) -> CatalogPmuAggregate {
    let mut aggregate = CatalogPmuAggregate::default();
    for record in batch
        .profiles
        .iter()
        .flat_map(|entry| entry.profile.records.iter())
    {
        use mister_magik_perf_events::HardwareEvent;
        let counters = &record.counters.counters;
        aggregate.cycles = aggregate
            .cycles
            .saturating_add(counters.get(HardwareEvent::Cycles).unwrap_or_default());
        aggregate.instructions = aggregate.instructions.saturating_add(
            counters
                .get(HardwareEvent::Instructions)
                .unwrap_or_default(),
        );
        aggregate.speculative_instructions = aggregate.speculative_instructions.saturating_add(
            counters
                .get(HardwareEvent::SpeculativeInstructions)
                .unwrap_or_default(),
        );
        aggregate.neon_instructions = aggregate.neon_instructions.saturating_add(
            counters
                .get(HardwareEvent::NeonInstructions)
                .unwrap_or_default(),
        );
        aggregate.neon_clock_cycles = aggregate.neon_clock_cycles.saturating_add(
            counters
                .get(HardwareEvent::NeonClockCycles)
                .unwrap_or_default(),
        );
        aggregate.data_dependent_stall_cycles =
            aggregate.data_dependent_stall_cycles.saturating_add(
                counters
                    .get(HardwareEvent::DataDependentStallCycles)
                    .unwrap_or_default(),
            );
        aggregate.l1d_accesses = aggregate
            .l1d_accesses
            .saturating_add(counters.get(HardwareEvent::L1dAccesses).unwrap_or_default());
        aggregate.l1d_refills = aggregate
            .l1d_refills
            .saturating_add(counters.get(HardwareEvent::L1dRefills).unwrap_or_default());
        aggregate.branches = aggregate
            .branches
            .saturating_add(counters.get(HardwareEvent::Branches).unwrap_or_default());
        aggregate.branch_mispredicts = aggregate.branch_mispredicts.saturating_add(
            counters
                .get(HardwareEvent::BranchMispredicts)
                .unwrap_or_default(),
        );
    }
    aggregate.instructions_per_cycle = ratio(aggregate.instructions, aggregate.cycles);
    aggregate.l1d_refill_ratio = ratio(aggregate.l1d_refills, aggregate.l1d_accesses);
    aggregate.branch_mispredict_ratio = ratio(aggregate.branch_mispredicts, aggregate.branches);
    aggregate.speculative_instructions_per_cycle =
        ratio(aggregate.speculative_instructions, aggregate.cycles);
    aggregate.neon_instruction_share = ratio(
        aggregate.neon_instructions,
        aggregate.speculative_instructions,
    );
    aggregate.neon_instructions_per_active_cycle =
        ratio(aggregate.neon_instructions, aggregate.neon_clock_cycles);
    aggregate.neon_clock_duty = ratio(aggregate.neon_clock_cycles, aggregate.cycles);
    aggregate.data_dependent_stall_ratio =
        ratio(aggregate.data_dependent_stall_cycles, aggregate.cycles);
    aggregate
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn peak_rss_kib() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    parse_peak_rss_kib(&status)
}

fn parse_peak_rss_kib(status: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmHWM:")?.trim();
        value.strip_suffix(" kB")?.trim().parse().ok()
    })
}

fn profile_status(profile: &mister_magik_perf_events::ThreadProfile) -> &'static str {
    if profile.enabled && profile.failure.is_none() && !profile.records.is_empty() {
        "ok"
    } else {
        "failed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn populated_thread_profile() -> mister_magik_perf_events::ThreadProfile {
        mister_magik_perf_events::ThreadProfile {
            schema: "mister-magik-pmu-thread-profile-v2",
            counter_set: mister_magik_perf_events::CounterSet::General,
            enabled: true,
            sample_every: 1,
            attempted_spans: 1,
            dropped_spans: 0,
            records: vec![mister_magik_perf_events::SpanRecord {
                name: "catalog.scan".into(),
                counters: mister_magik_perf_events::CounterDelta {
                    counters: mister_magik_perf_events::CounterValues::from([
                        (mister_magik_perf_events::HardwareEvent::Cycles, 20),
                        (mister_magik_perf_events::HardwareEvent::Instructions, 10),
                        (mister_magik_perf_events::HardwareEvent::L1dAccesses, 4),
                        (mister_magik_perf_events::HardwareEvent::L1dRefills, 1),
                        (mister_magik_perf_events::HardwareEvent::Branches, 5),
                        (
                            mister_magik_perf_events::HardwareEvent::BranchMispredicts,
                            1,
                        ),
                    ]),
                    ..mister_magik_perf_events::CounterDelta::default()
                },
                derived: mister_magik_perf_events::DerivedMetrics::default(),
            }],
            failure: None,
            read_format: None,
            scope: None,
        }
    }

    #[test]
    fn profile_status_requires_enabled_nonempty_failure_free_evidence() {
        let empty = mister_magik_perf_events::ThreadProfile {
            schema: "mister-magik-pmu-thread-profile-v2",
            counter_set: mister_magik_perf_events::CounterSet::General,
            enabled: true,
            sample_every: 1,
            attempted_spans: 1,
            dropped_spans: 0,
            records: Vec::new(),
            failure: None,
            read_format: None,
            scope: None,
        };
        assert_eq!(profile_status(&empty), "failed");
    }

    #[test]
    fn catalog_profile_root_is_exact_and_bounded() {
        assert!(validate_catalog_profile_root(Path::new(CATALOG_PROFILE_ROOT)).is_ok());
        assert!(validate_catalog_profile_root(Path::new("/tmp/mister-magik")).is_err());
        assert!(validate_catalog_profile_root(Path::new("/media/fat/mister-magik")).is_err());
        assert!(validate_catalog_profile_root(Path::new("relative/catalog")).is_err());
    }

    #[test]
    fn catalog_profile_roots_append_the_fixture_without_replacing_defaults() {
        let roots = catalog_profile_library_roots(Path::new("/tmp/fixture"));
        assert!(roots.starts_with("/media/fat/_Arcade|/media/fat/games|"));
        assert!(roots.ends_with("|/tmp/fixture"));
    }

    #[test]
    fn process_profile_status_and_aggregate_require_complete_evidence() {
        let batch = mister_magik_perf_events::ProcessProfileBatch {
            profiles: vec![mister_magik_perf_events::SubmittedThreadProfile {
                label: "catalog-builder".into(),
                profile: populated_thread_profile(),
            }],
            dropped_profiles: 0,
        };
        assert_eq!(process_profile_status(&batch), "ok");
        let aggregate = aggregate_process_profile(&batch);
        assert_eq!(aggregate.instructions_per_cycle, 0.5);
        assert_eq!(aggregate.l1d_refill_ratio, 0.25);
        assert_eq!(aggregate.branch_mispredict_ratio, 0.2);
    }

    #[test]
    fn catalog_operation_report_has_stable_json_fields() {
        let report = CatalogOperationReport {
            operation: "fresh-build",
            status: "ok",
            elapsed_us: 1,
            peak_rss_kib: Some(2),
            summary: None,
            timings: Vec::new(),
            planned_systems: vec!["snes".into()],
            all_published_systems: true,
            rebuilt_systems: vec!["snes".into()],
            removed_systems: Vec::new(),
            manifest_generation: 3,
            manifest_systems: vec!["snes".into()],
            manifest_games: 2,
            profile: mister_magik_perf_events::ProcessProfileBatch::default(),
            aggregate: CatalogPmuAggregate::default(),
        };
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["operation"], "fresh-build");
        assert_eq!(value["manifest_games"], 2);
        assert!(value["profile"]["profiles"].is_array());
    }

    #[test]
    fn peak_rss_parser_requires_linux_status_units() {
        assert_eq!(
            parse_peak_rss_kib("Name:\ttest\nVmHWM:\t1234 kB\n"),
            Some(1234)
        );
        assert_eq!(parse_peak_rss_kib("VmHWM:\t1234 MB\n"), None);
    }
}
