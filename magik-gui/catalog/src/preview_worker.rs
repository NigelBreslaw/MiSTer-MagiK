//! Background arcade preview image loader.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Instant, UNIX_EPOCH};

pub const DEFAULT_PREVIEW_RADIUS: usize = 12;
pub const DEFAULT_PREVIEW_CACHE_CAP: usize = DEFAULT_PREVIEW_RADIUS * 2 + 1;

#[derive(Clone, Debug)]
pub struct PreviewRequest {
    pub generation: u64,
    pub title: String,
    pub preview_archive_path: String,
    pub preview_asset_key: String,
    pub requested_at: Instant,
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
    pub request_age_us: u64,
    pub read_us: u64,
    pub decode_us: u64,
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
        let filter = std::env::var("MISTER_PREVIEW_RESIZE_FILTER")
            .or_else(|_| std::env::var("MISTER_PREVIEW_RESIZE"))
            .ok()
            .map(|s| PreviewResizeFilter::from_label(&s))
            .unwrap_or(PreviewResizeFilter::Hybrid);
        if filter == PreviewResizeFilter::Off {
            return Self::off();
        }
        let (max_w, max_h) = std::env::var("MISTER_PREVIEW_RESIZE_MAX")
            .ok()
            .and_then(|s| parse_size(&s))
            .unwrap_or((320, 320));
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
}

impl PreviewLoadSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::DecodedCache => "decoded_cache",
            Self::ArchiveMem => "archive_mem",
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

pub struct PreviewWorker {
    tx: mpsc::Sender<PreviewCommand>,
    rx: mpsc::Receiver<PreviewResult>,
    next_generation: u64,
}

#[derive(Clone, Debug)]
enum PreviewCommand {
    Request(PreviewRequest),
}

impl Default for PreviewWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewWorker {
    pub fn new() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<PreviewCommand>();
        let (res_tx, res_rx) = mpsc::channel::<PreviewResult>();
        std::thread::Builder::new()
            .name("preview-loader".to_string())
            .spawn(move || preview_thread(req_rx, res_tx))
            .expect("spawn preview-loader");
        Self {
            tx: req_tx,
            rx: res_rx,
            next_generation: 1,
        }
    }

    pub fn request_selected(
        &mut self,
        title: String,
        preview_archive_path: String,
        preview_asset_key: String,
    ) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        let _ = self.tx.send(PreviewCommand::Request(PreviewRequest {
            generation,
            title,
            preview_archive_path,
            preview_asset_key,
            requested_at: Instant::now(),
            priority: PreviewPriority::Selected,
        }));
        generation
    }

    pub fn request_prefetch(
        &mut self,
        title: String,
        preview_archive_path: String,
        preview_asset_key: String,
        distance: usize,
    ) {
        let generation = self.next_generation;
        self.next_generation += 1;
        let _ = self.tx.send(PreviewCommand::Request(PreviewRequest {
            generation,
            title,
            preview_archive_path,
            preview_asset_key,
            requested_at: Instant::now(),
            priority: PreviewPriority::Prefetch { distance },
        }));
    }

    pub fn drain(&self) -> Vec<PreviewResult> {
        let mut out = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            out.push(result);
        }
        out
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
        if let Some(p) = path(&items[idx]) {
            if seen.insert(p) {
                out.push(p);
            }
        }
    }
    out
}

fn preview_thread(rx: mpsc::Receiver<PreviewCommand>, tx: mpsc::Sender<PreviewResult>) {
    lower_thread_priority();
    let _ = preview_archives();
    let mut queue: Vec<PreviewRequest> = Vec::new();
    let mut decoded_cache = PreviewDecodedCache::new(preview_decoded_cache_cap());
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(command) => enqueue_command(&mut queue, command),
                Err(_) => break,
            }
        }
        while let Ok(command) = rx.try_recv() {
            enqueue_command(&mut queue, command);
        }
        if let Some(req) = pop_next_preview_request(&mut queue) {
            let result = load_preview(req, &mut decoded_cache);
            if tx.send(result).is_err() {
                break;
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

fn preview_decoded_cache_cap() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| {
        std::env::var("MISTER_PREVIEW_DECODED_CACHE_CAP")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(DEFAULT_PREVIEW_CACHE_CAP * 3)
    })
}

