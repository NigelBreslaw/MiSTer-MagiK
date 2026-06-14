use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, OnceLock};

use mister_magik_ui as slint_ui;
use slint::platform::software_renderer::Rgb565Pixel;
use slint::ComponentHandle;
use slint_ui::launcher::PreviewStatus;

use crate::arcade_catalog::ArcadeGameEntry;
use crate::preview_worker::{
    preview_window_indices, preview_window_paths, PreviewPixels, PreviewPriority, PreviewResult,
    PreviewWorker, DEFAULT_PREVIEW_CACHE_CAP, DEFAULT_PREVIEW_RADIUS,
};
use crate::ui_display::{UI_FB_H, UI_FB_W};

const PREVIEW_MAX_AREA: u32 = (UI_FB_W as u32 * UI_FB_H as u32 * 40) / 100;
const MAX_PREFETCH_RESULTS_PER_FRAME: usize = 1;
pub(crate) const ARCADE_PREVIEW_BOX_X: usize = 8;
pub(crate) const ARCADE_PREVIEW_BOX_Y: usize = 92;
pub(crate) const ARCADE_PREVIEW_BOX_W: u32 = 320;
pub(crate) const ARCADE_PREVIEW_BOX_H: u32 = 320;

fn preview_trace_enabled() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        matches!(
            std::env::var("MISTER_PREVIEW_TRACE").as_deref(),
            Ok("1") | Ok("on") | Ok("true") | Ok("yes")
        )
    })
}

fn preview_loading_enabled() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        !matches!(
            std::env::var("MISTER_PREVIEW_LOADING").as_deref(),
            Ok("0") | Ok("off") | Ok("false") | Ok("no")
        )
    })
}

pub(crate) fn preview_visual_pct() -> u32 {
    static VALUE: OnceLock<u32> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MISTER_PREVIEW_VISUAL_PCT")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(100)
            .clamp(10, 100)
    })
}

struct PreviewImage {
    pixels: PreviewImagePixels,
    source_w: u32,
    source_h: u32,
    display_w: u32,
    display_h: u32,
}

enum PreviewImagePixels {
    Rgb565 {
        pixels: Vec<Rgb565Pixel>,
        stride_pixels: usize,
    },
}

#[derive(Default)]
struct PreviewImageCache {
    entries: VecDeque<(String, Arc<PreviewImage>)>,
    failed_paths: VecDeque<String>,
}

impl PreviewImageCache {
    const FAILED_CAP: usize = 128;

    fn get(&mut self, path: &str) -> Option<Arc<PreviewImage>> {
        let idx = self.entries.iter().position(|(p, _)| p == path)?;
        let (_, image) = self.entries.remove(idx)?;
        let out = Arc::clone(&image);
        self.entries.push_back((path.to_string(), image));
        Some(out)
    }

    fn peek(&self, path: &str) -> Option<&PreviewImage> {
        self.entries
            .iter()
            .find_map(|(p, image)| (p == path).then_some(image.as_ref()))
    }

    fn peek_shared(&self, path: &str) -> Option<&Arc<PreviewImage>> {
        self.entries
            .iter()
            .find_map(|(p, image)| (p == path).then_some(image))
    }

    fn insert(
        &mut self,
        path: String,
        image: Arc<PreviewImage>,
        window_paths: &[String],
        visible_path: Option<&str>,
    ) {
        if let Some(idx) = self.entries.iter().position(|(p, _)| p == &path) {
            self.entries.remove(idx);
        }
        self.entries.push_back((path, image));
        self.retain_window(window_paths, visible_path);
    }

    fn insert_failed(&mut self, path: String) {
        if let Some(idx) = self.failed_paths.iter().position(|p| p == &path) {
            self.failed_paths.remove(idx);
        }
        self.failed_paths.push_back(path);
        while self.failed_paths.len() > Self::FAILED_CAP {
            self.failed_paths.pop_front();
        }
    }

