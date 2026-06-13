//! Background arcade preview image loader.

use crate::arcade_catalog::ImageLoadTiming;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Instant;

pub const DEFAULT_PREVIEW_RADIUS: usize = 5;
pub const DEFAULT_PREVIEW_CACHE_CAP: usize = DEFAULT_PREVIEW_RADIUS * 2 + 1;

#[derive(Clone, Debug)]
pub struct PreviewRequest {
    pub generation: u64,
    pub title: String,
    pub image_path: String,
    pub requested_at: Instant,
    pub priority: PreviewPriority,
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
    pub image_path: String,
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
    pub storage_format: PreviewStorageFormat,
    pub resize_filter: PreviewResizeFilter,
    pub priority: PreviewPriority,
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
        match label.to_ascii_lowercase().replace('_', "-").as_str() {
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

#[derive(Clone, Debug)]
pub enum PreviewPixels {
    Rgb565 {
        width: u32,
        height: u32,
        stride_bytes: u32,
        words: Vec<u16>,
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

    pub fn request_selected(&mut self, title: String, image_path: String) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        let _ = self.tx.send(PreviewCommand::Request(PreviewRequest {
            generation,
            title,
            image_path,
            requested_at: Instant::now(),
            priority: PreviewPriority::Selected,
        }));
        generation
    }

    pub fn request_prefetch(&mut self, title: String, image_path: String, distance: usize) {
        let generation = self.next_generation;
        self.next_generation += 1;
        let _ = self.tx.send(PreviewCommand::Request(PreviewRequest {
            generation,
            title,
            image_path,
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
        let idx = self.entries.iter().position(|(entry_key, _)| entry_key == key)?;
        let (key, loaded) = self.entries.remove(idx);
        let clone_t = Instant::now();
        let mut out = loaded.clone();
        out.timing.read_us = 0;
        out.timing.decode_us = 0;
        out.timing.resize_us = 0;
        out.timing.encoded_bytes = 0;
        out.timing.total_us = clone_t.elapsed().as_micros() as u64;
        self.entries.push((key, loaded));
        Some(out)
    }

    fn insert(&mut self, key: String, loaded: &LoadedPreviewPixels) {
        if self.cap == 0 {
            return;
        }
        if let Some(idx) = self.entries.iter().position(|(entry_key, _)| *entry_key == key) {
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
            if let Some(existing) = queue
                .iter_mut()
                .find(|existing| existing.image_path == req.image_path)
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

fn preview_cache_key(image_path: &str, resize: PreviewResizeSpec) -> String {
    format!(
        "{}|{}|{}x{}",
        image_path,
        resize.filter.label(),
        resize.max_w,
        resize.max_h
    )
}

fn load_preview(req: PreviewRequest, decoded_cache: &mut PreviewDecodedCache) -> PreviewResult {
    let resize = PreviewResizeSpec::from_env();
    let storage = PreviewStorageFormat::from_env();
    let cache_key = preview_cache_key(&req.image_path, resize);
    let mut cache_hit = false;
    let loaded_result = if let Some(loaded) = decoded_cache.get(&cache_key) {
        cache_hit = true;
        Ok(loaded)
    } else {
        load_preview_pixels(&req.image_path, resize).inspect(|loaded| {
            decoded_cache.insert(cache_key, loaded);
        })
    };
    match loaded_result {
        Ok(loaded) => {
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace decoded generation={} cache_hit={} format={} filter={} source={}x{} output={}x{} total_us={} read_us={} decode_us={} resize_us={} encoded_bytes={} decoded_bytes={} path={}",
                    req.generation,
                    if cache_hit { 1 } else { 0 },
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
                    req.image_path
                );
            }
            let decoded_bytes = loaded.image.decoded_bytes();
            PreviewResult {
                generation: req.generation,
                title: req.title,
                image_path: req.image_path,
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
                storage_format: storage,
                resize_filter: resize.filter,
                priority: req.priority,
            }
        }
        Err(e) => {
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace decode_failed generation={} age_us={} path={} error={}",
                    req.generation,
                    req.requested_at.elapsed().as_micros(),
                    req.image_path,
                    e
                );
            }
            PreviewResult {
                generation: req.generation,
                title: req.title,
                image_path: req.image_path,
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
    image_path: &str,
    resize: PreviewResizeSpec,
) -> Result<LoadedPreviewPixels, String> {
    load_raw565_preview_timed(image_path, resize)
}

pub fn preview_cache_path(image_path: &str, resize: PreviewResizeSpec) -> PathBuf {
    let source = Path::new(image_path);
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("preview");
    let cache_dir = format!("raw565-{}", resize.cache_label());
    if let Ok(root) = std::env::var("MISTER_PREVIEW_CACHE_DIR") {
        return Path::new(&root)
            .join(cache_dir)
            .join(format!("{stem}.rgb565"));
    }
    if let Some(parent) = source.parent() {
        if parent.file_name().and_then(|s| s.to_str()) == Some("screenshot") {
            if let Some(media) = parent.parent() {
                return media
                    .join("screenshot-magik")
                    .join(cache_dir)
                    .join(format!("{stem}.rgb565"));
            }
        }
        return parent
            .join("screenshot-magik")
            .join(cache_dir)
            .join(format!("{stem}.rgb565"));
    }
    PathBuf::from(format!("{stem}.rgb565"))
}

pub fn raw565_preview_cache_path(image_path: &str, resize: PreviewResizeSpec) -> PathBuf {
    preview_cache_path(image_path, resize)
}

fn load_raw565_preview_timed(
    image_path: &str,
    resize: PreviewResizeSpec,
) -> Result<LoadedPreviewPixels, String> {
    let path = raw565_preview_cache_path(image_path, resize);
    if let Some(archive) = preview_archive()? {
        if let Some(stem) = Path::new(&path).file_name().and_then(|s| s.to_str()) {
            if let Some(loaded) = archive.load_timed(stem)? {
                return Ok(loaded);
            }
        }
    }
    let total_t = Instant::now();
    let read_t = Instant::now();
    let data = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let read_us = read_t.elapsed().as_micros() as u64;
    let decode_t = Instant::now();
    let image = decode_raw565_preview_bytes(&data)?;
    let decode_us = decode_t.elapsed().as_micros() as u64;
    let total_us = total_t.elapsed().as_micros() as u64;
    let decoded_bytes = image.decoded_bytes();
    Ok(LoadedPreviewPixels {
        timing: ImageLoadTiming {
            read_us,
            decode_us,
            resize_us: 0,
            total_us,
            encoded_bytes: data.len(),
            decoded_bytes,
            source_width: image.width(),
            source_height: image.height(),
        },
        image,
    })
}

#[derive(Clone, Copy, Debug)]
struct PreviewArchiveEntry {
    raw_len: usize,
    compressed_len: usize,
    offset: u64,
}

struct PreviewArchive {
    file: Mutex<File>,
    scratch: Mutex<PreviewArchiveScratch>,
    codec: PreviewArchiveCodec,
    entries: HashMap<String, PreviewArchiveEntry>,
}

#[derive(Clone, Copy, Debug)]
enum PreviewArchiveCodec {
    Lz4Block,
    Raw,
}

#[derive(Default)]
struct PreviewArchiveScratch {
    compressed: Vec<u8>,
    raw: Vec<u8>,
}

fn preview_archive() -> Result<Option<&'static PreviewArchive>, String> {
    static ARCHIVE: OnceLock<Option<Result<PreviewArchive, String>>> = OnceLock::new();
    let value = ARCHIVE.get_or_init(|| {
        let path = match std::env::var("MISTER_PREVIEW_ARCHIVE") {
            Ok(path) if !path.is_empty() => path,
            _ => return None,
        };
        Some(PreviewArchive::open(Path::new(&path)))
    });
    match value {
        Some(Ok(archive)) => Ok(Some(archive)),
        Some(Err(err)) => Err(err.clone()),
        None => Ok(None),
    }
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
            entries.insert(name, PreviewArchiveEntry {
                raw_len,
                compressed_len,
                offset,
            });
        }
        Ok(Self {
            file: Mutex::new(file),
            scratch: Mutex::new(PreviewArchiveScratch::default()),
            codec,
            entries,
        })
    }

    fn load_timed(&self, name: &str) -> Result<Option<LoadedPreviewPixels>, String> {
        let Some(entry) = self.entries.get(name).copied() else {
            return Ok(None);
        };
        let total_t = Instant::now();
        let read_t = Instant::now();
        let mut scratch = self
            .scratch
            .lock()
            .map_err(|_| "preview archive scratch lock poisoned".to_string())?;
        scratch.compressed.resize(entry.compressed_len, 0);
        {
            let mut file = self
                .file
                .lock()
                .map_err(|_| "preview archive file lock poisoned".to_string())?;
            file.seek(SeekFrom::Start(entry.offset))
                .map_err(|e| format!("preview archive seek {name}: {e}"))?;
            file.read_exact(&mut scratch.compressed)
                .map_err(|e| format!("preview archive read {name}: {e}"))?;
        }
        let read_us = read_t.elapsed().as_micros() as u64;

        let decode_t = Instant::now();
        let PreviewArchiveScratch { compressed, raw } = &mut *scratch;
        let data = match self.codec {
            PreviewArchiveCodec::Lz4Block => {
                decode_lz4_block_entry_into(compressed, entry.raw_len, raw)
                    .map_err(|e| format!("preview archive lz4 decode {name}: {e}"))?
            }
            PreviewArchiveCodec::Raw => {
                if compressed.len() != entry.raw_len {
                    return Err(format!(
                        "preview archive raw length mismatch {name}: got={} expected={}",
                        compressed.len(),
                        entry.raw_len
                    ));
                }
                compressed.as_slice()
            }
        };
        let image = decode_raw565_preview_bytes(&data)?;
        let decode_us = decode_t.elapsed().as_micros() as u64;
        let total_us = total_t.elapsed().as_micros() as u64;
        let decoded_bytes = image.decoded_bytes();
        Ok(Some(LoadedPreviewPixels {
            timing: ImageLoadTiming {
                read_us,
                decode_us,
                resize_us: 0,
                total_us,
                encoded_bytes: entry.compressed_len,
                decoded_bytes,
                source_width: image.width(),
                source_height: image.height(),
            },
            image,
        }))
    }
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
    if stride_bytes as usize % 16 != 0 || (stride_bytes as usize) < min_stride {
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
    let mut words = Vec::with_capacity(expected / 2);
    for chunk in data[20..].chunks_exact(2) {
        words.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    Ok(PreviewPixels::Rgb565 {
        width,
        height,
        stride_bytes,
        words,
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
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 10);
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
        let items = vec![
            Some("a"),
            Some("b"),
            None,
            Some("b"),
            Some("c"),
            Some("d"),
        ];
        let paths = preview_window_paths(&items, 2, 3, |p| *p);
        assert_eq!(paths, vec!["b", "a", "c", "d"]);
    }

    #[test]
    fn preview_queue_pops_selected_before_prefetch_and_keeps_remaining_work() {
        let now = Instant::now();
        let mut queue = vec![
            PreviewRequest {
                generation: 1,
                title: "near".to_string(),
                image_path: "near.png".to_string(),
                requested_at: now,
                priority: PreviewPriority::Prefetch { distance: 1 },
            },
            PreviewRequest {
                generation: 2,
                title: "selected".to_string(),
                image_path: "selected.png".to_string(),
                requested_at: now,
                priority: PreviewPriority::Selected,
            },
            PreviewRequest {
                generation: 3,
                title: "far".to_string(),
                image_path: "far.png".to_string(),
                requested_at: now,
                priority: PreviewPriority::Prefetch { distance: 4 },
            },
        ];

        assert_eq!(pop_next_preview_request(&mut queue).unwrap().generation, 2);
        assert_eq!(queue.len(), 2);
        assert_eq!(pop_next_preview_request(&mut queue).unwrap().generation, 1);
        assert_eq!(pop_next_preview_request(&mut queue).unwrap().generation, 3);
        assert!(pop_next_preview_request(&mut queue).is_none());
    }

    #[test]
    fn hybrid_filter_uses_nearest_for_upscale_and_lanczos_for_downscale_labels() {
        assert_eq!(PreviewResizeFilter::from_label("hybrid"), PreviewResizeFilter::Hybrid);
        assert_eq!(PreviewResizeFilter::Hybrid.label(), "hybrid");
    }

    #[test]
    fn cache_path_lives_outside_original_screenshot_dir() {
        let resize = PreviewResizeSpec {
            filter: PreviewResizeFilter::Lanczos,
            max_w: 320,
            max_h: 320,
        };
        let original = "/media/fat/_Arcade/media/screenshot/1941u.png";
        let raw565 = raw565_preview_cache_path(original, resize);
        assert_eq!(
            raw565,
            PathBuf::from(
                "/media/fat/_Arcade/media/screenshot-magik/raw565-lanczos-320x320/1941u.rgb565"
            )
        );
        assert_ne!(raw565, PathBuf::from(original));
    }

    #[test]
    fn cache_path_uses_source_stem_for_jpg_originals() {
        let resize = PreviewResizeSpec {
            filter: PreviewResizeFilter::Hybrid,
            max_w: 320,
            max_h: 320,
        };
        let original = "/media/fat/_Arcade/media/screenshot/astrass.jpg";
        let raw565 = raw565_preview_cache_path(original, resize);
        assert_eq!(
            raw565,
            PathBuf::from(
                "/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320/astrass.rgb565"
            )
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
    fn missing_raw565_cache_fails_without_original_decode_fallback() {
        let resize = PreviewResizeSpec {
            filter: PreviewResizeFilter::Hybrid,
            max_w: 320,
            max_h: 320,
        };
        let err = load_preview_pixels("/tmp/missing/media/screenshot/tiny.png", resize)
            .expect_err("missing raw565 cache must not decode original screenshots");
        assert!(err.contains("raw565-hybrid-320x320/tiny.rgb565"));
    }
}
