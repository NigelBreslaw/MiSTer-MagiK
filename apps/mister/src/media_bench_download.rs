// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::artifact_publish::{
    ArtifactPublishLabels, prepare_artifact_publish, sync_path_rust_best_effort,
    timestamped_temp_path_for,
};
use crate::media_pack_save::{
    PackSaveMetrics, cleanup_pack_publish_temps, publish_pack_file_with_progress,
};
use mister_magik_fb::media_update::{
    DEFAULT_ASSET_DIR, DEFAULT_IMAGE_SIZE, DEFAULT_MANIFEST_URL, MediaPack, MediaVariant,
    parse_manifest_json, size_qualified_pack_path, state_path, valid_image_size,
};
use mister_magik_media_contract::{MEDIA_CONNECT_TIMEOUT_SECS, MEDIA_TRANSFER_TIMEOUT_SECS};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HEADER: &str = "screenshot_download_bench_tsv\tlabel\tsystem\tvariant\tencoded_bytes\tdecoded_bytes\tdownload_ms\tdecompress_ms\tsave_ms\tverify_ms\ttotal_ms\twire_mbps\tdecoded_mbps\tetag\tcontent_encoding\tcf_cache_status\tresult";

#[derive(Clone, Debug, PartialEq, Eq)]
struct BenchConfig {
    manifest_url: String,
    system: String,
    variant: String,
    iterations: usize,
    label: String,
    image_size: String,
    asset_dir: PathBuf,
    prime_cache: bool,
    save_strategy: SaveStrategy,
}

#[derive(Clone, Debug)]
struct BenchRow {
    label: String,
    system: String,
    variant: String,
    encoded_bytes: u64,
    decoded_bytes: u64,
    download_ms: u64,
    decompress_ms: u64,
    save_ms: u64,
    verify_ms: u64,
    total_ms: u64,
    wire_mbps: f64,
    decoded_mbps: f64,
    etag: String,
    content_encoding: String,
    cf_cache_status: String,
    result: String,
}

#[derive(Clone, Debug, Default)]
struct HttpMetadata {
    etag: String,
    content_encoding: String,
    cf_cache_status: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveStrategy {
    Staged,
    StreamFat,
}

impl SaveStrategy {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "staged" | "stage" | "current" => Ok(Self::Staged),
            "stream-fat" | "stream" | "direct" => Ok(Self::StreamFat),
            other => Err(format!("unsupported --save-strategy: {other}")),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::StreamFat => "stream-fat",
        }
    }
}

