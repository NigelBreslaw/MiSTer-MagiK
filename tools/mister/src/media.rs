use serde_json::{json, Value};
use ssh2::{ExtendedData, Session};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const DEFAULT_REMOTE_ASSET_DIR: &str = "/media/fat/mister-magik/assets";
const REMOTE_STATE_PATH: &str = "/media/fat/mister-magik/assets/.screenshot-media-state.json";
const BENCH_TSV: &str = "history/toolchain-bench/results-screenshot-download.tsv";
const BENCH_HEADER: &str = "type\tlabel\tsystem\tvariant\tencoded_bytes\tdecoded_bytes\tdownload_ms\tdecompress_ms\tsave_ms\tverify_ms\ttotal_ms\twire_mbps\tdecoded_mbps\tetag\tcontent_encoding\tcf_cache_status\tresult";

#[derive(Clone, Debug)]
struct MediaManifest {
    schema_version: u64,
    published_at: String,
    base_url: String,
    packs: Vec<MediaPack>,
}

#[derive(Clone, Debug)]
struct MediaPack {
    system: String,
    local_path: String,
    asset_count: Option<u64>,
    identity: MediaVariant,
}

#[derive(Clone, Debug)]
struct MediaVariant {
    remote_path: String,
    decoded_bytes: u64,
    decoded_sha256: String,
    etag: Option<String>,
}

#[derive(Clone, Debug)]
struct MediaArgs {
    manifest_url: String,
    system: String,
    variant: String,
    variants: Vec<String>,
    label: String,
    save_preference: bool,
}

#[derive(Clone, Debug)]
struct RemoteBenchRow {
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
    wire_mbps: String,
    decoded_mbps: String,
    etag: String,
    content_encoding: String,
    cf_cache_status: String,
    result: String,
}

