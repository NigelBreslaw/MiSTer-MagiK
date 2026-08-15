// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_media_contract::ManifestTrustMode;
use serde_json::{Map, Value, json};
use ssh2::{ExtendedData, Session};
use std::env;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

static DEFAULT_REMOTE_ASSET_DIR: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}/assets",
        mister_magik_platform_manifest_contract::PUBLIC_PATHS.root
    )
});
#[cfg(test)]
static DEFAULT_ARCADE_ARCHIVE_PATH: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}/arcade-screenshots.mmlz4b",
        DEFAULT_REMOTE_ASSET_DIR.as_str()
    )
});
const DEFAULT_IMAGE_SIZE: &str = "320x320";
const DEFAULT_MANIFEST_URL: &str = mister_magik_media_contract::DEFAULT_MANIFEST_URL;
const OFFICIAL_ASSET_HTTPS_ORIGIN: &str = mister_magik_media_contract::OFFICIAL_ASSET_HTTPS_ORIGIN;
const OFFICIAL_ASSET_HTTP_ORIGIN: &str = mister_magik_media_contract::OFFICIAL_ASSET_HTTP_ORIGIN;
const OFFICIAL_PACK_OBJECT_PREFIX: &str = "mister-magik/v1/packs/";

fn remote_asset_dir() -> String {
    env::var("MISTER_MAGIK_ASSET_DIR").unwrap_or_else(|_| DEFAULT_REMOTE_ASSET_DIR.to_string())
}

fn remote_state_path() -> String {
    format!("{}/.screenshot-media-state.json", remote_asset_dir())
}

fn layout_local_path(path: &str) -> String {
    if let Some(suffix) = path.strip_prefix(DEFAULT_REMOTE_ASSET_DIR.as_str()) {
        format!("{}{suffix}", remote_asset_dir())
    } else {
        path.to_string()
    }
}

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
    version: String,
    image_size: String,
    local_path: String,
    asset_count: Option<u64>,
    identity: MediaVariant,
    index: Option<MediaIndex>,
}

#[derive(Clone, Debug)]
struct MediaVariant {
    remote_path: String,
    decoded_bytes: u64,
    decoded_sha256: String,
    etag: Option<String>,
}

#[derive(Clone, Debug)]
struct MediaIndex {
    object: String,
    bytes: u64,
    sha256: String,
    codec: String,
    archive_bytes: u64,
    archive_sha256: String,
}

#[derive(Clone, Debug)]
struct MediaArgs {
    manifest_url: String,
    system: String,
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
            "media_check\t{}\t{}\tlocal_path={}\tremote_url={}\tindex_url={}\tetag={}",
            pack.system,
            status,
            pack.local_path,
            manifest_url_for_pack(&manifest, pack),
            manifest_url_for_index(&manifest, pack).unwrap_or_default(),
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
        let status = remote_pack_status(sess, pack)?;
        if status == "current" {
            println!(
                "media_download\t{}\tskipped-current\tlocal_path={}",
                pack.system, pack.local_path
            );
            continue;
        }
        let mut pack_row = None;
        let mut index_row = None;
        let index_only_repair = status == "index-missing" || status.starts_with("index-stale:");
        if !index_only_repair {
            let row = run_remote_media_download(
                sess,
                &manifest,
                pack,
                &default_label(),
                "identity",
                true,
            )?;
            println!("{}", row.to_tsv());
            if row.result != "downloaded" {
                return Err(
                    format!("media download failed for {}: {}", pack.system, row.result).into(),
                );
            }
            pack_row = Some(row);
        }
        if pack.index.is_some() {
            let row = run_remote_index_download(sess, &manifest, pack, &default_label(), true)?;
            println!("{}", row.to_tsv());
            if row.result != "downloaded" {
                return Err(format!(
                    "media index download failed for {}: {}",
                    pack.system, row.result
                )
                .into());
            }
            index_row = Some(row);
        }
        update_remote_state_after_download(
            sess,
            pack,
            "identity",
            pack_row.as_ref(),
            index_row.as_ref(),
        )?;
    }
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
        manifest_url: env::var("MISTER_MEDIA_MANIFEST_URL")
            .unwrap_or_else(|_| DEFAULT_MANIFEST_URL.to_string()),
        system: "all".to_string(),
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
            "-h" | "--help" => {
                media_usage();
                return Err("help requested".into());
            }
            other => return Err(format!("unknown media option: {other}").into()),
        }
        idx += 1;
    }
    Ok(parsed)
}

pub(crate) fn media_usage() {
    println!("media options: --manifest-url URL --system id|all");
}

fn load_manifest(url: &str) -> Result<MediaManifest> {
    load_manifest_with(
        url,
        mister_magik_media_contract::configured_manifest_trust_mode(),
        fetch_https_bytes,
        |manifest, signature| {
            mister_magik_media_contract::verify_manifest_signature(manifest, signature)
                .map(|_| ())
                .map_err(Into::into)
        },
    )
}

