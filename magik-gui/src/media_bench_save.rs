use crate::media_pack_save::{
    publish_pack_file_for_bench, temp_path_for, PackSaveMetrics, PackSaveMode,
    PROGRESS_COPY_CHUNK_BYTES,
};
use mister_magik_fb::media_update::{
    size_qualified_pack_path, valid_image_size, DEFAULT_ASSET_DIR, DEFAULT_IMAGE_SIZE,
};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const HEADER: &str = "screenshot_save_bench_tsv\tlabel\tsystem\tmode\titeration\tbytes\tcopy_ms\tsync_ms\trename_ms\tparent_sync_ms\ttotal_ms\tprogress_events\tresult";
const DEFAULT_SIZE_BYTES: u64 = 24 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct BenchConfig {
    label: String,
    system: String,
    image_size: String,
    asset_dir: PathBuf,
    size_bytes: u64,
    iterations: usize,
    modes: Vec<PackSaveMode>,
}

#[derive(Clone, Debug)]
struct SaveRow {
    label: String,
    system: String,
    mode: PackSaveMode,
    iteration: usize,
    metrics: PackSaveMetrics,
    result: String,
}

pub(crate) fn run() {
    match run_inner(std::env::args().skip(2)) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("media-bench-save failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run_inner<I>(args: I) -> Result<(), String>
where
    I: IntoIterator<Item = String>,
{
    let config = parse_args(args)?;
    fs::create_dir_all(&config.asset_dir)
        .map_err(|e| format!("create asset dir {}: {e}", config.asset_dir.display()))?;
    let source = resolve_source_path(&config)?;
    println!("{HEADER}");
    for mode in &config.modes {
        for iteration in 1..=config.iterations {
            let row = run_one(&config, &source, *mode, iteration);
            println!("{}", row.to_tsv());
            if row.result != "bench-ok" {
                return Err(row.to_tsv());
            }
        }
    }
    Ok(())
}

fn parse_args<I>(args: I) -> Result<BenchConfig, String>
where
    I: IntoIterator<Item = String>,
{
    let mut config = BenchConfig {
        label: default_label(),
        system: "neogeo".to_string(),
        image_size: std::env::var("MISTER_MEDIA_SIZE")
            .unwrap_or_else(|_| DEFAULT_IMAGE_SIZE.to_string()),
        asset_dir: PathBuf::from(
            std::env::var("MISTER_MEDIA_ASSET_DIR")
                .unwrap_or_else(|_| DEFAULT_ASSET_DIR.to_string()),
        ),
        size_bytes: DEFAULT_SIZE_BYTES,
        iterations: 1,
        modes: vec![PackSaveMode::Legacy, PackSaveMode::Progress],
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--label" => {
                config.label = args.next().ok_or("--label requires a value")?;
            }
            "--system" => {
                config.system = args.next().ok_or("--system requires an id")?;
            }
            "--image-size" | "--size" => {
                config.image_size = args.next().ok_or("--image-size requires WxH")?;
            }
            "--asset-dir" => {
                config.asset_dir = PathBuf::from(args.next().ok_or("--asset-dir requires a path")?);
            }
            "--size-bytes" => {
                config.size_bytes = args
                    .next()
                    .ok_or("--size-bytes requires a byte count")?
                    .parse::<u64>()
                    .map_err(|e| format!("invalid --size-bytes: {e}"))?;
            }
            "--iterations" => {
                config.iterations = args
                    .next()
                    .ok_or("--iterations requires a count")?
                    .parse::<usize>()
                    .map_err(|e| format!("invalid --iterations: {e}"))?;
            }
            "--modes" => {
                config.modes = parse_modes(&args.next().ok_or("--modes requires a list")?)?;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if config.iterations == 0 {
        return Err("--iterations must be greater than zero".to_string());
    }
    if config.size_bytes == 0 {
        return Err("--size-bytes must be greater than zero".to_string());
    }
    if !valid_image_size(&config.image_size) {
        return Err(format!("invalid image size: {}", config.image_size));
    }
    if !config
        .label
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err("--label must contain only letters, numbers, _, ., or -".to_string());
    }
    Ok(config)
}

fn print_usage() {
    println!(
        "usage: mister-magik-fb media-bench-save --label LABEL --system ID --iterations N --modes old,progress"
    );
}

fn parse_modes(value: &str) -> Result<Vec<PackSaveMode>, String> {
    let modes: Result<Vec<_>, _> = value
        .split(',')
        .map(|part| match part.trim() {
            "old" | "legacy" => Ok(PackSaveMode::Legacy),
            "progress" | "chunked" => Ok(PackSaveMode::Progress),
            "" => Err("empty save benchmark mode".to_string()),
            other => Err(format!("unsupported save benchmark mode: {other}")),
        })
        .collect();
    let modes = modes?;
    if modes.is_empty() {
        Err("--modes must include at least one mode".to_string())
    } else {
        Ok(modes)
    }
}

fn resolve_source_path(config: &BenchConfig) -> Result<PathBuf, String> {
    let local_pack = PathBuf::from(size_qualified_pack_path(
        &config.asset_dir.display().to_string(),
        &config.system,
        &config.image_size,
    )?);
    if local_pack.exists() {
        return Ok(local_pack);
    }
    let work_dir = PathBuf::from("/tmp/mister-magik-media-save-bench");
    fs::create_dir_all(&work_dir)
        .map_err(|e| format!("create save bench work dir {}: {e}", work_dir.display()))?;
    let source = work_dir.join(format!(
        "{}-{}-{}.source",
        config.system, config.image_size, config.size_bytes
    ));
    if source.metadata().map(|meta| meta.len()).ok() != Some(config.size_bytes) {
        write_deterministic_source(&source, config.size_bytes)?;
    }
    Ok(source)
}

fn write_deterministic_source(path: &Path, size_bytes: u64) -> Result<(), String> {
    let mut file = File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut written = 0u64;
    let mut buffer = vec![0u8; PROGRESS_COPY_CHUNK_BYTES];
    for (idx, byte) in buffer.iter_mut().enumerate() {
        *byte = (idx % 251) as u8;
    }
    while written < size_bytes {
        let remaining = (size_bytes - written).min(buffer.len() as u64) as usize;
        file.write_all(&buffer[..remaining])
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        written += remaining as u64;
    }
    file.sync_all()
        .map_err(|e| format!("sync {}: {e}", path.display()))?;
    Ok(())
}

fn run_one(config: &BenchConfig, source: &Path, mode: PackSaveMode, iteration: usize) -> SaveRow {
    let mut row = SaveRow {
        label: config.label.clone(),
        system: config.system.clone(),
        mode,
        iteration,
        metrics: PackSaveMetrics::default(),
        result: "bench-ok".to_string(),
    };
    let final_path = bench_final_path(config, mode, iteration);
    let result = publish_pack_file_for_bench(source, &final_path, mode, |_| {});
    match result {
        Ok(metrics) => row.metrics = metrics,
        Err(error) => row.result = error,
    }
    let _ = fs::remove_file(&final_path);
    let _ = fs::remove_file(temp_path_for(&final_path));
    row
}

fn bench_final_path(config: &BenchConfig, mode: PackSaveMode, iteration: usize) -> PathBuf {
    config.asset_dir.join(format!(
        ".{}-{}-save-bench-{}-{}-{}.mmlz4b",
        config.system,
        config.image_size,
        mode.label(),
        iteration,
        unix_ms_now()
    ))
}

fn default_label() -> String {
    format!("screenshot-save-{}", unix_ms_now())
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

impl SaveRow {
    fn to_tsv(&self) -> String {
        format!(
            "screenshot_save_bench_tsv\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.label,
            self.system,
            self.mode.label(),
            self.iteration,
            self.metrics.bytes,
            self.metrics.copy_ms,
            self.metrics.sync_ms,
            self.metrics.rename_ms,
            self.metrics.parent_sync_ms,
            self.metrics.total_ms,
            self.metrics.progress_events,
            tsv(&self.result),
        )
    }
}

fn tsv(value: &str) -> String {
    value
        .replace('\t', " ")
        .replace('\r', "")
        .replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_save_benchmark_args() {
        let config = parse_args([
            "--label".to_string(),
            "SAVE-20260623".to_string(),
            "--system".to_string(),
            "neogeo".to_string(),
            "--iterations".to_string(),
            "10".to_string(),
            "--modes".to_string(),
            "old,progress".to_string(),
            "--size-bytes".to_string(),
            "1234".to_string(),
        ])
        .unwrap();

        assert_eq!(config.label, "SAVE-20260623");
        assert_eq!(config.system, "neogeo");
        assert_eq!(config.iterations, 10);
        assert_eq!(config.modes, [PackSaveMode::Legacy, PackSaveMode::Progress]);
        assert_eq!(config.size_bytes, 1234);
    }

    #[test]
    fn rejects_unknown_save_benchmark_mode() {
        let error = parse_args(["--modes".to_string(), "old,fast".to_string()]).unwrap_err();

        assert!(error.contains("unsupported save benchmark mode"));
    }
}
