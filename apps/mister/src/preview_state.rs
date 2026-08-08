// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use mister_magik_ui as slint_ui;
use slint::ComponentHandle;
use slint::platform::software_renderer::Rgb565Pixel;
use slint_ui::launcher::PreviewStatus;

use crate::arcade_catalog::{ArcadeGameEntry, ArcadeGameView};
use crate::preview_worker::{
    DEFAULT_PREVIEW_CACHE_CAP, DEFAULT_PREVIEW_RADIUS, PreviewLoadSource, PreviewPixels,
    PreviewPriority, PreviewResult, PreviewWorker, preview_asset_cache_key, preview_window_indices,
};
use crate::ui_display::{UI_FB_H, UI_FB_W};

const PREVIEW_MAX_AREA: u32 = (UI_FB_W as u32 * UI_FB_H as u32 * 40) / 100;
const MAX_PREFETCH_RESULTS_PER_FRAME: usize = 1;
const DIRECTIONAL_PREFETCH_TAIL_RADIUS: usize = 2;
const DEFAULT_TURBO_PREVIEW_LOOKAHEAD: usize = 32;
const MAX_TURBO_PREVIEW_LOOKAHEAD: usize = 64;
const TURBO_PREVIEW_BACKTAIL: usize = 4;
const TURBO_PREVIEW_CACHE_CAP: usize = 512;
const TURBO_PREVIEW_TRANSITION_DURATION_NUMERATOR: u32 = 63;
const TURBO_PREVIEW_TRANSITION_DURATION_DENOMINATOR: u32 = 130;
const PREFETCH_SCROLL_SETTLE: Duration = Duration::from_millis(40);
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

fn preview_startup_trace_enabled() -> bool {
    preview_trace_enabled()
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

fn preview_turbo_runway_enabled() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        !matches!(
            std::env::var("MISTER_PREVIEW_TURBO_RUNWAY").as_deref(),
            Ok("0") | Ok("off") | Ok("false") | Ok("no")
        )
    })
}

fn preview_turbo_lookahead() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MISTER_PREVIEW_TURBO_LOOKAHEAD")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(DEFAULT_TURBO_PREVIEW_LOOKAHEAD)
            .clamp(DEFAULT_PREVIEW_RADIUS, MAX_TURBO_PREVIEW_LOOKAHEAD)
    })
}

fn preview_prefetch_allowed(scroll_active: bool) -> bool {
    !scroll_active || preview_turbo_runway_enabled()
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
        words: Arc<[u16]>,
        stride_pixels: usize,
    },
}

const _: () = {
    assert!(std::mem::size_of::<Rgb565Pixel>() == std::mem::size_of::<u16>());
    assert!(std::mem::align_of::<Rgb565Pixel>() == std::mem::align_of::<u16>());
};

fn rgb565_words_as_pixels(words: &[u16]) -> &[Rgb565Pixel] {
    // SAFETY: Rgb565Pixel is repr(transparent) over u16 in Slint's software
    // renderer, with size/alignment asserted above.
    unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<Rgb565Pixel>(), words.len()) }
}

#[derive(Default)]
struct PreviewImageCache {
    entries: VecDeque<(String, Arc<PreviewImage>)>,
    failed_paths: VecDeque<(String, Instant)>,
}

