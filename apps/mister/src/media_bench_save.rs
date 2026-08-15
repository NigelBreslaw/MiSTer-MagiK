// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::artifact_publish::{
    ArtifactPublishLabels, prepare_artifact_publish, sync_path_rust_best_effort,
    timestamped_temp_path_for,
};
use crate::media_pack_save::{
    PROGRESS_COPY_CHUNK_BYTES, PackSaveMetrics, publish_pack_file_for_bench, temp_path_for,
};
use mister_magik_fb::media_update::{
    DEFAULT_ASSET_DIR, DEFAULT_IMAGE_SIZE, index_path_for_pack_path, size_qualified_pack_path,
    state_path, valid_image_size,
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
    artifact: BenchArtifact,
}

#[derive(Clone, Debug)]
struct SaveRow {
    label: String,
    system: String,
    iteration: usize,
    artifact: BenchArtifact,
    metrics: PackSaveMetrics,
    result: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BenchArtifact {
    Pack,
    Index,
    State,
}

impl BenchArtifact {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pack" => Ok(Self::Pack),
            "index" => Ok(Self::Index),
            "state" => Ok(Self::State),
            other => Err(format!("unsupported --artifact: {other}")),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Pack => "progress",
            Self::Index => "index",
            Self::State => "state",
        }
    }
}