pub(crate) fn media_check(sess: &Session, args: &[String]) -> Result<()> {
    let parsed = parse_media_args(args)?;
    let manifest = load_manifest(&parsed.manifest_url)?;
    print_manifest_summary(&manifest);
    for pack in selected_packs(&manifest, &parsed.system)? {
        let status = remote_pack_status(sess, pack)?;
        println!(
            "media_check\t{}\t{}\tlocal_path={}\tremote_url={}\tetag={}",
            pack.system,
            status,
            pack.local_path,
            manifest_url_for_pack(&manifest, pack),
            pack.identity.etag.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

pub(crate) fn media_download(sess: &Session, args: &[String]) -> Result<()> {
    let parsed = parse_media_args(args)?;
    let manifest = load_manifest(&parsed.manifest_url)?;
    let packs = selected_packs(&manifest, &parsed.system)?;
    for pack in packs {
        if remote_pack_status(sess, pack)? == "current" {
            println!(
                "media_download\t{}\tskipped-current\tlocal_path={}",
                pack.system, pack.local_path
            );
            continue;
        }
        let variant = resolve_variant(sess, pack, &parsed.variant)?;
        let row = run_remote_media_download(sess, &manifest, pack, &parsed.label, &variant, true)?;
        println!("{}", row.to_tsv());
        if row.result != "downloaded" {
            return Err(
                format!("media download failed for {}: {}", pack.system, row.result).into(),
            );
        }
        update_remote_state(sess, pack, &variant, &row)?;
    }
    Ok(())
}

pub(crate) fn media_bench_download(sess: &Session, args: &[String]) -> Result<()> {
    let parsed = parse_media_args(args)?;
    let manifest = load_manifest(&parsed.manifest_url)?;
    let packs = selected_packs(&manifest, &parsed.system)?;
    for pack in packs {
        let variants = if parsed.variants.is_empty() {
            vec![parsed.variant.clone()]
        } else {
            parsed.variants.clone()
        };
        let mut rows = Vec::new();
        for requested in variants {
            let variant = resolve_variant(sess, pack, &requested)?;
            let row =
                run_remote_media_download(sess, &manifest, pack, &parsed.label, &variant, false)?;
            append_profile_row(BENCH_TSV, BENCH_HEADER, &row.to_tsv())?;
            println!("{}", row.to_tsv());
            rows.push(row);
        }
        if parsed.save_preference {
            if let Some(best) = rows
                .iter()
                .filter(|row| matches!(row.result.as_str(), "downloaded" | "bench-ok"))
                .min_by_key(|row| row.total_ms)
            {
                update_remote_state(sess, pack, &best.variant, best)?;
                println!(
                    "media_preference\t{}\t{}\ttotal_ms={}",
                    pack.system, best.variant, best.total_ms
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn media_cloudflare_check(args: &[String]) -> Result<()> {
    if media_help_requested(args) {
        media_usage();
        return Ok(());
    }
    let parsed = parse_media_args(args)?;
    let manifest = load_manifest(&parsed.manifest_url)?;
    let pack = selected_packs(&manifest, &parsed.system)?
        .into_iter()
        .next()
        .ok_or("manifest has no selected packs")?;
    let url = manifest_url_for_pack(&manifest, pack);
    println!("cloudflare_probe_url\t{url}");
    for variant in ["identity", "gzip", "brotli"] {
        let accept = accept_encoding_for_variant(variant);
        let headers = curl_headers(&url, accept)?;
        let content_encoding = header_value(&headers, "content-encoding").unwrap_or("identity");
        let cache_status = header_value(&headers, "cf-cache-status").unwrap_or("");
        let server = header_value(&headers, "server").unwrap_or("");
        println!(
            "cloudflare_header_probe\tvariant={variant}\taccept_encoding={accept}\tcontent_encoding={content_encoding}\tcf_cache_status={cache_status}\tserver={server}"
        );
    }
    cloudflare_api_probe()?;
    Ok(())
}

impl RemoteBenchRow {
    fn to_tsv(&self) -> String {
        format!(
            "screenshot_download_bench_tsv\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
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

fn parse_media_args(args: &[String]) -> Result<MediaArgs> {
    let mut parsed = MediaArgs {
        manifest_url: env::var("MISTER_MEDIA_MANIFEST_URL").unwrap_or_default(),
        system: "all".to_string(),
        variant: "identity".to_string(),
        variants: Vec::new(),
        label: default_label(),
        save_preference: false,
    };
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--manifest-url" => {
                idx += 1;
                parsed.manifest_url = args.get(idx).ok_or("--manifest-url needs URL")?.clone();
            }
            "--system" => {
                idx += 1;
                parsed.system = args.get(idx).ok_or("--system needs id|all")?.clone();
            }
            "--variant" => {
                idx += 1;
                parsed.variant = normalize_variant(args.get(idx).ok_or("--variant needs name")?)?;
            }
            "--variants" => {
                idx += 1;
                parsed.variants = args
                    .get(idx)
                    .ok_or("--variants needs comma-list")?
                    .split(',')
                    .map(normalize_variant)
                    .collect::<Result<Vec<_>>>()?;
            }
            "--label" => {
                idx += 1;
                parsed.label = args.get(idx).ok_or("--label needs value")?.clone();
            }
            "--save-preference" => parsed.save_preference = true,
            "-h" | "--help" => {
                media_usage();
                return Err("help requested".into());
            }
            other => return Err(format!("unknown media option: {other}").into()),
        }
        idx += 1;
    }
    if parsed.manifest_url.is_empty() {
        return Err("set MISTER_MEDIA_MANIFEST_URL or pass --manifest-url".into());
    }
    if !parsed
        .label
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err("--label must contain only letters, numbers, _, ., or -".into());
    }
    Ok(parsed)
}

pub(crate) fn media_help_requested(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
}

pub(crate) fn media_usage() {
    println!(
        "media options: --manifest-url URL --system id|all --variant identity|gzip|brotli|auto --variants identity,gzip,brotli --label LABEL --save-preference"
    );
}

fn normalize_variant(raw: &str) -> Result<String> {
    match raw {
        "identity" | "none" | "plain" => Ok("identity".to_string()),
        "gzip" | "gz" => Ok("gzip".to_string()),
        "brotli" | "br" => Ok("brotli".to_string()),
        "auto" => Ok("auto".to_string()),
        other => Err(format!("unknown media variant: {other}").into()),
    }
}

fn load_manifest(url: &str) -> Result<MediaManifest> {
    let out = Command::new("curl").args(["-fsSL", url]).output()?;
    if !out.status.success() {
        return Err(format!(
            "failed to fetch manifest from {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    parse_manifest(&serde_json::from_slice(&out.stdout)?)
}

fn parse_manifest(value: &Value) -> Result<MediaManifest> {
    let schema_version = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or("manifest missing schema_version")?;
    let published_at = value
        .get("published_at")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let base_url = value
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string();
    let pack_values = value
        .get("packs")
        .and_then(Value::as_array)
        .ok_or("manifest missing packs array")?;
    let mut packs = Vec::new();
    for pack in pack_values {
        let system = required_str(pack, "system")?.to_string();
        let local_path = pack
            .get("local_path")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{DEFAULT_REMOTE_ASSET_DIR}/{system}-screenshots.mmlz4b"));
        let asset_count = pack.get("asset_count").and_then(Value::as_u64);
        let identity_value = pack
            .get("variants")
            .and_then(|variants| variants.get("identity"))
            .unwrap_or(pack);
        let identity = MediaVariant {
            remote_path: identity_value
                .get("remote_path")
                .or_else(|| pack.get("remote_path"))
                .and_then(Value::as_str)
                .ok_or_else(|| format!("pack {system} missing identity remote_path"))?
                .to_string(),
            decoded_bytes: identity_value
                .get("decoded_bytes")
                .or_else(|| identity_value.get("bytes"))
                .or_else(|| pack.get("bytes"))
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("pack {system} missing identity bytes"))?,
            decoded_sha256: identity_value
                .get("sha256_decoded")
                .or_else(|| identity_value.get("sha256"))
                .or_else(|| pack.get("sha256"))
                .and_then(Value::as_str)
                .ok_or_else(|| format!("pack {system} missing identity sha256"))?
                .to_ascii_lowercase(),
            etag: identity_value
                .get("etag")
                .or_else(|| pack.get("etag"))
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        packs.push(MediaPack {
            system,
            local_path,
            asset_count,
            identity,
        });
    }
    Ok(MediaManifest {
        schema_version,
        published_at,
        base_url,
        packs,
    })
}

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("manifest missing {key}").into())
}

fn selected_packs<'a>(manifest: &'a MediaManifest, system: &str) -> Result<Vec<&'a MediaPack>> {
    if system == "all" {
        return Ok(manifest.packs.iter().collect());
    }
    let packs: Vec<_> = manifest
        .packs
        .iter()
        .filter(|pack| pack.system == system)
        .collect();
    if packs.is_empty() {
        Err(format!("manifest has no screenshot pack for system '{system}'").into())
    } else {
        Ok(packs)
    }
}

fn print_manifest_summary(manifest: &MediaManifest) {
    println!(
        "media_manifest\tschema_version={}\tpublished_at={}\tbase_url={}\tpacks={}",
        manifest.schema_version,
        manifest.published_at,
        manifest.base_url,
        manifest.packs.len()
    );
}

fn remote_pack_status(sess: &Session, pack: &MediaPack) -> Result<String> {
    let cmd = format!(
        "if [ -f {path} ]; then got=$(sha256sum {path} 2>/dev/null | awk '{{print $1}}'); if [ \"$got\" = {sha} ]; then echo current; else echo stale:$got; fi; else echo missing; fi",
        path = shell_quote(&pack.local_path),
        sha = shell_quote(&pack.identity.decoded_sha256),
    );
    Ok(exec_stdout(sess, &cmd)?.trim().to_string())
}

fn resolve_variant(sess: &Session, pack: &MediaPack, requested: &str) -> Result<String> {
    if requested != "auto" {
        return Ok(requested.to_string());
    }
    let cmd = format!("cat {} 2>/dev/null || true", shell_quote(REMOTE_STATE_PATH));
    let text = exec_stdout(sess, &cmd)?;
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(value
        .get("systems")
        .and_then(|systems| systems.get(&pack.system))
        .and_then(|system| system.get("preferred_variant"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "identity".to_string()))
}

fn run_remote_media_download(
    sess: &Session,
    manifest: &MediaManifest,
    pack: &MediaPack,
    label: &str,
    variant: &str,
    publish: bool,
) -> Result<RemoteBenchRow> {
    let script = remote_script();
    let remote_script = format!("/tmp/mister-magik-media-{}.sh", unix_ms_now());
    sftp_write(sess, &remote_script, script.as_bytes())?;
    let url = manifest_url_for_pack(manifest, pack);
    let accept = accept_encoding_for_variant(variant);
    let mode = if publish { "publish" } else { "bench" };
    let cmd = format!(
        "chmod +x {script}; {script} {mode} {label} {system} {variant} {accept} {url} {local} {sha} {bytes}; rc=$?; rm -f {script}; exit $rc",
        script = shell_quote(&remote_script),
        mode = shell_quote(mode),
        label = shell_quote(label),
        system = shell_quote(&pack.system),
        variant = shell_quote(variant),
        accept = shell_quote(accept),
        url = shell_quote(&url),
        local = shell_quote(&pack.local_path),
        sha = shell_quote(&pack.identity.decoded_sha256),
        bytes = shell_quote(&pack.identity.decoded_bytes.to_string()),
    );
    let out = exec(sess, &cmd)?;
    if !out.stderr.trim().is_empty() {
        eprint!("{}", out.stderr);
    }
    let row_line = out
        .stdout
        .lines()
        .find(|line| line.starts_with("screenshot_download_bench_tsv\t"))
        .ok_or_else(|| {
            format!(
                "remote media runner produced no benchmark row: {}",
                out.stdout
            )
        })?;
    let row = parse_remote_row(row_line)?;
    if out.rc != 0 && row.result != "downloaded" && row.result != "bench-ok" {
        return Ok(row);
    }
    Ok(row)
}

fn parse_remote_row(line: &str) -> Result<RemoteBenchRow> {
    let parts: Vec<_> = line.split('\t').collect();
    if parts.len() < 17 {
        return Err(format!("bad benchmark row: {line}").into());
    }
    Ok(RemoteBenchRow {
        label: parts[1].to_string(),
        system: parts[2].to_string(),
        variant: parts[3].to_string(),
        encoded_bytes: parts[4].parse()?,
        decoded_bytes: parts[5].parse()?,
        download_ms: parts[6].parse()?,
        decompress_ms: parts[7].parse()?,
        save_ms: parts[8].parse()?,
        verify_ms: parts[9].parse()?,
        total_ms: parts[10].parse()?,
        wire_mbps: parts[11].to_string(),
        decoded_mbps: parts[12].to_string(),
        etag: parts[13].to_string(),
        content_encoding: parts[14].to_string(),
        cf_cache_status: parts[15].to_string(),
        result: parts[16].to_string(),
    })
}

fn update_remote_state(
    sess: &Session,
    pack: &MediaPack,
    variant: &str,
    row: &RemoteBenchRow,
) -> Result<()> {
    let cmd = format!("cat {} 2>/dev/null || true", shell_quote(REMOTE_STATE_PATH));
    let current = exec_stdout(sess, &cmd)?;
    let mut root = serde_json::from_str::<Value>(&current)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    root.insert("schema_version".to_string(), json!(1));
    root.insert("updated_at_unix_ms".to_string(), json!(unix_ms_now()));
    let mut systems = root
        .remove("systems")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    systems.insert(
        pack.system.clone(),
        json!({
            "local_path": pack.local_path,
            "sha256": pack.identity.decoded_sha256,
            "bytes": pack.identity.decoded_bytes,
            "asset_count": pack.asset_count,
            "etag": pack.identity.etag,
            "preferred_variant": variant,
            "last_result": row.result,
            "last_total_ms": row.total_ms,
            "last_download_ms": row.download_ms,
            "last_decompress_ms": row.decompress_ms,
            "last_save_ms": row.save_ms,
            "last_verify_ms": row.verify_ms,
        }),
    );
    root.insert("systems".to_string(), Value::Object(systems));
    let bytes = serde_json::to_vec_pretty(&Value::Object(root))?;
    let tmp = format!("/tmp/mister-magik-media-state-{}.json", unix_ms_now());
    sftp_write(sess, &tmp, &bytes)?;
    let publish = format!(
        "mkdir -p {dir}; cp {tmp} {state}.tmp; sync {state}.tmp 2>/dev/null || sync; mv {state}.tmp {state}; sync {dir} 2>/dev/null || sync; rm -f {tmp}",
        dir = shell_quote(DEFAULT_REMOTE_ASSET_DIR),
        tmp = shell_quote(&tmp),
        state = shell_quote(REMOTE_STATE_PATH),
    );
    let out = exec(sess, &publish)?;
    if out.rc != 0 {
        return Err(format!("failed to update remote media state: {}", out.stdout).into());
    }
    Ok(())
}

fn manifest_url_for_pack(manifest: &MediaManifest, pack: &MediaPack) -> String {
    if pack.identity.remote_path.starts_with("http://")
        || pack.identity.remote_path.starts_with("https://")
    {
        pack.identity.remote_path.clone()
    } else {
        format!(
            "{}/{}",
            manifest.base_url.trim_end_matches('/'),
            pack.identity.remote_path.trim_start_matches('/')
        )
    }
}

fn accept_encoding_for_variant(variant: &str) -> &'static str {
    match variant {
        "gzip" => "gzip",
        "brotli" => "br",
        "identity" | "auto" => "identity",
        _ => "identity",
    }
}

fn curl_headers(url: &str, accept_encoding: &str) -> Result<BTreeMap<String, String>> {
    let out = Command::new("curl")
        .args([
            "-fsSI",
            "-H",
            &format!("Accept-Encoding: {accept_encoding}"),
            url,
        ])
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "curl header probe failed for {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut headers = BTreeMap::new();
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Ok(headers)
}

fn header_value<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers.get(&name.to_ascii_lowercase()).map(String::as_str)
}

fn cloudflare_api_probe() -> Result<()> {
    let (Ok(zone), Ok(token)) = (
        env::var("CLOUDFLARE_ZONE_ID"),
        env::var("CLOUDFLARE_API_TOKEN"),
    ) else {
        println!("cloudflare_api_probe\tskipped\tset CLOUDFLARE_ZONE_ID and CLOUDFLARE_API_TOKEN");
        return Ok(());
    };
    let brotli = cloudflare_api_get(&zone, &token, "settings/brotli")?;
    println!("cloudflare_api_brotli\t{}", one_line_json(&brotli));
    match cloudflare_api_get(
        &zone,
        &token,
        "rulesets/phases/http_response_compression/entrypoint",
    ) {
        Ok(ruleset) => println!(
            "cloudflare_api_compression_rules\t{}",
            one_line_json(&ruleset)
        ),
        Err(err) => println!("cloudflare_api_compression_rules\terror\t{err}"),
    }
    Ok(())
}

fn cloudflare_api_get(zone: &str, token: &str, path: &str) -> Result<Value> {
    let url = format!("https://api.cloudflare.com/client/v4/zones/{zone}/{path}");
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            &format!("Authorization: Bearer {token}"),
            &url,
        ])
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .to_string()
            .into());
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

fn one_line_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
}

fn remote_script() -> String {
    r#"#!/bin/sh
set -u
mode="$1"
label="$2"
system="$3"
variant="$4"
accept_encoding="$5"
url="$6"
local_path="$7"
expected_sha="$8"
expected_bytes="$9"
asset_dir="$(dirname "$local_path")"
work_dir="${MISTER_MEDIA_WORK_DIR:-/tmp/mister-magik-media-download}"
mkdir -p "$work_dir" "$asset_dir"
stamp="$$-$(awk '{ printf "%d", $1 * 1000 }' /proc/uptime)"
encoded="$work_dir/$system.$variant.$stamp.encoded"
decoded="$work_dir/$system.$variant.$stamp.decoded"
headers="$work_dir/$system.$variant.$stamp.headers"
final_tmp="$asset_dir/.$system-screenshots.$stamp.tmp"
result="downloaded"
content_encoding="identity"
cf_cache_status=""
etag=""
download_ms=0
decompress_ms=0
save_ms=0
verify_ms=0
total_start="$(awk '{ printf "%d", $1 * 1000 }' /proc/uptime)"

ms_now() {
  awk '{ printf "%d", $1 * 1000 }' /proc/uptime
}

elapsed() {
  now="$(ms_now)"
  echo $((now - $1))
}

mbps() {
  bytes="$1"
  ms="$2"
  if [ "$ms" -le 0 ]; then
    echo "0.00"
  else
    awk -v b="$bytes" -v ms="$ms" 'BEGIN { printf "%.2f", (b * 8.0) / (ms * 1000.0) }'
  fi
}

cleanup() {
  rm -f "$encoded" "$decoded" "$headers" "$final_tmp"
}

finish() {
  total_ms="$(elapsed "$total_start")"
  encoded_bytes="$(wc -c < "$encoded" 2>/dev/null || echo 0)"
  decoded_bytes="$(wc -c < "$decoded" 2>/dev/null || echo 0)"
  wire_mbps="$(mbps "$encoded_bytes" "$download_ms")"
  decoded_mbps="$(mbps "$decoded_bytes" "$total_ms")"
  printf 'screenshot_download_bench_tsv\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$label" "$system" "$variant" "$encoded_bytes" "$decoded_bytes" "$download_ms" "$decompress_ms" "$save_ms" "$verify_ms" "$total_ms" "$wire_mbps" "$decoded_mbps" "$etag" "$content_encoding" "$cf_cache_status" "$result"
  cleanup
  case "$result" in
    downloaded|bench-ok) exit 0 ;;
    *) exit 1 ;;
  esac
}

if ! command -v wget >/dev/null 2>&1; then
  result="missing-wget"
  finish
fi
if ! command -v sha256sum >/dev/null 2>&1; then
  result="missing-sha256sum"
  finish
fi

t="$(ms_now)"
wget -S --header "Accept-Encoding: $accept_encoding" -O "$encoded" "$url" >"$headers" 2>&1
rc=$?
download_ms="$(elapsed "$t")"
content_encoding="$(grep -i '^[[:space:]]*Content-Encoding:' "$headers" 2>/dev/null | tail -n 1 | sed 's/.*:[[:space:]]*//' | tr -d '\r')"
cf_cache_status="$(grep -i '^[[:space:]]*cf-cache-status:' "$headers" 2>/dev/null | tail -n 1 | sed 's/.*:[[:space:]]*//' | tr -d '\r')"
etag="$(grep -i '^[[:space:]]*etag:' "$headers" 2>/dev/null | tail -n 1 | sed 's/.*:[[:space:]]*//' | tr -d '\r')"
if [ -z "$content_encoding" ]; then
  content_encoding="identity"
fi
if [ "$rc" -ne 0 ]; then
  result="download-failed-$rc"
  finish
fi

t="$(ms_now)"
case "$content_encoding" in
  identity|none)
    cp "$encoded" "$decoded"
    rc=$?
    ;;
  gzip|x-gzip)
    gzip -dc "$encoded" > "$decoded"
    rc=$?
    ;;
  br)
    if command -v brotli >/dev/null 2>&1; then
      brotli -d -c "$encoded" > "$decoded"
      rc=$?
    else
      rc=127
      result="decode-unavailable-br"
    fi
    ;;
  *)
    rc=126
    result="unsupported-content-encoding-$content_encoding"
    ;;