    fn retain_window(&mut self, window_paths: &[String], visible_path: Option<&str>) {
        if !window_paths.is_empty() {
            self.entries.retain(|(path, _)| {
                visible_path.is_some_and(|visible| visible == path)
                    || window_paths.iter().any(|keep| keep == path)
            });
        }
        while self.entries.len() > DEFAULT_PREVIEW_CACHE_CAP {
            if self
                .entries
                .front()
                .is_some_and(|(path, _)| visible_path.is_some_and(|visible| visible == path))
                && self.entries.len() > 1
            {
                if let Some(entry) = self.entries.pop_front() {
                    self.entries.push_back(entry);
                }
            } else {
                self.entries.pop_front();
            }
        }
    }

    fn contains(&self, path: &str) -> bool {
        self.entries.iter().any(|(p, _)| p == path)
    }

    fn contains_failed(&self, path: &str) -> bool {
        self.failed_paths.iter().any(|p| p == path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewDisplaySize {
    w: u32,
    h: u32,
}

fn preview_display_size(
    source_w: u32,
    source_h: u32,
    pane_w: u32,
    pane_h: u32,
) -> PreviewDisplaySize {
    if source_w == 0 || source_h == 0 || pane_w == 0 || pane_h == 0 {
        return PreviewDisplaySize { w: 0, h: 0 };
    }

    let max_area = PREVIEW_MAX_AREA.min(pane_w.saturating_mul(pane_h)).max(1);
    let max_area = (max_area.saturating_mul(preview_visual_pct()) / 100).max(1);

    let integer_upscale = (pane_w / source_w).min(pane_h / source_h).max(1);
    let area_upscale = ((max_area as f64) / (source_w.saturating_mul(source_h).max(1) as f64))
        .sqrt()
        .floor() as u32;
    let scale = integer_upscale.min(area_upscale.max(1)).max(1);
    PreviewDisplaySize {
        w: source_w.saturating_mul(scale),
        h: source_h.saturating_mul(scale),
    }
}

fn apply_preview_image_bridge(
    bridge: &slint_ui::launcher::MisterBridge,
    preview_image: &PreviewImage,
) {
    bridge.set_arcade_preview_status(PreviewStatus::Ready);
    bridge.set_arcade_preview_source_width(preview_image.source_w as i32);
    bridge.set_arcade_preview_source_height(preview_image.source_h as i32);
    bridge.set_arcade_preview_display_width(preview_image.display_w as i32);
    bridge.set_arcade_preview_display_height(preview_image.display_h as i32);
}

fn clear_preview_image_bridge(bridge: &slint_ui::launcher::MisterBridge) {
    bridge.set_arcade_preview_source_width(0);
    bridge.set_arcade_preview_source_height(0);
    bridge.set_arcade_preview_display_width(0);
    bridge.set_arcade_preview_display_height(0);
}

pub(crate) struct PreviewState {
    worker: PreviewWorker,
    selected_mra_path: Option<String>,
    selected_image_path: Option<String>,
    current_generation: u64,
    cache: PreviewImageCache,
    has_visible_preview: bool,
    visible_path: String,
    previous_image: Option<Arc<PreviewImage>>,
    raw_transition_id: u64,
    window_paths: Vec<String>,
    pending_prefetch_paths: HashSet<String>,
    ready_backlog: VecDeque<PreviewResult>,
    raw_dirty: bool,
}

pub(crate) struct PreviewRawFrame<'a> {
    pub(crate) pixels: PreviewRawPixels<'a>,
    pub(crate) source_w: u32,
    pub(crate) source_h: u32,
    pub(crate) display_w: u32,
    pub(crate) display_h: u32,
}

pub(crate) struct PreviewRawTransitionFrame<'a> {
    pub(crate) previous: Option<PreviewRawFrame<'a>>,
    pub(crate) current: PreviewRawFrame<'a>,
    pub(crate) transition_id: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum PreviewRawPixels<'a> {
    Empty,
    #[allow(dead_code)]
    Rgb8(&'a [u8]),
    Rgb565 {
        pixels: &'a [Rgb565Pixel],
        stride_pixels: usize,
    },
}

impl PreviewState {
    pub(crate) fn new() -> Self {
        Self {
            worker: PreviewWorker::new(),
            selected_mra_path: None,
            selected_image_path: None,
            current_generation: 0,
            cache: PreviewImageCache::default(),
            has_visible_preview: false,
            visible_path: String::new(),
            previous_image: None,
            raw_transition_id: 0,
            window_paths: Vec::new(),
            pending_prefetch_paths: HashSet::new(),
            ready_backlog: VecDeque::new(),
            raw_dirty: false,
        }
    }

    pub(crate) fn clear(&mut self, bridge: &slint_ui::launcher::MisterBridge) {
        if self.selected_mra_path.is_some()
            || self.current_generation != 0
            || self.has_visible_preview
        {
            self.selected_mra_path = None;
            self.selected_image_path = None;
            self.current_generation = 0;
            self.has_visible_preview = false;
            self.visible_path.clear();
            self.previous_image = None;
            self.raw_transition_id = self.raw_transition_id.wrapping_add(1);
            self.window_paths.clear();
            self.pending_prefetch_paths.clear();
            self.ready_backlog.clear();
            self.raw_dirty = false;
            bridge.set_arcade_preview_placeholder_visible(true);
            bridge.set_arcade_preview_status(PreviewStatus::Empty);
            bridge.set_arcade_preview_title("".into());
            clear_preview_image_bridge(bridge);
        }
    }

    pub(crate) fn trace_cache_state(&self) -> &'static str {
        self.selected_image_path
            .as_deref()
            .map(|path| {
                if self.visible_path == path {
                    "exact"
                } else if self.cache.contains(path) {
                    "cached"
                } else if self.has_visible_preview {
                    "stale"
                } else {
                    "placeholder"
                }
            })
            .unwrap_or("empty")
    }

    pub(crate) fn take_raw_dirty(&mut self) -> bool {
        let dirty = self.raw_dirty;
        self.raw_dirty = false;
        dirty
    }

    fn raw_frame_from_image(image: &PreviewImage) -> PreviewRawFrame<'_> {
        PreviewRawFrame {
            pixels: match &image.pixels {
                PreviewImagePixels::Rgb565 {
                    pixels,
                    stride_pixels,
                } => PreviewRawPixels::Rgb565 {
                    pixels,
                    stride_pixels: *stride_pixels,
                },
            },
            source_w: image.source_w,
            source_h: image.source_h,
            display_w: image.display_w,
            display_h: image.display_h,
        }
    }