pub fn run() {
    match run_inner(std::env::args().skip(2)) {
        Ok(()) => {}
        Err(error) => {
            crate::ui_errln!("media-bench-save failed: {error}");
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
    crate::ui_logln!("{HEADER}");
    for iteration in 1..=config.iterations {
        let row = run_one(&config, &source, iteration);
        crate::ui_logln!("{}", row.to_tsv());
        if row.result != "bench-ok" {
            return Err(row.to_tsv());
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
        artifact: BenchArtifact::Pack,
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
            "--artifact" => {
                config.artifact = BenchArtifact::parse(
                    &args.next().ok_or("--artifact requires pack|index|state")?,
                )?;
            }
            "--modes" => {
                return Err(
                    "--modes was removed; media save has one progress-capable path".to_string(),
                );
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
    crate::ui_logln!(
        "usage: mister-magik-fb media-bench-save --label LABEL --system ID --iterations N [--artifact pack|index|state]"
    );
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

fn run_one(config: &BenchConfig, source: &Path, iteration: usize) -> SaveRow {
    let mut row = SaveRow {
        label: config.label.clone(),
        system: config.system.clone(),
        iteration,
        artifact: config.artifact,
        metrics: PackSaveMetrics::default(),
        result: "bench-ok".to_string(),
    };
    let final_path = bench_final_path(config, iteration);
    let result = match config.artifact {
        BenchArtifact::Pack => publish_pack_file_for_bench(source, &final_path, |_| {}),
        BenchArtifact::Index => publish_index_for_bench(source, &final_path),
        BenchArtifact::State => publish_state_for_bench(config, iteration),
    };
    match result {
        Ok(metrics) => row.metrics = metrics,
        Err(error) => row.result = error,
    }
    let _ = fs::remove_file(&final_path);
    let _ = fs::remove_file(index_path_for_pack_path(&final_path));
    let _ = fs::remove_file(bench_state_path(&config.asset_dir, iteration));
    let _ = fs::remove_file(temp_path_for(&final_path));
    row
}

fn bench_final_path(config: &BenchConfig, iteration: usize) -> PathBuf {
    config.asset_dir.join(format!(
        ".{}-{}-save-bench-progress-{}-{}.mmlz4b",
        config.system,
        config.image_size,
        iteration,
        unix_ms_now()
    ))
}

fn observe_media_fault(point: &str, path: &Path) {
    let mut fault_control =
        mister_magik_mister_runtime::direct_reset_fault::process_fault_control();
    mister_magik_catalog::fs_fault::maybe_fault_with_control(point, path, &mut fault_control);
}

fn publish_index_for_bench(source: &Path, pack_path: &Path) -> Result<PackSaveMetrics, String> {
    let final_path = index_path_for_pack_path(pack_path);
    let publish = prepare_artifact_publish(
        &final_path,
        timestamped_temp_path_for(&final_path, "screenshot-pack-index", unix_ms_now()),
        ArtifactPublishLabels {
            destination: "bench index destination",
            parent: "bench index parent",
        },
    )?;
    let mut input = File::open(source).map_err(|e| format!("open {}: {e}", source.display()))?;
    let mut output = File::create(publish.temp_path())
        .map_err(|e| format!("create {}: {e}", publish.temp_path().display()))?;
    let bytes = std::io::copy(&mut input, &mut output)
        .map_err(|e| format!("copy bench index {}: {e}", publish.temp_path().display()))?;
    observe_media_fault("media.index.after_temp_write", &final_path);
    output
        .sync_all()
        .map_err(|e| format!("sync bench index {}: {e}", publish.temp_path().display()))?;
    observe_media_fault("media.index.after_temp_sync", &final_path);
    drop(output);
    publish.install_temp(Some("bench index"))?;
    observe_media_fault("media.index.after_rename_before_parent_sync", &final_path);
    sync_path_rust_best_effort(publish.parent());
    Ok(PackSaveMetrics {
        bytes,
        progress_events: 3,
        ..Default::default()
    })
}

fn publish_state_for_bench(
    config: &BenchConfig,
    iteration: usize,
) -> Result<PackSaveMetrics, String> {
    let path = bench_state_path(&config.asset_dir, iteration);
    let publish = prepare_artifact_publish(
        &path,
        timestamped_temp_path_for(&path, "media-state", unix_ms_now()),
        ArtifactPublishLabels {
            destination: "bench media state",
            parent: "bench media state parent",
        },
    )?;
    let text = format!(
        "{{\n  \"schema\": 1,\n  \"bench\": \"media-bench-save\",\n  \"system\": \"{}\",\n  \"image_size\": \"{}\",\n  \"iteration\": {}\n}}\n",
        config.system, config.image_size, iteration
    );
    fs::write(publish.temp_path(), text.as_bytes()).map_err(|e| {
        format!(
            "write bench media state {}: {e}",
            publish.temp_path().display()
        )
    })?;
    observe_media_fault("media.state.after_temp_write", &path);
    File::open(publish.temp_path())
        .and_then(|file| file.sync_all())
        .map_err(|e| {
            format!(
                "sync bench media state {}: {e}",
                publish.temp_path().display()
            )
        })?;
    observe_media_fault("media.state.after_temp_sync", &path);
    publish.install_temp(Some("bench media state"))?;
    observe_media_fault("media.state.after_rename_before_parent_sync", &path);
    sync_path_rust_best_effort(publish.parent());
    Ok(PackSaveMetrics {
        bytes: text.len() as u64,
        progress_events: 3,
        ..Default::default()
    })
}

fn bench_state_path(asset_dir: &Path, iteration: usize) -> PathBuf {
    PathBuf::from(format!(
        "{}.save-bench-{iteration}-{}",
        state_path(&asset_dir.display().to_string()),
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
            self.artifact.label(),
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
            "--size-bytes".to_string(),
            "1234".to_string(),
        ])
        .unwrap();

        assert_eq!(config.label, "SAVE-20260623");
        assert_eq!(config.system, "neogeo");
        assert_eq!(config.iterations, 10);
        assert_eq!(config.size_bytes, 1234);
        assert_eq!(config.artifact, BenchArtifact::Pack);
    }

    #[test]
    fn parses_save_benchmark_artifact_modes() {
        let index = parse_args(["--artifact".to_string(), "index".to_string()]).unwrap();
        let state = parse_args(["--artifact".to_string(), "state".to_string()]).unwrap();

        assert_eq!(index.artifact, BenchArtifact::Index);
        assert_eq!(state.artifact, BenchArtifact::State);
    }

    #[test]
    fn rejects_removed_save_benchmark_mode_option() {
        let error = parse_args(["--modes".to_string(), "unsupported".to_string()]).unwrap_err();

        assert!(error.contains("--modes was removed"));
    }
}