esac
decompress_ms="$(elapsed "$t")"
if [ "$rc" -ne 0 ]; then
  if [ "$result" = "downloaded" ]; then
    result="decompress-failed-$rc"
  fi
  finish
fi

t="$(ms_now)"
got_sha="$(sha256sum "$decoded" | awk '{print $1}')"
verify_ms="$(elapsed "$t")"
if [ "$got_sha" != "$expected_sha" ]; then
  result="sha256-mismatch"
  finish
fi
actual_bytes="$(wc -c < "$decoded")"
if [ "$actual_bytes" != "$expected_bytes" ]; then
  result="size-mismatch"
  finish
fi

t="$(ms_now)"
if [ "$mode" = "publish" ]; then
  cat "$decoded" > "$final_tmp"
  sync "$final_tmp" 2>/dev/null || sync
  mv "$final_tmp" "$local_path"
  sync "$asset_dir" 2>/dev/null || sync
else
  cat "$decoded" > "$final_tmp"
  sync "$final_tmp" 2>/dev/null || sync
  rm -f "$final_tmp"
  sync "$asset_dir" 2>/dev/null || sync
  result="bench-ok"
fi
save_ms="$(elapsed "$t")"
finish
"#
    .to_string()
}

fn exec(sess: &Session, command: &str) -> Result<ExecOutput> {
    let mut channel = sess.channel_session()?;
    channel.handle_extended_data(ExtendedData::Normal)?;
    channel.exec(command)?;
    let mut stdout = String::new();
    channel.read_to_string(&mut stdout)?;
    let mut stderr = String::new();
    channel.stderr().read_to_string(&mut stderr)?;
    channel.wait_close()?;
    Ok(ExecOutput {
        rc: channel.exit_status()?,
        stdout,
        stderr,
    })
}