    fn begin_raw_transition_to(&mut self, next_path: &str) {
        if self.visible_path == next_path {
            return;
        }
        self.previous_image = if self.has_visible_preview {
            self.cache.peek_shared(&self.visible_path).map(Arc::clone)
        } else {
            None
        };
        self.raw_transition_id = self.raw_transition_id.wrapping_add(1);
    }

    fn begin_raw_transition_to_empty(&mut self) {
        self.previous_image = if self.has_visible_preview {
            self.cache.peek_shared(&self.visible_path).map(Arc::clone)
        } else {
            None
        };
        self.has_visible_preview = false;
        self.visible_path.clear();
        self.raw_transition_id = self.raw_transition_id.wrapping_add(1);
        self.raw_dirty = true;
    }

    fn select_empty_preview(&mut self) {
        self.current_generation = 0;
        self.selected_image_path = None;
        self.begin_raw_transition_to_empty();
    }

    pub(crate) fn finish_raw_empty_transition_if_idle(&mut self) {
        if !self.has_visible_preview && self.visible_path.is_empty() && !self.raw_dirty {
            self.previous_image = None;
        }
    }

    pub(crate) fn raw_frame(&self) -> Option<PreviewRawFrame<'_>> {
        if !self.has_visible_preview {
            return None;
        }
        let image = self.cache.peek(&self.visible_path)?;
        Some(Self::raw_frame_from_image(image))
    }

    pub(crate) fn raw_transition_frame(&self) -> Option<PreviewRawTransitionFrame<'_>> {
        let current = if self.has_visible_preview {
            self.raw_frame()?
        } else if self.previous_image.is_some() || self.raw_dirty {
            PreviewRawFrame {
                pixels: PreviewRawPixels::Empty,
                source_w: 1,
                source_h: 1,
                display_w: ARCADE_PREVIEW_BOX_W,
                display_h: ARCADE_PREVIEW_BOX_H,
            }
        } else {
            return None;
        };
        Some(PreviewRawTransitionFrame {
            previous: self
                .previous_image
                .as_ref()
                .map(|image| Self::raw_frame_from_image(image)),
            current,
            transition_id: self.raw_transition_id,
        })
    }
}