fn pop_next_preview_request(queue: &mut Vec<PreviewRequest>) -> Option<PreviewRequest> {
    queue.sort_by_key(|req| (req.priority.rank(), req.requested_at));
    if queue.is_empty() {
        None
    } else {
        Some(queue.remove(0))
    }
}

fn enqueue_command(queue: &mut Vec<PreviewRequest>, command: PreviewCommand) {
    match command {
        PreviewCommand::Request(req) => {
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
                    || req.priority.rank() <= DEFAULT_PREVIEW_CACHE_CAP
            });
        }
    }
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

fn load_preview(req: PreviewRequest, decoded_cache: &mut PreviewDecodedCache) -> PreviewResult {
    let resize = PreviewResizeSpec::from_env();
    let storage = PreviewStorageFormat::from_env();
    let cache_key = preview_cache_key(&req.preview_archive_path, &req.preview_asset_key, resize);
    let mut cache_hit = false;
    let loaded_result = if let Some(loaded) = decoded_cache.get(&cache_key) {
        cache_hit = true;
        Ok(loaded)
    } else {
        load_preview_pixels(&req.preview_archive_path, &req.preview_asset_key, resize).inspect(|loaded| {
            decoded_cache.insert(cache_key, loaded);
        })
    };
    match loaded_result {
        Ok(loaded) => {
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace decoded generation={} priority={:?} cache_hit={} load_source={} format={} filter={} source={}x{} output={}x{} total_us={} read_us={} decode_us={} resize_us={} encoded_bytes={} decoded_bytes={} archive_path={} asset_key={}",
                    req.generation,
                    req.priority,
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
                    loaded.timing.resize_us,
                    loaded.timing.encoded_bytes,
                    loaded.image.decoded_bytes(),
                    req.preview_archive_path,
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
                request_age_us: req.requested_at.elapsed().as_micros() as u64,
                read_us: loaded.timing.read_us,
                decode_us: loaded.timing.decode_us,
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
                eprintln!(
                    "preview_trace decode_failed generation={} age_us={} archive_path={} asset_key={} error={}",
                    req.generation,
                    req.requested_at.elapsed().as_micros(),
                    req.preview_archive_path,
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
                request_age_us: req.requested_at.elapsed().as_micros() as u64,
                read_us: 0,
                decode_us: 0,
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

#[derive(Clone, Debug)]
struct LoadedPreviewPixels {
    timing: ImageLoadTiming,
    image: PreviewPixels,
}

fn load_preview_pixels(
    preview_archive_path: &str,
    preview_asset_key: &str,
    resize: PreviewResizeSpec,
) -> Result<LoadedPreviewPixels, String> {
    let _ = resize;
    load_raw565_preview_asset_timed(preview_archive_path, preview_asset_key)
}

pub fn load_preview_asset_pixels(
    preview_archive_path: &str,
    preview_asset_key: &str,
) -> Result<PreviewPixels, String> {
    load_raw565_preview_asset_timed(preview_archive_path, preview_asset_key)
        .map(|loaded| loaded.image)
}

fn load_raw565_preview_asset_timed(
    preview_archive_path: &str,
    preview_asset_key: &str,
) -> Result<LoadedPreviewPixels, String> {
    let archive_path = preview_archive_path.trim();
    let asset_key = preview_asset_key.trim();
    if archive_path.is_empty() || asset_key.is_empty() {
        return Err("preview asset missing archive path or key".to_string());
    }
    let entry_name = format!("{asset_key}.rgb565");
    let Some(archives) = preview_archives_for_paths(vec![archive_path.to_string()])? else {
        return Err(format!("preview archive not configured {archive_path}"));
    };
    for archive in archives.iter() {
        if let Some(loaded) = archive.load_timed(&entry_name)? {
            return Ok(loaded);
        }
    }
    Err(format!(
        "preview asset {entry_name} missing from archive {archive_path}"
    ))
}

#[derive(Clone, Copy, Debug)]
struct PreviewArchiveEntry {
    raw_len: usize,
    compressed_len: usize,
    offset: u64,
}

struct PreviewArchive {
    scratch: Mutex<PreviewArchiveScratch>,
    codec: PreviewArchiveCodec,
    bytes: Arc<[u8]>,
    entries: HashMap<String, PreviewArchiveEntry>,
}

#[derive(Clone, Copy, Debug)]
enum PreviewArchiveCodec {
    Lz4Block,
    Raw,
}

#[derive(Default)]
struct PreviewArchiveScratch {
    raw: Vec<u8>,
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
}

fn preview_archives() -> Result<Option<Arc<Vec<PreviewArchive>>>, String> {
    preview_archives_for_paths(preview_archive_paths_from_env())
}

fn preview_archives_for_paths(paths: Vec<String>) -> Result<Option<Arc<Vec<PreviewArchive>>>, String> {
    static ARCHIVES: OnceLock<Mutex<Option<CachedPreviewArchives>>> = OnceLock::new();

    let cache = ARCHIVES.get_or_init(|| Mutex::new(None));
    if paths.is_empty() {
        if let Ok(mut cached) = cache.lock() {
            *cached = None;
        }
        return Ok(None);
    }

    let fingerprints = preview_archive_fingerprints_for_paths(paths)?;
    if let Ok(cached) = cache.lock() {
        if let Some(cached) = cached.as_ref() {
            if cached.fingerprints == fingerprints {
                return Ok(Some(Arc::clone(&cached.archives)));
            }
        }
    }

    let mut archives = Vec::with_capacity(fingerprints.len());
    for fingerprint in &fingerprints {
        archives.push(PreviewArchive::open(Path::new(&fingerprint.path))?);
    }
    let archives = Arc::new(archives);
    if let Ok(mut cached) = cache.lock() {
        *cached = Some(CachedPreviewArchives {
            fingerprints,
            archives: Arc::clone(&archives),
        });
    }
    Ok(Some(archives))
}

fn preview_archive_paths_from_env() -> Vec<String> {
    let mut paths = Vec::new();
    if let Ok(value) = std::env::var("MISTER_PREVIEW_ARCHIVES") {
        for path in value
            .split(':')
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            paths.push(path.to_string());
        }
    }
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
    Ok(preview_archive_fingerprints_for_paths(preview_archive_paths_from_env())?
        .into_iter()
        .map(|fingerprint| (fingerprint.path, fingerprint.size, fingerprint.mtime_secs))
        .collect())
}

fn preview_archive_fingerprints_for_paths(
    paths: Vec<String>,
) -> Result<Vec<PreviewArchiveFingerprint>, String> {
    paths
        .into_iter()
        .map(|path| {
            let meta = std::fs::metadata(&path)
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

#[cfg(test)]
fn preview_archive_entry_stems(path: &Path) -> Result<HashSet<String>, String> {
    Ok(preview_archive_index(path)?.entries.into_iter().collect())
}

pub fn preview_archive_index(path: &Path) -> Result<PreviewArchiveIndex, String> {
    let mut file =
        File::open(path).map_err(|e| format!("open preview archive {}: {e}", path.display()))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)
        .map_err(|e| format!("read preview archive magic {}: {e}", path.display()))?;
    let codec = match &magic {
        PreviewArchive::LZ4_BLOCK_MAGIC => "lz4-block",
        PreviewArchive::RAW_MAGIC => "raw",
        _ => return Err(format!("{}: bad preview archive magic", path.display())),
    };
    let count = read_u32(&mut file)? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = read_u16(&mut file)? as usize;
        let _raw_len = read_u32(&mut file)?;
        let _compressed_len = read_u32(&mut file)?;
        let _offset = read_u64(&mut file)?;
        let mut name = vec![0u8; name_len];
        file.read_exact(&mut name)
            .map_err(|e| format!("read preview archive entry name: {e}"))?;
        let name =
            String::from_utf8(name).map_err(|e| format!("preview archive entry name utf8: {e}"))?;
        if let Some(stem) = Path::new(&name).file_stem().and_then(|s| s.to_str()) {
            entries.push(stem.to_ascii_lowercase());
        }
    }
    entries.sort();
    entries.dedup();
    Ok(PreviewArchiveIndex {
        path: path.display().to_string(),
        codec,
        entries,
    })
}

fn auto_preview_archive_path() -> Option<String> {
    if matches!(
        std::env::var("MISTER_PREVIEW_ARCHIVE_AUTO").as_deref(),
        Ok("0") | Ok("off") | Ok("false") | Ok("no")
    ) {
        return None;
    }
    let resize = PreviewResizeSpec::from_env();
    let root = std::env::var("MISTER_PREVIEW_CACHE_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/media/fat/_Arcade/media/screenshot-magik"));
    auto_preview_archive_path_in_root(&root, resize)
}

fn auto_preview_archive_path_in_root(root: &Path, resize: PreviewResizeSpec) -> Option<String> {
    let max_w = if resize.max_w == 0 { 320 } else { resize.max_w };
    let max_h = if resize.max_h == 0 { 320 } else { resize.max_h };
    let archive = root.join(format!("{max_w}x{max_h}-screenshots.mmlz4b"));
    archive.exists().then(|| archive.display().to_string())
}

fn neogeo_preview_archive_path_from_env() -> Option<String> {
    match std::env::var("MISTER_NEOGEO_PREVIEW_ARCHIVE") {
        Ok(path) if !path.is_empty() => Some(path),
        _ => None,
    }
}

fn auto_neogeo_archive_path() -> Option<String> {
    let path = PathBuf::from("/media/fat/mister-magik/assets/neogeo-screenshots.mmlz4b");
    path.exists().then(|| path.display().to_string())
}

fn console_preview_archive_paths_from_env() -> Vec<String> {
    [
        "MISTER_NES_PREVIEW_ARCHIVE",
        "MISTER_SNES_PREVIEW_ARCHIVE",
        "MISTER_N64_PREVIEW_ARCHIVE",
        "MISTER_SMS_PREVIEW_ARCHIVE",
        "MISTER_MEGADRIVE_PREVIEW_ARCHIVE",
        "MISTER_SATURN_PREVIEW_ARCHIVE",
    ]
    .into_iter()
    .filter_map(|name| match std::env::var(name) {
        Ok(path) if !path.is_empty() => Some(path),
        _ => None,
    })
    .collect()
}

fn auto_console_archive_paths() -> Vec<String> {
    [
        "nes-screenshots.mmlz4b",
        "snes-screenshots.mmlz4b",
        "n64-screenshots.mmlz4b",
        "sms-screenshots.mmlz4b",
        "megadrive-screenshots.mmlz4b",
        "saturn-screenshots.mmlz4b",
    ]
    .into_iter()
    .filter_map(|name| {
        let path = PathBuf::from("/media/fat/mister-magik/assets").join(name);
        path.exists().then(|| path.display().to_string())
    })
    .collect()
}

impl PreviewArchive {
    const LZ4_BLOCK_MAGIC: &'static [u8; 8] = b"MMLZ4B1\0";
    const RAW_MAGIC: &'static [u8; 8] = b"MMRAWP1\0";

    fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path)
            .map_err(|e| format!("open preview archive {}: {e}", path.display()))?;
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)
            .map_err(|e| format!("read preview archive magic {}: {e}", path.display()))?;
        let codec = match &magic {
            Self::LZ4_BLOCK_MAGIC => PreviewArchiveCodec::Lz4Block,
            Self::RAW_MAGIC => PreviewArchiveCodec::Raw,
            _ => return Err(format!("{}: bad preview archive magic", path.display())),
        };
        let count = read_u32(&mut file)? as usize;
        let mut entries = HashMap::with_capacity(count);
        for _ in 0..count {
            let name_len = read_u16(&mut file)? as usize;
            let raw_len = read_u32(&mut file)? as usize;
            let compressed_len = read_u32(&mut file)? as usize;
            let offset = read_u64(&mut file)?;
            let mut name = vec![0u8; name_len];
            file.read_exact(&mut name)
                .map_err(|e| format!("read preview archive entry name: {e}"))?;
            let name = String::from_utf8(name)
                .map_err(|e| format!("preview archive entry name utf8: {e}"))?;
            entries.insert(
                name.to_ascii_lowercase(),
                PreviewArchiveEntry {
                    raw_len,
                    compressed_len,
                    offset,
                },
            );
        }
        let bytes = Arc::from(read_archive_bytes(path)?.into_boxed_slice());
        Ok(Self {
            bytes,
            scratch: Mutex::new(PreviewArchiveScratch::default()),
            codec,
            entries,
        })
    }

    fn load_timed(&self, name: &str) -> Result<Option<LoadedPreviewPixels>, String> {
        let key = name.to_ascii_lowercase();
        let Some(entry) = self.entries.get(&key).copied() else {
            return Ok(None);
        };
        let total_t = Instant::now();
        let read_t = Instant::now();
        let mut scratch = self
            .scratch
            .lock()
            .map_err(|_| "preview archive scratch lock poisoned".to_string())?;
        let PreviewArchiveScratch { raw } = &mut *scratch;
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
        let data = match self.codec {
            PreviewArchiveCodec::Lz4Block => {
                decode_lz4_block_entry_into(compressed_slice, entry.raw_len, raw)
                    .map_err(|e| format!("preview archive lz4 decode {name}: {e}"))?
            }
            PreviewArchiveCodec::Raw => {
                if compressed_slice.len() != entry.raw_len {
                    return Err(format!(
                        "preview archive raw length mismatch {name}: got={} expected={}",
                        compressed_slice.len(),
                        entry.raw_len
                    ));
                }
                compressed_slice
            }
        };
        let image = decode_raw565_preview_bytes(data)?;
        let decode_us = decode_t.elapsed().as_micros() as u64;
        let total_us = total_t.elapsed().as_micros() as u64;
        Ok(Some(LoadedPreviewPixels {
            timing: ImageLoadTiming {
                read_us,
                decode_us,
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

fn read_archive_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let mut file =
        File::open(path).map_err(|e| format!("preload preview archive {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("read preview archive {}: {e}", path.display()))?;
    Ok(bytes)
}

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
            out.resize(raw_len, 0);
            let len = lz4_flex::block::decompress_into(block, out).map_err(|e| e.to_string())?;
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

fn read_u16(file: &mut File) -> Result<u16, String> {
    let mut buf = [0u8; 2];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32(file: &mut File) -> Result<u32, String> {
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(file: &mut File) -> Result<u64, String> {
    let mut buf = [0u8; 8];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(u64::from_le_bytes(buf))
}

fn decode_raw565_preview_bytes(data: &[u8]) -> Result<PreviewPixels, String> {
    if data.len() < 20 || &data[..8] != b"MM56501\0" {
        return Err("raw565 preview bad header".into());
    }
    let width = u32::from_le_bytes(data[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(data[12..16].try_into().unwrap());
    let stride_bytes = u32::from_le_bytes(data[16..20].try_into().unwrap());
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
    #[cfg(target_endian = "little")]
    let words = {
        let mut words = vec![0; expected / 2];
        unsafe {
            std::ptr::copy_nonoverlapping(
                data[20..].as_ptr(),
                words.as_mut_ptr() as *mut u8,
                expected,
            );
        }
        words
    };
    #[cfg(not(target_endian = "little"))]
    let mut words = Vec::with_capacity(expected / 2);
    #[cfg(not(target_endian = "little"))]
    {
        for chunk in data[20..].chunks_exact(2) {
            words.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
    }
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

fn lower_thread_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "/tmp/320x320-screenshots.mmlz4b",
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
        enqueue_command(
            &mut queue,
            PreviewCommand::Request(queued_request(
                1,
                "prefetch",
                "same",
                PreviewPriority::Prefetch { distance: 3 },
            )),
        );
        enqueue_command(
            &mut queue,
            PreviewCommand::Request(queued_request(
                2,
                "selected",
                "same",
                PreviewPriority::Selected,
            )),
        );
        enqueue_command(
            &mut queue,
            PreviewCommand::Request(queued_request(
                3,
                "too far",
                "far",
                PreviewPriority::Prefetch {
                    distance: DEFAULT_PREVIEW_CACHE_CAP + 1,
                },
            )),
        );

        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].generation, 2);
        assert_eq!(queue[0].priority, PreviewPriority::Selected);
    }

    #[test]
    fn enqueue_selected_supersedes_older_selected_work() {
        let mut queue = Vec::new();
        enqueue_command(
            &mut queue,
            PreviewCommand::Request(queued_request(
                1,
                "old selected",
                "old",
                PreviewPriority::Selected,
            )),
        );
        enqueue_command(
            &mut queue,
            PreviewCommand::Request(queued_request(
                2,
                "near",
                "near",
                PreviewPriority::Prefetch { distance: 1 },
            )),
        );
        enqueue_command(
            &mut queue,
            PreviewCommand::Request(queued_request(
                3,
                "new selected",
                "new",
                PreviewPriority::Selected,
            )),
        );

        assert_eq!(queue.len(), 2);
        assert!(queue.iter().all(|req| req.generation != 1));
        assert_eq!(pop_next_preview_request(&mut queue).unwrap().generation, 3);
        assert_eq!(pop_next_preview_request(&mut queue).unwrap().generation, 2);
    }

    #[test]
    fn decoded_cache_is_lru_and_resets_timing_on_hits() {
        let mut cache = PreviewDecodedCache::new(1);
        let first = LoadedPreviewPixels {
            timing: ImageLoadTiming {
                read_us: 11,
                decode_us: 22,
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
        let mut cache = PreviewDecodedCache::new(2);

        let result = load_preview(req, &mut cache);

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
            "mister-magik-preview-index-{}.mmraw",
            std::process::id()
        ));
        let name = b"mpatrol.rgb565";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(PreviewArchive::RAW_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&20u32.to_le_bytes());
        bytes.extend_from_slice(&20u32.to_le_bytes());
        bytes.extend_from_slice(&128u64.to_le_bytes());
        bytes.extend_from_slice(name);
        std::fs::write(&path, bytes).expect("write rawpack fixture");

        let stems = preview_archive_entry_stems(&path).expect("read rawpack index");

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
        let mut bytes = Vec::new();
        bytes.extend_from_slice(PreviewArchive::LZ4_BLOCK_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&8192u32.to_le_bytes());
        bytes.extend_from_slice(&17u32.to_le_bytes());
        bytes.extend_from_slice(&4096u64.to_le_bytes());
        bytes.extend_from_slice(name);
        std::fs::write(&path, bytes).expect("write lz4 block fixture");

        let index = preview_archive_index(&path).expect("read lz4 block index");

        assert_eq!(index.codec, "lz4-block");
        assert_eq!(index.entries, vec!["1941u"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn archive_lookup_matches_mixed_case_cache_names() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-case-{}.mmraw",
            std::process::id()
        ));
        let payload = raw565_fixture(2, 1, &[0xf800, 0x07e0]);
        write_raw_archive(&path, "sonic.rgb565", &payload);

        let archive = PreviewArchive::open(&path).expect("open raw archive");
        let loaded = archive
            .load_timed("Sonic.rgb565")
            .expect("load mixed-case cache name")
            .expect("archive entry");

        assert_eq!(loaded.image.width(), 2);
        assert_eq!(loaded.image.height(), 1);
        assert_eq!(loaded.image.decoded_bytes(), 16);
        assert_eq!(loaded.timing.load_source, PreviewLoadSource::ArchiveMem);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auto_preview_archive_uses_size_named_screenshots_pack() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-preview-auto-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create archive root");
        let archive = root.join("320x320-screenshots.mmlz4b");
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
    fn missing_archive_asset_fails_without_original_decode_fallback() {
        let err = load_preview_pixels(
            "/tmp/missing/320x320-screenshots.mmlz4b",
            "tiny",
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
    fn load_preview_reads_requested_archive_asset_and_reports_dimensions() {
        let archive_path = std::env::temp_dir().join(format!(
            "mister-magik-preview-load-{}.mmraw",
            std::process::id()
        ));
        write_raw_archive(
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
        let mut cache = PreviewDecodedCache::new(2);
        let result = load_preview(req, &mut cache);

        assert_eq!(result.generation, 88);
        assert_eq!(result.title, "Tiny");
        assert_eq!(result.preview_archive_path, archive_path.display().to_string());
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
    fn preview_archive_cache_reopens_when_file_fingerprint_changes() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-preview-refresh-{}.mmraw",
            std::process::id()
        ));
        write_raw_archive(&path, "old.rgb565", &raw565_fixture(1, 1, &[0xf800]));
        let first = preview_archives_for_paths(vec![path.display().to_string()])
            .expect("open first archive")
            .expect("first archive");

        write_raw_archive(
            &path,
            "new-long.rgb565",
            &raw565_fixture(2, 1, &[0x07e0, 0x001f]),
        );
        let second = preview_archives_for_paths(vec![path.display().to_string()])
            .expect("open changed archive")
            .expect("changed archive");

        assert!(!Arc::ptr_eq(&first, &second));
        assert!(second[0]
            .load_timed("new-long.rgb565")
            .expect("load from changed archive")
            .is_some());
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

    fn write_raw_archive(path: &Path, name: &str, payload: &[u8]) {
        let index_len = 8 + 4 + 2 + 4 + 4 + 8 + name.len();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(PreviewArchive::RAW_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(index_len as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(payload);
        std::fs::write(path, bytes).expect("write raw archive fixture");
    }
}
