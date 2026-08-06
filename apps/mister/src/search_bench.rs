// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_catalog::persisted_search::{PersistedSearchTiming, search_system_shards};
use mister_magik_catalog::shard_registry::{production_registry_limits, read_latest_manifest_lazy};
use serde_json::{Value, json};
use std::path::Path;

const QUERIES: &[&str] = &["pac", "street", "capcom", "2 player"];
const WARMUP_ITERATIONS: usize = 1;
const MEASURED_ITERATIONS: usize = 20;

pub fn run() {
    match benchmark(mister_magik_catalog::catalog_config::default_sharded_catalog_path()) {
        Ok(summary) => crate::ui_logln!("{summary}"),
        Err(error) => {
            crate::ui_errln!("search benchmark failed: {error}");
            std::process::exit(1);
        }
    }
}

pub(crate) fn benchmark(storage_root: impl AsRef<Path>) -> Result<Value, String> {
    let storage_root = storage_root.as_ref();
    let limits = production_registry_limits();
    let manifest =
        read_latest_manifest_lazy(storage_root, limits).map_err(|error| error.to_string())?;
    if !manifest
        .systems
        .iter()
        .any(|system| system.system_id.as_str() == "arcade")
    {
        return Err("active catalog has no arcade system shard".to_string());
    }
    let system_ids = vec!["arcade".to_string()];
    let mut query_summaries = Vec::with_capacity(QUERIES.len());
    let mut all_samples = TimingSamples::default();

    for query in QUERIES {
        let first = search_system_shards(storage_root, &system_ids, query, limits)
            .map_err(|error| error.to_string())?;
        for _ in 0..WARMUP_ITERATIONS {
            search_system_shards(storage_root, &system_ids, query, limits)
                .map_err(|error| error.to_string())?;
        }

        let mut samples = TimingSamples::default();
        for _ in 0..MEASURED_ITERATIONS {
            let result = search_system_shards(storage_root, &system_ids, query, limits)
                .map_err(|error| error.to_string())?;
            samples.push(result.timing);
            all_samples.push(result.timing);
        }
        query_summaries.push(json!({
            "query": query,
            "first": timing_json(first.timing),
            "first_matches": first.matches.len(),
            "autocomplete": first.autocomplete.map(|candidate| candidate.word),
            "warm": samples.summary(),
        }));
    }

    Ok(json!({
        "schema": "mister-magik-search-benchmark-v1",
        "system_ids": system_ids,
        "warmup_iterations": WARMUP_ITERATIONS,
        "measured_iterations": MEASURED_ITERATIONS,
        "queries": query_summaries,
        "warm_all_queries": all_samples.summary(),
    }))
}

#[derive(Default)]
struct TimingSamples {
    rust_prepare_us: Vec<u64>,
    sqlite_us: Vec<u64>,
    rust_finalize_us: Vec<u64>,
    total_us: Vec<u64>,
}

impl TimingSamples {
    fn push(&mut self, timing: PersistedSearchTiming) {
        self.rust_prepare_us.push(timing.rust_prepare_us);
        self.sqlite_us.push(timing.sqlite_us);
        self.rust_finalize_us.push(timing.rust_finalize_us);
        self.total_us.push(timing.total_us);
    }

    fn summary(&self) -> Value {
        json!({
            "rust_prepare_us": distribution(&self.rust_prepare_us),
            "sqlite_us": distribution(&self.sqlite_us),
            "rust_finalize_us": distribution(&self.rust_finalize_us),
            "total_us": distribution(&self.total_us),
        })
    }
}

fn timing_json(timing: PersistedSearchTiming) -> Value {
    json!({
        "rust_prepare_us": timing.rust_prepare_us,
        "sqlite_us": timing.sqlite_us,
        "rust_finalize_us": timing.rust_finalize_us,
        "total_us": timing.total_us,
    })
}

fn distribution(samples: &[u64]) -> Value {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    json!({
        "p50": percentile(&sorted, 50),
        "p95": percentile(&sorted, 95),
        "max": sorted.last().copied().unwrap_or(0),
    })
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = (sorted.len() - 1).saturating_mul(percentile).div_ceil(100);
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_nearest_rank_without_exceeding_the_sample() {
        let samples = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        assert_eq!(percentile(&samples, 50), 6);
        assert_eq!(percentile(&samples, 95), 10);
        assert_eq!(percentile(&[], 95), 0);
    }
}