fn is_current_selected_result(
    result: &PreviewResult,
    current_generation: u64,
    selected_image_path: Option<&str>,
) -> bool {
    result.generation == current_generation
        && selected_image_path.is_some_and(|path| path == result.image_path)
}

fn next_ready_result_index(
    backlog: &VecDeque<PreviewResult>,
    current_generation: u64,
    selected_image_path: Option<&str>,
    selected_processed: bool,
    prefetch_results: usize,
) -> Option<usize> {
    if !selected_processed {
        if let Some(idx) = backlog.iter().position(|result| {
            is_current_selected_result(result, current_generation, selected_image_path)
        }) {
            return Some(idx);
        }
    }

    let result = backlog.front()?;
    if matches!(result.priority, PreviewPriority::Selected) {
        return Some(0);
    }
    (prefetch_results < MAX_PREFETCH_RESULTS_PER_FRAME).then_some(0)
}

pub(crate) fn request_arcade_preview_window(
    bridge: &slint_ui::launcher::MisterBridge,
    games: &[ArcadeGameEntry],
    selected: usize,
    preview: &mut PreviewState,
) -> bool {
    if !preview_loading_enabled() {
        preview.clear(bridge);
        return false;
    }
    let game = games.get(selected);
    let Some(game) = game else {
        preview.selected_mra_path = None;
        preview.selected_image_path = None;
        preview.current_generation = 0;
        preview.has_visible_preview = false;
        preview.visible_path.clear();
        preview.window_paths.clear();
        preview.pending_prefetch_paths.clear();
        bridge.set_arcade_preview_placeholder_visible(true);
        bridge.set_arcade_preview_status(PreviewStatus::Empty);
        bridge.set_arcade_preview_title("".into());
        clear_preview_image_bridge(bridge);
        return true;
    };
    bridge.set_arcade_preview_placeholder_visible(true);

    preview.window_paths = preview_window_paths(games, selected, DEFAULT_PREVIEW_RADIUS, |game| {
        game.has_image.then_some(game.image_path.as_str())
    })
    .into_iter()
    .map(str::to_string)
    .collect();
    preview
        .cache
        .retain_window(&preview.window_paths, Some(&preview.visible_path));

    if preview
        .selected_mra_path
        .as_deref()
        .is_some_and(|path| path == game.mra_path)
    {
        if let Some(path) = preview.selected_image_path.clone() {
            if preview.visible_path != path {
                if let Some(image) = preview.cache.get(&path) {
                    bridge.set_arcade_preview_title(game.title.clone().into());
                    preview.current_generation = 0;
                    preview.has_visible_preview = true;
                    preview.begin_raw_transition_to(&path);
                    preview.visible_path = path;
                    preview.raw_dirty = true;
                    apply_preview_image_bridge(bridge, &image);
                    request_preview_prefetches(games, selected, preview);
                    return true;
                }
                if preview.cache.contains_failed(&path) {
                    preview.select_empty_preview();
                    bridge.set_arcade_preview_status(PreviewStatus::Empty);
                    request_preview_prefetches(games, selected, preview);
                    return true;
                }
            }
        }
        request_preview_prefetches(games, selected, preview);
        return false;
    }
    preview.selected_mra_path = Some(game.mra_path.clone());

    bridge.set_arcade_preview_title(game.title.clone().into());
    if game.has_image {
        preview.selected_image_path = Some(game.image_path.clone());
        if preview.cache.contains_failed(&game.image_path) {
            preview.select_empty_preview();
            bridge.set_arcade_preview_status(PreviewStatus::Empty);
            request_preview_prefetches(games, selected, preview);
            return true;
        }
        if let Some(image) = preview.cache.get(&game.image_path) {
            preview.current_generation = 0;
            preview.has_visible_preview = true;
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace cache_hit title={} path={}",
                    game.title, game.image_path
                );
            }
            preview.begin_raw_transition_to(&game.image_path);
            preview.visible_path = game.image_path.clone();
            preview.raw_dirty = true;
            apply_preview_image_bridge(bridge, &image);
            request_preview_prefetches(games, selected, preview);
            return true;
        }
        preview.current_generation = preview
            .worker
            .request_selected(game.title.clone(), game.image_path.clone());
        if preview_trace_enabled() {
            eprintln!(
                "preview_trace requested generation={} title={} path={}",
                preview.current_generation, game.title, game.image_path
            );
        }
        if !preview.has_visible_preview {
            clear_preview_image_bridge(bridge);
        }
        bridge.set_arcade_preview_status(PreviewStatus::Loading);
        request_preview_prefetches(games, selected, preview);
        return true;
    }
    preview.select_empty_preview();
    bridge.set_arcade_preview_placeholder_visible(true);
    clear_preview_image_bridge(bridge);
    bridge.set_arcade_preview_status(PreviewStatus::Empty);
    true
}

