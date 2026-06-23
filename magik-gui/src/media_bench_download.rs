use mister_magik_fb::media_update::{
    parse_manifest_json, size_qualified_pack_path, valid_image_size, MediaPack, MediaVariant,
    DEFAULT_ASSET_DIR, DEFAULT_IMAGE_SIZE, DEFAULT_MANIFEST_URL,
};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
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

pub(crate) fn run() {
    match run_inner(std::env::args().skip(2)) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("media-bench-download failed: {error}");
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
    let packs: Vec<_> = manifest
        .packs
        .iter()
        .filter(|pack| {
            pack.image_size == config.image_size
                && (config.system == "all" || pack.id == config.system)
        })
        .collect();
    if packs.is_empty() {
        return Err(format!(
            "manifest has no packs for system={} image_size={}",
            config.system, config.image_size
        ));
    }
    println!("{HEADER}");
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
            eprintln!(
                "media_bench_prime\tsystem={}\tvariant={}\tcf_cache_status={}\ttotal_ms={}",
                pack.id, config.variant, row.cf_cache_status, row.total_ms
            );
        }
        for iteration in 1..=config.iterations {
            let label = format!("{}-{:02}", config.label, iteration);
            let row = run_one(&config, pack, variant, &local_path, &label)?;
            println!("{}", row.to_tsv());
        }
    }
    Ok(())
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
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest-url" => {
                config.manifest_url = args.next().ok_or("--manifest-url requires a URL")?;
            }
            "--system" => {
                config.system = args.next().ok_or("--system requires id|all")?;
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
    println!(
        "usage: mister-magik-fb media-bench-download --system ID [--variant identity] --iterations N [--prime-cache]"
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
    let output = Command::new("wget")
        .args(["-q", "-O", "-", url])
        .output()
        .map_err(|e| format!("spawn wget: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "wget manifest failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("manifest utf8: {e}"))
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
    let final_tmp = final_temp_path(local_path, &stamp);
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
        let download_started = Instant::now();
        let metadata = download_to_path(&variant.url, &encoded)?;
        row.download_ms = elapsed_ms(download_started.elapsed());
        row.etag = metadata.etag;
        row.content_encoding = metadata.content_encoding;
        row.cf_cache_status = metadata.cf_cache_status;
        row.encoded_bytes = file_len(&encoded)?;

        let verify_started = Instant::now();
        verify_file(&encoded, variant.bytes, &variant.sha256)?;
        row.verify_ms += elapsed_ms(verify_started.elapsed());

        row.decompress_ms = 0;
        row.decoded_bytes = file_len(&encoded)?;

        let verify_started = Instant::now();
        verify_file(&encoded, pack.raw.bytes, &pack.raw.sha256)?;
        row.verify_ms += elapsed_ms(verify_started.elapsed());

        let save_started = Instant::now();
        copy_file_durable(&encoded, &final_tmp)?;
        fs::remove_file(&final_tmp)
            .map_err(|e| format!("remove bench temp {}: {e}", final_tmp.display()))?;
        sync_path(
            local_path
                .parent()
                .unwrap_or_else(|| Path::new(DEFAULT_ASSET_DIR)),
        );
        row.save_ms = elapsed_ms(save_started.elapsed());
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        row.result = error;
    }
    row.total_ms = elapsed_ms(started.elapsed());
    row.wire_mbps = mbps(row.encoded_bytes, row.download_ms);
    row.decoded_mbps = mbps(row.decoded_bytes, row.total_ms);
    let _ = fs::remove_file(&encoded);
    let _ = fs::remove_file(&final_tmp);
    if row.result != "bench-ok" {
        return Err(row.to_tsv());
    }
    Ok(row)
}

fn download_to_path(url: &str, path: &Path) -> Result<HttpMetadata, String> {
    let output = Command::new("wget")
        .arg("-S")
        .arg("--header")
        .arg("Accept-Encoding: identity")
        .arg("-O")
        .arg(path)
        .arg(url)
        .output()
        .map_err(|e| format!("spawn wget: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "download-failed-{}:{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_wget_headers(&String::from_utf8_lossy(&output.stderr)))
}

fn parse_wget_headers(text: &str) -> HttpMetadata {
    let mut headers = BTreeMap::new();
    for line in text.lines() {
        let Some((name, value)) = line.trim().split_once(':') else {
            continue;
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    HttpMetadata {
        etag: headers.get("etag").cloned().unwrap_or_default(),
        content_encoding: headers
            .get("content-encoding")
            .cloned()
            .unwrap_or_else(|| "identity".to_string()),
        cf_cache_status: headers.get("cf-cache-status").cloned().unwrap_or_default(),
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
    let text = String::from_utf8(output.stdout).map_err(|e| format!("sha256 utf8: {e}"))?;
    text.split_whitespace()
        .next()
        .filter(|sha| sha.len() == 64)
        .map(str::to_string)
        .ok_or_else(|| format!("could not parse sha256sum output: {text}"))
}

fn copy_file_durable(src: &Path, dst: &Path) -> Result<(), String> {
    let mut input = File::open(src).map_err(|e| format!("open {}: {e}", src.display()))?;
    let mut output = File::create(dst).map_err(|e| format!("create {}: {e}", dst.display()))?;
    std::io::copy(&mut input, &mut output)
        .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
    output
        .sync_all()
        .map_err(|e| format!("sync {}: {e}", dst.display()))?;
    Ok(())
}

fn sync_path(path: &Path) {
    let _ = Command::new("sync").arg(path).status();
}

fn final_temp_path(local_path: &Path, stamp: &str) -> PathBuf {
    local_path.with_file_name(format!(
        ".{}.bench-{stamp}.tmp",
        local_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("screenshot-pack")
    ))
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
        ])
        .unwrap();

        assert_eq!(config.system, "arcade");
        assert_eq!(config.variant, "identity");
        assert_eq!(config.iterations, 10);
        assert!(config.prime_cache);
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
    fn parses_wget_cache_headers() {
        let metadata = parse_wget_headers(
            "  HTTP/1.1 200 OK\n  ETag: \"abc\"\n  CF-Cache-Status: HIT\n  Content-Encoding: identity\n",
        );

        assert_eq!(metadata.etag, "\"abc\"");
        assert_eq!(metadata.cf_cache_status, "HIT");
        assert_eq!(metadata.content_encoding, "identity");
    }
}