struct ExecOutput {
    rc: i32,
    stdout: String,
    stderr: String,
}

fn exec_stdout(sess: &Session, command: &str) -> Result<String> {
    let out = exec(sess, command)?;
    if out.rc != 0 {
        Err(format!("remote command failed: {}", out.stdout).into())
    } else {
        Ok(out.stdout)
    }
}

fn sftp_write(sess: &Session, remote: &str, bytes: &[u8]) -> Result<()> {
    let sftp = sess.sftp()?;
    let mut file = sftp.create(Path::new(remote))?;
    file.write_all(bytes)?;
    Ok(())
}

fn append_profile_row(path: &str, header: &str, row: &str) -> Result<()> {
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let needs_header = !path.exists() || path.metadata()?.len() == 0;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if needs_header {
        writeln!(file, "{header}")?;
    }
    writeln!(file, "{row}")?;
    Ok(())
}

fn default_label() -> String {
    format!("screenshot-download-{}", unix_ms_now())
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn tsv(s: &str) -> String {
    s.replace(['\t', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_phase_one_manifest_shape() {
        let value = json!({
            "schema_version": 1,
            "published_at": "2026-06-22T00:00:00Z",
            "base_url": "https://media.example.test/screenshots/v1",
            "packs": [{
                "system": "megadrive",
                "remote_path": "packs/megadrive/megadrive-screenshots.mmlz4b",
                "local_path": "/media/fat/mister-magik/assets/megadrive-screenshots.mmlz4b",
                "bytes": 123,
                "sha256": "abcdef",
                "etag": "\"abc\"",
                "asset_count": 42
            }]
        });

        let manifest = parse_manifest(&value).unwrap();

        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.packs[0].system, "megadrive");
        assert_eq!(manifest.packs[0].identity.decoded_bytes, 123);
        assert_eq!(
            manifest_url_for_pack(&manifest, &manifest.packs[0]),
            "https://media.example.test/screenshots/v1/packs/megadrive/megadrive-screenshots.mmlz4b"
        );
    }

    #[test]
    fn parses_variant_manifest_shape() {
        let value = json!({
            "schema_version": 2,
            "base_url": "https://media.example.test",
            "packs": [{
                "system": "nes",
                "variants": {
                    "identity": {
                        "remote_path": "/packs/nes.mmlz4b",
                        "decoded_bytes": 99,
                        "sha256_decoded": "ABCDEF"
                    }
                }
            }]
        });

        let manifest = parse_manifest(&value).unwrap();

        assert_eq!(
            manifest.packs[0].local_path,
            "/media/fat/mister-magik/assets/nes-screenshots.mmlz4b"
        );
        assert_eq!(manifest.packs[0].identity.decoded_sha256, "abcdef");
    }

    #[test]
    fn parses_remote_benchmark_row() {
        let row = parse_remote_row(
            "screenshot_download_bench_tsv\tlabel\tnes\tgzip\t10\t20\t1\t2\t3\t4\t10\t80.00\t16.00\tetag\tgzip\tHIT\tbench-ok",
        )
        .unwrap();

        assert_eq!(row.variant, "gzip");
        assert_eq!(row.decompress_ms, 2);
        assert_eq!(row.save_ms, 3);
        assert_eq!(row.result, "bench-ok");
    }
}
