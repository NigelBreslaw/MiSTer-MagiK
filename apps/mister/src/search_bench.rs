// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_catalog::persisted_search::{PersistedSearchCatalog, PersistedSearchTiming};
use mister_magik_catalog::shard_registry::production_registry_limits;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const QUERIES: &[&str] = &["pac", "street", "capcom", "2 player"];
const WARMUP_ITERATIONS: usize = 1;
const MEASURED_ITERATIONS: usize = 20;
const SUITE_RUNS: usize = 3;

pub fn run() {
    match benchmark(mister_magik_catalog::catalog_config::default_sharded_catalog_path()) {
        Ok(summary) => crate::ui_logln!("{summary}"),
        Err(error) => {
            crate::ui_errln!("search benchmark failed: {error}");
            std::process::exit(1);
        }
    }
}

pub fn benchmark(storage_root: impl AsRef<Path>) -> Result<Value, String> {
    let storage_root = storage_root.as_ref();
    let limits = production_registry_limits();
    let search_catalog =
        PersistedSearchCatalog::open(storage_root, limits).map_err(|error| error.to_string())?;
    if !search_catalog.contains_system("arcade") {
        return Err("active catalog has no arcade system shard".to_string());
    }
    let system_ids = vec!["arcade".to_string()];
    let process_before = process_metrics();
    let runtime_before = mister_magik_catalog::persisted_search::runtime_metrics();
    let mut runs = Vec::with_capacity(SUITE_RUNS);
    let mut all_samples = TimingSamples::default();
    for run in 1..=SUITE_RUNS {
        let run_before = mister_magik_catalog::persisted_search::runtime_metrics();
        let mut query_summaries = Vec::with_capacity(QUERIES.len());
        let mut run_samples = TimingSamples::default();
        for query in QUERIES {
            let first = search_catalog
                .search(&system_ids, query)
                .map_err(|error| error.to_string())?;
            let result_hash = hash_search_result(&first);
            for _ in 0..WARMUP_ITERATIONS {
                search_catalog
                    .search(&system_ids, query)
                    .map_err(|error| error.to_string())?;
            }

            let mut samples = TimingSamples::default();
            for _ in 0..MEASURED_ITERATIONS {
                let result = search_catalog
                    .search(&system_ids, query)
                    .map_err(|error| error.to_string())?;
                if hash_search_result(&result) != result_hash {
                    return Err(format!(
                        "search result changed within run {run} query {query}"
                    ));
                }
                samples.push(result.timing);
                run_samples.push(result.timing);
                all_samples.push(result.timing);
            }
            query_summaries.push(json!({
                "query": query,
                "first": timing_json(first.timing),
                "first_matches": first.matches.len(),
                "autocomplete": first.autocomplete.map(|candidate| candidate.word),
                "result_hash": result_hash,
                "warm": samples.summary(),
            }));
        }
        let run_after = mister_magik_catalog::persisted_search::runtime_metrics();
        runs.push(json!({
            "run": run,
            "queries": query_summaries,
            "warm_all_queries": run_samples.summary(),
            "sqlite_opens": run_after.sqlite_opens.saturating_sub(run_before.sqlite_opens),
            "statement_prepares": run_after.statement_prepares.saturating_sub(run_before.statement_prepares),
        }));
    }
    let runtime_after = mister_magik_catalog::persisted_search::runtime_metrics();
    let process_after = process_metrics();

    Ok(json!({
        "schema": "mister-magik-search-benchmark-v2",
        "system_ids": system_ids,
        "suite_runs": SUITE_RUNS,
        "warmup_iterations": WARMUP_ITERATIONS,
        "measured_iterations": MEASURED_ITERATIONS,
        "runs": runs,
        "warm_all_queries": all_samples.summary(),
        "runtime_churn": {
            "worker_threads": 0,
            "sqlite_opens": runtime_after.sqlite_opens.saturating_sub(runtime_before.sqlite_opens),
            "statement_prepares": runtime_after.statement_prepares.saturating_sub(runtime_before.statement_prepares),
        },
        "process": {
            "before": process_before,
            "after": process_after,
            "minor_fault_delta": process_after.minor_faults.saturating_sub(process_before.minor_faults),
            "major_fault_delta": process_after.major_faults.saturating_sub(process_before.major_faults),
        },
    }))
}

#[derive(Default)]
struct TimingSamples {
    rust_prepare_us: Vec<u64>,
    sqlite_open_us: Vec<u64>,
    statement_prepare_us: Vec<u64>,
    sqlite_execute_us: Vec<u64>,
    sqlite_us: Vec<u64>,
    rust_finalize_us: Vec<u64>,
    total_us: Vec<u64>,
}

impl TimingSamples {
    fn push(&mut self, timing: PersistedSearchTiming) {
        self.rust_prepare_us.push(timing.rust_prepare_us);
        self.sqlite_open_us.push(timing.sqlite_open_us);
        self.statement_prepare_us.push(timing.statement_prepare_us);
        self.sqlite_execute_us.push(timing.sqlite_execute_us);
        self.sqlite_us.push(timing.sqlite_us);
        self.rust_finalize_us.push(timing.rust_finalize_us);
        self.total_us.push(timing.total_us);
    }

    fn summary(&self) -> Value {
        json!({
            "rust_prepare_us": distribution(&self.rust_prepare_us),
            "sqlite_open_us": distribution(&self.sqlite_open_us),
            "statement_prepare_us": distribution(&self.statement_prepare_us),
            "sqlite_execute_us": distribution(&self.sqlite_execute_us),
            "sqlite_us": distribution(&self.sqlite_us),
            "rust_finalize_us": distribution(&self.rust_finalize_us),
            "total_us": distribution(&self.total_us),
        })
    }
}

fn timing_json(timing: PersistedSearchTiming) -> Value {
    json!({
        "rust_prepare_us": timing.rust_prepare_us,
        "sqlite_open_us": timing.sqlite_open_us,
        "statement_prepare_us": timing.statement_prepare_us,
        "sqlite_execute_us": timing.sqlite_execute_us,
        "sqlite_us": timing.sqlite_us,
        "rust_finalize_us": timing.rust_finalize_us,
        "total_us": timing.total_us,
    })
}

fn hash_search_result(
    result: &mister_magik_catalog::persisted_search::PersistedCollectionSearchResult,
) -> String {
    let mut hash = Sha256::new();
    for entry in &result.matches {
        hash.update(entry.system_id.as_bytes());
        hash.update([0]);
        hash.update(entry.ordinal.to_le_bytes());
        hash.update(entry.rank.to_bits().to_le_bytes());
    }
    if let Some(candidate) = &result.autocomplete {
        hash.update(candidate.word.as_bytes());
        hash.update([candidate.source_rank]);
        hash.update(candidate.score.to_le_bytes());
    }
    let digest = hash.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
struct ProcessMetrics {
    rss_kib: u64,
    hwm_kib: u64,
    minor_faults: u64,
    major_faults: u64,
}

fn process_metrics() -> ProcessMetrics {
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kib = |label: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(label))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    };
    let stat = fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let fields = stat
        .rsplit_once(") ")
        .map(|(_, fields)| fields.split_whitespace().collect::<Vec<_>>())
        .unwrap_or_default();
    ProcessMetrics {
        rss_kib: kib("VmRSS:"),
        hwm_kib: kib("VmHWM:"),
        minor_faults: fields
            .get(7)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        major_faults: fields
            .get(9)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
    }
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
