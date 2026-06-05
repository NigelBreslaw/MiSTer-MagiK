//! Preview image loading benchmark for arcade screenshots.

use crate::arcade_catalog::{self, ArcadeCatalog, ArcadeGameEntry, ImageLoadTiming};
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
struct BenchConfig {
    count: usize,
}

impl BenchConfig {
    fn from_env() -> Self {
        Self {
            count: std::env::var("MISTER_PREVIEW_BENCH_COUNT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
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
    let root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    println!("preview-bench mode=sync root={root} count={}", cfg.count);
    let (catalog, timings) =
        arcade_catalog::build_with_options(&root, arcade_catalog::BuildOptions::default(), None);
    timings.print_summary();
    run_sync(&catalog, cfg);
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
