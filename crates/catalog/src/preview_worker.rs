// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Background arcade preview image loader.

use memmap2::Mmap;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::media_identity::{
    DEFAULT_SCREENSHOT_IMAGE_SIZE, default_screenshot_asset_dir, legacy_screenshot_pack_path,
    preferred_screenshot_image_size, screenshot_media_state_path_in_root,
    screenshot_pack_id_from_legacy_filename, size_qualified_screenshot_pack_path_in_root,
    supported_screenshot_pack_ids, valid_screenshot_image_size,
};
#[cfg(test)]
use crate::preview_archive::{MAX_PREVIEW_ARCHIVE_ENTRIES, MAX_PREVIEW_ARCHIVE_RAW_BYTES};
use crate::preview_archive::{
    PreviewArchiveEntry, SIDECAR_INDEX_MAGIC, V2_PIXELS_MAGIC, read_file_entry, read_sidecar_entry,
    read_u32, validate_entry_count, validate_entry_geometry,
};
use crate::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use crate::work_coordinator;

pub const DEFAULT_PREVIEW_RADIUS: usize = 12;
pub const DEFAULT_PREVIEW_CACHE_CAP: usize = DEFAULT_PREVIEW_RADIUS * 2 + 1;
pub const TURBO_PREVIEW_DECODED_CACHE_CAP: usize = 96;
const PREFETCH_REQUEST_CAP: usize = 64;
const PREFETCH_RESULT_CAP: usize = 8;
const MISSING_ARCHIVE_TTL: Duration = Duration::from_secs(5);
const PREVIEW_ARCHIVE_METADATA_TTL: Duration = Duration::from_secs(5);
const DEFAULT_MEDIA_SIZE: &str = DEFAULT_SCREENSHOT_IMAGE_SIZE;

#[derive(Clone, Debug)]
pub struct PreviewRequest {
    pub generation: u64,
    pub title: String,
    pub preview_archive_path: String,
    pub preview_asset_key: String,
    pub requested_at: Instant,
    pub requested_at_ms: u64,
    pub priority: PreviewPriority,
}

