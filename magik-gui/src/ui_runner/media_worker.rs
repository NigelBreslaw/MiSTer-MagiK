use mister_magik_fb::media_update::{
    pack_status_from_state, parse_manifest_json, size_qualified_pack_path, state_path,
    valid_image_size, LocalPackStatus, MediaPack, MediaUpdatePolicy, MediaVariant,
    DEFAULT_ASSET_DIR, DEFAULT_IMAGE_SIZE, DEFAULT_MANIFEST_URL,
};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(super) fn start_screenshot_media_worker() -> Option<mpsc::Receiver<MediaWorkerMessage>> {
    let config = match MediaWorkerConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("screenshot media worker disabled: {error}");
            return None;
        }
    };
    if config.policy == MediaUpdatePolicy::Off {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("screenshot-media".to_string())
        .spawn(move || run_screenshot_media_worker(config, tx))
        .ok()?;
    Some(rx)
}

fn run_screenshot_media_worker(config: MediaWorkerConfig, tx: mpsc::Sender<MediaWorkerMessage>) {
    let _ = tx.send(MediaWorkerMessage::Timing {
        name: "screenshot_media_update_start".to_string(),
        detail: format!(
            "policy={} manifest_url={} image_size={} asset_dir={}",
            config.policy.label(),
            config.manifest_url,
            config.image_size,
            config.asset_dir.display()
        ),
    });
    send_progress(
        &tx,
        MediaProgressEvent::new("all", &config.image_size, "identity", "manifest_fetch"),
    );
    let (manifest_text, manifest_metadata) = match fetch_manifest_text(&config.manifest_url) {
        Ok(fetched) => fetched,
        Err(error) => {
            let _ = tx.send(MediaWorkerMessage::Failed {
                detail: format!("manifest fetch failed: {error}"),
            });
            return;
        }
    };
    let _ = tx.send(MediaWorkerMessage::CacheMetadata {
        scope: "manifest".to_string(),
        metadata: manifest_metadata,
    });
    let manifest = match parse_manifest_json(&config.manifest_url, &manifest_text) {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = tx.send(MediaWorkerMessage::Failed {
                detail: format!("manifest parse failed: {error}"),
            });
            return;
        }
    };
    let state = read_media_state(&config.asset_dir);
    let mut counts = MediaCheckCounts::default();
    let selected_packs: Vec<_> = manifest
        .packs
        .iter()
        .filter(|pack| pack.image_size == config.image_size)
        .collect();
    let pack_count = selected_packs.len();
    for (idx, pack) in selected_packs.into_iter().enumerate() {
        let pack_index = idx + 1;
        let local_path = match size_qualified_pack_path(
            &config.asset_dir.display().to_string(),
            &pack.id,
            &pack.image_size,
        ) {
            Ok(path) => PathBuf::from(path),
            Err(error) => {
                counts.checked += 1;
                counts.failed += 1;
                send_progress(
                    &tx,
                    MediaProgressEvent::for_pack(
                        pack, "identity", "failed", pack_index, pack_count,
                    )
                    .with_detail(&error),
                );
                let _ = tx.send(MediaWorkerMessage::PackStatus {
                    system: pack.id.clone(),
                    image_size: pack.image_size.clone(),
                    status: "failed".to_string(),
                    detail: error,
                });
                continue;
            }
        };
        send_progress(
            &tx,
            MediaProgressEvent::for_pack(pack, "identity", "check", pack_index, pack_count),
        );
        let status = pack_status_from_state(pack, &local_path, state.as_ref());
        counts.checked += 1;
        match status.label() {
            "current" => counts.current += 1,
            "missing" => counts.missing += 1,
            "stale" => counts.stale += 1,
            _ => counts.failed += 1,
        }
        let detail = match &status {
            mister_magik_fb::media_update::LocalPackStatus::Stale { reason } => {
                format!("local_path={} reason={reason}", local_path.display())
            }
            _ => format!("local_path={}", local_path.display()),
        };
        let _ = tx.send(MediaWorkerMessage::PackStatus {
            system: pack.id.clone(),
            image_size: pack.image_size.clone(),
            status: status.label().to_string(),
            detail,
        });
        if matches!(status, LocalPackStatus::Current) {
            send_progress(
                &tx,
                MediaProgressEvent::for_pack(
                    pack,
                    "identity",
                    "skipped-current",
                    pack_index,
                    pack_count,
                )
                .with_done_bytes(pack.raw.bytes),
            );
        }
        if config.policy == MediaUpdatePolicy::Download
            && !matches!(status, LocalPackStatus::Current)
        {
            match download_raw_pack(&config, pack, &local_path, pack_index, pack_count, &tx) {
                Ok(()) => {
                    counts.downloaded += 1;
                    send_progress(
                        &tx,
                        MediaProgressEvent::for_pack(
                            pack, "identity", "done", pack_index, pack_count,
                        )
                        .with_done_bytes(pack.raw.bytes),
                    );
                    let _ = tx.send(MediaWorkerMessage::PackStatus {
                        system: pack.id.clone(),
                        image_size: pack.image_size.clone(),
                        status: "downloaded".to_string(),
                        detail: format!("local_path={}", local_path.display()),
                    });
                }
                Err(error) => {
                    counts.failed += 1;
                    send_progress(
                        &tx,
                        MediaProgressEvent::for_pack(
                            pack, "identity", "failed", pack_index, pack_count,
                        )
                        .with_detail(&error),
                    );
                    let _ = tx.send(MediaWorkerMessage::PackStatus {
                        system: pack.id.clone(),
                        image_size: pack.image_size.clone(),
                        status: "failed".to_string(),
                        detail: error,
                    });
                }
            }
        }
    }
    let _ = tx.send(MediaWorkerMessage::Done {
        detail: format!(
            "packs={} current={} missing={} stale={} downloaded={} failed={}",
            counts.total(),
            counts.current,
            counts.missing,
            counts.stale,
            counts.downloaded,
            counts.failed
        ),
    });
}

