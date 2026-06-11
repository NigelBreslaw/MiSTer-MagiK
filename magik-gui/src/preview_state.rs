use std::collections::{HashSet, VecDeque};
use std::sync::OnceLock;
use std::time::Instant;

use mister_magik_ui as slint_ui;
use slint::{ComponentHandle, Image, Rgb8Pixel, SharedPixelBuffer};
use slint_ui::launcher::PreviewStatus;

use crate::arcade_catalog::ArcadeGameEntry;
use crate::preview_worker::{
    preview_window_indices, preview_window_paths, PreviewPriority, PreviewWorker,
    DEFAULT_PREVIEW_CACHE_CAP, DEFAULT_PREVIEW_RADIUS,
};
use crate::ui_display::{UI_FB_H, UI_FB_W};

const PREVIEW_MAX_AREA: u32 = (UI_FB_W as u32 * UI_FB_H as u32 * 40) / 100;
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

fn png_to_slint_image(width: u32, height: u32, rgb: Vec<u8>) -> Image {
    let buffer = SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(&rgb, width, height);
    Image::from_rgb8(buffer)
}

#[derive(Clone)]
struct PreviewImage {
    image: Image,
    source_w: u32,
    source_h: u32,
    display_w: u32,
    display_h: u32,
}

#[derive(Default)]
struct PreviewImageCache {
    entries: VecDeque<(String, PreviewImage)>,
    failed_paths: VecDeque<String>,
}

impl PreviewImageCache {
    const FAILED_CAP: usize = 128;

    fn get(&mut self, path: &str) -> Option<PreviewImage> {
        let idx = self.entries.iter().position(|(p, _)| p == path)?;
        let (_, image) = self.entries.remove(idx)?;
        let out = image.clone();
        self.entries.push_back((path.to_string(), image));
        Some(out)
    }

    fn insert(&mut self, path: String, image: PreviewImage, window_paths: &[String]) {
        if let Some(idx) = self.entries.iter().position(|(p, _)| p == &path) {
            self.entries.remove(idx);
        }
        self.entries.push_back((path, image));
        self.retain_window(window_paths);
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

    fn retain_window(&mut self, window_paths: &[String]) {
        if !window_paths.is_empty() {
            self.entries
                .retain(|(path, _)| window_paths.iter().any(|keep| keep == path));
        }
        while self.entries.len() > DEFAULT_PREVIEW_CACHE_CAP {
            self.entries.pop_front();
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
    bridge.set_arcade_preview_image(preview_image.image.clone());
    bridge.set_arcade_preview_has_image(true);
    bridge.set_arcade_preview_status(PreviewStatus::Ready);
    bridge.set_arcade_preview_source_width(preview_image.source_w as i32);
    bridge.set_arcade_preview_source_height(preview_image.source_h as i32);
    bridge.set_arcade_preview_display_width(preview_image.display_w as i32);
    bridge.set_arcade_preview_display_height(preview_image.display_h as i32);
}

fn clear_preview_image_bridge(bridge: &slint_ui::launcher::MisterBridge) {
    bridge.set_arcade_preview_image(Image::default());
    bridge.set_arcade_preview_has_image(false);
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
    window_paths: Vec<String>,
    pending_prefetch_paths: HashSet<String>,
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
            window_paths: Vec::new(),
            pending_prefetch_paths: HashSet::new(),
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
            self.window_paths.clear();
            self.pending_prefetch_paths.clear();
            bridge.set_arcade_preview_has_image(false);
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
}

pub(crate) fn request_arcade_preview(
    bridge: &slint_ui::launcher::MisterBridge,
    games: &[ArcadeGameEntry],
    selected: usize,
    preview: &mut PreviewState,
) {
    let _ = request_arcade_preview_window(bridge, games, selected, preview);
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
    preview.cache.retain_window(&preview.window_paths);

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
                    preview.visible_path = path;
                    apply_preview_image_bridge(bridge, &image);
                    request_preview_prefetches(games, selected, preview);
                    return true;
                }
                if preview.cache.contains_failed(&path) {
                    preview.current_generation = 0;
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
            preview.current_generation = 0;
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
            preview.visible_path = game.image_path.clone();
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
    preview.current_generation = 0;
    preview.selected_image_path = None;
    preview.has_visible_preview = false;
    preview.visible_path.clear();
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
        return false;
    }
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let mut dirty = false;
    for result in preview.worker.drain() {
        preview.pending_prefetch_paths.remove(&result.image_path);
        let is_selected_result = result.generation == preview.current_generation
            && preview
                .selected_image_path
                .as_deref()
                .is_some_and(|path| path == result.image_path);
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
                    image.width,
                    image.height,
                    result.total_us,
                    result.read_us,
                    result.decode_us,
                    result.resize_us,
                    result.encoded_bytes,
                    result.decoded_bytes,
                    result.image_path
                );
            }
            let source_w = image.width;
            let source_h = image.height;
            let display = preview_display_size(
                source_w,
                source_h,
                ARCADE_PREVIEW_BOX_W,
                ARCADE_PREVIEW_BOX_H,
            );
            let slint_image_t = Instant::now();
            let slint_image = png_to_slint_image(source_w, source_h, image.rgb);
            let slint_image_us = slint_image_t.elapsed().as_micros() as u64;
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace slint_image generation={} slint_image_us={} output={}x{} path={}",
                    result.generation, slint_image_us, source_w, source_h, result.image_path
                );
            }
            let image = PreviewImage {
                image: slint_image,
                source_w,
                source_h,
                display_w: display.w,
                display_h: display.h,
            };
            let image_path = result.image_path;
            preview
                .cache
                .insert(image_path.clone(), image.clone(), &preview.window_paths);
            if is_selected_result {
                preview.current_generation = 0;
                bridge.set_arcade_preview_title(result.title.into());
                preview.has_visible_preview = true;
                preview.visible_path = image_path;
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
                preview.current_generation = 0;
                preview.has_visible_preview = false;
                preview.visible_path.clear();
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
}
