// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::artifact_publish::{
    ArtifactPublishLabels, hidden_timestamped_temp_path_for, prepare_artifact_publish,
    sync_path_rust_best_effort, timestamped_temp_path_for,
};
use crate::media_update::{
    DEFAULT_ASSET_DIR, DEFAULT_IMAGE_SIZE, DEFAULT_MANIFEST_URL, LocalPackStatus, MediaIndex,
    MediaPack, MediaUpdatePolicy, MediaVariant, index_path_for_pack_path, pack_status_from_state,
    parse_manifest_json, size_qualified_pack_path, state_path, valid_image_size,
};
use mister_magik_catalog::media_identity::preferred_screenshot_image_size;
use mister_magik_catalog::preview_worker::invalidate_preview_archive_metadata_cache;
use mister_magik_catalog::production_sharded_projection::{
    PreviewAvailabilityReconciliationOutcome, production_registry_limits,
    reconcile_production_preview_availability,
};
use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use mister_magik_media_contract::{MEDIA_CONNECT_TIMEOUT_SECS, MEDIA_TRANSFER_TIMEOUT_SECS};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_CONCURRENT_MEDIA_DOWNLOADS: usize = 1;
#[cfg(test)]
const MAX_CONCURRENT_MEDIA_DOWNLOADS: usize = 1;
const MANIFEST_FETCH_ATTEMPTS: usize = 30;
const MANIFEST_FETCH_FAST_RETRY: Duration = Duration::from_secs(1);
const MANIFEST_FETCH_FAST_RETRY_WINDOW: Duration = Duration::from_secs(20);
const MANIFEST_FETCH_INITIAL_BACKOFF: Duration = Duration::from_secs(2);
const MANIFEST_FETCH_MAX_RETRY: Duration = Duration::from_secs(10);
const MEDIA_DOWNLOAD_WORK_DIR: &str = "/tmp/mister-magik-media-download";

pub struct MediaWorkerHandle {
    command_tx: mpsc::Sender<MediaWorkerCommand>,
    message_rx: mpsc::Receiver<MediaWorkerMessage>,
}

impl MediaWorkerHandle {
    pub fn ensure_system(&self, system_id: &str) {
        let _ = self.command_tx.send(MediaWorkerCommand::EnsureSystem {
            system_id: system_id.to_string(),
        });
    }

    pub fn set_interaction_active(&self, active: bool, reason: &str) {
        let _ = self
            .command_tx
            .send(MediaWorkerCommand::SetInteractionActive {
                active,
                reason: reason.to_string(),
            });
    }

    pub fn finish(&self) {
        let _ = self.command_tx.send(MediaWorkerCommand::Finish);
    }

    pub fn try_recv(&self) -> Option<MediaWorkerMessage> {
        self.message_rx.try_recv().ok()
    }
}

pub fn start_screenshot_media_worker() -> Option<MediaWorkerHandle> {
    let config = match MediaWorkerConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            crate::ui_errln!("screenshot media worker disabled: {error}");
            return None;
        }
    };
    if config.policy == MediaUpdatePolicy::Off {
        return None;
    }
    start_screenshot_media_worker_with_config(config)
}