fn download_raw_pack(
    config: &MediaWorkerConfig,
    pack: &MediaPack,
    local_path: &Path,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) -> Result<(), String> {
    let variant = pack
        .variant_for_compression("none")
        .ok_or_else(|| format!("pack {} has no compression=none variant", pack.id))?;
    fs::create_dir_all(&config.asset_dir)
        .map_err(|e| format!("create asset dir {}: {e}", config.asset_dir.display()))?;
    let work_dir = PathBuf::from("/tmp/mister-magik-media-download");
    fs::create_dir_all(&work_dir)
        .map_err(|e| format!("create media work dir {}: {e}", work_dir.display()))?;
    let encoded_tmp = work_dir.join(format!(
        "{}-{}-{}.mmlz4b.download",
        pack.id,
        pack.image_size,
        unix_ms_now()
    ));
    let headers_tmp = work_dir.join(format!(
        "{}-{}-{}.headers",
        pack.id,
        pack.image_size,
        unix_ms_now()
    ));
    let result = download_variant_to_path(
        variant,
        pack,
        &encoded_tmp,
        &headers_tmp,
        pack_index,
        pack_count,
        tx,
    )
    .and_then(|metadata| {
        let _ = tx.send(MediaWorkerMessage::CacheMetadata {
            scope: format!("pack:{}", pack.id),
            metadata: metadata.clone(),
        });
        send_progress(
            tx,
            MediaProgressEvent::for_pack(pack, "identity", "verify", pack_index, pack_count)
                .with_done_bytes(variant.bytes),
        );
        verify_downloaded_file(&encoded_tmp, variant.bytes, &variant.sha256).map(|()| metadata)
    })
    .and_then(|metadata| {
        send_progress(
            tx,
            MediaProgressEvent::for_pack(pack, "identity", "save", pack_index, pack_count)
                .with_done_bytes(variant.bytes),
        );
        publish_pack_file(&encoded_tmp, local_path).map(|()| metadata)
    })
    .and_then(|metadata| {
        write_download_state(
            &config.asset_dir,
            pack,
            local_path,
            variant,
            Some(&metadata),
        )
    });
    let _ = fs::remove_file(encoded_tmp);
    let _ = fs::remove_file(headers_tmp);
    result
}