impl PreviewRequest {
    fn preview_key(&self) -> String {
        preview_asset_cache_key(&self.preview_archive_path, &self.preview_asset_key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewPriority {
    Selected,
    Prefetch { distance: usize },
}

impl PreviewPriority {
    fn rank(self) -> usize {
        match self {
            Self::Selected => 0,
            Self::Prefetch { distance } => 1 + distance,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreviewResult {
    pub generation: u64,
    pub title: String,
    pub preview_archive_path: String,
    pub preview_asset_key: String,
    pub image: Option<PreviewPixels>,
    pub requested_at_ms: u64,
    pub completed_at_ms: u64,
    pub request_age_us: u64,
    pub read_us: u64,
    pub decode_us: u64,
    pub raw565_parse_us: u64,
    pub resize_us: u64,
    pub total_us: u64,
    pub encoded_bytes: usize,
    pub decoded_bytes: usize,
    pub source_width: u32,
    pub source_height: u32,
    pub load_source: PreviewLoadSource,
    pub storage_format: PreviewStorageFormat,
    pub resize_filter: PreviewResizeFilter,
    pub priority: PreviewPriority,
}

impl PreviewResult {
    pub fn preview_key(&self) -> String {
        preview_asset_cache_key(&self.preview_archive_path, &self.preview_asset_key)
    }
}

/// A one-slot hand-off. Publishing replaces unstarted work instead of allowing
/// a selection burst to grow an unbounded queue behind an in-flight decode.
struct LatestMailbox<T> {
    state: Mutex<LatestMailboxState<T>>,
    ready: Condvar,
}

struct LatestMailboxState<T> {
    item: Option<T>,
    closed: bool,
}

impl<T> LatestMailbox<T> {
    fn new() -> Self {
        Self {
            state: Mutex::new(LatestMailboxState {
                item: None,
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    fn publish(&self, item: T) -> bool {
        let mut state = self.state.lock().expect("preview mailbox lock");
        if state.closed {
            return false;
        }
        state.item = Some(item);
        self.ready.notify_one();
        true
    }

    fn take_blocking(&self) -> Option<T> {
        let mut state = self.state.lock().expect("preview mailbox lock");
        loop {
            if let Some(item) = state.item.take() {
                return Some(item);
            }
            if state.closed {
                return None;
            }
            state = self.ready.wait(state).expect("preview mailbox wait");
        }
    }

    fn try_take(&self) -> Option<T> {
        self.state.lock().expect("preview mailbox lock").item.take()
    }

    fn close(&self) {
        let mut state = self.state.lock().expect("preview mailbox lock");
        state.closed = true;
        state.item = None;
        self.ready.notify_all();
    }
}

pub fn preview_asset_cache_key(preview_archive_path: &str, preview_asset_key: &str) -> String {
    let archive_path = preview_archive_path.trim();
    let asset_key = preview_asset_key.trim();
    if archive_path.is_empty() {
        asset_key.to_string()
    } else {
        format!("{archive_path}|{asset_key}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewResizeFilter {
    Off,
    Nearest,
    Box,
    Lanczos,
    Hybrid,
}

impl PreviewResizeFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Nearest => "nearest",
            Self::Box => "box",
            Self::Lanczos => "lanczos",
            Self::Hybrid => "hybrid",
        }
    }

    pub fn from_label(label: &str) -> Self {
        match label.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "nearest" | "nearest-neighbor" => Self::Nearest,
            "box" | "area" | "box-area" => Self::Box,
            "lanczos" | "lanczos3" => Self::Lanczos,
            "hybrid" | "hybrid-arcade" => Self::Hybrid,
            _ => Self::Off,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewResizeSpec {
    pub filter: PreviewResizeFilter,
    pub max_w: u32,
    pub max_h: u32,
}

impl PreviewResizeSpec {
    pub fn off() -> Self {
        Self {
            filter: PreviewResizeFilter::Off,
            max_w: 0,
            max_h: 0,
        }
    }

    pub fn from_env() -> Self {
        PreviewWorkerConfig::capture_process().resize()
    }

    pub fn from_values(
        filter: Option<&str>,
        legacy_filter: Option<&str>,
        max: Option<&str>,
    ) -> Self {
        let filter = filter
            .or(legacy_filter)
            .map(PreviewResizeFilter::from_label)
            .unwrap_or(PreviewResizeFilter::Hybrid);
        if filter == PreviewResizeFilter::Off {
            return Self::off();
        }
        let (max_w, max_h) = max.and_then(parse_size).unwrap_or((320, 320));
        Self {
            filter,
            max_w: max_w.max(1),
            max_h: max_h.max(1),
        }
    }

    pub fn cache_label(self) -> String {
        format!("{}-{}x{}", self.filter.label(), self.max_w, self.max_h)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewWorkerConfig {
    resize: PreviewResizeSpec,
    storage_format: PreviewStorageFormat,
    decoded_cache_cap: usize,
    archive_mem_primary: bool,
    archive_mem_warm: bool,
    archive_background_warm: bool,
    archive_warm_skipped: bool,
    archive_paths: Vec<String>,
}

impl Default for PreviewWorkerConfig {
    fn default() -> Self {
        Self {
            resize: PreviewResizeSpec {
                filter: PreviewResizeFilter::Hybrid,
                max_w: 320,
                max_h: 320,
            },
            storage_format: PreviewStorageFormat::RawRgb565,
            decoded_cache_cap: TURBO_PREVIEW_DECODED_CACHE_CAP,
            archive_mem_primary: false,
            archive_mem_warm: false,
            archive_background_warm: false,
            archive_warm_skipped: false,
            archive_paths: Vec::new(),
        }
    }
}

impl PreviewWorkerConfig {
    pub fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        let resize = PreviewResizeSpec::from_values(
            get("MISTER_PREVIEW_RESIZE_FILTER"),
            get("MISTER_PREVIEW_RESIZE"),
            get("MISTER_PREVIEW_RESIZE_MAX"),
        );
        let decoded_cache_cap = get("MISTER_PREVIEW_DECODED_CACHE_CAP")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(TURBO_PREVIEW_DECODED_CACHE_CAP);
        let archive_mem_primary = env_value_truthy(get("MISTER_PREVIEW_ARCHIVE_MEM_PRIMARY"))
            || env_value_truthy(get("MISTER_PREVIEW_FORCE_ARCHIVE_MEM"));
        let archive_mem_warm = archive_mem_primary
            || env_value_truthy(get("MISTER_PREVIEW_ARCHIVE_MEM_WARM"))
            || env_value_truthy(get("MISTER_PREVIEW_ARCHIVE_BACKGROUND_WARM"));
        let archive_warm_skipped = env_value_truthy(get("MISTER_PREVIEW_SCROLL_SKIP_ARCHIVE_WARM"));
        let archive_background_warm = archive_mem_warm && !archive_warm_skipped;
        let archive_paths = preview_archive_paths_from_values(resize, &mut get);
        Self {
            resize,
            storage_format: PreviewStorageFormat::RawRgb565,
            decoded_cache_cap,
            archive_mem_primary,
            archive_mem_warm,
            archive_background_warm,
            archive_warm_skipped,
            archive_paths,
        }
    }

    pub fn capture_process() -> Self {
        let values = std::env::vars().collect::<HashMap<_, _>>();
        Self::capture_with(|name| values.get(name).map(String::as_str))
    }

    pub fn resize(&self) -> PreviewResizeSpec {
        self.resize
    }

    pub fn storage_format(&self) -> PreviewStorageFormat {
        self.storage_format
    }

    pub fn decoded_cache_cap(&self) -> usize {
        self.decoded_cache_cap
    }

    pub fn archive_mem_primary(&self) -> bool {
        self.archive_mem_primary
    }

    pub fn archive_mem_warm(&self) -> bool {
        self.archive_mem_warm
    }

    pub fn archive_background_warm(&self) -> bool {
        self.archive_background_warm
    }

    pub fn archive_warm_skipped(&self) -> bool {
        self.archive_warm_skipped
    }

    pub fn archive_paths(&self) -> &[String] {
        &self.archive_paths
    }
}

fn env_value_truthy(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("on") | Some("true") | Some("yes"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewStorageFormat {
    RawRgb565,
}

impl PreviewStorageFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::RawRgb565 => "raw-rgb565",
        }
    }

    fn from_env() -> Self {
        Self::RawRgb565
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewLoadSource {
    DecodedCache,
    #[default]
    ArchiveMem,
    IndexPread,
}

impl PreviewLoadSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::DecodedCache => "decoded_cache",
            Self::ArchiveMem => "archive_mem",
            Self::IndexPread => "index_pread",
        }
    }
}

#[derive(Clone, Debug)]
pub enum PreviewPixels {
    Rgb565 {
        width: u32,
        height: u32,
        stride_bytes: u32,
        words: Arc<[u16]>,
    },
}

#[derive(Clone, Debug)]
pub struct LoadedPreviewAsset {
    pub pixels: PreviewPixels,
    pub read_us: u64,
    pub decode_us: u64,
    pub raw565_parse_us: u64,
    pub resize_us: u64,
    pub total_us: u64,
    pub encoded_bytes: usize,
    pub load_source: PreviewLoadSource,
    pub storage_format: PreviewStorageFormat,
    pub resize_filter: PreviewResizeFilter,
}

/// A compressed preview pack kept resident for repeated, on-demand decoding.
///
/// The reusable decode buffer stays owned by this handle so callers retain
/// only the decoded images they explicitly keep.
pub struct ResidentPreviewArchive {
    archive: PreviewArchive,
    scratch: PreviewArchiveScratch,
    asset_keys: Vec<String>,
}

impl ResidentPreviewArchive {
    pub fn open(path: &Path) -> Result<Self, String> {
        let archive = PreviewArchive::open(path)?;
        let mut asset_keys = archive
            .entries
            .keys()
            .filter_map(|name| name.strip_suffix(".rgb565"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        asset_keys.sort_unstable();
        Ok(Self {
            archive,
            scratch: PreviewArchiveScratch::default(),
            asset_keys,
        })
    }

    pub fn asset_keys(&self) -> &[String] {
        &self.asset_keys
    }

    pub fn compressed_bytes(&self) -> usize {
        self.archive.bytes.len()
    }

    pub fn load_pixels(&mut self, asset_key: &str) -> Result<PreviewPixels, String> {
        let entry_name = format!("{}.rgb565", asset_key.trim());
        self.archive
            .load_timed(&entry_name, &mut self.scratch)?
            .map(|loaded| loaded.image)
            .ok_or_else(|| format!("preview archive entry missing {entry_name}"))
    }

    pub fn load_pixels_at(&mut self, index: usize) -> Result<PreviewPixels, String> {
        let asset_key = self
            .asset_keys
            .get(index)
            .ok_or_else(|| format!("preview archive index out of range {index}"))?
            .clone();
        self.load_pixels(&asset_key)
    }
}

impl PreviewPixels {
    pub fn width(&self) -> u32 {
        match self {
            Self::Rgb565 { width, .. } => *width,
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            Self::Rgb565 { height, .. } => *height,
        }
    }

    pub fn decoded_bytes(&self) -> usize {
        match self {
            Self::Rgb565 {
                stride_bytes,
                height,
                ..
            } => *stride_bytes as usize * *height as usize,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ImageLoadTiming {
    read_us: u64,
    decode_us: u64,
    decode_cpu_us: u64,
    raw565_parse_us: u64,
    raw565_parse_cpu_us: u64,
    resize_us: u64,
    total_us: u64,
    encoded_bytes: usize,
    source_width: u32,
    source_height: u32,
    load_source: PreviewLoadSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewArchiveIndex {
    pub path: String,
    pub codec: &'static str,
    pub entries: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewArchiveSidecarStems {
    pub archive_path: String,
    pub index_path: String,
    pub entries: Vec<String>,
}

pub struct PreviewWorker {
    selected_tx: Arc<LatestMailbox<PreviewRequest>>,
    prefetch_tx: mpsc::SyncSender<PreviewRequest>,
    selected_rx: Arc<LatestMailbox<PreviewResult>>,
    prefetch_rx: mpsc::Receiver<PreviewResult>,
    decoded_cache: SharedPreviewDecodedCache,
    trace_start: Instant,
    next_generation: Arc<AtomicU64>,
    config: PreviewWorkerConfig,
}

#[derive(Clone)]
pub struct PreviewSelectedRequestHandle {
    selected_tx: Arc<LatestMailbox<PreviewRequest>>,
    trace_start: Instant,
    next_generation: Arc<AtomicU64>,
}

impl PreviewSelectedRequestHandle {
    pub fn reserve_generation(&self) -> u64 {
        self.next_generation.fetch_add(1, Ordering::Relaxed).max(1)
    }

    pub fn publish_reserved(
        &self,
        generation: u64,
        title: String,
        preview_archive_path: String,
        preview_asset_key: String,
    ) -> bool {
        self.selected_tx.publish(PreviewRequest {
            generation,
            title,
            preview_archive_path,
            preview_asset_key,
            requested_at: Instant::now(),
            requested_at_ms: self.trace_start.elapsed().as_millis() as u64,
            priority: PreviewPriority::Selected,
        })
    }
}

impl Default for PreviewWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewWorker {
    pub fn new() -> Self {
        Self::new_with_trace_start(Instant::now())
    }

    pub fn new_with_trace_start(trace_start: Instant) -> Self {
        Self::new_with_config(trace_start, PreviewWorkerConfig::capture_process())
    }

    pub fn new_with_config(trace_start: Instant, config: PreviewWorkerConfig) -> Self {
        let selected_tx = Arc::new(LatestMailbox::new());
        let (prefetch_tx, prefetch_rx) = mpsc::sync_channel::<PreviewRequest>(PREFETCH_REQUEST_CAP);
        let selected_rx = Arc::new(LatestMailbox::new());
        let (prefetch_result_tx, prefetch_result_rx) =
            mpsc::sync_channel::<PreviewResult>(PREFETCH_RESULT_CAP);
        let decoded_cache = Arc::new(Mutex::new(PreviewDecodedCache::new(
            config.decoded_cache_cap(),
        )));
        let selected_cache = Arc::clone(&decoded_cache);
        let selected_request_rx = Arc::clone(&selected_tx);
        let selected_result_tx = Arc::clone(&selected_rx);
        let selected_config = config.clone();
        std::thread::Builder::new()
            .name("preview-selected-loader".to_string())
            .spawn(move || {
                preview_selected_thread(
                    selected_request_rx,
                    selected_result_tx,
                    selected_cache,
                    trace_start,
                    selected_config,
                )
            })
            .expect("spawn preview-selected-loader");
        let prefetch_trace_start = trace_start;
        let prefetch_cache = Arc::clone(&decoded_cache);
        let prefetch_config = config.clone();
        std::thread::Builder::new()
            .name("preview-prefetch-loader".to_string())
            .spawn(move || {
                preview_prefetch_thread(
                    prefetch_rx,
                    prefetch_result_tx,
                    prefetch_cache,
                    prefetch_trace_start,
                    prefetch_config,
                )
            })
            .expect("spawn preview-prefetch-loader");
        Self {
            selected_tx,
            prefetch_tx,
            selected_rx,
            prefetch_rx: prefetch_result_rx,
            decoded_cache,
            trace_start,
            next_generation: Arc::new(AtomicU64::new(1)),
            config,
        }
    }

    pub fn selected_request_handle(&self) -> PreviewSelectedRequestHandle {
        PreviewSelectedRequestHandle {
            selected_tx: Arc::clone(&self.selected_tx),
            trace_start: self.trace_start,
            next_generation: Arc::clone(&self.next_generation),
        }
    }

    pub fn request_selected(
        &mut self,
        title: String,
        preview_archive_path: String,
        preview_asset_key: String,
    ) -> u64 {
        let handle = self.selected_request_handle();
        let generation = handle.reserve_generation();
        let _ = handle.publish_reserved(generation, title, preview_archive_path, preview_asset_key);
        generation
    }

    pub fn request_prefetch(
        &mut self,
        title: String,
        preview_archive_path: String,
        preview_asset_key: String,
        distance: usize,
    ) -> bool {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed).max(1);
        match self.prefetch_tx.try_send(PreviewRequest {
            generation,
            title,
            preview_archive_path,
            preview_asset_key,
            requested_at: Instant::now(),
            requested_at_ms: self.trace_start.elapsed().as_millis() as u64,
            priority: PreviewPriority::Prefetch { distance },
        }) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) | Err(mpsc::TrySendError::Disconnected(_)) => false,
        }
    }

    pub fn take_latest_selected_result(&self) -> Option<PreviewResult> {
        self.selected_rx.try_take()
    }

    pub fn take_prefetch_result(&self) -> Option<PreviewResult> {
        self.prefetch_rx.try_recv().ok()
    }

    pub fn discard_ready_results(&self) {
        let _ = self.take_latest_selected_result();
        while self.take_prefetch_result().is_some() {}
    }

    pub fn load_decoded_cache_asset(
        &mut self,
        preview_archive_path: &str,
        preview_asset_key: &str,
    ) -> Option<LoadedPreviewAsset> {
        let resize = self.config.resize();
        let direct_cache_key = preview_cache_key(preview_archive_path, preview_asset_key, resize);
        let loaded = decoded_cache_get(&self.decoded_cache, &direct_cache_key).or_else(|| {
            let resolved_archive_path = resolve_preview_archive_path(preview_archive_path);
            if resolved_archive_path == preview_archive_path {
                None
            } else {
                let resolved_cache_key =
                    preview_cache_key(&resolved_archive_path, preview_asset_key, resize);
                decoded_cache_get(&self.decoded_cache, &resolved_cache_key)
            }
        })?;
        Some(LoadedPreviewAsset {
            pixels: loaded.image,
            read_us: loaded.timing.read_us,
            decode_us: loaded.timing.decode_us,
            raw565_parse_us: loaded.timing.raw565_parse_us,
            resize_us: loaded.timing.resize_us,
            total_us: loaded.timing.total_us,
            encoded_bytes: loaded.timing.encoded_bytes,
            load_source: loaded.timing.load_source,
            storage_format: self.config.storage_format(),
            resize_filter: resize.filter,
        })
    }
}

impl Drop for PreviewWorker {
    fn drop(&mut self) {
        self.selected_tx.close();
    }
}

pub fn preview_window_indices(len: usize, selected: usize, radius: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let selected = selected.min(len - 1);
    let start = selected.saturating_sub(radius);
    let end = selected.saturating_add(radius).min(len - 1);
    let mut indices: Vec<usize> = (start..=end).collect();
    indices.sort_by_key(|idx| (idx.abs_diff(selected), *idx));
    indices
}

pub fn preview_window_paths<'a, T, F>(
    items: &'a [T],
    selected: usize,
    radius: usize,
    mut path: F,
) -> Vec<&'a str>
where
    F: FnMut(&'a T) -> Option<&'a str>,
{
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for idx in preview_window_indices(items.len(), selected, radius) {
        if let Some(p) = path(&items[idx])
            && seen.insert(p)
        {
            out.push(p);
        }
    }
    out
}

type SharedPreviewDecodedCache = Arc<Mutex<PreviewDecodedCache>>;

fn preview_selected_thread(
    rx: Arc<LatestMailbox<PreviewRequest>>,
    tx: Arc<LatestMailbox<PreviewResult>>,
    decoded_cache: SharedPreviewDecodedCache,
    trace_start: Instant,
    config: PreviewWorkerConfig,
) {
    apply_runtime_thread_policy(RuntimeThreadRole::PreviewSelected);
    let mut scratch = PreviewArchiveScratch::default();
    while let Some(req) = rx.take_blocking() {
        let _lease = work_coordinator::foreground("selected-preview");
        let result = load_preview(req, &decoded_cache, &mut scratch, trace_start, &config);
        if !tx.publish(result) {
            break;
        }
    }
}

fn preview_prefetch_thread(
    rx: mpsc::Receiver<PreviewRequest>,
    tx: mpsc::SyncSender<PreviewResult>,
    decoded_cache: SharedPreviewDecodedCache,
    trace_start: Instant,
    config: PreviewWorkerConfig,
) {
    apply_runtime_thread_policy(RuntimeThreadRole::PreviewPrefetch);
    let lease = work_coordinator::background("preview-prefetch");
    let mut queue: Vec<PreviewRequest> = Vec::new();
    let mut scratch = PreviewArchiveScratch::default();
    let mut newest_generation = 0;
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(req) => {
                    newest_generation = newest_generation.max(req.generation);
                    enqueue_preview_request(&mut queue, req);
                }
                Err(_) => break,
            }
        }
        while let Ok(req) = rx.try_recv() {
            newest_generation = newest_generation.max(req.generation);
            enqueue_preview_request(&mut queue, req);
        }
        prune_stale_prefetch_requests(&mut queue, newest_generation);
        if let Some(req) = pop_next_preview_request(&mut queue) {
            let _ = lease.cooperate();
            let result = load_preview(req, &decoded_cache, &mut scratch, trace_start, &config);
            match tx.try_send(result) {
                Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => break,
            }
        }
    }
}

struct PreviewDecodedCache {
    cap: usize,
    entries: Vec<(String, LoadedPreviewPixels)>,
}

impl PreviewDecodedCache {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            entries: Vec::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<LoadedPreviewPixels> {
        let idx = self
            .entries
            .iter()
            .position(|(entry_key, _)| entry_key == key)?;
        let (key, loaded) = self.entries.remove(idx);
        let clone_t = Instant::now();
        let mut out = loaded.clone();
        out.timing.read_us = 0;
        out.timing.decode_us = 0;
        out.timing.decode_cpu_us = 0;
        out.timing.raw565_parse_us = 0;
        out.timing.raw565_parse_cpu_us = 0;
        out.timing.resize_us = 0;
        out.timing.encoded_bytes = 0;
        out.timing.total_us = clone_t.elapsed().as_micros() as u64;
        out.timing.load_source = PreviewLoadSource::DecodedCache;
        self.entries.push((key, loaded));
        Some(out)
    }

    fn insert(&mut self, key: String, loaded: &LoadedPreviewPixels) {
        if self.cap == 0 {
            return;
        }
        if let Some(idx) = self
            .entries
            .iter()
            .position(|(entry_key, _)| *entry_key == key)
        {
            self.entries.remove(idx);
        }
        self.entries.push((key, loaded.clone()));
        while self.entries.len() > self.cap {
            self.entries.remove(0);
        }
    }
}

fn pop_next_preview_request(queue: &mut Vec<PreviewRequest>) -> Option<PreviewRequest> {
    queue.sort_by_key(|req| (req.priority.rank(), req.requested_at));
    if queue.is_empty() {
        None
    } else {
        Some(queue.remove(0))
    }
}

fn enqueue_preview_request(queue: &mut Vec<PreviewRequest>, req: PreviewRequest) {
    if matches!(req.priority, PreviewPriority::Selected) {
        queue.retain(|queued| !matches!(queued.priority, PreviewPriority::Selected));
    }
    if let Some(existing) = queue
        .iter_mut()
        .find(|existing| existing.preview_key() == req.preview_key())
    {
        if req.priority.rank() <= existing.priority.rank() {
            *existing = req;
        }
    } else {
        queue.push(req);
    }
    queue.retain(|req| {
        matches!(req.priority, PreviewPriority::Selected)
            || req.priority.rank() <= TURBO_PREVIEW_DECODED_CACHE_CAP
    });
}

fn prune_stale_prefetch_requests(queue: &mut Vec<PreviewRequest>, newest_generation: u64) {
    let keep_after = newest_generation.saturating_sub(TURBO_PREVIEW_DECODED_CACHE_CAP as u64);
    queue.retain(|req| {
        let keep =
            matches!(req.priority, PreviewPriority::Selected) || req.generation >= keep_after;
        if !keep && preview_trace_enabled() {
            crate::catalog_errln!(
                "preview_trace prefetch_cancelled generation={} newest_generation={} reason=stale_generation archive_path={} asset_key={}",
                req.generation, newest_generation, req.preview_archive_path, req.preview_asset_key
            );
        }
        keep
    });
}

fn preview_cache_key(
    preview_archive_path: &str,
    preview_asset_key: &str,
    resize: PreviewResizeSpec,
) -> String {
    format!(
        "{}|{}|{}x{}",
        preview_asset_cache_key(preview_archive_path, preview_asset_key),
        resize.filter.label(),
        resize.max_w,
        resize.max_h
    )
}

fn load_preview(
    req: PreviewRequest,
    decoded_cache: &SharedPreviewDecodedCache,
    scratch: &mut PreviewArchiveScratch,
    trace_start: Instant,
    config: &PreviewWorkerConfig,
) -> PreviewResult {
    let resize = config.resize();
    let storage = config.storage_format();
    let resolved_archive_path = resolve_preview_archive_path(&req.preview_archive_path);
    let cache_key = preview_cache_key(&resolved_archive_path, &req.preview_asset_key, resize);
    let mut cache_hit = false;
    let queue_age_us = req.requested_at.elapsed().as_micros() as u64;
    let loaded_result = if let Some(loaded) = decoded_cache_get(decoded_cache, &cache_key) {
        cache_hit = true;
        Ok(loaded)
    } else {
        load_preview_pixels_with_config(
            &resolved_archive_path,
            &req.preview_asset_key,
            scratch,
            resize,
            config,
        )
        .inspect(|loaded| {
            decoded_cache_insert(decoded_cache, cache_key, loaded);
        })
    };
    match loaded_result {
        Ok(loaded) => {
            if preview_trace_enabled() {
                crate::catalog_errln!(
                    "preview_trace decoded generation={} priority={:?} queue_age_us={} cache_hit={} load_source={} format={} filter={} source={}x{} output={}x{} total_us={} read_us={} decode_us={} decode_cpu_us={} raw565_parse_us={} raw565_parse_cpu_us={} decode_plus_parse_us={} decode_plus_parse_cpu_us={} resize_us={} encoded_bytes={} decoded_bytes={} archive_path={} asset_key={}",
                    req.generation,
                    req.priority,
                    queue_age_us,
                    if cache_hit { 1 } else { 0 },
                    loaded.timing.load_source.label(),
                    storage.label(),
                    resize.filter.label(),
                    loaded.timing.source_width,
                    loaded.timing.source_height,
                    loaded.image.width(),
                    loaded.image.height(),
                    loaded.timing.total_us,
                    loaded.timing.read_us,
                    loaded.timing.decode_us,
                    loaded.timing.decode_cpu_us,
                    loaded.timing.raw565_parse_us,
                    loaded.timing.raw565_parse_cpu_us,
                    loaded.timing.decode_us + loaded.timing.raw565_parse_us,
                    loaded.timing.decode_cpu_us + loaded.timing.raw565_parse_cpu_us,
                    loaded.timing.resize_us,
                    loaded.timing.encoded_bytes,
                    loaded.image.decoded_bytes(),
                    resolved_archive_path,
                    req.preview_asset_key
                );
            }
            let decoded_bytes = loaded.image.decoded_bytes();
            PreviewResult {
                generation: req.generation,
                title: req.title,
                preview_archive_path: req.preview_archive_path,
                preview_asset_key: req.preview_asset_key,
                image: Some(loaded.image),
                requested_at_ms: req.requested_at_ms,
                completed_at_ms: trace_start.elapsed().as_millis() as u64,
                request_age_us: req.requested_at.elapsed().as_micros() as u64,
                read_us: loaded.timing.read_us,
                decode_us: loaded.timing.decode_us,
                raw565_parse_us: loaded.timing.raw565_parse_us,
                resize_us: loaded.timing.resize_us,
                total_us: loaded.timing.total_us,
                encoded_bytes: loaded.timing.encoded_bytes,
                decoded_bytes,
                source_width: loaded.timing.source_width,
                source_height: loaded.timing.source_height,
                load_source: loaded.timing.load_source,
                storage_format: storage,
                resize_filter: resize.filter,
                priority: req.priority,
            }
        }
        Err(e) => {
            if preview_trace_enabled() {
                crate::catalog_errln!(
                    "preview_trace decode_failed generation={} queue_age_us={} age_us={} archive_path={} asset_key={} error={}",
                    req.generation,
                    queue_age_us,
                    req.requested_at.elapsed().as_micros(),
                    resolved_archive_path,
                    req.preview_asset_key,
                    e
                );
            }
            PreviewResult {
                generation: req.generation,
                title: req.title,
                preview_archive_path: req.preview_archive_path,
                preview_asset_key: req.preview_asset_key,
                image: None,
                requested_at_ms: req.requested_at_ms,
                completed_at_ms: trace_start.elapsed().as_millis() as u64,
                request_age_us: req.requested_at.elapsed().as_micros() as u64,
                read_us: 0,
                decode_us: 0,
                raw565_parse_us: 0,
                resize_us: 0,
                total_us: 0,
                encoded_bytes: 0,
                decoded_bytes: 0,
                source_width: 0,
                source_height: 0,
                load_source: PreviewLoadSource::ArchiveMem,
                storage_format: storage,
                resize_filter: resize.filter,
                priority: req.priority,
            }
        }
    }
}

fn decoded_cache_get(
    decoded_cache: &SharedPreviewDecodedCache,
    key: &str,
) -> Option<LoadedPreviewPixels> {
    decoded_cache.lock().ok()?.get(key)
}

fn decoded_cache_insert(
    decoded_cache: &SharedPreviewDecodedCache,
    key: String,
    loaded: &LoadedPreviewPixels,
) {
    if let Ok(mut decoded_cache) = decoded_cache.lock() {
        decoded_cache.insert(key, loaded);
    }
}

#[derive(Clone, Debug)]
struct LoadedPreviewPixels {
    timing: ImageLoadTiming,
    image: PreviewPixels,
}

fn load_preview_pixels(
    preview_archive_path: &str,
    preview_asset_key: &str,
    scratch: &mut PreviewArchiveScratch,
    resize: PreviewResizeSpec,
) -> Result<LoadedPreviewPixels, String> {
    let config = PreviewWorkerConfig::capture_process();
    load_preview_pixels_with_config(
        preview_archive_path,
        preview_asset_key,
        scratch,
        resize,
        &config,
    )
}

fn load_preview_pixels_with_config(
    preview_archive_path: &str,
    preview_asset_key: &str,
    scratch: &mut PreviewArchiveScratch,
    resize: PreviewResizeSpec,
    config: &PreviewWorkerConfig,
) -> Result<LoadedPreviewPixels, String> {
    let _ = resize;
    load_raw565_preview_asset_timed(
        preview_archive_path,
        preview_asset_key,
        scratch,
        config.archive_mem_primary(),
        config.archive_background_warm(),
    )
}

pub fn load_preview_asset_pixels(
    preview_archive_path: &str,
    preview_asset_key: &str,
) -> Result<PreviewPixels, String> {
    let mut scratch = PreviewArchiveScratch::default();
    let resolved_archive_path = resolve_preview_archive_path(preview_archive_path);
    let config = PreviewWorkerConfig::capture_process();
    load_raw565_preview_asset_timed(
        &resolved_archive_path,
        preview_asset_key,
        &mut scratch,
        config.archive_mem_primary(),
        config.archive_background_warm(),
    )
    .map(|loaded| loaded.image)
}

pub fn load_preview_asset_pixels_timed(
    preview_archive_path: &str,
    preview_asset_key: &str,
) -> Result<LoadedPreviewAsset, String> {
    let mut scratch = PreviewArchiveScratch::default();
    load_preview_asset_pixels_timed_with_scratch(
        preview_archive_path,
        preview_asset_key,
        &mut scratch,
    )
}

fn load_preview_asset_pixels_timed_with_scratch(
    preview_archive_path: &str,
    preview_asset_key: &str,
    scratch: &mut PreviewArchiveScratch,
) -> Result<LoadedPreviewAsset, String> {
    let resize = PreviewResizeSpec::from_env();
    let storage_format = PreviewStorageFormat::from_env();
    let resolved_archive_path = resolve_preview_archive_path(preview_archive_path);
    load_preview_pixels(&resolved_archive_path, preview_asset_key, scratch, resize).map(|loaded| {
        LoadedPreviewAsset {
            pixels: loaded.image,
            read_us: loaded.timing.read_us,
            decode_us: loaded.timing.decode_us,
            raw565_parse_us: loaded.timing.raw565_parse_us,
            resize_us: loaded.timing.resize_us,
            total_us: loaded.timing.total_us,
            encoded_bytes: loaded.timing.encoded_bytes,
            load_source: loaded.timing.load_source,
            storage_format,
            resize_filter: resize.filter,
        }
    })
}

fn load_raw565_preview_asset_timed(
    preview_archive_path: &str,
    preview_asset_key: &str,
    scratch: &mut PreviewArchiveScratch,
    archive_mem_primary: bool,
    archive_background_warm: bool,
) -> Result<LoadedPreviewPixels, String> {
    let archive_path = preview_archive_path.trim();
    let asset_key = preview_asset_key.trim();
    if archive_path.is_empty() || asset_key.is_empty() {
        return Err("preview asset missing archive path or key".to_string());
    }
    let entry_name = format!("{asset_key}.rgb565");
    if archive_mem_primary
        && let Some(loaded) =
            load_raw565_preview_asset_from_archive_mem(archive_path, &entry_name, scratch)?
    {
        return Ok(loaded);
    }
    if let Some(loaded) =
        try_load_raw565_preview_asset_from_index(Path::new(archive_path), &entry_name, scratch)
    {
        match loaded {
            PreviewIndexLoad::Loaded(loaded) => {
                if archive_background_warm {
                    start_background_preview_archive_load(archive_path.to_string());
                }
                return Ok(loaded);
            }
            PreviewIndexLoad::MissingEntry => {
                return Err(format!(
                    "preview asset {entry_name} missing from index for archive {archive_path}"
                ));
            }
            PreviewIndexLoad::NoSidecar => {}
        }
    }
    if let Some(loaded) =
        load_raw565_preview_asset_from_archive_mem(archive_path, &entry_name, scratch)?
    {
        return Ok(loaded);
    }
    Err(format!(
        "preview asset {entry_name} missing from archive {archive_path}"
    ))
}

fn load_raw565_preview_asset_from_archive_mem(
    archive_path: &str,
    entry_name: &str,
    scratch: &mut PreviewArchiveScratch,
) -> Result<Option<LoadedPreviewPixels>, String> {
    let paths = [archive_path.to_string()];
    if let Some(archives) = cached_preview_archives_for_paths(&paths) {
        for archive in archives.iter() {
            if let Some(loaded) = archive.load_timed(entry_name, scratch)? {
                return Ok(Some(loaded));
            }
        }
    }
    let Some(archives) = preview_archives_for_paths(vec![archive_path.to_string()])? else {
        return Err(format!("preview archive not configured {archive_path}"));
    };
    for archive in archives.iter() {
        if let Some(loaded) = archive.load_timed(entry_name, scratch)? {
            return Ok(Some(loaded));
        }
    }
    Ok(None)
}

#[derive(Clone, Debug)]
struct PreviewArchiveSidecarIndex {
    archive_sha256: String,
    entries: HashMap<String, PreviewArchiveEntry>,
}

#[derive(Clone, Debug)]
struct CachedPreviewArchiveSidecarIndex {
    archive_fingerprint: PreviewArchiveFingerprint,
    index_fingerprint: PreviewArchiveFingerprint,
    index: Arc<PreviewArchiveSidecarIndex>,
    checked_at: Instant,
}

#[derive(Clone)]
struct PreviewArchiveSidecarLookup {
    archive_fingerprint: PreviewArchiveFingerprint,
    index: Arc<PreviewArchiveSidecarIndex>,
}

struct PreviewArchive {
    bytes: Mmap,
    entries: HashMap<String, PreviewArchiveEntry>,
}

#[derive(Default)]
struct PreviewArchiveScratch {
    index_pread_file: Option<CachedIndexPreadFile>,
}

struct CachedIndexPreadFile {
    fingerprint: PreviewArchiveFingerprint,
    file: File,
}

impl PreviewArchiveScratch {
    fn index_pread_file(
        &mut self,
        archive_path: &Path,
        fingerprint: &PreviewArchiveFingerprint,
    ) -> Result<&File, String> {
        let reuse = self
            .index_pread_file
            .as_ref()
            .is_some_and(|cached| cached.fingerprint == *fingerprint);
        if !reuse {
            let file = open_preview_archive_file_for_index_pread(archive_path)
                .map_err(|e| format!("open preview archive {}: {e}", archive_path.display()))?;
            self.index_pread_file = Some(CachedIndexPreadFile {
                fingerprint: fingerprint.clone(),
                file,
            });
        }
        self.index_pread_file
            .as_ref()
            .map(|cached| &cached.file)
            .ok_or_else(|| {
                format!(
                    "open preview archive {}: cache empty",
                    archive_path.display()
                )
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewArchiveFingerprint {
    path: String,
    size: u64,
    mtime_secs: i64,
}

struct CachedPreviewArchives {
    fingerprints: Vec<PreviewArchiveFingerprint>,
    archives: Arc<Vec<PreviewArchive>>,
    checked_at: Instant,
}

struct MissingPreviewArchive {
    path: String,
    failed_at: Instant,
    error: String,
}

/// Open and cache configured preview archives before latency-sensitive work starts.
pub fn warm_preview_archives_from_env() -> Result<bool, String> {
    warm_preview_archives_with_config(&PreviewWorkerConfig::capture_process())
}

pub fn warm_preview_archives_with_config(config: &PreviewWorkerConfig) -> Result<bool, String> {
    if config.archive_mem_warm() {
        return preview_archives_for_paths(config.archive_paths().to_vec())
            .map(|archives| archives.is_some());
    }
    warm_preview_sidecar_indexes(config.archive_paths())
}

fn warm_preview_sidecar_indexes(paths: &[String]) -> Result<bool, String> {
    let mut warmed = false;
    for path in paths {
        let resolved_path = resolve_preview_archive_path(path);
        match preview_archive_sidecar_lookup(Path::new(&resolved_path)) {
            Ok(Some(_)) => warmed = true,
            Ok(None) => {}
            Err(error) => {
                if preview_trace_enabled() {
                    crate::catalog_errln!(
                        "preview_trace sidecar_warm_skipped archive_path={} error={}",
                        resolved_path,
                        error
                    );
                }
            }
        }
    }
    Ok(warmed)
}

fn preview_archive_paths_from_values<'a>(
    resize: PreviewResizeSpec,
    mut get: impl FnMut(&str) -> Option<&'a str>,
) -> Vec<String> {
    let root = get("MISTER_PREVIEW_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_screenshot_asset_dir);
    let auto_enabled = !matches!(
        get("MISTER_PREVIEW_ARCHIVE_AUTO"),
        Some("0") | Some("off") | Some("false") | Some("no")
    );
    let mut paths = Vec::new();
    if let Some(value) = get("MISTER_PREVIEW_ARCHIVES") {
        paths.extend(
            value
                .split(':')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_owned),
        );
    }
    if let Some(path) = get("MISTER_PREVIEW_ARCHIVE").filter(|path| !path.is_empty()) {
        paths.push(path.to_owned());
    } else if auto_enabled && let Some(path) = auto_preview_archive_path_in_root(&root, resize) {
        paths.push(path);
    }
    if let Some(path) = get("MISTER_NEOGEO_PREVIEW_ARCHIVE").filter(|path| !path.is_empty()) {
        paths.push(path.to_owned());
    } else if auto_enabled && let Some(path) = auto_archive_path_for_system(&root, "neogeo") {
        paths.push(path);
    }
    for (system, name) in [
        ("nes", "MISTER_NES_PREVIEW_ARCHIVE"),
        ("snes", "MISTER_SNES_PREVIEW_ARCHIVE"),
        ("n64", "MISTER_N64_PREVIEW_ARCHIVE"),
        ("sms", "MISTER_SMS_PREVIEW_ARCHIVE"),
        ("megadrive", "MISTER_MEGADRIVE_PREVIEW_ARCHIVE"),
        ("saturn", "MISTER_SATURN_PREVIEW_ARCHIVE"),
        ("amiga", "MISTER_AMIGA_PREVIEW_ARCHIVE"),
    ] {
        if let Some(path) = get(name).filter(|path| !path.is_empty()) {
            paths.push(path.to_owned());
        } else if auto_enabled && let Some(path) = auto_archive_path_for_system(&root, system) {
            paths.push(path);
        }
    }
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths
}

pub fn invalidate_preview_archive_metadata_cache(reason: &str) {
    if let Ok(mut cached) = preview_archive_cache().lock() {
        *cached = None;
    }
    if let Ok(mut cached) = preview_archive_sidecar_index_cache().lock() {
        cached.clear();
    }
    if let Ok(mut cached) = resolved_preview_archive_path_cache().lock() {
        cached.clear();
    }
    if preview_trace_enabled() {
        crate::catalog_errln!(
            "preview_trace metadata_cache event=forced_invalidation reason={reason}"
        );
    }
}

fn preview_archives_for_paths(
    paths: Vec<String>,
) -> Result<Option<Arc<Vec<PreviewArchive>>>, String> {
    preview_archives_for_paths_with_cache(
        paths,
        preview_archive_cache(),
        missing_preview_archive_cache(),
    )
}

fn preview_archives_for_paths_with_cache(
    paths: Vec<String>,
    cache: &Mutex<Option<CachedPreviewArchives>>,
    missing_cache: &Mutex<Vec<MissingPreviewArchive>>,
) -> Result<Option<Arc<Vec<PreviewArchive>>>, String> {
    if paths.is_empty() {
        if let Ok(mut cached) = cache.lock() {
            *cached = None;
        }
        return Ok(None);
    }

    if let Some(error) = cached_missing_preview_archive_error(missing_cache, &paths) {
        return Err(error);
    }

    if let Some(archives) = cached_preview_archives_for_paths_in(cache, &paths) {
        return Ok(Some(archives));
    }

    let fingerprints = match preview_archive_fingerprints_for_paths(paths) {
        Ok(fingerprints) => fingerprints,
        Err(e) => {
            cache_missing_preview_archive_error(missing_cache, &e);
            return Err(e);
        }
    };
    if let Ok(cached) = cache.lock()
        && let Some(cached) = cached.as_ref()
        && cached.fingerprints == fingerprints
    {
        return Ok(Some(Arc::clone(&cached.archives)));
    }

    let mut archives = Vec::with_capacity(fingerprints.len());
    for fingerprint in &fingerprints {
        match PreviewArchive::open(Path::new(&fingerprint.path)) {
            Ok(archive) => archives.push(archive),
            Err(e) => {
                cache_missing_preview_archive_error(missing_cache, &e);
                return Err(e);
            }
        }
    }
    let archives = Arc::new(archives);
    if let Ok(mut cached) = cache.lock() {
        *cached = Some(CachedPreviewArchives {
            fingerprints,
            archives: Arc::clone(&archives),
            checked_at: Instant::now(),
        });
    }
    Ok(Some(archives))
}

fn preview_archive_cache() -> &'static Mutex<Option<CachedPreviewArchives>> {
    static ARCHIVES: OnceLock<Mutex<Option<CachedPreviewArchives>>> = OnceLock::new();
    ARCHIVES.get_or_init(|| Mutex::new(None))
}

fn missing_preview_archive_cache() -> &'static Mutex<Vec<MissingPreviewArchive>> {
    static MISSING: OnceLock<Mutex<Vec<MissingPreviewArchive>>> = OnceLock::new();
    MISSING.get_or_init(|| Mutex::new(Vec::new()))
}

fn cached_preview_archives_for_paths(paths: &[String]) -> Option<Arc<Vec<PreviewArchive>>> {
    cached_preview_archives_for_paths_in(preview_archive_cache(), paths)
}

fn cached_preview_archives_for_paths_in(
    cache: &Mutex<Option<CachedPreviewArchives>>,
    paths: &[String],
) -> Option<Arc<Vec<PreviewArchive>>> {
    let mut cached = cache.lock().ok()?;
    let cached = cached.as_mut()?;
    if cached.fingerprints.len() != paths.len()
        || cached
            .fingerprints
            .iter()
            .zip(paths.iter())
            .any(|(fingerprint, path)| fingerprint.path != *path)
    {
        return None;
    }
    let now = Instant::now();
    if now.duration_since(cached.checked_at) < PREVIEW_ARCHIVE_METADATA_TTL {
        if preview_trace_enabled() {
            crate::catalog_errln!(
                "preview_trace metadata_cache event=hit scope=archive paths={} ttl_ms={}",
                paths.len(),
                PREVIEW_ARCHIVE_METADATA_TTL.as_millis()
            );
        }
        return Some(Arc::clone(&cached.archives));
    }
    let fingerprints = preview_archive_fingerprints_for_paths(paths.to_vec()).ok()?;
    if cached.fingerprints == fingerprints {
        cached.checked_at = now;
        if preview_trace_enabled() {
            crate::catalog_errln!(
                "preview_trace metadata_cache event=miss scope=archive reason=ttl_revalidated paths={}",
                paths.len()
            );
        }
        Some(Arc::clone(&cached.archives))
    } else {
        if preview_trace_enabled() {
            crate::catalog_errln!(
                "preview_trace metadata_cache event=forced_invalidation scope=archive reason=fingerprint_changed paths={}",
                paths.len()
            );
        }
        None
    }
}

fn cached_missing_preview_archive_error(
    cache: &Mutex<Vec<MissingPreviewArchive>>,
    paths: &[String],
) -> Option<String> {
    let now = Instant::now();
    let mut cache = cache.lock().ok()?;
    cache.retain(|missing| now.duration_since(missing.failed_at) < MISSING_ARCHIVE_TTL);
    paths.iter().find_map(|path| {
        cache
            .iter()
            .find(|missing| missing.path == *path)
            .map(|missing| missing.error.clone())
    })
}

fn cache_missing_preview_archive_error(cache: &Mutex<Vec<MissingPreviewArchive>>, error: &str) {
    let Some(path) = missing_preview_archive_path_from_error(error) else {
        return;
    };
    if let Ok(mut cache) = cache.lock() {
        let now = Instant::now();
        if let Some(existing) = cache.iter_mut().find(|missing| missing.path == path) {
            existing.failed_at = now;
            existing.error = error.to_string();
        } else {
            cache.push(MissingPreviewArchive {
                path,
                failed_at: now,
                error: error.to_string(),
            });
        }
    }
}

fn missing_preview_archive_path_from_error(error: &str) -> Option<String> {
    let marker = "preview archive ";
    let start = error.find(marker)? + marker.len();
    let tail = &error[start..];
    let end = tail.find(':')?;
    let path = tail[..end].trim();
    (!path.is_empty()).then(|| path.to_string())
}

pub fn preview_archive_paths_from_env() -> Vec<String> {
    let mut paths = Vec::new();
    append_preview_archive_paths_env(&mut paths);
    if let Some(path) = preview_archive_path_from_env() {
        paths.push(path);
    }
    if let Some(path) = auto_preview_archive_path() {
        paths.push(path);
    }
    if let Some(path) = neogeo_preview_archive_path_from_env().or_else(auto_neogeo_archive_path) {
        paths.push(path);
    }
    paths.extend(console_preview_archive_paths_from_env());
    paths.extend(auto_console_archive_paths());
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

pub fn preview_archive_paths_for_catalog_projection() -> Vec<String> {
    let mut paths = Vec::new();
    append_preview_archive_paths_env(&mut paths);
    if let Some(path) = preview_archive_path_from_env() {
        paths.push(path);
    } else if let Some(path) = default_preview_archive_path() {
        paths.push(path);
    }
    paths.push(neogeo_preview_archive_path_from_env().unwrap_or_else(default_neogeo_archive_path));
    paths.extend(console_preview_archive_paths_from_env());
    paths.extend(default_console_archive_paths());
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn append_preview_archive_paths_env(paths: &mut Vec<String>) {
    if let Ok(value) = std::env::var("MISTER_PREVIEW_ARCHIVES") {
        for path in value
            .split(':')
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            paths.push(path.to_string());
        }
    }
}

fn preview_archive_path_from_env() -> Option<String> {
    match std::env::var("MISTER_PREVIEW_ARCHIVE") {
        Ok(path) if !path.is_empty() => Some(path),
        _ => None,
    }
}

pub fn preview_archive_entry_stems_from_env() -> Result<Option<HashSet<String>>, String> {
    let indexes = preview_archive_indexes_from_env()?;
    if indexes.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        indexes
            .into_iter()
            .flat_map(|index| index.entries.into_iter())
            .collect(),
    ))
}

pub fn preview_archive_index_from_env() -> Result<Option<PreviewArchiveIndex>, String> {
    let Some(path) = preview_archive_paths_from_env().into_iter().next() else {
        return Ok(None);
    };
    preview_archive_index(Path::new(&path)).map(Some)
}

pub fn preview_archive_indexes_from_env() -> Result<Vec<PreviewArchiveIndex>, String> {
    preview_archive_paths_from_env()
        .into_iter()
        .map(|path| preview_archive_index(Path::new(&path)))
        .collect()
}

pub fn preview_archive_fingerprint_from_env() -> Result<Option<(String, u64, i64)>, String> {
    Ok(preview_archive_fingerprints_from_env()?.into_iter().next())
}

pub fn preview_archive_fingerprints_from_env() -> Result<Vec<(String, u64, i64)>, String> {
    Ok(
        preview_archive_fingerprints_for_paths(preview_archive_paths_from_env())?
            .into_iter()
            .map(|fingerprint| (fingerprint.path, fingerprint.size, fingerprint.mtime_secs))
            .collect(),
    )
}

fn preview_archive_fingerprints_for_paths(
    paths: Vec<String>,
) -> Result<Vec<PreviewArchiveFingerprint>, String> {
    paths
        .into_iter()
        .map(|path| {
            let meta = preview_archive_metadata(&path)
                .map_err(|e| format!("metadata preview archive {path}: {e}"))?;
            let mtime_secs = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
                .unwrap_or(0);
            Ok(PreviewArchiveFingerprint {
                path,
                size: meta.len(),
                mtime_secs,
            })
        })
        .collect()
}

fn preview_archive_metadata(path: &str) -> std::io::Result<std::fs::Metadata> {
    #[cfg(test)]
    {
        let calls = PREVIEW_ARCHIVE_METADATA_CALLS.get_or_init(|| Mutex::new(HashMap::new()));
        if let Ok(mut calls) = calls.lock() {
            *calls.entry(path.to_string()).or_insert(0) += 1;
        }
    }
    std::fs::metadata(path)
}

#[cfg(test)]
static PREVIEW_ARCHIVE_METADATA_CALLS: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();

#[cfg(test)]
fn preview_archive_metadata_calls(path: &str) -> usize {
    PREVIEW_ARCHIVE_METADATA_CALLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|calls| calls.get(path).copied())
        .unwrap_or(0)
}

#[cfg(test)]
static INDEX_PREAD_ARCHIVE_OPEN_CALLS: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();

#[cfg(test)]
fn index_pread_archive_open_calls(path: &str) -> usize {
    INDEX_PREAD_ARCHIVE_OPEN_CALLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|calls| calls.get(path).copied())
        .unwrap_or(0)
}

#[cfg(test)]
fn expire_preview_archive_metadata_cache_for_tests() {
    let expired_at = Instant::now() - PREVIEW_ARCHIVE_METADATA_TTL - Duration::from_millis(1);
    if let Ok(mut cached) = preview_archive_cache().lock()
        && let Some(cached) = cached.as_mut()
    {
        cached.checked_at = expired_at;
    }
    if let Ok(mut cached) = preview_archive_sidecar_index_cache().lock() {
        for cached in cached.values_mut() {
            cached.checked_at = expired_at;
        }
    }
}

#[cfg(test)]
fn preview_archive_entry_stems(path: &Path) -> Result<HashSet<String>, String> {
    Ok(preview_archive_index(path)?.entries.into_iter().collect())
}

pub fn preview_archive_index(path: &Path) -> Result<PreviewArchiveIndex, String> {
    let mut file =
        File::open(path).map_err(|e| format!("open preview archive {}: {e}", path.display()))?;
    let archive_bytes = file
        .metadata()
        .map_err(|e| format!("metadata preview archive {}: {e}", path.display()))?
        .len();
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)
        .map_err(|e| format!("read preview archive magic {}: {e}", path.display()))?;
    if &magic != V2_PIXELS_MAGIC {
        return Err(format!("{}: bad preview archive v2 magic", path.display()));
    }
    let count = read_u32(&mut file)? as usize;
    validate_entry_count(count, "preview archive index")?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let (name, _) = read_file_entry(&mut file, "preview archive index", archive_bytes)?;
        if let Some(stem) = Path::new(&name).file_stem().and_then(|s| s.to_str()) {
            entries.push(stem.to_ascii_lowercase());
        }
    }
    entries.sort();
    entries.dedup();
    Ok(PreviewArchiveIndex {
        path: path.display().to_string(),
        codec: "lz4-block",
        entries,
    })
}

pub fn resolved_preview_archive_path(preview_archive_path: &str) -> String {
    resolve_preview_archive_path(preview_archive_path)
}

pub fn preview_archive_path_for_system(system_id: &str) -> String {
    let root = default_preview_archive_root();
    legacy_archive_path_for_system(&root, system_id)
}

pub fn preview_archive_sidecar_entry_stems(
    path: &Path,
) -> Result<Option<PreviewArchiveSidecarStems>, String> {
    let Some(index) = preview_archive_sidecar_index(path)? else {
        return Ok(None);
    };
    let mut entries = index
        .entries
        .keys()
        .filter_map(|name| Path::new(name).file_stem().and_then(|stem| stem.to_str()))
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    entries.sort();
    entries.dedup();
    Ok(Some(PreviewArchiveSidecarStems {
        archive_path: path.display().to_string(),
        index_path: preview_archive_sidecar_path(path).display().to_string(),
        entries,
    }))
}

pub fn preview_archive_sidecar_path_for_archive(archive_path: &Path) -> PathBuf {
    preview_archive_sidecar_path(archive_path)
}

fn auto_preview_archive_path() -> Option<String> {
    if preview_archive_auto_disabled() {
        return None;
    }
    let resize = PreviewResizeSpec::from_env();
    let root = default_preview_archive_root();
    auto_preview_archive_path_in_root(&root, resize)
}

fn default_preview_archive_path() -> Option<String> {
    if preview_archive_auto_disabled() {
        return None;
    }
    let resize = PreviewResizeSpec::from_env();
    let root = default_preview_archive_root();
    Some(default_preview_archive_path_in_root(&root, resize))
}

fn preview_archive_auto_disabled() -> bool {
    matches!(
        std::env::var("MISTER_PREVIEW_ARCHIVE_AUTO").as_deref(),
        Ok("0") | Ok("off") | Ok("false") | Ok("no")
    )
}

fn default_preview_archive_root() -> PathBuf {
    std::env::var("MISTER_PREVIEW_CACHE_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(default_screenshot_asset_dir)
}

fn auto_preview_archive_path_in_root(root: &Path, resize: PreviewResizeSpec) -> Option<String> {
    let archive = auto_archive_path_for_system(root, "arcade")
        .unwrap_or_else(|| default_preview_archive_path_in_root(root, resize));
    Path::new(&archive).exists().then_some(archive)
}

fn default_preview_archive_path_in_root(root: &Path, _resize: PreviewResizeSpec) -> String {
    legacy_archive_path_for_system(root, "arcade")
}

fn neogeo_preview_archive_path_from_env() -> Option<String> {
    match std::env::var("MISTER_NEOGEO_PREVIEW_ARCHIVE") {
        Ok(path) if !path.is_empty() => Some(path),
        _ => None,
    }
}

fn auto_neogeo_archive_path() -> Option<String> {
    let root = default_preview_archive_root();
    auto_archive_path_for_system(&root, "neogeo")
}

fn default_neogeo_archive_path() -> String {
    legacy_archive_path_for_system(&default_screenshot_asset_dir(), "neogeo")
}

fn console_preview_archive_paths_from_env() -> Vec<String> {
    [
        "MISTER_NES_PREVIEW_ARCHIVE",
        "MISTER_SNES_PREVIEW_ARCHIVE",
        "MISTER_N64_PREVIEW_ARCHIVE",
        "MISTER_SMS_PREVIEW_ARCHIVE",
        "MISTER_MEGADRIVE_PREVIEW_ARCHIVE",
        "MISTER_SATURN_PREVIEW_ARCHIVE",
        "MISTER_AMIGA_PREVIEW_ARCHIVE",
    ]
    .into_iter()
    .filter_map(|name| match std::env::var(name) {
        Ok(path) if !path.is_empty() => Some(path),
        _ => None,
    })
    .collect()
}

fn auto_console_archive_paths() -> Vec<String> {
    supported_screenshot_pack_ids()
        .filter(|system| !matches!(*system, "arcade" | "neogeo"))
        .filter_map(|system| auto_archive_path_for_system(&default_preview_archive_root(), system))
        .collect()
}

fn default_console_archive_paths() -> Vec<String> {
    supported_screenshot_pack_ids()
        .filter(|system| !matches!(*system, "arcade" | "neogeo"))
        .map(|system| legacy_archive_path_for_system(&default_screenshot_asset_dir(), system))
        .collect()
}

fn resolve_preview_archive_path(preview_archive_path: &str) -> String {
    let path = Path::new(preview_archive_path.trim());
    let Some(system) = system_from_legacy_archive_path(path) else {
        return preview_archive_path.to_string();
    };
    if let Ok(cache) = resolved_preview_archive_path_cache().lock()
        && let Some(resolved) = cache.get(preview_archive_path)
    {
        return resolved.clone();
    }
    let root = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(default_screenshot_asset_dir);
    let resolved = preferred_archive_path_for_system(&root, system)
        .unwrap_or_else(|| preview_archive_path.to_string());
    if let Ok(mut cache) = resolved_preview_archive_path_cache().lock() {
        cache.insert(preview_archive_path.to_string(), resolved.clone());
    }
    resolved
}

fn resolved_preview_archive_path_cache() -> &'static Mutex<HashMap<String, String>> {
    static PATHS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    PATHS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn auto_archive_path_for_system(root: &Path, system: &str) -> Option<String> {
    preferred_archive_path_for_system(root, system).or_else(|| {
        let legacy = legacy_archive_path_for_system(root, system);
        Path::new(&legacy).exists().then_some(legacy)
    })
}

fn preferred_archive_path_for_system(root: &Path, system: &str) -> Option<String> {
    let preferred_size = preferred_media_size(system);
    state_archive_path_for_system(root, system, &preferred_size)
        .filter(|path| Path::new(path).exists())
        .or_else(|| {
            let sized = size_qualified_archive_path_for_system(root, system, &preferred_size)?;
            Path::new(&sized).exists().then_some(sized)
        })
        .or_else(|| {
            if preferred_size == DEFAULT_MEDIA_SIZE {
                return None;
            }
            let sized = size_qualified_archive_path_for_system(root, system, DEFAULT_MEDIA_SIZE)?;
            Path::new(&sized).exists().then_some(sized)
        })
}

fn preferred_media_size(system: &str) -> String {
    std::env::var("MISTER_MEDIA_SIZE")
        .ok()
        .filter(|size| valid_screenshot_image_size(size))
        .unwrap_or_else(|| preferred_screenshot_image_size(system).to_string())
}

fn state_archive_path_for_system(
    root: &Path,
    system: &str,
    preferred_size: &str,
) -> Option<String> {
    let state_path = screenshot_media_state_path_in_root(root);
    let text = std::fs::read_to_string(state_path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let system_state = value
        .get("systems")
        .and_then(|systems| systems.get(system))
        .or_else(|| value.get("packs").and_then(|packs| packs.get(system)))?;
    direct_state_local_path(system_state).or_else(|| {
        let size = system_state
            .get("preferred_size")
            .and_then(serde_json::Value::as_str)
            .filter(|size| valid_screenshot_image_size(size))
            .unwrap_or(preferred_size);
        system_state
            .get("packs")
            .and_then(|packs| packs.get(size))
            .and_then(direct_state_local_path)
    })
}

fn direct_state_local_path(value: &serde_json::Value) -> Option<String> {
    value
        .get("local_path")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
}

fn system_from_legacy_archive_path(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    screenshot_pack_id_from_legacy_filename(name).map(|system| system.as_str())
}

fn legacy_archive_path_for_system(root: &Path, system: &str) -> String {
    legacy_screenshot_pack_path(root, system)
        .unwrap_or_else(|_| root.join(format!("{system}-screenshots.mmlz4b")))
        .display()
        .to_string()
}

fn size_qualified_archive_path_for_system(
    root: &Path,
    system: &str,
    image_size: &str,
) -> Option<String> {
    size_qualified_screenshot_pack_path_in_root(root, system, image_size)
        .ok()
        .map(|path| path.display().to_string())
}

enum PreviewIndexLoad {
    Loaded(LoadedPreviewPixels),
    NoSidecar,
    MissingEntry,
}

fn try_load_raw565_preview_asset_from_index(
    archive_path: &Path,
    entry_name: &str,
    scratch: &mut PreviewArchiveScratch,
) -> Option<PreviewIndexLoad> {
    match load_raw565_preview_asset_from_index(archive_path, entry_name, scratch) {
        Ok(loaded) => Some(loaded),
        Err(error) => {
            if preview_trace_enabled() {
                crate::catalog_errln!(
                    "preview_trace index_pread_failed archive_path={} asset_key={} error={}",
                    archive_path.display(),
                    entry_name,
                    error
                );
            }
            None
        }
    }
}

fn load_raw565_preview_asset_from_index(
    archive_path: &Path,
    entry_name: &str,
    scratch: &mut PreviewArchiveScratch,
) -> Result<PreviewIndexLoad, String> {
    let Some(sidecar) = preview_archive_sidecar_lookup(archive_path)? else {
        return Ok(PreviewIndexLoad::NoSidecar);
    };
    let key = entry_name.to_ascii_lowercase();
    let Some(entry) = sidecar.index.entries.get(&key).copied() else {
        return Ok(PreviewIndexLoad::MissingEntry);
    };
    let total_t = Instant::now();
    let read_t = Instant::now();
    let mut payload = vec![0u8; entry.compressed_len];
    let file = scratch.index_pread_file(archive_path, &sidecar.archive_fingerprint)?;
    read_exact_at(file, &mut payload, entry.offset)
        .map_err(|e| format!("pread preview archive {}: {e}", archive_path.display()))?;
    let read_us = read_t.elapsed().as_micros() as u64;

    let decode_t = Instant::now();
    let decode_cpu_t = thread_cpu_us();
    let words = decode_preview_archive_entry_words(&payload, entry)
        .map_err(|e| format!("preview archive index decode {entry_name}: {e}"))?;
    let decode_us = decode_t.elapsed().as_micros() as u64;
    let decode_cpu_us = elapsed_thread_cpu_us(decode_cpu_t);
    let parse_t = Instant::now();
    let parse_cpu_t = thread_cpu_us();
    let image = decode_pixel_preview_words(entry, words)?;
    let raw565_parse_us = parse_t.elapsed().as_micros() as u64;
    let raw565_parse_cpu_us = elapsed_thread_cpu_us(parse_cpu_t);
    let total_us = total_t.elapsed().as_micros() as u64;
    Ok(PreviewIndexLoad::Loaded(LoadedPreviewPixels {
        timing: ImageLoadTiming {
            read_us,
            decode_us,
            decode_cpu_us,
            raw565_parse_us,
            raw565_parse_cpu_us,
            resize_us: 0,
            total_us,
            encoded_bytes: entry.compressed_len,
            source_width: image.width(),
            source_height: image.height(),
            load_source: PreviewLoadSource::IndexPread,
        },
        image,
    }))
}

fn preview_archive_sidecar_index(
    archive_path: &Path,
) -> Result<Option<Arc<PreviewArchiveSidecarIndex>>, String> {
    Ok(preview_archive_sidecar_lookup(archive_path)?.map(|lookup| lookup.index))
}

fn preview_archive_sidecar_lookup(
    archive_path: &Path,
) -> Result<Option<PreviewArchiveSidecarLookup>, String> {
    let index_path = preview_archive_sidecar_path(archive_path);
    let cache = preview_archive_sidecar_index_cache();
    let cache_key = archive_path.display().to_string();
    let now = Instant::now();
    if let Ok(mut cache) = cache.lock()
        && let Some(cached) = cache.get_mut(&cache_key)
    {
        if now.duration_since(cached.checked_at) < PREVIEW_ARCHIVE_METADATA_TTL {
            if preview_trace_enabled() {
                crate::catalog_errln!(
                    "preview_trace metadata_cache event=hit scope=sidecar archive_path={} ttl_ms={}",
                    archive_path.display(),
                    PREVIEW_ARCHIVE_METADATA_TTL.as_millis()
                );
            }
            return Ok(Some(PreviewArchiveSidecarLookup {
                archive_fingerprint: cached.archive_fingerprint.clone(),
                index: Arc::clone(&cached.index),
            }));
        }
        let archive_fingerprint = preview_archive_fingerprint(archive_path)?;
        let index_fingerprint = preview_archive_fingerprint(&index_path)?;
        if cached.archive_fingerprint == archive_fingerprint
            && cached.index_fingerprint == index_fingerprint
        {
            cached.checked_at = now;
            if preview_trace_enabled() {
                crate::catalog_errln!(
                    "preview_trace metadata_cache event=miss scope=sidecar reason=ttl_revalidated archive_path={}",
                    archive_path.display()
                );
            }
            return Ok(Some(PreviewArchiveSidecarLookup {
                archive_fingerprint,
                index: Arc::clone(&cached.index),
            }));
        } else if preview_trace_enabled() {
            crate::catalog_errln!(
                "preview_trace metadata_cache event=forced_invalidation scope=sidecar reason=fingerprint_changed archive_path={}",
                archive_path.display()
            );
        }
    }
    if !index_path.is_file() {
        return Ok(None);
    }
    let archive_fingerprint = preview_archive_fingerprint(archive_path)?;
    let index_fingerprint = preview_archive_fingerprint(&index_path)?;
    let read_t = Instant::now();
    let index = read_preview_archive_sidecar_index(&index_path, archive_fingerprint.size)?;
    let trust =
        preview_archive_sidecar_trust(archive_path, &index_path, &archive_fingerprint, &index)?;
    let read_us = read_t.elapsed().as_micros() as u64;
    if preview_trace_enabled() {
        crate::catalog_errln!(
            "preview_trace sidecar_index_trusted archive_path={} index_path={} entries={} archive_bytes={} archive_sha256={} trust={} read_us={}",
            archive_path.display(),
            index_path.display(),
            index.entries.len(),
            archive_fingerprint.size,
            index.archive_sha256,
            trust,
            read_us
        );
    }
    let index = Arc::new(index);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(
            cache_key,
            CachedPreviewArchiveSidecarIndex {
                archive_fingerprint: archive_fingerprint.clone(),
                index_fingerprint,
                index: Arc::clone(&index),
                checked_at: Instant::now(),
            },
        );
    }
    Ok(Some(PreviewArchiveSidecarLookup {
        archive_fingerprint,
        index,
    }))
}

fn preview_archive_sidecar_index_cache()
-> &'static Mutex<HashMap<String, CachedPreviewArchiveSidecarIndex>> {
    static INDEXES: OnceLock<Mutex<HashMap<String, CachedPreviewArchiveSidecarIndex>>> =
        OnceLock::new();
    INDEXES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn preview_archive_sidecar_path(archive_path: &Path) -> PathBuf {
    let mut path = archive_path.as_os_str().to_os_string();
    path.push(".idx");
    PathBuf::from(path)
}

fn preview_archive_fingerprint(path: &Path) -> Result<PreviewArchiveFingerprint, String> {
    let path_text = path.display().to_string();
    let meta = preview_archive_metadata(&path_text)
        .map_err(|e| format!("metadata preview archive {path_text}: {e}"))?;
    let mtime_secs = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0);
    Ok(PreviewArchiveFingerprint {
        path: path_text,
        size: meta.len(),
        mtime_secs,
    })
}

fn preview_archive_sidecar_trust(
    archive_path: &Path,
    index_path: &Path,
    archive_fingerprint: &PreviewArchiveFingerprint,
    index: &PreviewArchiveSidecarIndex,
) -> Result<&'static str, String> {
    let Some(root) = archive_path.parent() else {
        return Ok("structural");
    };
    let state_path = screenshot_media_state_path_in_root(root);
    let Ok(text) = std::fs::read_to_string(&state_path) else {
        return Ok("structural");
    };
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("parse screenshot media state {}: {e}", state_path.display()))?;
    let archive_text = archive_path.display().to_string();
    let index_text = index_path.display().to_string();
    let mut found_archive_state = false;
    if let Some(systems) = value.get("systems").and_then(serde_json::Value::as_object) {
        for system_state in systems.values() {
            if sidecar_state_entry_matches(
                system_state,
                &archive_text,
                &index_text,
                archive_fingerprint.size,
                &index.archive_sha256,
                &mut found_archive_state,
            )? {
                return Ok("state_sha");
            }
        }
    }
    if let Some(packs) = value.get("packs").and_then(serde_json::Value::as_object) {
        for pack_state in packs.values() {
            if sidecar_state_entry_matches(
                pack_state,
                &archive_text,
                &index_text,
                archive_fingerprint.size,
                &index.archive_sha256,
                &mut found_archive_state,
            )? {
                return Ok("state_sha");
            }
        }
    }
    if found_archive_state {
        return Err(format!(
            "{}: preview archive index state mismatch",
            index_path.display()
        ));
    }
    Ok("structural")
}

fn sidecar_state_entry_matches(
    value: &serde_json::Value,
    archive_path: &str,
    index_path: &str,
    archive_bytes: u64,
    archive_sha256: &str,
    found_archive_state: &mut bool,
) -> Result<bool, String> {
    if pack_state_matches(
        value,
        archive_path,
        index_path,
        archive_bytes,
        archive_sha256,
        found_archive_state,
    )? {
        return Ok(true);
    }
    if let Some(size) = value
        .get("preferred_size")
        .and_then(serde_json::Value::as_str)
        && let Some(pack_state) = value.get("packs").and_then(|packs| packs.get(size))
        && pack_state_matches(
            pack_state,
            archive_path,
            index_path,
            archive_bytes,
            archive_sha256,
            found_archive_state,
        )?
    {
        return Ok(true);
    }
    if let Some(packs) = value.get("packs").and_then(serde_json::Value::as_object) {
        for pack_state in packs.values() {
            if pack_state_matches(
                pack_state,
                archive_path,
                index_path,
                archive_bytes,
                archive_sha256,
                found_archive_state,
            )? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn pack_state_matches(
    pack_state: &serde_json::Value,
    archive_path: &str,
    index_path: &str,
    archive_bytes: u64,
    archive_sha256: &str,
    found_archive_state: &mut bool,
) -> Result<bool, String> {
    if pack_state
        .get("local_path")
        .and_then(serde_json::Value::as_str)
        != Some(archive_path)
    {
        return Ok(false);
    }
    *found_archive_state = true;
    let Some(index_state) = pack_state.get("index") else {
        return Err(format!("{index_path}: preview archive index state missing"));
    };
    if index_state
        .get("local_path")
        .and_then(serde_json::Value::as_str)
        != Some(index_path)
    {
        return Err(format!(
            "{index_path}: preview archive index path mismatch in state"
        ));
    }
    if index_state
        .get("archive_bytes")
        .and_then(serde_json::Value::as_u64)
        != Some(archive_bytes)
    {
        return Err(format!(
            "{index_path}: preview archive byte count mismatch in state"
        ));
    }
    if index_state
        .get("archive_sha256")
        .and_then(serde_json::Value::as_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
        != Some(archive_sha256)
    {
        return Err(format!(
            "{index_path}: preview archive sha mismatch in state"
        ));
    }
    Ok(true)
}

fn read_preview_archive_sidecar_index(
    index_path: &Path,
    archive_bytes: u64,
) -> Result<PreviewArchiveSidecarIndex, String> {
    let bytes = std::fs::read(index_path)
        .map_err(|e| format!("read preview archive index {}: {e}", index_path.display()))?;
    if bytes.len() < 84 {
        return Err(format!(
            "{}: preview archive index too short",
            index_path.display()
        ));
    }
    if &bytes[..8] != SIDECAR_INDEX_MAGIC {
        return Err(format!(
            "{}: bad preview archive index magic",
            index_path.display()
        ));
    }
    let indexed_archive_bytes = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    if indexed_archive_bytes != archive_bytes {
        return Err(format!(
            "{}: archive byte mismatch indexed={} actual={archive_bytes}",
            index_path.display(),
            indexed_archive_bytes
        ));
    }
    let archive_sha = std::str::from_utf8(&bytes[16..80])
        .map_err(|e| format!("preview archive index sha utf8: {e}"))?;
    if archive_sha.len() != 64 || !archive_sha.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!(
            "{}: bad preview archive index sha",
            index_path.display()
        ));
    }
    let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    validate_entry_count(count, "preview archive sidecar index")?;
    let mut pos = 84usize;
    let mut entries = HashMap::with_capacity(count);
    for _ in 0..count {
        let context = format!("preview archive index {}", index_path.display());
        let (name, entry) = read_sidecar_entry(&bytes, &mut pos, &context, archive_bytes)?;
        let key = name.to_ascii_lowercase();
        if entries.insert(key.clone(), entry).is_some() {
            return Err(format!(
                "{}: duplicate preview archive index entry {key}",
                index_path.display()
            ));
        }
    }
    if pos != bytes.len() {
        return Err(format!(
            "{}: trailing preview archive index bytes",
            index_path.display()
        ));
    }
    Ok(PreviewArchiveSidecarIndex {
        archive_sha256: archive_sha.to_ascii_lowercase(),
        entries,
    })
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    let mut done = 0usize;
    while done < buf.len() {
        let read = file.read_at(&mut buf[done..], offset + done as u64)?;
        if read == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        done += read;
    }
    Ok(())
}

fn open_preview_archive_file_for_index_pread(path: &Path) -> std::io::Result<File> {
    #[cfg(test)]
    {
        let path_text = path.display().to_string();
        let calls = INDEX_PREAD_ARCHIVE_OPEN_CALLS.get_or_init(|| Mutex::new(HashMap::new()));
        if let Ok(mut calls) = calls.lock() {
            *calls.entry(path_text).or_insert(0) += 1;
        }
    }
    File::open(path)
}

fn start_background_preview_archive_load(archive_path: String) {
    static LOADING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let loading = LOADING.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut loading) = loading.lock()
        && !loading.insert(archive_path.clone())
    {
        return;
    }
    let loading = LOADING.get_or_init(|| Mutex::new(HashSet::new()));
    let _ = std::thread::Builder::new()
        .name("preview-archive-load".to_string())
        .spawn(move || {
            let _ = preview_archives_for_paths(vec![archive_path.clone()]);
            if let Ok(mut loading) = loading.lock() {
                loading.remove(&archive_path);
            }
        });
}

impl PreviewArchive {
    fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path)
            .map_err(|e| format!("open preview archive {}: {e}", path.display()))?;
        let archive_bytes = file
            .metadata()
            .map_err(|e| format!("metadata preview archive {}: {e}", path.display()))?
            .len();
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)
            .map_err(|e| format!("read preview archive magic {}: {e}", path.display()))?;
        if &magic != V2_PIXELS_MAGIC {
            return Err(format!("{}: bad preview archive v2 magic", path.display()));
        }
        let count = read_u32(&mut file)? as usize;
        let entries = read_v2_pixel_entries(&mut file, count, archive_bytes)?;
        // SAFETY: the immutable mapping owns its file-backed pages and this
        // process never writes or truncates preview packs while a reader is
        // alive. Deployment publishes packs atomically under a new inode.
        let bytes = unsafe { Mmap::map(&file) }
            .map_err(|e| format!("map preview archive {}: {e}", path.display()))?;
        Ok(Self { bytes, entries })
    }

    fn load_timed(
        &self,
        name: &str,
        _scratch: &mut PreviewArchiveScratch,
    ) -> Result<Option<LoadedPreviewPixels>, String> {
        let key = name.to_ascii_lowercase();
        let Some(entry) = self.entries.get(&key).copied() else {
            return Ok(None);
        };
        let total_t = Instant::now();
        let read_t = Instant::now();
        let start = entry.offset as usize;
        let end = start
            .checked_add(entry.compressed_len)
            .ok_or_else(|| format!("preview archive offset overflow {name}"))?;
        let compressed_slice = self
            .bytes
            .get(start..end)
            .ok_or_else(|| format!("preview archive slice out of range {name}"))?;
        let read_us = read_t.elapsed().as_micros() as u64;

        let decode_t = Instant::now();
        let decode_cpu_t = thread_cpu_us();
        let words = decode_preview_archive_entry_words(compressed_slice, entry)
            .map_err(|e| format!("preview archive lz4 decode {name}: {e}"))?;
        let decode_us = decode_t.elapsed().as_micros() as u64;
        let decode_cpu_us = elapsed_thread_cpu_us(decode_cpu_t);
        let parse_t = Instant::now();
        let parse_cpu_t = thread_cpu_us();
        let image = decode_pixel_preview_words(entry, words)?;
        let raw565_parse_us = parse_t.elapsed().as_micros() as u64;
        let raw565_parse_cpu_us = elapsed_thread_cpu_us(parse_cpu_t);
        let total_us = total_t.elapsed().as_micros() as u64;
        Ok(Some(LoadedPreviewPixels {
            timing: ImageLoadTiming {
                read_us,
                decode_us,
                decode_cpu_us,
                raw565_parse_us,
                raw565_parse_cpu_us,
                resize_us: 0,
                total_us,
                encoded_bytes: entry.compressed_len,
                source_width: image.width(),
                source_height: image.height(),
                load_source: PreviewLoadSource::ArchiveMem,
            },
            image,
        }))
    }
}

fn read_v2_pixel_entries(
    file: &mut File,
    count: usize,
    archive_bytes: u64,
) -> Result<HashMap<String, PreviewArchiveEntry>, String> {
    validate_entry_count(count, "preview archive v2")?;
    let mut entries = HashMap::with_capacity(count);
    for _ in 0..count {
        let (name, entry) = read_file_entry(file, "preview archive v2", archive_bytes)?;
        let key = name.to_ascii_lowercase();
        if entries.insert(key.clone(), entry).is_some() {
            return Err(format!("preview archive v2 duplicate entry {key}"));
        }
    }
    Ok(entries)
}

#[cfg(target_os = "linux")]
fn thread_cpu_us() -> Option<u64> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts points to initialized writable storage for the duration of the
    // syscall; failures are handled by returning None.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    if rc == 0 {
        Some(ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1_000)
    } else {
        None
    }
}

#[cfg(not(target_os = "linux"))]
fn thread_cpu_us() -> Option<u64> {
    None
}

fn elapsed_thread_cpu_us(start: Option<u64>) -> u64 {
    start
        .and_then(|start| thread_cpu_us().map(|end| end.saturating_sub(start)))
        .unwrap_or(0)
}

fn decode_preview_archive_entry_words(
    payload: &[u8],
    entry: PreviewArchiveEntry,
) -> Result<Arc<[u16]>, String> {
    if !entry.raw_len.is_multiple_of(2) {
        return Err(format!(
            "preview archive entry has odd RGB565 byte length {}",
            entry.raw_len
        ));
    }
    let mut words = Arc::<[u16]>::new_uninit_slice(entry.raw_len / 2);
    let uninit = Arc::get_mut(&mut words).expect("new RGB565 Arc must be uniquely owned");
    // SAFETY: the uninitialized u16 allocation is aligned for bytes and spans
    // exactly `raw_len`; successful branches below initialize every byte.
    let destination =
        unsafe { std::slice::from_raw_parts_mut(uninit.as_mut_ptr().cast::<u8>(), entry.raw_len) };
    match entry.payload_flag {
        0 => {
            let len = lz4_flex::block::decompress_into(payload, destination)
                .map_err(|e| e.to_string())?;
            if len != entry.raw_len {
                return Err(format!(
                    "lz4 pixel length mismatch got={len} expected={}",
                    entry.raw_len
                ));
            }
        }
        1 => {
            if payload.len() != entry.raw_len {
                return Err(format!(
                    "raw pixel length mismatch got={} expected={}",
                    payload.len(),
                    entry.raw_len
                ));
            }
            destination.copy_from_slice(payload);
        }
        other => return Err(format!("bad preview archive payload flag {other}")),
    }
    #[cfg(target_endian = "big")]
    for pixel in destination.chunks_exact_mut(2) {
        pixel.swap(0, 1);
    }
    // SAFETY: every byte was initialized above and big-endian targets were
    // converted from the archive's little-endian representation in place.
    Ok(unsafe { words.assume_init() })
}

#[cfg(test)]
fn decode_lz4_block_entry_into<'a>(
    data: &'a [u8],
    raw_len: usize,
    out: &'a mut Vec<u8>,
) -> Result<&'a [u8], String> {
    let (&flag, block) = data
        .split_first()
        .ok_or_else(|| "empty lz4 block entry".to_string())?;
    match flag {
        0 => {
            if out.len() < raw_len {
                out.resize(raw_len, 0);
            }
            let len = lz4_flex::block::decompress_into(block, &mut out[..raw_len])
                .map_err(|e| e.to_string())?;
            if len != raw_len {
                return Err(format!(
                    "lz4 block length mismatch got={len} expected={raw_len}"
                ));
            }
            Ok(&out[..len])
        }
        1 => {
            if block.len() != raw_len {
                return Err(format!(
                    "raw lz4 block length mismatch got={} expected={raw_len}",
                    block.len()
                ));
            }
            Ok(block)
        }
        other => Err(format!("bad lz4 block flag {other}")),
    }
}