pub fn start_screenshot_media_worker_with_config(
    config: MediaWorkerConfig,
) -> Option<MediaWorkerHandle> {
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
    apply_runtime_thread_policy(RuntimeThreadRole::MediaWorker);
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
        MANIFEST_FETCH_INITIAL_BACKOFF,
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
    let mut pending_reconciliation = BTreeMap::<String, (MediaPack, PathBuf)>::new();
    let mut finish_requested = false;
    let mut interaction_active = false;
    let mut interaction_reason = "idle".to_string();
    let mut defer_logged = false;
    let mut benchmark_quiescent_since = None;
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
                &mut pending_reconciliation,
                &mut counts,
                &tx,
            );
        }
        poll_active_downloads(&mut active, &mut pending_reconciliation, &mut counts, &tx);
        if config.benchmark_auto_finish
            && queue.requested_count > 0
            && active.is_empty()
            && queue.pending.is_empty()
        {
            let quiescent_since = benchmark_quiescent_since.get_or_insert_with(Instant::now);
            if quiescent_since.elapsed() >= Duration::from_millis(500) {
                let _ = tx.send(MediaWorkerMessage::Timing {
                    name: "screenshot_media_benchmark_auto_finish".to_string(),
                    detail: format!(
                        "requested={} quiescent_ms={}",
                        queue.requested_count,
                        quiescent_since.elapsed().as_millis()
                    ),
                });
                finish_requested = true;
            }
        } else {
            benchmark_quiescent_since = None;
        }
        if finish_requested && active.is_empty() && queue.pending.is_empty() {
            for (_system, (pack, local_path)) in std::mem::take(&mut pending_reconciliation) {
                reconcile_pack_preview_availability(&config.catalog_root, &pack, &local_path, &tx);
            }
            break;
        }
        match command_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(MediaWorkerCommand::EnsureSystem { system_id }) => {
                benchmark_quiescent_since = None;
                match queue.enqueue(&system_id, &packs_by_system) {
                    MediaEnqueueResult::Queued { pack_index } => {
                        let _ = tx.send(MediaWorkerMessage::Timing {
                            name: "screenshot_media_system_queued".to_string(),
                            detail: format!(
                                "system={system_id} pack_index={pack_index} requested={} pending={}",
                                queue.requested_count,
                                queue.pending.len()
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
        .filter(|pack| {
            let preferred = if image_size == DEFAULT_IMAGE_SIZE {
                preferred_screenshot_image_size(&pack.id)
            } else {
                image_size
            };
            pack.image_size == preferred
        })
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
    show_completion_progress: bool,
    rx: mpsc::Receiver<Result<(), String>>,
}

fn start_ready_downloads(
    config: &MediaWorkerConfig,
    packs_by_system: &BTreeMap<String, MediaPack>,
    state: Option<&Value>,
    queue: &mut MediaRequestQueue,
    active: &mut Vec<ActiveDownload>,
    pending_reconciliation: &mut BTreeMap<String, (MediaPack, PathBuf)>,
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
        let _ = tx.send(MediaWorkerMessage::Timing {
            name: "screenshot_media_system_start".to_string(),
            detail: format!(
                "system={} pack_index={} pack_count={} pending={} active={} max_concurrent={} policy={}",
                pack.id,
                request.pack_index,
                pack_count,
                queue.pending.len(),
                active.len() + 1,
                config.max_concurrent_downloads,
                config.policy.label()
            ),
        });
        cleanup_pack_publish_temps(&local_path);
        cleanup_pack_publish_temps(&index_path_for_pack_path(&local_path));
        let status = pack_status_from_state(&pack, &local_path, state);
        let show_download_progress = media_status_shows_download_progress(&status);
        if show_download_progress || matches!(status, LocalPackStatus::Current) {
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
        }
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
            LocalPackStatus::IndexMissing => {
                format!(
                    "local_path={} index_path={} reason=index-missing",
                    local_path.display(),
                    index_path_for_pack_path(&local_path).display()
                )
            }
            LocalPackStatus::IndexStale { reason } => {
                format!(
                    "local_path={} index_path={} reason={reason}",
                    local_path.display(),
                    index_path_for_pack_path(&local_path).display()
                )
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
            pending_reconciliation.insert(pack.id.clone(), (pack.clone(), local_path.clone()));
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
            if show_download_progress {
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
            }
            continue;
        }
        let (result_tx, result_rx) = mpsc::channel();
        let download_config = config.clone();
        let download_pack = pack.clone();
        let download_local_path = local_path.clone();
        let download_status = status.clone();
        let download_tx = tx.clone();
        std::thread::Builder::new()
            .name(format!("screenshot-media-{}", pack.id))
            .spawn(move || {
                // A requested pack is visible user work. Acquire foreground
                // ownership before raising this worker to nice 0/AllOnline;
                // index-sidecar repair remains cooperative background work.
                let _lease = mister_magik_catalog::work_coordinator::foreground("media-download");
                apply_runtime_thread_policy(RuntimeThreadRole::MediaDownload);
                let result = download_pack_assets(
                    &download_config,
                    &download_pack,
                    &download_local_path,
                    &download_status,
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
            show_completion_progress: show_download_progress,
            rx: result_rx,
        });
    }
}

fn media_status_shows_download_progress(status: &LocalPackStatus) -> bool {
    status.requires_pack_download()
}

fn poll_active_downloads(
    active: &mut Vec<ActiveDownload>,
    pending_reconciliation: &mut BTreeMap<String, (MediaPack, PathBuf)>,
    counts: &mut MediaCheckCounts,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) {
    let mut idx = 0;
    while idx < active.len() {
        match active[idx].rx.try_recv() {
            Ok(Ok(())) => {
                let done = active.remove(idx);
                counts.downloaded += 1;
                pending_reconciliation.insert(
                    done.pack.id.clone(),
                    (done.pack.clone(), done.local_path.clone()),
                );
                if done.show_completion_progress {
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
                }
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

fn download_pack_assets(
    config: &MediaWorkerConfig,
    pack: &MediaPack,
    local_path: &Path,
    status: &LocalPackStatus,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) -> Result<(), String> {
    if status.requires_pack_download() {
        return download_raw_pack_and_index(config, pack, local_path, pack_index, pack_count, tx);
    }
    if status.requires_index_download() {
        return download_index_for_current_pack(
            config, pack, local_path, pack_index, pack_count, tx,
        );
    }
    Ok(())
}

fn download_raw_pack_and_index(
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
    let work_dir = PathBuf::from(MEDIA_DOWNLOAD_WORK_DIR);
    fs::create_dir_all(&work_dir)
        .map_err(|e| format!("create media work dir {}: {e}", work_dir.display()))?;
    let headers_tmp = work_dir.join(format!(
        "{}-{}-{}.headers",
        pack.id,
        pack.image_size,
        unix_ms_now()
    ));
    let stage_path = work_dir.join(format!(
        "{}-{}-{}.mmlz4b.stage",
        pack.id,
        pack.image_size,
        unix_ms_now()
    ));
    let _ = fs::remove_file(&stage_path);
    let index_path = index_path_for_pack_path(local_path);
    let index_publish = if pack.index.is_some() {
        Some(prepare_artifact_publish(
            &index_path,
            hidden_timestamped_temp_path_for(&index_path, "screenshot-pack-index", unix_ms_now()),
            ArtifactPublishLabels {
                destination: "pack index destination",
                parent: "pack index destination parent",
            },
        )?)
    } else {
        None
    };
    let result = (|| {
        let pending_index = match (&pack.index, index_publish.as_ref()) {
            (Some(index), Some(index_publish)) => Some(start_silent_index_download(
                index,
                pack,
                index_publish,
                &work_dir,
                pack_index,
                pack_count,
                tx,
            )?),
            _ => None,
        };
        let pack_stream_result = stream_variant_to_stage_file(
            variant,
            pack,
            &stage_path,
            &headers_tmp,
            pack_index,
            pack_count,
            tx,
        );
        let index_stream_result = pending_index
            .map(join_silent_index_download)
            .transpose()
            .map_err(|error| format!("pack index download failed: {error}"))?;
        let (streamed, metadata) = pack_stream_result?;
        let _ = tx.send(MediaWorkerMessage::CacheMetadata {
            scope: format!("pack:{}", pack.id),
            metadata: metadata.clone(),
        });
        send_progress(
            tx,
            MediaProgressEvent::for_pack(pack, "identity", "verify", pack_index, pack_count)
                .with_done_bytes(variant.bytes),
        );
        verify_streamed_download(&streamed, variant.bytes, &variant.sha256)?;
        verify_streamed_download(&streamed, pack.raw.bytes, &pack.raw.sha256)?;
        let index_streamed = if let Some((index_streamed, index_metadata)) = index_stream_result {
            if let Some(index) = pack.index.as_ref() {
                let _ = tx.send(MediaWorkerMessage::CacheMetadata {
                    scope: format!("pack-index:{}", pack.id),
                    metadata: index_metadata,
                });
                verify_streamed_download(&index_streamed, index.bytes, &index.sha256)
                    .map_err(|error| format!("pack index verify failed: {error}"))?;
            }
            Some(index_streamed)
        } else {
            None
        };
        install_staged_pack(
            &stage_path,
            &streamed,
            local_path,
            pack,
            pack_index,
            pack_count,
            tx,
        )?;
        if let (Some(index), Some(index_publish), Some(index_streamed)) =
            (&pack.index, index_publish.as_ref(), index_streamed.as_ref())
        {
            install_streamed_index_silent(
                index_publish,
                index_streamed,
                index,
                pack,
                pack_index,
                pack_count,
                tx,
            )
            .map_err(|error| format!("pack index install failed: {error}"))?;
        }
        write_download_state(
            &config.asset_dir,
            pack,
            local_path,
            variant,
            Some(&metadata),
        )
    })();
    if result.is_err() {
        let _ = fs::remove_file(&stage_path);
        cleanup_pack_publish_temps(local_path);
        if let Some(index_publish) = index_publish.as_ref() {
            index_publish.cleanup_temp();
        }
    }
    let _ = fs::remove_file(headers_tmp);
    if result.is_ok() {
        let _ = fs::remove_file(stage_path);
    }
    result
}

fn download_index_for_current_pack(
    config: &MediaWorkerConfig,
    pack: &MediaPack,
    local_path: &Path,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) -> Result<(), String> {
    let index = pack
        .index
        .as_ref()
        .ok_or_else(|| format!("pack {} has no index sidecar", pack.id))?;
    let variant = pack
        .variant_for_compression("none")
        .ok_or_else(|| format!("pack {} has no compression=none variant", pack.id))?;
    fs::create_dir_all(&config.asset_dir)
        .map_err(|e| format!("create asset dir {}: {e}", config.asset_dir.display()))?;
    let work_dir = PathBuf::from(MEDIA_DOWNLOAD_WORK_DIR);
    fs::create_dir_all(&work_dir)
        .map_err(|e| format!("create media work dir {}: {e}", work_dir.display()))?;
    let index_path = index_path_for_pack_path(local_path);
    let headers_tmp = work_dir.join(format!(
        "{}-{}-{}-index.headers",
        pack.id,
        pack.image_size,
        unix_ms_now()
    ));
    let publish = prepare_artifact_publish(
        &index_path,
        hidden_timestamped_temp_path_for(&index_path, "screenshot-pack-index", unix_ms_now()),
        ArtifactPublishLabels {
            destination: "pack index destination",
            parent: "pack index destination parent",
        },
    )?;
    let result = stream_index_to_publish_temp(
        index,
        pack,
        &publish,
        &headers_tmp,
        pack_index,
        pack_count,
        tx,
        false,
    )
    .and_then(|(streamed, metadata)| {
        let _ = tx.send(MediaWorkerMessage::CacheMetadata {
            scope: format!("pack-index:{}", pack.id),
            metadata,
        });
        verify_streamed_download(&streamed, index.bytes, &index.sha256)
            .map_err(|error| format!("pack index verify failed: {error}"))?;
        install_streamed_index_silent(&publish, &streamed, index, pack, pack_index, pack_count, tx)
            .map_err(|error| format!("pack index install failed: {error}"))?;
        write_download_state(&config.asset_dir, pack, local_path, variant, None)
    });
    if result.is_err() {
        publish.cleanup_temp();
    }
    let _ = fs::remove_file(headers_tmp);
    result
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StreamedPackDownload {
    bytes: u64,
    sha256: String,
}

struct PendingIndexDownload {
    handle: JoinHandle<Result<(StreamedPackDownload, HttpCacheMetadata), String>>,
    headers_tmp: PathBuf,
}

fn stream_variant_to_stage_file(
    variant: &MediaVariant,
    pack: &MediaPack,
    stage_path: &Path,
    headers_path: &Path,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) -> Result<(StreamedPackDownload, HttpCacheMetadata), String> {
    stream_media_object_to_path(
        &variant.url,
        variant.bytes,
        "identity",
        "pack",
        pack,
        stage_path,
        headers_path,
        pack_index,
        pack_count,
        tx,
        true,
    )
}

fn stream_index_to_publish_temp(
    index: &MediaIndex,
    pack: &MediaPack,
    publish: &crate::artifact_publish::ArtifactPublishPlan,
    headers_path: &Path,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
    emit_progress: bool,
) -> Result<(StreamedPackDownload, HttpCacheMetadata), String> {
    stream_media_object_to_path(
        &index.url,
        index.bytes,
        "index",
        "pack index",
        pack,
        publish.temp_path(),
        headers_path,
        pack_index,
        pack_count,
        tx,
        emit_progress,
    )
}

fn start_silent_index_download(
    index: &MediaIndex,
    pack: &MediaPack,
    publish: &crate::artifact_publish::ArtifactPublishPlan,
    work_dir: &Path,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) -> Result<PendingIndexDownload, String> {
    let headers_tmp = work_dir.join(format!(
        "{}-{}-{}-index.headers",
        pack.id,
        pack.image_size,
        unix_ms_now()
    ));
    let index = index.clone();
    let pack = pack.clone();
    let publish = publish.clone();
    let thread_headers_tmp = headers_tmp.clone();
    let tx = tx.clone();
    let handle = std::thread::Builder::new()
        .name(format!("screenshot-media-{}-index", pack.id))
        .spawn(move || {
            let _lease = mister_magik_catalog::work_coordinator::background("media-index");
            apply_runtime_thread_policy(RuntimeThreadRole::MediaIndex);
            stream_index_to_publish_temp(
                &index,
                &pack,
                &publish,
                &thread_headers_tmp,
                pack_index,
                pack_count,
                &tx,
                false,
            )
        })
        .map_err(|e| format!("spawn pack index download: {e}"))?;
    Ok(PendingIndexDownload {
        handle,
        headers_tmp,
    })
}

fn join_silent_index_download(
    pending: PendingIndexDownload,
) -> Result<(StreamedPackDownload, HttpCacheMetadata), String> {
    let result = pending
        .handle
        .join()
        .map_err(|_| "pack index download worker panicked".to_string())
        .and_then(|result| result);
    let _ = fs::remove_file(pending.headers_tmp);
    result
}

fn stream_media_object_to_path(
    url: &str,
    expected_bytes: u64,
    progress_variant: &str,
    object_label: &str,
    pack: &MediaPack,
    output_path: &Path,
    headers_path: &Path,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
    emit_progress: bool,
) -> Result<(StreamedPackDownload, HttpCacheMetadata), String> {
    if emit_progress {
        send_progress(
            tx,
            MediaProgressEvent::for_pack(
                pack,
                progress_variant,
                "download_start",
                pack_index,
                pack_count,
            ),
        );
    }
    let mut output =
        File::create(output_path).map_err(|e| format!("create {}: {e}", output_path.display()))?;
    let mut sha = spawn_sha256_stdin()?;
    let started = Instant::now();
    let mut curl = Command::new("curl");
    add_curl_download_args(&mut curl, url, headers_path, expected_bytes);
    let mut child = match curl.stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(child) => child,
        Err(error) => {
            drop(sha.stdin.take());
            let _ = sha.wait();
            return Err(format!("spawn curl: {error}"));
        }
    };
    let mut input = match child.stdout.take() {
        Some(input) => input,
        None => {
            terminate_and_reap(&mut child);
            drop(sha.stdin.take());
            let _ = sha.wait();
            return Err("missing curl stdout pipe".to_string());
        }
    };
    let mut sha_stdin = match sha.stdin.take() {
        Some(stdin) => stdin,
        None => {
            terminate_and_reap(&mut child);
            let _ = sha.wait();
            return Err("missing sha256 stdin pipe".to_string());
        }
    };
    let mut buffer = vec![0u8; crate::media_pack_save::PROGRESS_COPY_CHUNK_BYTES];
    let mut bytes = 0u64;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
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
                expected_bytes,
                object_label,
            )?;
            if emit_progress
                && (last_emit.elapsed() >= Duration::from_millis(250) || bytes >= expected_bytes)
            {
                last_emit = Instant::now();
                send_progress(
                    tx,
                    MediaProgressEvent::for_pack(
                        pack,
                        progress_variant,
                        "download",
                        pack_index,
                        pack_count,
                    )
                    .with_bytes(bytes, expected_bytes)
                    .with_download_mbps(mbps(bytes, started.elapsed())),
                );
            }
        }
        output.flush().map_err(|e| {
            format!(
                "flush streamed {object_label} {}: {e}",
                output_path.display()
            )
        })
    })();
    drop(sha_stdin);
    drop(output);
    drop(input);
    if transfer_result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| format!("wait curl: {e}"))?;
    let sha_output = sha
        .wait_with_output()
        .map_err(|e| format!("wait sha256 command: {e}"))?;
    transfer_result?;
    if !status.success() {
        return Err(format!("curl exited with {status}"));
    }
    if !sha_output.status.success() {
        return Err(format!("sha256 command exited with {}", sha_output.status));
    }
    let actual_sha = parse_sha256_output(&sha_output.stdout)?;
    let header_text = fs::read_to_string(headers_path).unwrap_or_default();
    if emit_progress {
        send_progress(
            tx,
            MediaProgressEvent::for_pack(
                pack,
                progress_variant,
                "download_done",
                pack_index,
                pack_count,
            )
            .with_bytes(bytes, expected_bytes)
            .with_download_mbps(mbps(bytes, started.elapsed())),
        );
    }
    Ok((
        StreamedPackDownload {
            bytes,
            sha256: actual_sha,
        },
        parse_http_headers(&header_text, url, "response"),
    ))
}

fn terminate_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn mbps(bytes: u64, elapsed: Duration) -> f64 {
    let ms = elapsed.as_millis() as f64;
    if ms <= 0.0 {
        0.0
    } else {
        (bytes as f64 * 8.0) / (ms * 1000.0)
    }
}

fn verify_streamed_download(
    streamed: &StreamedPackDownload,
    expected_bytes: u64,
    expected_sha: &str,
) -> Result<(), String> {
    if streamed.bytes != expected_bytes {
        return Err(format!(
            "size mismatch expected={expected_bytes} actual={}",
            streamed.bytes
        ));
    }
    if streamed.sha256 != expected_sha {
        return Err(format!(
            "sha256 mismatch expected={expected_sha} actual={}",
            streamed.sha256
        ));
    }
    Ok(())
}

fn install_staged_pack(
    stage_path: &Path,
    streamed: &StreamedPackDownload,
    final_path: &Path,
    pack: &MediaPack,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) -> Result<(), String> {
    let bytes = stage_path
        .metadata()
        .map_err(|e| format!("stat staged screenshot pack {}: {e}", stage_path.display()))?
        .len();
    if bytes != streamed.bytes {
        return Err(format!(
            "staged file size mismatch counted={} stat={bytes}",
            streamed.bytes
        ));
    }
    crate::media_pack_save::publish_pack_file_with_progress(stage_path, final_path, |event| {
        send_progress(
            tx,
            MediaProgressEvent::for_pack(
                pack,
                "identity",
                event.phase.label(),
                pack_index,
                pack_count,
            )
            .with_bytes(event.bytes_done, event.bytes_total),
        );
    })?;
    invalidate_preview_archive_metadata_cache("media_pack_published");
    Ok(())
}

fn install_streamed_index_silent(
    publish: &crate::artifact_publish::ArtifactPublishPlan,
    streamed: &StreamedPackDownload,
    index: &MediaIndex,
    pack: &MediaPack,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) -> Result<(), String> {
    if streamed.bytes != index.bytes {
        return Err(format!(
            "streamed index size mismatch expected={} actual={}",
            index.bytes, streamed.bytes
        ));
    }
    install_streamed_object(
        publish,
        streamed,
        pack,
        "index",
        "screenshot pack index",
        pack_index,
        pack_count,
        tx,
        false,
    )
}

fn install_streamed_object(
    publish: &crate::artifact_publish::ArtifactPublishPlan,
    streamed: &StreamedPackDownload,
    pack: &MediaPack,
    progress_variant: &str,
    install_label: &str,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
    emit_progress: bool,
) -> Result<(), String> {
    let mut fault_control = mister_magik_catalog::fs_fault::NoopDirectResetFaultControl;
    install_streamed_object_with_fault_control(
        publish,
        streamed,
        pack,
        progress_variant,
        install_label,
        pack_index,
        pack_count,
        tx,
        emit_progress,
        &mut fault_control,
    )
}

#[allow(clippy::too_many_arguments)]
fn install_streamed_object_with_fault_control(
    publish: &crate::artifact_publish::ArtifactPublishPlan,
    streamed: &StreamedPackDownload,
    pack: &MediaPack,
    progress_variant: &str,
    install_label: &str,
    pack_index: usize,
    pack_count: usize,
    tx: &mpsc::Sender<MediaWorkerMessage>,
    emit_progress: bool,
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
    let bytes = publish
        .temp_path()
        .metadata()
        .map_err(|e| {
            format!(
                "stat streamed {install_label} {}: {e}",
                publish.temp_path().display()
            )
        })?
        .len();
    if bytes != streamed.bytes {
        return Err(format!(
            "streamed file size mismatch counted={} stat={bytes}",
            streamed.bytes
        ));
    }
    if progress_variant == "index" {
        mister_magik_catalog::fs_fault::maybe_fault_with_control(
            "media.index.after_temp_write",
            publish.temp_path(),
            fault_control,
        );
    }
    if emit_progress {
        send_progress(
            tx,
            MediaProgressEvent::for_pack(pack, progress_variant, "save", pack_index, pack_count)
                .with_done_bytes(streamed.bytes),
        );
    }
    let file = File::options()
        .read(true)
        .open(publish.temp_path())
        .map_err(|e| format!("open streamed pack {}: {e}", publish.temp_path().display()))?;
    if emit_progress {
        send_progress(
            tx,
            MediaProgressEvent::for_pack(pack, progress_variant, "sync", pack_index, pack_count)
                .with_done_bytes(streamed.bytes),
        );
    }
    file.sync_all().map_err(|e| {
        format!(
            "sync streamed {install_label} {}: {e}",
            publish.temp_path().display()
        )
    })?;
    if progress_variant == "index" {
        mister_magik_catalog::fs_fault::maybe_fault_with_control(
            "media.index.after_temp_sync",
            publish.temp_path(),
            fault_control,
        );
    }
    if emit_progress {
        send_progress(
            tx,
            MediaProgressEvent::for_pack(pack, progress_variant, "rename", pack_index, pack_count)
                .with_done_bytes(streamed.bytes),
        );
    }
    publish.install_temp(Some(install_label))?;
    if progress_variant == "index" {
        mister_magik_catalog::fs_fault::maybe_fault_with_control(
            "media.index.after_rename_before_parent_sync",
            publish.temp_path(),
            fault_control,
        );
    }
    invalidate_preview_archive_metadata_cache("media_pack_published");
    if emit_progress {
        send_progress(
            tx,
            MediaProgressEvent::for_pack(
                pack,
                progress_variant,
                "parent-sync",
                pack_index,
                pack_count,
            )
            .with_done_bytes(streamed.bytes),
        );
    }
    sync_path_rust_best_effort(publish.parent());
    Ok(())
}

fn spawn_sha256_stdin() -> Result<Child, String> {
    Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .or_else(|_| {
            Command::new("shasum")
                .arg("-a")
                .arg("256")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
        })
        .map_err(|e| format!("spawn sha256 command: {e}"))
}

fn parse_sha256_output(output: &[u8]) -> Result<String, String> {
    mister_magik_media_contract::Sha256::parse_command_output(output)
        .map(mister_magik_media_contract::Sha256::into_string)
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
    if let Some(index) = &pack.index {
        pack_state["index"] = serde_json::json!({
            "codec": index.codec,
            "object": index.object,
            "bytes": index.bytes,
            "sha256": index.sha256,
            "archive_bytes": index.archive_bytes,
            "archive_sha256": index.archive_sha256,
            "local_path": index_path_for_pack_path(local_path).display().to_string(),
        });
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
    let mut fault_control = mister_magik_catalog::fs_fault::NoopDirectResetFaultControl;
    write_json_atomic_with_fault_control(path, value, &mut fault_control)
}

fn write_json_atomic_with_fault_control(
    path: &Path,
    value: &Value,
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
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
    file.write_all(text.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|e| format!("write media state {}: {e}", publish.temp_path().display()))?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "media.state.after_temp_write",
        path,
        fault_control,
    );
    file.sync_all()
        .map_err(|e| format!("sync media state {}: {e}", publish.temp_path().display()))?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "media.state.after_temp_sync",
        path,
        fault_control,
    );
    publish.install_temp(Some("media state"))?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "media.state.after_rename_before_parent_sync",
        path,
        fault_control,
    );
    invalidate_preview_archive_metadata_cache("media_state_published");
    sync_path_rust_best_effort(publish.parent());
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
    let fetched = crate::media_http::fetch_manifest(manifest_url)?;
    let body = String::from_utf8(fetched.bytes).map_err(|e| format!("manifest utf8: {e}"))?;
    Ok((
        body,
        parse_http_headers(&fetched.headers, manifest_url, "response"),
    ))
}

fn add_curl_download_args(command: &mut Command, url: &str, headers_path: &Path, max_bytes: u64) {
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
        .arg("Accept-Encoding: identity")
        .arg("-D")
        .arg(headers_path)
        .arg("-o")
        .arg("-");
    if url.starts_with("https://") && Path::new("/etc/ssl/certs/cacert.pem").is_file() {
        command.arg("--cacert").arg("/etc/ssl/certs/cacert.pem");
    }
    command.arg(url);
}

fn fetch_manifest_text_with_retry<F>(
    manifest_url: &str,
    attempts: usize,
    initial_backoff: Duration,
    tx: &mpsc::Sender<MediaWorkerMessage>,
    fetch: F,
) -> Result<(String, HttpCacheMetadata), String>
where
    F: FnMut(&str) -> Result<(String, HttpCacheMetadata), String>,
{
    fetch_manifest_text_with_retry_and_sleep(
        manifest_url,
        attempts,
        initial_backoff,
        tx,
        fetch,
        std::thread::sleep,
    )
}

fn fetch_manifest_text_with_retry_and_sleep<F, S>(
    manifest_url: &str,
    attempts: usize,
    initial_backoff: Duration,
    tx: &mpsc::Sender<MediaWorkerMessage>,
    mut fetch: F,
    mut sleep: S,
) -> Result<(String, HttpCacheMetadata), String>
where
    F: FnMut(&str) -> Result<(String, HttpCacheMetadata), String>,
    S: FnMut(Duration),
{
    let attempts = attempts.max(1);
    let started = Instant::now();
    let mut retry_policy = ManifestRetryPolicy::new(initial_backoff);
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        match fetch(manifest_url) {
            Ok(fetched) => return Ok(fetched),
            Err(error) => {
                last_error = error;
                if attempt == attempts {
                    break;
                }
                let retry = retry_policy.next_delay(started.elapsed());
                let _ = tx.send(MediaWorkerMessage::Timing {
                    name: "screenshot_media_manifest_retry".to_string(),
                    detail: format!(
                        "attempt={attempt} attempts={attempts} elapsed_ms={} retry_ms={} error={last_error}",
                        started.elapsed().as_millis(),
                        retry.as_millis()
                    ),
                });
                if !retry.is_zero() {
                    sleep(retry);
                }
            }
        }
    }
    Err(last_error)
}

#[derive(Clone, Debug)]
struct ManifestRetryPolicy {
    backoff: Duration,
}

impl ManifestRetryPolicy {
    fn new(initial_backoff: Duration) -> Self {
        Self {
            backoff: initial_backoff,
        }
    }

    fn next_delay(&mut self, elapsed: Duration) -> Duration {
        if elapsed < MANIFEST_FETCH_FAST_RETRY_WINDOW {
            return MANIFEST_FETCH_FAST_RETRY;
        }
        let delay = self.backoff;
        self.backoff = self.backoff.saturating_mul(2).min(MANIFEST_FETCH_MAX_RETRY);
        delay
    }
}

fn read_media_state(asset_dir: &Path) -> Option<Value> {
    let text = fs::read_to_string(state_path(&asset_dir.display().to_string())).ok()?;
    serde_json::from_str(&text).ok()
}

#[derive(Clone, Debug)]
pub struct MediaWorkerConfig {
    policy: MediaUpdatePolicy,
    manifest_url: String,
    image_size: String,
    asset_dir: PathBuf,
    catalog_root: PathBuf,
    max_concurrent_downloads: usize,
    benchmark_auto_finish: bool,
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
        let manifest_url = std::env::var("MISTER_MEDIA_MANIFEST_URL")
            .unwrap_or_else(|_| DEFAULT_MANIFEST_URL.to_string());
        mister_magik_media_contract::validate_https_manifest_url(&manifest_url)?;
        Ok(Self {
            policy,
            manifest_url,
            image_size,
            asset_dir: PathBuf::from(
                std::env::var("MISTER_MEDIA_ASSET_DIR")
                    .unwrap_or_else(|_| DEFAULT_ASSET_DIR.to_string()),
            ),
            catalog_root: mister_magik_catalog::catalog_config::default_sharded_catalog_path(),
            max_concurrent_downloads: media_download_concurrency_from_env(),
            benchmark_auto_finish: media_benchmark_auto_finish_enabled(),
        })
    }

    pub fn for_host(asset_dir: PathBuf, catalog_root: PathBuf) -> Result<Self, String> {
        let manifest_url = DEFAULT_MANIFEST_URL.to_string();
        mister_magik_media_contract::validate_https_manifest_url(&manifest_url)?;
        Ok(Self {
            policy: MediaUpdatePolicy::Download,
            manifest_url,
            image_size: DEFAULT_IMAGE_SIZE.to_string(),
            asset_dir,
            catalog_root,
            max_concurrent_downloads: DEFAULT_MAX_CONCURRENT_MEDIA_DOWNLOADS,
            benchmark_auto_finish: false,
        })
    }
}