fn download_variant_to_path(
    variant: &MediaVariant,
    pack: &MediaPack,
    output_path: &Path,
    headers_path: &Path,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) -> Result<HttpCacheMetadata, String> {
    send_progress(
        tx,
        MediaProgressEvent::for_pack(pack, "identity", "download_start", pack_index, pack_count),
    );
    let headers = File::create(headers_path)
        .map_err(|e| format!("create headers file {}: {e}", headers_path.display()))?;
    let started = Instant::now();
    let mut child = Command::new("wget")
        .arg("-S")
        .arg("--header")
        .arg("Accept-Encoding: identity")
        .arg("-O")
        .arg(output_path)
        .arg(&variant.url)
        .stdout(Stdio::null())
        .stderr(Stdio::from(headers))
        .spawn()
        .map_err(|e| format!("spawn wget: {e}"))?;
    let mut last_bytes = 0;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    loop {
        let bytes = output_path.metadata().map(|meta| meta.len()).unwrap_or(0);
        if bytes != last_bytes || last_emit.elapsed() >= Duration::from_millis(500) {
            last_bytes = bytes;
            last_emit = Instant::now();
            send_progress(
                tx,
                MediaProgressEvent::for_pack(pack, "identity", "download", pack_index, pack_count)
                    .with_bytes(bytes, variant.bytes)
                    .with_download_mbps(mbps(bytes, started.elapsed())),
            );
        }
        match child.try_wait().map_err(|e| format!("wait wget: {e}"))? {
            Some(status) if status.success() => {
                let bytes = output_path.metadata().map(|meta| meta.len()).unwrap_or(0);
                let header_text = fs::read_to_string(headers_path).unwrap_or_default();
                send_progress(
                    tx,
                    MediaProgressEvent::for_pack(
                        pack,
                        "identity",
                        "download_done",
                        pack_index,
                        pack_count,
                    )
                    .with_bytes(bytes, variant.bytes)
                    .with_download_mbps(mbps(bytes, started.elapsed())),
                );
                return Ok(parse_wget_headers(&header_text, &variant.url, "response"));
            }
            Some(status) => return Err(format!("wget exited with {status}")),
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn mbps(bytes: u64, elapsed: Duration) -> f64 {
    let ms = elapsed.as_millis() as f64;
    if ms <= 0.0 {
        0.0
    } else {
        (bytes as f64 * 8.0) / (ms * 1000.0)
    }
}

fn verify_downloaded_file(
    path: &Path,
    expected_bytes: u64,
    expected_sha: &str,
) -> Result<(), String> {
    let bytes = path
        .metadata()
        .map_err(|e| format!("stat downloaded file {}: {e}", path.display()))?
        .len();
    if bytes != expected_bytes {
        return Err(format!(
            "size mismatch expected={expected_bytes} actual={bytes}"
        ));
    }
    let sha = sha256_hex(path)?;
    if sha != expected_sha {
        return Err(format!(
            "sha256 mismatch expected={expected_sha} actual={sha}"
        ));
    }
    Ok(())
}

fn sha256_hex(path: &Path) -> Result<String, String> {
    let output = Command::new("sha256sum").arg(path).output().or_else(|_| {
        Command::new("shasum")
            .arg("-a")
            .arg("256")
            .arg(path)
            .output()
    });
    let output = output.map_err(|e| format!("spawn sha256 command: {e}"))?;
    if !output.status.success() {
        return Err(format!("sha256 command exited with {}", output.status));
    }
    let text = String::from_utf8(output.stdout).map_err(|e| format!("sha256 output utf8: {e}"))?;
    text.split_whitespace()
        .next()
        .filter(|sha| sha.len() == 64)
        .map(str::to_string)
        .ok_or_else(|| format!("could not parse sha256 output: {text}"))
}

fn publish_pack_file(encoded_path: &Path, local_path: &Path) -> Result<(), String> {
    let parent = local_path
        .parent()
        .ok_or_else(|| format!("pack path has no parent: {}", local_path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("create pack parent {}: {e}", parent.display()))?;
    let final_tmp = local_path.with_file_name(format!(
        "{}.tmp-{}",
        local_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("screenshot-pack"),
        unix_ms_now()
    ));
    copy_file_durable(encoded_path, &final_tmp)?;
    fs::rename(&final_tmp, local_path).map_err(|e| {
        let _ = fs::remove_file(&final_tmp);
        format!(
            "rename pack {} -> {}: {e}",
            final_tmp.display(),
            local_path.display()
        )
    })?;
    sync_path(parent);
    Ok(())
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

fn write_download_state(
    asset_dir: &Path,
    pack: &MediaPack,
    local_path: &Path,
    variant: &MediaVariant,
    metadata: Option<&HttpCacheMetadata>,
) -> Result<(), String> {
    let path = PathBuf::from(state_path(&asset_dir.display().to_string()));
    let mut root = fs::read_to_string(&path)
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
    let mut pack_state = serde_json::json!({
        "version": pack.version,
        "image_size": pack.image_size,
        "sha256": pack.raw.sha256,
        "bytes": pack.raw.bytes,
        "variant": "identity",
        "compression": variant.compression,
        "local_path": local_path.display().to_string(),
        "object": variant.object,
        "updated_at_unix": unix_secs_now(),
    });
    if let Some(metadata) = metadata {
        pack_state["cache"] = cache_metadata_json(metadata);
    }
    packs.insert(pack.image_size.clone(), pack_state);
    root.insert("schema".to_string(), Value::from(1));
    root.insert("updated_at_unix".to_string(), Value::from(unix_secs_now()));
    write_json_atomic(&path, &Value::Object(root))
}

fn cache_metadata_json(metadata: &HttpCacheMetadata) -> Value {
    serde_json::json!({
        "status": metadata.status,
        "etag": metadata.etag,
        "last_modified": metadata.last_modified,
        "cache_control": metadata.cache_control,
        "age": metadata.age,
        "cf_cache_status": metadata.cf_cache_status,
        "cf_ray": metadata.cf_ray,
        "content_length": metadata.content_length,
        "content_encoding": metadata.content_encoding,
        "effective_url": metadata.effective_url,
        "source": metadata.source,
    })
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("state path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("create state parent {}: {e}", parent.display()))?;
    let tmp = path.with_file_name(format!(
        "{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("media-state"),
        unix_ms_now()
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .map_err(|e| format!("create state tmp {}: {e}", tmp.display()))?;
    let text =
        serde_json::to_string_pretty(value).map_err(|e| format!("serialize media state: {e}"))?;
    use std::io::Write;
    file.write_all(text.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|e| format!("write media state {}: {e}", tmp.display()))?;
    file.sync_all()
        .map_err(|e| format!("sync media state {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!(
            "rename media state {} -> {}: {e}",
            tmp.display(),
            path.display()
        )
    })?;
    sync_path(parent);
    Ok(())
}

fn sync_path(path: &Path) {
    match Command::new("sync").arg(path).status() {
        Ok(status) if status.success() => {}
        _ => {
            let _ = Command::new("sync").status();
        }
    };
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

fn fetch_manifest_text(manifest_url: &str) -> Result<(String, HttpCacheMetadata), String> {
    let headers_path = PathBuf::from(format!(
        "/tmp/mister-magik-media-manifest-{}.headers",
        unix_ms_now()
    ));
    let headers = File::create(&headers_path)
        .map_err(|e| format!("create manifest headers {}: {e}", headers_path.display()))?;
    let output = Command::new("wget")
        .arg("-S")
        .arg("--header")
        .arg("Accept-Encoding: identity")
        .arg("-O")
        .arg("-")
        .arg(manifest_url)
        .stderr(Stdio::from(headers))
        .output()
        .map_err(|e| format!("spawn wget: {e}"))?;
    let header_text = fs::read_to_string(&headers_path).unwrap_or_default();
    let _ = fs::remove_file(headers_path);
    if !output.status.success() {
        return Err(format!("wget exited with {}", output.status));
    }
    let body = String::from_utf8(output.stdout).map_err(|e| format!("manifest utf8: {e}"))?;
    Ok((
        body,
        parse_wget_headers(&header_text, manifest_url, "response"),
    ))
}

fn read_media_state(asset_dir: &Path) -> Option<Value> {
    let text = fs::read_to_string(state_path(&asset_dir.display().to_string())).ok()?;
    serde_json::from_str(&text).ok()
}

#[derive(Clone, Debug)]
struct MediaWorkerConfig {
    policy: MediaUpdatePolicy,
    manifest_url: String,
    image_size: String,
    asset_dir: PathBuf,
}

impl MediaWorkerConfig {
    fn from_env() -> Result<Self, String> {
        let policy =
            MediaUpdatePolicy::parse(std::env::var("MISTER_MEDIA_UPDATE").ok().as_deref())?;
        let image_size =
            std::env::var("MISTER_MEDIA_SIZE").unwrap_or_else(|_| DEFAULT_IMAGE_SIZE.to_string());
        if !valid_image_size(&image_size) {
            return Err(format!("invalid MISTER_MEDIA_SIZE: {image_size}"));
        }
        Ok(Self {
            policy,
            manifest_url: std::env::var("MISTER_MEDIA_MANIFEST_URL")
                .unwrap_or_else(|_| DEFAULT_MANIFEST_URL.to_string()),
            image_size,
            asset_dir: PathBuf::from(
                std::env::var("MISTER_MEDIA_ASSET_DIR")
                    .unwrap_or_else(|_| DEFAULT_ASSET_DIR.to_string()),
            ),
        })
    }
}

#[derive(Default)]
struct MediaCheckCounts {
    checked: usize,
    current: usize,
    missing: usize,
    stale: usize,
    downloaded: usize,
    failed: usize,
}

impl MediaCheckCounts {
    fn total(&self) -> usize {
        self.checked
    }
}

#[derive(Clone, Debug)]
pub(super) enum MediaWorkerMessage {
    Timing {
        name: String,
        detail: String,
    },
    Progress(MediaProgressEvent),
    CacheMetadata {
        scope: String,
        metadata: HttpCacheMetadata,
    },
    PackStatus {
        system: String,
        image_size: String,
        status: String,
        detail: String,
    },
    Failed {
        detail: String,
    },
    Done {
        detail: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct MediaProgressEvent {
    pub system: String,
    pub image_size: String,
    pub variant: String,
    pub phase: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub pack_index: usize,
    pub pack_count: usize,
    pub download_mbps: Option<f64>,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct HttpCacheMetadata {
    pub status: Option<u16>,
    pub etag: String,
    pub last_modified: String,
    pub cache_control: String,
    pub age: String,
    pub cf_cache_status: String,
    pub cf_ray: String,
    pub content_length: String,
    pub content_encoding: String,
    pub effective_url: String,
    pub source: String,
}

impl HttpCacheMetadata {
    pub(super) fn log_detail(&self, scope: &str) -> String {
        format!(
            "scope={} source={} status={} etag={} last_modified={} cache_control={} age={} cf_cache_status={} cf_ray={} content_length={} content_encoding={} effective_url={}",
            scope,
            self.source,
            self.status.map(|value| value.to_string()).unwrap_or_default(),
            self.etag,
            self.last_modified,
            self.cache_control,
            self.age,
            self.cf_cache_status,
            self.cf_ray,
            self.content_length,
            self.content_encoding,
            self.effective_url
        )
    }
}

fn parse_wget_headers(text: &str, effective_url: &str, source: &str) -> HttpCacheMetadata {
    let mut metadata = HttpCacheMetadata {
        effective_url: effective_url.to_string(),
        source: source.to_string(),
        ..Default::default()
    };
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("HTTP/") {
            metadata.status = rest
                .split_whitespace()
                .nth(1)
                .or_else(|| rest.split_whitespace().next())
                .and_then(|value| value.parse::<u16>().ok());
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_end_matches('\r').to_string();
        match name.trim().to_ascii_lowercase().as_str() {
            "etag" => metadata.etag = value,
            "last-modified" => metadata.last_modified = value,
            "cache-control" => metadata.cache_control = value,
            "age" => metadata.age = value,
            "cf-cache-status" => metadata.cf_cache_status = value,
            "cf-ray" => metadata.cf_ray = value,
            "content-length" => metadata.content_length = value,
            "content-encoding" => metadata.content_encoding = value,
            "location" => metadata.effective_url = value,
            _ => {}
        }
    }
    metadata
}

impl MediaProgressEvent {
    fn new(system: &str, image_size: &str, variant: &str, phase: &str) -> Self {
        Self {
            system: system.to_string(),
            image_size: image_size.to_string(),
            variant: variant.to_string(),
            phase: phase.to_string(),
            bytes_done: 0,
            bytes_total: 0,
            pack_index: 0,
            pack_count: 0,
            download_mbps: None,
            detail: String::new(),
        }
    }

    fn for_pack(
        pack: &MediaPack,
        variant: &str,
        phase: &str,
        pack_index: usize,
        pack_count: usize,
    ) -> Self {
        Self::new(&pack.id, &pack.image_size, variant, phase)
            .with_bytes(0, pack.raw.bytes)
            .with_pack_position(pack_index, pack_count)
    }

    fn with_bytes(mut self, done: u64, total: u64) -> Self {
        self.bytes_done = done;
        self.bytes_total = total;
        self
    }

    fn with_pack_position(mut self, pack_index: usize, pack_count: usize) -> Self {
        self.pack_index = pack_index;
        self.pack_count = pack_count;
        self
    }

    fn with_done_bytes(mut self, total: u64) -> Self {
        self.bytes_done = total;
        self.bytes_total = total;
        self
    }

    fn with_download_mbps(mut self, mbps: f64) -> Self {
        self.download_mbps = Some(mbps);
        self
    }

    fn with_detail(mut self, detail: &str) -> Self {
        self.detail = detail.to_string();
        self
    }

    fn percent(&self) -> i32 {
        self.bytes_done
            .min(self.bytes_total)
            .saturating_mul(100)
            .checked_div(self.bytes_total)
            .map(|value| value as i32)
            .unwrap_or(-1)
    }

    pub(super) fn log_detail(&self) -> String {
        let mbps = self
            .download_mbps
            .map(|value| format!("{value:.2}"))
            .unwrap_or_default();
        format!(
            "system={} image_size={} variant={} phase={} bytes_done={} bytes_total={} percent={} pack_index={} pack_count={} download_mbps={} detail={}",
            self.system,
            self.image_size,
            self.variant,
            self.phase,
            self.bytes_done,
            self.bytes_total,
            self.percent(),
            self.pack_index,
            self.pack_count,
            mbps,
            self.detail
        )
    }
}

fn send_progress(tx: &mpsc::Sender<MediaWorkerMessage>, event: MediaProgressEvent) {
    let _ = tx.send(MediaWorkerMessage::Progress(event));
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_fb::media_update::parse_manifest_json;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn pack_fixture() -> MediaPack {
        let text = format!(
            r#"{{
  "schema": 1,
  "generated_at": "2026-06-22T16:58:28Z",
  "packs": [
    {{
      "id": "arcade",
      "version": "2026.06.22",
      "object": "mister-magik/v1/packs/arcade/2026.06.22/{SHA}.mmlz4b",
      "bytes": 4,
      "sha256": "{SHA}",
      "codec": "mmlz4b"
    }}
  ]
}}"#
        );
        parse_manifest_json(DEFAULT_MANIFEST_URL, &text)
            .unwrap()
            .packs
            .remove(0)
    }

    #[test]
    fn media_worker_policy_defaults_to_download() {
        assert_eq!(
            MediaUpdatePolicy::parse(None).unwrap(),
            MediaUpdatePolicy::Download
        );
        assert_eq!(
            MediaUpdatePolicy::parse(Some("check-only")).unwrap(),
            MediaUpdatePolicy::Check
        );
        assert_eq!(
            MediaUpdatePolicy::parse(Some("off")).unwrap(),
            MediaUpdatePolicy::Off
        );
        assert!(MediaUpdatePolicy::parse(Some("maybe")).is_err());
    }

    #[test]
    fn progress_event_calculates_percent_and_log_detail() {
        let event = MediaProgressEvent::new("arcade", "320x320", "identity", "download")
            .with_bytes(25, 100)
            .with_pack_position(1, 3)
            .with_download_mbps(12.345)
            .with_detail("sample");

        assert_eq!(event.percent(), 25);
        let detail = event.log_detail();
        assert!(detail.contains("system=arcade"));
        assert!(detail.contains("percent=25"));
        assert!(detail.contains("download_mbps=12.35"));
        assert!(detail.contains("detail=sample"));
    }

    #[test]
    fn progress_event_reports_indeterminate_without_total() {
        let event = MediaProgressEvent::new("all", "320x320", "identity", "manifest");

        assert_eq!(event.percent(), -1);
    }

    #[test]
    fn mbps_uses_wire_megabits_per_second() {
        assert_eq!(mbps(1_000_000, Duration::from_millis(1000)), 8.0);
    }

    #[test]
    fn parses_cloudflare_cache_headers_from_wget_output() {
        let headers = "\
  HTTP/1.1 200 OK\r
  Cache-Control: public, max-age=31536000, immutable\r
  ETag: \"abc\"\r
  Last-Modified: Tue, 23 Jun 2026 08:00:00 GMT\r
  Age: 42\r
  CF-Cache-Status: HIT\r
  CF-Ray: test-ray\r
  Content-Length: 123\r
  Content-Encoding: identity\r
";

        let metadata = parse_wget_headers(headers, "https://assets.example/pack", "response");

        assert_eq!(metadata.status, Some(200));
        assert_eq!(metadata.cf_cache_status, "HIT");
        assert_eq!(metadata.age, "42");
        assert_eq!(metadata.etag, "\"abc\"");
        assert_eq!(metadata.content_length, "123");
    }

    #[test]
    fn verify_downloaded_file_rejects_bad_checksum() {
        let dir = temp_dir("mister-magik-verify-bad-sha");
        let path = dir.join("pack.mmlz4b");
        fs::write(&path, b"pack").unwrap();

        let err = verify_downloaded_file(&path, 4, SHA).unwrap_err();

        assert!(err.contains("sha256 mismatch"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn publish_pack_file_replaces_only_after_validated_copy() {
        let dir = temp_dir("mister-magik-publish-pack");
        let encoded = dir.join("downloaded.mmlz4b");
        let final_path = dir.join("arcade-screenshots-320x320.mmlz4b");
        fs::write(&encoded, b"new").unwrap();
        fs::write(&final_path, b"old").unwrap();

        publish_pack_file(&encoded, &final_path).unwrap();

        assert_eq!(fs::read(&final_path).unwrap(), b"new");
        assert!(!dir.join("arcade-screenshots-320x320.mmlz4b.tmp").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_verification_leaves_existing_pack_untouched() {
        let dir = temp_dir("mister-magik-bad-download");
        let encoded = dir.join("downloaded.mmlz4b");
        let final_path = dir.join("arcade-screenshots-320x320.mmlz4b");
        fs::write(&encoded, b"bad").unwrap();
        fs::write(&final_path, b"old").unwrap();

        assert!(verify_downloaded_file(&encoded, 3, SHA).is_err());

        assert_eq!(fs::read(&final_path).unwrap(), b"old");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn write_download_state_records_size_qualified_pack() {
        let dir = temp_dir("mister-magik-write-state");
        let pack = pack_fixture();
        let local_path = dir.join("arcade-screenshots-320x320.mmlz4b");
        let variant = pack.variant_for_compression("none").unwrap();
        let metadata = HttpCacheMetadata {
            status: Some(200),
            cf_cache_status: "MISS".to_string(),
            content_length: "4".to_string(),
            effective_url: variant.url.clone(),
            source: "response".to_string(),
            ..Default::default()
        };

        write_download_state(&dir, &pack, &local_path, variant, Some(&metadata)).unwrap();

        let state: Value = serde_json::from_str(
            &fs::read_to_string(dir.join(".screenshot-media-state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            state["systems"]["arcade"]["packs"]["320x320"]["local_path"],
            local_path.display().to_string()
        );
        assert_eq!(
            state["systems"]["arcade"]["packs"]["320x320"]["variant"],
            "identity"
        );
        assert_eq!(
            state["systems"]["arcade"]["packs"]["320x320"]["cache"]["cf_cache_status"],
            "MISS"
        );
        assert_eq!(
            state["systems"]["arcade"]["packs"]["320x320"]["cache"]["content_length"],
            "4"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
