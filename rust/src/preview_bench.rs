//! Preview image loading benchmark for arcade screenshots.

use crate::arcade_catalog::{self, ArcadeCatalog, ArcadeGameEntry, ImageLoadTiming};
use crate::preview_worker::PreviewWorker;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
struct BenchConfig {
    count: usize,
    interval_ms: u64,
}

impl BenchConfig {
    fn from_env() -> Self {
        Self {
            count: std::env::var("MISTER_PREVIEW_BENCH_COUNT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
            interval_ms: std::env::var("MISTER_PREVIEW_BENCH_INTERVAL_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(16),
        }
    }
}

#[derive(Default)]
struct Summary {
    samples: Vec<ImageLoadTiming>,
    failures: usize,
}

impl Summary {
    fn record(&mut self, timing: ImageLoadTiming) {
        self.samples.push(timing);
    }

    fn fail(&mut self) {
        self.failures += 1;
    }

    fn print(&self, elapsed_us: u64) {
        println!("preview_bench_summary");
        println!("samples={}", self.samples.len());
        println!("failures={}", self.failures);
        println!("elapsed_us={elapsed_us}");
        print_stats("read_us", self.samples.iter().map(|s| s.read_us).collect());
        print_stats("decode_us", self.samples.iter().map(|s| s.decode_us).collect());
        print_stats("total_us", self.samples.iter().map(|s| s.total_us).collect());
        print_stats(
            "encoded_bytes",
            self.samples.iter().map(|s| s.encoded_bytes as u64).collect(),
        );
        print_stats(
            "rgba_bytes",
            self.samples.iter().map(|s| s.rgba_bytes as u64).collect(),
        );
    }
}

pub fn run() {
    let cfg = BenchConfig::from_env();
    let mode = std::env::args().nth(2).unwrap_or_else(|| "sync".to_string());
    let root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    println!(
        "preview-bench mode={mode} root={root} count={} interval_ms={}",
        cfg.count, cfg.interval_ms
    );
    let (catalog, timings) =
        arcade_catalog::build_with_options(&root, arcade_catalog::BuildOptions::default(), None);
    timings.print_summary();
    match mode.as_str() {
        "sync" => run_sync(&catalog, cfg),
        "async" => run_async(&catalog, cfg),
        other => {
            eprintln!("unknown preview-bench mode '{other}' (use: sync | async)");
            std::process::exit(2);
        }
    }
}

fn run_sync(catalog: &ArcadeCatalog, cfg: BenchConfig) {
    let games = preview_games(catalog, cfg.count);
    println!("preview_bench_images={}", games.len());
    println!(
        "preview_bench_tsv\tidx\ttitle\tencoded_bytes\trgba_bytes\twidth\theight\tread_us\tdecode_us\ttotal_us\tok"
    );

    let start = Instant::now();
    let mut summary = Summary::default();
    for (idx, game) in games.iter().enumerate() {
        match arcade_catalog::load_png_rgba8_timed(&game.image_path) {
            Ok(loaded) => {
                let t = loaded.timing;
                summary.record(t);
                println!(
                    "preview_bench_tsv\t{idx}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\tyes",
                    sanitize(&game.title),
                    t.encoded_bytes,
                    t.rgba_bytes,
                    loaded.image.width,
                    loaded.image.height,
                    t.read_us,
                    t.decode_us,
                    t.total_us,
                );
            }
            Err(e) => {
                summary.fail();
                println!(
                    "preview_bench_tsv\t{idx}\t{}\t0\t0\t0\t0\t0\t0\t0\tno:{e}",
                    sanitize(&game.title)
                );
            }
        }
    }
    summary.print(start.elapsed().as_micros() as u64);
}

fn run_async(catalog: &ArcadeCatalog, cfg: BenchConfig) {
    let games = preview_games(catalog, cfg.count);
    println!("preview_bench_images={}", games.len());
    println!(
        "preview_bench_tsv\tidx\ttitle\tencoded_bytes\trgba_bytes\twidth\theight\tread_us\tdecode_us\ttotal_us\tlatency_us\tsubmit_us\tapplied\tok"
    );

    let mut worker = PreviewWorker::new();
    let start = Instant::now();
    let mut summary = Summary::default();
    let mut submit_samples = Vec::new();
    let mut latency_samples = Vec::new();
    let mut applied = 0usize;
    let mut stale = 0usize;
    let mut latest_generation = 0u64;
    let mut submitted = 0usize;
    let mut received = 0usize;

    for (idx, game) in games.iter().enumerate() {
        for result in worker.drain() {
            received += 1;
            let is_applied = result.generation == latest_generation;
            if is_applied {
                applied += 1;
                latency_samples.push(result.latency_us);
            } else {
                stale += 1;
            }
            print_async_result(&mut summary, result, 0, is_applied);
        }

        let submit_t = Instant::now();
        latest_generation = worker.request(idx, game.title.clone(), game.image_path.clone());
        let submit_us = submit_t.elapsed().as_micros() as u64;
        submit_samples.push(submit_us);
        submitted += 1;
        std::thread::sleep(Duration::from_millis(cfg.interval_ms));
    }

    while received < submitted {
        let Some(result) = worker.recv() else {
            break;
        };
        received += 1;
        let is_applied = result.generation == latest_generation;
        if is_applied {
            applied += 1;
            latency_samples.push(result.latency_us);
        } else {
            stale += 1;
        }
        print_async_result(&mut summary, result, 0, is_applied);
    }

    summary.print(start.elapsed().as_micros() as u64);
    println!("submitted={submitted}");
    println!("applied={applied}");
    println!("stale={stale}");
    print_stats("submit_us", submit_samples);
    print_stats("latency_us", latency_samples);
}

fn print_async_result(
    summary: &mut Summary,
    result: crate::preview_worker::PreviewResult,
    submit_us: u64,
    applied: bool,
) {
    if let Some(image) = result.image {
        let t = result.timing;
        summary.record(t);
        println!(
            "preview_bench_tsv\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\tyes",
            result.selected,
            sanitize(&result.title),
            t.encoded_bytes,
            t.rgba_bytes,
            image.width,
            image.height,
            t.read_us,
            t.decode_us,
            t.total_us,
            result.latency_us,
            submit_us,
            applied,
        );
    } else {
        summary.fail();
        println!(
            "preview_bench_tsv\t{}\t{}\t0\t0\t0\t0\t0\t0\t0\t{}\t{}\t{}\tno:{}",
            result.selected,
            sanitize(&result.title),
            result.latency_us,
            submit_us,
            applied,
            result.error.unwrap_or_else(|| "unknown".to_string())
        );
    }
}

fn preview_games(catalog: &ArcadeCatalog, count: usize) -> Vec<&ArcadeGameEntry> {
    catalog
        .games
        .iter()
        .filter(|g| g.has_image)
        .take(count)
        .collect()
}

fn sanitize(s: &str) -> String {
    s.replace('\t', " ").replace('\n', " ")
}

fn print_stats(name: &str, mut values: Vec<u64>) {
    values.sort_unstable();
    if values.is_empty() {
        println!("{name}_avg=0");
        println!("{name}_p50=0");
        println!("{name}_p90=0");
        println!("{name}_max=0");
        return;
    }
    let sum: u128 = values.iter().map(|&v| v as u128).sum();
    println!("{name}_avg={}", sum / values.len() as u128);
    println!("{name}_p50={}", percentile(&values, 50));
    println!("{name}_p90={}", percentile(&values, 90));
    println!("{name}_max={}", values[values.len() - 1]);
}

fn percentile(values: &[u64], pct: usize) -> u64 {
    let idx = ((values.len().saturating_sub(1)) * pct) / 100;
    values[idx]
}