impl PreviewImageCache {
    const FAILED_CAP: usize = 128;
    const FAILED_TTL: Duration = Duration::from_secs(5 * 60);

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
        window_preview_keys: &[String],
        visible_preview_key: Option<&str>,
    ) -> u32 {
        if let Some(idx) = self.entries.iter().position(|(p, _)| p == &path) {
            self.entries.remove(idx);
        }
        self.entries.push_back((path, image));
        self.retain_window(window_preview_keys, visible_preview_key)
    }

    fn insert_failed(&mut self, path: String) {
        self.prune_failed();
        if let Some(idx) = self.failed_paths.iter().position(|(p, _)| p == &path) {
            self.failed_paths.remove(idx);
        }
        self.failed_paths.push_back((path, Instant::now()));
        while self.failed_paths.len() > Self::FAILED_CAP {
            self.failed_paths.pop_front();
        }
    }

    fn clear_failed(&mut self) {
        self.failed_paths.clear();
    }

    fn retain_window(
        &mut self,
        window_preview_keys: &[String],
        visible_preview_key: Option<&str>,
    ) -> u32 {
        let mut evicted = 0;
        let turbo_window = window_preview_keys.len() > DEFAULT_PREVIEW_CACHE_CAP;
        if !window_preview_keys.is_empty() && !turbo_window {
            let before = self.entries.len();
            self.entries.retain(|(path, _)| {
                visible_preview_key.is_some_and(|visible| visible == path)
                    || window_preview_keys.iter().any(|keep| keep == path)
            });
            evicted += before.saturating_sub(self.entries.len()) as u32;
        }
        let cap = if turbo_window {
            TURBO_PREVIEW_CACHE_CAP
        } else {
            DEFAULT_PREVIEW_CACHE_CAP
        };
        while self.entries.len() > cap {
            if self
                .entries
                .front()
                .is_some_and(|(path, _)| visible_preview_key.is_some_and(|visible| visible == path))
                && self.entries.len() > 1
            {
                if let Some(entry) = self.entries.pop_front() {
                    self.entries.push_back(entry);
                }
            } else {
                self.entries.pop_front();
                evicted += 1;
            }
        }
        evicted
    }

    fn contains(&self, path: &str) -> bool {
        self.entries.iter().any(|(p, _)| p == path)
    }

    fn contains_failed(&mut self, path: &str) -> bool {
        self.prune_failed();
        self.failed_paths.iter().any(|(p, _)| p == path)
    }

    fn prune_failed(&mut self) {
        let now = Instant::now();
        self.failed_paths
            .retain(|(_, failed_at)| now.duration_since(*failed_at) < Self::FAILED_TTL);
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

fn preview_image_from_pixels(pixels: PreviewPixels) -> PreviewImage {
    let source_w = pixels.width();
    let source_h = pixels.height();
    let display = preview_display_size(
        source_w,
        source_h,
        ARCADE_PREVIEW_BOX_W,
        ARCADE_PREVIEW_BOX_H,
    );
    let pixels = match pixels {
        PreviewPixels::Rgb565 {
            stride_bytes,
            words,
            ..
        } => PreviewImagePixels::Rgb565 {
            words,
            stride_pixels: stride_bytes as usize / 2,
        },
    };
    PreviewImage {
        pixels,
        source_w,
        source_h,
        display_w: display.w,
        display_h: display.h,
    }
}

pub(crate) struct PreviewState {
    worker: PreviewWorker,
    trace_start: Instant,
    selected_mra_path: Option<String>,
    selected_preview_key: Option<String>,
    terminal_empty: bool,
    current_generation: u64,
    presentation_generation: u64,
    presentation_state: PreviewPresentationState,
    demand: PreviewDemand,
    route: PreviewRoute,
    cache: PreviewImageCache,
    has_visible_preview: bool,
    visible_preview_key: String,
    visible_preview_load_source: &'static str,
    previous_image: Option<Arc<PreviewImage>>,
    previous_was_empty: bool,
    empty_base_commit_pending: bool,
    selection_transition: PreviewSelectionTransition,
    raw_transition_id: u64,
    raw_transition_duration_numerator: u32,
    raw_transition_duration_denominator: u32,
    window_preview_keys: Vec<String>,
    window_shape: Option<PreviewWindowShape>,
    pending_prefetch_keys: HashSet<String>,
    deferred_selected_result: Option<PreviewResult>,
    raw_dirty: bool,
    last_prefetch_selected: Option<usize>,
    prefetch_direction: i8,
    last_prefetch_window: Option<PreviewPrefetchWindow>,
    prefetch_throttle_until: Option<Instant>,
    last_apply_trace: PreviewApplyTrace,
    frame_cache_evictions: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PreviewSelectionTransition {
    #[default]
    InstantOnEntry,
    CrossFade,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewPresentationTarget {
    Image,
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewPresentationState {
    Detached,
    Loading {
        generation: u64,
        retained_image: bool,
    },
    Visible {
        generation: u64,
    },
    Animating {
        generation: u64,
        target: PreviewPresentationTarget,
    },
    RetirementPending {
        generation: u64,
    },
}

impl PreviewPresentationState {
    pub(crate) const fn owns_direct_layer(self) -> bool {
        matches!(
            self,
            Self::Visible { .. }
                | Self::Loading {
                    retained_image: true,
                    ..
                }
                | Self::Animating { .. }
                | Self::RetirementPending { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PreviewDemand {
    #[default]
    Empty,
    Image,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PreviewRoute {
    Eligible,
    Occluded,
    #[default]
    Unavailable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PreviewFrameIntent {
    #[default]
    None,
    Present {
        generation: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PreviewPresentationCommit {
    generation: u64,
    transition_id: u64,
    final_target_presented: bool,
    empty_base_committed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PreviewApplyTrace {
    pub(crate) worker_drained: u32,
    pub(crate) ready_processed: u32,
    pub(crate) selected_processed: u32,
    pub(crate) prefetch_processed: u32,
    pub(crate) stale_results: u32,
    pub(crate) cache_inserts: u32,
    pub(crate) cache_evictions: u32,
    pub(crate) failed_results: u32,
    pub(crate) backlog_len: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewWindowShape {
    selected: usize,
    len: usize,
    radius: usize,
    signature: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewPrefetchWindow {
    selected: usize,
    len: usize,
    direction: i8,
    turbo_active: bool,
}

pub(crate) struct PreviewRawFrame<'a> {
    pub(crate) pixels: PreviewRawPixels<'a>,
    pub(crate) source_w: u32,
    pub(crate) source_h: u32,
    pub(crate) display_w: u32,
    pub(crate) display_h: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewRawFrameStatus {
    Empty,
    Ready,
    Invalid,
}

impl<'a> PreviewRawFrame<'a> {
    pub(crate) fn status(&self) -> PreviewRawFrameStatus {
        if matches!(self.pixels, PreviewRawPixels::Empty) {
            return PreviewRawFrameStatus::Empty;
        }
        if self.source_w == 0 || self.source_h == 0 || self.display_w == 0 || self.display_h == 0 {
            return PreviewRawFrameStatus::Invalid;
        }
        let source_w = self.source_w as usize;
        let source_h = self.source_h as usize;
        match self.pixels {
            PreviewRawPixels::Empty => PreviewRawFrameStatus::Empty,
            PreviewRawPixels::Rgb8(rgb) => {
                if raw_frame_len_is_valid(rgb.len(), source_w, source_h, 3) {
                    PreviewRawFrameStatus::Ready
                } else {
                    PreviewRawFrameStatus::Invalid
                }
            }
            PreviewRawPixels::Rgb565 {
                pixels,
                stride_pixels,
            } => {
                if stride_pixels >= source_w
                    && raw_frame_stride_len_is_valid(pixels.len(), stride_pixels, source_h)
                {
                    PreviewRawFrameStatus::Ready
                } else {
                    PreviewRawFrameStatus::Invalid
                }
            }
        }
    }
}

pub(crate) struct PreviewRawTransitionFrame<'a> {
    pub(crate) previous: Option<PreviewRawFrame<'a>>,
    pub(crate) current: PreviewRawFrame<'a>,
    pub(crate) transition_id: u64,
    pub(crate) duration_numerator: u32,
    pub(crate) duration_denominator: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewTransitionPace {
    Normal,
    Turbo,
}

impl PreviewTransitionPace {
    fn duration_ratio(self) -> (u32, u32) {
        match self {
            Self::Normal => (1, 1),
            Self::Turbo => (
                TURBO_PREVIEW_TRANSITION_DURATION_NUMERATOR,
                TURBO_PREVIEW_TRANSITION_DURATION_DENOMINATOR,
            ),
        }
    }
}

fn preview_transition_pace(turbo_active: bool) -> PreviewTransitionPace {
    if turbo_active {
        PreviewTransitionPace::Turbo
    } else {
        PreviewTransitionPace::Normal
    }
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

fn raw_frame_len_is_valid(
    len: usize,
    source_w: usize,
    source_h: usize,
    bytes_per_pixel: usize,
) -> bool {
    source_w
        .checked_mul(source_h)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .is_some_and(|needed| len >= needed)
}

fn raw_frame_stride_len_is_valid(len: usize, stride_pixels: usize, source_h: usize) -> bool {
    stride_pixels
        .checked_mul(source_h)
        .is_some_and(|needed| len >= needed)
}

impl PreviewState {
    pub(crate) fn new() -> Self {
        Self::new_with_trace_start(Instant::now())
    }

    pub(crate) fn new_with_trace_start(trace_start: Instant) -> Self {
        Self {
            worker: PreviewWorker::new_with_trace_start(trace_start),
            trace_start,
            selected_mra_path: None,
            selected_preview_key: None,
            terminal_empty: true,
            current_generation: 0,
            presentation_generation: 0,
            presentation_state: PreviewPresentationState::Detached,
            demand: PreviewDemand::Empty,
            route: PreviewRoute::Unavailable,
            cache: PreviewImageCache::default(),
            has_visible_preview: false,
            visible_preview_key: String::new(),
            visible_preview_load_source: "none",
            previous_image: None,
            previous_was_empty: false,
            empty_base_commit_pending: false,
            selection_transition: PreviewSelectionTransition::InstantOnEntry,
            raw_transition_id: 0,
            raw_transition_duration_numerator: 1,
            raw_transition_duration_denominator: 1,
            window_preview_keys: Vec::new(),
            window_shape: None,
            pending_prefetch_keys: HashSet::new(),
            deferred_selected_result: None,
            raw_dirty: false,
            last_prefetch_selected: None,
            prefetch_direction: 0,
            last_prefetch_window: None,
            prefetch_throttle_until: None,
            last_apply_trace: PreviewApplyTrace::default(),
            frame_cache_evictions: 0,
        }
    }

    pub(crate) fn clear(&mut self, bridge: &slint_ui::launcher::MisterBridge) {
        if self.presentation_state == PreviewPresentationState::Detached
            && self.selected_mra_path.is_none()
            && self.current_generation == 0
            && !self.has_visible_preview
        {
            self.terminal_empty = true;
            return;
        }
        self.terminal_empty = true;
        self.demand_empty(PreviewTransitionPace::Normal);
        self.selected_mra_path = None;
        self.selected_preview_key = None;
        self.selection_transition = PreviewSelectionTransition::InstantOnEntry;
        self.current_generation = 0;
        self.window_preview_keys.clear();
        self.window_shape = None;
        self.pending_prefetch_keys.clear();
        self.deferred_selected_result = None;
        self.last_prefetch_selected = None;
        self.prefetch_direction = 0;
        self.last_prefetch_window = None;
        self.prefetch_throttle_until = None;
        bridge.set_arcade_preview_placeholder_visible(true);
        bridge.set_arcade_preview_status(PreviewStatus::Empty);
        bridge.set_arcade_preview_title("".into());
        clear_preview_image_bridge(bridge);
    }

    pub(crate) fn trace_cache_state(&self) -> &'static str {
        self.selected_preview_key
            .as_deref()
            .map(|path| {
                if self.visible_preview_key == path {
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

    pub(crate) fn raw_dirty(&self) -> bool {
        self.raw_dirty
    }

    fn raw_frame_from_image(image: &PreviewImage) -> PreviewRawFrame<'_> {
        PreviewRawFrame {
            pixels: match &image.pixels {
                PreviewImagePixels::Rgb565 {
                    words,
                    stride_pixels,
                } => PreviewRawPixels::Rgb565 {
                    pixels: rgb565_words_as_pixels(words),
                    stride_pixels: *stride_pixels,
                },
            },
            source_w: image.source_w,
            source_h: image.source_h,
            display_w: image.display_w,
            display_h: image.display_h,
        }
    }

    fn begin_raw_transition_to(&mut self, next_path: &str, pace: PreviewTransitionPace) {
        if self.visible_preview_key == next_path {
            return;
        }
        let had_visible_preview = !self.visible_preview_key.is_empty();
        self.previous_image = if had_visible_preview {
            self.cache
                .peek_shared(&self.visible_preview_key)
                .map(Arc::clone)
        } else {
            None
        };
        self.previous_was_empty = self.previous_image.is_none()
            && !had_visible_preview
            && self.selection_transition == PreviewSelectionTransition::CrossFade;
        self.raw_transition_id = self.raw_transition_id.wrapping_add(1);
        (
            self.raw_transition_duration_numerator,
            self.raw_transition_duration_denominator,
        ) = pace.duration_ratio();
        self.demand = PreviewDemand::Image;
        let generation = self.next_presentation_generation();
        self.presentation_state = PreviewPresentationState::Animating {
            generation,
            target: PreviewPresentationTarget::Image,
        };
    }

    fn begin_raw_transition_to_empty(&mut self, pace: PreviewTransitionPace) {
        if !self.has_visible_preview && self.visible_preview_key.is_empty() {
            return;
        }
        self.previous_image = if self.has_visible_preview {
            self.cache
                .peek_shared(&self.visible_preview_key)
                .map(Arc::clone)
        } else {
            None
        };
        self.previous_was_empty = false;
        self.has_visible_preview = false;
        self.visible_preview_key.clear();
        self.visible_preview_load_source = "none";
        self.raw_transition_id = self.raw_transition_id.wrapping_add(1);
        (
            self.raw_transition_duration_numerator,
            self.raw_transition_duration_denominator,
        ) = pace.duration_ratio();
        self.raw_dirty = true;
    }

    fn select_empty_preview(&mut self, pace: PreviewTransitionPace) {
        self.current_generation = 0;
        self.selected_preview_key = None;
        self.terminal_empty = true;
        self.demand_empty(pace);
    }

    pub(crate) const fn terminal_empty(&self) -> bool {
        self.terminal_empty
    }

    fn next_presentation_generation(&mut self) -> u64 {
        self.presentation_generation = self.presentation_generation.wrapping_add(1).max(1);
        self.presentation_generation
    }

    fn demand_loading(&mut self) {
        let retained_image = self.presentation_state.owns_direct_layer();
        self.demand = PreviewDemand::Image;
        let generation = self.next_presentation_generation();
        self.presentation_state = PreviewPresentationState::Loading {
            generation,
            retained_image,
        };
    }

    fn demand_empty(&mut self, pace: PreviewTransitionPace) {
        if self.demand == PreviewDemand::Empty
            && matches!(
                self.presentation_state,
                PreviewPresentationState::Detached
                    | PreviewPresentationState::Animating {
                        target: PreviewPresentationTarget::Empty,
                        ..
                    }
                    | PreviewPresentationState::RetirementPending { .. }
            )
        {
            return;
        }
        self.demand = PreviewDemand::Empty;
        let generation = self.next_presentation_generation();
        if self.presentation_state.owns_direct_layer() || self.has_visible_preview {
            self.begin_raw_transition_to_empty(pace);
            self.presentation_state = PreviewPresentationState::Animating {
                generation,
                target: PreviewPresentationTarget::Empty,
            };
            // Empty is not presentation-complete until the cached backing is
            // black and that update has been confirmed.
            self.empty_base_commit_pending = true;
            self.raw_dirty = true;
        } else {
            self.previous_image = None;
            self.previous_was_empty = false;
            self.empty_base_commit_pending = false;
            self.raw_dirty = false;
            self.presentation_state = PreviewPresentationState::Detached;
        }
    }

    pub(crate) const fn presentation_state(&self) -> PreviewPresentationState {
        self.presentation_state
    }

    pub(crate) fn set_route(&mut self, route: PreviewRoute) {
        if self.route == route {
            return;
        }
        self.route = route;
        match route {
            PreviewRoute::Unavailable => {
                if self.presentation_state.owns_direct_layer()
                    && !matches!(
                        self.presentation_state,
                        PreviewPresentationState::RetirementPending { .. }
                    )
                {
                    let generation = self.next_presentation_generation();
                    self.presentation_state =
                        PreviewPresentationState::RetirementPending { generation };
                    self.previous_image = None;
                    self.previous_was_empty = false;
                    self.empty_base_commit_pending = false;
                    self.raw_dirty = false;
                }
            }
            PreviewRoute::Eligible
                if self.demand == PreviewDemand::Image
                    && matches!(
                        self.presentation_state,
                        PreviewPresentationState::Detached
                            | PreviewPresentationState::RetirementPending { .. }
                    ) =>
            {
                let generation = self.next_presentation_generation();
                if self.has_visible_preview {
                    self.raw_transition_id = self.raw_transition_id.wrapping_add(1);
                    self.raw_dirty = true;
                    self.presentation_state = PreviewPresentationState::Animating {
                        generation,
                        target: PreviewPresentationTarget::Image,
                    };
                } else {
                    self.presentation_state = PreviewPresentationState::Loading {
                        generation,
                        retained_image: false,
                    };
                }
            }
            PreviewRoute::Eligible | PreviewRoute::Occluded => {}
        }
    }

    pub(crate) const fn direct_layer_desired(&self) -> bool {
        self.route == PreviewRoute::Eligible
            && self.demand == PreviewDemand::Image
            && matches!(
                self.presentation_state,
                PreviewPresentationState::Visible { .. }
                    | PreviewPresentationState::Loading {
                        retained_image: true,
                        ..
                    }
                    | PreviewPresentationState::Animating { .. }
            )
    }

    pub(crate) const fn retirement_generation(&self) -> Option<u64> {
        match self.presentation_state {
            PreviewPresentationState::RetirementPending { generation } => Some(generation),
            _ => None,
        }
    }

    pub(crate) const fn presentation_generation(&self) -> u64 {
        self.presentation_generation
    }

    pub(crate) const fn presentation_label(&self) -> &'static str {
        match self.presentation_state {
            PreviewPresentationState::Detached => "detached",
            PreviewPresentationState::Loading { .. } => "loading",
            PreviewPresentationState::Visible { .. } => "visible",
            PreviewPresentationState::Animating { .. } => "animating",
            PreviewPresentationState::RetirementPending { .. } => "retirement-pending",
        }
    }

    pub(crate) const fn frame_intent(&self) -> PreviewFrameIntent {
        match (self.route, self.presentation_state) {
            (PreviewRoute::Eligible, PreviewPresentationState::Animating { generation, .. }) => {
                PreviewFrameIntent::Present { generation }
            }
            _ => PreviewFrameIntent::None,
        }
    }

    pub(crate) const fn presentation_requires_present(&self) -> bool {
        matches!(self.frame_intent(), PreviewFrameIntent::Present { .. })
    }

    pub(crate) const fn empty_base_commit_pending(&self) -> bool {
        self.empty_base_commit_pending
    }

    pub(crate) fn presentation_commit(
        &self,
        final_target_presented: bool,
        empty_base_committed: bool,
    ) -> Option<PreviewPresentationCommit> {
        (final_target_presented || empty_base_committed).then_some(PreviewPresentationCommit {
            generation: self.presentation_generation,
            transition_id: self.raw_transition_id,
            final_target_presented,
            empty_base_committed,
        })
    }

    pub(crate) fn confirm_presentation(&mut self, commit: PreviewPresentationCommit) {
        if commit.generation != self.presentation_generation
            || commit.transition_id != self.raw_transition_id
        {
            return;
        }
        if commit.empty_base_committed {
            self.empty_base_commit_pending = false;
        }
        if commit.final_target_presented {
            self.previous_image = None;
            self.previous_was_empty = false;
        }
        if !self.empty_base_commit_pending
            && self.previous_image.is_none()
            && !self.previous_was_empty
        {
            self.presentation_state = match self.demand {
                PreviewDemand::Image => PreviewPresentationState::Visible {
                    generation: self.presentation_generation,
                },
                PreviewDemand::Empty => PreviewPresentationState::RetirementPending {
                    generation: self.presentation_generation,
                },
            };
        }
    }

    pub(crate) fn confirm_retirement(&mut self, generation: u64) {
        if self.presentation_state == (PreviewPresentationState::RetirementPending { generation }) {
            self.presentation_state = PreviewPresentationState::Detached;
        }
    }

    pub(crate) fn raw_frame(&self) -> Option<PreviewRawFrame<'_>> {
        if !self.has_visible_preview {
            return None;
        }
        let image = self.cache.peek(&self.visible_preview_key)?;
        Some(Self::raw_frame_from_image(image))
    }

    pub(crate) fn raw_frame_status(&self) -> PreviewRawFrameStatus {
        self.raw_frame()
            .map(|frame| frame.status())
            .unwrap_or(PreviewRawFrameStatus::Empty)
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
        let previous = self
            .previous_image
            .as_ref()
            .map(|image| Self::raw_frame_from_image(image))
            .or_else(|| {
                self.previous_was_empty.then_some(PreviewRawFrame {
                    pixels: PreviewRawPixels::Empty,
                    source_w: 1,
                    source_h: 1,
                    display_w: ARCADE_PREVIEW_BOX_W,
                    display_h: ARCADE_PREVIEW_BOX_H,
                })
            });
        Some(PreviewRawTransitionFrame {
            previous,
            current,
            transition_id: self.raw_transition_id,
            duration_numerator: self.raw_transition_duration_numerator,
            duration_denominator: self.raw_transition_duration_denominator,
        })
    }
}

fn is_current_selected_result(
    result: &PreviewResult,
    current_generation: u64,
    selected_preview_key: Option<&str>,
) -> bool {
    result.generation == current_generation
        && selected_preview_key.is_some_and(|key| key == result.preview_key())
}

fn game_preview_key(game: &ArcadeGameEntry) -> Option<String> {
    (game.has_preview
        && !game.preview_archive_path.is_empty()
        && !game.preview_asset_key.is_empty())
    .then(|| preview_asset_cache_key(&game.preview_archive_path, &game.preview_asset_key))
}

fn preview_window_keys(games: ArcadeGameView<'_>, selected: usize, radius: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for idx in preview_window_indices(games.len(), selected, radius) {
        if let Some(key) = games.get(idx).and_then(game_preview_key) {
            if seen.insert(key.clone()) {
                out.push(key);
            }
        }
    }
    out
}

fn hash_preview_window_part(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn preview_window_signature(games: ArcadeGameView<'_>, selected: usize, radius: usize) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for idx in preview_window_indices(games.len(), selected, radius) {
        hash ^= idx as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        let Some(game) = games.get(idx) else {
            continue;
        };
        hash ^= u64::from(game.has_preview);
        hash = hash_preview_window_part(hash, game.preview_archive_path.as_bytes());
        hash = hash_preview_window_part(hash, game.preview_asset_key.as_bytes());
        hash = hash_preview_window_part(hash, game.mra_path.as_bytes());
    }
    hash
}

fn preview_window_shape(
    games: ArcadeGameView<'_>,
    selected: usize,
    radius: usize,
) -> PreviewWindowShape {
    PreviewWindowShape {
        selected,
        len: games.len(),
        radius,
        signature: preview_window_signature(games, selected, radius),
    }
}

fn refresh_preview_window(
    games: ArcadeGameView<'_>,
    selected: usize,
    radius: usize,
    preview: &mut PreviewState,
) {
    preview.window_preview_keys = preview_window_keys(games, selected, radius);
    preview.window_shape = Some(preview_window_shape(games, selected, radius));
    preview.frame_cache_evictions += preview.cache.retain_window(
        &preview.window_preview_keys,
        Some(&preview.visible_preview_key),
    );
}

struct PreviewCandidate<'a> {
    index: usize,
    game: &'a ArcadeGameEntry,
    preview_key: String,
}

fn first_preview_candidate(
    games: ArcadeGameView<'_>,
    selected: usize,
    radius: usize,
) -> Option<PreviewCandidate<'_>> {
    let selected_game = games.get(selected)?;
    if let Some(preview_key) = game_preview_key(selected_game) {
        return Some(PreviewCandidate {
            index: selected,
            game: selected_game,
            preview_key,
        });
    }

    preview_window_indices(games.len(), selected, radius)
        .into_iter()
        .filter(|idx| *idx != selected)
        .find_map(|idx| {
            let game = games.get(idx)?;
            let preview_key = game_preview_key(game)?;
            Some(PreviewCandidate {
                index: idx,
                game,
                preview_key,
            })
        })
}

fn first_available_preview_candidate<'a>(
    games: ArcadeGameView<'a>,
    selected: usize,
    radius: usize,
    cache: &mut PreviewImageCache,
) -> Option<PreviewCandidate<'a>> {
    let selected_game = games.get(selected)?;
    if let Some(preview_key) = game_preview_key(selected_game) {
        if !cache.contains_failed(&preview_key) {
            return Some(PreviewCandidate {
                index: selected,
                game: selected_game,
                preview_key,
            });
        }
    } else {
        return None;
    }

    preview_window_indices(games.len(), selected, radius)
        .into_iter()
        .filter(|idx| *idx != selected)
        .find_map(|idx| {
            let game = games.get(idx)?;
            let preview_key = game_preview_key(game)?;
            (!cache.contains_failed(&preview_key)).then_some(PreviewCandidate {
                index: idx,
                game,
                preview_key,
            })
        })
}

fn preview_cache_state_for_candidate(
    preview: &mut PreviewState,
    candidate_key: Option<&str>,
) -> &'static str {
    let Some(candidate_key) = candidate_key else {
        return "no_candidate";
    };
    if preview.visible_preview_key == candidate_key {
        "exact"
    } else if preview.cache.contains(candidate_key) {
        "cached"
    } else if preview.cache.contains_failed(candidate_key) {
        "failed"
    } else if preview.pending_prefetch_keys.contains(candidate_key) {
        "pending"
    } else if preview.has_visible_preview {
        "stale"
    } else {
        "blank"
    }
}

fn preview_coverage_event_for_state(cache_state: &str, has_candidate: bool) -> &'static str {
    match (has_candidate, cache_state) {
        (false, _) => "preview_selection_sample",
        (true, "exact") => "preview_visible_exact",
        (true, "stale") | (true, "cached") | (true, "pending") => "preview_visible_stale",
        (true, "blank") | (true, "failed") => "preview_visible_blank",
        (true, _) => "preview_selection_sample",
    }
}

fn preview_state_is_miss(cache_state: &str, has_candidate: bool) -> bool {
    has_candidate && !matches!(cache_state, "exact")
}

fn trace_preview_coverage_sample(
    preview: &mut PreviewState,
    selected: usize,
    selected_game: &ArcadeGameEntry,
    candidate: Option<&PreviewCandidate<'_>>,
    turbo_active: bool,
) {
    if !preview_startup_trace_enabled() {
        return;
    }
    let candidate_key = candidate.map(|candidate| candidate.preview_key.as_str());
    let cache_state = preview_cache_state_for_candidate(preview, candidate_key);
    let has_candidate = candidate.is_some();
    let event = preview_coverage_event_for_state(cache_state, has_candidate);
    let candidate_index = candidate
        .map(|candidate| candidate.index.to_string())
        .unwrap_or_default();
    let title = candidate
        .map(|candidate| candidate.game.title.as_ref())
        .unwrap_or(selected_game.title.as_ref());
    let system = candidate
        .map(|candidate| candidate.game.system_id.as_ref())
        .unwrap_or(selected_game.system_id.as_ref());
    let asset_key = candidate
        .map(|candidate| candidate.game.preview_asset_key.as_ref())
        .unwrap_or("");
    let visible_asset_key = preview
        .visible_preview_key
        .rsplit('|')
        .next()
        .unwrap_or(preview.visible_preview_key.as_str());
    let pack_state = if preview.visible_preview_load_source == "archive_mem" {
        "archive_mem_ready"
    } else {
        "index_or_cache"
    };
    crate::ui_errln!(
        "startup_timing\t{event}\t{}ms\tsystem={}\tselected_index={}\tcandidate_index={}\ttitle={}\thas_preview={}\tasset_key={}\tvisible_asset_key={}\tgeneration={}\tturbo_active={}\tcache_state={}\tload_source={}\tpack_state={}",
        preview.trace_elapsed_ms(),
        system,
        selected,
        candidate_index,
        title,
        if has_candidate { 1 } else { 0 },
        asset_key,
        visible_asset_key,
        preview.current_generation,
        if turbo_active { 1 } else { 0 },
        cache_state,
        preview.visible_preview_load_source,
        pack_state
    );
    if preview_state_is_miss(cache_state, has_candidate) {
        crate::ui_errln!(
            "startup_timing\tpreview_miss\t{}ms\tsystem={}\tselected_index={}\tcandidate_index={}\ttitle={}\thas_preview=1\tasset_key={}\tvisible_asset_key={}\tgeneration={}\tturbo_active={}\tcache_state={}\tload_source={}\tpack_state={}",
            preview.trace_elapsed_ms(),
            system,
            selected,
            candidate_index,
            title,
            asset_key,
            visible_asset_key,
            preview.current_generation,
            if turbo_active { 1 } else { 0 },
            cache_state,
            preview.visible_preview_load_source,
            pack_state
        );
    }
}

fn next_ready_result_index(
    backlog: &VecDeque<PreviewResult>,
    current_generation: u64,
    selected_preview_key: Option<&str>,
    selected_processed: bool,
    prefetch_results: usize,
    defer_selected_result: bool,
) -> Option<usize> {
    if defer_selected_result {
        if let Some(idx) = backlog.iter().position(|result| {
            matches!(result.priority, PreviewPriority::Selected)
                && !is_current_selected_result(result, current_generation, selected_preview_key)
        }) {
            return Some(idx);
        }
        if prefetch_results < MAX_PREFETCH_RESULTS_PER_FRAME {
            return backlog
                .iter()
                .position(|result| matches!(result.priority, PreviewPriority::Prefetch { .. }));
        }
        return None;
    }

    if !selected_processed {
        if let Some(idx) = backlog.iter().position(|result| {
            is_current_selected_result(result, current_generation, selected_preview_key)
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
    games: ArcadeGameView<'_>,
    selected: usize,
    preview: &mut PreviewState,
    defer_selected_application: bool,
    scroll_active: bool,
    turbo_active: bool,
) -> bool {
    if !preview_loading_enabled() {
        preview.clear(bridge);
        return false;
    }
    let selected_game = games.get(selected);
    let Some(selected_game) = selected_game else {
        preview.selected_mra_path = None;
        preview.select_empty_preview(preview_transition_pace(turbo_active));
        preview.window_preview_keys.clear();
        preview.window_shape = None;
        preview.pending_prefetch_keys.clear();
        preview.last_prefetch_selected = None;
        preview.prefetch_direction = 0;
        preview.last_prefetch_window = None;
        bridge.set_arcade_preview_placeholder_visible(true);
        bridge.set_arcade_preview_status(PreviewStatus::Empty);
        bridge.set_arcade_preview_title("".into());
        clear_preview_image_bridge(bridge);
        return true;
    };
    bridge.set_arcade_preview_placeholder_visible(true);

    let turbo_runway_active = turbo_active && preview_turbo_runway_enabled();
    let prefetch_radius = if turbo_runway_active {
        preview_turbo_lookahead()
    } else {
        DEFAULT_PREVIEW_RADIUS
    };

    let candidate = first_available_preview_candidate(
        games,
        selected,
        DEFAULT_PREVIEW_RADIUS,
        &mut preview.cache,
    );
    let candidate_preview_key = candidate
        .as_ref()
        .map(|candidate| candidate.preview_key.as_str());
    if preview
        .selected_mra_path
        .as_deref()
        .is_some_and(|path| path == selected_game.mra_path.as_ref())
        && preview.selected_preview_key.as_deref() == candidate_preview_key
    {
        let window_shape = preview_window_shape(games, selected, prefetch_radius);
        if preview.window_shape != Some(window_shape) {
            refresh_preview_window(games, selected, prefetch_radius, preview);
        }
        if let Some(path) = preview.selected_preview_key.clone() {
            if preview.visible_preview_key != path {
                if let Some(image) = preview.cache.get(&path) {
                    if defer_selected_application {
                        request_preview_prefetches_if_allowed(
                            games,
                            selected,
                            preview,
                            scroll_active,
                            turbo_active,
                        );
                        return false;
                    }
                    if let Some(candidate) = candidate.as_ref() {
                        bridge.set_arcade_preview_title(candidate.game.title.as_ref().into());
                    }
                    preview.current_generation = 0;
                    preview.has_visible_preview = true;
                    preview.begin_raw_transition_to(&path, preview_transition_pace(turbo_active));
                    preview.visible_preview_key = path;
                    preview.visible_preview_load_source = "decoded_cache";
                    preview.raw_dirty = true;
                    apply_preview_image_bridge(bridge, &image);
                    request_preview_prefetches_if_allowed(
                        games,
                        selected,
                        preview,
                        scroll_active,
                        turbo_active,
                    );
                    trace_preview_coverage_sample(
                        preview,
                        selected,
                        selected_game,
                        candidate.as_ref(),
                        turbo_active,
                    );
                    return true;
                }
                if preview.cache.contains_failed(&path) {
                    preview.select_empty_preview(preview_transition_pace(turbo_active));
                    bridge.set_arcade_preview_status(PreviewStatus::Empty);
                    request_preview_prefetches_if_allowed(
                        games,
                        selected,
                        preview,
                        scroll_active,
                        turbo_active,
                    );
                    trace_preview_coverage_sample(
                        preview,
                        selected,
                        selected_game,
                        candidate.as_ref(),
                        turbo_active,
                    );
                    return true;
                }
            }
        }
        request_preview_prefetches_if_allowed(
            games,
            selected,
            preview,
            scroll_active,
            turbo_active,
        );
        trace_preview_coverage_sample(
            preview,
            selected,
            selected_game,
            candidate.as_ref(),
            turbo_active,
        );
        return false;
    }
    refresh_preview_window(games, selected, prefetch_radius, preview);
    preview.selection_transition = if preview.selected_mra_path.is_some() {
        PreviewSelectionTransition::CrossFade
    } else {
        PreviewSelectionTransition::InstantOnEntry
    };
    preview.selected_mra_path = Some(selected_game.mra_path.to_string());
    preview.terminal_empty = false;

    let selected_has_preview = game_preview_key(selected_game).is_some();
    let Some(candidate) = candidate else {
        if preview_startup_trace_enabled() {
            crate::ui_errln!(
                "startup_timing\tpreview_selected_candidate\t{}ms\tsystem={}\tselected_index={}\ttitle={}\thas_preview=0\tasset_key=\tcandidate_index=\tselected_has_preview={}",
                preview.trace_elapsed_ms(),
                selected_game.system_id,
                selected,
                selected_game.title,
                if selected_has_preview { 1 } else { 0 }
            );
        }
        preview.select_empty_preview(preview_transition_pace(turbo_active));
        bridge.set_arcade_preview_placeholder_visible(true);
        clear_preview_image_bridge(bridge);
        bridge.set_arcade_preview_status(PreviewStatus::Empty);
        request_preview_prefetches_if_allowed(
            games,
            selected,
            preview,
            scroll_active,
            turbo_active,
        );
        trace_preview_coverage_sample(preview, selected, selected_game, None, turbo_active);
        return true;
    };

    let candidate_game = candidate.game;
    bridge.set_arcade_preview_title(candidate_game.title.as_ref().into());
    if preview_startup_trace_enabled() {
        crate::ui_errln!(
            "startup_timing\tpreview_selected_candidate\t{}ms\tsystem={}\tselected_index={}\ttitle={}\thas_preview=1\tasset_key={}\tcandidate_index={}\tselected_has_preview={}",
            preview.trace_elapsed_ms(),
            candidate_game.system_id,
            selected,
            candidate_game.title,
            candidate_game.preview_asset_key,
            candidate.index,
            if selected_has_preview { 1 } else { 0 }
        );
    }
    let preview_key = candidate.preview_key.clone();
    preview.selected_preview_key = Some(preview_key.clone());
    if preview.cache.contains_failed(&preview_key) {
        preview.select_empty_preview(preview_transition_pace(turbo_active));
        bridge.set_arcade_preview_status(PreviewStatus::Empty);
        request_preview_prefetches_if_allowed(
            games,
            selected,
            preview,
            scroll_active,
            turbo_active,
        );
        trace_preview_coverage_sample(
            preview,
            selected,
            selected_game,
            Some(&candidate),
            turbo_active,
        );
        return true;
    }
    if let Some(image) = preview.cache.get(&preview_key) {
        if defer_selected_application {
            request_preview_prefetches_if_allowed(
                games,
                selected,
                preview,
                scroll_active,
                turbo_active,
            );
            return false;
        }
        preview.current_generation = 0;
        preview.has_visible_preview = true;
        preview.visible_preview_load_source = "decoded_cache";
        if preview_trace_enabled() {
            crate::ui_errln!(
                "preview_trace cache_hit title={} archive_path={} asset_key={}",
                candidate_game.title,
                candidate_game.preview_archive_path,
                candidate_game.preview_asset_key
            );
        }
        if preview_startup_trace_enabled() {
            crate::ui_errln!(
                "startup_timing\tpreview_selected_applied\t{}ms\tsystem={}\tselected_index={}\ttitle={}\thas_preview=1\tasset_key={}\tgeneration=0\tload_source=decoded_cache\ttotal_us=0\tread_us=0\tdecode_us=0\tage_us=0",
                preview.trace_elapsed_ms(),
                candidate_game.system_id,
                selected,
                candidate_game.title,
                candidate_game.preview_asset_key
            );
        }
        preview.begin_raw_transition_to(&preview_key, preview_transition_pace(turbo_active));
        preview.visible_preview_key = preview_key;
        preview.raw_dirty = true;
        apply_preview_image_bridge(bridge, &image);
        request_preview_prefetches_if_allowed(
            games,
            selected,
            preview,
            scroll_active,
            turbo_active,
        );
        trace_preview_coverage_sample(
            preview,
            selected,
            selected_game,
            Some(&candidate),
            turbo_active,
        );
        return true;
    }
    let requested_at_ms = preview.trace_elapsed_ms();
    if preview_startup_trace_enabled() {
        crate::ui_errln!(
            "startup_timing\tpreview_selected_requested\t{}ms\tsystem={}\tselected_index={}\ttitle={}\thas_preview=1\tasset_key={}\tgeneration=0",
            requested_at_ms,
            candidate_game.system_id,
            selected,
            candidate_game.title,
            candidate_game.preview_asset_key
        );
    }
    if turbo_active {
        if let Some(loaded) = preview.worker.load_decoded_cache_asset(
            candidate_game.preview_archive_path.as_ref(),
            candidate_game.preview_asset_key.as_ref(),
        ) {
            let completed_at_ms = preview.trace_elapsed_ms();
            let load_source = loaded.load_source;
            let total_us = loaded.total_us;
            let read_us = loaded.read_us;
            let decode_us = loaded.decode_us;
            let raw565_parse_us = loaded.raw565_parse_us;
            let age_us = completed_at_ms.saturating_sub(requested_at_ms) * 1000;
            if preview_startup_trace_enabled() {
                crate::ui_errln!(
                    "startup_timing\tpreview_selected_decoded\t{}ms\tsystem={}\ttitle={}\thas_preview=1\tasset_key={}\tgeneration=0\tload_source={}\ttotal_us={}\tread_us={}\tdecode_us={}\traw565_parse_us={}\tage_us={}",
                    completed_at_ms,
                    candidate_game.system_id,
                    candidate_game.title,
                    candidate_game.preview_asset_key,
                    load_source.label(),
                    total_us,
                    read_us,
                    decode_us,
                    raw565_parse_us,
                    age_us
                );
            }
            let loaded_image = Arc::new(preview_image_from_pixels(loaded.pixels));
            preview.frame_cache_evictions += preview.cache.insert(
                preview_key.clone(),
                Arc::clone(&loaded_image),
                &preview.window_preview_keys,
                Some(&preview.visible_preview_key),
            );
            preview.current_generation = 0;
            preview.selected_preview_key = Some(preview_key.clone());
            preview.has_visible_preview = true;
            preview.visible_preview_load_source = load_source.label();
            preview.begin_raw_transition_to(&preview_key, preview_transition_pace(turbo_active));
            preview.visible_preview_key = preview_key;
            preview.raw_dirty = true;
            apply_preview_image_bridge(bridge, &loaded_image);
            if preview_startup_trace_enabled() {
                crate::ui_errln!(
                    "startup_timing\tpreview_selected_applied\t{}ms\tsystem={}\tselected_index={}\ttitle={}\thas_preview=1\tasset_key={}\tgeneration=0\tload_source={}\ttotal_us={}\tread_us={}\tdecode_us={}\tage_us={}",
                    preview.trace_elapsed_ms(),
                    candidate_game.system_id,
                    selected,
                    candidate_game.title,
                    candidate_game.preview_asset_key,
                    load_source.label(),
                    total_us,
                    read_us,
                    decode_us,
                    age_us
                );
            }
            trace_preview_coverage_sample(
                preview,
                selected,
                selected_game,
                Some(&candidate),
                turbo_active,
            );
            return true;
        }
    }
    request_selected_preview_async(preview, selected, selected_game, &candidate);
    trace_preview_coverage_sample(
        preview,
        selected,
        selected_game,
        Some(&candidate),
        turbo_active,
    );
    false
}

fn request_selected_preview_async(
    preview: &mut PreviewState,
    selected: usize,
    selected_game: &ArcadeGameEntry,
    candidate: &PreviewCandidate<'_>,
) {
    let generation = preview.worker.request_selected(
        candidate.game.title.to_string(),
        candidate.game.preview_archive_path.to_string(),
        candidate.game.preview_asset_key.to_string(),
    );
    preview.current_generation = generation;
    preview.demand_loading();
    if preview_startup_trace_enabled() {
        crate::ui_errln!(
            "startup_timing\tpreview_selected_async_requested\t{}ms\tsystem={}\tselected_index={}\ttitle={}\thas_preview=1\tasset_key={}\tgeneration={}",
            preview.trace_elapsed_ms(),
            selected_game.system_id,
            selected,
            candidate.game.title,
            candidate.game.preview_asset_key,
            generation
        );
    }
}

impl PreviewState {
    fn trace_elapsed_ms(&self) -> u64 {
        self.trace_start.elapsed().as_millis() as u64
    }

    pub(crate) fn clear_failed_preview_cache(&mut self) {
        self.cache.clear_failed();
    }

    pub(crate) fn last_apply_trace(&self) -> PreviewApplyTrace {
        self.last_apply_trace
    }

    pub(crate) fn take_frame_cache_evictions(&mut self) -> u32 {
        let evictions = self.frame_cache_evictions;
        self.frame_cache_evictions = 0;
        evictions
    }
}

fn request_preview_prefetches_if_allowed(
    games: ArcadeGameView<'_>,
    selected: usize,
    preview: &mut PreviewState,
    scroll_active: bool,
    turbo_active: bool,
) {
    if preview_prefetch_allowed(scroll_active) {
        request_preview_prefetches(games, selected, preview, turbo_active);
    }
}

fn request_preview_prefetches(
    games: ArcadeGameView<'_>,
    selected: usize,
    preview: &mut PreviewState,
    turbo_active: bool,
) {
    let turbo_runway_for_prefetch = turbo_active && preview_turbo_runway_enabled();
    let turbo_active = turbo_runway_for_prefetch;
    let selected_changed = preview
        .last_prefetch_selected
        .is_some_and(|previous| previous != selected);
    if let Some(previous) = preview.last_prefetch_selected {
        if selected > previous {
            preview.prefetch_direction = 1;
        } else if selected < previous {
            preview.prefetch_direction = -1;
        }
    }
    preview.last_prefetch_selected = Some(selected);
    if prefetch_should_throttle(preview, selected_changed, turbo_runway_for_prefetch) {
        return;
    }
    if turbo_active {
        prune_pending_prefetch_keys_for_turbo(games, selected, preview);
    }
    let window = PreviewPrefetchWindow {
        selected,
        len: games.len(),
        direction: preview.prefetch_direction,
        turbo_active,
    };
    if preview.last_prefetch_window == Some(window)
        && prefetch_window_is_covered(games, selected, preview, turbo_active)
    {
        return;
    }
    preview.last_prefetch_window = Some(window);

    for (rank, idx) in direction_aware_prefetch_indices(
        games.len(),
        selected,
        if turbo_active {
            preview_turbo_lookahead()
        } else {
            DEFAULT_PREVIEW_RADIUS
        },
        preview.prefetch_direction,
        if turbo_active {
            TURBO_PREVIEW_BACKTAIL
        } else {
            DIRECTIONAL_PREFETCH_TAIL_RADIUS
        },
    )
    .into_iter()
    .enumerate()
    {
        let Some(game) = games.get(idx) else {
            continue;
        };
        let Some(preview_key) = game_preview_key(game) else {
            continue;
        };
        if preview.cache.contains(&preview_key)
            || preview.cache.contains_failed(&preview_key)
            || preview.pending_prefetch_keys.contains(&preview_key)
        {
            continue;
        }
        let distance = idx.abs_diff(selected);
        if preview.worker.request_prefetch(
            game.title.to_string(),
            game.preview_archive_path.to_string(),
            game.preview_asset_key.to_string(),
            rank + 1,
        ) {
            preview.pending_prefetch_keys.insert(preview_key);
        } else {
            continue;
        }
        if preview_trace_enabled() {
            crate::ui_errln!(
                "preview_trace prefetch distance={} rank={} direction={} title={} archive_path={} asset_key={}",
                distance,
                rank + 1,
                preview.prefetch_direction,
                game.title,
                game.preview_archive_path,
                game.preview_asset_key
            );
        }
    }
}

fn prune_pending_prefetch_keys_for_turbo(
    games: ArcadeGameView<'_>,
    selected: usize,
    preview: &mut PreviewState,
) {
    let keep: HashSet<String> = direction_aware_prefetch_indices(
        games.len(),
        selected,
        preview_turbo_lookahead(),
        preview.prefetch_direction,
        TURBO_PREVIEW_BACKTAIL,
    )
    .into_iter()
    .filter_map(|idx| games.get(idx))
    .filter_map(game_preview_key)
    .collect();
    preview
        .pending_prefetch_keys
        .retain(|preview_key| keep.contains(preview_key));
}

fn prefetch_should_throttle(
    preview: &mut PreviewState,
    selected_changed: bool,
    turbo_active: bool,
) -> bool {
    if turbo_active {
        preview.prefetch_throttle_until = None;
        return false;
    }
    let now = Instant::now();
    if selected_changed {
        preview.prefetch_throttle_until = Some(now + PREFETCH_SCROLL_SETTLE);
        preview.pending_prefetch_keys.clear();
        if preview_trace_enabled() {
            crate::ui_errln!(
                "preview_trace prefetch_throttled reason=selection_changed duration_ms={}",
                PREFETCH_SCROLL_SETTLE.as_millis()
            );
        }
        return true;
    }
    if let Some(until) = preview.prefetch_throttle_until {
        if now < until {
            if preview_trace_enabled() {
                let remaining_ms = until.duration_since(now).as_millis();
                crate::ui_errln!(
                    "preview_trace prefetch_throttled reason=settle_wait remaining_ms={remaining_ms}"
                );
            }
            return true;
        }
        preview.prefetch_throttle_until = None;
    }
    false
}

fn prefetch_window_is_covered(
    games: ArcadeGameView<'_>,
    selected: usize,
    preview: &mut PreviewState,
    turbo_active: bool,
) -> bool {
    direction_aware_prefetch_indices(
        games.len(),
        selected,
        if turbo_active {
            preview_turbo_lookahead()
        } else {
            DEFAULT_PREVIEW_RADIUS
        },
        preview.prefetch_direction,
        if turbo_active {
            TURBO_PREVIEW_BACKTAIL
        } else {
            DIRECTIONAL_PREFETCH_TAIL_RADIUS
        },
    )
    .into_iter()
    .filter_map(|idx| games.get(idx))
    .filter_map(game_preview_key)
    .all(|preview_key| {
        preview.cache.contains(&preview_key)
            || preview.cache.contains_failed(&preview_key)
            || preview.pending_prefetch_keys.contains(&preview_key)
    })
}

fn direction_aware_prefetch_indices(
    len: usize,
    selected: usize,
    radius: usize,
    direction: i8,
    tail_radius: usize,
) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let selected = selected.min(len - 1);
    if direction == 0 {
        return preview_window_indices(len, selected, radius)
            .into_iter()
            .filter(|idx| *idx != selected)
            .collect();
    }

    let mut out = Vec::new();
    let mut push_unique = |idx: usize| {
        if idx < len && idx != selected && !out.contains(&idx) {
            out.push(idx);
        }
    };

    if direction > 0 {
        let end = selected.saturating_add(radius).min(len - 1);
        for idx in selected.saturating_add(1)..=end {
            push_unique(idx);
        }
        for distance in 1..=tail_radius.min(radius) {
            if let Some(idx) = selected.checked_sub(distance) {
                push_unique(idx);
            }
        }
    } else {
        for distance in 1..=radius {
            if let Some(idx) = selected.checked_sub(distance) {
                push_unique(idx);
            } else {
                break;
            }
        }
        let end = selected
            .saturating_add(tail_radius.min(radius))
            .min(len - 1);
        for idx in selected.saturating_add(1)..=end {
            push_unique(idx);
        }
    }

    out
}

fn deferred_selected_preview_is_ready(preview: &mut PreviewState) -> bool {
    let Some(path) = preview.selected_preview_key.as_deref() else {
        return false;
    };
    preview.visible_preview_key != path
        && (preview.cache.contains(path) || preview.cache.contains_failed(path))
}

fn same_selected_preview_needs_request(
    preview: &mut PreviewState,
    defer_selected_application: bool,
) -> bool {
    !defer_selected_application && deferred_selected_preview_is_ready(preview)
}

pub(crate) fn schedule_arcade_preview_window(
    bridge: &slint_ui::launcher::MisterBridge,
    games: ArcadeGameView<'_>,
    selected: usize,
    preview: &mut PreviewState,
    defer_selected_application: bool,
    scroll_active: bool,
    turbo_active: bool,
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
        .is_some_and(|path| path == game.mra_path.as_ref())
    {
        if same_selected_preview_needs_request(preview, defer_selected_application) {
            return request_arcade_preview_window(
                bridge,
                games,
                selected,
                preview,
                defer_selected_application,
                scroll_active,
                turbo_active,
            );
        }
        request_preview_prefetches_if_allowed(
            games,
            selected,
            preview,
            scroll_active,
            turbo_active,
        );
        return false;
    }
    request_arcade_preview_window(
        bridge,
        games,
        selected,
        preview,
        defer_selected_application,
        scroll_active,
        turbo_active,
    )
}

pub(crate) fn prewarm_arcade_selected_preview(
    games: ArcadeGameView<'_>,
    selected: usize,
    preview: &mut PreviewState,
) -> bool {
    if !preview_loading_enabled() {
        return false;
    }
    let Some(selected_game) = games.get(selected) else {
        return false;
    };
    let Some(candidate) = first_available_preview_candidate(
        games,
        selected,
        DEFAULT_PREVIEW_RADIUS,
        &mut preview.cache,
    ) else {
        return false;
    };
    let preview_key = candidate.preview_key.clone();
    preview.selected_mra_path = Some(selected_game.mra_path.to_string());
    preview.selected_preview_key = Some(preview_key.clone());
    if preview.cache.contains(&preview_key) || preview.cache.contains_failed(&preview_key) {
        return false;
    }
    let generation = preview.worker.request_selected(
        candidate.game.title.to_string(),
        candidate.game.preview_archive_path.to_string(),
        candidate.game.preview_asset_key.to_string(),
    );
    preview.current_generation = generation;
    preview.demand_loading();
    crate::ui_errln!(
        "startup_timing\tpreview_selected_prewarm_requested\t{}ms\tsystem={}\tselected_index={}\ttitle={}\thas_preview=1\tasset_key={}\tgeneration={}",
        preview.trace_elapsed_ms(),
        candidate.game.system_id,
        selected,
        candidate.game.title,
        candidate.game.preview_asset_key,
        generation
    );
    true
}

pub(crate) fn apply_ready_preview(
    app: &slint_ui::launcher::Launcher,
    preview: &mut PreviewState,
    defer_selected_result: bool,
    turbo_active: bool,
) -> bool {
    preview.last_apply_trace = PreviewApplyTrace::default();
    if !preview_loading_enabled() {
        preview.worker.discard_ready_results();
        preview.deferred_selected_result = None;
        return false;
    }
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let mut dirty = false;
    let mut ready_backlog = VecDeque::new();
    if !defer_selected_result {
        if let Some(result) = preview.deferred_selected_result.take() {
            ready_backlog.push_front(result);
        }
    }
    if let Some(result) = preview.worker.take_latest_selected_result() {
        preview.last_apply_trace.worker_drained += 1;
        if defer_selected_result
            && is_current_selected_result(
                &result,
                preview.current_generation,
                preview.selected_preview_key.as_deref(),
            )
        {
            preview.deferred_selected_result = Some(result);
        } else {
            ready_backlog.push_front(result);
        }
    }
    if let Some(result) = preview.worker.take_prefetch_result() {
        preview.last_apply_trace.worker_drained += 1;
        ready_backlog.push_back(result);
    }
    preview.last_apply_trace.backlog_len =
        ready_backlog.len() as u32 + u32::from(preview.deferred_selected_result.is_some());
    let mut selected_processed = false;
    let mut prefetch_results = 0;
    while let Some(idx) = next_ready_result_index(
        &ready_backlog,
        preview.current_generation,
        preview.selected_preview_key.as_deref(),
        selected_processed,
        prefetch_results,
        defer_selected_result,
    ) {
        let Some(result) = ready_backlog.remove(idx) else {
            break;
        };
        preview.last_apply_trace.ready_processed += 1;
        let result_preview_key = result.preview_key();
        preview.pending_prefetch_keys.remove(&result_preview_key);
        let is_selected_result = is_current_selected_result(
            &result,
            preview.current_generation,
            preview.selected_preview_key.as_deref(),
        );
        if is_selected_result {
            selected_processed = true;
            preview.last_apply_trace.selected_processed += 1;
        } else if matches!(result.priority, PreviewPriority::Prefetch { .. }) {
            prefetch_results += 1;
            preview.last_apply_trace.prefetch_processed += 1;
        }
        if !is_selected_result && matches!(result.priority, PreviewPriority::Selected) {
            preview.last_apply_trace.stale_results += 1;
            if preview_trace_enabled() {
                crate::ui_errln!(
                    "preview_trace stale_result generation={} current_generation={} archive_path={} asset_key={}",
                    result.generation,
                    preview.current_generation,
                    result.preview_archive_path,
                    result.preview_asset_key
                );
            }
            continue;
        }
        let result_system_id = preview_result_system_id(&result);
        let result_title = result.title.clone();
        if let Some(image) = result.image {
            if preview_trace_enabled() && is_selected_result {
                crate::ui_errln!(
                    "preview_trace apply generation={} priority={:?} selected={} age_us={} load_source={} total_us={} read_us={} decode_us={} archive_path={} asset_key={}",
                    result.generation,
                    result.priority,
                    is_selected_result,
                    result.request_age_us,
                    result.load_source.label(),
                    result.total_us,
                    result.read_us,
                    result.decode_us,
                    result.preview_archive_path,
                    result.preview_asset_key
                );
            }
            if preview_startup_trace_enabled() && is_selected_result {
                if result.load_source == PreviewLoadSource::IndexPread {
                    crate::ui_errln!(
                        "startup_timing\tpreview_sidecar_ready\t{}ms\tsystem={}\ttitle={}\thas_preview=1\tasset_key={}\tgeneration={}\tload_source={}\tread_us={}",
                        result.completed_at_ms,
                        result_system_id,
                        result_title,
                        result.preview_asset_key,
                        result.generation,
                        result.load_source.label(),
                        result.read_us
                    );
                }
                crate::ui_errln!(
                    "startup_timing\tpreview_selected_decoded\t{}ms\tsystem={}\ttitle={}\thas_preview=1\tasset_key={}\tgeneration={}\tload_source={}\ttotal_us={}\tread_us={}\tdecode_us={}\traw565_parse_us={}\tage_us={}",
                    result.completed_at_ms,
                    result_system_id,
                    result_title,
                    result.preview_asset_key,
                    result.generation,
                    result.load_source.label(),
                    result.total_us,
                    result.read_us,
                    result.decode_us,
                    result.raw565_parse_us,
                    result.request_age_us
                );
            }
            let source_w = image.width();
            let source_h = image.height();
            if preview_trace_enabled() && is_selected_result {
                crate::ui_errln!(
                    "preview_trace raw_image generation={} output={}x{} archive_path={} asset_key={}",
                    result.generation,
                    source_w,
                    source_h,
                    result.preview_archive_path,
                    result.preview_asset_key
                );
            }
            let image = Arc::new(preview_image_from_pixels(image));
            preview.last_apply_trace.cache_inserts += 1;
            let cache_evictions = preview.cache.insert(
                result_preview_key.clone(),
                Arc::clone(&image),
                &preview.window_preview_keys,
                Some(&preview.visible_preview_key),
            );
            preview.last_apply_trace.cache_evictions += cache_evictions;
            preview.frame_cache_evictions += cache_evictions;
            if is_selected_result {
                preview.current_generation = 0;
                bridge.set_arcade_preview_title(result_title.clone().into());
                preview.has_visible_preview = true;
                preview.visible_preview_load_source = result.load_source.label();
                preview.begin_raw_transition_to(
                    &result_preview_key,
                    preview_transition_pace(turbo_active),
                );
                preview.visible_preview_key = result_preview_key;
                preview.raw_dirty = true;
                apply_preview_image_bridge(&bridge, &image);
                if preview_startup_trace_enabled() {
                    crate::ui_errln!(
                        "startup_timing\tpreview_selected_applied\t{}ms\tsystem={}\ttitle={}\thas_preview=1\tasset_key={}\tgeneration={}\tload_source={}\ttotal_us={}\tread_us={}\tdecode_us={}\tage_us={}",
                        preview.trace_elapsed_ms(),
                        result_system_id,
                        result_title,
                        result.preview_asset_key,
                        result.generation,
                        result.load_source.label(),
                        result.total_us,
                        result.read_us,
                        result.decode_us,
                        result.request_age_us
                    );
                }
                dirty = true;
            }
        } else {
            preview.last_apply_trace.failed_results += 1;
            preview.cache.insert_failed(result_preview_key);
            if preview_trace_enabled() {
                crate::ui_errln!(
                    "preview_trace cache_failed priority={:?} selected={} archive_path={} asset_key={}",
                    result.priority,
                    is_selected_result,
                    result.preview_archive_path,
                    result.preview_asset_key
                );
            }
            if is_selected_result {
                preview.select_empty_preview(preview_transition_pace(turbo_active));
                clear_preview_image_bridge(&bridge);
                bridge.set_arcade_preview_status(PreviewStatus::Empty);
                dirty = true;
            }
        }
    }
    preview.last_apply_trace.backlog_len =
        ready_backlog.len() as u32 + u32::from(preview.deferred_selected_result.is_some());
    dirty
}

fn preview_result_system_id(result: &PreviewResult) -> &'static str {
    let path = result.preview_archive_path.as_str();
    if path.contains("neogeo-screenshots") {
        "neogeo"
    } else if path.contains("saturn-screenshots") {
        "saturn"
    } else if path.contains("amiga-screenshots") {
        "amiga"
    } else if path.contains("atarilynx-screenshots") {
        "atarilynx"
    } else if path.contains("arcade-screenshots") {
        "arcade"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_runner::ui_platform::{MisterPlatform, MisterSoftwareWindow};
    use slint::ComponentHandle;
    use slint::platform::software_renderer::RepaintBufferType;
    use std::cell::Cell;
    use std::rc::Rc;

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
    fn rgb565_words_as_pixels_borrows_shared_payload() {
        let words = Arc::<[u16]>::from([0x001f, 0x07e0, 0xf800]);
        let ptr = words.as_ptr();

        let pixels = rgb565_words_as_pixels(&words);

        assert_eq!(pixels.as_ptr().cast::<u16>(), ptr);
        assert_eq!(pixels.len(), words.len());
        assert_eq!(
            pixels.iter().map(|p| p.0).collect::<Vec<_>>(),
            [0x001f, 0x07e0, 0xf800]
        );
    }

    #[test]
    fn first_preview_candidate_uses_selected_row_when_it_has_preview() {
        let games = vec![
            preview_game("Selected", "selected.mra", "selected.png", true),
            preview_game("Next", "next.mra", "next.png", true),
        ];

        let candidate = first_preview_candidate(ArcadeGameView::contiguous(&games), 0, 4)
            .expect("selected preview candidate");

        assert_eq!(candidate.index, 0);
        assert_eq!(candidate.game.title.as_ref(), "Selected");
        assert!(candidate.preview_key.ends_with("selected.png"));
    }

    #[test]
    fn first_preview_candidate_uses_first_window_row_with_preview() {
        let games = vec![
            preview_game("No Preview", "none.mra", "", false),
            preview_game("First Preview", "first.mra", "first.png", true),
            preview_game("Second Preview", "second.mra", "second.png", true),
        ];

        let candidate = first_preview_candidate(ArcadeGameView::contiguous(&games), 0, 4)
            .expect("fallback preview candidate");

        assert_eq!(candidate.index, 1);
        assert_eq!(candidate.game.title.as_ref(), "First Preview");
        assert!(candidate.preview_key.ends_with("first.png"));
    }

    #[test]
    fn first_preview_candidate_returns_none_for_empty_system() {
        assert!(
            first_preview_candidate(ArcadeGameView::empty(), 0, 4).is_none(),
            "empty systems have no preview candidate"
        );
    }

    #[test]
    fn available_preview_candidate_does_not_substitute_when_selected_has_no_preview() {
        let games = vec![
            preview_game("No Preview", "none.mra", "", false),
            preview_game("Neighbor Preview", "neighbor.mra", "neighbor.png", true),
        ];
        let mut cache = PreviewImageCache::default();

        assert!(
            first_available_preview_candidate(ArcadeGameView::contiguous(&games), 0, 4, &mut cache)
                .is_none(),
            "selected rows without screenshots should fade to empty, not borrow neighbors"
        );
    }

    #[test]
    fn first_preview_candidate_ignores_missing_pack_or_asset() {
        let mut missing_pack = preview_game("Missing Pack", "missing-pack.mra", "pack.png", true);
        missing_pack.preview_archive_path = "".into();
        let games = vec![
            missing_pack,
            preview_game("Missing Asset", "missing-asset.mra", "", true),
        ];

        assert!(
            first_preview_candidate(ArcadeGameView::contiguous(&games), 0, 4).is_none(),
            "malformed preview metadata is not a usable candidate"
        );
    }

    #[test]
    fn available_preview_candidate_skips_known_failed_assets() {
        let games = vec![
            preview_game("Missing In Pack", "missing.mra", "missing.png", true),
            preview_game("Fallback", "fallback.mra", "fallback.png", true),
        ];
        let mut cache = PreviewImageCache::default();
        let failed_key = game_preview_key(&games[0]).expect("failed preview key");
        cache.insert_failed(failed_key);

        let candidate =
            first_available_preview_candidate(ArcadeGameView::contiguous(&games), 0, 4, &mut cache)
                .expect("fallback candidate");

        assert_eq!(candidate.index, 1);
        assert_eq!(candidate.game.title.as_ref(), "Fallback");
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

        let idx = next_ready_result_index(&backlog, 11, Some("selected.png"), false, 0, false);

        assert_eq!(idx, Some(1));
    }

    #[test]
    fn ready_result_selector_defers_current_selected_preview_but_allows_prefetch() {
        let backlog = VecDeque::from(vec![
            preview_result(11, "selected.png", PreviewPriority::Selected),
            preview_result(
                12,
                "prefetch-a.png",
                PreviewPriority::Prefetch { distance: 1 },
            ),
        ]);

        let idx = next_ready_result_index(&backlog, 11, Some("selected.png"), false, 0, true);

        assert_eq!(idx, Some(1));
    }

    #[test]
    fn ready_result_selector_applies_deferred_selected_preview_after_idle() {
        let backlog = VecDeque::from(vec![
            preview_result(11, "selected.png", PreviewPriority::Selected),
            preview_result(
                12,
                "prefetch-a.png",
                PreviewPriority::Prefetch { distance: 1 },
            ),
        ]);

        let idx = next_ready_result_index(&backlog, 11, Some("selected.png"), false, 0, false);

        assert_eq!(idx, Some(0));
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
            next_ready_result_index(&backlog, 99, Some("selected.png"), false, 0, false),
            Some(0)
        );
        assert_eq!(
            next_ready_result_index(&backlog, 99, Some("selected.png"), false, 1, false),
            None
        );
    }

    #[test]
    fn direction_aware_prefetch_orders_ahead_before_small_tail() {
        assert_eq!(
            direction_aware_prefetch_indices(30, 10, 4, 1, DIRECTIONAL_PREFETCH_TAIL_RADIUS),
            vec![11, 12, 13, 14, 9, 8]
        );
        assert_eq!(
            direction_aware_prefetch_indices(30, 10, 4, -1, DIRECTIONAL_PREFETCH_TAIL_RADIUS),
            vec![9, 8, 7, 6, 11, 12]
        );
        assert_eq!(
            direction_aware_prefetch_indices(30, 10, 4, 0, DIRECTIONAL_PREFETCH_TAIL_RADIUS),
            vec![9, 11, 8, 12, 7, 13, 6, 14]
        );
    }

    #[test]
    fn turbo_prefetch_orders_sixty_four_ahead_before_backtail() {
        let forward = direction_aware_prefetch_indices(
            100,
            10,
            MAX_TURBO_PREVIEW_LOOKAHEAD,
            1,
            TURBO_PREVIEW_BACKTAIL,
        );
        assert_eq!(&forward[..64], &(11..=74).collect::<Vec<_>>()[..]);
        assert_eq!(&forward[64..], &[9, 8, 7, 6]);

        let reverse = direction_aware_prefetch_indices(
            100,
            74,
            MAX_TURBO_PREVIEW_LOOKAHEAD,
            -1,
            TURBO_PREVIEW_BACKTAIL,
        );
        assert_eq!(&reverse[..64], &(10..=73).rev().collect::<Vec<_>>()[..]);
        assert_eq!(&reverse[64..], &[75, 76, 77, 78]);
    }

    #[test]
    fn turbo_prefetch_clamps_at_edges() {
        assert_eq!(
            direction_aware_prefetch_indices(
                5,
                0,
                MAX_TURBO_PREVIEW_LOOKAHEAD,
                1,
                TURBO_PREVIEW_BACKTAIL
            ),
            vec![1, 2, 3, 4]
        );
        assert_eq!(
            direction_aware_prefetch_indices(
                5,
                4,
                MAX_TURBO_PREVIEW_LOOKAHEAD,
                -1,
                TURBO_PREVIEW_BACKTAIL
            ),
            vec![3, 2, 1, 0]
        );
    }

    #[test]
    fn normal_prefetch_throttle_clears_pending_keys_and_resumes_after_settle() {
        let mut preview = PreviewState::new();
        preview.pending_prefetch_keys.insert("stale".to_string());

        assert!(prefetch_should_throttle(&mut preview, true, false));
        assert!(preview.pending_prefetch_keys.is_empty());
        assert!(preview.prefetch_throttle_until.is_some());
        assert!(prefetch_should_throttle(&mut preview, false, false));

        preview.prefetch_throttle_until = Some(Instant::now() - Duration::from_millis(1));
        assert!(!prefetch_should_throttle(&mut preview, false, false));
        assert!(preview.prefetch_throttle_until.is_none());
    }

    #[test]
    fn turbo_prefetch_bypasses_throttle_and_preserves_pending_keys() {
        let mut preview = PreviewState::new();
        preview.pending_prefetch_keys.insert("runway".to_string());
        preview.prefetch_throttle_until = Some(Instant::now() + Duration::from_secs(1));

        assert!(!prefetch_should_throttle(&mut preview, true, true));
        assert!(preview.pending_prefetch_keys.contains("runway"));
        assert!(preview.prefetch_throttle_until.is_none());
    }

    #[test]
    fn prefetch_guard_allows_scroll_by_default_runway() {
        assert!(preview_prefetch_allowed(false));
        assert!(preview_prefetch_allowed(true));
    }

    #[test]
    fn normal_prefetch_uses_selection_change_throttle() {
        let games = vec![
            preview_game("Previous", "previous.mra", "previous.png", true),
            preview_game("Selected", "selected.mra", "selected.png", true),
            preview_game("Next", "next.mra", "next.png", true),
        ];
        let mut preview = PreviewState::new();
        preview.last_prefetch_selected = Some(0);
        preview.pending_prefetch_keys.insert("stale".to_string());

        request_preview_prefetches(ArcadeGameView::contiguous(&games), 1, &mut preview, false);

        assert!(preview.pending_prefetch_keys.is_empty());
        assert!(preview.prefetch_throttle_until.is_some());
    }

    #[test]
    fn turbo_prefetch_bypasses_selection_change_throttle_by_default() {
        let games = vec![
            preview_game("Previous", "previous.mra", "previous.png", true),
            preview_game("Selected", "selected.mra", "selected.png", true),
            preview_game("Next", "next.mra", "next.png", true),
        ];
        let mut preview = PreviewState::new();
        preview.last_prefetch_selected = Some(0);
        preview.pending_prefetch_keys.insert("stale".to_string());

        request_preview_prefetches(ArcadeGameView::contiguous(&games), 1, &mut preview, true);

        assert!(preview.prefetch_throttle_until.is_none());
        assert!(
            preview
                .pending_prefetch_keys
                .contains(&game_preview_key(&games[2]).unwrap())
        );
    }

    #[test]
    fn selected_row_without_preview_fades_visible_raw_preview_to_empty() {
        init_test_slint_platform();
        let app = slint_ui::launcher::Launcher::new().expect("launcher component");
        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
        let games = vec![
            preview_game("With Preview", "with-preview.mra", "with-preview.png", true),
            preview_game("No Preview", "no-preview.mra", "", false),
            preview_game("Neighbor Preview", "neighbor.mra", "neighbor.png", true),
        ];
        let mut preview = PreviewState::new();
        preview.cache.insert(
            game_preview_key(&games[0]).expect("visible preview key"),
            preview_image(0xf800),
            &[],
            None,
        );
        preview.selected_mra_path = Some(games[0].mra_path.to_string());
        preview.selected_preview_key = game_preview_key(&games[0]);
        preview.has_visible_preview = true;
        preview.visible_preview_key = preview
            .selected_preview_key
            .clone()
            .expect("selected preview key");

        let changed = request_arcade_preview_window(
            &bridge,
            ArcadeGameView::contiguous(&games),
            1,
            &mut preview,
            false,
            false,
            false,
        );

        assert!(changed);
        assert_eq!(preview.selected_mra_path.as_deref(), Some("no-preview.mra"));
        assert_eq!(preview.selected_preview_key, None);
        assert!(!preview.has_visible_preview);
        assert!(preview.visible_preview_key.is_empty());
        assert!(preview.raw_dirty());
        assert_eq!(bridge.get_arcade_preview_status(), PreviewStatus::Empty);
        let frame = preview
            .raw_transition_frame()
            .expect("empty raw transition frame");
        assert!(frame.previous.is_some());
        assert!(matches!(frame.current.pixels, PreviewRawPixels::Empty));
    }

    #[test]
    fn empty_demand_is_idempotent_while_detached() {
        init_test_slint_platform();
        let app = slint_ui::launcher::Launcher::new().expect("launcher component");
        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
        let mut preview = PreviewState::new();

        let generation = preview.presentation_generation;
        let transition_id = preview.raw_transition_id;
        preview.clear(&bridge);
        preview.clear(&bridge);

        assert!(preview.terminal_empty());
        assert_eq!(
            preview.presentation_state(),
            PreviewPresentationState::Detached
        );
        assert_eq!(preview.presentation_generation, generation);
        assert_eq!(preview.raw_transition_id, transition_id);
        assert_eq!(preview.frame_intent(), PreviewFrameIntent::None);
    }

    #[test]
    fn loading_presentation_retains_only_an_existing_image() {
        let mut preview = PreviewState::new();
        assert_eq!(
            preview.presentation_state(),
            PreviewPresentationState::Detached
        );

        preview.current_generation = 1;
        preview.demand_loading();
        assert_eq!(
            preview.presentation_state(),
            PreviewPresentationState::Loading {
                generation: 1,
                retained_image: false,
            }
        );
        assert!(!preview.presentation_state().owns_direct_layer());

        preview.presentation_state = PreviewPresentationState::Visible { generation: 1 };
        preview.demand_loading();
        assert_eq!(
            preview.presentation_state(),
            PreviewPresentationState::Loading {
                generation: 2,
                retained_image: true,
            }
        );
        assert!(preview.presentation_state().owns_direct_layer());
    }

    #[test]
    fn final_image_confirmation_completes_animation() {
        let mut preview = PreviewState::new();
        preview.set_route(PreviewRoute::Eligible);
        preview.cache.insert(
            "1941.png".into(),
            preview_image(0xf800),
            &["1941.png".into()],
            None,
        );
        preview.has_visible_preview = true;
        preview.begin_raw_transition_to("1941.png", PreviewTransitionPace::Normal);
        preview.visible_preview_key = "1941.png".into();

        assert!(matches!(
            preview.frame_intent(),
            PreviewFrameIntent::Present { generation: 1 }
        ));
        let commit = preview
            .presentation_commit(true, false)
            .expect("final image commit");
        preview.confirm_presentation(commit);

        assert_eq!(
            preview.presentation_state(),
            PreviewPresentationState::Visible { generation: 1 }
        );
        assert_eq!(preview.frame_intent(), PreviewFrameIntent::None);
    }

    #[test]
    fn stale_presentation_generation_cannot_complete_new_demand() {
        let mut preview = PreviewState::new();
        preview.has_visible_preview = true;
        preview.begin_raw_transition_to("first.png", PreviewTransitionPace::Normal);
        preview.visible_preview_key = "first.png".into();
        let stale = preview
            .presentation_commit(true, false)
            .expect("first image commit");

        preview.select_empty_preview(PreviewTransitionPace::Normal);
        preview.confirm_presentation(stale);

        assert!(matches!(
            preview.presentation_state(),
            PreviewPresentationState::Animating {
                generation: 2,
                target: PreviewPresentationTarget::Empty,
            }
        ));
        assert!(preview.previous_image.is_some() || preview.empty_base_commit_pending());
    }

    #[test]
    fn rapid_demand_supersession_advances_generation() {
        let mut preview = PreviewState::new();
        preview.demand_loading();
        preview.demand_empty(PreviewTransitionPace::Normal);
        preview.demand_loading();

        assert_eq!(preview.presentation_generation, 3);
        assert_eq!(
            preview.presentation_state(),
            PreviewPresentationState::Loading {
                generation: 3,
                retained_image: false,
            }
        );
    }

    #[test]
    fn unavailable_route_retires_without_requesting_preview_frames() {
        let mut preview = PreviewState::new();
        preview.set_route(PreviewRoute::Eligible);
        preview.has_visible_preview = true;
        preview.begin_raw_transition_to("1941.png", PreviewTransitionPace::Normal);
        preview.visible_preview_key = "1941.png".into();
        let commit = preview
            .presentation_commit(true, false)
            .expect("visible image commit");
        preview.confirm_presentation(commit);

        preview.set_route(PreviewRoute::Unavailable);

        assert!(preview.retirement_generation().is_some());
        assert_eq!(preview.frame_intent(), PreviewFrameIntent::None);
        assert!(!preview.direct_layer_desired());
    }

    #[test]
    fn route_reversal_reacquires_retained_image_in_the_same_frame() {
        let mut preview = PreviewState::new();
        preview.set_route(PreviewRoute::Eligible);
        preview.has_visible_preview = true;
        preview.demand = PreviewDemand::Image;
        preview.presentation_state = PreviewPresentationState::Visible { generation: 1 };
        preview.set_route(PreviewRoute::Unavailable);
        let retirement_generation = preview.retirement_generation().expect("retirement pending");

        preview.set_route(PreviewRoute::Eligible);

        assert!(preview.presentation_generation() > retirement_generation);
        assert!(preview.direct_layer_desired());
        assert!(matches!(
            preview.frame_intent(),
            PreviewFrameIntent::Present { .. }
        ));
    }

    #[test]
    fn occluded_route_preserves_navigation_snapshot_state_without_waking() {
        let mut preview = PreviewState::new();
        preview.set_route(PreviewRoute::Eligible);
        preview.has_visible_preview = true;
        preview.demand = PreviewDemand::Image;
        preview.presentation_state = PreviewPresentationState::Visible { generation: 1 };

        preview.set_route(PreviewRoute::Occluded);

        assert_eq!(
            preview.presentation_state(),
            PreviewPresentationState::Visible { generation: 1 }
        );
        assert_eq!(preview.frame_intent(), PreviewFrameIntent::None);
    }

    #[test]
    fn empty_missing_and_suppressed_routes_never_create_frame_intent() {
        let mut preview = PreviewState::new();

        // HDMI with no selected screenshot remains physically detached.
        preview.set_route(PreviewRoute::Eligible);
        assert_eq!(preview.frame_intent(), PreviewFrameIntent::None);

        // A missing screenshot can load without authorizing a presentation frame.
        preview.demand_loading();
        assert_eq!(preview.frame_intent(), PreviewFrameIntent::None);

        // CRT and low-memory suppression share the unavailable route. Repeated
        // catalog generations may supersede loading, but cannot wake rendering.
        preview.set_route(PreviewRoute::Unavailable);
        preview.demand_loading();
        preview.demand_loading();
        assert_eq!(preview.frame_intent(), PreviewFrameIntent::None);

        // Navigation/overlay occlusion also retains demand without producing work.
        preview.set_route(PreviewRoute::Occluded);
        assert_eq!(preview.frame_intent(), PreviewFrameIntent::None);
    }

    #[test]
    fn preview_miss_classification_requires_exact_candidate() {
        assert!(!preview_state_is_miss("exact", true));
        assert!(preview_state_is_miss("blank", true));
        assert!(preview_state_is_miss("stale", true));
        assert!(preview_state_is_miss("pending", true));
        assert!(preview_state_is_miss("failed", true));
        assert!(!preview_state_is_miss("blank", false));
    }

    #[test]
    fn preview_cache_hits_share_image_payload() {
        let mut cache = PreviewImageCache::default();
        let words = Arc::<[u16]>::from([0xffff]);
        let image = Arc::new(PreviewImage {
            pixels: PreviewImagePixels::Rgb565 {
                words: Arc::clone(&words),
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
        let (
            PreviewImagePixels::Rgb565 {
                words: first_words, ..
            },
            PreviewImagePixels::Rgb565 {
                words: second_words,
                ..
            },
        ) = (&first_hit.pixels, &second_hit.pixels);
        assert!(Arc::ptr_eq(first_words, second_words));
        assert!(Arc::ptr_eq(&words, first_words));
    }

    #[test]
    fn turbo_preview_cache_retention_defers_window_pruning() {
        let mut cache = PreviewImageCache::default();
        cache.insert("old.png".into(), preview_image(0xf800), &[], None);
        let normal_window = vec!["current.png".to_string()];
        cache.insert(
            "current.png".into(),
            preview_image(0x07e0),
            &normal_window,
            Some("current.png"),
        );
        assert!(!cache.contains("old.png"));

        let mut cache = PreviewImageCache::default();
        cache.insert("old.png".into(), preview_image(0xf800), &[], None);
        let turbo_window = (0..=DEFAULT_PREVIEW_CACHE_CAP)
            .map(|idx| format!("runway-{idx}.png"))
            .collect::<Vec<_>>();
        cache.insert(
            "current.png".into(),
            preview_image(0x07e0),
            &turbo_window,
            Some("current.png"),
        );

        assert!(cache.contains("old.png"));
    }

    #[test]
    fn raw_frame_status_treats_empty_as_empty_without_dimensions() {
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Empty,
            source_w: 0,
            source_h: 0,
            display_w: 0,
            display_h: 0,
        };

        assert_eq!(frame.status(), PreviewRawFrameStatus::Empty);
    }

    #[test]
    fn raw_frame_status_rejects_zero_sized_rgb565_frame() {
        let pixels = [Rgb565Pixel(0xffff)];
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &pixels,
                stride_pixels: 1,
            },
            source_w: 0,
            source_h: 1,
            display_w: 1,
            display_h: 1,
        };

        assert_eq!(frame.status(), PreviewRawFrameStatus::Invalid);
    }

    #[test]
    fn raw_frame_status_rejects_short_rgb565_payload() {
        let pixels = [Rgb565Pixel(0xffff); 3];
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &pixels,
                stride_pixels: 2,
            },
            source_w: 2,
            source_h: 2,
            display_w: 2,
            display_h: 2,
        };

        assert_eq!(frame.status(), PreviewRawFrameStatus::Invalid);
    }

    #[test]
    fn raw_frame_status_rejects_rgb565_stride_smaller_than_width() {
        let pixels = [Rgb565Pixel(0xffff); 4];
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &pixels,
                stride_pixels: 1,
            },
            source_w: 2,
            source_h: 2,
            display_w: 2,
            display_h: 2,
        };

        assert_eq!(frame.status(), PreviewRawFrameStatus::Invalid);
    }

    #[test]
    fn raw_frame_status_accepts_padded_rgb565_stride() {
        let pixels = [Rgb565Pixel(0xffff); 8];
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &pixels,
                stride_pixels: 4,
            },
            source_w: 2,
            source_h: 2,
            display_w: 2,
            display_h: 2,
        };

        assert_eq!(frame.status(), PreviewRawFrameStatus::Ready);
    }

    #[test]
    fn same_selected_preview_requests_cached_apply_after_deferral() {
        let mut preview = PreviewState::new();
        preview.selected_mra_path = Some("/_Arcade/selected.mra".into());
        preview.selected_preview_key = Some("selected.png".into());
        preview.visible_preview_key = "previous.png".into();
        preview.has_visible_preview = true;
        preview.cache.insert(
            "selected.png".into(),
            preview_image(0x07e0),
            &["selected.png".into()],
            Some("previous.png"),
        );

        assert!(!same_selected_preview_needs_request(&mut preview, true));
        assert!(same_selected_preview_needs_request(&mut preview, false));
    }

    #[test]
    fn same_selected_preview_skips_request_when_visible_is_current() {
        let mut preview = PreviewState::new();
        preview.selected_mra_path = Some("/_Arcade/selected.mra".into());
        preview.selected_preview_key = Some("selected.png".into());
        preview.visible_preview_key = "selected.png".into();
        preview.has_visible_preview = true;
        preview.cache.insert(
            "selected.png".into(),
            preview_image(0x07e0),
            &["selected.png".into()],
            Some("selected.png"),
        );

        assert!(!same_selected_preview_needs_request(&mut preview, false));
    }

    #[test]
    fn failed_preview_cache_expires_to_allow_same_process_asset_refresh() {
        let mut cache = PreviewImageCache::default();
        cache.insert_failed("missing.png".into());

        assert!(cache.contains_failed("missing.png"));

        cache.failed_paths[0].1 =
            Instant::now() - PreviewImageCache::FAILED_TTL - Duration::from_millis(1);

        assert!(!cache.contains_failed("missing.png"));
    }

    #[test]
    fn failed_preview_cache_can_be_cleared_after_media_publish() {
        let mut preview = PreviewState::new();
        preview
            .cache
            .insert_failed("pack.mmlz4b:missing.raw565".into());

        assert!(preview.cache.contains_failed("pack.mmlz4b:missing.raw565"));

        preview.clear_failed_preview_cache();

        assert!(!preview.cache.contains_failed("pack.mmlz4b:missing.raw565"));
    }

    #[test]
    fn empty_preview_transition_keeps_previous_frame_for_fade_out() {
        let mut preview = PreviewState::new();
        let visible_image = Arc::new(PreviewImage {
            pixels: PreviewImagePixels::Rgb565 {
                words: Arc::from([0xf800]),
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
        preview.selected_preview_key = Some("visible.png".into());
        preview.has_visible_preview = true;
        preview.visible_preview_key = "visible.png".into();
        let previous_transition_id = preview.raw_transition_id;

        preview.select_empty_preview(PreviewTransitionPace::Normal);

        assert_eq!(preview.selected_preview_key, None);
        assert!(preview.terminal_empty());
        assert!(!preview.has_visible_preview);
        assert!(preview.visible_preview_key.is_empty());
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

    #[test]
    fn empty_presentation_retires_only_after_black_base_and_final_frame_confirm() {
        let mut preview = PreviewState::new();
        preview.set_route(PreviewRoute::Eligible);
        preview.cache.insert(
            "1941.png".into(),
            preview_image(0xf800),
            &["1941.png".into()],
            Some("1941.png"),
        );
        preview.selected_preview_key = Some("1941.png".into());
        preview.has_visible_preview = true;
        preview.visible_preview_key = "1941.png".into();

        preview.select_empty_preview(PreviewTransitionPace::Normal);

        assert_eq!(
            preview.presentation_state(),
            PreviewPresentationState::Animating {
                generation: 1,
                target: PreviewPresentationTarget::Empty,
            }
        );
        assert!(preview.presentation_state().owns_direct_layer());

        let base_commit = preview
            .presentation_commit(false, true)
            .expect("black base commit");
        preview.confirm_presentation(base_commit);
        assert!(!preview.empty_base_commit_pending());
        assert!(preview.presentation_requires_present());

        let final_commit = preview
            .presentation_commit(true, false)
            .expect("final black frame commit");
        assert!(preview.presentation_requires_present());
        preview.confirm_presentation(final_commit);

        assert_eq!(
            preview.presentation_state(),
            PreviewPresentationState::RetirementPending { generation: 1 }
        );
        assert!(preview.presentation_state().owns_direct_layer());
        preview.confirm_retirement(1);
        assert_eq!(
            preview.presentation_state(),
            PreviewPresentationState::Detached
        );
        assert!(!preview.presentation_state().owns_direct_layer());
    }

    #[test]
    fn failed_final_black_present_keeps_direct_layer_owned() {
        let mut preview = PreviewState::new();
        preview.set_route(PreviewRoute::Eligible);
        preview.cache.insert(
            "1941.png".into(),
            preview_image(0xf800),
            &["1941.png".into()],
            Some("1941.png"),
        );
        preview.has_visible_preview = true;
        preview.visible_preview_key = "1941.png".into();
        preview.select_empty_preview(PreviewTransitionPace::Normal);

        let _unconfirmed = preview
            .presentation_commit(true, true)
            .expect("attempted final present");

        assert!(preview.presentation_requires_present());
        assert!(preview.presentation_state().owns_direct_layer());
        assert!(preview.empty_base_commit_pending());
    }

    #[test]
    fn turbo_empty_retarget_keeps_63_over_130_duration_and_can_retarget_to_image() {
        let mut preview = PreviewState::new();
        preview.cache.insert(
            "1941.png".into(),
            preview_image(0xf800),
            &["1941.png".into(), "next.png".into()],
            Some("1941.png"),
        );
        preview.cache.insert(
            "next.png".into(),
            preview_image(0x07e0),
            &["1941.png".into(), "next.png".into()],
            Some("1941.png"),
        );
        preview.selection_transition = PreviewSelectionTransition::CrossFade;
        preview.has_visible_preview = true;
        preview.visible_preview_key = "1941.png".into();

        preview.select_empty_preview(PreviewTransitionPace::Turbo);
        let empty_frame = preview.raw_transition_frame().expect("turbo empty frame");
        assert_eq!(
            (
                empty_frame.duration_numerator,
                empty_frame.duration_denominator,
            ),
            (
                TURBO_PREVIEW_TRANSITION_DURATION_NUMERATOR,
                TURBO_PREVIEW_TRANSITION_DURATION_DENOMINATOR,
            )
        );

        preview.has_visible_preview = true;
        preview.begin_raw_transition_to("next.png", PreviewTransitionPace::Turbo);
        preview.visible_preview_key = "next.png".into();
        preview.raw_dirty = true;

        assert_eq!(
            preview.presentation_state(),
            PreviewPresentationState::Animating {
                generation: 2,
                target: PreviewPresentationTarget::Image,
            }
        );
        let image_frame = preview.raw_transition_frame().expect("turbo image frame");
        assert_eq!(
            (
                image_frame.duration_numerator,
                image_frame.duration_denominator,
            ),
            (
                TURBO_PREVIEW_TRANSITION_DURATION_NUMERATOR,
                TURBO_PREVIEW_TRANSITION_DURATION_DENOMINATOR,
            )
        );
    }

    #[test]
    fn consecutive_empty_previews_keep_existing_fade_out() {
        let mut preview = PreviewState::new();
        let visible_image = Arc::new(PreviewImage {
            pixels: PreviewImagePixels::Rgb565 {
                words: Arc::from([0xf800]),
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
        preview.selected_preview_key = Some("visible.png".into());
        preview.has_visible_preview = true;
        preview.visible_preview_key = "visible.png".into();

        preview.select_empty_preview(PreviewTransitionPace::Normal);
        let first_transition_id = preview.raw_transition_id;
        let first_previous = Arc::clone(
            preview
                .previous_image
                .as_ref()
                .expect("first empty preview keeps previous image"),
        );

        preview.select_empty_preview(PreviewTransitionPace::Normal);

        assert_eq!(preview.raw_transition_id, first_transition_id);
        assert!(preview.raw_dirty);
        let second_previous = preview
            .previous_image
            .as_ref()
            .expect("second empty preview keeps previous image");
        assert!(Arc::ptr_eq(&first_previous, second_previous));

        let frame = preview
            .raw_transition_frame()
            .expect("empty preview transition frame");
        assert!(frame.previous.is_some());
        assert!(matches!(frame.current.pixels, PreviewRawPixels::Empty));
    }

    #[test]
    fn raw_transition_keeps_previous_image_when_animation_enabled() {
        let mut preview = PreviewState::new();
        preview.cache.insert(
            "previous.png".into(),
            preview_image(0xf800),
            &["previous.png".into(), "selected.png".into()],
            Some("previous.png"),
        );
        preview.cache.insert(
            "selected.png".into(),
            preview_image(0x07e0),
            &["previous.png".into(), "selected.png".into()],
            Some("previous.png"),
        );
        preview.has_visible_preview = true;
        preview.visible_preview_key = "previous.png".into();
        let previous_transition_id = preview.raw_transition_id;

        preview.begin_raw_transition_to("selected.png", PreviewTransitionPace::Normal);

        assert_eq!(
            preview.raw_transition_id,
            previous_transition_id.wrapping_add(1)
        );
        assert!(preview.previous_image.is_some());
    }

    #[test]
    fn first_preview_on_list_entry_is_shown_without_a_fade() {
        let mut preview = PreviewState::new();
        preview.cache.insert(
            "selected.png".into(),
            preview_image(0x07e0),
            &["selected.png".into()],
            None,
        );
        preview.has_visible_preview = true;

        preview.begin_raw_transition_to("selected.png", PreviewTransitionPace::Normal);
        preview.visible_preview_key = "selected.png".into();

        let frame = preview
            .raw_transition_frame()
            .expect("initial preview frame");
        assert!(frame.previous.is_none());
    }

    #[test]
    fn preview_after_an_empty_in_list_selection_fades_in_from_empty() {
        let mut preview = PreviewState::new();
        preview.selection_transition = PreviewSelectionTransition::CrossFade;
        preview.cache.insert(
            "selected.png".into(),
            preview_image(0x07e0),
            &["selected.png".into()],
            None,
        );
        preview.has_visible_preview = true;

        preview.begin_raw_transition_to("selected.png", PreviewTransitionPace::Normal);
        preview.visible_preview_key = "selected.png".into();

        let frame = preview
            .raw_transition_frame()
            .expect("fade-in preview frame");
        assert!(matches!(
            frame.previous.expect("empty fade origin").pixels,
            PreviewRawPixels::Empty
        ));
    }

    #[test]
    fn raw_transition_uses_full_duration_for_normal_pace() {
        let mut preview = PreviewState::new();
        preview.cache.insert(
            "previous.png".into(),
            preview_image(0xf800),
            &["previous.png".into(), "selected.png".into()],
            Some("previous.png"),
        );
        preview.cache.insert(
            "selected.png".into(),
            preview_image(0x07e0),
            &["previous.png".into(), "selected.png".into()],
            Some("previous.png"),
        );
        preview.has_visible_preview = true;
        preview.visible_preview_key = "previous.png".into();

        preview.begin_raw_transition_to("selected.png", preview_transition_pace(false));

        let frame = preview
            .raw_transition_frame()
            .expect("normal transition frame");
        assert_eq!(
            (frame.duration_numerator, frame.duration_denominator),
            (1, 1)
        );
    }

    #[test]
    fn raw_transition_uses_63_over_130_duration_for_turbo_pace() {
        let mut preview = PreviewState::new();
        preview.cache.insert(
            "previous.png".into(),
            preview_image(0xf800),
            &["previous.png".into(), "selected.png".into()],
            Some("previous.png"),
        );
        preview.cache.insert(
            "selected.png".into(),
            preview_image(0x07e0),
            &["previous.png".into(), "selected.png".into()],
            Some("previous.png"),
        );
        preview.has_visible_preview = true;
        preview.visible_preview_key = "previous.png".into();
        let previous_transition_id = preview.raw_transition_id;

        preview.begin_raw_transition_to("selected.png", PreviewTransitionPace::Turbo);

        assert_eq!(
            preview.raw_transition_id,
            previous_transition_id.wrapping_add(1)
        );
        assert!(preview.previous_image.is_some());
        let frame = preview
            .raw_transition_frame()
            .expect("turbo transition frame");
        assert_eq!(
            (frame.duration_numerator, frame.duration_denominator),
            (
                TURBO_PREVIEW_TRANSITION_DURATION_NUMERATOR,
                TURBO_PREVIEW_TRANSITION_DURATION_DENOMINATOR,
            )
        );
    }

    fn preview_result(
        generation: u64,
        preview_asset_key: &str,
        priority: PreviewPriority,
    ) -> PreviewResult {
        PreviewResult {
            generation,
            title: preview_asset_key.to_string(),
            preview_archive_path: String::new(),
            preview_asset_key: preview_asset_key.to_string(),
            image: None,
            requested_at_ms: 0,
            completed_at_ms: 0,
            request_age_us: 0,
            read_us: 0,
            decode_us: 0,
            raw565_parse_us: 0,
            resize_us: 0,
            total_us: 0,
            encoded_bytes: 0,
            decoded_bytes: 0,
            source_width: 0,
            source_height: 0,
            load_source: crate::preview_worker::PreviewLoadSource::ArchiveMem,
            storage_format: crate::preview_worker::PreviewStorageFormat::RawRgb565,
            resize_filter: crate::preview_worker::PreviewResizeFilter::Off,
            priority,
        }
    }

    fn preview_image(word: u16) -> Arc<PreviewImage> {
        Arc::new(PreviewImage {
            pixels: PreviewImagePixels::Rgb565 {
                words: Arc::from([word]),
                stride_pixels: 1,
            },
            source_w: 1,
            source_h: 1,
            display_w: 1,
            display_h: 1,
        })
    }

    fn preview_game(
        title: &str,
        mra_path: &str,
        preview_asset_key: &str,
        has_preview: bool,
    ) -> ArcadeGameEntry {
        ArcadeGameEntry {
            title: title.into(),
            mra_path: mra_path.into(),
            preview_archive_path: if has_preview {
                "/media/fat/mister-magik/assets/test-screenshots.mmlz4b".into()
            } else {
                "".into()
            },
            preview_asset_key: preview_asset_key.into(),
            has_preview,
            system_id: "saturn".into(),
            year: None,
            manufacturer: "".into(),
            category: "".into(),
            players: None,
            control: "".into(),
            is_new: false,
        }
    }

    fn init_test_slint_platform() {
        let window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let fixed_time = Some(Rc::new(Cell::new(Duration::ZERO)));
        let _ = slint::platform::set_platform(Box::new(MisterPlatform::new(window, fixed_time)));
    }
}
