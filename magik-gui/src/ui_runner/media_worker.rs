use crate::artifact_publish::{
    prepare_artifact_publish, sync_path_with_fallback, timestamped_temp_path_for,
    ArtifactPublishLabels,
};
use crate::media_pack_save::publish_pack_file_with_progress;
use mister_magik_fb::media_update::{
    pack_status_from_state, parse_manifest_json, size_qualified_pack_path, state_path,
    valid_image_size, LocalPackStatus, MediaPack, MediaUpdatePolicy, MediaVariant,
    DEFAULT_ASSET_DIR, DEFAULT_IMAGE_SIZE, DEFAULT_MANIFEST_URL,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_CONCURRENT_MEDIA_DOWNLOADS: usize = 1;
const MAX_CONCURRENT_MEDIA_DOWNLOADS: usize = 3;
const MANIFEST_FETCH_ATTEMPTS: usize = 6;
const MANIFEST_FETCH_INITIAL_RETRY: Duration = Duration::from_secs(2);
const MANIFEST_FETCH_MAX_RETRY: Duration = Duration::from_secs(10);

pub(super) struct MediaWorkerHandle {
    command_tx: mpsc::Sender<MediaWorkerCommand>,
    message_rx: mpsc::Receiver<MediaWorkerMessage>,
}

impl MediaWorkerHandle {
    pub(super) fn ensure_system(&self, system_id: &str) {
        let _ = self.command_tx.send(MediaWorkerCommand::EnsureSystem {
            system_id: system_id.to_string(),
        });
    }

    pub(super) fn set_interaction_active(&self, active: bool, reason: &str) {
        let _ = self
            .command_tx
            .send(MediaWorkerCommand::SetInteractionActive {
                active,
                reason: reason.to_string(),
            });
    }

    pub(super) fn finish(&self) {
        let _ = self.command_tx.send(MediaWorkerCommand::Finish);
    }

    pub(super) fn try_recv(&self) -> Option<MediaWorkerMessage> {
        self.message_rx.try_recv().ok()
    }
}

pub(super) fn start_screenshot_media_worker() -> Option<MediaWorkerHandle> {
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
    let (message_tx, message_rx) = mpsc::channel();
    let (command_tx, command_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("screenshot-media".to_string())
        .spawn(move || run_screenshot_media_worker(config, command_rx, message_tx))
        .ok()?;
    Some(MediaWorkerHandle {
        command_tx,
        message_rx,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MediaWorkerCommand {
    EnsureSystem { system_id: String },
    SetInteractionActive { active: bool, reason: String },
    Finish,
}

fn run_screenshot_media_worker(
    config: MediaWorkerConfig,
    command_rx: mpsc::Receiver<MediaWorkerCommand>,
    tx: mpsc::Sender<MediaWorkerMessage>,
) {
    let _ = tx.send(MediaWorkerMessage::Timing {
        name: "screenshot_media_update_start".to_string(),
        detail: format!(
            "policy={} manifest_url={} image_size={} asset_dir={} max_concurrent={}",
            config.policy.label(),
            config.manifest_url,
            config.image_size,
            config.asset_dir.display(),
            config.max_concurrent_downloads
        ),
    });
    send_progress(
        &tx,
        MediaProgressEvent::new("all", &config.image_size, "identity", "manifest_fetch"),
    );
    let (manifest_text, manifest_metadata) = match fetch_manifest_text_with_retry(
        &config.manifest_url,
        MANIFEST_FETCH_ATTEMPTS,
        MANIFEST_FETCH_INITIAL_RETRY,
        &tx,
        fetch_manifest_text,
    ) {
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
    let packs_by_system = packs_by_system_for_size(&manifest.packs, &config.image_size);
    let mut counts = MediaCheckCounts::default();
    let mut queue = MediaRequestQueue::default();
    let mut active = Vec::<ActiveDownload>::new();
    let mut finish_requested = false;
    let mut interaction_active = false;
    let mut interaction_reason = "idle".to_string();
    let mut defer_logged = false;
    loop {
        if interaction_active {
            if !defer_logged && !queue.pending.is_empty() {
                defer_logged = true;
                let _ = tx.send(MediaWorkerMessage::Timing {
                    name: "screenshot_media_defer".to_string(),
                    detail: format!(
                        "reason={} pending={} active={}",
                        interaction_reason,
                        queue.pending.len(),
                        active.len()
                    ),
                });
            }
        } else {
            defer_logged = false;
            start_ready_downloads(
                &config,
                &packs_by_system,
                state.as_ref(),
                &mut queue,
                &mut active,
                &mut counts,
                &tx,
            );
        }
        poll_active_downloads(&mut active, &mut counts, &tx);
        if finish_requested && active.is_empty() && queue.pending.is_empty() {
            break;
        }
        match command_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(MediaWorkerCommand::EnsureSystem { system_id }) => {
                match queue.enqueue(&system_id, &packs_by_system) {
                    MediaEnqueueResult::Queued { pack_index } => {
                        let _ = tx.send(MediaWorkerMessage::Timing {
                            name: "screenshot_media_system_queued".to_string(),
                            detail: format!(
                                "system={system_id} pack_index={pack_index} requested={}",
                                queue.requested_count
                            ),
                        });
                    }
                    MediaEnqueueResult::Duplicate => {
                        let _ = tx.send(MediaWorkerMessage::Timing {
                            name: "screenshot_media_system_duplicate".to_string(),
                            detail: format!("system={system_id}"),
                        });
                    }
                    MediaEnqueueResult::Unsupported => {
                        let _ = tx.send(MediaWorkerMessage::Timing {
                            name: "screenshot_media_system_ignored".to_string(),
                            detail: format!("system={system_id} reason=no-pack"),
                        });
                    }
                }
            }
            Ok(MediaWorkerCommand::SetInteractionActive { active, reason }) => {
                if interaction_active != active || interaction_reason != reason {
                    interaction_active = active;
                    interaction_reason = reason;
                    defer_logged = false;
                    let _ = tx.send(MediaWorkerMessage::Timing {
                        name: "screenshot_media_interaction_state".to_string(),
                        detail: format!(
                            "active={} reason={}",
                            if interaction_active { 1 } else { 0 },
                            interaction_reason
                        ),
                    });
                }
            }
            Ok(MediaWorkerCommand::Finish) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                finish_requested = true;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
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

fn packs_by_system_for_size(packs: &[MediaPack], image_size: &str) -> BTreeMap<String, MediaPack> {
    packs
        .iter()
        .filter(|pack| pack.image_size == image_size)
        .map(|pack| (pack.id.clone(), pack.clone()))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QueuedPackRequest {
    system_id: String,
    pack_index: usize,
}

#[derive(Default)]
struct MediaRequestQueue {
    seen: BTreeSet<String>,
    pending: VecDeque<QueuedPackRequest>,
    requested_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaEnqueueResult {
    Queued { pack_index: usize },
    Duplicate,
    Unsupported,
}

impl MediaRequestQueue {
    fn enqueue(
        &mut self,
        system_id: &str,
        packs_by_system: &BTreeMap<String, MediaPack>,
    ) -> MediaEnqueueResult {
        if !packs_by_system.contains_key(system_id) {
            return MediaEnqueueResult::Unsupported;
        }
        if !self.seen.insert(system_id.to_string()) {
            return MediaEnqueueResult::Duplicate;
        }
        self.requested_count += 1;
        let pack_index = self.requested_count;
        self.pending.push_back(QueuedPackRequest {
            system_id: system_id.to_string(),
            pack_index,
        });
        MediaEnqueueResult::Queued { pack_index }
    }
}

fn dequeue_startable_requests(
    pending: &mut VecDeque<QueuedPackRequest>,
    active_count: usize,
    max_concurrent: usize,
) -> Vec<QueuedPackRequest> {
    let slots = max_concurrent.saturating_sub(active_count);
    let mut startable = Vec::new();
    for _ in 0..slots {
        let Some(request) = pending.pop_front() else {
            break;
        };
        startable.push(request);
    }
    startable
}

struct ActiveDownload {
    pack: MediaPack,
    local_path: PathBuf,
    pack_index: usize,
    pack_count: usize,
    rx: mpsc::Receiver<Result<(), String>>,
}

fn start_ready_downloads(
    config: &MediaWorkerConfig,
    packs_by_system: &BTreeMap<String, MediaPack>,
    state: Option<&Value>,
    queue: &mut MediaRequestQueue,
    active: &mut Vec<ActiveDownload>,
    counts: &mut MediaCheckCounts,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) {
    for request in dequeue_startable_requests(
        &mut queue.pending,
        active.len(),
        config.max_concurrent_downloads,
    ) {
        let Some(pack) = packs_by_system.get(&request.system_id).cloned() else {
            continue;
        };
        let pack_count = queue.requested_count;
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
                    tx,
                    MediaProgressEvent::for_pack(
                        &pack,
                        "identity",
                        "failed",
                        request.pack_index,
                        pack_count,
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
        cleanup_pack_publish_temps(&local_path);
        send_progress(
            tx,
            MediaProgressEvent::for_pack(
                &pack,
                "identity",
                "check",
                request.pack_index,
                pack_count,
            ),
        );
        let status = pack_status_from_state(&pack, &local_path, state);
        counts.checked += 1;
        match status.label() {
            "current" => counts.current += 1,
            "missing" => counts.missing += 1,
            "stale" => counts.stale += 1,
            _ => counts.failed += 1,
        }
        let detail = match &status {
            LocalPackStatus::Stale { reason } => {
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
                tx,
                MediaProgressEvent::for_pack(
                    &pack,
                    "identity",
                    "skipped-current",
                    request.pack_index,
                    pack_count,
                )
                .with_done_bytes(pack.raw.bytes),
            );
            continue;
        }
        if config.policy != MediaUpdatePolicy::Download {
            send_progress(
                tx,
                MediaProgressEvent::for_pack(
                    &pack,
                    "identity",
                    "check-only",
                    request.pack_index,
                    pack_count,
                )
                .with_done_bytes(pack.raw.bytes),
            );
            continue;
        }
        let (result_tx, result_rx) = mpsc::channel();
        let download_config = config.clone();
        let download_pack = pack.clone();
        let download_local_path = local_path.clone();
        let download_tx = tx.clone();
        std::thread::Builder::new()
            .name(format!("screenshot-media-{}", pack.id))
            .spawn(move || {
                let result = download_raw_pack(
                    &download_config,
                    &download_pack,
                    &download_local_path,
                    request.pack_index,
                    pack_count,
                    &download_tx,
                );
                let _ = result_tx.send(result);
            })
            .ok();
        active.push(ActiveDownload {
            pack,
            local_path,
            pack_index: request.pack_index,
            pack_count,
            rx: result_rx,
        });
    }
}

fn poll_active_downloads(
    active: &mut Vec<ActiveDownload>,
    counts: &mut MediaCheckCounts,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) {
    let mut idx = 0;
    while idx < active.len() {
        match active[idx].rx.try_recv() {
            Ok(Ok(())) => {
                let done = active.remove(idx);
                counts.downloaded += 1;
                send_progress(
                    tx,
                    MediaProgressEvent::for_pack(
                        &done.pack,
                        "identity",
                        "done",
                        done.pack_index,
                        done.pack_count,
                    )
                    .with_done_bytes(done.pack.raw.bytes),
                );
                let _ = tx.send(MediaWorkerMessage::PackStatus {
                    system: done.pack.id,
                    image_size: done.pack.image_size,
                    status: "downloaded".to_string(),
                    detail: format!("local_path={}", done.local_path.display()),
                });
            }
            Ok(Err(error)) => {
                let failed = active.remove(idx);
                counts.failed += 1;
                send_progress(
                    tx,
                    MediaProgressEvent::for_pack(
                        &failed.pack,
                        "identity",
                        "failed",
                        failed.pack_index,
                        failed.pack_count,
                    )
                    .with_detail(&error),
                );
                let _ = tx.send(MediaWorkerMessage::PackStatus {
                    system: failed.pack.id,
                    image_size: failed.pack.image_size,
                    status: "failed".to_string(),
                    detail: error,
                });
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                let failed = active.remove(idx);
                counts.failed += 1;
                let detail = "download worker disconnected".to_string();
                send_progress(
                    tx,
                    MediaProgressEvent::for_pack(
                        &failed.pack,
                        "identity",
                        "failed",
                        failed.pack_index,
                        failed.pack_count,
                    )
                    .with_detail(&detail),
                );
                let _ = tx.send(MediaWorkerMessage::PackStatus {
                    system: failed.pack.id,
                    image_size: failed.pack.image_size,
                    status: "failed".to_string(),
                    detail,
                });
            }
            Err(mpsc::TryRecvError::Empty) => {
                idx += 1;
            }
        }
    }
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
        publish_pack_file_with_progress(&encoded_tmp, local_path, |progress| {
            send_progress(
                tx,
                MediaProgressEvent::for_pack(
                    pack,
                    "identity",
                    progress.phase.label(),
                    pack_index,
                    pack_count,
                )
                .with_bytes(progress.bytes_done, progress.bytes_total),
            );
        })
        .map(|_| metadata)
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

fn cleanup_pack_publish_temps(local_path: &Path) {
    crate::media_pack_save::cleanup_pack_publish_temps(local_path);
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
    let publish = prepare_artifact_publish(
        path,
        timestamped_temp_path_for(path, "media-state", unix_ms_now()),
        ArtifactPublishLabels {
            destination: "state path",
            parent: "state parent",
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
    use std::io::Write;
    file.write_all(text.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|e| format!("write media state {}: {e}", publish.temp_path().display()))?;
    file.sync_all()
        .map_err(|e| format!("sync media state {}: {e}", publish.temp_path().display()))?;
    publish.install_temp(Some("media state"))?;
    sync_path_with_fallback(publish.parent());
    Ok(())
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

fn fetch_manifest_text_with_retry<F>(
    manifest_url: &str,
    attempts: usize,
    initial_retry: Duration,
    tx: &mpsc::Sender<MediaWorkerMessage>,
    mut fetch: F,
) -> Result<(String, HttpCacheMetadata), String>
where
    F: FnMut(&str) -> Result<(String, HttpCacheMetadata), String>,
{
    let attempts = attempts.max(1);
    let mut retry = initial_retry;
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        match fetch(manifest_url) {
            Ok(fetched) => return Ok(fetched),
            Err(error) => {
                last_error = error;
                if attempt == attempts {
                    break;
                }
                let _ = tx.send(MediaWorkerMessage::Timing {
                    name: "screenshot_media_manifest_retry".to_string(),
                    detail: format!(
                        "attempt={attempt} attempts={attempts} retry_ms={} error={last_error}",
                        retry.as_millis()
                    ),
                });
                if !retry.is_zero() {
                    std::thread::sleep(retry);
                }
                retry = (retry.saturating_mul(2)).min(MANIFEST_FETCH_MAX_RETRY);
            }
        }
    }
    Err(last_error)
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
    max_concurrent_downloads: usize,
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
            max_concurrent_downloads: media_download_concurrency_from_env(),
        })
    }
}

fn media_download_concurrency_from_env() -> usize {
    std::env::var("MISTER_MEDIA_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_CONCURRENT_MEDIA_DOWNLOADS)
        .clamp(1, MAX_CONCURRENT_MEDIA_DOWNLOADS)
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

    fn pack_with_id(id: &str) -> MediaPack {
        let mut pack = pack_fixture();
        pack.id = id.to_string();
        pack
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
    fn media_request_queue_dedupes_and_ignores_unsupported_systems() {
        let mut packs = BTreeMap::new();
        packs.insert("arcade".to_string(), pack_with_id("arcade"));
        packs.insert("neogeo".to_string(), pack_with_id("neogeo"));
        let mut queue = MediaRequestQueue::default();

        assert_eq!(
            queue.enqueue("arcade", &packs),
            MediaEnqueueResult::Queued { pack_index: 1 }
        );
        assert_eq!(
            queue.enqueue("neogeo", &packs),
            MediaEnqueueResult::Queued { pack_index: 2 }
        );
        assert_eq!(
            queue.enqueue("arcade", &packs),
            MediaEnqueueResult::Duplicate
        );
        assert_eq!(
            queue.enqueue("pcengine", &packs),
            MediaEnqueueResult::Unsupported
        );

        assert_eq!(queue.pending.len(), 2);
        assert_eq!(queue.requested_count, 2);
    }

    #[test]
    fn media_request_queue_starts_at_configured_download_limit() {
        let mut pending = VecDeque::new();
        for index in 1..=5 {
            pending.push_back(QueuedPackRequest {
                system_id: format!("system-{index}"),
                pack_index: index,
            });
        }

        let first_batch =
            dequeue_startable_requests(&mut pending, 0, DEFAULT_MAX_CONCURRENT_MEDIA_DOWNLOADS);
        assert_eq!(first_batch.len(), DEFAULT_MAX_CONCURRENT_MEDIA_DOWNLOADS);
        assert_eq!(pending.len(), 4);

        let no_slots = dequeue_startable_requests(
            &mut pending,
            DEFAULT_MAX_CONCURRENT_MEDIA_DOWNLOADS,
            DEFAULT_MAX_CONCURRENT_MEDIA_DOWNLOADS,
        );
        assert!(no_slots.is_empty());
        assert_eq!(pending.len(), 4);

        let two_slots = dequeue_startable_requests(&mut pending, 1, MAX_CONCURRENT_MEDIA_DOWNLOADS);
        assert_eq!(two_slots.len(), 2);
        assert_eq!(pending.len(), 1);
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
    fn manifest_fetch_retry_recovers_after_transient_failures() {
        let (tx, rx) = mpsc::channel();
        let mut calls = 0usize;

        let (body, metadata) =
            fetch_manifest_text_with_retry(DEFAULT_MANIFEST_URL, 3, Duration::ZERO, &tx, |_| {
                calls += 1;
                if calls < 3 {
                    Err(format!("network-not-ready-{calls}"))
                } else {
                    Ok((
                        "{\"schema\":1,\"generated_at\":\"now\",\"packs\":[]}".to_string(),
                        HttpCacheMetadata {
                            status: Some(200),
                            ..Default::default()
                        },
                    ))
                }
            })
            .expect("third attempt should succeed");

        assert_eq!(calls, 3);
        assert!(body.contains("\"schema\""));
        assert_eq!(metadata.status, Some(200));
        let retries: Vec<_> = rx
            .try_iter()
            .filter_map(|message| match message {
                MediaWorkerMessage::Timing { name, detail }
                    if name == "screenshot_media_manifest_retry" =>
                {
                    Some(detail)
                }
                _ => None,
            })
            .collect();
        assert_eq!(retries.len(), 2);
        assert!(retries[0].contains("attempt=1"));
        assert!(retries[1].contains("attempt=2"));
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
    fn progress_publish_replaces_only_after_validated_copy() {
        let dir = temp_dir("mister-magik-publish-pack");
        let encoded = dir.join("downloaded.mmlz4b");
        let final_path = dir.join("arcade-screenshots-320x320.mmlz4b");
        fs::write(&encoded, b"new").unwrap();
        fs::write(&final_path, b"old").unwrap();
        let mut save_events = Vec::new();

        publish_pack_file_with_progress(&encoded, &final_path, |event| {
            save_events.push(event);
        })
        .unwrap();

        assert_eq!(fs::read(&final_path).unwrap(), b"new");
        assert!(!dir.join("arcade-screenshots-320x320.mmlz4b.tmp").exists());
        assert!(save_events
            .iter()
            .any(|event| event.phase.label() == "save"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cleanup_pack_publish_temps_removes_only_matching_pack_temps() {
        let dir = temp_dir("mister-magik-clean-publish-temp");
        let final_path = dir.join("arcade-screenshots-320x320.mmlz4b");
        let matching = dir.join("arcade-screenshots-320x320.mmlz4b.tmp-1");
        let other_pack = dir.join("neogeo-screenshots-320x320.mmlz4b.tmp-1");
        let final_file = dir.join("arcade-screenshots-320x320.mmlz4b");
        fs::write(&matching, b"partial").unwrap();
        fs::write(&other_pack, b"partial").unwrap();
        fs::write(&final_file, b"current").unwrap();

        cleanup_pack_publish_temps(&final_path);

        assert!(!matching.exists());
        assert!(other_pack.exists());
        assert_eq!(fs::read(&final_file).unwrap(), b"current");
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
