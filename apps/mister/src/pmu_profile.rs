// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_framebuffer_scenes::{Rgb565Pixel, SceneGeometry};
use mister_magik_screenshot_parade::{ScreenshotParade, ScreenshotParadeConfig};
use serde_json::{Value, json};
use std::time::Instant;

const SCREENSHOT_FRAMES: u64 = 180;
const SCREENSHOT_WIDTH: usize = 960;
const SCREENSHOT_HEIGHT: usize = 600;
const SCREENSHOT_SEED: u64 = 0x4d61_6769_4b50_4d55;

pub fn run(args: &[String]) {
    let result = match args {
        [workload] if workload == "probe" => Ok(crate::pmu_probe::probe()),
        [workload] if workload == "screensaver" => profile_screensaver(),
        [workload] if workload == "search" => profile_search(),
        _ => Err("usage: pmu-profile <probe|screensaver|search>".to_owned()),
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

    #[test]
    fn profile_status_requires_enabled_nonempty_failure_free_evidence() {
        let empty = mister_magik_perf_events::ThreadProfile {
            schema: "mister-magik-pmu-thread-profile-v1",
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
}