fn load_manifest_with<F, V>(
    url: &str,
    trust_mode: ManifestTrustMode,
    mut fetch: F,
    mut verify: V,
) -> Result<MediaManifest>
where
    F: FnMut(&str, u64, &str) -> Result<Vec<u8>>,
    V: FnMut(&[u8], &[u8]) -> Result<()>,
{
    mister_magik_media_contract::validate_https_manifest_url(url)?;
    let manifest = fetch(
        url,
        mister_magik_media_contract::MAX_MANIFEST_BYTES,
        "media manifest",
    )?;
    if trust_mode == ManifestTrustMode::SignedHttps {
        let signature_url = mister_magik_media_contract::manifest_signature_url(url)?;
        let signature = fetch(
            &signature_url,
            mister_magik_media_contract::MAX_MANIFEST_SIGNATURE_BYTES,
            "media manifest signature",
        )?;
        verify(&manifest, &signature)?;
    }
    parse_manifest(&serde_json::from_slice(&manifest)?, url)
}

fn fetch_https_bytes(url: &str, max_bytes: u64, label: &str) -> Result<Vec<u8>> {
    let mut command = Command::new("curl");
    add_manifest_curl_args(&mut command, url, max_bytes);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_child(&mut child);
            return Err(format!("missing curl stdout for {label}").into());
        }
    };
    let mut bytes = Vec::new();
    let read_result = stdout
        .by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes);
    if read_result.is_err() || bytes.len() as u64 > max_bytes {
        terminate_child(&mut child);
    }
    let output = child.wait_with_output()?;
    read_result?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!("{label} exceeds {max_bytes} bytes").into());
    }
    if !output.status.success() {
        return Err(format!(
            "failed to fetch {label} from {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(bytes)
}

fn add_manifest_curl_args(command: &mut Command, url: &str, max_bytes: u64) {
    command
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            "15",
            "--max-filesize",
        ])
        .arg(max_bytes.to_string())
        .args(["--header", "Accept-Encoding: identity", "-o", "-", url]);
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
}

