use mister_magik_fb::media_update::{
    size_qualified_pack_path, valid_image_size, DEFAULT_ASSET_DIR, DEFAULT_IMAGE_SIZE,
};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HEADER: &str = "screenshot_save_bench_tsv\tlabel\tsystem\tmode\titeration\tbytes\tcopy_ms\tsync_ms\trename_ms\tparent_sync_ms\ttotal_ms\tprogress_events\tresult";
const DEFAULT_SIZE_BYTES: u64 = 24 * 1024 * 1024;
const PROGRESS_COPY_CHUNK_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveMode {
    Old,
    Progress,
}

impl SaveMode {
    fn label(self) -> &'static str {
        match self {
            Self::Old => "old",
            Self::Progress => "progress",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BenchConfig {
    label: String,
    system: String,
    image_size: String,
    asset_dir: PathBuf,
    size_bytes: u64,
    iterations: usize,
    modes: Vec<SaveMode>,
}

#[derive(Clone, Debug, Default)]
struct SaveMetrics {
    bytes: u64,
    copy_ms: u64,
    sync_ms: u64,
    rename_ms: u64,
    parent_sync_ms: u64,
    total_ms: u64,
    progress_events: u64,
}

#[derive(Clone, Debug)]
struct SaveRow {
    label: String,
    system: String,
    mode: SaveMode,
    iteration: usize,
    metrics: SaveMetrics,
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
        modes: vec![SaveMode::Old, SaveMode::Progress],
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

fn parse_modes(value: &str) -> Result<Vec<SaveMode>, String> {
    let modes: Result<Vec<_>, _> = value
        .split(',')
        .map(|part| match part.trim() {
            "old" | "legacy" => Ok(SaveMode::Old),
            "progress" | "chunked" => Ok(SaveMode::Progress),
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

fn run_one(config: &BenchConfig, source: &Path, mode: SaveMode, iteration: usize) -> SaveRow {
    let mut row = SaveRow {
        label: config.label.clone(),
        system: config.system.clone(),
        mode,
        iteration,
        metrics: SaveMetrics::default(),
        result: "bench-ok".to_string(),
    };
    let final_path = bench_final_path(config, mode, iteration);
    let result = publish_save_bench(source, &final_path, mode);
    match result {
        Ok(metrics) => row.metrics = metrics,
        Err(error) => row.result = error,
    }
    let _ = fs::remove_file(&final_path);
    let _ = fs::remove_file(temp_path_for(&final_path));
    row
}

fn publish_save_bench(
    source: &Path,
    final_path: &Path,
    mode: SaveMode,
) -> Result<SaveMetrics, String> {
    let parent = final_path
        .parent()
        .ok_or_else(|| format!("bench destination has no parent: {}", final_path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("create bench destination parent {}: {e}", parent.display()))?;
    let tmp = temp_path_for(final_path);
    let _ = fs::remove_file(&tmp);
    let _ = fs::remove_file(final_path);
    let started = Instant::now();
    let mut metrics = SaveMetrics {
        bytes: file_len(source)?,
        ..Default::default()
    };
    let mut input = File::open(source).map_err(|e| format!("open {}: {e}", source.display()))?;
    let mut output = File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;

    let copy_started = Instant::now();
    metrics.progress_events = match mode {
        SaveMode::Old => copy_old(&mut input, &mut output)?,
        SaveMode::Progress => copy_with_progress(&mut input, &mut output)?,
    };
    metrics.copy_ms = elapsed_ms(copy_started.elapsed());

    let sync_started = Instant::now();
    output
        .sync_all()
        .map_err(|e| format!("sync {}: {e}", tmp.display()))?;
    metrics.sync_ms = elapsed_ms(sync_started.elapsed());
    drop(output);

    let rename_started = Instant::now();
    fs::rename(&tmp, final_path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename {} -> {}: {e}", tmp.display(), final_path.display())
    })?;
    metrics.rename_ms = elapsed_ms(rename_started.elapsed());

    let parent_sync_started = Instant::now();
    sync_path(parent);
    metrics.parent_sync_ms = elapsed_ms(parent_sync_started.elapsed());
    metrics.total_ms = elapsed_ms(started.elapsed());
    Ok(metrics)
}

fn copy_old(input: &mut File, output: &mut File) -> Result<u64, String> {
    std::io::copy(input, output).map_err(|e| format!("legacy copy failed: {e}"))?;
    Ok(0)
}

fn copy_with_progress(input: &mut File, output: &mut File) -> Result<u64, String> {
    let mut progress_events = 0u64;
    let mut buffer = vec![0u8; PROGRESS_COPY_CHUNK_BYTES];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|e| format!("progress copy read failed: {e}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|e| format!("progress copy write failed: {e}"))?;
        progress_events += 1;
    }
    Ok(progress_events)
}

fn bench_final_path(config: &BenchConfig, mode: SaveMode, iteration: usize) -> PathBuf {
    config.asset_dir.join(format!(
        ".{}-{}-save-bench-{}-{}-{}.mmlz4b",
        config.system,
        config.image_size,
        mode.label(),
        iteration,
        unix_ms_now()
    ))
}

fn temp_path_for(final_path: &Path) -> PathBuf {
    final_path.with_file_name(format!(
        "{}.tmp",
        final_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("save-bench")
    ))
}

fn file_len(path: &Path) -> Result<u64, String> {
    path.metadata()
        .map(|meta| meta.len())
        .map_err(|e| format!("stat {}: {e}", path.display()))
}

fn sync_path(path: &Path) {
    match Command::new("sync").arg(path).status() {
        Ok(status) if status.success() => {}
        _ => {
            let _ = Command::new("sync").status();
        }
    };
}

fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
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

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

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
        assert_eq!(config.modes, [SaveMode::Old, SaveMode::Progress]);
        assert_eq!(config.size_bytes, 1234);
    }

    #[test]
    fn rejects_unknown_save_benchmark_mode() {
        let error = parse_args(["--modes".to_string(), "old,fast".to_string()]).unwrap_err();

        assert!(error.contains("unsupported save benchmark mode"));
    }

    #[test]
    fn old_and_progress_modes_write_identical_bytes() {
        let dir = temp_dir("mister-magik-save-bench");
        let source = dir.join("source.bin");
        fs::write(&source, b"abcdef0123456789").unwrap();

        for mode in [SaveMode::Old, SaveMode::Progress] {
            let final_path = dir.join(format!("out-{}.bin", mode.label()));
            let metrics = publish_save_bench(&source, &final_path, mode).unwrap();
            assert_eq!(fs::read(&final_path).unwrap(), b"abcdef0123456789");
            assert_eq!(metrics.bytes, 16);
            if mode == SaveMode::Progress {
                assert!(metrics.progress_events > 0);
            } else {
                assert_eq!(metrics.progress_events, 0);
            }
        }

        let _ = fs::remove_dir_all(dir);
    }
}