fn request_preview_prefetches(
    games: &[ArcadeGameEntry],
    selected: usize,
    preview: &mut PreviewState,
) {
    for idx in preview_window_indices(games.len(), selected, DEFAULT_PREVIEW_RADIUS) {
        if idx == selected {
            continue;
        }
        let Some(game) = games.get(idx) else {
            continue;
        };
        if !game.has_image
            || preview.cache.contains(&game.image_path)
            || preview.cache.contains_failed(&game.image_path)
            || preview.pending_prefetch_paths.contains(&game.image_path)
        {
            continue;
        }
        let distance = idx.abs_diff(selected);
        preview
            .pending_prefetch_paths
            .insert(game.image_path.clone());
        preview
            .worker
            .request_prefetch(game.title.clone(), game.image_path.clone(), distance);
        if preview_trace_enabled() {
            eprintln!(
                "preview_trace prefetch distance={} title={} path={}",
                distance, game.title, game.image_path
            );
        }
    }
}

pub(crate) fn schedule_arcade_preview_window(
    bridge: &slint_ui::launcher::MisterBridge,
    games: &[ArcadeGameEntry],
    selected: usize,
    preview: &mut PreviewState,
) -> bool {
    if !preview_loading_enabled() {
        preview.clear(bridge);
        return false;
    }
    let Some(game) = games.get(selected) else {
        preview.clear(bridge);
        return true;
    };
    if preview
        .selected_mra_path
        .as_deref()
        .is_some_and(|path| path == game.mra_path)
    {
        request_preview_prefetches(games, selected, preview);
        return false;
    }
    request_arcade_preview_window(bridge, games, selected, preview)
}