fn parse_manifest(value: &Value, manifest_url: &str) -> Result<MediaManifest> {
    let schema_version = value
        .get("schema_version")
        .or_else(|| value.get("schema"))
        .and_then(Value::as_u64)
        .ok_or("manifest missing schema_version/schema")?;
    let published_at = value
        .get("published_at")
        .or_else(|| value.get("generated_at"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let base_url = value
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| manifest_object_base_url(manifest_url))
        .trim_end_matches('/')
        .to_string();
    let pack_values = value
        .get("packs")
        .and_then(Value::as_array)
        .ok_or("manifest missing packs array")?;
    let mut packs = Vec::new();
    for pack in pack_values {
        let system = pack
            .get("system")
            .or_else(|| pack.get("id"))
            .and_then(Value::as_str)
            .ok_or("manifest pack missing system/id")?
            .to_string();
        let local_path = layout_local_path(
            &pack
                .get("local_path")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| default_local_path_for_pack(&system)),
        );
        let asset_count = pack.get("asset_count").and_then(Value::as_u64);
        let version = pack
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or(&published_at)
            .to_string();
        let image_size =
            image_size_from_pack(pack).unwrap_or_else(|| DEFAULT_IMAGE_SIZE.to_string());
        let identity_value = pack
            .get("variants")
            .and_then(|variants| variants.get("identity"))
            .unwrap_or(pack);
        let identity = MediaVariant {
            remote_path: identity_value
                .get("remote_path")
                .or_else(|| identity_value.get("object"))
                .or_else(|| pack.get("remote_path"))
                .or_else(|| pack.get("object"))
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
        if identity.decoded_bytes == 0 {
            return Err(format!("pack {system} has zero bytes").into());
        }
        if identity.decoded_bytes > mister_magik_media_contract::MAX_MEDIA_PACK_BYTES {
            return Err(format!(
                "pack {system} exceeds {} bytes",
                mister_magik_media_contract::MAX_MEDIA_PACK_BYTES
            )
            .into());
        }
        mister_magik_media_contract::Sha256::parse(&identity.decoded_sha256)
            .map_err(|error| format!("pack {system} {error}"))?;
        if matches!(
            base_url.as_str(),
            OFFICIAL_ASSET_HTTPS_ORIGIN | OFFICIAL_ASSET_HTTP_ORIGIN
        ) && !identity.remote_path.starts_with("http://")
            && !identity.remote_path.starts_with("https://")
        {
            mister_magik_media_contract::validate_pack_object_path(
                identity.remote_path.trim_start_matches('/'),
            )?;
        }
        let codec = pack
            .get("codec")
            .and_then(Value::as_str)
            .unwrap_or("mmlz4b");
        if codec != "mmlz4b" {
            return Err(format!("pack {system} uses unsupported codec {codec}").into());
        }
        let index = pack
            .get("index")
            .map(|value| parse_index(&system, &identity, value))
            .transpose()?;
        packs.push(MediaPack {
            system,
            version,
            image_size,
            local_path,
            asset_count,
            identity,
            index,
        });
    }
    Ok(MediaManifest {
        schema_version,
        published_at,
        base_url,
        packs,
    })
}

fn manifest_object_base_url(manifest_url: &str) -> String {
    mister_magik_media_contract::manifest_origin(manifest_url).unwrap_or_default()
}

fn default_local_path_for_pack(system: &str) -> String {
    match system {
        "arcade" => format!("{}/arcade-screenshots.mmlz4b", remote_asset_dir()),
        other => format!("{}/{other}-screenshots.mmlz4b", remote_asset_dir()),
    }
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
    let cmd = if let Some(index) = &pack.index {
        format!(
            "if [ ! -f {path} ]; then echo missing; exit 0; fi; got=$(sha256sum {path} 2>/dev/null | awk '{{print $1}}'); if [ \"$got\" != {sha} ]; then echo stale:$got; exit 0; fi; if [ ! -f {index_path} ]; then echo index-missing; exit 0; fi; idxgot=$(sha256sum {index_path} 2>/dev/null | awk '{{print $1}}'); if [ \"$idxgot\" != {index_sha} ]; then echo index-stale:$idxgot; exit 0; fi; echo current",
            path = shell_quote(&pack.local_path),
            sha = shell_quote(&pack.identity.decoded_sha256),
            index_path = shell_quote(&local_index_path_for_pack(pack)),
            index_sha = shell_quote(&index.sha256),
        )
    } else {
        format!(
            "if [ -f {path} ]; then got=$(sha256sum {path} 2>/dev/null | awk '{{print $1}}'); if [ \"$got\" = {sha} ]; then echo current; else echo stale:$got; fi; else echo missing; fi",
            path = shell_quote(&pack.local_path),
            sha = shell_quote(&pack.identity.decoded_sha256),
        )
    };
    Ok(exec_stdout(sess, &cmd)?.trim().to_string())
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
    let accept = "identity";
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

fn run_remote_index_download(
    sess: &Session,
    manifest: &MediaManifest,
    pack: &MediaPack,
    label: &str,
    publish: bool,
) -> Result<RemoteBenchRow> {
    let Some(index) = &pack.index else {
        return Err(format!("pack {} has no media index", pack.system).into());
    };
    let script = remote_script();
    let remote_script = format!("/tmp/mister-magik-media-index-{}.sh", unix_ms_now());
    sftp_write(sess, &remote_script, script.as_bytes())?;
    let url = manifest_url_for_index(manifest, pack).ok_or("pack index URL missing")?;
    let mode = if publish { "publish" } else { "bench" };
    let cmd = format!(
        "chmod +x {script}; {script} {mode} {label} {system} index identity {url} {local} {sha} {bytes}; rc=$?; rm -f {script}; exit $rc",
        script = shell_quote(&remote_script),
        mode = shell_quote(mode),
        label = shell_quote(label),
        system = shell_quote(&format!("{}-index", pack.system)),
        url = shell_quote(&url),
        local = shell_quote(&local_index_path_for_pack(pack)),
        sha = shell_quote(&index.sha256),
        bytes = shell_quote(&index.bytes.to_string()),
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
                "remote index media runner produced no benchmark row: {}",
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
    if parts.len() < 17 || parts.first() != Some(&"screenshot_download_bench_tsv") {
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

fn update_remote_state_after_download(
    sess: &Session,
    pack: &MediaPack,
    variant: &str,
    row: Option<&RemoteBenchRow>,
    index_row: Option<&RemoteBenchRow>,
) -> Result<()> {
    let cmd = format!(
        "cat {} 2>/dev/null || true",
        shell_quote(&remote_state_path())
    );
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
    let mut entry = Map::new();
    entry.insert("local_path".to_string(), json!(pack.local_path));
    entry.insert("version".to_string(), json!(pack.version));
    entry.insert("image_size".to_string(), json!(pack.image_size));
    entry.insert("sha256".to_string(), json!(pack.identity.decoded_sha256));
    entry.insert("bytes".to_string(), json!(pack.identity.decoded_bytes));
    entry.insert("asset_count".to_string(), json!(pack.asset_count));
    entry.insert("etag".to_string(), json!(pack.identity.etag));
    entry.insert("preferred_variant".to_string(), json!(variant));
    if let Some(row) = row {
        entry.insert("last_result".to_string(), json!(row.result));
        entry.insert("last_total_ms".to_string(), json!(row.total_ms));
        entry.insert("last_download_ms".to_string(), json!(row.download_ms));
        entry.insert("last_decompress_ms".to_string(), json!(row.decompress_ms));
        entry.insert("last_save_ms".to_string(), json!(row.save_ms));
        entry.insert("last_verify_ms".to_string(), json!(row.verify_ms));
    }
    if let Some(index) = &pack.index {
        let mut index_entry = Map::new();
        index_entry.insert("codec".to_string(), json!(index.codec));
        index_entry.insert("object".to_string(), json!(index.object));
        index_entry.insert("bytes".to_string(), json!(index.bytes));
        index_entry.insert("sha256".to_string(), json!(index.sha256));
        index_entry.insert("archive_bytes".to_string(), json!(index.archive_bytes));
        index_entry.insert("archive_sha256".to_string(), json!(index.archive_sha256));
        index_entry.insert(
            "local_path".to_string(),
            json!(local_index_path_for_pack(pack)),
        );
        if let Some(row) = index_row {
            index_entry.insert("last_result".to_string(), json!(row.result));
            index_entry.insert("last_total_ms".to_string(), json!(row.total_ms));
            index_entry.insert("last_download_ms".to_string(), json!(row.download_ms));
            index_entry.insert("last_save_ms".to_string(), json!(row.save_ms));
            index_entry.insert("last_verify_ms".to_string(), json!(row.verify_ms));
        }
        entry.insert("index".to_string(), Value::Object(index_entry));
    }
    systems.insert(pack.system.clone(), Value::Object(entry));
    root.insert("systems".to_string(), Value::Object(systems));
    let bytes = serde_json::to_vec_pretty(&Value::Object(root))?;
    let tmp = format!("/tmp/mister-magik-media-state-{}.json", unix_ms_now());
    sftp_write(sess, &tmp, &bytes)?;
    let publish = format!(
        "mkdir -p {dir}; cp {tmp} {state}.tmp; sync {state}.tmp 2>/dev/null || sync; mv {state}.tmp {state}; sync {dir} 2>/dev/null || sync; rm -f {tmp}",
        dir = shell_quote(&remote_asset_dir()),
        tmp = shell_quote(&tmp),
        state = shell_quote(&remote_state_path()),
    );
    let out = exec(sess, &publish)?;
    if out.rc != 0 {
        return Err(format!("failed to update remote media state: {}", out.stdout).into());
    }
    Ok(())
}

fn parse_index(system: &str, identity: &MediaVariant, value: &Value) -> Result<MediaIndex> {
    let index = MediaIndex {
        object: value
            .get("object")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("pack {system} index missing object"))?
            .to_string(),
        bytes: value
            .get("bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("pack {system} index missing bytes"))?,
        sha256: value
            .get("sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("pack {system} index missing sha256"))?
            .to_ascii_lowercase(),
        codec: value
            .get("codec")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("pack {system} index missing codec"))?
            .to_string(),
        archive_bytes: value
            .get("archive_bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("pack {system} index missing archive_bytes"))?,
        archive_sha256: value
            .get("archive_sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("pack {system} index missing archive_sha256"))?
            .to_ascii_lowercase(),
    };
    if index.codec != "mmlz4b-index-v1" && index.codec != "mmlz4b-index-v2" {
        return Err(format!("pack {system} uses unsupported index codec {}", index.codec).into());
    }
    mister_magik_media_contract::Sha256::parse(&index.sha256)
        .map_err(|error| format!("pack {system} index {error}"))?;
    mister_magik_media_contract::Sha256::parse(&index.archive_sha256)
        .map_err(|error| format!("pack {system} index archive {error}"))?;
    if index.bytes == 0 {
        return Err(format!("pack {system} index has zero bytes").into());
    }
    if index.bytes > mister_magik_media_contract::MAX_MEDIA_INDEX_BYTES {
        return Err(format!(
            "pack {system} index exceeds {} bytes",
            mister_magik_media_contract::MAX_MEDIA_INDEX_BYTES
        )
        .into());
    }
    if !index.object.ends_with(".mmlz4b.idx") {
        return Err(format!("pack {system} index object must end with .mmlz4b.idx").into());
    }
    if index.archive_bytes != identity.decoded_bytes {
        return Err(format!(
            "pack {system} index archive_bytes mismatch expected={} got={}",
            identity.decoded_bytes, index.archive_bytes
        )
        .into());
    }
    if index.archive_sha256 != identity.decoded_sha256 {
        return Err(format!("pack {system} index archive_sha256 mismatch").into());
    }
    Ok(index)
}

fn image_size_from_pack(value: &Value) -> Option<String> {
    for key in [
        "image_size",
        "preview_size",
        "pack_size",
        "size",
        "resolution",
    ] {
        let Some(size) = value.get(key).and_then(Value::as_str) else {
            continue;
        };
        if valid_image_size(size) {
            return Some(size.to_string());
        }
    }
    let width = value
        .get("image_width")
        .or_else(|| value.get("width"))
        .and_then(Value::as_u64)?;
    let height = value
        .get("image_height")
        .or_else(|| value.get("height"))
        .and_then(Value::as_u64)?;
    let size = format!("{width}x{height}");
    valid_image_size(&size).then_some(size)
}

fn valid_image_size(value: &str) -> bool {
    let Some((width, height)) = value.split_once('x') else {
        return false;
    };
    let Ok(width) = width.parse::<u32>() else {
        return false;
    };
    let Ok(height) = height.parse::<u32>() else {
        return false;
    };
    width > 0 && height > 0
}

fn manifest_url_for_pack(manifest: &MediaManifest, pack: &MediaPack) -> String {
    manifest_url_for_object(manifest, &pack.identity.remote_path)
}

fn manifest_url_for_index(manifest: &MediaManifest, pack: &MediaPack) -> Option<String> {
    pack.index
        .as_ref()
        .map(|index| manifest_url_for_object(manifest, &index.object))
}

fn manifest_url_for_object(manifest: &MediaManifest, object: &str) -> String {
    if object.starts_with("http://") || object.starts_with("https://") {
        object.to_string()
    } else if manifest.base_url == OFFICIAL_ASSET_HTTPS_ORIGIN
        && object
            .trim_start_matches('/')
            .starts_with(OFFICIAL_PACK_OBJECT_PREFIX)
    {
        format!(
            "{}/{}",
            OFFICIAL_ASSET_HTTP_ORIGIN,
            object.trim_start_matches('/')
        )
    } else {
        format!(
            "{}/{}",
            manifest.base_url.trim_end_matches('/'),
            object.trim_start_matches('/')
        )
    }
}

fn local_index_path_for_pack(pack: &MediaPack) -> String {
    format!("{}.idx", pack.local_path)
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
body_pipe="$work_dir/$system.$variant.$stamp.body-pipe"
decode_pipe="$work_dir/$system.$variant.$stamp.decode-pipe"
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
  rm -f "$encoded" "$decoded" "$headers" "$body_pipe" "$decode_pipe" "$final_tmp"
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

if ! command -v curl >/dev/null 2>&1; then
  result="missing-curl"
  finish
fi
if ! command -v sha256sum >/dev/null 2>&1; then
  result="missing-sha256sum"
  finish
fi

t="$(ms_now)"
download_limit=134217728
if [ "$accept_encoding" = "identity" ] && [ "$expected_bytes" -lt "$download_limit" ]; then
  download_limit="$expected_bytes"
fi
rm -f "$body_pipe"
if ! mkfifo "$body_pipe"; then
  result="download-pipe-failed"
  finish
fi
if echo "$url" | grep -q '^https://' && [ -f /etc/ssl/certs/cacert.pem ]; then
  curl --fail --silent --show-error --proto '=http,https' \
    --connect-timeout 10 --max-time 1200 --max-filesize "$download_limit" \
    --cacert /etc/ssl/certs/cacert.pem \
    -H "Accept-Encoding: $accept_encoding" -D "$headers" -o "$body_pipe" "$url" &
else
  curl --fail --silent --show-error --proto '=http,https' \
    --connect-timeout 10 --max-time 1200 --max-filesize "$download_limit" \
    -H "Accept-Encoding: $accept_encoding" -D "$headers" -o "$body_pipe" "$url" &
fi
curl_pid=$!
exec 3<"$body_pipe"
head -c "$download_limit" <&3 > "$encoded"
head_rc=$?
extra_bytes="$(dd bs=1 count=1 <&3 2>/dev/null | wc -c)"
exec 3<&-
wait "$curl_pid"
rc=$?
rm -f "$body_pipe"
download_ms="$(elapsed "$t")"
content_encoding="$(grep -i '^[[:space:]]*Content-Encoding:' "$headers" 2>/dev/null | tail -n 1 | sed 's/.*:[[:space:]]*//' | tr -d '\r')"
cf_cache_status="$(grep -i '^[[:space:]]*cf-cache-status:' "$headers" 2>/dev/null | tail -n 1 | sed 's/.*:[[:space:]]*//' | tr -d '\r')"
etag="$(grep -i '^[[:space:]]*etag:' "$headers" 2>/dev/null | tail -n 1 | sed 's/.*:[[:space:]]*//' | tr -d '\r')"
if [ -z "$content_encoding" ]; then
  content_encoding="identity"
fi
if [ "$head_rc" -ne 0 ]; then
  result="download-bound-failed-$head_rc"
  finish
fi
if [ "$extra_bytes" -ne 0 ]; then
  result="download-size-limit"
  finish
fi
if [ "$rc" -ne 0 ]; then
  result="download-failed-$rc"
  finish
fi
encoded_bytes="$(wc -c < "$encoded")"

t="$(ms_now)"
rm -f "$decode_pipe"
if ! mkfifo "$decode_pipe"; then
  result="decode-pipe-failed"
  finish
fi
case "$content_encoding" in
  identity|none)
    cat "$encoded" > "$decode_pipe" &
    ;;
  gzip|x-gzip)
    gzip -dc "$encoded" > "$decode_pipe" &
    ;;
  br)
    if command -v brotli >/dev/null 2>&1; then
      brotli -d -c "$encoded" > "$decode_pipe" &
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
if [ "$result" = "downloaded" ]; then
  decode_pid=$!
  exec 3<"$decode_pipe"
  head -c "$expected_bytes" <&3 > "$decoded"
  decode_head_rc=$?
  extra_bytes="$(dd bs=1 count=1 <&3 2>/dev/null | wc -c)"
  exec 3<&-
  wait "$decode_pid"
  rc=$?
  rm -f "$decode_pipe"
  if [ "$extra_bytes" -ne 0 ]; then
    result="size-mismatch"
    finish
  fi
  if [ "$decode_head_rc" -ne 0 ]; then
    result="decode-bound-failed-$decode_head_rc"
    finish
  fi
fi
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

    fn manifest_fetch_fixture() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": 1,
            "generated_at": "2026-07-27T00:00:00Z",
            "packs": [{
                "id": "arcade",
                "object": "mister-magik/v1/packs/arcade/screenshots/320x320/v1/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.mmlz4b",
                "bytes": 3,
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "codec": "mmlz4b"
            }]
        }))
        .unwrap()
    }

    #[test]
    fn manifest_curl_is_https_only_and_bounded() {
        let mut command = Command::new("curl");
        add_manifest_curl_args(
            &mut command,
            "https://assets.example/manifest.json",
            mister_magik_media_contract::MAX_MANIFEST_BYTES,
        );
        let text = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");

        assert!(text.contains("--proto =https"));
        assert!(text.contains("--proto-redir =https"));
        assert!(text.contains("--connect-timeout 10"));
        assert!(text.contains("--max-time 15"));
        assert!(text.contains("--max-filesize 262144"));
    }

    #[test]
    fn unsigned_host_manifest_fetch_skips_signature_and_verification() {
        let bytes = manifest_fetch_fixture();
        let mut urls = Vec::new();
        let manifest = load_manifest_with(
            "https://assets.mistermagik.com/mister-magik/v1/manifest.json",
            ManifestTrustMode::UnsignedHttps,
            |url, _, _| {
                urls.push(url.to_string());
                Ok(bytes.clone())
            },
            |_, _| panic!("unsigned mode must not verify a signature"),
        )
        .unwrap();

        assert_eq!(
            urls,
            ["https://assets.mistermagik.com/mister-magik/v1/manifest.json"]
        );
        assert_eq!(manifest.packs.len(), 1);
    }

    #[test]
    fn signed_host_manifest_fetch_requests_and_verifies_signature() {
        let manifest_bytes = manifest_fetch_fixture();
        let signature_bytes = b"signature envelope".to_vec();
        let mut urls = Vec::new();
        let manifest = load_manifest_with(
            "https://assets.mistermagik.com/mister-magik/v1/manifest.json",
            ManifestTrustMode::SignedHttps,
            |url, _, label| {
                urls.push(url.to_string());
                Ok(if label == "media manifest" {
                    manifest_bytes.clone()
                } else {
                    signature_bytes.clone()
                })
            },
            |actual_manifest, actual_signature| {
                assert_eq!(actual_manifest, manifest_bytes);
                assert_eq!(actual_signature, signature_bytes);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            urls,
            [
                "https://assets.mistermagik.com/mister-magik/v1/manifest.json",
                "https://assets.mistermagik.com/mister-magik/v1/manifest.json.sig"
            ]
        );
        assert_eq!(manifest.packs.len(), 1);
    }

    #[test]
    fn remote_download_script_bounds_http_and_decoded_streams() {
        let script = remote_script();

        assert!(script.contains("--proto '=http,https'"));
        assert!(script.contains("--connect-timeout 10 --max-time 1200"));
        assert!(script.contains("--max-filesize \"$download_limit\""));
        assert!(script.contains("mkfifo \"$body_pipe\""));
        assert!(script.contains("head -c \"$download_limit\" <&3 > \"$encoded\""));
        assert!(script.contains("head -c \"$expected_bytes\" <&3 > \"$decoded\""));
        assert!(script.contains("dd bs=1 count=1 <&3"));
        assert!(script.contains(
            "rm -f \"$encoded\" \"$decoded\" \"$headers\" \"$body_pipe\" \"$decode_pipe\" \"$final_tmp\""
        ));
    }

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
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "etag": "\"abc\"",
                "asset_count": 42
            }]
        });

        let manifest = parse_manifest(
            &value,
            "https://media.example.test/screenshots/v1/manifest.json",
        )
        .unwrap();

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
                        "sha256_decoded": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
                    }
                }
            }]
        });

        let manifest = parse_manifest(&value, "https://media.example.test/manifest.json").unwrap();

        assert_eq!(
            manifest.packs[0].local_path,
            "/media/fat/mister-magik/assets/nes-screenshots.mmlz4b"
        );
        assert_eq!(
            manifest.packs[0].identity.decoded_sha256,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn parses_magik_cloud_manifest_shape() {
        let sha = "c5fa5b54d2f2955e87e4d245a8942ce641a9cc84d320ce245b4a259a095c7b42";
        let idx_sha = "e5fa5b54d2f2955e87e4d245a8942ce641a9cc84d320ce245b4a259a095c7b42";
        let value = json!({
            "schema": 1,
            "generated_at": "2026-06-22T16:52:03Z",
            "packs": [{
                "id": "arcade",
                "size": "320x320",
                "version": "2026.06.22",
                "object": format!("mister-magik/v1/packs/arcade/2026.06.22/{sha}.mmlz4b"),
                "bytes": 1234,
                "sha256": sha,
                "codec": "mmlz4b",
                "index": {
                    "object": format!("mister-magik/v1/packs/arcade/2026.06.22/{idx_sha}.mmlz4b.idx"),
                    "bytes": 321,
                    "sha256": idx_sha.to_ascii_uppercase(),
                    "codec": "mmlz4b-index-v1",
                    "archive_bytes": 1234,
                    "archive_sha256": sha.to_ascii_uppercase()
                }
            }]
        });

        let manifest = parse_manifest(
            &value,
            "https://assets.mistermagik.com/mister-magik/v1/manifest.json",
        )
        .unwrap();

        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.published_at, "2026-06-22T16:52:03Z");
        assert_eq!(manifest.packs[0].system, "arcade");
        assert_eq!(manifest.packs[0].version, "2026.06.22");
        assert_eq!(manifest.packs[0].image_size, "320x320");
        assert_eq!(
            manifest.packs[0].local_path,
            DEFAULT_ARCADE_ARCHIVE_PATH.as_str()
        );
        assert_eq!(
            manifest_url_for_pack(&manifest, &manifest.packs[0]),
            format!(
                "http://assets.mistermagik.com/mister-magik/v1/packs/arcade/2026.06.22/{sha}.mmlz4b"
            )
        );
        let index = manifest.packs[0].index.as_ref().unwrap();
        assert_eq!(index.bytes, 321);
        assert_eq!(index.sha256, idx_sha);
        assert_eq!(
            local_index_path_for_pack(&manifest.packs[0]),
            format!("{}.idx", DEFAULT_ARCADE_ARCHIVE_PATH.as_str())
        );
        let expected_index_url = format!(
            "http://assets.mistermagik.com/mister-magik/v1/packs/arcade/2026.06.22/{idx_sha}.mmlz4b.idx"
        );
        assert_eq!(
            manifest_url_for_index(&manifest, &manifest.packs[0]).as_deref(),
            Some(expected_index_url.as_str())
        );
    }

    #[test]
    fn host_manifest_size_limits_accept_boundaries_and_reject_one_byte_over() {
        let sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut value = json!({
            "schema": 1,
            "generated_at": "2026-07-27T00:00:00Z",
            "packs": [{
                "id": "arcade",
                "object": format!("mister-magik/v1/packs/arcade/screenshots/320x320/v1/{sha}.mmlz4b"),
                "bytes": mister_magik_media_contract::MAX_MEDIA_PACK_BYTES,
                "sha256": sha,
                "codec": "mmlz4b",
                "index": {
                    "object": format!("mister-magik/v1/packs/arcade/screenshots/320x320/v1/{sha}.mmlz4b.idx"),
                    "bytes": mister_magik_media_contract::MAX_MEDIA_INDEX_BYTES,
                    "sha256": sha,
                    "codec": "mmlz4b-index-v2",
                    "archive_bytes": mister_magik_media_contract::MAX_MEDIA_PACK_BYTES,
                    "archive_sha256": sha
                }
            }]
        });
        let url = "https://assets.mistermagik.com/mister-magik/v1/manifest.json";
        assert!(parse_manifest(&value, url).is_ok());

        value["packs"][0]["bytes"] = json!(mister_magik_media_contract::MAX_MEDIA_PACK_BYTES + 1);
        assert!(
            parse_manifest(&value, url)
                .unwrap_err()
                .to_string()
                .contains("exceeds")
        );

        value["packs"][0]["bytes"] = json!(mister_magik_media_contract::MAX_MEDIA_PACK_BYTES);
        value["packs"][0]["index"]["bytes"] =
            json!(mister_magik_media_contract::MAX_MEDIA_INDEX_BYTES + 1);
        assert!(
            parse_manifest(&value, url)
                .unwrap_err()
                .to_string()
                .contains("exceeds")
        );
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

    #[test]
    fn remote_benchmark_rows_reject_bad_shape_and_non_numeric_fields() {
        let wrong_prefix = "wrong\tlabel\tnes\tgzip\t10\t20\t1\t2\t3\t4\t10\t80.00\t16.00\tetag\tgzip\tHIT\tbench-ok";
        assert!(parse_remote_row(wrong_prefix).is_err());
        assert!(parse_remote_row("screenshot_download_bench_tsv\ttoo-short").is_err());
        assert!(parse_remote_row(
            "screenshot_download_bench_tsv\tlabel\tnes\tgzip\tbad\t20\t1\t2\t3\t4\t10\t80.00\t16.00\tetag\tgzip\tHIT\tbench-ok",
        )
        .is_err());
    }

    #[test]
    fn remote_benchmark_rows_and_tsv_output_sanitize_text_fields() {
        let row = RemoteBenchRow {
            label: "label".to_string(),
            system: "nes".to_string(),
            variant: "identity".to_string(),
            encoded_bytes: 10,
            decoded_bytes: 20,
            download_ms: 1,
            decompress_ms: 2,
            save_ms: 3,
            verify_ms: 4,
            total_ms: 10,
            wire_mbps: "80.00".to_string(),
            decoded_mbps: "16.00".to_string(),
            etag: "etag\twith\nspace".to_string(),
            content_encoding: "identity".to_string(),
            cf_cache_status: "HIT".to_string(),
            result: "bench-ok".to_string(),
        };

        let text = row.to_tsv();

        assert!(text.starts_with("screenshot_download_bench_tsv\tlabel\tnes\tidentity"));
        assert!(text.contains("etag with space"));
        assert_eq!(text.split('\t').count(), 17);
    }

    #[test]
    fn media_args_accept_only_manifest_and_system() {
        let args = vec![
            "--manifest-url".to_string(),
            "https://example.test/manifest.json".to_string(),
            "--system".to_string(),
            "nes".to_string(),
        ];

        let parsed = parse_media_args(&args).expect("parse media args");

        assert_eq!(parsed.manifest_url, "https://example.test/manifest.json");
        assert_eq!(parsed.system, "nes");
    }

    #[test]
    fn media_args_reject_missing_values_and_unknown_options() {
        assert!(parse_media_args(&["--system".to_string()]).is_err());
        assert!(parse_media_args(&["--variant".to_string(), "zip".to_string()]).is_err());
        assert!(parse_media_args(&["--variants".to_string(), "gzip,zip".to_string()]).is_err());
        assert!(parse_media_args(&["--label".to_string(), "bad label".to_string()]).is_err());
        assert!(parse_media_args(&["--surprise".to_string()]).is_err());
    }

    #[test]
    fn selected_packs_filters_systems_and_reports_missing_pack() {
        let value = json!({
            "schema_version": 1,
            "base_url": "https://media.example.test",
            "packs": [
                {
                    "system": "nes",
                    "remote_path": "packs/nes.mmlz4b",
                    "bytes": 10,
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                {
                    "system": "snes",
                    "remote_path": "packs/snes.mmlz4b",
                    "bytes": 20,
                    "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                }
            ]
        });
        let manifest = parse_manifest(&value, "https://media.example.test/manifest.json").unwrap();

        assert_eq!(selected_packs(&manifest, "all").unwrap().len(), 2);
        let selected = selected_packs(&manifest, "snes").unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].system, "snes");

        let err = selected_packs(&manifest, "arcade").expect_err("missing pack");
        assert!(
            err.to_string()
                .contains("manifest has no screenshot pack for system 'arcade'")
        );
    }

    #[test]
    fn manifest_url_helpers_cover_absolute_official_and_relative_objects() {
        let manifest = MediaManifest {
            schema_version: 1,
            published_at: String::new(),
            base_url: "https://assets.mistermagik.com".to_string(),
            packs: Vec::new(),
        };

        assert_eq!(
            manifest_object_base_url("https://example.test/path/manifest.json"),
            "https://example.test"
        );
        assert_eq!(manifest_object_base_url("not-a-url"), "");
        assert_eq!(
            manifest_url_for_object(&manifest, "mister-magik/v1/packs/nes/pack.mmlz4b"),
            "http://assets.mistermagik.com/mister-magik/v1/packs/nes/pack.mmlz4b"
        );
        assert_eq!(
            manifest_url_for_object(&manifest, "https://cdn.example.test/pack.mmlz4b"),
            "https://cdn.example.test/pack.mmlz4b"
        );
        assert_eq!(
            manifest_url_for_object(
                &MediaManifest {
                    base_url: "https://media.example.test/root/".to_string(),
                    ..manifest
                },
                "/packs/nes.mmlz4b",
            ),
            "https://media.example.test/root/packs/nes.mmlz4b"
        );
    }

    #[test]
    fn image_size_helpers_accept_aliases_and_reject_invalid_dimensions() {
        assert_eq!(
            image_size_from_pack(&json!({"preview_size": "640x480"})).as_deref(),
            Some("640x480")
        );
        assert_eq!(
            image_size_from_pack(&json!({"width": 320, "height": 240})).as_deref(),
            Some("320x240")
        );
        assert_eq!(image_size_from_pack(&json!({"size": "0x240"})), None);
        assert_eq!(image_size_from_pack(&json!({"width": 320})), None);
        assert!(!valid_image_size("320X240"));
        assert!(!valid_image_size("wide"));
    }

    #[test]
    fn index_parsing_rejects_codec_object_zero_and_archive_mismatches() {
        let identity = MediaVariant {
            remote_path: "packs/nes.mmlz4b".to_string(),
            decoded_bytes: 10,
            decoded_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            etag: None,
        };
        let mut index = json!({
            "object": "packs/nes.mmlz4b.idx",
            "bytes": 4,
            "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "codec": "mmlz4b-index-v2",
            "archive_bytes": 10,
            "archive_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        });

        assert_eq!(
            parse_index("nes", &identity, &index).unwrap().codec,
            "mmlz4b-index-v2"
        );

        index["codec"] = json!("zip-index");
        assert!(
            parse_index("nes", &identity, &index)
                .unwrap_err()
                .to_string()
                .contains("unsupported index codec")
        );
        index["codec"] = json!("mmlz4b-index-v1");

        index["bytes"] = json!(0);
        assert!(
            parse_index("nes", &identity, &index)
                .unwrap_err()
                .to_string()
                .contains("zero bytes")
        );
        index["bytes"] = json!(4);

        index["object"] = json!("packs/nes.txt");
        assert!(
            parse_index("nes", &identity, &index)
                .unwrap_err()
                .to_string()
                .contains("must end with .mmlz4b.idx")
        );
        index["object"] = json!("packs/nes.mmlz4b.idx");

        index["archive_sha256"] =
            json!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
        assert!(
            parse_index("nes", &identity, &index)
                .unwrap_err()
                .to_string()
                .contains("archive_sha256 mismatch")
        );
    }

    #[test]
    fn manifest_parsing_rejects_incomplete_or_unsupported_packs() {
        let missing_schema = json!({ "packs": [] });
        assert!(parse_manifest(&missing_schema, "https://example.test/manifest.json").is_err());

        let missing_packs = json!({ "schema_version": 1 });
        assert!(parse_manifest(&missing_packs, "https://example.test/manifest.json").is_err());

        let unsupported_codec = json!({
            "schema_version": 1,
            "packs": [{
                "system": "nes",
                "remote_path": "packs/nes.zip",
                "bytes": 10,
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "codec": "zip"
            }]
        });
        let err =
            parse_manifest(&unsupported_codec, "https://example.test/manifest.json").unwrap_err();
        assert!(err.to_string().contains("unsupported codec zip"));

        let index_mismatch = json!({
            "schema_version": 1,
            "packs": [{
                "system": "nes",
                "remote_path": "packs/nes.mmlz4b",
                "bytes": 10,
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "codec": "mmlz4b",
                "index": {
                    "object": "packs/nes.mmlz4b.idx",
                    "bytes": 4,
                    "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "codec": "mmlz4b-index-v1",
                    "archive_bytes": 11,
                    "archive_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }
            }]
        });
        let err =
            parse_manifest(&index_mismatch, "https://example.test/manifest.json").unwrap_err();
        assert!(err.to_string().contains("index archive_bytes mismatch"));
    }
}