#[cfg(feature = "bench-tools")]
fn media_benchmark_auto_finish_enabled() -> bool {
    matches!(
        std::env::var("MISTER_MEDIA_BENCH_CONTENTION").as_deref(),
        Ok("1" | "true" | "yes" | "on")
    )
}

#[cfg(not(feature = "bench-tools"))]
fn media_benchmark_auto_finish_enabled() -> bool {
    false
}

fn media_download_concurrency_from_env() -> usize {
    media_download_concurrency_from_value(std::env::var("MISTER_MEDIA_CONCURRENCY").ok().as_deref())
}

fn media_download_concurrency_from_value(_value: Option<&str>) -> usize {
    DEFAULT_MAX_CONCURRENT_MEDIA_DOWNLOADS
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
pub enum MediaWorkerMessage {
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
    PreviewAvailabilityUpdated {
        outcome: PreviewAvailabilityReconciliationOutcome,
    },
    PreviewAvailabilityFailed {
        system: String,
        detail: String,
    },
    Failed {
        detail: String,
    },
    Done {
        detail: String,
    },
}

fn reconcile_pack_preview_availability(
    catalog_root: &Path,
    pack: &MediaPack,
    local_path: &Path,
    tx: &mpsc::Sender<MediaWorkerMessage>,
) {
    let system_id = match mister_magik_catalog::catalog_classify::SystemId::parse(&pack.id) {
        Ok(system_id) => system_id,
        Err(error) => {
            let _ = tx.send(MediaWorkerMessage::PreviewAvailabilityFailed {
                system: pack.id.clone(),
                detail: error.to_string(),
            });
            return;
        }
    };
    match reconcile_production_preview_availability(
        catalog_root,
        &system_id,
        local_path,
        production_registry_limits(),
    ) {
        Ok(outcome) => {
            let _ = tx.send(MediaWorkerMessage::PreviewAvailabilityUpdated { outcome });
        }
        Err(error) => {
            let _ = tx.send(MediaWorkerMessage::PreviewAvailabilityFailed {
                system: pack.id.clone(),
                detail: error.to_string(),
            });
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaProgressEvent {
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
pub struct HttpCacheMetadata {
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
    pub fn log_detail(&self, scope: &str) -> String {
        format!(
            "scope={} source={} status={} etag={} last_modified={} cache_control={} age={} cf_cache_status={} cf_ray={} content_length={} content_encoding={} effective_url={}",
            scope,
            self.source,
            self.status
                .map(|value| value.to_string())
                .unwrap_or_default(),
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

fn parse_http_headers(text: &str, effective_url: &str, source: &str) -> HttpCacheMetadata {
    let headers = mister_magik_media_contract::HttpHeaders::parse(text);
    let mut metadata = HttpCacheMetadata {
        effective_url: effective_url.to_string(),
        source: source.to_string(),
        status: headers.status,
        etag: headers.get("etag").unwrap_or_default().to_string(),
        last_modified: headers.get("last-modified").unwrap_or_default().to_string(),
        cache_control: headers.get("cache-control").unwrap_or_default().to_string(),
        age: headers.get("age").unwrap_or_default().to_string(),
        cf_cache_status: headers
            .get("cf-cache-status")
            .unwrap_or_default()
            .to_string(),
        cf_ray: headers.get("cf-ray").unwrap_or_default().to_string(),
        content_length: headers
            .get("content-length")
            .unwrap_or_default()
            .to_string(),
        content_encoding: headers
            .get("content-encoding")
            .unwrap_or_default()
            .to_string(),
        ..Default::default()
    };
    if let Some(location) = headers.get("location") {
        metadata.effective_url = location.to_string();
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

    pub fn log_detail(&self) -> String {
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
    use crate::media_update::parse_manifest_json;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[derive(Default)]
    struct RecordingFaultControl {
        points: Vec<String>,
    }

    impl mister_magik_catalog::fs_fault::DirectResetFaultControl for RecordingFaultControl {
        fn request_direct_reset(
            &mut self,
            request: &mister_magik_catalog::fs_fault::DirectResetFaultRequest,
        ) -> mister_magik_catalog::fs_fault::DirectResetFaultOutcome {
            self.points.push(request.point().to_string());
            mister_magik_catalog::fs_fault::DirectResetFaultOutcome::Noop
        }
    }

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
    fn media_state_fault_hook_preserves_atomic_publication_order() {
        let dir = temp_dir("mister-magik-media-state-fault-hook");
        let path = dir.join("state.json");
        let mut fault_control = RecordingFaultControl::default();

        write_json_atomic_with_fault_control(
            &path,
            &serde_json::json!({"schema": 1}),
            &mut fault_control,
        )
        .expect("write media state");

        assert_eq!(
            fault_control.points,
            vec![
                "media.state.after_temp_write",
                "media.state.after_temp_sync",
                "media.state.after_rename_before_parent_sync",
            ]
        );
        let _ = fs::remove_dir_all(dir);
    }

    fn indexed_pack_fixture() -> MediaPack {
        let text = format!(
            r#"{{
  "schema": 1,
  "generated_at": "2026-06-22T16:58:28Z",
  "packs": [
    {{
      "id": "arcade",
      "version": "2026.06.22",
      "object": "mister-magik/v1/packs/arcade/screenshots/320x320/2026.06.22/{SHA}.mmlz4b",
      "bytes": 4,
      "sha256": "{SHA}",
      "codec": "mmlz4b",
      "index": {{
        "object": "mister-magik/v1/packs/arcade/screenshots/320x320/2026.06.22/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.mmlz4b.idx",
        "bytes": 2,
        "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "codec": "mmlz4b-index-v2",
        "archive_bytes": 4,
        "archive_sha256": "{SHA}"
      }}
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
    fn default_media_selection_uses_native_snes_pack_geometry() {
        let mut snes_native = pack_with_id("snes");
        snes_native.image_size = "256x224".to_string();
        let mut snes_default = pack_with_id("snes");
        snes_default.image_size = DEFAULT_IMAGE_SIZE.to_string();
        let arcade = pack_with_id("arcade");

        let selected =
            packs_by_system_for_size(&[snes_default, snes_native, arcade], DEFAULT_IMAGE_SIZE);

        assert_eq!(selected["snes"].image_size, "256x224");
        assert_eq!(selected["arcade"].image_size, DEFAULT_IMAGE_SIZE);
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
    fn media_request_queue_starts_one_active_pack_at_a_time() {
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

        let still_no_slots =
            dequeue_startable_requests(&mut pending, 1, MAX_CONCURRENT_MEDIA_DOWNLOADS);
        assert!(still_no_slots.is_empty());
        assert_eq!(pending.len(), 4);
    }

    #[test]
    fn media_download_concurrency_env_is_clamped_to_single_pack() {
        assert_eq!(media_download_concurrency_from_value(Some("3")), 1);
    }

    #[test]
    fn index_only_repairs_do_not_request_visible_download_progress() {
        assert!(media_status_shows_download_progress(
            &LocalPackStatus::Missing
        ));
        assert!(media_status_shows_download_progress(
            &LocalPackStatus::Stale {
                reason: "sha-mismatch".to_string()
            }
        ));
        assert!(!media_status_shows_download_progress(
            &LocalPackStatus::IndexMissing
        ));
        assert!(!media_status_shows_download_progress(
            &LocalPackStatus::IndexStale {
                reason: "index-sha-mismatch".to_string()
            }
        ));
        assert!(!media_status_shows_download_progress(
            &LocalPackStatus::Current
        ));
    }

    #[test]
    fn mbps_uses_wire_megabits_per_second() {
        assert_eq!(mbps(1_000_000, Duration::from_millis(1000)), 8.0);
    }

    #[test]
    fn media_object_curl_is_http_capable_and_bounded() {
        let mut command = Command::new("curl");
        add_curl_download_args(
            &mut command,
            "http://assets.example/pack.mmlz4b",
            Path::new("/tmp/headers"),
            123,
        );
        let text = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");

        assert!(text.contains("--proto =http,https"));
        assert!(text.contains("--connect-timeout 10"));
        assert!(text.contains("--max-time 1200"));
        assert!(text.contains("--max-filesize 123"));
        assert!(text.contains("http://assets.example/pack.mmlz4b"));
    }

    #[test]
    fn parses_cloudflare_cache_headers_from_http_output() {
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

        let metadata = parse_http_headers(headers, "https://assets.example/pack", "response");

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
        let mut sleeps = Vec::new();

        let (body, metadata) = fetch_manifest_text_with_retry_and_sleep(
            DEFAULT_MANIFEST_URL,
            3,
            Duration::ZERO,
            &tx,
            |_| {
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
            },
            |duration| sleeps.push(duration),
        )
        .expect("third attempt should succeed");

        assert_eq!(calls, 3);
        assert_eq!(sleeps, [Duration::from_secs(1), Duration::from_secs(1)]);
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
        assert!(retries[0].contains("retry_ms=1000"));
        assert!(retries[1].contains("attempt=2"));
        assert!(retries[1].contains("retry_ms=1000"));
    }

    #[test]
    fn manifest_retry_policy_uses_fast_start_then_exponential_backoff() {
        let mut policy = ManifestRetryPolicy::new(Duration::from_secs(2));

        assert_eq!(policy.next_delay(Duration::ZERO), Duration::from_secs(1));
        assert_eq!(
            policy.next_delay(MANIFEST_FETCH_FAST_RETRY_WINDOW - Duration::from_millis(1)),
            Duration::from_secs(1)
        );
        assert_eq!(
            policy.next_delay(MANIFEST_FETCH_FAST_RETRY_WINDOW),
            Duration::from_secs(2)
        );
        assert_eq!(
            policy.next_delay(MANIFEST_FETCH_FAST_RETRY_WINDOW + Duration::from_secs(1)),
            Duration::from_secs(4)
        );
        assert_eq!(
            policy.next_delay(MANIFEST_FETCH_FAST_RETRY_WINDOW + Duration::from_secs(2)),
            Duration::from_secs(8)
        );
        assert_eq!(
            policy.next_delay(MANIFEST_FETCH_FAST_RETRY_WINDOW + Duration::from_secs(3)),
            Duration::from_secs(10)
        );
        assert_eq!(
            policy.next_delay(MANIFEST_FETCH_FAST_RETRY_WINDOW + Duration::from_secs(4)),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn verify_streamed_download_rejects_bad_checksum() {
        let streamed = StreamedPackDownload {
            bytes: 4,
            sha256: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string(),
        };

        let err = verify_streamed_download(&streamed, 4, SHA).unwrap_err();

        assert!(err.contains("sha256 mismatch"));
    }

    #[test]
    fn staged_pack_publish_replaces_only_after_validated_download() {
        let dir = temp_dir("mister-magik-stage-publish-pack");
        let final_path = dir.join("arcade-screenshots-320x320.mmlz4b");
        let stage_path = dir.join("download-stage.mmlz4b");
        let temp_path = crate::media_pack_save::temp_path_for(&final_path);
        fs::write(&stage_path, b"new").unwrap();
        fs::write(&final_path, b"old").unwrap();
        let streamed = StreamedPackDownload {
            bytes: 3,
            sha256: SHA.to_string(),
        };
        let pack = pack_fixture();
        let (tx, rx) = mpsc::channel();

        install_staged_pack(&stage_path, &streamed, &final_path, &pack, 1, 1, &tx).unwrap();

        assert_eq!(fs::read(&final_path).unwrap(), b"new");
        assert!(!temp_path.exists());
        let phases: Vec<_> = rx
            .try_iter()
            .filter_map(|message| match message {
                MediaWorkerMessage::Progress(event) => Some(event.phase),
                _ => None,
            })
            .collect();
        assert_eq!(phases, ["save", "sync", "rename", "parent-sync"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn staged_pack_publish_rejects_size_mismatch_without_replacing_destination() {
        let dir = temp_dir("mister-magik-stage-publish-size-mismatch");
        let final_path = dir.join("arcade-screenshots-320x320.mmlz4b");
        let stage_path = dir.join("download-stage.mmlz4b");
        fs::write(&stage_path, b"new-but-too-long").unwrap();
        fs::write(&final_path, b"old").unwrap();
        let streamed = StreamedPackDownload {
            bytes: 3,
            sha256: SHA.to_string(),
        };
        let pack = pack_fixture();
        let (tx, _rx) = mpsc::channel();

        let err =
            install_staged_pack(&stage_path, &streamed, &final_path, &pack, 1, 1, &tx).unwrap_err();

        assert!(err.contains("staged file size mismatch"));
        assert_eq!(fs::read(&final_path).unwrap(), b"old");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn streamed_index_publish_is_silent_on_success() {
        let dir = temp_dir("mister-magik-stream-publish-index");
        let final_path = dir.join("arcade-screenshots-320x320.mmlz4b.idx");
        let temp_path =
            hidden_timestamped_temp_path_for(&final_path, "screenshot-pack-index", "test");
        let publish = prepare_artifact_publish(
            &final_path,
            temp_path.clone(),
            ArtifactPublishLabels {
                destination: "test pack index destination",
                parent: "test pack index parent",
            },
        )
        .unwrap();
        fs::write(publish.temp_path(), b"ix").unwrap();
        let pack = indexed_pack_fixture();
        let index = pack.index.as_ref().unwrap();
        let streamed = StreamedPackDownload {
            bytes: index.bytes,
            sha256: index.sha256.clone(),
        };
        let (tx, rx) = mpsc::channel();

        install_streamed_index_silent(&publish, &streamed, index, &pack, 1, 1, &tx).unwrap();

        assert_eq!(fs::read(&final_path).unwrap(), b"ix");
        assert!(!temp_path.exists());
        assert!(
            rx.try_iter()
                .all(|message| !matches!(message, MediaWorkerMessage::Progress(_)))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cleanup_pack_publish_temps_removes_only_matching_pack_temps() {
        let dir = temp_dir("mister-magik-clean-publish-temp");
        let final_path = dir.join("arcade-screenshots-320x320.mmlz4b");
        let matching = dir.join("arcade-screenshots-320x320.mmlz4b.tmp-1");
        let hidden_matching = dir.join(".arcade-screenshots-320x320.mmlz4b.tmp-2");
        let other_pack = dir.join("neogeo-screenshots-320x320.mmlz4b.tmp-1");
        let final_file = dir.join("arcade-screenshots-320x320.mmlz4b");
        fs::write(&matching, b"partial").unwrap();
        fs::write(&hidden_matching, b"partial").unwrap();
        fs::write(&other_pack, b"partial").unwrap();
        fs::write(&final_file, b"current").unwrap();

        cleanup_pack_publish_temps(&final_path);

        assert!(!matching.exists());
        assert!(!hidden_matching.exists());
        assert!(other_pack.exists());
        assert_eq!(fs::read(&final_file).unwrap(), b"current");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_verification_leaves_existing_pack_untouched() {
        let dir = temp_dir("mister-magik-bad-stream-download");
        let final_path = dir.join("arcade-screenshots-320x320.mmlz4b");
        let temp_path = hidden_timestamped_temp_path_for(&final_path, "screenshot-pack", "test");
        let publish = prepare_artifact_publish(
            &final_path,
            temp_path.clone(),
            ArtifactPublishLabels {
                destination: "test pack destination",
                parent: "test pack parent",
            },
        )
        .unwrap();
        fs::write(publish.temp_path(), b"bad").unwrap();
        fs::write(&final_path, b"old").unwrap();
        let streamed = StreamedPackDownload {
            bytes: 3,
            sha256: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string(),
        };

        assert!(verify_streamed_download(&streamed, 3, SHA).is_err());

        assert_eq!(fs::read(&final_path).unwrap(), b"old");
        assert!(publish.temp_path().exists());
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

    #[test]
    fn write_download_state_records_pack_index() {
        let dir = temp_dir("mister-magik-write-index-state");
        let pack = indexed_pack_fixture();
        let local_path = dir.join("arcade-screenshots-320x320.mmlz4b");
        let variant = pack.variant_for_compression("none").unwrap();

        write_download_state(&dir, &pack, &local_path, variant, None).unwrap();

        let state: Value = serde_json::from_str(
            &fs::read_to_string(dir.join(".screenshot-media-state.json")).unwrap(),
        )
        .unwrap();
        let index_state = &state["systems"]["arcade"]["packs"]["320x320"]["index"];
        assert_eq!(index_state["codec"], "mmlz4b-index-v2");
        assert_eq!(index_state["bytes"], 2);
        assert_eq!(index_state["archive_bytes"], 4);
        assert_eq!(
            index_state["local_path"],
            index_path_for_pack_path(&local_path).display().to_string()
        );
        let _ = fs::remove_dir_all(dir);
    }
}
#[cfg(not(feature = "bench-tools"))]
#[test]
fn production_build_cannot_enable_media_benchmark_auto_finish() {
    assert!(!media_benchmark_auto_finish_enabled());
}
