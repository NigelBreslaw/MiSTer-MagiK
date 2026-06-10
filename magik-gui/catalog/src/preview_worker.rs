//! Background arcade preview image loader.

use crate::arcade_catalog::{self, DecodedImage, ImageLoadTiming, LoadedImage};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
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
    pub image: Option<DecodedImage>,
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
}

impl PreviewResizeFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Nearest => "nearest",
            Self::Box => "box",
            Self::Lanczos => "lanczos",
        }
    }

    fn from_label(label: &str) -> Self {
        match label.to_ascii_lowercase().replace('_', "-").as_str() {
            "nearest" | "nearest-neighbor" => Self::Nearest,
            "box" | "area" | "box-area" => Self::Box,
            "lanczos" | "lanczos3" => Self::Lanczos,
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
            .unwrap_or(PreviewResizeFilter::Off);
        if filter == PreviewResizeFilter::Off {
            return Self::off();
        }
        let (max_w, max_h) = std::env::var("MISTER_PREVIEW_RESIZE_MAX")
            .ok()
            .and_then(|s| parse_size(&s))
            .unwrap_or((320, 240));
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
    Png,
    RawRgb,
}

impl PreviewStorageFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::RawRgb => "raw-rgb",
        }
    }

    fn from_env() -> Self {
        match std::env::var("MISTER_PREVIEW_FORMAT")
            .unwrap_or_else(|_| "png".into())
            .to_ascii_lowercase()
            .replace('_', "-")
            .as_str()
        {
            "raw" | "rgb" | "raw-rgb" => Self::RawRgb,
            _ => Self::Png,
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
    loop {
        match rx.recv() {
            Ok(command) => enqueue_command(&mut queue, command),
            Err(_) => break,
        }
        while let Ok(command) = rx.try_recv() {
            enqueue_command(&mut queue, command);
        }
        queue.sort_by_key(|req| (req.priority.rank(), req.requested_at));
        if let Some(req) = queue.first().cloned() {
            queue.remove(0);
            let result = load_preview(req);
            if tx.send(result).is_err() {
                break;
            }
        }
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

fn load_preview(req: PreviewRequest) -> PreviewResult {
    let resize = PreviewResizeSpec::from_env();
    let storage = PreviewStorageFormat::from_env();
    match load_preview_image(&req.image_path, storage, resize) {
        Ok(loaded) => {
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace decoded generation={} format={} filter={} source={}x{} output={}x{} total_us={} read_us={} decode_us={} resize_us={} encoded_bytes={} decoded_bytes={} path={}",
                    req.generation,
                    storage.label(),
                    resize.filter.label(),
                    loaded.timing.source_width,
                    loaded.timing.source_height,
                    loaded.image.width,
                    loaded.image.height,
                    loaded.timing.total_us,
                    loaded.timing.read_us,
                    loaded.timing.decode_us,
                    loaded.timing.resize_us,
                    loaded.timing.encoded_bytes,
                    loaded.timing.decoded_bytes,
                    req.image_path
                );
            }
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
                decoded_bytes: loaded.timing.decoded_bytes,
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

pub fn load_preview_image(
    image_path: &str,
    storage: PreviewStorageFormat,
    resize: PreviewResizeSpec,
) -> Result<LoadedImage, String> {
    match storage {
        PreviewStorageFormat::Png => {
            let mut loaded = arcade_catalog::load_png_rgb8_timed(image_path)?;
            apply_resize(&mut loaded, resize);
            Ok(loaded)
        }
        PreviewStorageFormat::RawRgb => load_raw_preview_timed(image_path, resize).or_else(|_| {
            let mut loaded = arcade_catalog::load_png_rgb8_timed(image_path)?;
            apply_resize(&mut loaded, resize);
            Ok(loaded)
        }),
    }
}

pub fn raw_preview_cache_path(image_path: &str, resize: PreviewResizeSpec) -> PathBuf {
    let source = Path::new(image_path);
    let filename = source
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("preview.png");
    let stem = filename.strip_suffix(".png").unwrap_or(filename);
    if let Ok(root) = std::env::var("MISTER_PREVIEW_CACHE_DIR") {
        return Path::new(&root)
            .join(format!("raw-{}", resize.cache_label()))
            .join(format!("{stem}.rgb"));
    }
    if let Some(parent) = source.parent() {
        if parent.file_name().and_then(|s| s.to_str()) == Some("screenshot") {
            if let Some(media) = parent.parent() {
                return media
                    .join("screenshot-magik")
                    .join(format!("raw-{}", resize.cache_label()))
                    .join(format!("{stem}.rgb"));
            }
        }
        return parent
            .join("screenshot-magik")
            .join(format!("raw-{}", resize.cache_label()))
            .join(format!("{stem}.rgb"));
    }
    PathBuf::from(format!("{stem}.rgb"))
}

pub fn write_raw_preview_cache(image_path: &str, resize: PreviewResizeSpec) -> Result<PathBuf, String> {
    let mut loaded = arcade_catalog::load_png_rgb8_timed(image_path)?;
    apply_resize(&mut loaded, resize);
    let out = raw_preview_cache_path(image_path, resize);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let mut bytes = Vec::with_capacity(16 + loaded.image.rgb.len());
    bytes.extend_from_slice(b"MMRGB01\0");
    bytes.extend_from_slice(&loaded.image.width.to_le_bytes());
    bytes.extend_from_slice(&loaded.image.height.to_le_bytes());
    bytes.extend_from_slice(&loaded.image.rgb);
    std::fs::write(&out, bytes).map_err(|e| format!("write {}: {e}", out.display()))?;
    Ok(out)
}

fn load_raw_preview_timed(image_path: &str, resize: PreviewResizeSpec) -> Result<LoadedImage, String> {
    let path = raw_preview_cache_path(image_path, resize);
    let total_t = Instant::now();
    let read_t = Instant::now();
    let data = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let read_us = read_t.elapsed().as_micros() as u64;
    let decode_t = Instant::now();
    let image = decode_raw_preview_bytes(&data)?;
    let decode_us = decode_t.elapsed().as_micros() as u64;
    let total_us = total_t.elapsed().as_micros() as u64;
    Ok(LoadedImage {
        timing: ImageLoadTiming {
            read_us,
            decode_us,
            resize_us: 0,
            total_us,
            encoded_bytes: data.len(),
            decoded_bytes: image.rgb.len(),
            source_width: image.width,
            source_height: image.height,
        },
        image,
    })
}

fn decode_raw_preview_bytes(data: &[u8]) -> Result<DecodedImage, String> {
    if data.len() < 16 || &data[..8] != b"MMRGB01\0" {
        return Err("raw preview bad header".into());
    }
    let width = u32::from_le_bytes(data[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(data[12..16].try_into().unwrap());
    let expected = width as usize * height as usize * 3;
    if data.len() - 16 != expected {
        return Err(format!(
            "raw preview length mismatch got={} expected={}",
            data.len() - 16,
            expected
        ));
    }
    Ok(DecodedImage {
        width,
        height,
        rgb: data[16..].to_vec(),
    })
}

fn apply_resize(loaded: &mut LoadedImage, resize: PreviewResizeSpec) {
    if resize.filter == PreviewResizeFilter::Off {
        return;
    }
    let Some((target_w, target_h)) =
        resize_target(loaded.image.width, loaded.image.height, resize.max_w, resize.max_h)
    else {
        return;
    };
    let t = Instant::now();
    let resized = resize_rgb8(
        &loaded.image.rgb,
        loaded.image.width,
        loaded.image.height,
        target_w,
        target_h,
        resize.filter,
    );
    loaded.timing.resize_us = t.elapsed().as_micros() as u64;
    loaded.image.width = target_w;
    loaded.image.height = target_h;
    loaded.image.rgb = resized;
    loaded.timing.decoded_bytes = loaded.image.rgb.len();
    loaded.timing.total_us += loaded.timing.resize_us;
}

fn resize_target(width: u32, height: u32, max_w: u32, max_h: u32) -> Option<(u32, u32)> {
    if width <= max_w && height <= max_h {
        return None;
    }
    let scale = (max_w as f64 / width as f64).min(max_h as f64 / height as f64);
    let target_w = ((width as f64 * scale).round() as u32).max(1);
    let target_h = ((height as f64 * scale).round() as u32).max(1);
    if target_w == width && target_h == height {
        None
    } else {
        Some((target_w, target_h))
    }
}

fn resize_rgb8(
    src: &[u8],
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
    filter: PreviewResizeFilter,
) -> Vec<u8> {
    match filter {
        PreviewResizeFilter::Nearest => resize_nearest(src, sw, sh, dw, dh),
        PreviewResizeFilter::Box => resize_box(src, sw, sh, dw, dh),
        PreviewResizeFilter::Lanczos => resize_lanczos(src, sw, sh, dw, dh),
        PreviewResizeFilter::Off => src.to_vec(),
    }
}

fn resize_nearest(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0; dw as usize * dh as usize * 3];
    for y in 0..dh {
        let sy = ((y as u64 * sh as u64) / dh as u64).min(sh as u64 - 1) as usize;
        for x in 0..dw {
            let sx = ((x as u64 * sw as u64) / dw as u64).min(sw as u64 - 1) as usize;
            let s = (sy * sw as usize + sx) * 3;
            let d = (y as usize * dw as usize + x as usize) * 3;
            out[d..d + 3].copy_from_slice(&src[s..s + 3]);
        }
    }
    out
}

fn resize_box(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0; dw as usize * dh as usize * 3];
    for y in 0..dh {
        let y0 = ((y as f64 * sh as f64 / dh as f64).floor() as u32).min(sh - 1);
        let y1 = ((((y + 1) as f64 * sh as f64 / dh as f64).ceil() as u32).max(y0 + 1)).min(sh);
        for x in 0..dw {
            let x0 = ((x as f64 * sw as f64 / dw as f64).floor() as u32).min(sw - 1);
            let x1 =
                ((((x + 1) as f64 * sw as f64 / dw as f64).ceil() as u32).max(x0 + 1)).min(sw);
            let mut acc = [0u32; 3];
            let mut count = 0u32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let s = (sy as usize * sw as usize + sx as usize) * 3;
                    acc[0] += src[s] as u32;
                    acc[1] += src[s + 1] as u32;
                    acc[2] += src[s + 2] as u32;
                    count += 1;
                }
            }
            let d = (y as usize * dw as usize + x as usize) * 3;
            out[d] = (acc[0] / count) as u8;
            out[d + 1] = (acc[1] / count) as u8;
            out[d + 2] = (acc[2] / count) as u8;
        }
    }
    out
}

fn resize_lanczos(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0; dw as usize * dh as usize * 3];
    let sx_scale = sw as f64 / dw as f64;
    let sy_scale = sh as f64 / dh as f64;
    for y in 0..dh {
        let cy = (y as f64 + 0.5) * sy_scale - 0.5;
        let y_start = (cy - 3.0).floor() as i32;
        let y_end = (cy + 3.0).ceil() as i32;
        for x in 0..dw {
            let cx = (x as f64 + 0.5) * sx_scale - 0.5;
            let x_start = (cx - 3.0).floor() as i32;
            let x_end = (cx + 3.0).ceil() as i32;
            let mut acc = [0.0f64; 3];
            let mut weight_sum = 0.0f64;
            for sy in y_start..=y_end {
                if sy < 0 || sy >= sh as i32 {
                    continue;
                }
                let wy = lanczos_weight(cy - sy as f64);
                if wy == 0.0 {
                    continue;
                }
                for sx in x_start..=x_end {
                    if sx < 0 || sx >= sw as i32 {
                        continue;
                    }
                    let wx = lanczos_weight(cx - sx as f64);
                    let w = wx * wy;
                    if w == 0.0 {
                        continue;
                    }
                    let s = (sy as usize * sw as usize + sx as usize) * 3;
                    acc[0] += src[s] as f64 * w;
                    acc[1] += src[s + 1] as f64 * w;
                    acc[2] += src[s + 2] as f64 * w;
                    weight_sum += w;
                }
            }
            let d = (y as usize * dw as usize + x as usize) * 3;
            for c in 0..3 {
                out[d + c] = if weight_sum == 0.0 {
                    0
                } else {
                    (acc[c] / weight_sum).round().clamp(0.0, 255.0) as u8
                };
            }
        }
    }
    out
}

fn lanczos_weight(x: f64) -> f64 {
    let x = x.abs();
    if x < f64::EPSILON {
        return 1.0;
    }
    if x >= 3.0 {
        return 0.0;
    }
    sinc(x) * sinc(x / 3.0)
}

fn sinc(x: f64) -> f64 {
    let pix = std::f64::consts::PI * x;
    pix.sin() / pix
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
}