pub fn run() {
    match run_inner(std::env::args().skip(2)) {
        Ok(()) => {}
        Err(error) => {
            crate::ui_errln!("media-bench-download failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run_inner<I>(args: I) -> Result<(), String>
where
    I: IntoIterator<Item = String>,
{
    let config = parse_args(args)?;
    let manifest_text = fetch_text(&config.manifest_url)?;
    let manifest = parse_manifest_json(&config.manifest_url, &manifest_text)?;
    let mut packs: Vec<_> = manifest
        .packs
        .iter()
        .filter(|pack| {
            pack.image_size == config.image_size
                && (matches!(config.system.as_str(), "all" | "representative")
                    || pack.id == config.system)
        })
        .collect();
    if config.system == "representative" {
        packs = representative_packs(packs);
    }
    if packs.is_empty() {
        return Err(format!(
            "manifest has no packs for system={} image_size={}",
            config.system, config.image_size
        ));
    }
    crate::ui_logln!("{HEADER}");
    let rss_before_kb = proc_status_kb("VmRSS");
    let hwm_before_kb = proc_status_kb("VmHWM");
    let mut reports = Vec::new();
    for pack in packs {
        let local_path = PathBuf::from(size_qualified_pack_path(
            &config.asset_dir.display().to_string(),
            &pack.id,
            &pack.image_size,
        )?);
        let variant = pack
            .variant_for_compression("none")
            .ok_or_else(|| format!("pack {} has no raw identity variant", pack.id))?;
        if config.prime_cache {
            let row = run_one(&config, pack, variant, &local_path, "prime-cache")?;
            crate::ui_errln!(
                "media_bench_prime\tsystem={}\tvariant={}\tcf_cache_status={}\ttotal_ms={}",
                pack.id,
                config.variant,
                row.cf_cache_status,
                row.total_ms
            );
        }
        for iteration in 1..=config.iterations {
            let label = format!("{}-{:02}", config.label, iteration);
            let row = run_one(&config, pack, variant, &local_path, &label)?;
            crate::ui_logln!("{}", row.to_tsv());
            reports.push(row.to_json(pack, config.save_strategy));
        }
    }
    crate::ui_logln!(
        "{}",
        serde_json::json!({
            "schema": "mister-magik-media-pack-persistence-v1",
            "status": "passed",
            "save_strategy": config.save_strategy.label(),
            "production_format": "raw-mmlz4b",
            "decode_ms": 0,
            "representative_policy": if config.system == "representative" {
                "small-median-largest"
            } else {
                "explicit-system-selection"
            },
            "row_count": reports.len(),
            "rows": reports,
            "process": {
                "rss_before_kb": rss_before_kb,
                "rss_after_kb": proc_status_kb("VmRSS"),
                "hwm_before_kb": hwm_before_kb,
                "hwm_after_kb": proc_status_kb("VmHWM"),
            },
        })
    );
    Ok(())
}

fn representative_packs(mut packs: Vec<&MediaPack>) -> Vec<&MediaPack> {
    packs.sort_by(|left, right| {
        left.raw
            .bytes
            .cmp(&right.raw.bytes)
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut selected = Vec::with_capacity(3.min(packs.len()));
    for index in [0, packs.len() / 2, packs.len().saturating_sub(1)] {
        if let Some(pack) = packs.get(index)
            && selected
                .iter()
                .all(|selected: &&MediaPack| selected.id != pack.id)
        {
            selected.push(*pack);
        }
    }
    selected
}

fn parse_args<I>(args: I) -> Result<BenchConfig, String>
where
    I: IntoIterator<Item = String>,
{
    let mut config = BenchConfig {
        manifest_url: std::env::var("MISTER_MEDIA_MANIFEST_URL")
            .unwrap_or_else(|_| DEFAULT_MANIFEST_URL.to_string()),
        system: "arcade".to_string(),
        variant: "identity".to_string(),
        iterations: 1,
        label: default_label(),
        image_size: std::env::var("MISTER_MEDIA_SIZE")
            .unwrap_or_else(|_| DEFAULT_IMAGE_SIZE.to_string()),
        asset_dir: PathBuf::from(
            std::env::var("MISTER_MEDIA_ASSET_DIR")
                .unwrap_or_else(|_| DEFAULT_ASSET_DIR.to_string()),
        ),
        prime_cache: false,
        save_strategy: SaveStrategy::Staged,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest-url" => {
                config.manifest_url = args.next().ok_or("--manifest-url requires a URL")?;
            }
            "--system" => {
                config.system = args
                    .next()
                    .ok_or("--system requires id|all|representative")?;
            }
            "--variants" => {
                config.variant =
                    parse_identity_variant(&args.next().ok_or("--variants requires a list")?)?;
            }
            "--variant" => {
                config.variant =
                    parse_identity_variant(&args.next().ok_or("--variant requires a value")?)?;
            }
            "--iterations" => {
                config.iterations = args
                    .next()
                    .ok_or("--iterations requires a count")?
                    .parse::<usize>()
                    .map_err(|e| format!("invalid --iterations: {e}"))?;
            }
            "--label" => {
                config.label = args.next().ok_or("--label requires a value")?;
            }
            "--image-size" | "--size" => {
                config.image_size = args.next().ok_or("--image-size requires WxH")?;
            }
            "--asset-dir" => {
                config.asset_dir = PathBuf::from(args.next().ok_or("--asset-dir requires a path")?);
            }
            "--prime-cache" => config.prime_cache = true,
            "--save-strategy" => {
                config.save_strategy =
                    SaveStrategy::parse(&args.next().ok_or("--save-strategy requires a value")?)?;
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
        "usage: mister-magik-fb media-bench-download --system ID|all|representative [--variant identity] --iterations N [--prime-cache] [--save-strategy staged|stream-fat]"
    );
}

fn parse_identity_variant(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.contains(',') {
        let mut parsed = value.split(',');
        let first = parsed.next().unwrap_or_default().trim();
        if parsed.all(|part| part.trim().is_empty()) {
            return parse_identity_variant(first);
        }
        return Err(
            "MagiK only supports the raw identity screenshot benchmark variant".to_string(),
        );
    }
    match value {
        "identity" | "none" | "plain" => Ok("identity".to_string()),
        other => Err(format!(
            "unsupported variant {other}: MagiK only benchmarks raw identity downloads"
        )),
    }
}

fn fetch_text(url: &str) -> Result<String, String> {
    let fetched = crate::media_http::fetch_manifest(url)?;
    String::from_utf8(fetched.bytes).map_err(|e| format!("manifest utf8: {e}"))
}

fn run_one(
    config: &BenchConfig,
    pack: &MediaPack,
    variant: &MediaVariant,
    local_path: &Path,
    label: &str,
) -> Result<BenchRow, String> {
    fs::create_dir_all(&config.asset_dir)
        .map_err(|e| format!("create asset dir {}: {e}", config.asset_dir.display()))?;
    let work_dir = PathBuf::from("/tmp/mister-magik-media-bench");
    fs::create_dir_all(&work_dir)
        .map_err(|e| format!("create work dir {}: {e}", work_dir.display()))?;
    let stamp = format!("{}-{}", std::process::id(), unix_ms_now());
    let encoded = work_dir.join(format!("{}.identity.{stamp}.encoded", pack.id));
    let bench_final = bench_final_path(local_path, &stamp);
    let bench_state = bench_state_path(&config.asset_dir, &stamp);
    let started = Instant::now();
    let mut row = BenchRow {
        label: label.to_string(),
        system: pack.id.clone(),
        variant: "identity".to_string(),
        encoded_bytes: 0,
        decoded_bytes: 0,
        download_ms: 0,
        decompress_ms: 0,
        save_ms: 0,
        verify_ms: 0,
        total_ms: 0,
        wire_mbps: 0.0,
        decoded_mbps: 0.0,
        etag: String::new(),
        content_encoding: String::new(),
        cf_cache_status: String::new(),
        result: "bench-ok".to_string(),
    };
    let result = (|| {
        let metadata = match config.save_strategy {
            SaveStrategy::Staged => {
                run_staged_download_publish(pack, variant, &encoded, &bench_final, label, &mut row)?
            }
            SaveStrategy::StreamFat => {
                run_stream_fat_download_publish(variant, &bench_final, label, pack, &mut row)?
            }
        };

        let state_started = Instant::now();
        write_bench_download_state(&bench_state, pack, &bench_final, variant, &metadata)?;
        let state_ms = elapsed_ms(state_started.elapsed());
        emit_stage_row(
            label,
            pack,
            "state",
            state_ms,
            file_len(&bench_state).unwrap_or(0),
            "bench-ok",
            &format!(
                "save_strategy={} path={}",
                config.save_strategy.label(),
                bench_state.display()
            ),
        );
        row.save_ms += state_ms;
        Ok::<(), String>(())
    })();
    let cleanup_started = Instant::now();
    let cleanup_result = cleanup_bench_artifacts(&encoded, &bench_final, &bench_state);
    let cleanup_ms = elapsed_ms(cleanup_started.elapsed());
    let cleanup_detail = cleanup_result
        .as_ref()
        .map(|removed| format!("removed={removed}"))
        .unwrap_or_else(|error| tsv(error));
    emit_stage_row(
        label,
        pack,
        "cleanup",
        cleanup_ms,
        0,
        cleanup_result
            .as_ref()
            .map(|_| "bench-ok")
            .unwrap_or("bench-cleanup-failed"),
        &format!(
            "save_strategy={} {cleanup_detail}",
            config.save_strategy.label()
        ),
    );
    if let Err(error) = result {
        row.result = error;
    } else if let Err(error) = cleanup_result {
        row.result = error;
    } else {
        row.save_ms += cleanup_ms;
    }
    row.total_ms = elapsed_ms(started.elapsed());
    row.wire_mbps = mbps(row.encoded_bytes, row.download_ms);
    row.decoded_mbps = mbps(row.decoded_bytes, row.total_ms);
    if row.result != "bench-ok" {
        return Err(row.to_tsv());
    }
    Ok(row)
}

fn run_staged_download_publish(
    pack: &MediaPack,
    variant: &MediaVariant,
    encoded: &Path,
    bench_final: &Path,
    label: &str,
    row: &mut BenchRow,
) -> Result<HttpMetadata, String> {
    let headers_path = encoded.with_extension("headers");
    let stream = stream_fat_download_to_publish_temp(variant, encoded, &headers_path);
    let _ = fs::remove_file(&headers_path);
    let stream = stream?;
    let metadata = stream.metadata;
    row.download_ms = stream.download_ms;
    row.verify_ms = stream.verify_ms;
    row.etag = metadata.etag.clone();
    row.content_encoding = metadata.content_encoding.clone();
    row.cf_cache_status = metadata.cf_cache_status.clone();
    row.encoded_bytes = stream.bytes;

    verify_downloaded_bytes(stream.bytes, &stream.sha256, variant.bytes, &variant.sha256)?;

    row.decompress_ms = 0;
    row.decoded_bytes = file_len(encoded)?;

    let verify_started = Instant::now();
    verify_file(encoded, pack.raw.bytes, &pack.raw.sha256)?;
    row.verify_ms += elapsed_ms(verify_started.elapsed());

    let publish_metrics = publish_pack_file_with_progress(encoded, bench_final, |_| {})?;
    emit_publish_stage_rows(label, pack, SaveStrategy::Staged, &publish_metrics);
    row.save_ms += publish_metrics.total_ms;
    Ok(metadata)
}

fn run_stream_fat_download_publish(
    variant: &MediaVariant,
    bench_final: &Path,
    label: &str,
    pack: &MediaPack,
    row: &mut BenchRow,
) -> Result<HttpMetadata, String> {
    let publish = prepare_artifact_publish(
        bench_final,
        timestamped_temp_path_for(bench_final, "screenshot-pack", unix_ms_now()),
        ArtifactPublishLabels {
            destination: "streamed benchmark pack",
            parent: "streamed benchmark pack parent",
        },
    )?;
    let headers_dir = PathBuf::from("/tmp/mister-magik-media-bench");
    fs::create_dir_all(&headers_dir)
        .map_err(|e| format!("create headers dir {}: {e}", headers_dir.display()))?;
    let headers_path =
        headers_dir.join(format!("{}.stream-fat.{}.headers", pack.id, unix_ms_now()));
    let result = stream_fat_download_to_publish_temp(variant, publish.temp_path(), &headers_path);
    let _ = fs::remove_file(&headers_path);
    let stream = result?;
    row.download_ms = stream.download_ms;
    row.verify_ms = stream.verify_ms;
    row.decompress_ms = 0;
    row.encoded_bytes = stream.bytes;
    row.decoded_bytes = stream.bytes;
    row.etag = stream.metadata.etag.clone();
    row.content_encoding = stream.metadata.content_encoding.clone();
    row.cf_cache_status = stream.metadata.cf_cache_status.clone();

    emit_stage_row(
        label,
        pack,
        "stream_write",
        stream.download_ms,
        stream.bytes,
        "bench-ok",
        &format!("save_strategy={}", SaveStrategy::StreamFat.label()),
    );
    emit_stage_row(
        label,
        pack,
        "stream_verify_finalize",
        stream.verify_ms,
        stream.bytes,
        "bench-ok",
        &format!("save_strategy={}", SaveStrategy::StreamFat.label()),
    );
    verify_downloaded_bytes(
        stream.bytes,
        &stream.sha256,
        pack.raw.bytes,
        &pack.raw.sha256,
    )?;
    let save_metrics = install_verified_streamed_temp(
        &publish,
        stream.bytes,
        &stream.sha256,
        variant.bytes,
        &variant.sha256,
    )?;
    emit_stream_publish_stage_rows(label, pack, &save_metrics);
    row.save_ms += save_metrics.total_ms;
    Ok(stream.metadata)
}

struct StreamDownloadResult {
    metadata: HttpMetadata,
    bytes: u64,
    sha256: String,
    download_ms: u64,
    verify_ms: u64,
}

#[derive(Debug)]
struct StreamPublishMetrics {
    bytes: u64,
    sync_ms: u64,
    rename_ms: u64,
    parent_sync_ms: u64,
    total_ms: u64,
}

fn stream_fat_download_to_publish_temp(
    variant: &MediaVariant,
    temp_path: &Path,
    headers_path: &Path,
) -> Result<StreamDownloadResult, String> {
    let mut curl = Command::new("curl");
    add_curl_download_args(&mut curl, &variant.url, Some(headers_path), variant.bytes);
    let mut curl = curl
        .arg("-o")
        .arg("-")
        .arg(&variant.url)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn curl: {e}"))?;
    let mut sha = match Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(sha) => sha,
        Err(error) => {
            terminate_and_reap(&mut curl);
            return Err(format!("spawn sha256sum: {error}"));
        }
    };
    let mut output = match File::create(temp_path) {
        Ok(output) => output,
        Err(error) => {
            terminate_and_reap(&mut curl);
            drop(sha.stdin.take());
            let _ = sha.wait();
            return Err(format!("create {}: {error}", temp_path.display()));
        }
    };
    let mut input = match curl.stdout.take() {
        Some(input) => input,
        None => {
            terminate_and_reap(&mut curl);
            drop(sha.stdin.take());
            let _ = sha.wait();
            return Err("missing curl stdout pipe".to_string());
        }
    };
    let mut sha_stdin = match sha.stdin.take() {
        Some(stdin) => stdin,
        None => {
            terminate_and_reap(&mut curl);
            let _ = sha.wait();
            return Err("missing sha256sum stdin pipe".to_string());
        }
    };
    let started = Instant::now();
    let mut bytes = 0u64;
    let mut buffer = vec![0u8; 256 * 1024];
    let transfer_result = (|| {
        loop {
            let read = input
                .read(&mut buffer)
                .map_err(|e| format!("read curl stdout: {e}"))?;
            if read == 0 {
                break;
            }
            bytes = crate::media_http::write_bounded_stream_chunk(
                &mut output,
                &mut sha_stdin,
                &buffer[..read],
                bytes,
                variant.bytes,
                "benchmark pack",
            )?;
        }
        output
            .flush()
            .map_err(|e| format!("flush streamed pack {}: {e}", temp_path.display()))
    })();
    drop(sha_stdin);
    drop(output);
    drop(input);
    if transfer_result.is_err() {
        let _ = curl.kill();
    }
    let download_ms = elapsed_ms(started.elapsed());
    let curl_status = curl.wait().map_err(|e| format!("wait curl: {e}"))?;
    transfer_result?;
    if !curl_status.success() {
        return Err(format!("download-failed-{curl_status}"));
    }
    let verify_started = Instant::now();
    let sha_output = sha
        .wait_with_output()
        .map_err(|e| format!("wait sha256sum: {e}"))?;
    let verify_ms = elapsed_ms(verify_started.elapsed());
    if !sha_output.status.success() {
        return Err(format!("sha256sum failed with {}", sha_output.status));
    }
    let actual_sha = parse_sha256_output(&sha_output.stdout)?;
    let header_text = fs::read_to_string(headers_path).unwrap_or_default();
    Ok(StreamDownloadResult {
        metadata: parse_http_headers(&header_text),
        bytes,
        sha256: actual_sha,
        download_ms,
        verify_ms,
    })
}

fn terminate_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn parse_sha256_output(output: &[u8]) -> Result<String, String> {
    mister_magik_media_contract::Sha256::parse_command_output(output)
        .map(mister_magik_media_contract::Sha256::into_string)
}

fn verify_downloaded_bytes(
    actual_bytes: u64,
    actual_sha: &str,
    expected_bytes: u64,
    expected_sha: &str,
) -> Result<(), String> {
    if actual_bytes != expected_bytes {
        return Err(format!(
            "size-mismatch-expected-{expected_bytes}-actual-{actual_bytes}"
        ));
    }
    if actual_sha != expected_sha {
        return Err(format!(
            "sha256-mismatch-expected-{expected_sha}-actual-{actual_sha}"
        ));
    }
    Ok(())
}

fn install_verified_streamed_temp(
    publish: &crate::artifact_publish::ArtifactPublishPlan,
    actual_bytes: u64,
    actual_sha: &str,
    expected_bytes: u64,
    expected_sha: &str,
) -> Result<StreamPublishMetrics, String> {
    verify_downloaded_bytes(actual_bytes, actual_sha, expected_bytes, expected_sha)?;
    let started = Instant::now();
    let bytes = file_len(publish.temp_path())?;
    let sync_started = Instant::now();
    File::options()
        .read(true)
        .open(publish.temp_path())
        .and_then(|file| file.sync_all())
        .map_err(|e| format!("sync {}: {e}", publish.temp_path().display()))?;
    let sync_ms = elapsed_ms(sync_started.elapsed());

    let rename_started = Instant::now();
    publish.install_temp(Some("streamed benchmark pack"))?;
    let rename_ms = elapsed_ms(rename_started.elapsed());

    let parent_sync_started = Instant::now();
    sync_path_rust_best_effort(publish.parent());
    let parent_sync_ms = elapsed_ms(parent_sync_started.elapsed());

    Ok(StreamPublishMetrics {
        bytes,
        sync_ms,
        rename_ms,
        parent_sync_ms,
        total_ms: elapsed_ms(started.elapsed()),
    })
}

fn add_curl_download_args(
    command: &mut Command,
    url: &str,
    headers_path: Option<&Path>,
    max_bytes: u64,
) {
    command
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("--proto")
        .arg("=http,https")
        .arg("--connect-timeout")
        .arg(MEDIA_CONNECT_TIMEOUT_SECS.to_string())
        .arg("--max-time")
        .arg(MEDIA_TRANSFER_TIMEOUT_SECS.to_string())
        .arg("--max-filesize")
        .arg(max_bytes.to_string())
        .arg("--header")
        .arg("Accept-Encoding: identity");
    if let Some(headers_path) = headers_path {
        command.arg("-D").arg(headers_path);
    }
    if url.starts_with("https://") && Path::new("/etc/ssl/certs/cacert.pem").is_file() {
        command.arg("--cacert").arg("/etc/ssl/certs/cacert.pem");
    }
}

fn parse_http_headers(text: &str) -> HttpMetadata {
    let headers = mister_magik_media_contract::HttpHeaders::parse(text);
    HttpMetadata {
        etag: headers.get("etag").unwrap_or_default().to_string(),
        content_encoding: headers
            .get("content-encoding")
            .unwrap_or("identity")
            .to_string(),
        cf_cache_status: headers
            .get("cf-cache-status")
            .unwrap_or_default()
            .to_string(),
    }
}

fn verify_file(path: &Path, expected_bytes: u64, expected_sha: &str) -> Result<(), String> {
    let actual_bytes = file_len(path)?;
    if actual_bytes != expected_bytes {
        return Err(format!(
            "size-mismatch-expected-{expected_bytes}-actual-{actual_bytes}"
        ));
    }
    let actual_sha = sha256_hex(path)?;
    if actual_sha != expected_sha {
        return Err(format!(
            "sha256-mismatch-expected-{expected_sha}-actual-{actual_sha}"
        ));
    }
    Ok(())
}

fn sha256_hex(path: &Path) -> Result<String, String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("spawn sha256sum: {e}"))?;
    if !output.status.success() {
        return Err(format!("sha256sum failed with {}", output.status));
    }
    parse_sha256_output(&output.stdout)
}

fn emit_publish_stage_rows(
    label: &str,
    pack: &MediaPack,
    strategy: SaveStrategy,
    metrics: &PackSaveMetrics,
) {
    for (stage, ms) in [
        ("publish_copy", metrics.copy_ms),
        ("publish_sync", metrics.sync_ms),
        ("publish_rename", metrics.rename_ms),
        ("publish_parent_sync", metrics.parent_sync_ms),
    ] {
        emit_stage_row(
            label,
            pack,
            stage,
            ms,
            metrics.bytes,
            "bench-ok",
            &format!(
                "save_strategy={} progress_events={}",
                strategy.label(),
                metrics.progress_events
            ),
        );
    }
}

fn emit_stream_publish_stage_rows(label: &str, pack: &MediaPack, metrics: &StreamPublishMetrics) {
    for (stage, ms) in [
        ("stream_sync", metrics.sync_ms),
        ("stream_rename", metrics.rename_ms),
        ("stream_parent_sync", metrics.parent_sync_ms),
    ] {
        emit_stage_row(
            label,
            pack,
            stage,
            ms,
            metrics.bytes,
            "bench-ok",
            &format!("save_strategy={}", SaveStrategy::StreamFat.label()),
        );
    }
}

fn emit_stage_row(
    label: &str,
    pack: &MediaPack,
    stage: &str,
    ms: u64,
    bytes: u64,
    result: &str,
    detail: &str,
) {
    crate::ui_logln!(
        "stage_tsv\tlabel={}\tsuite_label={}\tbenchmark=media-bench-download\tsystem={}\tstage={}\tms={}\tbytes={}\tresult={}\tdetail={}",
        tsv(label),
        suite_label(label),
        tsv(&pack.id),
        tsv(stage),
        ms,
        bytes,
        tsv(result),
        tsv(detail)
    );
}

fn suite_label(label: &str) -> String {
    label
        .rsplit_once('-')
        .and_then(|(prefix, suffix)| {
            (suffix.len() == 2 && suffix.chars().all(|ch| ch.is_ascii_digit())).then_some(prefix)
        })
        .unwrap_or(label)
        .to_string()
}

fn bench_final_path(local_path: &Path, stamp: &str) -> PathBuf {
    let file_name = local_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("screenshot-pack");
    local_path.with_file_name(format!(".{file_name}.bench-{stamp}"))
}

fn bench_state_path(asset_dir: &Path, stamp: &str) -> PathBuf {
    PathBuf::from(format!(
        "{}.bench-{stamp}",
        state_path(&asset_dir.display().to_string())
    ))
}

fn write_bench_download_state(
    path: &Path,
    pack: &MediaPack,
    local_path: &Path,
    variant: &MediaVariant,
    metadata: &HttpMetadata,
) -> Result<(), String> {
    let mut root = fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let systems = root
        .entry("systems".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    let systems = systems
        .as_object_mut()
        .ok_or("media state systems must be an object")?;
    let system = systems
        .entry(pack.id.clone())
        .or_insert_with(|| Value::Object(Default::default()));
    let system = system
        .as_object_mut()
        .ok_or("media state system entry must be an object")?;
    system.insert(
        "preferred_size".to_string(),
        Value::String(pack.image_size.clone()),
    );
    system.insert(
        "preferred_variant".to_string(),
        Value::String("identity".to_string()),
    );
    let packs = system
        .entry("packs".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    let packs = packs
        .as_object_mut()
        .ok_or("media state packs must be an object")?;
    packs.insert(
        pack.image_size.clone(),
        serde_json::json!({
            "version": pack.version,
            "image_size": pack.image_size,
            "sha256": pack.raw.sha256,
            "bytes": pack.raw.bytes,
            "variant": "identity",
            "compression": variant.compression,
            "local_path": local_path.display().to_string(),
            "object": variant.object,
            "updated_at_unix": unix_secs_now(),
            "cache": {
                "etag": metadata.etag,
                "content_encoding": metadata.content_encoding,
                "cf_cache_status": metadata.cf_cache_status,
                "source": "media-bench-download",
            },
        }),
    );
    root.insert("schema".to_string(), Value::from(1));
    root.insert("updated_at_unix".to_string(), Value::from(unix_secs_now()));
    write_json_atomic(path, &Value::Object(root))
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let publish = prepare_artifact_publish(
        path,
        timestamped_temp_path_for(path, "media-state", unix_ms_now()),
        ArtifactPublishLabels {
            destination: "bench media state",
            parent: "bench media state parent",
        },
    )?;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(publish.temp_path())
        .map_err(|e| format!("create state tmp {}: {e}", publish.temp_path().display()))?;
    let text =
        serde_json::to_string_pretty(value).map_err(|e| format!("serialize media state: {e}"))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|e| format!("write media state {}: {e}", publish.temp_path().display()))?;
    file.sync_all()
        .map_err(|e| format!("sync media state {}: {e}", publish.temp_path().display()))?;
    publish.install_temp(Some("bench media state"))?;
    sync_path_rust_best_effort(publish.parent());
    Ok(())
}

fn cleanup_bench_artifacts(
    encoded: &Path,
    bench_final: &Path,
    bench_state: &Path,
) -> Result<usize, String> {
    let mut removed = 0usize;
    cleanup_pack_publish_temps(bench_final);
    for path in [encoded, bench_final, bench_state] {
        match fs::remove_file(path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("remove bench artifact {}: {error}", path.display())),
        }
    }
    Ok(removed)
}

fn file_len(path: &Path) -> Result<u64, String> {
    path.metadata()
        .map(|meta| meta.len())
        .map_err(|e| format!("stat {}: {e}", path.display()))
}

fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn mbps(bytes: u64, ms: u64) -> f64 {
    if ms == 0 {
        0.0
    } else {
        (bytes as f64 * 8.0) / (ms as f64 * 1000.0)
    }
}

fn default_label() -> String {
    format!("screenshot-download-{}", unix_ms_now())
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn unix_secs_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

impl BenchRow {
    fn to_tsv(&self) -> String {
        format!(
            "screenshot_download_bench_tsv\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{}\t{}\t{}\t{}",
            self.label,
            self.system,
            self.variant,
            self.encoded_bytes,
            self.decoded_bytes,
            self.download_ms,
            self.decompress_ms,
            self.save_ms,
            self.verify_ms,
            self.total_ms,
            self.wire_mbps,
            self.decoded_mbps,
            tsv(&self.etag),
            tsv(&self.content_encoding),
            tsv(&self.cf_cache_status),
            tsv(&self.result),
        )
    }

    fn to_json(&self, pack: &MediaPack, strategy: SaveStrategy) -> Value {
        serde_json::json!({
            "label": self.label,
            "system": self.system,
            "variant": self.variant,
            "pack_bytes": pack.raw.bytes,
            "pack_sha256": pack.raw.sha256,
            "index": pack.index.as_ref().map(|index| serde_json::json!({
                "bytes": index.bytes,
                "sha256": index.sha256,
                "download_overlap_ms": Value::Null,
                "overlap_status": "not-exercised-by-isolated-pack-persistence-arm",
            })),
            "timing_ms": {
                "network_and_destination_write": self.download_ms,
                "decode": 0,
                "verification": self.verify_ms,
                "save_and_publish": self.save_ms,
                "total_flow": self.total_ms,
            },
            "bytes": {
                "network": self.encoded_bytes,
                "tmpfs": if strategy == SaveStrategy::Staged { self.encoded_bytes } else { 0 },
                "exfat": self.decoded_bytes,
            },
            "throughput_mbps": {
                "network": self.wire_mbps,
                "total_flow": self.decoded_mbps,
            },
            "exfat_writer_concurrency": 1,
            "etag": self.etag,
            "content_encoding": self.content_encoding,
            "cf_cache_status": self.cf_cache_status,
            "result": self.result,
        })
    }
}

fn proc_status_kb(field: &str) -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key == field).then(|| value.split_whitespace().next()?.parse().ok())?
        })
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

    fn test_pack() -> MediaPack {
        MediaPack {
            id: "neogeo".to_string(),
            version: "2026-06-24".to_string(),
            image_size: "320x320".to_string(),
            raw: test_variant(),
            variants: vec![test_variant()],
            index: None,
        }
    }

    fn test_variant() -> MediaVariant {
        MediaVariant {
            compression: "none".to_string(),
            codec: "mmlz4b".to_string(),
            object: "packs/neogeo.mmlz4b".to_string(),
            bytes: 4,
            sha256: "abcd".to_string(),
            url: "https://assets.example.test/packs/neogeo.mmlz4b".to_string(),
        }
    }

    #[test]
    fn benchmark_object_curl_is_http_capable_and_bounded() {
        let mut command = Command::new("curl");
        add_curl_download_args(
            &mut command,
            "http://assets.example/pack.mmlz4b",
            Some(Path::new("/tmp/headers")),
            321,
        );
        let text = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");

        assert!(text.contains("--proto =http,https"));
        assert!(text.contains("--connect-timeout 10"));
        assert!(text.contains("--max-time 1200"));
        assert!(text.contains("--max-filesize 321"));
    }

    #[test]
    fn parses_benchmark_args() {
        let config = parse_args([
            "--system".to_string(),
            "arcade".to_string(),
            "--variants".to_string(),
            "identity".to_string(),
            "--iterations".to_string(),
            "10".to_string(),
            "--label".to_string(),
            "CACHE-20260623".to_string(),
            "--prime-cache".to_string(),
            "--save-strategy".to_string(),
            "stream-fat".to_string(),
        ])
        .unwrap();

        assert_eq!(config.system, "arcade");
        assert_eq!(config.variant, "identity");
        assert_eq!(config.iterations, 10);
        assert!(config.prime_cache);
        assert_eq!(config.save_strategy, SaveStrategy::StreamFat);
    }

    #[test]
    fn representative_selection_uses_small_median_and_largest_packs() {
        let mut packs = (0..5)
            .map(|index| {
                let mut pack = test_pack();
                pack.id = format!("system-{index}");
                pack.raw.bytes = (index + 1) * 100;
                pack
            })
            .collect::<Vec<_>>();
        packs.reverse();

        let selected = representative_packs(packs.iter().collect());
        assert_eq!(
            selected
                .iter()
                .map(|pack| pack.raw.bytes)
                .collect::<Vec<_>>(),
            vec![100, 300, 500]
        );
    }

    #[test]
    fn save_strategy_defaults_to_staged_and_rejects_unknown_values() {
        assert_eq!(
            parse_args(Vec::<String>::new()).unwrap().save_strategy,
            SaveStrategy::Staged
        );

        let error = parse_args([
            "--save-strategy".to_string(),
            "optimistic-direct".to_string(),
        ])
        .unwrap_err();

        assert!(error.contains("unsupported --save-strategy"));
    }

    #[test]
    fn rejects_compressed_benchmark_variants() {
        let error = parse_args([
            "--system".to_string(),
            "arcade".to_string(),
            "--variants".to_string(),
            "identity,gzip,brotli".to_string(),
        ])
        .unwrap_err();

        assert!(error.contains("only supports the raw identity"));
    }

    #[test]
    fn parses_http_cache_headers() {
        let metadata = parse_http_headers(
            "  HTTP/1.1 200 OK\n  ETag: \"abc\"\n  CF-Cache-Status: HIT\n  Content-Encoding: identity\n",
        );

        assert_eq!(metadata.etag, "\"abc\"");
        assert_eq!(metadata.cf_cache_status, "HIT");
        assert_eq!(metadata.content_encoding, "identity");
    }

    #[test]
    fn derives_suite_labels_from_iteration_labels() {
        assert_eq!(suite_label("DL-20260624-01"), "DL-20260624");
        assert_eq!(suite_label("DL-20260624"), "DL-20260624");
    }

    #[test]
    fn benchmark_paths_are_hidden_and_label_scoped() {
        let final_path =
            Path::new("/media/fat/mister-magik/assets/neogeo-screenshots-320x320.mmlz4b");
        assert_eq!(
            bench_final_path(final_path, "123").display().to_string(),
            "/media/fat/mister-magik/assets/.neogeo-screenshots-320x320.mmlz4b.bench-123"
        );
        assert_eq!(
            bench_state_path(Path::new("/media/fat/mister-magik/assets"), "123")
                .display()
                .to_string(),
            "/media/fat/mister-magik/assets/.screenshot-media-state.json.bench-123"
        );
    }

    #[test]
    fn writes_benchmark_download_state_without_touching_production_state() {
        let dir = temp_dir("mister-magik-download-bench-state");
        let state = bench_state_path(&dir, "state-test");
        let final_path = dir.join(".neogeo-screenshots-320x320.mmlz4b.bench-state-test");
        let metadata = HttpMetadata {
            etag: "\"etag\"".to_string(),
            content_encoding: "identity".to_string(),
            cf_cache_status: "HIT".to_string(),
        };

        write_bench_download_state(
            &state,
            &test_pack(),
            &final_path,
            &test_variant(),
            &metadata,
        )
        .unwrap();

        let value: Value = serde_json::from_str(&fs::read_to_string(&state).unwrap()).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(
            value["systems"]["neogeo"]["packs"]["320x320"]["local_path"],
            final_path.display().to_string()
        );
        assert_eq!(
            value["systems"]["neogeo"]["packs"]["320x320"]["cache"]["cf_cache_status"],
            "HIT"
        );
        assert!(!dir.join(".screenshot-media-state.json").exists());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn streamed_publish_rejects_bad_checksum_without_renaming_final_file() {
        let dir = temp_dir("mister-magik-stream-fat-bad-sha");
        let final_path = dir.join(".neogeo-screenshots-320x320.mmlz4b.bench-test");
        let temp_path =
            final_path.with_file_name(".neogeo-screenshots-320x320.mmlz4b.bench-test.tmp");
        let publish = prepare_artifact_publish(
            &final_path,
            temp_path.clone(),
            ArtifactPublishLabels {
                destination: "stream test final",
                parent: "stream test parent",
            },
        )
        .unwrap();
        fs::write(publish.temp_path(), b"bad-bytes").unwrap();

        let error = install_verified_streamed_temp(
            &publish,
            9,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            9,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .unwrap_err();

        assert!(error.contains("sha256-mismatch"));
        assert!(publish.temp_path().exists());
        assert!(!final_path.exists());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn streamed_publish_rejects_bad_size_without_renaming_final_file() {
        let dir = temp_dir("mister-magik-stream-fat-bad-size");
        let final_path = dir.join(".neogeo-screenshots-320x320.mmlz4b.bench-test");
        let temp_path =
            final_path.with_file_name(".neogeo-screenshots-320x320.mmlz4b.bench-test.tmp");
        let publish = prepare_artifact_publish(
            &final_path,
            temp_path.clone(),
            ArtifactPublishLabels {
                destination: "stream test final",
                parent: "stream test parent",
            },
        )
        .unwrap();
        fs::write(publish.temp_path(), b"bad-bytes").unwrap();

        let error = install_verified_streamed_temp(
            &publish,
            9,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            10,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap_err();

        assert!(error.contains("size-mismatch"));
        assert!(publish.temp_path().exists());
        assert!(!final_path.exists());

        let _ = fs::remove_dir_all(dir);
    }
}