pub(crate) fn apply_ready_preview(
    app: &slint_ui::launcher::Launcher,
    preview: &mut PreviewState,
) -> bool {
    if !preview_loading_enabled() {
        for _ in preview.worker.drain() {}
        preview.ready_backlog.clear();
        return false;
    }
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let mut dirty = false;
    preview.ready_backlog.extend(preview.worker.drain());
    let mut selected_processed = false;
    let mut prefetch_results = 0;
    while let Some(idx) = next_ready_result_index(
        &preview.ready_backlog,
        preview.current_generation,
        preview.selected_image_path.as_deref(),
        selected_processed,
        prefetch_results,
    ) {
        let Some(result) = preview.ready_backlog.remove(idx) else {
            break;
        };
        preview.pending_prefetch_paths.remove(&result.image_path);
        let is_selected_result = is_current_selected_result(
            &result,
            preview.current_generation,
            preview.selected_image_path.as_deref(),
        );
        if is_selected_result {
            selected_processed = true;
        } else if matches!(result.priority, PreviewPriority::Prefetch { .. }) {
            prefetch_results += 1;
        }
        if !is_selected_result && matches!(result.priority, PreviewPriority::Selected) {
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace stale_result generation={} current_generation={} path={}",
                    result.generation, preview.current_generation, result.image_path
                );
            }
            continue;
        }
        if let Some(image) = result.image {
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace apply generation={} priority={:?} selected={} age_us={} format={} filter={:?} source={}x{} output={}x{} total_us={} read_us={} decode_us={} resize_us={} encoded_bytes={} decoded_bytes={} path={}",
                    result.generation,
                    result.priority,
                    is_selected_result,
                    result.request_age_us,
                    result.storage_format.label(),
                    result.resize_filter,
                    result.source_width,
                    result.source_height,
                    image.width(),
                    image.height(),
                    result.total_us,
                    result.read_us,
                    result.decode_us,
                    result.resize_us,
                    result.encoded_bytes,
                    result.decoded_bytes,
                    result.image_path
                );
            }
            let source_w = image.width();
            let source_h = image.height();
            let display = preview_display_size(
                source_w,
                source_h,
                ARCADE_PREVIEW_BOX_W,
                ARCADE_PREVIEW_BOX_H,
            );
            let pixels = match image {
                PreviewPixels::Rgb565 {
                    stride_bytes,
                    words,
                    ..
                } => PreviewImagePixels::Rgb565 {
                    pixels: words.into_iter().map(Rgb565Pixel).collect(),
                    stride_pixels: stride_bytes as usize / 2,
                },
            };
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace raw_image generation={} output={}x{} path={}",
                    result.generation, source_w, source_h, result.image_path
                );
            }
            let image = Arc::new(PreviewImage {
                pixels,
                source_w,
                source_h,
                display_w: display.w,
                display_h: display.h,
            });
            let image_path = result.image_path;
            preview.cache.insert(
                image_path.clone(),
                Arc::clone(&image),
                &preview.window_paths,
                Some(&preview.visible_path),
            );
            if is_selected_result {
                preview.current_generation = 0;
                bridge.set_arcade_preview_title(result.title.into());
                preview.has_visible_preview = true;
                preview.begin_raw_transition_to(&image_path);
                preview.visible_path = image_path;
                preview.raw_dirty = true;
                apply_preview_image_bridge(&bridge, &image);
                dirty = true;
            }
        } else {
            preview.cache.insert_failed(result.image_path.clone());
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace cache_failed priority={:?} selected={} path={}",
                    result.priority, is_selected_result, result.image_path
                );
            }
            if is_selected_result {
                preview.select_empty_preview();
                clear_preview_image_bridge(&bridge);
                bridge.set_arcade_preview_status(PreviewStatus::Empty);
                dirty = true;
            }
        }
    }
    dirty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_size_enlarges_only_by_integer_scale() {
        let size = preview_display_size(100, 50, ARCADE_PREVIEW_BOX_W, ARCADE_PREVIEW_BOX_H);
        assert_eq!(size, PreviewDisplaySize { w: 300, h: 150 });
        assert_eq!(size.w % 100, 0);
        assert_eq!(size.h % 50, 0);
        assert!(size.w * size.h <= PREVIEW_MAX_AREA);
    }

    #[test]
    fn preview_size_keeps_large_images_native_for_aperture_clipping() {
        let size = preview_display_size(1920, 1080, ARCADE_PREVIEW_BOX_W, ARCADE_PREVIEW_BOX_H);
        assert_eq!(size, PreviewDisplaySize { w: 1920, h: 1080 });
        assert_eq!(size.w as u64 * 1080, size.h as u64 * 1920);
    }

    #[test]
    fn preview_size_keeps_odd_ratio_integer_dimensions() {
        let size = preview_display_size(321, 225, ARCADE_PREVIEW_BOX_W, ARCADE_PREVIEW_BOX_H);
        assert_eq!(size, PreviewDisplaySize { w: 321, h: 225 });
        assert_eq!(size.w as u64 * 225, size.h as u64 * 321);
    }

    #[test]
    fn preview_size_keeps_common_arcade_screenshot_native_when_resize_is_off() {
        let size = preview_display_size(320, 224, ARCADE_PREVIEW_BOX_W, ARCADE_PREVIEW_BOX_H);
        assert_eq!(size, PreviewDisplaySize { w: 320, h: 224 });
    }

    #[test]
    fn ready_result_selector_prioritizes_current_selected_preview() {
        let backlog = VecDeque::from(vec![
            preview_result(
                10,
                "prefetch-a.png",
                PreviewPriority::Prefetch { distance: 1 },
            ),
            preview_result(11, "selected.png", PreviewPriority::Selected),
            preview_result(
                12,
                "prefetch-b.png",
                PreviewPriority::Prefetch { distance: 2 },
            ),
        ]);

        let idx = next_ready_result_index(&backlog, 11, Some("selected.png"), false, 0);

        assert_eq!(idx, Some(1));
    }

    #[test]
    fn ready_result_selector_limits_prefetches_per_frame() {
        let backlog = VecDeque::from(vec![
            preview_result(
                1,
                "prefetch-a.png",
                PreviewPriority::Prefetch { distance: 1 },
            ),
            preview_result(
                2,
                "prefetch-b.png",
                PreviewPriority::Prefetch { distance: 2 },
            ),
        ]);

        assert_eq!(
            next_ready_result_index(&backlog, 99, Some("selected.png"), false, 0),
            Some(0)
        );
        assert_eq!(
            next_ready_result_index(&backlog, 99, Some("selected.png"), false, 1),
            None
        );
    }

    #[test]
    fn preview_cache_hits_share_image_payload() {
        let mut cache = PreviewImageCache::default();
        let image = Arc::new(PreviewImage {
            pixels: PreviewImagePixels::Rgb565 {
                pixels: vec![Rgb565Pixel(0xffff)],
                stride_pixels: 1,
            },
            source_w: 1,
            source_h: 1,
            display_w: 1,
            display_h: 1,
        });

        cache.insert("preview.png".into(), Arc::clone(&image), &[], None);
        let first_hit = cache.get("preview.png").expect("first cache hit");
        let second_hit = cache.get("preview.png").expect("second cache hit");

        assert!(Arc::ptr_eq(&image, &first_hit));
        assert!(Arc::ptr_eq(&first_hit, &second_hit));
    }

    #[test]
    fn empty_preview_transition_keeps_previous_frame_for_fade_out() {
        let mut preview = PreviewState::new();
        let visible_image = Arc::new(PreviewImage {
            pixels: PreviewImagePixels::Rgb565 {
                pixels: vec![Rgb565Pixel(0xf800)],
                stride_pixels: 1,
            },
            source_w: 1,
            source_h: 1,
            display_w: 1,
            display_h: 1,
        });
        preview.cache.insert(
            "visible.png".into(),
            Arc::clone(&visible_image),
            &[],
            Some("visible.png"),
        );
        preview.selected_image_path = Some("visible.png".into());
        preview.has_visible_preview = true;
        preview.visible_path = "visible.png".into();
        let previous_transition_id = preview.raw_transition_id;

        preview.select_empty_preview();

        assert_eq!(preview.selected_image_path, None);
        assert!(!preview.has_visible_preview);
        assert!(preview.visible_path.is_empty());
        assert!(preview.raw_dirty);
        assert_eq!(
            preview.raw_transition_id,
            previous_transition_id.wrapping_add(1)
        );

        let frame = preview
            .raw_transition_frame()
            .expect("empty preview transition frame");
        assert!(frame.previous.is_some());
        assert!(matches!(frame.current.pixels, PreviewRawPixels::Empty));
    }

    fn preview_result(
        generation: u64,
        image_path: &str,
        priority: PreviewPriority,
    ) -> PreviewResult {
        PreviewResult {
            generation,
            title: image_path.to_string(),
            image_path: image_path.to_string(),
            image: None,
            request_age_us: 0,
            read_us: 0,
            decode_us: 0,
            resize_us: 0,
            total_us: 0,
            encoded_bytes: 0,
            decoded_bytes: 0,
            source_width: 0,
            source_height: 0,
            storage_format: crate::preview_worker::PreviewStorageFormat::RawRgb565,
            resize_filter: crate::preview_worker::PreviewResizeFilter::Off,
            priority,
        }
    }
}