#[cfg(test)]
fn decode_pixel_preview_bytes(
    entry: PreviewArchiveEntry,
    data: &[u8],
) -> Result<PreviewPixels, String> {
    validate_entry_geometry(
        "pixel preview",
        "<decoded>",
        entry.width,
        entry.height,
        entry.stride_bytes,
        data.len(),
    )?;
    let words = raw565_words_from_le_bytes(data, "pixel preview")?;
    Ok(PreviewPixels::Rgb565 {
        width: entry.width,
        height: entry.height,
        stride_bytes: entry.stride_bytes,
        words: Arc::from(words.into_boxed_slice()),
    })
}

fn decode_pixel_preview_words(
    entry: PreviewArchiveEntry,
    words: Arc<[u16]>,
) -> Result<PreviewPixels, String> {
    validate_entry_geometry(
        "pixel preview",
        "<decoded>",
        entry.width,
        entry.height,
        entry.stride_bytes,
        words.len().saturating_mul(2),
    )?;
    Ok(PreviewPixels::Rgb565 {
        width: entry.width,
        height: entry.height,
        stride_bytes: entry.stride_bytes,
        words,
    })
}

#[cfg(test)]
fn raw565_words_from_le_bytes(data: &[u8], context: &str) -> Result<Vec<u16>, String> {
    if !data.len().is_multiple_of(2) {
        return Err(format!(
            "{context} has odd byte length {} for RGB565 words",
            data.len()
        ));
    }
    let mut words = Vec::with_capacity(data.len() / 2);
    for chunk in data.chunks_exact(2) {
        words.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    Ok(words)
}

#[cfg(test)]
fn decode_raw565_preview_bytes(data: &[u8]) -> Result<PreviewPixels, String> {
    if data.len() < 20 || &data[..8] != b"MM56501\0" {
        return Err("raw565 preview bad header".into());
    }
    let width = u32::from_le_bytes(data[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(data[12..16].try_into().unwrap());
    let stride_bytes = u32::from_le_bytes(data[16..20].try_into().unwrap());
    if width == 0 || height == 0 {
        return Err(format!(
            "raw565 preview bad dimensions width={} height={}",
            width, height
        ));
    }
    let min_stride = width as usize * 2;
    if !(stride_bytes as usize).is_multiple_of(16) || (stride_bytes as usize) < min_stride {
        return Err(format!(
            "raw565 preview bad stride width={} stride={}",
            width, stride_bytes
        ));
    }
    let expected = stride_bytes as usize * height as usize;
    if data.len() - 20 != expected {
        return Err(format!(
            "raw565 preview length mismatch got={} expected={}",
            data.len() - 20,
            expected
        ));
    }
    let words = raw565_words_from_le_bytes(&data[20..], "raw565 preview")?;
    Ok(PreviewPixels::Rgb565 {
        width,
        height,
        stride_bytes,
        words: Arc::from(words.into_boxed_slice()),
    })
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x').or_else(|| s.split_once('X'))?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn preview_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MISTER_PREVIEW_TRACE").as_deref(),
            Ok("1") | Ok("on") | Ok("true") | Ok("yes")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_config_captures_clamped_settings_and_deduplicated_archives() {
        let values = HashMap::from([
            ("MISTER_PREVIEW_RESIZE_FILTER", "nearest"),
            ("MISTER_PREVIEW_RESIZE_MAX", "640x480"),
            ("MISTER_PREVIEW_DECODED_CACHE_CAP", "7"),
            ("MISTER_PREVIEW_ARCHIVE_MEM_PRIMARY", "true"),
            ("MISTER_PREVIEW_ARCHIVES", "/tmp/a:/tmp/a:/tmp/b"),
            ("MISTER_PREVIEW_ARCHIVE_AUTO", "off"),
        ]);
        let config = PreviewWorkerConfig::capture_with(|name| values.get(name).copied());

        assert_eq!(config.resize().filter, PreviewResizeFilter::Nearest);
        assert_eq!((config.resize().max_w, config.resize().max_h), (640, 480));
        assert_eq!(config.decoded_cache_cap(), 7);
        assert!(config.archive_mem_primary());
        assert!(config.archive_mem_warm());
        assert_eq!(config.archive_paths(), &["/tmp/a", "/tmp/b"]);
    }

    #[test]
    fn preview_window_orders_selected_then_nearby() {
        assert_eq!(
            preview_window_indices(20, 10, 5),
            vec![10, 9, 11, 8, 12, 7, 13, 6, 14, 5, 15]
        );
    }

    #[test]
    fn preview_window_clamps_at_edges() {
        assert_eq!(preview_window_indices(4, 0, 5), vec![0, 1, 2, 3]);
        assert_eq!(preview_window_indices(4, 3, 5), vec![3, 2, 1, 0]);
    }

    #[test]
    fn preview_window_paths_dedupes_missing_and_duplicate_paths() {
        let items = vec![Some("a"), Some("b"), None, Some("b"), Some("c"), Some("d")];
        let paths = preview_window_paths(&items, 2, 3, |p| *p);
        assert_eq!(paths, vec!["b", "a", "c", "d"]);
    }

    fn preview_request(
        generation: u64,
        title: &str,
        preview_archive_path: &str,
        preview_asset_key: &str,
        priority: PreviewPriority,
    ) -> PreviewRequest {
        PreviewRequest {
            generation,
            title: title.to_string(),
            preview_archive_path: preview_archive_path.to_string(),
            preview_asset_key: preview_asset_key.to_string(),
            requested_at: Instant::now(),
            requested_at_ms: 0,
            priority,
        }
    }

    fn queued_request(
        generation: u64,
        title: &str,
        preview_asset_key: &str,
        priority: PreviewPriority,
    ) -> PreviewRequest {
        preview_request(
            generation,
            title,
            "/tmp/arcade-screenshots.mmlz4b",
            preview_asset_key,
            priority,
        )
    }

    #[test]
    fn preview_queue_pops_selected_before_prefetch_and_keeps_remaining_work() {
        let mut queue = vec![
            queued_request(1, "near", "near", PreviewPriority::Prefetch { distance: 1 }),
            queued_request(2, "selected", "selected", PreviewPriority::Selected),
            queued_request(3, "far", "far", PreviewPriority::Prefetch { distance: 4 }),
        ];

        assert_eq!(pop_next_preview_request(&mut queue).unwrap().generation, 2);
        assert_eq!(queue.len(), 2);
        assert_eq!(pop_next_preview_request(&mut queue).unwrap().generation, 1);
        assert_eq!(pop_next_preview_request(&mut queue).unwrap().generation, 3);
        assert!(pop_next_preview_request(&mut queue).is_none());
    }

    #[test]
    fn enqueue_replaces_lower_priority_duplicate_and_drops_far_prefetch() {
        let mut queue = Vec::new();
        enqueue_preview_request(
            &mut queue,
            queued_request(
                1,
                "prefetch",
                "same",
                PreviewPriority::Prefetch { distance: 3 },
            ),
        );
        enqueue_preview_request(
            &mut queue,
            queued_request(2, "selected", "same", PreviewPriority::Selected),
        );
        enqueue_preview_request(
            &mut queue,
            queued_request(
                3,
                "too far",
                "far",
                PreviewPriority::Prefetch {
                    distance: TURBO_PREVIEW_DECODED_CACHE_CAP + 1,
                },
            ),
        );

        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].generation, 2);
        assert_eq!(queue[0].priority, PreviewPriority::Selected);
    }

    #[test]
    fn enqueue_selected_supersedes_older_selected_work() {
        let mut queue = Vec::new();
        enqueue_preview_request(
            &mut queue,
            queued_request(1, "old selected", "old", PreviewPriority::Selected),
        );
        enqueue_preview_request(
            &mut queue,
            queued_request(2, "near", "near", PreviewPriority::Prefetch { distance: 1 }),
        );
        enqueue_preview_request(
            &mut queue,
            queued_request(3, "new selected", "new", PreviewPriority::Selected),
        );

        assert_eq!(queue.len(), 2);
        assert!(queue.iter().all(|req| req.generation != 1));
        assert_eq!(pop_next_preview_request(&mut queue).unwrap().generation, 3);
        assert_eq!(pop_next_preview_request(&mut queue).unwrap().generation, 2);
    }

    #[test]
    fn latest_mailbox_replaces_unstarted_selected_work() {
        let mailbox = LatestMailbox::new();
        assert!(mailbox.publish(queued_request(
            1,
            "old selected",
            "old",
            PreviewPriority::Selected,
        )));
        assert!(mailbox.publish(queued_request(
            2,
            "new selected",
            "new",
            PreviewPriority::Selected,
        )));

        let request = mailbox.take_blocking().expect("latest request");
        assert_eq!(request.generation, 2);
        assert_eq!(request.preview_asset_key, "new");
        mailbox.close();
        assert!(mailbox.take_blocking().is_none());
    }

    #[test]
    fn selected_request_handle_reserves_shared_generations_and_publishes_latest() {
        let selected_tx = Arc::new(LatestMailbox::new());
        let handle = PreviewSelectedRequestHandle {
            selected_tx: Arc::clone(&selected_tx),
            trace_start: Instant::now(),
            next_generation: Arc::new(AtomicU64::new(1)),
        };
        let first = handle.reserve_generation();
        let second = handle.reserve_generation();

        assert_eq!((first, second), (1, 2));
        assert!(handle.publish_reserved(
            first,
            "old selected".to_string(),
            "old archive".to_string(),
            "old key".to_string(),
        ));
        assert!(handle.publish_reserved(
            second,
            "new selected".to_string(),
            "new archive".to_string(),
            "new key".to_string(),
        ));

        let request = selected_tx.try_take().expect("latest selected request");
        assert_eq!(request.generation, second);
        assert_eq!(request.preview_asset_key, "new key");
    }

    #[test]
    fn stale_prefetch_prune_keeps_selected_and_recent_generations() {
        let mut queue = vec![
            queued_request(1, "old selected", "selected", PreviewPriority::Selected),
            queued_request(
                2,
                "old prefetch",
                "old",
                PreviewPriority::Prefetch { distance: 1 },
            ),
            queued_request(
                TURBO_PREVIEW_DECODED_CACHE_CAP as u64 + 9,
                "recent prefetch",
                "recent",
                PreviewPriority::Prefetch { distance: 1 },
            ),
        ];

        prune_stale_prefetch_requests(&mut queue, TURBO_PREVIEW_DECODED_CACHE_CAP as u64 + 10);

        assert_eq!(
            queue.iter().map(|req| req.generation).collect::<Vec<_>>(),
            vec![1, TURBO_PREVIEW_DECODED_CACHE_CAP as u64 + 9]
        );
    }

    #[test]
    fn decoded_cache_is_lru_and_resets_timing_on_hits() {
        let mut cache = PreviewDecodedCache::new(1);
        let first = LoadedPreviewPixels {
            timing: ImageLoadTiming {
                read_us: 11,
                decode_us: 22,
                decode_cpu_us: 12,
                raw565_parse_us: 33,
                raw565_parse_cpu_us: 13,
                resize_us: 33,
                total_us: 66,
                encoded_bytes: 44,
                source_width: 1,
                source_height: 1,
                load_source: PreviewLoadSource::ArchiveMem,
            },
            image: PreviewPixels::Rgb565 {
                width: 1,
                height: 1,
                stride_bytes: 16,
                words: Arc::from([0xf800]),
            },
        };
        let second = LoadedPreviewPixels {
            timing: ImageLoadTiming {
                source_width: 2,
                source_height: 1,
                ..first.timing
            },
            image: PreviewPixels::Rgb565 {
                width: 2,
                height: 1,
                stride_bytes: 16,
                words: Arc::from([0x07e0, 0x001f]),
            },
        };

        cache.insert("a".into(), &first);
        let hit = cache.get("a").expect("cache hit");
        assert_eq!(hit.timing.read_us, 0);
        assert_eq!(hit.timing.decode_us, 0);
        assert_eq!(hit.timing.decode_cpu_us, 0);
        assert_eq!(hit.timing.raw565_parse_us, 0);
        assert_eq!(hit.timing.raw565_parse_cpu_us, 0);
        assert_eq!(hit.timing.resize_us, 0);
        assert_eq!(hit.timing.encoded_bytes, 0);
        assert_eq!(hit.timing.load_source, PreviewLoadSource::DecodedCache);
        assert_eq!(hit.timing.source_width, 1);
        let (
            PreviewPixels::Rgb565 {
                words: first_words, ..
            },
            PreviewPixels::Rgb565 {
                words: hit_words, ..
            },
        ) = (&first.image, &hit.image);
        assert!(Arc::ptr_eq(first_words, hit_words));

        cache.insert("b".into(), &second);
        assert!(cache.get("a").is_none());
        assert_eq!(cache.get("b").unwrap().timing.source_width, 2);
    }

    #[test]
    fn load_preview_failure_preserves_request_metadata() {
        let req = preview_request(
            77,
            "Missing",
            "/tmp/missing/320x320-screenshots.mmlz4b",
            "missing",
            PreviewPriority::Selected,
        );
        let cache = Arc::new(Mutex::new(PreviewDecodedCache::new(2)));
        let mut scratch = PreviewArchiveScratch::default();

        let result = load_preview(
            req,
            &cache,
            &mut scratch,
            Instant::now(),
            &PreviewWorkerConfig::default(),
        );

        assert_eq!(result.generation, 77);
        assert_eq!(result.title, "Missing");
        assert_eq!(
            result.preview_archive_path,
            "/tmp/missing/320x320-screenshots.mmlz4b"
        );
        assert_eq!(result.preview_asset_key, "missing");
        assert!(result.image.is_none());
        assert_eq!(result.decoded_bytes, 0);
        assert_eq!(result.source_width, 0);
        assert_eq!(result.source_height, 0);
        assert_eq!(result.resize_filter, PreviewResizeFilter::Hybrid);
        assert_eq!(result.priority, PreviewPriority::Selected);
    }

    #[test]
    fn hybrid_filter_uses_nearest_for_upscale_and_lanczos_for_downscale_labels() {
        assert_eq!(
            PreviewResizeFilter::from_label("hybrid"),
            PreviewResizeFilter::Hybrid
        );
        assert_eq!(PreviewResizeFilter::Hybrid.label(), "hybrid");
    }

    #[test]
    fn preview_archive_entry_stems_reads_index_only() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-stems-{}.mmlz4b",
            std::process::id()
        ));
        let name = b"mpatrol.rgb565";
        let index_len = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(V2_PIXELS_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&(index_len as u64).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&[0; 16]);
        std::fs::write(&path, bytes).expect("write lz4 block fixture");

        let stems = preview_archive_entry_stems(&path).expect("read lz4 block index");

        assert!(stems.contains("mpatrol"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn lz4_preview_archive_index_reads_names_without_payload_decode() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-{}.mmlz4b",
            std::process::id()
        ));
        let name = b"1941u.rgb565";
        let index_len = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(V2_PIXELS_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&(index_len as u64).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&[0; 16]);
        std::fs::write(&path, bytes).expect("write lz4 block fixture");

        let index = preview_archive_index(&path).expect("read lz4 block index");

        assert_eq!(index.codec, "lz4-block");
        assert_eq!(index.entries, vec!["1941u"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_archive_index_rejects_bad_geometry_without_payload_decode() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-entry-bad-geometry-{}.mmlz4b",
            std::process::id()
        ));
        let name = b"bad.rgb565";
        let index_len = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(V2_PIXELS_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&18u32.to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&18u32.to_le_bytes());
        bytes.extend_from_slice(&(index_len as u64).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&[0; 18]);
        std::fs::write(&path, bytes).expect("write bad geometry index fixture");

        let err = preview_archive_index(&path).expect_err("bad geometry must fail");

        assert!(err.contains("bad geometry"), "unexpected error: {err}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sidecar_entry_stems_reads_idx_membership() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-sidecar-stems-{}.mmlz4b",
            std::process::id()
        ));
        let payload = raw565_fixture(2, 1, &[0xf800, 0x07e0]);
        write_lz4_block_archive_with_index(&path, "Tiny.Game.rgb565", &payload);

        let stems = preview_archive_sidecar_entry_stems(&path)
            .expect("read sidecar")
            .expect("sidecar exists");

        assert_eq!(stems.archive_path, path.display().to_string());
        assert_eq!(
            stems.index_path,
            preview_archive_sidecar_path(&path).display().to_string()
        );
        assert_eq!(stems.entries, vec!["tiny.game"]);
        let _ = std::fs::remove_file(preview_archive_sidecar_path(&path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn archive_lookup_matches_mixed_case_cache_names() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-case-{}.mmlz4b",
            std::process::id()
        ));
        let payload = raw565_fixture(2, 1, &[0xf800, 0x07e0]);
        write_lz4_block_archive(&path, "sonic.rgb565", &payload);

        let archive = PreviewArchive::open(&path).expect("open lz4 block archive");
        let mut scratch = PreviewArchiveScratch::default();
        let loaded = archive
            .load_timed("Sonic.rgb565", &mut scratch)
            .expect("load mixed-case cache name")
            .expect("archive entry");

        assert_eq!(loaded.image.width(), 2);
        assert_eq!(loaded.image.height(), 1);
        assert_eq!(loaded.image.decoded_bytes(), 16);
        assert_eq!(loaded.timing.load_source, PreviewLoadSource::ArchiveMem);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn structurally_valid_sidecar_metadata_is_trusted() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-stale-sidecar-{}.mmlz4b",
            std::process::id()
        ));
        let name = "tiny.rgb565";
        let payload = raw565_fixture(2, 1, &[0xf800, 0x07e0]);
        write_lz4_block_archive(&path, name, &payload);
        let archive_bytes = std::fs::metadata(&path).unwrap().len();
        let archive_payload_offset = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let index_path = preview_archive_sidecar_path(&path);
        let mut index = Vec::new();
        index.extend_from_slice(b"MMIDX02\0");
        index.extend_from_slice(&archive_bytes.to_le_bytes());
        index
            .extend_from_slice(b"0000000000000000000000000000000000000000000000000000000000000000");
        index.extend_from_slice(&1u32.to_le_bytes());
        index.extend_from_slice(&(name.len() as u16).to_le_bytes());
        index.extend_from_slice(&1u32.to_le_bytes());
        index.extend_from_slice(&1u32.to_le_bytes());
        index.extend_from_slice(&16u32.to_le_bytes());
        index.extend_from_slice(&16u32.to_le_bytes());
        index.push(1);
        index.extend_from_slice(&16u32.to_le_bytes());
        index.extend_from_slice(&(archive_payload_offset as u64).to_le_bytes());
        index.extend_from_slice(name.as_bytes());
        std::fs::write(&index_path, index).expect("write stale sidecar fixture");
        let mut scratch = PreviewArchiveScratch::default();

        let loaded = load_preview_pixels(
            &path.display().to_string(),
            "tiny",
            &mut scratch,
            PreviewResizeSpec::off(),
        )
        .expect("load should use trusted sidecar metadata");

        assert_eq!(loaded.timing.load_source, PreviewLoadSource::IndexPread);
        assert_eq!(loaded.timing.source_width, 1);
        assert_eq!(loaded.timing.source_height, 1);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn generated_sidecar_state_sha_mismatch_falls_back_to_archive_metadata() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-preview-state-sha-mismatch-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create archive root");
        let path = root.join("arcade-screenshots-320x320.mmlz4b");
        let name = "tiny.rgb565";
        let payload = raw565_fixture(2, 1, &[0xf800, 0x07e0]);
        write_lz4_block_archive_with_index(&path, name, &payload);
        let index_path = preview_archive_sidecar_path(&path);
        let state_path = crate::media_identity::screenshot_media_state_path_in_root(&root);
        let state = serde_json::json!({
            "systems": {
                "arcade": {
                    "preferred_size": "320x320",
                    "packs": {
                        "320x320": {
                            "local_path": path.display().to_string(),
                            "index": {
                                "local_path": index_path.display().to_string(),
                                "archive_bytes": std::fs::metadata(&path).unwrap().len(),
                                "archive_sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                            }
                        }
                    }
                }
            }
        });
        std::fs::write(&state_path, state.to_string()).expect("write media state");
        let mut scratch = PreviewArchiveScratch::default();

        let loaded = load_preview_pixels(
            &path.display().to_string(),
            "tiny",
            &mut scratch,
            PreviewResizeSpec::off(),
        )
        .expect("load should fall back when generated sidecar state mismatches");

        assert_eq!(loaded.timing.load_source, PreviewLoadSource::ArchiveMem);
        assert_eq!(loaded.timing.source_width, 2);
        assert_eq!(loaded.timing.source_height, 1);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(state_path);
        let _ = std::fs::remove_dir(root);
    }

    #[test]
    fn generated_sidecar_state_checks_non_preferred_pack_size() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-preview-state-nonpreferred-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create archive root");
        let path = root.join("arcade-screenshots-240x240.mmlz4b");
        let name = "tiny.rgb565";
        let payload = raw565_fixture(2, 1, &[0xf800, 0x07e0]);
        write_lz4_block_archive_with_index(&path, name, &payload);
        let index_path = preview_archive_sidecar_path(&path);
        let state_path = crate::media_identity::screenshot_media_state_path_in_root(&root);
        let state = serde_json::json!({
            "systems": {
                "arcade": {
                    "preferred_size": "320x320",
                    "packs": {
                        "320x320": {
                            "local_path": root.join("arcade-screenshots-320x320.mmlz4b").display().to_string(),
                            "index": {
                                "local_path": root.join("arcade-screenshots-320x320.mmlz4b.idx").display().to_string(),
                                "archive_bytes": 1,
                                "archive_sha256": "0000000000000000000000000000000000000000000000000000000000000000"
                            }
                        },
                        "240x240": {
                            "local_path": path.display().to_string(),
                            "index": {
                                "local_path": index_path.display().to_string(),
                                "archive_bytes": std::fs::metadata(&path).unwrap().len(),
                                "archive_sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                            }
                        }
                    }
                }
            }
        });
        std::fs::write(&state_path, state.to_string()).expect("write media state");
        let mut scratch = PreviewArchiveScratch::default();

        let loaded = load_preview_pixels(
            &path.display().to_string(),
            "tiny",
            &mut scratch,
            PreviewResizeSpec::off(),
        )
        .expect("non-preferred generated pack must still use state checks");

        assert_eq!(loaded.timing.load_source, PreviewLoadSource::ArchiveMem);
        assert_eq!(loaded.timing.source_width, 2);
        assert_eq!(loaded.timing.source_height, 1);
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(state_path);
        let _ = std::fs::remove_dir(root);
    }

    #[test]
    fn archive_mem_timing_splits_lz4_decode_from_raw565_parse() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-timing-split-{}.mmlz4b",
            std::process::id()
        ));
        let payload = raw565_fixture(2, 1, &[0xf800, 0x07e0]);
        write_lz4_block_archive(&path, "timing.rgb565", &payload);

        let archive = PreviewArchive::open(&path).expect("open lz4 block archive");
        let mut scratch = PreviewArchiveScratch::default();
        let loaded = archive
            .load_timed("timing.rgb565", &mut scratch)
            .expect("load timing fixture")
            .expect("archive entry");

        assert_eq!(loaded.timing.load_source, PreviewLoadSource::ArchiveMem);
        assert!(loaded.timing.total_us >= loaded.timing.decode_us);
        assert!(loaded.timing.total_us >= loaded.timing.raw565_parse_us);
        assert_eq!(loaded.image.width(), 2);
        assert_eq!(loaded.image.height(), 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn lz4_block_decode_scratch_grows_without_shrinking() {
        let large = raw565_fixture(
            4,
            2,
            &[
                0xf800, 0x07e0, 0x001f, 0xffff, 0x0000, 0x1111, 0x2222, 0x3333,
            ],
        );
        let small = raw565_fixture(1, 1, &[0x07e0]);
        let mut large_payload = vec![0];
        large_payload.extend_from_slice(&lz4_flex::block::compress(&large));
        let mut small_payload = vec![0];
        small_payload.extend_from_slice(&lz4_flex::block::compress(&small));
        let mut scratch = Vec::new();

        let decoded_large = decode_lz4_block_entry_into(&large_payload, large.len(), &mut scratch)
            .expect("decode large lz4 block");
        assert_eq!(decoded_large, large.as_slice());
        let grown_len = scratch.len();
        assert_eq!(grown_len, large.len());

        let decoded_small = decode_lz4_block_entry_into(&small_payload, small.len(), &mut scratch)
            .expect("decode small lz4 block");
        assert_eq!(decoded_small, small.as_slice());
        assert_eq!(scratch.len(), grown_len);
    }

    #[test]
    fn preview_load_uses_index_pread_before_archive_cache() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-pread-{}.mmlz4b",
            std::process::id()
        ));
        write_lz4_block_archive_with_index(
            &path,
            "tiny.rgb565",
            &raw565_fixture(3, 1, &[0xf800, 0x07e0, 0x001f]),
        );
        let mut scratch = PreviewArchiveScratch::default();
        let resize = PreviewResizeSpec {
            filter: PreviewResizeFilter::Hybrid,
            max_w: 320,
            max_h: 320,
        };

        let first = load_preview_pixels(&path.display().to_string(), "tiny", &mut scratch, resize)
            .expect("load via index pread");

        assert_eq!(first.timing.load_source, PreviewLoadSource::IndexPread);
        assert_eq!(first.timing.source_width, 3);
        assert_eq!(first.timing.source_height, 1);

        let _ = preview_archives_for_paths(vec![path.display().to_string()])
            .expect("warm archive after index pread");
        let second = load_preview_pixels(&path.display().to_string(), "tiny", &mut scratch, resize)
            .expect("load via index pread with warmed archive available");
        assert_eq!(second.timing.load_source, PreviewLoadSource::IndexPread);

        let _ = std::fs::remove_file(preview_archive_sidecar_path(&path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn valid_preview_index_missing_entry_does_not_full_load_archive() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-missing-entry-{}.mmlz4b",
            std::process::id()
        ));
        write_lz4_block_archive_with_index(&path, "tiny.rgb565", &raw565_fixture(1, 1, &[0xf800]));
        preview_archives_for_paths(vec![path.display().to_string()])
            .expect("warm archive before missing indexed load");
        let mut scratch = PreviewArchiveScratch::default();

        let err = load_preview_pixels(
            &path.display().to_string(),
            "other",
            &mut scratch,
            PreviewResizeSpec {
                filter: PreviewResizeFilter::Hybrid,
                max_w: 320,
                max_h: 320,
            },
        )
        .expect_err("valid index missing entry should not fall back to archive_mem");

        assert!(
            err.contains("missing from index"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_file(preview_archive_sidecar_path(&path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_preview_index_falls_back_to_archive_mem() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-fallback-{}.mmlz4b",
            std::process::id()
        ));
        write_lz4_block_archive(&path, "tiny.rgb565", &raw565_fixture(1, 1, &[0xf800]));
        std::fs::write(preview_archive_sidecar_path(&path), b"bad-index").expect("write bad index");
        let mut scratch = PreviewArchiveScratch::default();

        let loaded = load_preview_pixels(
            &path.display().to_string(),
            "tiny",
            &mut scratch,
            PreviewResizeSpec {
                filter: PreviewResizeFilter::Hybrid,
                max_w: 320,
                max_h: 320,
            },
        )
        .expect("fall back to full archive");

        assert_eq!(loaded.timing.load_source, PreviewLoadSource::ArchiveMem);
        let _ = std::fs::remove_file(preview_archive_sidecar_path(&path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn duplicate_preview_index_entries_fall_back_to_archive_mem() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-duplicate-{}.mmlz4b",
            std::process::id()
        ));
        let payload = raw565_fixture(1, 1, &[0xf800]);
        write_lz4_block_archive(&path, "tiny.rgb565", &payload);
        write_duplicate_lz4_block_archive_index(&path, "tiny.rgb565", payload.len());
        let mut scratch = PreviewArchiveScratch::default();

        let loaded = load_preview_pixels(
            &path.display().to_string(),
            "tiny",
            &mut scratch,
            PreviewResizeSpec {
                filter: PreviewResizeFilter::Hybrid,
                max_w: 320,
                max_h: 320,
            },
        )
        .expect("fall back to full archive for duplicate index entries");

        assert_eq!(loaded.timing.load_source, PreviewLoadSource::ArchiveMem);
        let _ = std::fs::remove_file(preview_archive_sidecar_path(&path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auto_preview_archive_uses_assets_arcade_pack() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-preview-auto-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create archive root");
        let archive = root.join("arcade-screenshots-320x320.mmlz4b");
        std::fs::write(&archive, b"lz4").expect("write archive marker");
        let resize = PreviewResizeSpec {
            filter: PreviewResizeFilter::Hybrid,
            max_w: 320,
            max_h: 320,
        };

        let selected = auto_preview_archive_path_in_root(&root, resize).expect("archive path");

        assert_eq!(selected, archive.display().to_string());
        let _ = std::fs::remove_file(archive);
        let _ = std::fs::remove_dir(root);
    }

    #[test]
    fn auto_preview_archive_falls_back_to_legacy_pack() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-preview-legacy-auto-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create archive root");
        let archive = root.join("arcade-screenshots.mmlz4b");
        std::fs::write(&archive, b"lz4").expect("write archive marker");
        let resize = PreviewResizeSpec {
            filter: PreviewResizeFilter::Hybrid,
            max_w: 320,
            max_h: 320,
        };

        let selected = auto_preview_archive_path_in_root(&root, resize).expect("archive path");

        assert_eq!(selected, archive.display().to_string());
        let _ = std::fs::remove_file(archive);
        let _ = std::fs::remove_dir(root);
    }

    #[test]
    fn state_resolution_maps_legacy_path_to_size_qualified_pack() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-preview-state-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create archive root");
        let archive = root.join("neogeo-screenshots-320x320.mmlz4b");
        std::fs::write(&archive, b"lz4").expect("write archive marker");
        let state = format!(
            r#"{{
  "systems": {{
    "neogeo": {{
      "preferred_size": "320x320",
      "packs": {{
        "320x320": {{
          "local_path": "{}"
        }}
      }}
    }}
  }}
}}"#,
            archive.display()
        );
        let state_path = crate::media_identity::screenshot_media_state_path_in_root(&root);
        std::fs::write(&state_path, state).expect("write media state");
        let legacy = root.join("neogeo-screenshots.mmlz4b");

        let resolved = resolve_preview_archive_path(&legacy.display().to_string());

        assert_eq!(resolved, archive.display().to_string());
        let _ = std::fs::remove_file(archive);
        let _ = std::fs::remove_file(state_path);
        let _ = std::fs::remove_dir(root);
    }

    #[test]
    fn preview_load_resolves_sized_pack_but_keeps_catalog_path_key() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-preview-load-sized-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create archive root");
        let archive = root.join("arcade-screenshots-320x320.mmlz4b");
        let payload = raw565_fixture(2, 1, &[0xf800, 0x07e0]);
        write_lz4_block_archive(&archive, "pacman.rgb565", &payload);
        let legacy = root.join("arcade-screenshots.mmlz4b");
        let request = PreviewRequest {
            generation: 7,
            title: "Pac-Man".to_string(),
            preview_archive_path: legacy.display().to_string(),
            preview_asset_key: "pacman".to_string(),
            requested_at: Instant::now(),
            requested_at_ms: 0,
            priority: PreviewPriority::Selected,
        };
        let cache = Arc::new(Mutex::new(PreviewDecodedCache::new(
            DEFAULT_PREVIEW_CACHE_CAP,
        )));
        let mut scratch = PreviewArchiveScratch::default();

        let result = load_preview(
            request,
            &cache,
            &mut scratch,
            Instant::now(),
            &PreviewWorkerConfig::default(),
        );

        assert!(result.image.is_some());
        assert_eq!(result.preview_archive_path, legacy.display().to_string());
        let _ = std::fs::remove_file(archive);
        let _ = std::fs::remove_dir(root);
    }

    #[test]
    fn catalog_projection_paths_include_default_console_packs_without_stat() {
        let paths = preview_archive_paths_for_catalog_projection();

        assert!(
            paths
                .iter()
                .any(|path| path == "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b")
        );
        assert!(
            paths
                .iter()
                .any(|path| path == "/media/fat/mister-magik/assets/nes-screenshots.mmlz4b")
        );
        assert!(
            paths
                .iter()
                .any(|path| path == "/media/fat/mister-magik/assets/saturn-screenshots.mmlz4b")
        );
    }

    #[test]
    fn raw565_preview_header_round_trips_pixels() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MM56501\0");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&0xf800u16.to_le_bytes());
        bytes.extend_from_slice(&0x07e0u16.to_le_bytes());
        bytes.extend_from_slice(&0x001fu16.to_le_bytes());
        bytes.resize(20 + 16, 0);
        let decoded = decode_raw565_preview_bytes(&bytes).unwrap();
        match decoded {
            PreviewPixels::Rgb565 {
                width,
                height,
                stride_bytes,
                words,
            } => {
                assert_eq!(width, 3);
                assert_eq!(height, 1);
                assert_eq!(stride_bytes, 16);
                assert_eq!(&words[..3], &[0xf800, 0x07e0, 0x001f]);
            }
        }
    }

    #[test]
    fn archive_decode_writes_raw_and_lz4_pixels_into_final_words() {
        let expected: [u16; 8] = [0xf800, 0x07e0, 0x001f, 0xffff, 0x1234, 0xabcd, 0, 0];
        let raw = expected
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect::<Vec<_>>();
        for (payload_flag, payload) in [(1, raw.clone()), (0, lz4_flex::block::compress(&raw))] {
            let entry = PreviewArchiveEntry {
                raw_len: raw.len(),
                compressed_len: payload.len(),
                offset: 0,
                width: 3,
                height: 1,
                stride_bytes: 16,
                payload_flag,
            };
            let words = decode_preview_archive_entry_words(&payload, entry).unwrap();
            assert_eq!(&*words, &expected);
            let image = decode_pixel_preview_words(entry, words).unwrap();
            match image {
                PreviewPixels::Rgb565 { words, .. } => assert_eq!(&*words, &expected),
            }
        }
    }

    #[test]
    fn raw565_preview_header_rejects_zero_dimensions() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MM56501\0");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.resize(20 + 16, 0);

        let err = decode_raw565_preview_bytes(&bytes)
            .expect_err("zero-width raw565 preview must be rejected");

        assert!(err.contains("dimensions"), "unexpected error: {err}");
    }

    #[test]
    fn raw565_padding_is_16_byte_aligned_and_zeroed() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MM56501\0");
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        for _ in 0..2 {
            for _ in 0..5 {
                bytes.extend_from_slice(&0xffffu16.to_le_bytes());
            }
            bytes.extend_from_slice(&[0; 6]);
        }
        assert_eq!(&bytes[..8], b"MM56501\0");
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 5);
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 16);
        assert_eq!(bytes.len(), 20 + 16 * 2);
        assert!(bytes[20 + 10..20 + 16].iter().all(|b| *b == 0));
        assert!(bytes[20 + 16 + 10..20 + 32].iter().all(|b| *b == 0));
    }

    #[test]
    fn pixel_preview_rejects_odd_stride_before_word_conversion() {
        let entry = PreviewArchiveEntry {
            raw_len: 3,
            compressed_len: 3,
            offset: 0,
            width: 1,
            height: 1,
            stride_bytes: 3,
            payload_flag: 1,
        };

        let err = decode_pixel_preview_bytes(entry, &[0, 0, 0])
            .expect_err("odd RGB565 stride must be rejected");

        assert!(err.contains("bad stride"), "unexpected error: {err}");
    }

    #[test]
    fn raw565_word_conversion_rejects_odd_byte_length() {
        let err = raw565_words_from_le_bytes(&[0xaa, 0xbb, 0xcc], "fixture")
            .expect_err("odd byte length must be rejected");

        assert!(err.contains("odd byte length"), "unexpected error: {err}");
    }

    #[test]
    fn preview_archive_open_rejects_oversized_entry_count_before_allocating() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-too-many-{}.mmlz4b",
            std::process::id()
        ));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(V2_PIXELS_MAGIC);
        bytes.extend_from_slice(&((MAX_PREVIEW_ARCHIVE_ENTRIES as u32) + 1).to_le_bytes());
        std::fs::write(&path, bytes).expect("write oversized count archive fixture");

        let err = match PreviewArchive::open(&path) {
            Ok(_) => panic!("oversized count must fail"),
            Err(err) => err,
        };

        assert!(err.contains("max"), "unexpected error: {err}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_archive_open_rejects_oversized_raw_len_before_allocating() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-too-large-{}.mmlz4b",
            std::process::id()
        ));
        let name = "huge.rgb565";
        let raw_len = MAX_PREVIEW_ARCHIVE_RAW_BYTES + 2;
        let index_len = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(V2_PIXELS_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(raw_len as u32).to_le_bytes());
        bytes.extend_from_slice(&(raw_len as u32).to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&(index_len as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        std::fs::write(&path, bytes).expect("write oversized raw length archive fixture");

        let err = match PreviewArchive::open(&path) {
            Ok(_) => panic!("oversized raw len must fail"),
            Err(err) => err,
        };

        assert!(err.contains("too large"), "unexpected error: {err}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_archive_sidecar_rejects_oversized_entry_count_before_allocating() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-too-many-{}.mmlz4b",
            std::process::id()
        ));
        write_lz4_block_archive(&path, "tiny.rgb565", &raw565_fixture(1, 1, &[0xf800]));
        let archive_bytes = std::fs::metadata(&path).unwrap().len();
        let index_path = preview_archive_sidecar_path(&path);
        let mut index = Vec::new();
        index.extend_from_slice(b"MMIDX02\0");
        index.extend_from_slice(&archive_bytes.to_le_bytes());
        index
            .extend_from_slice(b"0000000000000000000000000000000000000000000000000000000000000000");
        index.extend_from_slice(&((MAX_PREVIEW_ARCHIVE_ENTRIES as u32) + 1).to_le_bytes());
        std::fs::write(&index_path, index).expect("write oversized sidecar count fixture");

        let err = read_preview_archive_sidecar_index(&index_path, archive_bytes)
            .expect_err("oversized sidecar count must fail");

        assert!(err.contains("max"), "unexpected error: {err}");
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_archive_sidecar_rejects_oversized_encoded_len_before_allocating() {
        let index_path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-huge-encoded-{}.mmlz4b.idx",
            std::process::id()
        ));
        let name = "tiny.rgb565";
        let raw_len = 16usize;
        let compressed_len = 1024usize;
        let offset = 4096u64;
        let archive_bytes = offset + compressed_len as u64;
        let mut index = Vec::new();
        index.extend_from_slice(b"MMIDX02\0");
        index.extend_from_slice(&archive_bytes.to_le_bytes());
        index
            .extend_from_slice(b"0000000000000000000000000000000000000000000000000000000000000000");
        index.extend_from_slice(&1u32.to_le_bytes());
        index.extend_from_slice(&(name.len() as u16).to_le_bytes());
        index.extend_from_slice(&1u32.to_le_bytes());
        index.extend_from_slice(&1u32.to_le_bytes());
        index.extend_from_slice(&16u32.to_le_bytes());
        index.extend_from_slice(&(raw_len as u32).to_le_bytes());
        index.push(0);
        index.extend_from_slice(&(compressed_len as u32).to_le_bytes());
        index.extend_from_slice(&offset.to_le_bytes());
        index.extend_from_slice(name.as_bytes());
        std::fs::write(&index_path, index).expect("write huge encoded sidecar fixture");

        let err = read_preview_archive_sidecar_index(&index_path, archive_bytes)
            .expect_err("oversized encoded length must fail before allocation");

        assert!(
            err.contains("encoded length too large"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_file(index_path);
    }

    #[test]
    fn preview_archive_sidecar_rejects_geometry_mismatch_before_decode() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-bad-geometry-{}.mmlz4b",
            std::process::id()
        ));
        let name = "tiny.rgb565";
        let payload = raw565_fixture(1, 1, &[0xf800]);
        write_lz4_block_archive(&path, name, &payload);
        let archive_bytes = std::fs::metadata(&path).unwrap().len();
        let pixel_len = payload.len() - 20;
        let archive_payload_offset = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let index_path = preview_archive_sidecar_path(&path);
        let mut index = Vec::new();
        index.extend_from_slice(b"MMIDX02\0");
        index.extend_from_slice(&archive_bytes.to_le_bytes());
        index
            .extend_from_slice(b"0000000000000000000000000000000000000000000000000000000000000000");
        index.extend_from_slice(&1u32.to_le_bytes());
        index.extend_from_slice(&(name.len() as u16).to_le_bytes());
        index.extend_from_slice(&1u32.to_le_bytes());
        index.extend_from_slice(&1u32.to_le_bytes());
        index.extend_from_slice(&16u32.to_le_bytes());
        index.extend_from_slice(&((pixel_len as u32) + 2).to_le_bytes());
        index.push(1);
        index.extend_from_slice(&(pixel_len as u32).to_le_bytes());
        index.extend_from_slice(&(archive_payload_offset as u64).to_le_bytes());
        index.extend_from_slice(name.as_bytes());
        std::fs::write(&index_path, index).expect("write bad geometry sidecar fixture");

        let err = read_preview_archive_sidecar_index(&index_path, archive_bytes)
            .expect_err("sidecar geometry mismatch must fail");

        assert!(err.contains("bad geometry"), "unexpected error: {err}");
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_archive_sidecar_rejects_trailing_bytes() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-trailing-{}.mmlz4b",
            std::process::id()
        ));
        write_lz4_block_archive_with_index(&path, "tiny.rgb565", &raw565_fixture(1, 1, &[0xf800]));
        let archive_bytes = std::fs::metadata(&path).unwrap().len();
        let index_path = preview_archive_sidecar_path(&path);
        let mut index = std::fs::read(&index_path).expect("read generated sidecar");
        index.extend_from_slice(b"garbage");
        std::fs::write(&index_path, index).expect("write sidecar with trailing bytes");

        let err = read_preview_archive_sidecar_index(&index_path, archive_bytes)
            .expect_err("sidecar trailing bytes must fail");

        assert!(err.contains("trailing"), "unexpected error: {err}");
        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_archive_asset_fails_without_original_decode_fallback() {
        let mut scratch = PreviewArchiveScratch::default();
        let err = load_preview_pixels(
            "/tmp/missing/320x320-screenshots.mmlz4b",
            "tiny",
            &mut scratch,
            PreviewResizeSpec {
                filter: PreviewResizeFilter::Hybrid,
                max_w: 320,
                max_h: 320,
            },
        )
        .expect_err("missing archive must not decode original screenshots");
        assert!(err.contains("320x320-screenshots.mmlz4b"));
    }

    #[test]
    fn missing_archive_failure_is_cached_by_archive_path() {
        let archive_path = std::env::temp_dir().join(format!(
            "mister-magik-missing-preview-{}.mmlz4b",
            std::process::id()
        ));
        let archive_path = archive_path.display().to_string();
        let resize = PreviewResizeSpec {
            filter: PreviewResizeFilter::Hybrid,
            max_w: 320,
            max_h: 320,
        };
        let mut scratch = PreviewArchiveScratch::default();

        let first = load_preview_pixels(&archive_path, "first", &mut scratch, resize)
            .expect_err("first missing archive request should fail");
        let calls_after_first = preview_archive_metadata_calls(&archive_path);
        let second = load_preview_pixels(&archive_path, "second", &mut scratch, resize)
            .expect_err("second missing archive request should fail from archive cache");

        assert!(first.contains(&archive_path));
        assert!(second.contains(&archive_path));
        assert_eq!(calls_after_first, 1);
        assert_eq!(
            preview_archive_metadata_calls(&archive_path),
            calls_after_first
        );
    }

    #[test]
    fn load_preview_reads_requested_archive_asset_and_reports_dimensions() {
        let archive_path = std::env::temp_dir().join(format!(
            "mister-magik-preview-load-{}.mmlz4b",
            std::process::id()
        ));
        write_lz4_block_archive(
            &archive_path,
            "tiny.rgb565",
            &raw565_fixture(3, 1, &[0xf800, 0x07e0, 0x001f]),
        );

        let req = preview_request(
            88,
            "Tiny",
            &archive_path.display().to_string(),
            "tiny",
            PreviewPriority::Selected,
        );
        let cache = Arc::new(Mutex::new(PreviewDecodedCache::new(2)));
        let mut scratch = PreviewArchiveScratch::default();
        let result = load_preview(
            req,
            &cache,
            &mut scratch,
            Instant::now(),
            &PreviewWorkerConfig::default(),
        );

        assert_eq!(result.generation, 88);
        assert_eq!(result.title, "Tiny");
        assert_eq!(
            result.preview_archive_path,
            archive_path.display().to_string()
        );
        assert_eq!(result.preview_asset_key, "tiny");
        assert_eq!(result.source_width, 3);
        assert_eq!(result.source_height, 1);
        assert_eq!(result.decoded_bytes, 16);
        assert_eq!(result.storage_format.label(), "raw-rgb565");
        assert_eq!(result.resize_filter, PreviewResizeFilter::Hybrid);
        let image = result.image.expect("decoded image");
        assert_eq!(image.width(), 3);
        assert_eq!(image.height(), 1);
        assert_eq!(image.decoded_bytes(), 16);
        let _ = std::fs::remove_file(archive_path);
    }

    #[test]
    fn preview_worker_decodes_selected_request_on_background_thread() {
        let archive_path = std::env::temp_dir().join(format!(
            "mister-magik-preview-worker-{}.mmlz4b",
            std::process::id()
        ));
        write_lz4_block_archive(
            &archive_path,
            "selected.rgb565",
            &raw565_fixture(2, 1, &[0xf800, 0x07e0]),
        );
        let mut worker = PreviewWorker::new();

        let generation = worker.request_selected(
            "Selected".to_string(),
            archive_path.display().to_string(),
            "selected".to_string(),
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let result = loop {
            if let Some(result) = worker.take_latest_selected_result() {
                break result;
            }
            assert!(
                Instant::now() < deadline,
                "preview worker did not return a result"
            );
            std::thread::sleep(Duration::from_millis(10));
        };

        assert_eq!(result.generation, generation);
        assert_eq!(result.title, "Selected");
        assert_eq!(result.priority, PreviewPriority::Selected);
        assert_eq!(result.load_source, PreviewLoadSource::ArchiveMem);
        assert_eq!(result.source_width, 2);
        assert_eq!(result.source_height, 1);
        assert!(result.image.is_some());
        let _ = std::fs::remove_file(archive_path);
    }

    #[test]
    fn preview_archive_cache_reopens_when_file_fingerprint_changes() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-refresh-{}.mmlz4b",
            std::process::id()
        ));
        write_lz4_block_archive(&path, "old.rgb565", &raw565_fixture(1, 1, &[0xf800]));
        // Cache behavior tests need private state because the production cache is
        // intentionally shared and other Rust tests run in parallel.
        let cache = Mutex::new(None);
        let missing_cache = Mutex::new(Vec::new());
        let first = preview_archives_for_paths_with_cache(
            vec![path.display().to_string()],
            &cache,
            &missing_cache,
        )
        .expect("open first archive")
        .expect("first archive");

        write_lz4_block_archive(
            &path,
            "new-long.rgb565",
            &raw565_fixture(2, 1, &[0x07e0, 0x001f]),
        );
        let expired_at = Instant::now() - PREVIEW_ARCHIVE_METADATA_TTL - Duration::from_millis(1);
        cache
            .lock()
            .expect("archive cache")
            .as_mut()
            .expect("cached archive")
            .checked_at = expired_at;
        let second = preview_archives_for_paths_with_cache(
            vec![path.display().to_string()],
            &cache,
            &missing_cache,
        )
        .expect("open changed archive")
        .expect("changed archive");

        assert!(!Arc::ptr_eq(&first, &second));
        let mut scratch = PreviewArchiveScratch::default();
        assert!(
            second[0]
                .load_timed("new-long.rgb565", &mut scratch)
                .expect("load from changed archive")
                .is_some()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_archive_cache_hit_avoids_metadata_inside_ttl() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-ttl-hit-{}.mmlz4b",
            std::process::id()
        ));
        let path_text = path.display().to_string();
        write_lz4_block_archive(&path, "tiny.rgb565", &raw565_fixture(1, 1, &[0xf800]));

        // Keep another preview test from replacing the process-global cache
        // between the two lookups this assertion is intended to compare.
        let cache = Mutex::new(None);
        let missing_cache = Mutex::new(Vec::new());
        let first =
            preview_archives_for_paths_with_cache(vec![path_text.clone()], &cache, &missing_cache)
                .expect("open archive")
                .expect("archive");
        let calls_after_first = preview_archive_metadata_calls(&path_text);
        let second =
            preview_archives_for_paths_with_cache(vec![path_text.clone()], &cache, &missing_cache)
                .expect("reuse archive")
                .expect("archive");

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            preview_archive_metadata_calls(&path_text),
            calls_after_first
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sidecar_index_cache_hit_avoids_metadata_inside_ttl_until_invalidated() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-sidecar-ttl-{}.mmlz4b",
            std::process::id()
        ));
        let name = "tiny.rgb565";
        write_lz4_block_archive_with_index(&path, name, &raw565_fixture(1, 1, &[0xf800]));
        let path_text = path.display().to_string();
        let index_path = preview_archive_sidecar_path(&path);
        let index_path_text = index_path.display().to_string();

        let first = preview_archive_sidecar_index(&path)
            .expect("first sidecar lookup")
            .expect("sidecar index");
        let archive_calls_after_first = preview_archive_metadata_calls(&path_text);
        let index_calls_after_first = preview_archive_metadata_calls(&index_path_text);

        let second = preview_archive_sidecar_index(&path)
            .expect("second sidecar lookup")
            .expect("sidecar index");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            preview_archive_metadata_calls(&path_text),
            archive_calls_after_first
        );
        assert_eq!(
            preview_archive_metadata_calls(&index_path_text),
            index_calls_after_first
        );

        let resolved_cache_key = format!("test-resolved-{path_text}");
        resolved_preview_archive_path_cache()
            .lock()
            .unwrap()
            .insert(resolved_cache_key.clone(), path_text.clone());
        invalidate_preview_archive_metadata_cache("test_media_generation_changed");
        assert!(
            !resolved_preview_archive_path_cache()
                .lock()
                .unwrap()
                .contains_key(&resolved_cache_key)
        );
        let third = preview_archive_sidecar_index(&path)
            .expect("reload after invalidation")
            .expect("sidecar index");
        assert!(!Arc::ptr_eq(&second, &third));
        assert!(preview_archive_metadata_calls(&path_text) > archive_calls_after_first);
        assert!(preview_archive_metadata_calls(&index_path_text) > index_calls_after_first);

        let _ = std::fs::remove_file(index_path);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn index_pread_reuses_open_archive_file_until_fingerprint_changes() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-index-pread-open-cache-{}.mmlz4b",
            std::process::id()
        ));
        let name = "tiny.rgb565";
        write_lz4_block_archive_with_index(&path, name, &raw565_fixture(1, 1, &[0xf800]));
        let path_text = path.display().to_string();
        let mut scratch = PreviewArchiveScratch::default();

        assert!(matches!(
            load_raw565_preview_asset_from_index(&path, name, &mut scratch)
                .expect("first index pread"),
            PreviewIndexLoad::Loaded(_)
        ));
        assert!(matches!(
            load_raw565_preview_asset_from_index(&path, name, &mut scratch)
                .expect("second index pread"),
            PreviewIndexLoad::Loaded(_)
        ));
        assert_eq!(index_pread_archive_open_calls(&path_text), 1);

        write_lz4_block_archive_with_index(&path, name, &raw565_fixture(2, 1, &[0x07e0, 0x001f]));
        expire_preview_archive_metadata_cache_for_tests();
        assert!(matches!(
            load_raw565_preview_asset_from_index(&path, name, &mut scratch)
                .expect("changed index pread"),
            PreviewIndexLoad::Loaded(_)
        ));
        assert_eq!(index_pread_archive_open_calls(&path_text), 2);

        let _ = std::fs::remove_file(preview_archive_sidecar_path(&path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_worker_async_load_reuses_index_pread_archive_file() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-worker-sync-open-cache-{}.mmlz4b",
            std::process::id()
        ));
        let name = "tiny.rgb565";
        write_lz4_block_archive_with_index(&path, name, &raw565_fixture(1, 1, &[0xf800]));
        let path_text = path.display().to_string();
        let mut worker = PreviewWorker::new();

        for generation in 0..2 {
            worker.request_selected("tiny".to_string(), path_text.clone(), "tiny".to_string());
            let deadline = Instant::now() + Duration::from_secs(2);
            while worker.take_latest_selected_result().is_none() {
                assert!(
                    Instant::now() < deadline,
                    "async selected load {generation} did not complete"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        assert_eq!(index_pread_archive_open_calls(&path_text), 1);

        let _ = std::fs::remove_file(preview_archive_sidecar_path(&path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn lz4_block_archive_rejects_entry_length_mismatch() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-bad-length-{}.mmlz4b",
            std::process::id()
        ));
        let payload = raw565_fixture(1, 1, &[0xf800]);
        write_lz4_block_archive_with_lengths(
            &path,
            "bad.rgb565",
            &payload,
            payload.len() - 20,
            payload.len() - 21,
        );
        let err = match PreviewArchive::open(&path) {
            Ok(_) => panic!("raw pixel length mismatch should fail early"),
            Err(err) => err,
        };

        assert!(
            err.contains("raw payload length mismatch"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn resize_filters_parse_aliases_and_default_to_off() {
        assert_eq!(
            PreviewResizeFilter::from_label(" nearest_neighbor\n"),
            PreviewResizeFilter::Nearest
        );
        assert_eq!(
            PreviewResizeFilter::from_label("box-area"),
            PreviewResizeFilter::Box
        );
        assert_eq!(
            PreviewResizeFilter::from_label("LANCZOS3"),
            PreviewResizeFilter::Lanczos
        );
        assert_eq!(
            PreviewResizeFilter::from_label("hybrid_arcade"),
            PreviewResizeFilter::Hybrid
        );
        assert_eq!(
            PreviewResizeFilter::from_label("unknown"),
            PreviewResizeFilter::Off
        );
    }

    #[test]
    fn resize_spec_cache_labels_include_filter_and_bounds() {
        let off = PreviewResizeSpec::off();
        assert_eq!(off.cache_label(), "off-0x0");

        let spec = PreviewResizeSpec {
            filter: PreviewResizeFilter::Lanczos,
            max_w: 320,
            max_h: 240,
        };
        assert_eq!(spec.cache_label(), "lanczos-320x240");
        assert_eq!(parse_size("640x480"), Some((640, 480)));
        assert_eq!(parse_size("640X480"), Some((640, 480)));
        assert_eq!(parse_size("640,480"), None);
    }

    fn raw565_fixture(width: u32, height: u32, pixels: &[u16]) -> Vec<u8> {
        let stride_bytes = ((width * 2) as usize).next_multiple_of(16) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MM56501\0");
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&stride_bytes.to_le_bytes());
        for row in 0..height as usize {
            let row_start = row * width as usize;
            let row_end = row_start + width as usize;
            for pixel in &pixels[row_start..row_end] {
                bytes.extend_from_slice(&pixel.to_le_bytes());
            }
            bytes.resize(20 + (row + 1) * stride_bytes as usize, 0);
        }
        bytes
    }

    fn write_lz4_block_archive(path: &Path, name: &str, payload: &[u8]) {
        assert_eq!(&payload[..8], b"MM56501\0");
        let width = u32::from_le_bytes(payload[8..12].try_into().unwrap());
        let height = u32::from_le_bytes(payload[12..16].try_into().unwrap());
        let stride_bytes = u32::from_le_bytes(payload[16..20].try_into().unwrap());
        let pixels = &payload[20..];
        let index_len = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(V2_PIXELS_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&stride_bytes.to_le_bytes());
        bytes.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(index_len as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(pixels);
        std::fs::write(path, bytes).expect("write v2 pixel archive fixture");
    }

    fn write_lz4_block_archive_with_index(path: &Path, name: &str, payload: &[u8]) {
        write_lz4_block_archive(path, name, payload);
        let archive_bytes = std::fs::metadata(path).unwrap().len();
        let width = u32::from_le_bytes(payload[8..12].try_into().unwrap());
        let height = u32::from_le_bytes(payload[12..16].try_into().unwrap());
        let stride_bytes = u32::from_le_bytes(payload[16..20].try_into().unwrap());
        let pixel_len = payload.len() - 20;
        let index_len = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let mut index = Vec::new();
        index.extend_from_slice(b"MMIDX02\0");
        index.extend_from_slice(&archive_bytes.to_le_bytes());
        index
            .extend_from_slice(b"0000000000000000000000000000000000000000000000000000000000000000");
        index.extend_from_slice(&1u32.to_le_bytes());
        index.extend_from_slice(&(name.len() as u16).to_le_bytes());
        index.extend_from_slice(&width.to_le_bytes());
        index.extend_from_slice(&height.to_le_bytes());
        index.extend_from_slice(&stride_bytes.to_le_bytes());
        index.extend_from_slice(&(pixel_len as u32).to_le_bytes());
        index.push(1);
        index.extend_from_slice(&(pixel_len as u32).to_le_bytes());
        index.extend_from_slice(&(index_len as u64).to_le_bytes());
        index.extend_from_slice(name.as_bytes());
        std::fs::write(preview_archive_sidecar_path(path), index)
            .expect("write lz4 block archive index fixture");
    }

    fn write_duplicate_lz4_block_archive_index(path: &Path, name: &str, payload_len: usize) {
        let archive_bytes = std::fs::metadata(path).unwrap().len();
        let archive_payload_offset = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let mut index = Vec::new();
        index.extend_from_slice(b"MMIDX02\0");
        index.extend_from_slice(&archive_bytes.to_le_bytes());
        index
            .extend_from_slice(b"0000000000000000000000000000000000000000000000000000000000000000");
        index.extend_from_slice(&2u32.to_le_bytes());
        for _ in 0..2 {
            index.extend_from_slice(&(name.len() as u16).to_le_bytes());
            index.extend_from_slice(&1u32.to_le_bytes());
            index.extend_from_slice(&1u32.to_le_bytes());
            index.extend_from_slice(&16u32.to_le_bytes());
            index.extend_from_slice(&(payload_len as u32).to_le_bytes());
            index.push(1);
            index.extend_from_slice(&(payload_len as u32).to_le_bytes());
            index.extend_from_slice(&(archive_payload_offset as u64).to_le_bytes());
            index.extend_from_slice(name.as_bytes());
        }
        std::fs::write(preview_archive_sidecar_path(path), index)
            .expect("write duplicate lz4 block archive index fixture");
    }

    fn write_lz4_block_archive_with_lengths(
        path: &Path,
        name: &str,
        payload: &[u8],
        raw_len: usize,
        compressed_len: usize,
    ) {
        assert_eq!(&payload[..8], b"MM56501\0");
        let width = u32::from_le_bytes(payload[8..12].try_into().unwrap());
        let height = u32::from_le_bytes(payload[12..16].try_into().unwrap());
        let stride_bytes = u32::from_le_bytes(payload[16..20].try_into().unwrap());
        let pixels = &payload[20..];
        let index_len = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(V2_PIXELS_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&stride_bytes.to_le_bytes());
        bytes.extend_from_slice(&(raw_len as u32).to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&(compressed_len as u32).to_le_bytes());
        bytes.extend_from_slice(&(index_len as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(pixels);
        std::fs::write(path, bytes).expect("write lz4 block archive fixture");
    }
}
