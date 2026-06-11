//! Shared vsync render loop and Slint bench scene dispatch.
#![cfg_attr(mister_ui_scope_launcher, allow(dead_code))]

use crate::fb::{Display, Pixel, VsyncPacer};
use crate::fb_format::FramebufferFormat;
use crate::fpga::{Fpga, Mode};
use crate::vt::VtGraphicsGuard;
use mister_magik_fb::vsync_pacer::VsyncPaceSource;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel, SoftwareRenderer, TargetPixel,
};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, Image, ModelRc, PhysicalSize, SharedString, VecModel};
#[cfg(feature = "video")]
use slint::{Rgb8Pixel, SharedPixelBuffer};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mister_magik_ui as slint_ui;

use crate::arcade_catalog::{
    self, ArcadeCatalog, ArcadeGameEntry, ARCADE_LIST_VISIBLE_H, ARCADE_ROW_HEIGHT,
    HOME_LIST_VISIBLE_W, HOME_TILE_GAP, HOME_TILE_WIDTH,
};
use crate::arcade_list_renderer::{
    ArcadeListRenderer, ArcadeListUpdate, ARCADE_LIST_H, ARCADE_LIST_W, ARCADE_LIST_X,
    ARCADE_LIST_Y,
};
use crate::bitmap_text::ConsoleFont;
use crate::boot_analytics;
use crate::controller_db::ControllerDb;
use crate::cpu_profile;
use crate::display_config::DisplayConfig;
use crate::frame_profile::{FrameProfiler, FrameRect, FrameSample};
use crate::input::{PadInfo, PadPool};
use crate::launcher::{self, LauncherAction, LauncherNav, Screen};
use crate::library_db;
use crate::preview_state::{
    apply_ready_preview, preview_raw_blitter_enabled, preview_visual_pct,
    request_arcade_preview_window, schedule_arcade_preview_window, PreviewRawFrame,
    PreviewRawPixels, PreviewRawTransitionFrame, PreviewState, ARCADE_PREVIEW_BOX_H,
    ARCADE_PREVIEW_BOX_W, ARCADE_PREVIEW_BOX_X, ARCADE_PREVIEW_BOX_Y,
};
use crate::preview_worker::DEFAULT_PREVIEW_CACHE_CAP;
use crate::runtime_status::{self, LauncherStatus};
use crate::screenshot_transitions::{
    PreviewTransitionDemo, PreviewTransitionEffect, PreviewTransitionTrace,
};
use crate::setup_nav::{SetupAction, SetupNav, SetupPhase};
use crate::ui_display::{UiDisplay, SLINT_UI_SCALE, UI_FB_H, UI_FB_W, UI_HDMI_H, UI_HDMI_W};
use mister_magik_fb::effects::{EffectKind, EffectSize, EFFECT_SIZES};
use slint::platform::software_renderer::PhysicalRegion;
use slint_ui::launcher::PreviewStatus;
use std::cell::Cell;
#[cfg(mister_bench_scenes)]
use std::fs::File;
#[cfg(mister_bench_scenes)]
use std::io::Write;
use std::path::PathBuf;
use std::sync::{mpsc, Mutex, OnceLock};

const AUTO_CONTROLLER_SETUP_ENABLED: bool = false;
const DEFAULT_DIRTY_RECT_BROAD_PCT: usize = 85;

fn screen_label(screen: Screen) -> &'static str {
    match screen {
        Screen::Home => "home",
        Screen::Controller => "controller",
        Screen::Arcade => "arcade",
        Screen::Settings => "settings",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LauncherRunMode {
    Launcher,
    Arcade,
}

impl LauncherRunMode {
    fn label(self) -> &'static str {
        match self {
            Self::Launcher => "launcher",
            Self::Arcade => "arcade",
        }
    }

    fn initial_screen(self) -> Screen {
        match self {
            Self::Launcher => Screen::Home,
            Self::Arcade => Screen::Arcade,
        }
    }

    fn enforce(self, nav: &mut LauncherNav) {
        if self == Self::Arcade {
            nav.screen = Screen::Arcade;
        }
    }
}

pub const UI_SCENES: &[&str] = &[
    "launcher",
    "arcade",
    "blend_velocity",
    #[cfg(not(mister_ui_scope_launcher))]
    "demo",
    "controller_test",
    #[cfg(mister_bench_scenes)]
    "full_motion",
    #[cfg(mister_bench_scenes)]
    "static_ui",
    #[cfg(mister_bench_scenes)]
    "local_motion",
    #[cfg(mister_bench_scenes)]
    "console_scroll",
    #[cfg(all(feature = "video", mister_bench_scenes))]
    "video_playback",
];

pub(crate) struct MisterPlatform {
    pub(crate) window: Rc<MinimalSoftwareWindow>,
    pub(crate) start: Instant,
    pub(crate) fixed_time: Option<Rc<Cell<Duration>>>,
}

#[derive(Clone)]
pub(crate) struct AnimationClock {
    fixed_time: Option<Rc<Cell<Duration>>>,
    fixed_step: Duration,
}

impl AnimationClock {
    pub(crate) fn from_env() -> Self {
        match std::env::var("MISTER_ANIMATION_CLOCK")
            .ok()
            .map(|s| s.to_ascii_lowercase().replace('_', "-"))
            .as_deref()
        {
            None | Some("") | Some("fixed60") | Some("fixed-60") | Some("frame")
            | Some("frame-clock") => Self {
                fixed_time: Some(Rc::new(Cell::new(Duration::ZERO))),
                fixed_step: Duration::from_nanos(16_666_667),
            },
            Some("wall") | Some("wall-clock") => Self {
                fixed_time: None,
                fixed_step: Duration::from_nanos(16_666_667),
            },
            other => {
                eprintln!("ui: unknown MISTER_ANIMATION_CLOCK={other:?}; use wall|fixed60");
                Self {
                    fixed_time: None,
                    fixed_step: Duration::from_nanos(16_666_667),
                }
            }
        }
    }

    pub(crate) fn platform_time(&self) -> Option<Rc<Cell<Duration>>> {
        self.fixed_time.clone()
    }

    fn label(&self) -> &'static str {
        if self.fixed_time.is_some() {
            "fixed60"
        } else {
            "wall"
        }
    }

    fn advance(&self) {
        if let Some(t) = &self.fixed_time {
            t.set(t.get() + self.fixed_step);
        }
    }
}

pub(crate) fn update_slint_animations(animation_clock: &AnimationClock) {
    animation_clock.advance();
    slint::platform::update_timers_and_animations();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameOrder {
    RenderThenVsync,
    VsyncThenRender,
}

impl FrameOrder {
    fn from_env() -> Self {
        match std::env::var("MISTER_FRAME_ORDER")
            .ok()
            .map(|s| s.to_ascii_lowercase().replace('_', "-"))
            .as_deref()
        {
            None | Some("") | Some("render-then-vsync") | Some("render") => Self::RenderThenVsync,
            Some("vsync-then-render") | Some("vsync-first") | Some("vsync") => {
                Self::VsyncThenRender
            }
            other => {
                eprintln!(
                    "ui: unknown MISTER_FRAME_ORDER={other:?}; use render-then-vsync|vsync-first"
                );
                Self::RenderThenVsync
            }
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::RenderThenVsync => "render-then-vsync",
            Self::VsyncThenRender => "vsync-first",
        }
    }
}

impl Platform for MisterPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> core::time::Duration {
        self.fixed_time
            .as_ref()
            .map(|t| t.get())
            .unwrap_or_else(|| self.start.elapsed())
    }
}

/// `ui [scene] [secs]` — scene defaults to `launcher`; secs defaults to 0 (infinite).
pub fn parse_ui_args() -> (String, u64) {
    let a2 = std::env::args().nth(2);
    let a3 = std::env::args().nth(3);
    match (a2.as_deref(), a3.as_deref()) {
        (Some(s), Some(t)) if t.parse::<u64>().is_ok() => (normalize_scene(s), t.parse().unwrap()),
        (Some(s), None) if s.parse::<u64>().is_ok() => ("launcher".into(), s.parse().unwrap()),
        (Some(s), Some(t)) => (normalize_scene(s), t.parse::<u64>().unwrap_or(0)),
        (Some(s), None) => (normalize_scene(s), 0),
        _ => ("launcher".into(), 0),
    }
}

fn normalize_scene(s: &str) -> String {
    if UI_SCENES.contains(&s) {
        s.to_string()
    } else {
        eprintln!("unknown scene '{s}' (use: {})", UI_SCENES.join(" | "));
        std::process::exit(2);
    }
}

pub fn print_scenes() {
    println!("Slint UI scenes (runtime framebuffer sized, ui-scale {SLINT_UI_SCALE}):");
    for s in UI_SCENES {
        println!("  {s}");
    }
}

pub fn print_effects() {
    println!("Framebuffer effects:");
    for &kind in EffectKind::all() {
        println!("  {}", kind.name());
    }
    println!("Supported internal sizes:");
    for &(w, h) in EFFECT_SIZES {
        let scale = EffectSize { w, h }.scale_to_1080p().unwrap_or(0);
        if scale > 0 {
            println!("  {w}x{h} ({scale}x to 1920x1080)");
        } else {
            println!("  {w}x{h}");
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DirtyRect {
    pub(crate) x0: usize,
    pub(crate) y0: usize,
    pub(crate) x1: usize,
    pub(crate) y1: usize,
}

impl DirtyRect {
    pub(crate) fn rows(self) -> u32 {
        (self.y1 - self.y0) as u32
    }

    fn width(self) -> usize {
        self.x1 - self.x0
    }

    fn is_full_width(self, render_w: usize) -> bool {
        self.x0 == 0 && self.x1 >= render_w
    }

    fn contains(self, other: DirtyRect) -> bool {
        self.x0 <= other.x0 && self.y0 <= other.y0 && self.x1 >= other.x1 && self.y1 >= other.y1
    }

    pub(crate) fn intersection(self, other: DirtyRect) -> Option<DirtyRect> {
        let x0 = self.x0.max(other.x0);
        let y0 = self.y0.max(other.y0);
        let x1 = self.x1.min(other.x1);
        let y1 = self.y1.min(other.y1);
        if x1 > x0 && y1 > y0 {
            Some(DirtyRect { x0, y0, x1, y1 })
        } else {
            None
        }
    }

    #[cfg_attr(not(feature = "video"), allow(dead_code))]
    fn union(self, other: DirtyRect) -> DirtyRect {
        DirtyRect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }
}

fn dirty_rect_broad_pct() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MISTER_DIRTY_RECT_BROAD_PCT")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .map(|value| value.clamp(1, 100))
            .unwrap_or(DEFAULT_DIRTY_RECT_BROAD_PCT)
    })
}

fn dirty_rect_is_broad(rect: DirtyRect, render_w: usize) -> bool {
    rect.width() * 100 >= render_w * dirty_rect_broad_pct()
}

fn launcher_dirty_opt_enabled() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        !matches!(
            std::env::var("MISTER_LAUNCHER_DIRTY_OPT").as_deref(),
            Ok("0") | Ok("off") | Ok("false") | Ok("no")
        )
    })
}

fn preview_stress_enabled() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        matches!(
            std::env::var("MISTER_PREVIEW_STRESS").as_deref(),
            Ok("1") | Ok("on") | Ok("true") | Ok("yes")
        )
    })
}

fn preview_run_label() -> String {
    std::env::var("MISTER_PREVIEW_RUN_LABEL").unwrap_or_default()
}

fn catalog_refresh_requested() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        matches!(
            std::env::var("MISTER_CATALOG_REFRESH").as_deref(),
            Ok("1") | Ok("on") | Ok("true") | Ok("yes")
        )
    })
}

fn forced_arcade_selected_index() -> Option<usize> {
    std::env::var("MISTER_ARCADE_SELECTED_INDEX")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
}

fn apply_forced_arcade_selected(nav: &mut LauncherNav, catalog: &ArcadeCatalog) {
    let Some(index) = forced_arcade_selected_index() else {
        return;
    };
    let count = active_system_game_slice(catalog, nav).len();
    if count == 0 {
        return;
    }
    nav.screen = Screen::Arcade;
    nav.arcade.selected = index.min(count - 1);
    nav.arcade.snap_to_selected();
    keep_bench_arcade_visible(&mut nav.arcade.scroll_y, nav.arcade.selected, count);
}

fn format_dirty_rect(rect: Option<DirtyRect>) -> String {
    match rect {
        Some(rect) => format!(
            "x0={} y0={} x1={} y1={} rows={}",
            rect.x0,
            rect.y0,
            rect.x1,
            rect.y1,
            rect.rows()
        ),
        None => "none".to_string(),
    }
}
pub(crate) struct FbModeGuard {
    previous: crate::fb::FbInfo,
    active: bool,
}

impl FbModeGuard {
    #[allow(dead_code)]
    pub(crate) fn set_temporary(w: usize, h: usize) -> std::io::Result<Self> {
        Self::set_temporary_format(w, h, FramebufferFormat::Xrgb8888)
    }

    pub(crate) fn set_temporary_format(
        w: usize,
        h: usize,
        format: FramebufferFormat,
    ) -> std::io::Result<Self> {
        let previous = Display::current_info()?;
        Display::write_mister_mode_format(format, w, h, format.stride_bytes(w))?;
        remember_fb_mode_for_exit(previous);
        Ok(Self {
            previous,
            active: true,
        })
    }

    fn restore_now(&mut self) {
        if !self.active {
            return;
        }
        match Display::restore_mister_mode(self.previous) {
            Ok(()) => {
                clear_fb_mode_for_exit();
                self.active = false;
            }
            Err(e) => {
                eprintln!("warning: failed to restore framebuffer mode: {e}");
            }
        }
    }
}

impl Drop for FbModeGuard {
    fn drop(&mut self) {
        self.restore_now();
    }
}

static FB_MODE_RESTORE: Mutex<Option<crate::fb::FbInfo>> = Mutex::new(None);
static FB_MODE_RESTORE_ATEXIT: OnceLock<()> = OnceLock::new();

fn remember_fb_mode_for_exit(previous: crate::fb::FbInfo) {
    FB_MODE_RESTORE_ATEXIT.get_or_init(|| unsafe {
        libc::atexit(restore_fb_mode_at_exit);
    });
    if let Ok(mut slot) = FB_MODE_RESTORE.lock() {
        *slot = Some(previous);
    }
}

fn clear_fb_mode_for_exit() {
    if let Ok(mut slot) = FB_MODE_RESTORE.lock() {
        *slot = None;
    }
}

extern "C" fn restore_fb_mode_at_exit() {
    let previous = FB_MODE_RESTORE.lock().ok().and_then(|mut slot| slot.take());
    if let Some(previous) = previous {
        let _ = Display::restore_mister_mode(previous);
    }
}

fn ui_fpga_scaled_mode() -> Mode {
    Mode {
        hact: UI_HDMI_W,
        hbp: 3,
        vact: UI_HDMI_H,
        vbp: 2,
    }
}

fn dirty_rect(region: &PhysicalRegion, render_w: usize, render_h: usize) -> Option<DirtyRect> {
    let o = region.bounding_box_origin();
    let s = region.bounding_box_size();
    if s.width == 0 || s.height == 0 {
        return None;
    }
    let x0 = o.x.max(0) as usize;
    let x1 = ((o.x + s.width as i32) as usize).min(render_w);
    let y0 = o.y.max(0) as usize;
    let y1 = ((o.y + s.height as i32) as usize).min(render_h);
    if x1 > x0 && y1 > y0 {
        Some(DirtyRect { x0, y0, x1, y1 })
    } else {
        None
    }
}

fn copy_cached_rows(disp: &mut Display, ui: &UiDisplay, cached: &[Pixel], y0: usize, y1: usize) {
    debug_assert_eq!(ui.fb_scale(), 1);
    disp.copy_rows(cached, y0, y1);
}

fn copy_cached_rect(disp: &mut Display, ui: &UiDisplay, cached: &[Pixel], rect: DirtyRect) {
    if rect.is_full_width(ui.render_w()) || dirty_rect_is_broad(rect, ui.render_w()) {
        copy_cached_rows(disp, ui, cached, rect.y0, rect.y1);
        return;
    }
    debug_assert_eq!(ui.fb_scale(), 1);
    disp.copy_rect(cached, ui.render_w(), rect.x0, rect.y0, rect.x1, rect.y1);
}

fn copy_cached_rows_565(
    disp: &mut Display,
    ui: &UiDisplay,
    cached: &[Rgb565Pixel],
    y0: usize,
    y1: usize,
) {
    debug_assert_eq!(ui.fb_scale(), 1);
    disp.copy_rows_565(cached, y0, y1);
}

fn copy_cached_rect_565(
    disp: &mut Display,
    ui: &UiDisplay,
    cached: &[Rgb565Pixel],
    rect: DirtyRect,
) {
    if rect.is_full_width(ui.render_w()) || dirty_rect_is_broad(rect, ui.render_w()) {
        copy_cached_rows_565(disp, ui, cached, rect.y0, rect.y1);
        return;
    }
    debug_assert_eq!(ui.fb_scale(), 1);
    disp.copy_rect_565(cached, ui.render_w(), rect.x0, rect.y0, rect.x1, rect.y1);
}

fn preview_screen_rect(ui: &UiDisplay) -> DirtyRect {
    const CABINET_W: usize = 336;
    const CABINET_H: usize = 520;
    let right_x = ui.render_w() / 2;
    let right_w = ui.render_w().saturating_sub(right_x);
    let cabinet_x = right_x + right_w.saturating_sub(CABINET_W) / 2;
    let cabinet_y = ui.render_h().saturating_sub(CABINET_H) / 2;
    DirtyRect {
        x0: cabinet_x + ARCADE_PREVIEW_BOX_X,
        y0: cabinet_y + ARCADE_PREVIEW_BOX_Y,
        x1: cabinet_x + ARCADE_PREVIEW_BOX_X + ARCADE_PREVIEW_BOX_W as usize,
        y1: cabinet_y + ARCADE_PREVIEW_BOX_Y + ARCADE_PREVIEW_BOX_H as usize,
    }
}

fn rgb565_to_pixel(pixel: Rgb565Pixel) -> Pixel {
    let v = pixel.0;
    let r5 = (v >> 11) & 0x1f;
    let g6 = (v >> 5) & 0x3f;
    let b5 = v & 0x1f;
    let r = ((r5 << 3) | (r5 >> 2)) as u32;
    let g = ((g6 << 2) | (g6 >> 4)) as u32;
    let b = ((b5 << 3) | (b5 >> 2)) as u32;
    Pixel((r << 16) | (g << 8) | b)
}

fn pixel_to_rgb(pixel: Pixel) -> (u8, u8, u8) {
    (
        ((pixel.0 >> 16) & 0xff) as u8,
        ((pixel.0 >> 8) & 0xff) as u8,
        (pixel.0 & 0xff) as u8,
    )
}

fn rgb565_to_rgb(pixel: Rgb565Pixel) -> (u8, u8, u8) {
    pixel_to_rgb(rgb565_to_pixel(pixel))
}

fn raw_preview_scaled_rect(ui: &UiDisplay, frame: &PreviewRawFrame<'_>) -> Option<DirtyRect> {
    if frame.source_w == 0 || frame.source_h == 0 || frame.display_w == 0 || frame.display_h == 0 {
        return None;
    }
    match frame.pixels {
        PreviewRawPixels::Rgb8(rgb)
            if rgb.len() < frame.source_w as usize * frame.source_h as usize * 3 =>
        {
            return None;
        }
        PreviewRawPixels::Rgb565 {
            pixels,
            stride_pixels,
        } if stride_pixels < frame.source_w as usize
            || pixels.len() < stride_pixels * frame.source_h as usize =>
        {
            return None;
        }
        _ => {}
    }

    let screen = preview_screen_rect(ui);
    let image_x =
        screen.x0 as isize + (ARCADE_PREVIEW_BOX_W as isize - frame.display_w as isize) / 2;
    let image_y =
        screen.y0 as isize + (ARCADE_PREVIEW_BOX_H as isize - frame.display_h as isize) / 2;
    let x0 = screen.x0.max(image_x.max(0) as usize);
    let y0 = screen.y0.max(image_y.max(0) as usize);
    let x1 = screen
        .x1
        .min((image_x + frame.display_w as isize).max(0) as usize)
        .min(ui.render_w());
    let y1 = screen
        .y1
        .min((image_y + frame.display_h as isize).max(0) as usize)
        .min(ui.render_h());

    (x1 > x0 && y1 > y0).then_some(DirtyRect { x0, y0, x1, y1 })
}

fn sample_preview_rgb(
    frame: &PreviewRawFrame<'_>,
    screen: DirtyRect,
    x: usize,
    y: usize,
    offset_x: isize,
    scale_num: u32,
    scale_den: u32,
) -> Option<(u8, u8, u8)> {
    if frame.source_w == 0
        || frame.source_h == 0
        || frame.display_w == 0
        || frame.display_h == 0
        || scale_den == 0
    {
        return None;
    }
    if scale_num == scale_den
        && frame.display_w == frame.source_w
        && frame.display_h == frame.source_h
    {
        let image_x = screen.x0 as isize
            + (ARCADE_PREVIEW_BOX_W as isize - frame.source_w as isize) / 2
            + offset_x;
        let image_y =
            screen.y0 as isize + (ARCADE_PREVIEW_BOX_H as isize - frame.source_h as isize) / 2;
        let src_x = x as isize - image_x;
        let src_y = y as isize - image_y;
        if src_x < 0
            || src_y < 0
            || src_x >= frame.source_w as isize
            || src_y >= frame.source_h as isize
        {
            return None;
        }
        let src_x = src_x as usize;
        let src_y = src_y as usize;
        return match frame.pixels {
            PreviewRawPixels::Rgb8(rgb) => {
                let si = (src_y * frame.source_w as usize + src_x) * 3;
                (si + 2 < rgb.len()).then(|| (rgb[si], rgb[si + 1], rgb[si + 2]))
            }
            PreviewRawPixels::Rgb565 {
                pixels,
                stride_pixels,
            } => {
                let idx = src_y * stride_pixels + src_x;
                (idx < pixels.len()).then(|| rgb565_to_rgb(pixels[idx]))
            }
        };
    }
    let scaled_w = ((frame.display_w as u64 * scale_num as u64) / scale_den as u64)
        .max(1)
        .min(isize::MAX as u64) as isize;
    let scaled_h = ((frame.display_h as u64 * scale_num as u64) / scale_den as u64)
        .max(1)
        .min(isize::MAX as u64) as isize;
    let center_x = screen.x0 as isize + ARCADE_PREVIEW_BOX_W as isize / 2 + offset_x;
    let center_y = screen.y0 as isize + ARCADE_PREVIEW_BOX_H as isize / 2;
    let image_x = center_x - scaled_w / 2;
    let image_y = center_y - scaled_h / 2;
    let local_x = x as isize - image_x;
    let local_y = y as isize - image_y;
    if local_x < 0 || local_y < 0 || local_x >= scaled_w || local_y >= scaled_h {
        return None;
    }
    let src_w = frame.source_w as usize;
    let src_h = frame.source_h as usize;
    let src_x = ((local_x as u64 * frame.source_w as u64) / scaled_w as u64)
        .min(frame.source_w.saturating_sub(1) as u64) as usize;
    let src_y = ((local_y as u64 * frame.source_h as u64) / scaled_h as u64)
        .min(frame.source_h.saturating_sub(1) as u64) as usize;
    match frame.pixels {
        PreviewRawPixels::Rgb8(rgb) => {
            let si = (src_y * src_w + src_x) * 3;
            (si + 2 < rgb.len()).then(|| (rgb[si], rgb[si + 1], rgb[si + 2]))
        }
        PreviewRawPixels::Rgb565 {
            pixels,
            stride_pixels,
        } => {
            if src_x >= src_w || src_y >= src_h || src_y * stride_pixels + src_x >= pixels.len() {
                None
            } else {
                Some(rgb565_to_rgb(pixels[src_y * stride_pixels + src_x]))
            }
        }
    }
}

fn blend_rgb(from: (u8, u8, u8), to: (u8, u8, u8), alpha: u8) -> (u8, u8, u8) {
    let a = alpha as u16;
    let ia = 255u16.saturating_sub(a);
    (
        ((from.0 as u16 * ia + to.0 as u16 * a) / 255) as u8,
        ((from.1 as u16 * ia + to.1 as u16 * a) / 255) as u8,
        ((from.2 as u16 * ia + to.2 as u16 * a) / 255) as u8,
    )
}

fn brighten_rgb(rgb: (u8, u8, u8), add: u8) -> (u8, u8, u8) {
    (
        rgb.0.saturating_add(add),
        rgb.1.saturating_add(add),
        rgb.2.saturating_add(add),
    )
}

fn hash2_u8(x: usize, y: usize) -> u8 {
    let mut v = (x as u32).wrapping_mul(0x45d9f3b) ^ (y as u32).wrapping_mul(0x119de1f3);
    v ^= v >> 16;
    v = v.wrapping_mul(0x45d9f3b);
    (v >> 24) as u8
}

struct Raw565View<'a> {
    pixels: &'a [Rgb565Pixel],
    stride_pixels: usize,
    w: usize,
    h: usize,
    x: isize,
    y: isize,
}

fn raw565_view<'a>(
    frame: &'a PreviewRawFrame<'a>,
    screen: DirtyRect,
    offset_x: isize,
) -> Option<Raw565View<'a>> {
    if frame.display_w != frame.source_w || frame.display_h != frame.source_h {
        return None;
    }
    let PreviewRawPixels::Rgb565 {
        pixels,
        stride_pixels,
    } = frame.pixels
    else {
        return None;
    };
    let w = frame.source_w as usize;
    let h = frame.source_h as usize;
    if w == 0 || h == 0 || stride_pixels < w || pixels.len() < stride_pixels * h {
        return None;
    }
    Some(Raw565View {
        pixels,
        stride_pixels,
        w,
        h,
        x: screen.x0 as isize + (ARCADE_PREVIEW_BOX_W as isize - w as isize) / 2 + offset_x,
        y: screen.y0 as isize + (ARCADE_PREVIEW_BOX_H as isize - h as isize) / 2,
    })
}

fn sample_raw565(view: &Raw565View<'_>, x: usize, y: usize) -> Option<Rgb565Pixel> {
    let sx = x as isize - view.x;
    let sy = y as isize - view.y;
    if sx < 0 || sy < 0 || sx >= view.w as isize || sy >= view.h as isize {
        None
    } else {
        Some(view.pixels[sy as usize * view.stride_pixels + sx as usize])
    }
}

fn blend_565(from: Rgb565Pixel, to: Rgb565Pixel, alpha: u8) -> Rgb565Pixel {
    let a = alpha as u32;
    let ia = 255u32.saturating_sub(a);
    let f = from.0 as u32;
    let t = to.0 as u32;
    let fr = (f >> 11) & 0x1f;
    let fg = (f >> 5) & 0x3f;
    let fb = f & 0x1f;
    let tr = (t >> 11) & 0x1f;
    let tg = (t >> 5) & 0x3f;
    let tb = t & 0x1f;
    let r = (fr * ia + tr * a) / 255;
    let g = (fg * ia + tg * a) / 255;
    let b = (fb * ia + tb * a) / 255;
    Rgb565Pixel(((r << 11) | (g << 5) | b) as u16)
}

fn darken_565(pixel: Rgb565Pixel) -> Rgb565Pixel {
    let v = pixel.0 as u32;
    let r = (((v >> 11) & 0x1f) * 5) / 8;
    let g = (((v >> 5) & 0x3f) * 5) / 8;
    let b = ((v & 0x1f) * 5) / 8;
    Rgb565Pixel(((r << 11) | (g << 5) | b) as u16)
}

fn brighten_565(pixel: Rgb565Pixel) -> Rgb565Pixel {
    let v = pixel.0 as u32;
    let r = ((v >> 11) & 0x1f).saturating_add(8).min(0x1f);
    let g = ((v >> 5) & 0x3f).saturating_add(16).min(0x3f);
    let b = (v & 0x1f).saturating_add(8).min(0x1f);
    Rgb565Pixel(((r << 11) | (g << 5) | b) as u16)
}

fn mosaic_block_size(progress: f32) -> usize {
    let p = progress.clamp(0.0, 1.0);
    if p >= 0.96 {
        1
    } else if p >= 0.78 {
        2
    } else if p >= 0.58 {
        4
    } else if p >= 0.38 {
        8
    } else if p >= 0.18 {
        16
    } else {
        32
    }
}

fn blit_transition_565_fast(
    cached: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    frame: &PreviewRawTransitionFrame<'_>,
    effect: PreviewTransitionEffect,
    progress: f32,
) -> Option<()> {
    let current = raw565_view(&frame.current, screen, 0)?;
    let alpha = (progress.clamp(0.0, 1.0) * 255.0).round() as u8;
    let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
    let prev_offset = -((progress * screen.width() as f32).round() as isize);
    let current_offset = ((1.0 - progress) * screen.width() as f32).round() as isize;
    let previous = frame
        .previous
        .as_ref()
        .and_then(|prev| raw565_view(prev, screen, 0));
    let slide_previous = frame
        .previous
        .as_ref()
        .and_then(|prev| raw565_view(prev, screen, prev_offset));
    let slide_current = raw565_view(&frame.current, screen, current_offset);
    let reveal_w = ((screen.width() as f32) * progress).round() as usize;
    let reveal_h = ((screen.rows() as f32) * progress).round() as usize;
    let cx = screen.width() / 2;
    let cy = screen.rows() as usize / 2;
    let zoom_w = reveal_w / 2;
    let zoom_h = reveal_h / 2;

    for y in screen.y0..screen.y1.min(ui.render_h()) {
        let row = y * ui.render_w();
        let local_y = y - screen.y0;
        for x in screen.x0..screen.x1.min(ui.render_w()) {
            let local_x = x - screen.x0;
            let prev = previous
                .as_ref()
                .and_then(|view| sample_raw565(view, x, y))
                .unwrap_or(black);
            let curr = sample_raw565(&current, x, y).unwrap_or(black);
            cached[row + x] = match effect {
                PreviewTransitionEffect::Cut => curr,
                PreviewTransitionEffect::Fade => blend_565(prev, curr, alpha),
                PreviewTransitionEffect::Wipe => {
                    if local_x < reveal_w {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::Slide => slide_current
                    .as_ref()
                    .and_then(|view| sample_raw565(view, x, y))
                    .or_else(|| {
                        slide_previous
                            .as_ref()
                            .and_then(|view| sample_raw565(view, x, y))
                    })
                    .unwrap_or(black),
                PreviewTransitionEffect::Zoom => {
                    if local_x.abs_diff(cx) <= zoom_w && local_y.abs_diff(cy) <= zoom_h {
                        blend_565(prev, curr, alpha)
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::Scanline => {
                    if local_y < reveal_h {
                        let blended = blend_565(prev, curr, alpha);
                        if local_y & 3 == 0 {
                            darken_565(blended)
                        } else {
                            blended
                        }
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::Checker => {
                    if hash2_u8(local_x / 16, local_y / 16) <= alpha {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::Dissolve => {
                    if hash2_u8(local_x / 2, local_y / 2) <= alpha {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::CrtBeamWipe => {
                    let beam_y = (progress * (screen.rows() as f32 + 4.0)).round() as isize - 2;
                    let dy = local_y as isize - beam_y;
                    let base = if dy <= 0 {
                        curr
                    } else if dy <= 10 {
                        blend_565(prev, curr, 220u8.saturating_sub((dy as u8) * 18))
                    } else {
                        prev
                    };
                    if dy.abs() <= 2 {
                        brighten_565(base)
                    } else {
                        base
                    }
                }
                PreviewTransitionEffect::MosaicResolve => {
                    let block = mosaic_block_size(progress);
                    let sample_x = (screen.x0 + (local_x / block) * block + block / 2)
                        .min(screen.x1.saturating_sub(1));
                    let sample_y = (screen.y0 + (local_y / block) * block + block / 2)
                        .min(screen.y1.saturating_sub(1));
                    let chunky = sample_raw565(&current, sample_x, sample_y).unwrap_or(curr);
                    blend_565(prev, chunky, alpha)
                }
            };
        }
    }
    Some(())
}

fn transition_rgb(
    frame: &PreviewRawTransitionFrame<'_>,
    screen: DirtyRect,
    effect: PreviewTransitionEffect,
    progress: f32,
    x: usize,
    y: usize,
) -> (u8, u8, u8) {
    let alpha = (progress.clamp(0.0, 1.0) * 255.0).round() as u8;
    let local_x = x.saturating_sub(screen.x0);
    let local_y = y.saturating_sub(screen.y0);
    let prev = frame
        .previous
        .as_ref()
        .and_then(|prev| sample_preview_rgb(prev, screen, x, y, 0, 1024, 1024))
        .unwrap_or((0, 0, 0));
    let current =
        sample_preview_rgb(&frame.current, screen, x, y, 0, 1024, 1024).unwrap_or((0, 0, 0));

    match effect {
        PreviewTransitionEffect::Cut => current,
        PreviewTransitionEffect::Fade => blend_rgb(prev, current, alpha),
        PreviewTransitionEffect::Wipe => {
            let reveal_w = ((screen.width() as f32) * progress).round() as usize;
            if local_x < reveal_w {
                current
            } else {
                prev
            }
        }
        PreviewTransitionEffect::Slide => {
            let pane_w = screen.width() as isize;
            let offset = ((1.0 - progress) * pane_w as f32).round() as isize;
            let prev_offset = -((progress * pane_w as f32).round() as isize);
            let sliding_current =
                sample_preview_rgb(&frame.current, screen, x, y, offset, 1024, 1024);
            let sliding_prev = frame
                .previous
                .as_ref()
                .and_then(|prev| sample_preview_rgb(prev, screen, x, y, prev_offset, 1024, 1024));
            sliding_current.or(sliding_prev).unwrap_or((0, 0, 0))
        }
        PreviewTransitionEffect::Zoom => {
            let cx = screen.width() / 2;
            let cy = screen.rows() as usize / 2;
            let reveal_w = ((screen.width() as f32) * progress).round() as usize / 2;
            let reveal_h = ((screen.rows() as f32) * progress).round() as usize / 2;
            if local_x.abs_diff(cx) <= reveal_w && local_y.abs_diff(cy) <= reveal_h {
                blend_rgb(prev, current, alpha)
            } else {
                prev
            }
        }
        PreviewTransitionEffect::Scanline => {
            let mut rgb = blend_rgb(prev, current, alpha);
            if local_y & 3 == 0 {
                rgb.0 = ((rgb.0 as u16 * 5) / 8) as u8;
                rgb.1 = ((rgb.1 as u16 * 5) / 8) as u8;
                rgb.2 = ((rgb.2 as u16 * 5) / 8) as u8;
            }
            if local_y < ((screen.rows() as f32) * progress).round() as usize {
                rgb
            } else {
                prev
            }
        }
        PreviewTransitionEffect::Checker => {
            let tile = 16usize;
            let gate = hash2_u8(local_x / tile, local_y / tile);
            if gate <= alpha {
                current
            } else {
                prev
            }
        }
        PreviewTransitionEffect::Dissolve => {
            let gate = hash2_u8(local_x / 2, local_y / 2);
            if gate <= alpha {
                current
            } else {
                prev
            }
        }
        PreviewTransitionEffect::CrtBeamWipe => {
            let beam_y = (progress * (screen.rows() as f32 + 4.0)).round() as isize - 2;
            let dy = local_y as isize - beam_y;
            let base = if dy <= 0 {
                current
            } else if dy <= 10 {
                blend_rgb(prev, current, 220u8.saturating_sub((dy as u8) * 18))
            } else {
                prev
            };
            if dy.abs() <= 2 {
                brighten_rgb(base, 72)
            } else {
                base
            }
        }
        PreviewTransitionEffect::MosaicResolve => {
            let block = mosaic_block_size(progress);
            let sample_x = (screen.x0 + (local_x / block) * block + block / 2)
                .min(screen.x1.saturating_sub(1));
            let sample_y = (screen.y0 + (local_y / block) * block + block / 2)
                .min(screen.y1.saturating_sub(1));
            let chunky =
                sample_preview_rgb(&frame.current, screen, sample_x, sample_y, 0, 1024, 1024)
                    .unwrap_or(current);
            blend_rgb(prev, chunky, alpha)
        }
    }
}

struct PresentProbe {
    pixels: Vec<Pixel>,
}

impl PresentProbe {
    const X: usize = 12;
    const Y: usize = 12;
    const W: usize = 208;
    const H: usize = 72;

    fn from_env() -> Option<Self> {
        matches!(
            std::env::var("MISTER_PRESENT_PROBE").as_deref(),
            Ok("1") | Ok("on") | Ok("true") | Ok("yes")
        )
        .then(|| Self {
            pixels: vec![Pixel(0); Self::W * Self::H],
        })
    }

    fn present(&mut self, disp: &mut Display, frame: u64) -> u32 {
        self.draw(frame);
        disp.copy_rect_from(Self::X, Self::Y, Self::W, Self::H, &self.pixels);
        Self::H as u32
    }

    fn draw(&mut self, frame: u64) {
        self.pixels.fill(Pixel::from_rgb(0, 0, 0));
        let edge = if frame & 1 == 0 {
            Pixel::from_rgb(0, 255, 255)
        } else {
            Pixel::from_rgb(255, 0, 255)
        };
        self.fill_rect(0, 0, Self::W, Self::H, Pixel::from_rgb(8, 8, 14));
        self.stroke_rect(0, 0, Self::W, Self::H, edge);

        let flash = if frame & 1 == 0 {
            Pixel::from_rgb(255, 255, 255)
        } else {
            Pixel::from_rgb(0, 0, 0)
        };
        self.fill_rect(6, 6, 36, 36, flash);
        self.stroke_rect(6, 6, 36, 36, edge);

        let marker_x = 48 + (frame as usize % 150);
        self.fill_rect(marker_x, 4, 3, Self::H - 8, Pixel::from_rgb(255, 40, 40));

        let mut value = (frame % 10_000) as u16;
        let mut digits = [0u8; 4];
        for digit in digits.iter_mut().rev() {
            *digit = (value % 10) as u8;
            value /= 10;
        }
        for (i, digit) in digits.into_iter().enumerate() {
            self.draw_digit(58 + i * 28, 9, digit, Pixel::from_rgb(255, 242, 96));
        }

        for bit in 0..8 {
            let on = ((frame >> (7 - bit)) & 1) != 0;
            let color = if on {
                Pixel::from_rgb(64, 255, 96)
            } else {
                Pixel::from_rgb(32, 44, 40)
            };
            self.fill_rect(8 + bit * 24, 52, 18, 12, color);
            self.stroke_rect(8 + bit * 24, 52, 18, 12, Pixel::from_rgb(160, 160, 160));
        }
    }

    fn draw_digit(&mut self, x: usize, y: usize, digit: u8, color: Pixel) {
        const SEGMENTS: [u8; 10] = [
            0b1111110, 0b0110000, 0b1101101, 0b1111001, 0b0110011, 0b1011011, 0b1011111, 0b1110000,
            0b1111111, 0b1111011,
        ];
        let mask = SEGMENTS[digit as usize];
        let seg = |this: &mut Self, bit: u8, rx: usize, ry: usize, rw: usize, rh: usize| {
            if (mask & (1 << bit)) != 0 {
                this.fill_rect(x + rx, y + ry, rw, rh, color);
            }
        };
        seg(self, 6, 3, 0, 18, 4);
        seg(self, 5, 20, 3, 4, 14);
        seg(self, 4, 20, 21, 4, 14);
        seg(self, 3, 3, 36, 18, 4);
        seg(self, 2, 0, 21, 4, 14);
        seg(self, 1, 0, 3, 4, 14);
        seg(self, 0, 3, 18, 18, 4);
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Pixel) {
        let x1 = (x + w).min(Self::W);
        let y1 = (y + h).min(Self::H);
        for yy in y..y1 {
            let row = yy * Self::W;
            for xx in x..x1 {
                self.pixels[row + xx] = color;
            }
        }
    }

    fn stroke_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Pixel) {
        if w == 0 || h == 0 {
            return;
        }
        self.fill_rect(x, y, w, 1, color);
        self.fill_rect(x, y + h - 1, w, 1, color);
        self.fill_rect(x, y, 1, h, color);
        self.fill_rect(x + w - 1, y, 1, h, color);
    }
}

struct EffectLabelOverlay {
    font: ConsoleFont,
    pixels: Vec<Pixel>,
}

impl EffectLabelOverlay {
    const X: usize = 10;
    const Y: usize = 10;
    const W: usize = 260;
    const H: usize = 26;

    fn new() -> Self {
        Self {
            font: ConsoleFont::new(10.0),
            pixels: vec![Pixel(0); Self::W * Self::H],
        }
    }

    fn draw(&mut self, target: &mut UiFrameTarget, ui: &UiDisplay, effect: &str) -> DirtyRect {
        self.pixels.fill(Pixel::from_rgb(0, 0, 0));
        self.fill_rect(1, 1, Self::W - 2, Self::H - 2, Pixel::from_rgb(10, 14, 20));
        self.stroke_rect(0, 0, Self::W, Self::H, Pixel::from_rgb(69, 229, 255));
        self.font.draw_text_clipped(
            &mut self.pixels,
            Self::W,
            Self::W,
            0,
            Self::H,
            8,
            18,
            &format!("EFFECT: {}", effect.to_ascii_uppercase()),
            Pixel::from_rgb(255, 244, 126),
        );
        target.blit_pixel_rect(ui, Self::X, Self::Y, Self::W, Self::H, &self.pixels)
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Pixel) {
        let x1 = (x + w).min(Self::W);
        let y1 = (y + h).min(Self::H);
        for yy in y..y1 {
            let row = yy * Self::W;
            for xx in x..x1 {
                self.pixels[row + xx] = color;
            }
        }
    }

    fn stroke_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Pixel) {
        if w == 0 || h == 0 {
            return;
        }
        self.fill_rect(x, y, w, 1, color);
        self.fill_rect(x, y + h - 1, w, 1, color);
        self.fill_rect(x, y, 1, h, color);
        self.fill_rect(x + w - 1, y, 1, h, color);
    }
}

pub(crate) enum UiFrameTarget {
    Xrgb8888 { cached: Vec<Pixel> },
    Rgb565 { cached: Vec<Rgb565Pixel> },
}

impl UiFrameTarget {
    fn cached(ui: &UiDisplay) -> Self {
        Self::cached_with_format(ui, FramebufferFormat::from_env())
    }

    fn cached_with_format(ui: &UiDisplay, format: FramebufferFormat) -> Self {
        match format {
            FramebufferFormat::Xrgb8888 => Self::Xrgb8888 {
                cached: vec![Pixel(0); ui.render_w() * ui.render_h()],
            },
            FramebufferFormat::Rgb565 => Self::Rgb565 {
                cached: vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()],
            },
        }
    }

    fn open(ui: &UiDisplay) -> Self {
        let format = FramebufferFormat::from_env();
        println!("slint-render-target=cached fb-format={}", format.label());
        Self::cached_with_format(ui, format)
    }

    fn render(&mut self, renderer: &SoftwareRenderer, ui: &UiDisplay) -> PhysicalRegion {
        match self {
            Self::Xrgb8888 { cached } => renderer.render(cached, ui.render_w()),
            Self::Rgb565 { cached } => renderer.render(cached, ui.render_w()),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Xrgb8888 { .. } => "cached-8888",
            Self::Rgb565 { .. } => "cached-565",
        }
    }

    fn present_rect(
        &mut self,
        f: &mut Fpga,
        disp: &mut Display,
        ui: &UiDisplay,
        rect: DirtyRect,
    ) -> u32 {
        let _ = f;
        match self {
            Self::Xrgb8888 { cached } => copy_cached_rect(disp, ui, cached, rect),
            Self::Rgb565 { cached } => copy_cached_rect_565(disp, ui, cached, rect),
        }
        rect.rows()
    }

    fn blit_raw_preview(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawFrame<'_>,
        clear_screen: bool,
    ) -> Option<DirtyRect> {
        let rect = raw_preview_scaled_rect(ui, frame)?;
        let screen = preview_screen_rect(ui);
        let image_x =
            screen.x0 as isize + (ARCADE_PREVIEW_BOX_W as isize - frame.display_w as isize) / 2;
        let image_y =
            screen.y0 as isize + (ARCADE_PREVIEW_BOX_H as isize - frame.display_h as isize) / 2;
        let scale_x = (frame.display_w / frame.source_w).max(1) as usize;
        let scale_y = (frame.display_h / frame.source_h).max(1) as usize;
        let src_w = frame.source_w as usize;
        let src_h = frame.source_h as usize;

        match self {
            Self::Xrgb8888 { cached } => {
                if clear_screen {
                    for y in screen.y0..screen.y1.min(ui.render_h()) {
                        let row = y * ui.render_w();
                        for x in screen.x0..screen.x1.min(ui.render_w()) {
                            cached[row + x] = Pixel(0);
                        }
                    }
                }
                match frame.pixels {
                    PreviewRawPixels::Rgb8(rgb) => {
                        for y in rect.y0..rect.y1 {
                            let src_y =
                                ((y as isize - image_y).max(0) as usize / scale_y).min(src_h - 1);
                            let row = y * ui.render_w();
                            for x in rect.x0..rect.x1 {
                                let src_x = ((x as isize - image_x).max(0) as usize / scale_x)
                                    .min(src_w - 1);
                                let si = (src_y * src_w + src_x) * 3;
                                cached[row + x] =
                                    Pixel::from_rgb(rgb[si], rgb[si + 1], rgb[si + 2]);
                            }
                        }
                    }
                    PreviewRawPixels::Rgb565 {
                        pixels,
                        stride_pixels,
                    } => {
                        for y in rect.y0..rect.y1 {
                            let src_y =
                                ((y as isize - image_y).max(0) as usize / scale_y).min(src_h - 1);
                            let row = y * ui.render_w();
                            for x in rect.x0..rect.x1 {
                                let src_x = ((x as isize - image_x).max(0) as usize / scale_x)
                                    .min(src_w - 1);
                                cached[row + x] =
                                    rgb565_to_pixel(pixels[src_y * stride_pixels + src_x]);
                            }
                        }
                    }
                }
            }
            Self::Rgb565 { cached } => {
                if clear_screen {
                    let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
                    for y in screen.y0..screen.y1.min(ui.render_h()) {
                        let row = y * ui.render_w();
                        for x in screen.x0..screen.x1.min(ui.render_w()) {
                            cached[row + x] = black;
                        }
                    }
                }
                match frame.pixels {
                    PreviewRawPixels::Rgb565 {
                        pixels,
                        stride_pixels,
                    } if frame.display_w == frame.source_w && frame.display_h == frame.source_h => {
                        for y in rect.y0..rect.y1 {
                            let src_y = (y as isize - image_y).max(0) as usize;
                            let src_x = (rect.x0 as isize - image_x).max(0) as usize;
                            let src_a = src_y * stride_pixels + src_x;
                            let dst_a = y * ui.render_w() + rect.x0;
                            cached[dst_a..dst_a + rect.width()]
                                .copy_from_slice(&pixels[src_a..src_a + rect.width()]);
                        }
                    }
                    PreviewRawPixels::Rgb565 {
                        pixels,
                        stride_pixels,
                    } => {
                        for y in rect.y0..rect.y1 {
                            let src_y =
                                ((y as isize - image_y).max(0) as usize / scale_y).min(src_h - 1);
                            let row = y * ui.render_w();
                            for x in rect.x0..rect.x1 {
                                let src_x = ((x as isize - image_x).max(0) as usize / scale_x)
                                    .min(src_w - 1);
                                cached[row + x] = pixels[src_y * stride_pixels + src_x];
                            }
                        }
                    }
                    PreviewRawPixels::Rgb8(rgb) => {
                        for y in rect.y0..rect.y1 {
                            let src_y =
                                ((y as isize - image_y).max(0) as usize / scale_y).min(src_h - 1);
                            let row = y * ui.render_w();
                            for x in rect.x0..rect.x1 {
                                let src_x = ((x as isize - image_x).max(0) as usize / scale_x)
                                    .min(src_w - 1);
                                let si = (src_y * src_w + src_x) * 3;
                                cached[row + x] = <Rgb565Pixel as TargetPixel>::from_rgb(
                                    rgb[si],
                                    rgb[si + 1],
                                    rgb[si + 2],
                                );
                            }
                        }
                    }
                }
            }
        }
        Some(if clear_screen { screen } else { rect })
    }

    fn blit_raw_preview_transition(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawTransitionFrame<'_>,
        effect: PreviewTransitionEffect,
        progress: f32,
    ) -> DirtyRect {
        let screen = preview_screen_rect(ui);
        match self {
            Self::Xrgb8888 { cached } => {
                for y in screen.y0..screen.y1.min(ui.render_h()) {
                    let row = y * ui.render_w();
                    for x in screen.x0..screen.x1.min(ui.render_w()) {
                        let rgb = transition_rgb(frame, screen, effect, progress, x, y);
                        cached[row + x] = Pixel::from_rgb(rgb.0, rgb.1, rgb.2);
                    }
                }
            }
            Self::Rgb565 { cached } => {
                if blit_transition_565_fast(cached, ui, screen, frame, effect, progress).is_some() {
                    return screen;
                }
                for y in screen.y0..screen.y1.min(ui.render_h()) {
                    let row = y * ui.render_w();
                    for x in screen.x0..screen.x1.min(ui.render_w()) {
                        let rgb = transition_rgb(frame, screen, effect, progress, x, y);
                        cached[row + x] =
                            <Rgb565Pixel as TargetPixel>::from_rgb(rgb.0, rgb.1, rgb.2);
                    }
                }
            }
        }
        screen
    }

    fn blit_pixel_rect(
        &mut self,
        ui: &UiDisplay,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Pixel],
    ) -> DirtyRect {
        let w = w.min(ui.render_w().saturating_sub(x));
        let h = h.min(ui.render_h().saturating_sub(y));
        match self {
            Self::Xrgb8888 { cached } => {
                for yy in 0..h {
                    let dst = (y + yy) * ui.render_w() + x;
                    let src_idx = yy * w;
                    cached[dst..dst + w].copy_from_slice(&src[src_idx..src_idx + w]);
                }
            }
            Self::Rgb565 { cached } => {
                for yy in 0..h {
                    let dst = (y + yy) * ui.render_w() + x;
                    let src_idx = yy * w;
                    for xx in 0..w {
                        let rgb = pixel_to_rgb(src[src_idx + xx]);
                        cached[dst + xx] =
                            <Rgb565Pixel as TargetPixel>::from_rgb(rgb.0, rgb.1, rgb.2);
                    }
                }
            }
        }
        DirtyRect {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
        }
    }

    fn present_rows(
        &mut self,
        f: &mut Fpga,
        disp: &mut Display,
        ui: &UiDisplay,
        y0: usize,
        y1: usize,
    ) -> u32 {
        let _ = f;
        match self {
            Self::Xrgb8888 { cached } => copy_cached_rows(disp, ui, cached, y0, y1),
            Self::Rgb565 { cached } => copy_cached_rows_565(disp, ui, cached, y0, y1),
        }
        y1.saturating_sub(y0) as u32
    }

    pub(crate) fn copy_rect_from(
        &mut self,
        disp: &mut Display,
        ui: &UiDisplay,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Pixel],
    ) {
        let _ = ui;
        disp.copy_rect_from(x, y, w, h, src);
    }
}

fn blit_raw_preview_if_needed(
    target: &mut UiFrameTarget,
    ui: &UiDisplay,
    preview: &mut PreviewState,
    transition: &mut PreviewTransitionDemo,
    elapsed: Duration,
    slint_dirty: Option<DirtyRect>,
) -> (Option<DirtyRect>, PreviewTransitionTrace) {
    let raw_dirty = preview.take_raw_dirty();
    let slint_touched_preview = slint_dirty
        .and_then(|rect| rect.intersection(preview_screen_rect(ui)))
        .is_some();
    let transition_frame = preview.raw_transition_frame();
    let trace = transition.update(transition_frame.as_ref(), elapsed);
    if !raw_dirty && !slint_touched_preview && !trace.active {
        return (None, trace);
    }
    let Some(transition_frame) = transition_frame else {
        return (None, trace);
    };
    let raw_rect = if trace.active {
        target.blit_raw_preview_transition(ui, &transition_frame, trace.effect, trace.progress)
    } else {
        let Some(raw_rect) = target.blit_raw_preview(ui, &transition_frame.current, raw_dirty)
        else {
            return (None, trace);
        };
        raw_rect
    };
    if slint_dirty.is_some_and(|rect| rect.contains(raw_rect)) {
        (None, trace)
    } else {
        (Some(raw_rect), trace)
    }
}

fn copy_arcade_list_update(
    target: &mut UiFrameTarget,
    disp: &mut Display,
    ui: &UiDisplay,
    renderer: &mut ArcadeListRenderer,
    update: ArcadeListUpdate,
) -> u32 {
    match update {
        ArcadeListUpdate::Full(rect) => {
            renderer.copy_layer_to_target(target, disp, ui);
            rect.rows()
        }
        ArcadeListUpdate::Scroll { .. } => {
            renderer.copy_layer_to_target(target, disp, ui);
            ArcadeListRenderer::dirty_rect().rows()
        }
    }
}

fn frame_rect(rect: DirtyRect) -> FrameRect {
    FrameRect {
        x0: rect.x0 as u32,
        y0: rect.y0 as u32,
        x1: rect.x1 as u32,
        y1: rect.y1 as u32,
    }
}

fn configure_window(ui: &UiDisplay, window: &Rc<MinimalSoftwareWindow>) {
    window.set_size(PhysicalSize::new(
        ui.render_w() as u32,
        ui.render_h() as u32,
    ));
}

macro_rules! with_scene_app {
    ($module:ident::$ty:ident, $ui:expr, $window:expr, $app:ident, $body:block) => {{
        boot_analytics::event(
            "app_construct_attempt",
            format!("scene_type={}", stringify!($module::$ty)),
        );
        let $app = slint_ui::$module::$ty::new().expect(stringify!($ty));
        boot_analytics::event(
            "app_construct",
            format!("scene_type={} ok=1", stringify!($module::$ty)),
        );
        let mister_ui = $app.global::<slint_ui::$module::MisterUi>();
        mister_ui.set_scale(SLINT_UI_SCALE);
        mister_ui.set_window_width($ui.render_w() as i32);
        mister_ui.set_window_height($ui.render_h() as i32);
        configure_window($ui, $window);
        $body
    }};
}

pub fn run_ui(f: &mut Fpga) {
    let (scene, secs) = parse_ui_args();
    boot_analytics::event("run_ui_start", format!("scene={scene} secs={secs}"));
    println!("ui scene={scene} secs={secs}");
    println!("ui_render_mode=cached");

    let _vt = VtGraphicsGuard::enter_or_warn();

    let fb_format = FramebufferFormat::from_env();
    println!(
        "ui-fb-mode=temporary {UI_FB_W}x{UI_FB_H} format={} fpga-scale=1920x1080 restore=on-drop",
        fb_format.label()
    );
    let _fb_mode_guard = match FbModeGuard::set_temporary_format(UI_FB_W, UI_FB_H, fb_format) {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("failed to set temporary framebuffer mode for FPGA-scaled UI: {e}");
            std::process::exit(1);
        }
    };

    println!("display-open-path=temporary-fb-fpga-scale");
    let mut disp = match Display::open_with_format(UI_FB_W, UI_FB_H, fb_format) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open display (/dev/fb0): {e}");
            std::process::exit(1);
        }
    };
    let ui = UiDisplay::for_framebuffer(disp.width(), disp.height());
    println!("{}", ui.log_line());
    disp.record_visual_sample("after_display_open_before_initial_route");
    let display_config = match DisplayConfig::detect(f, disp.info(), &ui) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("failed to read display configuration from FPGA: {e}");
            std::process::exit(1);
        }
    };
    println!("{}", display_config.log_line());
    boot_analytics::event(
        "display_config_detected",
        display_config.boot_analytics_detail(),
    );
    if std::env::var_os("MISTER_MAGIK_PARENT").is_some() {
        println!("MiSTer_MagiK parent detected; Slint reasserting framebuffer route");
    }
    let route_mode = ui_fpga_scaled_mode();
    let route_mode_label = "fpga-scale-1920x1080";
    let set_vga_fb = std::env::var_os("MISTER_DIRECT_VIDEO").is_some();
    boot_analytics::event(
        "initial_fb_enable_direct_attempt",
        format!(
            "w={} h={} mode={route_mode_label} set_vga_fb={set_vga_fb}",
            disp.width(),
            disp.height()
        ),
    );
    let flag = match f.fb_enable_format(
        0,
        disp.width() as u16,
        disp.height() as u16,
        route_mode,
        Some(0),
        Some(0),
        set_vga_fb,
        fb_format,
    ) {
        Ok(flag) => flag,
        Err(e) => {
            eprintln!("failed to route framebuffer for Slint UI: {e}");
            std::process::exit(1);
        }
    };
    boot_analytics::event(
        "initial_fb_enable_direct_done",
        format!("support_flag={flag}"),
    );
    disp.record_visual_sample("after_initial_route_before_slint_draw");
    match f.set_audio_volume(0) {
        Ok(()) => boot_analytics::event("set_audio_volume", "attenuation=0"),
        Err(e) => {
            eprintln!("warning: failed to set FPGA audio volume: {e}");
            boot_analytics::event("set_audio_volume_failed", format!("error={e}"));
        }
    }
    println!(
        "fb routed (support_flag={flag}); Slint software renderer (vsync, dirty-row copy, fpga_scale=true)"
    );

    if scene == "blend_velocity" {
        crate::ui_blend_velocity::run_blend_velocity_loop(secs, &mut disp);
        return;
    }

    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    let animation_clock = AnimationClock::from_env();
    slint::platform::set_platform(Box::new(MisterPlatform {
        window: window.clone(),
        start: Instant::now(),
        fixed_time: animation_clock.platform_time(),
    }))
    .expect("set_platform");
    boot_analytics::event("slint_platform_set", "ok=1");

    match scene.as_str() {
        #[cfg(not(mister_ui_scope_launcher))]
        "demo" => {
            with_scene_app!(app::AppWindow, &ui, &window, app, {
                app.show().expect("show");
                let mut target = UiFrameTarget::open(&ui);
                run_frame_loop(
                    secs,
                    &ui,
                    &mut disp,
                    f,
                    &window,
                    &mut target,
                    &animation_clock,
                );
            });
        }
        #[cfg(mister_bench_scenes)]
        "full_motion" => {
            with_scene_app!(full_motion::FullMotion, &ui, &window, app, {
                app.show().expect("show");
                let mut target = UiFrameTarget::open(&ui);
                run_frame_loop(
                    secs,
                    &ui,
                    &mut disp,
                    f,
                    &window,
                    &mut target,
                    &animation_clock,
                );
            });
        }
        #[cfg(mister_bench_scenes)]
        "static_ui" => {
            with_scene_app!(static_ui::StaticUi, &ui, &window, app, {
                app.show().expect("show");
                let mut target = UiFrameTarget::open(&ui);
                run_frame_loop(
                    secs,
                    &ui,
                    &mut disp,
                    f,
                    &window,
                    &mut target,
                    &animation_clock,
                );
            });
        }
        #[cfg(mister_bench_scenes)]
        "local_motion" => {
            with_scene_app!(local_motion::LocalMotion, &ui, &window, app, {
                app.show().expect("show");
                let mut target = UiFrameTarget::open(&ui);
                run_frame_loop(
                    secs,
                    &ui,
                    &mut disp,
                    f,
                    &window,
                    &mut target,
                    &animation_clock,
                );
            });
        }
        #[cfg(mister_bench_scenes)]
        "console_scroll" => {
            with_scene_app!(console_scroll::ConsoleScroll, &ui, &window, app, {
                app.show().expect("show");
                run_console_scroll_loop(secs, &ui, &mut disp, &window, app, &animation_clock);
            });
        }
        #[cfg(all(feature = "video", mister_bench_scenes))]
        "video_playback" => {
            with_scene_app!(video_playback::VideoPlayback, &ui, &window, app, {
                app.show().expect("show");
                window.request_redraw();
                run_video_playback_loop(secs, &ui, &mut disp, &window, app, &animation_clock);
            });
        }
        "controller_test" => {
            let pad = open_pads();
            with_scene_app!(controller::ControllerTest, &ui, &window, app, {
                sync_bridge(&app, &pad);
                app.show().expect("show");
                window.request_redraw();
                run_controller_loop(secs, &ui, &mut disp, &window, pad, app, &animation_clock);
            });
        }
        "arcade" => {
            let pad = open_pads();
            with_scene_app!(launcher::Launcher, &ui, &window, app, {
                init_launcher_bridge(&app, &pad);
                boot_analytics::event("app_show_attempt", "scene=arcade");
                app.show().expect("show");
                boot_analytics::event("app_show", "scene=arcade ok=1");
                window.request_redraw();
                let mut target = UiFrameTarget::open(&ui);
                run_launcher_loop(
                    secs,
                    &ui,
                    &mut disp,
                    f,
                    &window,
                    &mut target,
                    pad,
                    app,
                    &animation_clock,
                    LauncherRunMode::Arcade,
                );
            });
        }
        "launcher" => {
            let pad = open_pads();
            with_scene_app!(launcher::Launcher, &ui, &window, app, {
                init_launcher_bridge(&app, &pad);
                boot_analytics::event("app_show_attempt", "scene=launcher");
                app.show().expect("show");
                boot_analytics::event("app_show", "scene=launcher ok=1");
                window.request_redraw();
                let mut target = UiFrameTarget::open(&ui);
                run_launcher_loop(
                    secs,
                    &ui,
                    &mut disp,
                    f,
                    &window,
                    &mut target,
                    pad,
                    app,
                    &animation_clock,
                    LauncherRunMode::Launcher,
                );
            });
        }
        _ => unreachable!(),
    }
}

fn open_pads() -> PadPool {
    PadPool::open_all().unwrap_or_else(|e| {
        eprintln!("failed to initialize gamepad input: {e}");
        std::process::exit(1);
    })
}

fn init_launcher_bridge(app: &slint_ui::launcher::Launcher, pad: &PadPool) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_screen_mode(0);
    bridge.set_selected_index(0);
    bridge.set_settings_focused(false);
    bridge.set_settings_selected(0);
    bridge.set_confirm_visible(false);
    bridge.set_confirm_title("".into());
    bridge.set_confirm_message("".into());
    bridge.set_confirm_left_label("".into());
    bridge.set_confirm_right_label("".into());
    bridge.set_confirm_selected(0);
    bridge.set_game_systems(ModelRc::new(VecModel::from(Vec::<
        slint_ui::launcher::GameSystem,
    >::new())));
    bridge.set_home_scroll_x(0);
    bridge.set_active_system_title("".into());
    bridge.set_arcade_selected(0);
    bridge.set_arcade_scroll_y(0);
    sync_launcher_arcade_geometry_bridge(&bridge);
    bridge.set_arcade_preview_has_image(false);
    bridge.set_arcade_preview_placeholder_visible(true);
    bridge.set_arcade_preview_status(PreviewStatus::Empty);
    bridge.set_arcade_preview_title("".into());
    bridge.set_arcade_preview_run_label(preview_run_label().into());
    bridge.set_arcade_preview_image(Image::default());
    bridge.set_arcade_preview_source_width(0);
    bridge.set_arcade_preview_source_height(0);
    bridge.set_arcade_preview_display_width(0);
    bridge.set_arcade_preview_display_height(0);
    bridge.set_catalog_scan_visible(false);
    bridge.set_catalog_scan_title("".into());
    bridge.set_catalog_scan_detail("".into());
    bridge.set_setup_visible(false);
    bridge.set_setup_phase(0);
    sync_bridge_pad_launcher(&bridge, pad);
}

fn sync_launcher_arcade_geometry_bridge(bridge: &slint_ui::launcher::MisterBridge) {
    bridge.set_arcade_list_x(ARCADE_LIST_X as i32);
    bridge.set_arcade_list_y(ARCADE_LIST_Y as i32);
    bridge.set_arcade_list_width(ARCADE_LIST_W as i32);
    bridge.set_arcade_list_height(ARCADE_LIST_H as i32);
    bridge.set_arcade_list_visible(!preview_stress_enabled());
    bridge.set_arcade_preview_box_x(ARCADE_PREVIEW_BOX_X as i32);
    bridge.set_arcade_preview_box_y(ARCADE_PREVIEW_BOX_Y as i32);
    bridge.set_arcade_preview_box_width(ARCADE_PREVIEW_BOX_W as i32);
    bridge.set_arcade_preview_box_height(ARCADE_PREVIEW_BOX_H as i32);
}

fn sync_bridge(app: &slint_ui::controller::ControllerTest, pad: &PadPool) {
    sync_bridge_pad_controller(&app.global::<slint_ui::controller::MisterBridge>(), pad);
}

fn sync_confirm_bridge(
    bridge: &slint_ui::launcher::MisterBridge,
    action: Option<launcher::ConfirmAction>,
) {
    match action {
        Some(launcher::ConfirmAction::ExitToMister) => {
            bridge.set_confirm_title("Exit to MiSTer".into());
            bridge.set_confirm_message("Use the stock MiSTer menu until reboot.".into());
            bridge.set_confirm_left_label("Exit to MiSTer".into());
            bridge.set_confirm_right_label("Return to MiSTer MagiK".into());
        }
        Some(launcher::ConfirmAction::ResetDatabase) => {
            bridge.set_confirm_title("Reset Database?".into());
            bridge.set_confirm_message("Delete the library database and reboot the MiSTer?".into());
            bridge.set_confirm_left_label("Cancel".into());
            bridge.set_confirm_right_label("Confirm".into());
        }
        Some(launcher::ConfirmAction::Restart) => {
            bridge.set_confirm_title("Restart MiSTer?".into());
            bridge.set_confirm_message("Reboot the MiSTer now?".into());
            bridge.set_confirm_left_label("Cancel".into());
            bridge.set_confirm_right_label("Confirm".into());
        }
        None => {
            bridge.set_confirm_title("".into());
            bridge.set_confirm_message("".into());
            bridge.set_confirm_left_label("".into());
            bridge.set_confirm_right_label("".into());
        }
    }
}

fn sync_bridge_launcher(
    app: &slint_ui::launcher::Launcher,
    pad: &PadPool,
    nav: &LauncherNav,
    setup: &SetupNav,
    loading_message: &str,
    loading_detail: &str,
    catalog: Option<&ArcadeCatalog>,
    preview: &mut PreviewState,
    models: &mut LauncherBridgeModels,
    catalog_version: usize,
) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    sync_bridge_pad_launcher(&bridge, pad);
    bridge.set_screen_mode(match nav.screen {
        Screen::Home => 0,
        Screen::Controller => 1,
        Screen::Arcade => 2,
        Screen::Settings => 3,
    });
    bridge.set_clock_text(launcher_clock_text().into());
    bridge.set_selected_index(nav.selected as i32);
    bridge.set_home_scroll_x(nav.scroll_x);
    bridge.set_settings_focused(nav.settings_focused);
    bridge.set_settings_selected(nav.settings_selected as i32);
    bridge.set_arcade_selected(nav.arcade.selected as i32);
    bridge.set_arcade_scroll_y(nav.arcade.scroll_y);
    let mut active_games_for_preview: Option<&[ArcadeGameEntry]> = None;
    if let Some(catalog) = catalog {
        let games = active_system_game_slice(catalog, nav);
        let title = active_system(catalog, nav)
            .map(|system| system.title.clone())
            .unwrap_or_else(|| "Games".to_string());
        bridge.set_game_systems(models.game_systems(catalog, catalog_version));
        bridge.set_active_system_title(title.into());
        bridge.set_arcade_games(models.arcade_games(catalog, nav, catalog_version));
        active_games_for_preview = Some(games);
    }
    bridge.set_confirm_visible(nav.confirm_action.is_some());
    bridge.set_confirm_selected(nav.confirm_selected as i32);
    sync_confirm_bridge(&bridge, nav.confirm_action);
    bridge.set_loading_message(loading_message.into());
    bridge.set_loading_detail(loading_detail.into());
    if nav.screen == Screen::Arcade {
        let games = active_games_for_preview
            .or_else(|| catalog.map(|catalog| active_system_game_slice(catalog, nav)))
            .unwrap_or(&[]);
        let _ = request_arcade_preview_window(&bridge, games, nav.arcade.selected, preview);
    } else {
        preview.clear(&bridge);
    }
    sync_setup_bridge(&bridge, pad, setup);
}

fn sync_bridge_launcher_light(
    app: &slint_ui::launcher::Launcher,
    nav: &LauncherNav,
    setup: &SetupNav,
    loading_message: &str,
    loading_detail: &str,
    catalog: &ArcadeCatalog,
    active_arcade_games: Option<&[ArcadeGameEntry]>,
    preview: &mut PreviewState,
) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_screen_mode(match nav.screen {
        Screen::Home => 0,
        Screen::Controller => 1,
        Screen::Arcade => 2,
        Screen::Settings => 3,
    });
    bridge.set_selected_index(nav.selected as i32);
    bridge.set_home_scroll_x(nav.scroll_x);
    bridge.set_settings_focused(nav.settings_focused);
    bridge.set_settings_selected(nav.settings_selected as i32);
    bridge.set_arcade_selected(nav.arcade.selected as i32);
    bridge.set_arcade_scroll_y(nav.arcade.scroll_y);
    bridge.set_confirm_visible(nav.confirm_action.is_some());
    bridge.set_confirm_selected(nav.confirm_selected as i32);
    sync_confirm_bridge(&bridge, nav.confirm_action);
    bridge.set_loading_message(loading_message.into());
    bridge.set_loading_detail(loading_detail.into());
    if nav.screen == Screen::Arcade {
        let games = active_arcade_games.unwrap_or_else(|| active_system_game_slice(catalog, nav));
        schedule_arcade_preview_window(&bridge, games, nav.arcade.selected, preview);
    } else {
        preview.clear(&bridge);
    }
    bridge.set_setup_visible(setup.is_active());
}

fn launcher_clock_text() -> String {
    unsafe {
        let mut now: libc::time_t = 0;
        if libc::time(&mut now) == -1 {
            return "--:--".to_string();
        }
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "--:--".to_string();
        }
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

fn slint_arcade_games(games: &[ArcadeGameEntry]) -> ModelRc<slint_ui::launcher::ArcadeGame> {
    let rows: Vec<slint_ui::launcher::ArcadeGame> = games
        .iter()
        .map(|g| slint_ui::launcher::ArcadeGame {
            title: g.title.clone().into(),
            mra_path: g.mra_path.clone().into(),
            image_path: g.image_path.clone().into(),
            has_image: g.has_image,
            system_id: g.system_id.clone().into(),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn slint_game_systems(catalog: &ArcadeCatalog) -> ModelRc<slint_ui::launcher::GameSystem> {
    let rows: Vec<slint_ui::launcher::GameSystem> = catalog
        .systems
        .iter()
        .map(|system| slint_ui::launcher::GameSystem {
            id: system.id.clone().into(),
            title: system.title.clone().into(),
            count: catalog.system_preview_game_count(&system.id) as i32,
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn empty_arcade_catalog(root: &str) -> ArcadeCatalog {
    ArcadeCatalog::new(PathBuf::from(root), Vec::new(), Vec::new())
}

fn active_system<'a>(
    catalog: &'a ArcadeCatalog,
    nav: &LauncherNav,
) -> Option<&'a arcade_catalog::GameSystemEntry> {
    catalog.systems.get(nav.selected)
}

fn active_system_game_slice<'a>(
    catalog: &'a ArcadeCatalog,
    nav: &LauncherNav,
) -> &'a [ArcadeGameEntry] {
    active_system(catalog, nav)
        .map(|system| catalog.system_preview_game_slice(&system.id))
        .unwrap_or(&[])
}

fn start_library_catalog_worker(root: String) -> mpsc::Receiver<CatalogWorkerMessage> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("library-catalog".to_string())
        .spawn(move || {
            lower_background_priority();
            let progress_tx = tx.clone();
            let mut progress = move |title: &str, detail: &str| {
                let _ = progress_tx.send(CatalogWorkerMessage::Progress {
                    title: title.to_string(),
                    detail: detail.to_string(),
                });
            };
            let mut cached_catalog_ready = false;
            match library_db::load_arcade_catalog_from_sqlite(&root) {
                Ok(loaded) => {
                    cached_catalog_ready = !loaded.catalog.games.is_empty();
                    let _ = tx.send(CatalogWorkerMessage::Ready {
                        catalog: loaded.catalog,
                        summary: None,
                        load_us: loaded.us,
                    });
                }
                Err(e) => {
                    eprintln!("library catalog cache load failed: {e}");
                    let _ = tx.send(CatalogWorkerMessage::Progress {
                        title: "Indexing library".to_string(),
                        detail: "No cached catalog; scanning library...".to_string(),
                    });
                }
            }
            if cached_catalog_ready && !catalog_refresh_requested() {
                let _ = tx.send(CatalogWorkerMessage::Done);
                return;
            }
            let summary = match library_db::refresh_default_sqlite_database(Some(&mut progress)) {
                Ok(summary) => Some(summary),
                Err(e) => {
                    eprintln!("library refresh failed: {e}");
                    let _ = tx.send(CatalogWorkerMessage::Progress {
                        title: "Library scan failed".to_string(),
                        detail: e,
                    });
                    None
                }
            };
            if let Some(summary) = summary.as_ref().filter(|summary| summary.skipped) {
                if cached_catalog_ready {
                    let _ = tx.send(CatalogWorkerMessage::Unchanged {
                        summary: summary.clone(),
                    });
                    return;
                }
            }
            if summary.is_some() {
                let _ = tx.send(CatalogWorkerMessage::Progress {
                    title: "Loading library".to_string(),
                    detail: "Opening SQLite catalog...".to_string(),
                });
            }
            match library_db::load_arcade_catalog_from_sqlite(&root) {
                Ok(loaded) => {
                    let _ = tx.send(CatalogWorkerMessage::Ready {
                        catalog: loaded.catalog,
                        summary,
                        load_us: loaded.us,
                    });
                }
                Err(e) => {
                    eprintln!("library catalog load failed: {e}");
                    let _ = tx.send(CatalogWorkerMessage::Progress {
                        title: "Library load failed".to_string(),
                        detail: e,
                    });
                }
            }
        })
        .expect("spawn library-catalog");
    rx
}

enum CatalogWorkerMessage {
    Progress {
        title: String,
        detail: String,
    },
    Ready {
        catalog: ArcadeCatalog,
        summary: Option<library_db::LibraryRefreshSummary>,
        load_us: u64,
    },
    Unchanged {
        summary: library_db::LibraryRefreshSummary,
    },
    Done,
}

fn lower_background_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

fn print_startup_event(start: Instant, name: &str, detail: impl std::fmt::Display) {
    let elapsed_ms = start.elapsed().as_millis();
    let detail = detail.to_string();
    boot_analytics::event(name, format!("since_run_ui_ms={elapsed_ms} {detail}"));
    println!("startup_timing\t{name}\t{}ms\t{detail}", elapsed_ms);
}

fn setup_pad_info<'a>(pad: &'a PadPool, setup: &SetupNav) -> &'a PadInfo {
    if setup.is_active() {
        pad.info_at(setup.target_pad_idx)
    } else {
        pad.info()
    }
}

#[derive(PartialEq, Eq)]
struct SetupBridgeKey {
    phase: SetupPhase,
    trigger_status: crate::controller_db::PadRegistryStatus,
    target_pad_idx: usize,
    list_index: usize,
    draft_label: String,
    draft_kind: crate::controller_db::ControllerKind,
}

impl SetupBridgeKey {
    fn from_setup(setup: &SetupNav) -> Self {
        Self {
            phase: setup.phase,
            trigger_status: setup.trigger_status,
            target_pad_idx: setup.target_pad_idx,
            list_index: setup.list_index,
            draft_label: setup.draft_label.clone(),
            draft_kind: setup.draft_kind,
        }
    }
}

#[derive(PartialEq, Eq)]
struct LauncherBridgeKey {
    screen: Screen,
    selected: usize,
    scroll_x: i32,
    settings_focused: bool,
    settings_selected: usize,
    confirm_action: Option<launcher::ConfirmAction>,
    confirm_selected: usize,
    arcade_selected: usize,
}

impl LauncherBridgeKey {
    fn from_nav(nav: &LauncherNav) -> Self {
        Self {
            screen: nav.screen,
            selected: nav.selected,
            scroll_x: nav.scroll_x,
            settings_focused: nav.settings_focused,
            settings_selected: nav.settings_selected,
            confirm_action: nav.confirm_action,
            confirm_selected: nav.confirm_selected,
            arcade_selected: nav.arcade.selected,
        }
    }
}

#[derive(Default)]
struct LauncherBridgeModels {
    game_systems_key: Option<usize>,
    game_systems: Option<ModelRc<slint_ui::launcher::GameSystem>>,
    arcade_games_key: Option<(usize, usize)>,
    arcade_games: Option<ModelRc<slint_ui::launcher::ArcadeGame>>,
}

impl LauncherBridgeModels {
    fn game_systems(
        &mut self,
        catalog: &ArcadeCatalog,
        catalog_version: usize,
    ) -> ModelRc<slint_ui::launcher::GameSystem> {
        if self.game_systems_key != Some(catalog_version) {
            self.game_systems = Some(slint_game_systems(catalog));
            self.game_systems_key = Some(catalog_version);
        }
        self.game_systems
            .as_ref()
            .expect("game system model should be initialized")
            .clone()
    }

    fn arcade_games(
        &mut self,
        catalog: &ArcadeCatalog,
        nav: &LauncherNav,
        catalog_version: usize,
    ) -> ModelRc<slint_ui::launcher::ArcadeGame> {
        let key = (catalog_version, nav.selected);
        if self.arcade_games_key != Some(key) {
            self.arcade_games = Some(slint_arcade_games(active_system_game_slice(catalog, nav)));
            self.arcade_games_key = Some(key);
        }
        self.arcade_games
            .as_ref()
            .expect("arcade game model should be initialized")
            .clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LauncherBenchScenario {
    Idle,
    HomeNav,
    ListScroll,
    SelectedFirst,
    StressScroll,
    CacheWarm,
    QuickTap,
    RapidTaps,
    HeldScroll,
    TurboHold,
    PreviewStepHold,
    ModelSync,
    PreviewChanges,
}

impl LauncherBenchScenario {
    fn from_env() -> Option<Self> {
        match std::env::var("MISTER_LAUNCHER_BENCH_SCENARIO")
            .ok()?
            .to_ascii_lowercase()
            .as_str()
        {
            "idle" => Some(Self::Idle),
            "home-nav" | "home_nav" => Some(Self::HomeNav),
            "list-scroll" | "list_scroll" => Some(Self::ListScroll),
            "selected-first" | "selected_first" => Some(Self::SelectedFirst),
            "velocity-scroll" | "velocity_scroll" | "smooth-scroll" | "smooth_scroll" => {
                Some(Self::HeldScroll)
            }
            "stress-scroll" | "stress_scroll" => Some(Self::StressScroll),
            "cache-warm" | "cache_warm" => Some(Self::CacheWarm),
            "quick-tap" | "quick_tap" => Some(Self::QuickTap),
            "rapid-taps" | "rapid_taps" => Some(Self::RapidTaps),
            "held-scroll" | "held_scroll" => Some(Self::HeldScroll),
            "turbo-hold" | "turbo_hold" => Some(Self::TurboHold),
            "preview-step-hold" | "preview_step_hold" | "step-hold" | "step_hold" => {
                Some(Self::PreviewStepHold)
            }
            "model-sync" | "model_sync" => Some(Self::ModelSync),
            "preview" | "preview-changes" | "preview_changes" => Some(Self::PreviewChanges),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::HomeNav => "home-nav",
            Self::ListScroll => "list-scroll",
            Self::SelectedFirst => "selected-first",
            Self::StressScroll => "stress-scroll",
            Self::CacheWarm => "cache-warm",
            Self::QuickTap => "quick-tap",
            Self::RapidTaps => "rapid-taps",
            Self::HeldScroll => "held-scroll",
            Self::TurboHold => "turbo-hold",
            Self::PreviewStepHold => "preview-step-hold",
            Self::ModelSync => "model-sync",
            Self::PreviewChanges => "preview-changes",
        }
    }

    fn period(self) -> Duration {
        match self {
            Self::Idle => Duration::MAX,
            Self::HomeNav => Duration::from_millis(300),
            Self::ListScroll => Duration::from_millis(120),
            Self::SelectedFirst => Duration::from_millis(700),
            Self::StressScroll => Duration::from_millis(60),
            Self::CacheWarm => Duration::from_millis(120),
            Self::ModelSync => Duration::from_millis(300),
            Self::QuickTap
            | Self::RapidTaps
            | Self::HeldScroll
            | Self::TurboHold
            | Self::PreviewStepHold => Duration::ZERO,
            Self::PreviewChanges => Duration::from_millis(500),
        }
    }
}

fn preview_step_hold_frames() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        let secs = std::env::var("MISTER_PREVIEW_STEP_HOLD_SECS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(5)
            .clamp(1, 60);
        secs.saturating_mul(60).max(1)
    })
}

fn launcher_bench_step(
    scenario: LauncherBenchScenario,
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    active_game_count: Option<usize>,
    step: usize,
    now: Instant,
) -> bool {
    match scenario {
        LauncherBenchScenario::Idle => false,
        LauncherBenchScenario::HomeNav => {
            let count = catalog.systems.len();
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Home;
            nav.settings_focused = false;
            let selected = step % count;
            if selected < nav.selected {
                nav.scroll_x = 0;
            }
            nav.selected = selected;
            keep_bench_home_visible(&mut nav.scroll_x, nav.selected, count);
            true
        }
        LauncherBenchScenario::ModelSync => {
            let count = catalog.systems.len();
            if count == 0 {
                return false;
            }
            let selected = (step / 2) % count;
            if selected < nav.selected {
                nav.scroll_x = 0;
            }
            nav.selected = selected;
            nav.settings_focused = false;
            if step % 2 == 0 {
                nav.screen = Screen::Home;
                keep_bench_home_visible(&mut nav.scroll_x, nav.selected, count);
            } else {
                nav.screen = Screen::Arcade;
                let game_count = catalog.system_preview_game_count(&catalog.systems[selected].id);
                nav.arcade.selected = nav.arcade.selected.min(game_count.saturating_sub(1));
                nav.arcade.snap_to_selected();
                keep_bench_arcade_visible(
                    &mut nav.arcade.scroll_y,
                    nav.arcade.selected,
                    game_count,
                );
            }
            true
        }
        LauncherBenchScenario::ListScroll
        | LauncherBenchScenario::PreviewChanges
        | LauncherBenchScenario::SelectedFirst
        | LauncherBenchScenario::StressScroll
        | LauncherBenchScenario::CacheWarm => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            let selected = match scenario {
                LauncherBenchScenario::SelectedFirst => (step.saturating_mul(7)) % count,
                LauncherBenchScenario::CacheWarm => {
                    let span = count.min(DEFAULT_PREVIEW_CACHE_CAP * 3).max(1);
                    let cycle = span.saturating_mul(2).saturating_sub(2).max(1);
                    let pos = step % cycle;
                    if pos < span {
                        pos
                    } else {
                        cycle - pos
                    }
                }
                _ => step % count,
            };
            if selected < nav.arcade.selected {
                nav.arcade.scroll_y = 0;
            }
            nav.arcade.selected = selected;
            nav.arcade.snap_to_selected();
            keep_bench_arcade_visible(&mut nav.arcade.scroll_y, nav.arcade.selected, count);
            true
        }
        LauncherBenchScenario::HeldScroll => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            let previous_dir = if step == 0 { 0 } else { 1 };
            nav.arcade.bench_direction_tick(1, previous_dir, count, now);
            true
        }
        LauncherBenchScenario::PreviewStepHold => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            if step % preview_step_hold_frames() == 0 {
                nav.arcade.handle_direction_input(1, 0, now, count);
            }
            nav.arcade.tick(count);
            true
        }
        LauncherBenchScenario::QuickTap => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            let (dir, previous_dir) = match step {
                0 => (1, 0),
                1 => (0, 1),
                _ => (0, 0),
            };
            nav.arcade
                .bench_direction_tick(dir, previous_dir, count, now);
            true
        }
        LauncherBenchScenario::RapidTaps => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            let (dir, previous_dir) = if step < 10 {
                if step % 2 == 0 {
                    (1, 0)
                } else {
                    (0, 1)
                }
            } else {
                (0, 0)
            };
            nav.arcade
                .bench_direction_tick(dir, previous_dir, count, now);
            true
        }
        LauncherBenchScenario::TurboHold => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            let (dir, previous_dir) = match step {
                0 => (1, 0),
                1 => (0, 1),
                2 => (1, 0),
                _ => (1, 1),
            };
            nav.arcade
                .bench_direction_tick(dir, previous_dir, count, now);
            true
        }
    }
}

fn launcher_bench_active_game_count(
    catalog: &ArcadeCatalog,
    nav: &LauncherNav,
    active_game_count: Option<usize>,
) -> Option<usize> {
    if let Some(count) = active_game_count {
        return Some(count);
    }
    let system = catalog.systems.get(nav.selected)?;
    Some(catalog.system_preview_game_count(&system.id))
}

fn keep_bench_home_visible(scroll_x: &mut i32, selected: usize, count: usize) {
    let item_w = HOME_TILE_WIDTH + HOME_TILE_GAP;
    let selected_x = selected as i32 * item_w;
    let selected_right = selected_x + HOME_TILE_WIDTH;
    if selected_x < *scroll_x {
        *scroll_x = selected_x;
    }
    if selected_right > *scroll_x + HOME_LIST_VISIBLE_W {
        *scroll_x = selected_right - HOME_LIST_VISIBLE_W;
    }
    let max_scroll = (count as i32 * item_w - HOME_TILE_GAP - HOME_LIST_VISIBLE_W).max(0);
    *scroll_x = (*scroll_x).clamp(0, max_scroll);
}

fn keep_bench_arcade_visible(scroll_y: &mut i32, selected: usize, count: usize) {
    let selected_y = selected as i32 * ARCADE_ROW_HEIGHT;
    let selected_bottom = selected_y + ARCADE_ROW_HEIGHT;
    if selected_y < *scroll_y {
        *scroll_y = selected_y;
    }
    if selected_bottom > *scroll_y + ARCADE_LIST_VISIBLE_H {
        *scroll_y = selected_bottom - ARCADE_LIST_VISIBLE_H;
    }
    let max_scroll = (count as i32 * ARCADE_ROW_HEIGHT - ARCADE_LIST_VISIBLE_H).max(0);
    *scroll_y = (*scroll_y).clamp(0, max_scroll);
}

fn sync_setup_bridge(bridge: &slint_ui::launcher::MisterBridge, pad: &PadPool, setup: &SetupNav) {
    let info = setup_pad_info(pad, setup);
    let db = pad.db();
    let active = setup.phase != SetupPhase::None;
    bridge.set_setup_visible(active);
    bridge.set_setup_phase(setup.phase as i32);
    if active {
        bridge.set_setup_title(setup.title().into());
        bridge.set_setup_selected(setup.list_index as i32);
        let idx = setup.target_pad_idx;
        let js_path = pad.path_at(idx);

        if setup.phase == SetupPhase::Configure {
            let fields = SetupNav::configure_fields(info, js_path, db);
            let labels: Vec<SharedString> = fields.iter().map(|(k, _)| k.clone().into()).collect();
            let values: Vec<SharedString> = fields.iter().map(|(_, v)| v.clone().into()).collect();
            bridge.set_setup_config_labels(ModelRc::new(VecModel::from(labels)));
            bridge.set_setup_config_values(ModelRc::new(VecModel::from(values)));
            bridge.set_setup_list(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            let live = SetupNav::configure_live_hint(pad.state_at(idx));
            bridge.set_setup_subtitle(live.into());
            bridge.set_setup_name(String::new().into());
            bridge.set_setup_kind_label(String::new().into());
        } else if setup.phase == SetupPhase::NameKind {
            bridge.set_setup_subtitle(setup.subtitle(info, db).into());
            bridge.set_setup_name(setup.draft_label.clone().into());
            bridge.set_setup_kind_label(setup.draft_kind_label().into());
            bridge.set_setup_list(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_labels(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_values(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
        } else if setup.phase == SetupPhase::PickExisting {
            bridge.set_setup_subtitle(setup.subtitle(info, db).into());
            bridge
                .set_setup_config_labels(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_values(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            let rows: Vec<SharedString> = db
                .list_entries()
                .iter()
                .map(|item| {
                    let port = if item.last_usb_port.is_empty() {
                        "unknown port".to_string()
                    } else {
                        format!("was {}", item.last_usb_port)
                    };
                    format!("{} — {}", item.label, port).into()
                })
                .collect();
            bridge.set_setup_list(ModelRc::new(VecModel::from(rows)));
        } else {
            bridge.set_setup_subtitle(setup.subtitle(info, db).into());
            bridge.set_setup_list(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_labels(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_values(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge.set_setup_name(String::new().into());
            bridge.set_setup_kind_label(String::new().into());
        }
    }
}

fn sync_bridge_pad_controller(bridge: &slint_ui::controller::MisterBridge, pad: &PadPool) {
    let state = pad.state();
    let info = pad.info();
    bridge.set_dpad_up(state.dpad_up);
    bridge.set_dpad_down(state.dpad_down);
    bridge.set_dpad_left(state.dpad_left);
    bridge.set_dpad_right(state.dpad_right);
    bridge.set_btn_a(state.btn_a);
    bridge.set_btn_b(state.btn_b);
    bridge.set_btn_x(state.btn_x);
    bridge.set_btn_y(state.btn_y);
    bridge.set_btn_l(state.btn_l);
    bridge.set_btn_r(state.btn_r);
    bridge.set_btn_zl(state.btn_zl);
    bridge.set_btn_zr(state.btn_zr);
    bridge.set_btn_select(state.btn_select);
    bridge.set_btn_start(state.btn_start);
    bridge.set_btn_l3(state.btn_l3);
    bridge.set_btn_r3(state.btn_r3);
    bridge.set_btn_home(state.btn_home);
    bridge.set_btn_capture(state.btn_capture);
    bridge.set_capture_available(info.capture_available);
    bridge.set_left_x(state.left_x);
    bridge.set_left_y(state.left_y);
    bridge.set_right_x(state.right_x);
    bridge.set_right_y(state.right_y);
    sync_device_info_controller(bridge, info, pad.db(), pad.path(), pad.len());
    bridge.set_pressed_now(state.pressed_now.clone().into());
    bridge.set_last_event_label(state.last_event_label.clone().into());
    bridge.set_last_raw_event(state.last_raw.clone().into());
}

fn sync_bridge_pad_launcher(bridge: &slint_ui::launcher::MisterBridge, pad: &PadPool) {
    let state = pad.state();
    let info = pad.info();
    bridge.set_dpad_up(state.dpad_up);
    bridge.set_dpad_down(state.dpad_down);
    bridge.set_dpad_left(state.dpad_left);
    bridge.set_dpad_right(state.dpad_right);
    bridge.set_btn_a(state.btn_a);
    bridge.set_btn_b(state.btn_b);
    bridge.set_btn_x(state.btn_x);
    bridge.set_btn_y(state.btn_y);
    bridge.set_btn_l(state.btn_l);
    bridge.set_btn_r(state.btn_r);
    bridge.set_btn_zl(state.btn_zl);
    bridge.set_btn_zr(state.btn_zr);
    bridge.set_btn_select(state.btn_select);
    bridge.set_btn_start(state.btn_start);
    bridge.set_btn_l3(state.btn_l3);
    bridge.set_btn_r3(state.btn_r3);
    bridge.set_btn_home(state.btn_home);
    bridge.set_btn_capture(state.btn_capture);
    bridge.set_capture_available(info.capture_available);
    bridge.set_left_x(state.left_x);
    bridge.set_left_y(state.left_y);
    bridge.set_right_x(state.right_x);
    bridge.set_right_y(state.right_y);
    sync_device_info_launcher(bridge, info, pad.db(), pad.path(), pad.len());
    bridge.set_pressed_now(state.pressed_now.clone().into());
    bridge.set_last_event_label(state.last_event_label.clone().into());
    bridge.set_last_raw_event(state.last_raw.clone().into());
}

fn sync_device_info_controller(
    bridge: &slint_ui::controller::MisterBridge,
    info: &PadInfo,
    db: &ControllerDb,
    js_path: &str,
    pad_count: usize,
) {
    let label = if pad_count > 1 {
        format!("{js_path} ({pad_count} pads)")
    } else {
        js_path.to_string()
    };
    bridge.set_device_label(label.into());
    bridge.set_device_name(db.display_label(info).into());
    bridge.set_usb_port(info.usb_port.clone().into());
    bridge.set_usb_id(format!("{}:{}", info.vendor_id, info.product_id).into());
    bridge.set_serial_id(if info.serial.is_empty() {
        "(no serial)".into()
    } else {
        info.serial.clone().into()
    });
    bridge.set_js_counts(
        format!(
            "js API: {} buttons, {} axes · evdev: {} keys, {} abs axes",
            info.js_buttons, info.js_axes, info.evdev_key_count, info.evdev_abs_count
        )
        .into(),
    );
}

fn sync_device_info_launcher(
    bridge: &slint_ui::launcher::MisterBridge,
    info: &PadInfo,
    db: &ControllerDb,
    js_path: &str,
    pad_count: usize,
) {
    let label = if pad_count > 1 {
        format!("{js_path} ({pad_count} pads)")
    } else {
        js_path.to_string()
    };
    bridge.set_device_label(label.into());
    bridge.set_device_name(db.display_label(info).into());
    bridge.set_usb_port(info.usb_port.clone().into());
    bridge.set_usb_id(format!("{}:{}", info.vendor_id, info.product_id).into());
    bridge.set_serial_id(if info.serial.is_empty() {
        "(no serial)".into()
    } else {
        info.serial.clone().into()
    });
    bridge.set_js_counts(
        format!(
            "js API: {} buttons, {} axes · evdev: {} keys, {} abs axes",
            info.js_buttons, info.js_axes, info.evdev_key_count, info.evdev_abs_count
        )
        .into(),
    );
}

fn run_bench_frame(
    ui: &UiDisplay,
    disp: &mut Display,
    f: &mut Fpga,
    target: &mut UiFrameTarget,
    window: &Rc<MinimalSoftwareWindow>,
    frame_order: FrameOrder,
    animation_clock: &AnimationClock,
    pacer: &mut VsyncPacer,
) -> FrameSample {
    let frame_start = Instant::now();
    let t0 = Instant::now();
    let mut this_rect: Option<DirtyRect> = None;

    match frame_order {
        FrameOrder::RenderThenVsync => {
            update_slint_animations(animation_clock);
            let t1 = Instant::now();
            window.draw_if_needed(|renderer| {
                let region = target.render(renderer, ui);
                this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
            });
            let t2 = Instant::now();
            let pace = pacer.wait();
            let t3 = Instant::now();
            let mut copy_us = 0;
            let mut present_rect = None;
            let rows = match this_rect {
                Some(rect) => {
                    let c0 = Instant::now();
                    let rows = target.present_rect(f, disp, ui, rect);
                    copy_us += c0.elapsed().as_micros() as u64;
                    present_rect = Some(frame_rect(rect));
                    rows
                }
                None => 0,
            };
            FrameSample {
                prepare_us: 0,
                anim_us: (t1 - t0).as_micros() as u64,
                slint_render_us: (t2 - t1).as_micros() as u64,
                custom_draw_us: 0,
                vsync_us: (t3 - t2).as_micros() as u64,
                fb_present_us: copy_us,
                cached_present_us: copy_us,
                overlay_present_us: 0,
                rows,
                present_rect,
                wall_us: frame_start.elapsed().as_micros() as u64,
                vsync_source: pace.source,
                vsync_period_us: pace.period_us,
                vsync_miss_streak: pace.miss_streak,
            }
        }
        FrameOrder::VsyncThenRender => {
            let pace = pacer.wait();
            let t1 = Instant::now();
            update_slint_animations(animation_clock);
            let t2 = Instant::now();
            window.draw_if_needed(|renderer| {
                let region = target.render(renderer, ui);
                this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
            });
            let t3 = Instant::now();
            let mut copy_us = 0;
            let mut present_rect = None;
            let rows = match this_rect {
                Some(rect) => {
                    let c0 = Instant::now();
                    let rows = target.present_rect(f, disp, ui, rect);
                    copy_us += c0.elapsed().as_micros() as u64;
                    present_rect = Some(frame_rect(rect));
                    rows
                }
                None => 0,
            };
            FrameSample {
                prepare_us: 0,
                anim_us: (t2 - t1).as_micros() as u64,
                slint_render_us: (t3 - t2).as_micros() as u64,
                custom_draw_us: 0,
                vsync_us: (t1 - t0).as_micros() as u64,
                fb_present_us: copy_us,
                cached_present_us: copy_us,
                overlay_present_us: 0,
                rows,
                present_rect,
                wall_us: frame_start.elapsed().as_micros() as u64,
                vsync_source: pace.source,
                vsync_period_us: pace.period_us,
                vsync_miss_streak: pace.miss_streak,
            }
        }
    }
}

fn run_frame_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    f: &mut Fpga,
    window: &Rc<MinimalSoftwareWindow>,
    target: &mut UiFrameTarget,
    animation_clock: &AnimationClock,
) {
    let start = Instant::now();
    let mut frames = 0u64;
    let mut profiler = FrameProfiler::from_env();
    let cpu = cpu_profile::start();
    let profile_on = profiler.enabled();

    // Legacy 1 Hz line (no anim column) when frame profiler is off — keeps bench-toolchain.sh parsing stable.
    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut render_us = 0u128;
    let mut vsync_us = 0u128;
    let mut copy_us = 0u128;
    let mut copy_rows_acc = 0u128;
    let configured_frame_order = FrameOrder::from_env();
    let frame_order = configured_frame_order;
    let mut pacer = VsyncPacer::from_env();

    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "bench scene running {label} (vsync-locked, dirty-row copy, frame-order={}, render-mode={}, animation-clock={})...",
        frame_order.label(),
        target.label(),
        animation_clock.label()
    );
    println!(
        "slint-render-mode={} frame-order={} requested-frame-order={}",
        target.label(),
        frame_order.label(),
        configured_frame_order.label()
    );
    while secs == 0 || start.elapsed().as_secs() < secs {
        let sample = run_bench_frame(
            ui,
            disp,
            f,
            target,
            window,
            frame_order,
            animation_clock,
            &mut pacer,
        );
        frames += 1;

        if profiler.enabled() {
            profiler.record(sample);
        } else {
            fps_frames += 1;
            render_us += sample.slint_render_us as u128;
            vsync_us += sample.vsync_us as u128;
            copy_us += sample.fb_present_us as u128;
            copy_rows_acc += sample.rows as u128;
            if fps_window_start.elapsed().as_millis() >= 1000 {
                let nn = fps_frames.max(1) as u128;
                println!(
                    "  fps ~ {fps_frames}  | slint-render {}us  vsync-wait {}us  fb-present {}us ({} logical rows avg)  vsync hits={} timeouts={} fallback={} errors={} hz={:.2}",
                    render_us / nn,
                    vsync_us / nn,
                    copy_us / nn,
                    copy_rows_acc / nn,
                    pacer.hits(),
                    pacer.timeouts(),
                    pacer.fallback_frames(),
                    pacer.errors(),
                    1_000_000.0 / pacer.period_us() as f64
                );
                fps_frames = 0;
                render_us = 0;
                vsync_us = 0;
                copy_us = 0;
                copy_rows_acc = 0;
                fps_window_start = Instant::now();
            }
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if profile_on {
        profiler.finish();
    }
    if let Err(e) = cpu_profile::finish(cpu) {
        eprintln!("{e}");
    }
}

#[cfg(all(feature = "video", mister_bench_scenes))]
const VIDEO_IMAGE_RECT: DirtyRect = DirtyRect {
    x0: 40,
    y0: 158,
    x1: 360,
    y1: 382,
};

#[cfg(all(feature = "video", mister_bench_scenes))]
#[derive(Default)]
struct VideoFramePhases {
    frame_updated: bool,
    decode_us: u64,
    recv_us: u64,
    image_us: u64,
    blit_us: u64,
    audio_us: u64,
}

#[cfg(all(feature = "video", mister_bench_scenes))]
#[derive(Default)]
struct VideoWindowTotals {
    frames: u64,
    video_frames: u64,
    decode_us: u128,
    recv_us: u128,
    image_us: u128,
    blit_us: u128,
    audio_us: u128,
    render_us: u128,
    vsync_us: u128,
    copy_us: u128,
    copy_rows: u128,
    copy_px: u128,
}

#[cfg(all(feature = "video", mister_bench_scenes))]
impl VideoWindowTotals {
    fn record(
        &mut self,
        phases: VideoFramePhases,
        sample: FrameSample,
        copy_rect: Option<DirtyRect>,
    ) {
        self.frames += 1;
        if phases.frame_updated {
            self.video_frames += 1;
        }
        self.decode_us += phases.decode_us as u128;
        self.recv_us += phases.recv_us as u128;
        self.image_us += phases.image_us as u128;
        self.blit_us += phases.blit_us as u128;
        self.audio_us += phases.audio_us as u128;
        self.render_us += sample.slint_render_us as u128;
        self.vsync_us += sample.vsync_us as u128;
        self.copy_us += sample.fb_present_us as u128;
        self.copy_rows += sample.rows as u128;
        if let Some(rect) = copy_rect {
            self.copy_px += rect.width() as u128 * rect.rows() as u128;
        }
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn avg_per_frame(value: u128, frames: u64) -> u128 {
        if frames == 0 {
            0
        } else {
            value / frames as u128
        }
    }

    fn avg_per_video_frame(value: u128, video_frames: u64) -> u128 {
        if video_frames == 0 {
            0
        } else {
            value / video_frames as u128
        }
    }
}

#[cfg(all(feature = "video", mister_bench_scenes))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoRenderMode {
    SlintImage,
    DirectBlit,
}

#[cfg(all(feature = "video", mister_bench_scenes))]
impl VideoRenderMode {
    fn from_env() -> Self {
        match std::env::var("MISTER_VIDEO_RENDER_MODE")
            .unwrap_or_else(|_| "slint-image".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "direct" | "direct-blit" | "direct_blit" => Self::DirectBlit,
            _ => Self::SlintImage,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::SlintImage => "slint-image",
            Self::DirectBlit => "direct-blit",
        }
    }
}

#[cfg(all(feature = "video", mister_bench_scenes))]
fn video_copy_rect(
    dirty: DirtyRect,
    video_dirty_clip_ready: bool,
    frame_updated: bool,
) -> DirtyRect {
    if video_dirty_clip_ready && frame_updated {
        dirty.intersection(VIDEO_IMAGE_RECT).unwrap_or(dirty)
    } else {
        dirty
    }
}

#[cfg(all(feature = "video", mister_bench_scenes))]
fn direct_video_copy_rect(dirty: Option<DirtyRect>, video_dirty_clip_ready: bool) -> DirtyRect {
    let Some(dirty) = dirty else {
        return VIDEO_IMAGE_RECT;
    };
    if !video_dirty_clip_ready {
        return dirty.union(VIDEO_IMAGE_RECT);
    }
    dirty
        .intersection(VIDEO_IMAGE_RECT)
        .unwrap_or(dirty.union(VIDEO_IMAGE_RECT))
}

#[cfg(all(feature = "video", mister_bench_scenes))]
fn blit_video_frame_to_cached(
    frame: &SharedPixelBuffer<Rgb8Pixel>,
    cached: &mut [Pixel],
    render_w: usize,
) {
    let src_w = frame.width() as usize;
    let src_h = frame.height() as usize;
    let bytes = frame.as_bytes();
    let dst_x = VIDEO_IMAGE_RECT.x0;
    let dst_y = VIDEO_IMAGE_RECT.y0;
    for y in 0..src_h {
        let src = &bytes[y * src_w * 3..(y + 1) * src_w * 3];
        let dst =
            &mut cached[(dst_y + y) * render_w + dst_x..(dst_y + y) * render_w + dst_x + src_w];
        unsafe {
            let mut src = src.as_ptr();
            let mut dst = dst.as_mut_ptr();
            for _ in 0..src_w {
                let r = *src as u32;
                let g = *src.add(1) as u32;
                let b = *src.add(2) as u32;
                dst.write(Pixel((r << 16) | (g << 8) | b));
                src = src.add(3);
                dst = dst.add(1);
            }
        }
    }
}

#[cfg(all(feature = "video", mister_bench_scenes))]
fn run_video_playback_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    window: &Rc<MinimalSoftwareWindow>,
    app: slint_ui::video_playback::VideoPlayback,
    animation_clock: &AnimationClock,
) {
    let path = std::env::var("MISTER_VIDEO_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::video_player::DEFAULT_VIDEO_PATH.to_string());
    let frame_worker = match crate::video_player::VideoFrameWorker::start(path.clone()) {
        Ok(worker) => worker,
        Err(e) => {
            eprintln!("video_playback: {e}");
            std::process::exit(1);
        }
    };
    let mut audio_sink = match crate::mr_audio::MrAudioSink::open_default() {
        Ok(sink) => sink,
        Err(e) => {
            eprintln!("video_playback audio: {e}");
            std::process::exit(1);
        }
    };

    let mut cached = vec![Pixel(0); ui.render_w() * ui.render_h()];
    let start = Instant::now();
    let mut next_video_at = Duration::ZERO;
    let frame_interval = frame_worker.frame_interval();
    let mut frames = 0u64;
    let mut profiler = FrameProfiler::from_env();
    let cpu = cpu_profile::start();
    let profile_on = profiler.enabled();
    let frame_order = FrameOrder::from_env();
    let render_mode = VideoRenderMode::from_env();
    let mut pacer = VsyncPacer::from_env();

    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut video_totals = VideoWindowTotals::default();
    let mut audio_stats = AudioWindowStats::default();
    let mut video_dirty_clip_ready = false;

    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "video_playback running {label} path={path} frame-order={} animation-clock={} video-render-mode={}",
        frame_order.label(),
        animation_clock.label(),
        render_mode.label()
    );
    println!("video_render_mode={}", render_mode.label());
    println!(
        "video_dirty_clip=on rect={}x{}+{},{}",
        VIDEO_IMAGE_RECT.width(),
        VIDEO_IMAGE_RECT.rows(),
        VIDEO_IMAGE_RECT.x0,
        VIDEO_IMAGE_RECT.y0
    );

    while secs == 0 || start.elapsed().as_secs() < secs {
        let frame_start = Instant::now();
        let t0 = Instant::now();
        let mut this_rect: Option<DirtyRect> = None;
        let mut phases = VideoFramePhases::default();
        let mut direct_frame: Option<SharedPixelBuffer<Rgb8Pixel>> = None;

        match frame_order {
            FrameOrder::RenderThenVsync => {
                update_slint_animations(animation_clock);
                let now = start.elapsed();
                if now >= next_video_at {
                    let recv_t0 = Instant::now();
                    match frame_worker.try_recv() {
                        Ok(Some(frame)) => {
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                            let crate::video_player::PlaybackFrame {
                                pixel_buffer,
                                audio,
                                audio_requested_frames,
                                loop_count,
                                decode_us,
                            } = frame;
                            phases.frame_updated = true;
                            phases.decode_us = decode_us;
                            match render_mode {
                                VideoRenderMode::SlintImage => {
                                    let image_t0 = Instant::now();
                                    app.set_frame(slint::Image::from_rgb8(pixel_buffer));
                                    phases.image_us = image_t0.elapsed().as_micros() as u64;
                                    window.request_redraw();
                                }
                                VideoRenderMode::DirectBlit => {
                                    direct_frame = Some(pixel_buffer);
                                }
                            }
                            let audio_t0 = Instant::now();
                            match audio_sink.write_frames(&audio) {
                                Ok(written) => {
                                    phases.audio_us = audio_t0.elapsed().as_micros() as u64;
                                    audio_stats.add(
                                        Duration::from_micros(phases.audio_us),
                                        audio_requested_frames,
                                        written,
                                        loop_count,
                                    );
                                }
                                Err(e) => {
                                    eprintln!("video_playback audio: {e}");
                                    break;
                                }
                            }
                            frame_worker.recycle_audio(audio);
                        }
                        Ok(None) => {
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                        }
                        Err(e) => {
                            eprintln!("video_playback: {e}");
                            break;
                        }
                    }
                    next_video_at += frame_interval;
                    while next_video_at < now {
                        next_video_at += frame_interval;
                    }
                }
                let t1 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let region = renderer.render(&mut cached, ui.render_w());
                    this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                });
                let t2 = Instant::now();
                if let Some(frame) = direct_frame.as_ref() {
                    let blit_t0 = Instant::now();
                    blit_video_frame_to_cached(frame, &mut cached, ui.render_w());
                    phases.blit_us = blit_t0.elapsed().as_micros() as u64;
                }
                let pace = pacer.wait();
                let t3 = Instant::now();
                let mut copied_rect = None;
                let rows = if direct_frame.is_some() {
                    let rect = direct_video_copy_rect(this_rect, video_dirty_clip_ready);
                    copy_cached_rect(disp, ui, &cached, rect);
                    copied_rect = Some(rect);
                    rect.rows()
                } else if let Some(rect) = this_rect {
                    let rect = video_copy_rect(rect, video_dirty_clip_ready, phases.frame_updated);
                    copy_cached_rect(disp, ui, &cached, rect);
                    copied_rect = Some(rect);
                    rect.rows()
                } else {
                    0
                };
                if rows > 0 {
                    video_dirty_clip_ready = true;
                }
                let t4 = Instant::now();
                let sample = FrameSample {
                    prepare_us: 0,
                    anim_us: (t1 - t0).as_micros() as u64,
                    slint_render_us: (t2 - t1).as_micros() as u64,
                    custom_draw_us: 0,
                    vsync_us: (t3 - t2).as_micros() as u64,
                    fb_present_us: (t4 - t3).as_micros() as u64,
                    cached_present_us: (t4 - t3).as_micros() as u64,
                    overlay_present_us: 0,
                    rows,
                    present_rect: copied_rect.map(frame_rect),
                    wall_us: frame_start.elapsed().as_micros() as u64,
                    vsync_source: pace.source,
                    vsync_period_us: pace.period_us,
                    vsync_miss_streak: pace.miss_streak,
                };
                record_video_sample(
                    phases,
                    sample,
                    copied_rect,
                    &mut profiler,
                    &mut fps_window_start,
                    &mut fps_frames,
                    &mut video_totals,
                    &mut audio_stats,
                );
            }
            FrameOrder::VsyncThenRender => {
                let pace = pacer.wait();
                let t1 = Instant::now();
                update_slint_animations(animation_clock);
                let now = start.elapsed();
                if now >= next_video_at {
                    let recv_t0 = Instant::now();
                    match frame_worker.try_recv() {
                        Ok(Some(frame)) => {
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                            let crate::video_player::PlaybackFrame {
                                pixel_buffer,
                                audio,
                                audio_requested_frames,
                                loop_count,
                                decode_us,
                            } = frame;
                            phases.frame_updated = true;
                            phases.decode_us = decode_us;
                            match render_mode {
                                VideoRenderMode::SlintImage => {
                                    let image_t0 = Instant::now();
                                    app.set_frame(slint::Image::from_rgb8(pixel_buffer));
                                    phases.image_us = image_t0.elapsed().as_micros() as u64;
                                    window.request_redraw();
                                }
                                VideoRenderMode::DirectBlit => {
                                    direct_frame = Some(pixel_buffer);
                                }
                            }
                            let audio_t0 = Instant::now();
                            match audio_sink.write_frames(&audio) {
                                Ok(written) => {
                                    phases.audio_us = audio_t0.elapsed().as_micros() as u64;
                                    audio_stats.add(
                                        Duration::from_micros(phases.audio_us),
                                        audio_requested_frames,
                                        written,
                                        loop_count,
                                    );
                                }
                                Err(e) => {
                                    eprintln!("video_playback audio: {e}");
                                    break;
                                }
                            }
                            frame_worker.recycle_audio(audio);
                        }
                        Ok(None) => {
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                        }
                        Err(e) => {
                            eprintln!("video_playback: {e}");
                            break;
                        }
                    }
                    next_video_at += frame_interval;
                    while next_video_at < now {
                        next_video_at += frame_interval;
                    }
                }
                let t2 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let region = renderer.render(&mut cached, ui.render_w());
                    this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                });
                let t3 = Instant::now();
                if let Some(frame) = direct_frame.as_ref() {
                    let blit_t0 = Instant::now();
                    blit_video_frame_to_cached(frame, &mut cached, ui.render_w());
                    phases.blit_us = blit_t0.elapsed().as_micros() as u64;
                }
                let mut copied_rect = None;
                let rows = if direct_frame.is_some() {
                    let rect = direct_video_copy_rect(this_rect, video_dirty_clip_ready);
                    copy_cached_rect(disp, ui, &cached, rect);
                    copied_rect = Some(rect);
                    rect.rows()
                } else if let Some(rect) = this_rect {
                    let rect = video_copy_rect(rect, video_dirty_clip_ready, phases.frame_updated);
                    copy_cached_rect(disp, ui, &cached, rect);
                    copied_rect = Some(rect);
                    rect.rows()
                } else {
                    0
                };
                if rows > 0 {
                    video_dirty_clip_ready = true;
                }
                let t4 = Instant::now();
                let sample = FrameSample {
                    prepare_us: 0,
                    anim_us: (t2 - t1).as_micros() as u64,
                    slint_render_us: (t3 - t2).as_micros() as u64,
                    custom_draw_us: 0,
                    vsync_us: (t1 - t0).as_micros() as u64,
                    fb_present_us: (t4 - t3).as_micros() as u64,
                    cached_present_us: (t4 - t3).as_micros() as u64,
                    overlay_present_us: 0,
                    rows,
                    present_rect: copied_rect.map(frame_rect),
                    wall_us: frame_start.elapsed().as_micros() as u64,
                    vsync_source: pace.source,
                    vsync_period_us: pace.period_us,
                    vsync_miss_streak: pace.miss_streak,
                };
                record_video_sample(
                    phases,
                    sample,
                    copied_rect,
                    &mut profiler,
                    &mut fps_window_start,
                    &mut fps_frames,
                    &mut video_totals,
                    &mut audio_stats,
                );
            }
        }
        frames += 1;
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Ok(status) = crate::mr_audio::read_status() {
        print!("video_playback audio status: {status}");
    }
    if profile_on {
        profiler.finish();
    }
    if let Err(e) = cpu_profile::finish(cpu) {
        eprintln!("{e}");
    }
}

#[cfg(all(feature = "video", mister_bench_scenes))]
#[derive(Default)]
struct AudioWindowStats {
    write_us: u128,
    requested_frames: u128,
    written_frames: u128,
    underruns: u64,
    loop_count: u64,
}

#[cfg(all(feature = "video", mister_bench_scenes))]
impl AudioWindowStats {
    fn add(
        &mut self,
        write_elapsed: Duration,
        requested_frames: usize,
        written_frames: usize,
        loop_count: u64,
    ) {
        self.write_us += write_elapsed.as_micros();
        self.requested_frames += requested_frames as u128;
        self.written_frames += written_frames as u128;
        if written_frames < requested_frames {
            self.underruns += 1;
        }
        self.loop_count = self.loop_count.max(loop_count);
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(all(feature = "video", mister_bench_scenes))]
#[allow(clippy::too_many_arguments)]
fn record_video_sample(
    phases: VideoFramePhases,
    sample: FrameSample,
    copy_rect: Option<DirtyRect>,
    profiler: &mut FrameProfiler,
    fps_window_start: &mut Instant,
    fps_frames: &mut u64,
    totals: &mut VideoWindowTotals,
    audio_stats: &mut AudioWindowStats,
) {
    if profiler.enabled() {
        profiler.record(sample);
        return;
    }

    *fps_frames += 1;
    totals.record(phases, sample, copy_rect);
    if fps_window_start.elapsed().as_millis() >= 1000 {
        let video_nn = totals.video_frames.max(1);
        println!(
            "  fps ~ {}  | video-frames {} recv {}us decode-worker {}us/frame image-update {}us/frame blit {}us/frame slint-render {}us vsync-wait {}us fb-present {}us ({} logical rows avg, {} px avg) audio-write {}us/frame audio {}/{}f underruns {} loops {}",
            *fps_frames,
            totals.video_frames,
            VideoWindowTotals::avg_per_frame(totals.recv_us, *fps_frames),
            VideoWindowTotals::avg_per_video_frame(totals.decode_us, video_nn),
            VideoWindowTotals::avg_per_video_frame(totals.image_us, video_nn),
            VideoWindowTotals::avg_per_video_frame(totals.blit_us, video_nn),
            VideoWindowTotals::avg_per_frame(totals.render_us, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.vsync_us, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.copy_us, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.copy_rows, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.copy_px, *fps_frames),
            VideoWindowTotals::avg_per_video_frame(audio_stats.write_us, video_nn),
            audio_stats.written_frames,
            audio_stats.requested_frames,
            audio_stats.underruns,
            audio_stats.loop_count
        );
        *fps_frames = 0;
        totals.reset();
        audio_stats.reset();
        *fps_window_start = Instant::now();
    }
}

#[cfg(mister_bench_scenes)]
const CONSOLE_LIST_X: usize = 40;
#[cfg(mister_bench_scenes)]
const CONSOLE_LIST_Y: usize = 116;
#[cfg(mister_bench_scenes)]
const CONSOLE_LIST_W: usize = 880;
#[cfg(mister_bench_scenes)]
const CONSOLE_LIST_H: usize = 356;
#[cfg(mister_bench_scenes)]
const CONSOLE_ROW_H: usize = 44;
#[cfg(mister_bench_scenes)]
const CONSOLE_FONT_PX: f32 = 16.0;
#[cfg(mister_bench_scenes)]
const CONSOLE_TRACE_DEFAULT_PATH: &str = "/tmp/mister-magik-console-scroll-trace.tsv";

#[cfg(mister_bench_scenes)]
struct ConsoleScrollTrace {
    file: File,
    start: Instant,
    frame: u64,
    fb_sample_step: usize,
    copy_budget_us: u64,
}

#[cfg(mister_bench_scenes)]
struct ConsoleScrollTraceSample {
    virtual_y: usize,
    slint_us: u64,
    ram_scroll_us: u64,
    strip_us: u64,
    vsync_wait_us: u64,
    fb_copy_us: u64,
    label_copy_us: u64,
    frame_wall_us: u64,
    copy_done_after_vsync_us: u64,
    fb_hash_us: u64,
    fb_hash: u64,
    fb_nonzero: u32,
}

#[cfg(mister_bench_scenes)]
impl ConsoleScrollTrace {
    fn open(display_h: usize, list_y: usize) -> Option<Self> {
        let path = std::env::var("MISTER_CONSOLE_SCROLL_TRACE_FILE").ok()?;
        let path = if path.is_empty() {
            CONSOLE_TRACE_DEFAULT_PATH.to_string()
        } else {
            path
        };
        let mut file = File::create(&path).ok()?;
        let _ = writeln!(
            file,
            "frame\telapsed_ms\tvirtual_y\tslint_us\tram_scroll_us\tstrip_us\tvsync_wait_us\tfb_copy_us\tlabel_copy_us\tframe_wall_us\tcopy_done_after_vsync_us\tcopy_budget_us\tfb_hash_us\tfb_hash\tfb_nonzero"
        );
        let fb_sample_step = std::env::var("MISTER_CONSOLE_SCROLL_TRACE_STEP")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(32);
        let copy_budget_us = if display_h == 0 {
            0
        } else {
            ((list_y as u64) * 16_667) / (display_h as u64)
        };
        println!(
            "console_scroll trace: path={path} fb_sample_step={fb_sample_step} copy_budget_us={copy_budget_us}"
        );
        Some(Self {
            file,
            start: Instant::now(),
            frame: 0,
            fb_sample_step,
            copy_budget_us,
        })
    }

    fn record(&mut self, sample: ConsoleScrollTraceSample) {
        let elapsed_ms = self.start.elapsed().as_millis();
        let _ = writeln!(
            self.file,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:016x}\t{}",
            self.frame,
            elapsed_ms,
            sample.virtual_y,
            sample.slint_us,
            sample.ram_scroll_us,
            sample.strip_us,
            sample.vsync_wait_us,
            sample.fb_copy_us,
            sample.label_copy_us,
            sample.frame_wall_us,
            sample.copy_done_after_vsync_us,
            self.copy_budget_us,
            sample.fb_hash_us,
            sample.fb_hash,
            sample.fb_nonzero
        );
        self.frame += 1;
    }
}

#[cfg(mister_bench_scenes)]
fn run_console_scroll_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    window: &Rc<MinimalSoftwareWindow>,
    app: slint_ui::console_scroll::ConsoleScroll,
    animation_clock: &AnimationClock,
) {
    let mut cached = vec![Pixel(0); ui.render_w() * ui.render_h()];
    let scale = ui.fb_scale();
    let fb_x = CONSOLE_LIST_X * scale;
    let fb_y = CONSOLE_LIST_Y * scale;
    let scroll_px = 2usize;
    let mut surface = vec![Pixel(0); CONSOLE_LIST_W * CONSOLE_LIST_H];
    let mut surface_y = 0usize;
    let mut font = ConsoleFont::new(CONSOLE_FONT_PX);
    let mut trace = ConsoleScrollTrace::open(disp.height(), fb_y);
    let mut pacer = VsyncPacer::from_env();
    let cpu = cpu_profile::start();

    window.request_redraw();
    update_slint_animations(animation_clock);
    window.draw_if_needed(|renderer| {
        let _ = renderer.render(&mut cached, ui.render_w());
    });
    copy_cached_rows(disp, ui, &cached, 0, ui.render_h());
    draw_console_virtual_strip(
        &mut surface,
        CONSOLE_LIST_W,
        CONSOLE_LIST_W,
        CONSOLE_LIST_H,
        0,
        0,
        &mut font,
    );
    copy_console_surface(disp, fb_x, fb_y, scale, &surface, surface_y);

    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!("console_scroll running {label} — fb scroll-copy + exposed-strip redraw");

    let start = Instant::now();
    let mut frames = 0u64;
    let mut virtual_y = 0usize;
    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut ram_scroll_us = 0u128;
    let mut strip_us = 0u128;
    let mut fb_copy_us = 0u128;
    let mut label_rect: Option<DirtyRect> = None;

    while secs == 0 || start.elapsed().as_secs() < secs {
        let frame_start = Instant::now();
        if fps_window_start.elapsed().as_millis() >= 1000 {
            let nn = fps_frames.max(1) as u128;
            let top_row = (virtual_y / CONSOLE_ROW_H) % 1000;
            app.set_fps_label(format!("fps {fps_frames}").into());
            app.set_blit_label(format!("ram scroll {}us", ram_scroll_us / nn).into());
            app.set_strip_label(format!("new strip {}us", strip_us / nn).into());
            app.set_row_label(format!("top row {top_row:03}").into());
            window.request_redraw();
            println!(
                "  fps ~ {fps_frames}  | ram-scroll {}us  exposed-strip {}us  fb-copy {}us  top-row {top_row}  vsync hits={} timeouts={} fallback={} errors={} hz={:.2}",
                ram_scroll_us / nn,
                strip_us / nn,
                fb_copy_us / nn,
                pacer.hits(),
                pacer.timeouts(),
                pacer.fallback_frames(),
                pacer.errors(),
                1_000_000.0 / pacer.period_us() as f64
            );
            fps_frames = 0;
            ram_scroll_us = 0;
            strip_us = 0;
            fb_copy_us = 0;
            fps_window_start = Instant::now();
        }

        update_slint_animations(animation_clock);
        window.draw_if_needed(|renderer| {
            let region = renderer.render(&mut cached, ui.render_w());
            label_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
        });
        let t_slint_done = Instant::now();

        let t0 = Instant::now();
        surface_y = (surface_y + scroll_px) % CONSOLE_LIST_H;
        let t1 = Instant::now();
        virtual_y = virtual_y.wrapping_add(scroll_px);
        draw_console_virtual_strip_wrapped(
            &mut surface,
            CONSOLE_LIST_W,
            (surface_y + CONSOLE_LIST_H - scroll_px) % CONSOLE_LIST_H,
            scroll_px,
            virtual_y + CONSOLE_LIST_H - scroll_px,
            &mut font,
        );
        let t2 = Instant::now();

        let t_wait_start = Instant::now();
        let _pace = pacer.wait();
        let t3 = Instant::now();
        copy_console_surface(disp, fb_x, fb_y, scale, &surface, surface_y);
        let t4 = Instant::now();
        if let Some(rect) = label_rect.take() {
            copy_cached_rect(disp, ui, &cached, rect);
        }
        let t5 = Instant::now();
        if let Some(trace) = trace.as_mut() {
            let hash_start = Instant::now();
            let (fb_hash, fb_nonzero) = disp.rect_sampled_signature(
                fb_x,
                fb_y,
                CONSOLE_LIST_W * scale,
                CONSOLE_LIST_H * scale,
                trace.fb_sample_step,
            );
            let hash_end = Instant::now();
            trace.record(ConsoleScrollTraceSample {
                virtual_y,
                slint_us: (t_slint_done - frame_start).as_micros() as u64,
                ram_scroll_us: (t1 - t0).as_micros() as u64,
                strip_us: (t2 - t1).as_micros() as u64,
                vsync_wait_us: (t3 - t_wait_start).as_micros() as u64,
                fb_copy_us: (t4 - t3).as_micros() as u64,
                label_copy_us: (t5 - t4).as_micros() as u64,
                frame_wall_us: (t5 - frame_start).as_micros() as u64,
                copy_done_after_vsync_us: (t4 - t3).as_micros() as u64,
                fb_hash_us: (hash_end - hash_start).as_micros() as u64,
                fb_hash,
                fb_nonzero,
            });
        }

        frames += 1;
        fps_frames += 1;
        ram_scroll_us += (t1 - t0).as_micros();
        strip_us += (t2 - t1).as_micros();
        fb_copy_us += (t4 - t3).as_micros();
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Err(e) = cpu_profile::finish(cpu) {
        eprintln!("{e}");
    }
}

#[cfg(mister_bench_scenes)]
fn copy_console_surface(
    disp: &mut Display,
    fb_x: usize,
    fb_y: usize,
    scale: usize,
    surface: &[Pixel],
    surface_y: usize,
) {
    if surface_y == 0 {
        disp.copy_rect_scaled_at(fb_x, fb_y, scale, surface, CONSOLE_LIST_W, CONSOLE_LIST_H);
        return;
    }

    let lower_h = CONSOLE_LIST_H - surface_y;
    disp.copy_rect_scaled_at(
        fb_x,
        fb_y,
        scale,
        &surface[surface_y * CONSOLE_LIST_W..],
        CONSOLE_LIST_W,
        lower_h,
    );
    disp.copy_rect_scaled_at(
        fb_x,
        fb_y + lower_h * scale,
        scale,
        surface,
        CONSOLE_LIST_W,
        surface_y,
    );
}

#[cfg(mister_bench_scenes)]
fn draw_console_virtual_strip_wrapped(
    dst: &mut [Pixel],
    stride: usize,
    dst_y: usize,
    height: usize,
    virtual_y_start: usize,
    font: &mut ConsoleFont,
) {
    let first_h = height.min(CONSOLE_LIST_H - dst_y);
    draw_console_virtual_strip(
        dst,
        stride,
        CONSOLE_LIST_W,
        first_h,
        dst_y,
        virtual_y_start,
        font,
    );

    if first_h < height {
        draw_console_virtual_strip(
            dst,
            stride,
            CONSOLE_LIST_W,
            height - first_h,
            0,
            virtual_y_start + first_h,
            font,
        );
    }
}

#[cfg(mister_bench_scenes)]
fn draw_console_virtual_strip(
    dst: &mut [Pixel],
    stride: usize,
    width: usize,
    height: usize,
    dst_y: usize,
    virtual_y_start: usize,
    font: &mut ConsoleFont,
) {
    let row_h = CONSOLE_ROW_H;
    for dy in 0..height {
        let vy = virtual_y_start + dy;
        let row = vy / row_h;
        let row_y = vy % row_h;
        let y = dst_y + dy;
        if y * stride >= dst.len() {
            break;
        }
        for dx in 0..width {
            let pos = y * stride + dx;
            if pos >= dst.len() {
                break;
            }
            dst[pos] = console_pixel(row, dx, row_y);
        }
    }

    let first_row = virtual_y_start / row_h;
    let last_row = (virtual_y_start + height.saturating_sub(1)) / row_h;
    for row in first_row..=last_row {
        let virtual_row_y = row * row_h;
        let row_screen_y = dst_y as isize + virtual_row_y as isize - virtual_y_start as isize;
        font.draw_text_clipped(
            dst,
            stride,
            width,
            dst_y,
            height,
            12,
            row_screen_y + 27,
            &format!("ROW {row:03}  MISTER GAME"),
            if row % 11 == 5 {
                Pixel(0x00fff2a8)
            } else {
                Pixel(0x00dbe7ff)
            },
        );
        font.draw_text_clipped(
            dst,
            stride,
            width,
            dst_y,
            height,
            CONSOLE_LIST_W as isize - 120,
            row_screen_y + 27,
            "COPY",
            Pixel(0x007dd3fc),
        );
    }
}

#[cfg(mister_bench_scenes)]
fn console_pixel(row: usize, x: usize, y: usize) -> Pixel {
    let selected = row % 11 == 5;
    let bg = if selected {
        Pixel(0x003a2750)
    } else if row % 2 == 0 {
        Pixel(0x00101928)
    } else {
        Pixel(0x000b1220)
    };
    if y < 1 || y >= CONSOLE_ROW_H - 1 {
        return if selected {
            Pixel(0x00f5d76e)
        } else {
            Pixel(0x001f2d44)
        };
    }
    if x < 1 || x >= CONSOLE_LIST_W - 1 {
        return Pixel(0x00263752);
    }
    bg
}

fn run_controller_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    window: &Rc<MinimalSoftwareWindow>,
    mut pad: PadPool,
    app: slint_ui::controller::ControllerTest,
    animation_clock: &AnimationClock,
) {
    let mut cached = vec![Pixel(0); ui.render_w() * ui.render_h()];
    let start = Instant::now();
    let mut frames = 0u64;
    let mut pacer = VsyncPacer::from_env();
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "controller_test running {label} — {} pad(s) connected",
        pad.len()
    );
    while secs == 0 || start.elapsed().as_secs() < secs {
        if pad.poll() {
            sync_bridge(&app, &pad);
            window.request_redraw();
        }
        update_slint_animations(animation_clock);
        let mut this_rect: Option<DirtyRect> = None;
        window.draw_if_needed(|renderer| {
            let region = renderer.render(&mut cached, ui.render_w());
            this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
        });
        let _pace = pacer.wait();
        if let Some(rect) = this_rect {
            copy_cached_rect(disp, ui, &cached, rect);
        }
        frames += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
}

fn recover_launcher_ui(f: &mut Fpga, ui: &UiDisplay, spawned_mister: &mut bool) {
    if *spawned_mister {
        launcher::stop_mister();
        if let Err(e) = f.fb_enable_format(
            0,
            ui.fb_w() as u16,
            ui.fb_h() as u16,
            ui_fpga_scaled_mode(),
            Some(0),
            Some(0),
            std::env::var_os("MISTER_DIRECT_VIDEO").is_some(),
            FramebufferFormat::from_env(),
        ) {
            eprintln!("failed to recover Slint framebuffer route after launch failure: {e}");
        }
        *spawned_mister = false;
    }
}

fn run_launcher_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    f: &mut Fpga,
    window: &Rc<MinimalSoftwareWindow>,
    target: &mut UiFrameTarget,
    mut pad: PadPool,
    app: slint_ui::launcher::Launcher,
    animation_clock: &AnimationClock,
    launcher_mode: LauncherRunMode,
) {
    let start = Instant::now();
    let mut frames = 0u64;
    let mut nav = LauncherNav::new();
    nav.screen = launcher_mode.initial_screen();
    let mut setup = SetupNav::new();
    let mut loading_title = String::new();
    let mut launch_started = Instant::now();
    let mut launch_spawned_mister = false;
    let mut last_clock_update = Instant::now() - Duration::from_secs(2);
    let mut last_clock_text = launcher_clock_text();
    let mut last_status_write = Instant::now() - Duration::from_secs(2);
    let launcher_bench_scenario = LauncherBenchScenario::from_env();
    let mut launcher_bench_next_step = Instant::now();
    let mut launcher_bench_step_idx = 0usize;
    let mut launcher_fps_window_start;
    let mut launcher_fps_frames = 0u64;
    let mut launcher_prepare_us = 0u128;
    let mut launcher_render_us = 0u128;
    let mut launcher_custom_draw_us = 0u128;
    let mut launcher_vsync_us = 0u128;
    let mut launcher_copy_us = 0u128;
    let mut launcher_cached_present_us = 0u128;
    let mut launcher_overlay_present_us = 0u128;
    let mut launcher_rows = 0u128;
    let dirty_opt = launcher_dirty_opt_enabled();
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "launcher running {label} — {} pad(s), D-pad to move, A to select, Home to go back...",
        pad.len()
    );
    println!(
        "launcher_mode={} fb_format={}",
        launcher_mode.label(),
        FramebufferFormat::from_env().label()
    );
    if let Some(scenario) = launcher_bench_scenario {
        println!("launcher_bench_scenario={}", scenario.label());
    }
    println!(
        "launcher_dirty_opt={}",
        if dirty_opt { "on" } else { "off" }
    );
    boot_analytics::event(
        "launcher_loop_start",
        format!("label={label} pads={}", pad.len()),
    );
    if AUTO_CONTROLLER_SETUP_ENABLED {
        if let Some(idx) = pad.index_needing_setup() {
            let status = pad.db().registry_status(pad.info_at(idx));
            eprintln!("controller setup: pad {idx} needs setup ({status:?}) - showing prompt");
            setup.open_for(status, idx);
        }
    }
    let mut pacer = VsyncPacer::from_env();
    let mut present_probe = PresentProbe::from_env();
    let mut boot_frame_profile = boot_analytics::LauncherFrameWriter::from_env();
    let mut preview = PreviewState::new();
    let mut preview_transition = PreviewTransitionDemo::from_env();
    let mut effect_label_overlay = preview_transition
        .label_overlay_enabled()
        .then(EffectLabelOverlay::new);
    let transition_picker_enabled = preview_transition.picker_enabled();
    let mut transition_picker_prev_left = false;
    let mut transition_picker_prev_right = false;
    let mut arcade_list_renderer = ArcadeListRenderer::new();
    let cpu = cpu_profile::start();
    let mut bridge_models = LauncherBridgeModels::default();
    let mut catalog_version = 0usize;
    let mut preview_scroll_trace = std::env::var("MISTER_PREVIEW_SCROLL_TRACE")
        .ok()
        .and_then(|path| {
            let mut file = std::fs::File::create(&path)
                .map_err(|e| eprintln!("preview scroll trace: create {path} failed: {e}"))
                .ok()?;
            std::io::Write::write_all(
                &mut file,
                b"frame\telapsed_us\tselected\tvisual_index\tcache_state\ttransition_effect\ttransition_progress\tarcade_update\trows\tprepare_us\tslint_render_us\tcustom_draw_us\tvsync_us\tfb_present_us\tcached_present_us\toverlay_present_us\tpresent_probe_us\twall_us\n",
            )
            .map_err(|e| eprintln!("preview scroll trace: header write failed: {e}"))
            .ok()?;
            println!("preview_scroll_trace={path}");
            Some(file)
        });
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    let preview_stress = preview_stress_enabled();
    println!(
        "preview_stress={} preview_visual_pct={} preview_blitter={}",
        if preview_stress { "on" } else { "off" },
        preview_visual_pct(),
        if preview_raw_blitter_enabled() {
            "raw"
        } else {
            "slint"
        }
    );
    println!(
        "preview_transition={} segment_secs={} duration_ms={}",
        preview_transition.labels(),
        preview_transition.segment.as_secs(),
        preview_transition.duration.as_millis()
    );
    let mut catalog = empty_arcade_catalog(&arcade_root);
    let mut catalog_ready = false;
    let catalog_refresh = catalog_refresh_requested();
    let catalog_rx;
    let mut catalog_refresh_done = false;
    if launcher_mode == LauncherRunMode::Arcade {
        match library_db::load_arcade_catalog_from_sqlite(&arcade_root) {
            Ok(loaded) if !loaded.catalog.games.is_empty() => {
                catalog = loaded.catalog;
                catalog_ready = true;
                catalog_version = catalog_version.wrapping_add(1);
                apply_forced_arcade_selected(&mut nav, &catalog);
                print_startup_event(
                    start,
                    "catalog_cache_load_sync",
                    format!("games={} load_us={}", catalog.len(), loaded.us),
                );
                if catalog_refresh {
                    print_startup_event(start, "catalog_worker_start", &arcade_root);
                    catalog_rx = Some(start_library_catalog_worker(arcade_root.clone()));
                } else {
                    catalog_rx = None;
                    catalog_refresh_done = true;
                }
            }
            Ok(loaded) => {
                print_startup_event(
                    start,
                    "catalog_cache_empty",
                    format!("games={} load_us={}", loaded.catalog.len(), loaded.us),
                );
                print_startup_event(start, "catalog_worker_start", &arcade_root);
                catalog_rx = Some(start_library_catalog_worker(arcade_root.clone()));
            }
            Err(e) => {
                eprintln!("arcade catalog cache load failed: {e}");
                print_startup_event(start, "catalog_cache_load_failed", e);
                print_startup_event(start, "catalog_worker_start", &arcade_root);
                catalog_rx = Some(start_library_catalog_worker(arcade_root.clone()));
            }
        }
    } else {
        print_startup_event(start, "catalog_cache_load_deferred", &arcade_root);
        print_startup_event(start, "catalog_worker_start", &arcade_root);
        catalog_rx = Some(start_library_catalog_worker(arcade_root.clone()));
    }
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_game_systems(bridge_models.game_systems(&catalog, catalog_version));
    bridge.set_catalog_scan_visible(!catalog_ready);
    bridge.set_catalog_scan_title(if catalog_ready {
        if catalog_refresh {
            "Refreshing library".into()
        } else {
            "".into()
        }
    } else {
        "Indexing library".into()
    });
    bridge.set_catalog_scan_detail(if catalog_ready {
        format!("Using cached {} games", catalog.len()).into()
    } else {
        "Starting scan...".into()
    });
    sync_bridge_launcher(
        &app,
        &pad,
        &nav,
        &setup,
        "",
        "",
        Some(&catalog),
        &mut preview,
        &mut bridge_models,
        catalog_version,
    );
    window.request_redraw();
    let run_start = if launcher_mode == LauncherRunMode::Arcade && catalog_ready {
        Instant::now()
    } else {
        start
    };
    launcher_fps_window_start = run_start;
    let mut first_frame_logged = false;
    let mut first_render_logged = false;
    let mut first_vsync_logged = false;
    let mut first_copy_logged = false;
    let mut first_visible_copy_done = false;
    let mut stable_frame_logged = false;
    while secs == 0 || run_start.elapsed().as_secs() < secs {
        let loop_start = Instant::now();
        let launching = launcher::launch_in_progress() || !loading_title.is_empty();
        let setup_active = setup.is_active();
        let mut light_bridge_dirty = false;
        let mut full_bridge_dirty = false;
        if last_clock_update.elapsed() >= Duration::from_secs(1) {
            let clock_text = launcher_clock_text();
            if dirty_opt {
                if clock_text != last_clock_text {
                    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                    bridge.set_clock_text(clock_text.clone().into());
                    last_clock_text = clock_text;
                    light_bridge_dirty = true;
                }
            } else {
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                bridge.set_clock_text(clock_text.clone().into());
                last_clock_text = clock_text;
                full_bridge_dirty = true;
            }
            last_clock_update = Instant::now();
        }

        if !catalog_refresh_done {
            while let Some(message) = catalog_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                match message {
                    CatalogWorkerMessage::Progress { title, detail } => {
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        let visible = if catalog_ready && launcher_mode == LauncherRunMode::Arcade {
                            title == "Library scan failed" || title == "Library load failed"
                        } else {
                            !catalog_ready
                                || title == "Indexing library"
                                || title == "Library changed"
                                || title == "Library scan failed"
                                || title == "Library load failed"
                        };
                        bridge.set_catalog_scan_visible(visible);
                        bridge.set_catalog_scan_title(title.into());
                        bridge.set_catalog_scan_detail(detail.into());
                        full_bridge_dirty = true;
                    }
                    CatalogWorkerMessage::Ready {
                        catalog: ready_catalog,
                        summary,
                        load_us,
                    } => {
                        catalog = ready_catalog;
                        catalog_version = catalog_version.wrapping_add(1);
                        catalog_ready = true;
                        apply_forced_arcade_selected(&mut nav, &catalog);
                        let cached_before_refresh = summary.is_none();
                        catalog_refresh_done = !cached_before_refresh;
                        print_startup_event(
                            start,
                            "library_ready",
                            format!("games={} load_us={load_us}", catalog.len()),
                        );
                        if let Some(summary) = summary {
                            let event = if summary.skipped {
                                "library_db_unchanged"
                            } else {
                                "library_db_saved"
                            };
                            print_startup_event(
                                start,
                                event,
                                format!(
                                    "bytes={} scan_us={} import_us={} discoveries={} normal_files={} containers={} entries={}",
                                    summary.bytes,
                                    summary.scan_us,
                                    summary.import_us,
                                    summary.discoveries,
                                    summary.normal_files,
                                    summary.containers,
                                    summary.entries
                                ),
                            );
                        }
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        bridge.set_catalog_scan_visible(false);
                        if cached_before_refresh {
                            bridge.set_catalog_scan_title("Refreshing library".into());
                            bridge.set_catalog_scan_detail(
                                format!("Using cached {} games", catalog.len()).into(),
                            );
                        } else {
                            bridge.set_catalog_scan_title("".into());
                            bridge.set_catalog_scan_detail("".into());
                        }
                        sync_bridge_launcher(
                            &app,
                            &pad,
                            &nav,
                            &setup,
                            &loading_title,
                            "",
                            Some(&catalog),
                            &mut preview,
                            &mut bridge_models,
                            catalog_version,
                        );
                        full_bridge_dirty = true;
                    }
                    CatalogWorkerMessage::Unchanged { summary } => {
                        catalog_refresh_done = true;
                        print_startup_event(
                            start,
                            "library_db_unchanged",
                            format!(
                                "bytes={} scan_us={} import_us={} discoveries={} normal_files={} containers={} entries={}",
                                summary.bytes,
                                summary.scan_us,
                                summary.import_us,
                                summary.discoveries,
                                summary.normal_files,
                                summary.containers,
                                summary.entries
                            ),
                        );
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        bridge.set_catalog_scan_visible(false);
                        bridge.set_catalog_scan_title("".into());
                        bridge.set_catalog_scan_detail("".into());
                        full_bridge_dirty = true;
                    }
                    CatalogWorkerMessage::Done => {
                        catalog_refresh_done = true;
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        bridge.set_catalog_scan_visible(false);
                        bridge.set_catalog_scan_title("".into());
                        bridge.set_catalog_scan_detail("".into());
                        full_bridge_dirty = true;
                    }
                }
            }
        }

        if let Some(scenario) = launcher_bench_scenario {
            if catalog_ready && launcher_bench_next_step.elapsed() >= scenario.period() {
                let before = LauncherBridgeKey::from_nav(&nav);
                if launcher_bench_step(
                    scenario,
                    &mut nav,
                    &catalog,
                    None,
                    launcher_bench_step_idx,
                    Instant::now(),
                ) {
                    let after = LauncherBridgeKey::from_nav(&nav);
                    if before != after {
                        if !dirty_opt || before.screen != after.screen {
                            full_bridge_dirty = true;
                        } else {
                            light_bridge_dirty = true;
                        }
                    }
                }
                launcher_bench_step_idx = launcher_bench_step_idx.wrapping_add(1);
                launcher_bench_next_step = Instant::now();
            }
        }

        launcher_mode.enforce(&mut nav);

        if !launching {
            let pad_changed = pad.poll();
            let frame_now = Instant::now();
            let state = pad.state();
            let active_idx = pad.active_idx();
            let info = pad.info();

            if setup_active && setup.target_pad_idx >= pad.len() {
                eprintln!(
                    "controller setup: pad {} disappeared; closing setup flow",
                    setup.target_pad_idx
                );
                setup.advance_to_next_pad(&pad);
                full_bridge_dirty = true;
            }

            if launcher_bench_scenario.is_none() && setup.is_active() {
                let setup_before = SetupBridgeKey::from_setup(&setup);
                let setup_info = pad.info_at(setup.target_pad_idx);
                match setup.handle_input(&state, frame_now, setup_info, pad.db()) {
                    SetupAction::None => {}
                    SetupAction::RegisterNew => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.register_new_at(idx) {
                            eprintln!("controller setup: register new: {e}");
                        }
                    }
                    SetupAction::ClaimExisting { list_index } => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.claim_existing_at(idx, list_index) {
                            eprintln!("controller setup: claim existing: {e}");
                        }
                    }
                    SetupAction::SaveFinish { label, kind } => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.finish_setup_at(idx, label, kind) {
                            eprintln!("controller setup: save: {e}");
                        } else {
                            eprintln!(
                                "controller setup: saved \"{}\" ({})",
                                pad.db().display_label(pad.info_at(idx)),
                                kind.as_str()
                            );
                        }
                        setup.advance_to_next_pad(&pad);
                    }
                    SetupAction::Done => {
                        setup.advance_to_next_pad(&pad);
                    }
                }
                let setup_after = SetupBridgeKey::from_setup(&setup);
                full_bridge_dirty |= pad_changed || setup_before != setup_after;
            } else if launcher_bench_scenario.is_none() {
                if AUTO_CONTROLLER_SETUP_ENABLED && pad_changed {
                    let setup_before = SetupBridgeKey::from_setup(&setup);
                    setup.maybe_open(info, active_idx, pad.db(), true);
                    full_bridge_dirty |= setup_before != SetupBridgeKey::from_setup(&setup);
                }
                if !setup.is_active() {
                    let nav_before = LauncherBridgeKey::from_nav(&nav);
                    if transition_picker_enabled && nav.screen == Screen::Arcade {
                        let left = state.dpad_left && !transition_picker_prev_left;
                        let right = state.dpad_right && !transition_picker_prev_right;
                        let changed = if left {
                            preview_transition.cycle_picker(-1)
                        } else if right {
                            preview_transition.cycle_picker(1)
                        } else {
                            false
                        };
                        if changed {
                            println!(
                                "preview_transition_picker={}",
                                preview_transition
                                    .current_label(frame_now.duration_since(run_start))
                            );
                            window.request_redraw();
                        }
                    }
                    transition_picker_prev_left = state.dpad_left;
                    transition_picker_prev_right = state.dpad_right;
                    if let Some(event) = nav.handle_input(&state, frame_now, &catalog) {
                        match event.action {
                            LauncherAction::ExitToMister => {
                                loading_title = "Exit to MiSTer".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Return to MiSTer MagiK after reboot",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, ui);
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                match launcher::exit_to_mister() {
                                    Ok(()) => std::process::exit(0),
                                    Err(e) => {
                                        eprintln!("exit to MiSTer failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::ResetDatabase => {
                                loading_title = "Resetting database…".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Rebooting MiSTer",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, ui);
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                match launcher::reset_database_and_reboot() {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        eprintln!("reset database failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::Restart => {
                                loading_title = "Restarting MiSTer…".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Please wait",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, ui);
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                match launcher::reboot_mister() {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        eprintln!("restart failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::LaunchGame => {}
                        }
                        let Some(mra) = event.path else {
                            continue;
                        };
                        loading_title =
                            format!("Loading {}…", launcher::game_title(&catalog, &mra));
                        sync_bridge_launcher(
                            &app,
                            &pad,
                            &nav,
                            &setup,
                            &loading_title,
                            "",
                            Some(&catalog),
                            &mut preview,
                            &mut bridge_models,
                            catalog_version,
                        );
                        window.request_redraw();
                        update_slint_animations(animation_clock);
                        window.draw_if_needed(|renderer| {
                            let region = target.render(renderer, ui);
                            let _ = region;
                        });
                        let _pace = pacer.wait();
                        target.present_rows(f, disp, ui, 0, ui.render_h());
                        match launcher::execute_game_launch(&mra) {
                            Ok(spawned) => {
                                launch_started = Instant::now();
                                launch_spawned_mister = spawned;
                            }
                            Err(e) => {
                                eprintln!("game launch failed: {e}");
                                launch_spawned_mister |= e.spawned_mister();
                                loading_title.clear();
                                launcher::reset_launch();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    "",
                                    "",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                );
                                recover_launcher_ui(f, ui, &mut launch_spawned_mister);
                            }
                        }
                        window.request_redraw();
                    }
                    let nav_after = LauncherBridgeKey::from_nav(&nav);
                    if pad_changed && nav.screen == Screen::Controller {
                        full_bridge_dirty = true;
                    } else if pad_changed && !dirty_opt {
                        full_bridge_dirty = true;
                    }
                    if nav_before != nav_after {
                        if !dirty_opt || nav_before.screen != nav_after.screen {
                            full_bridge_dirty = true;
                        } else {
                            light_bridge_dirty = true;
                        }
                    }
                }
            }

            launcher_mode.enforce(&mut nav);

            if full_bridge_dirty {
                sync_bridge_launcher(
                    &app,
                    &pad,
                    &nav,
                    &setup,
                    &loading_title,
                    "",
                    Some(&catalog),
                    &mut preview,
                    &mut bridge_models,
                    catalog_version,
                );
                window.request_redraw();
            } else if light_bridge_dirty {
                let active_games = if nav.screen == Screen::Arcade {
                    Some(active_system_game_slice(&catalog, &nav))
                } else {
                    None
                };
                sync_bridge_launcher_light(
                    &app,
                    &nav,
                    &setup,
                    &loading_title,
                    "",
                    &catalog,
                    active_games,
                    &mut preview,
                );
                window.request_redraw();
            }
        } else {
            let _ = pad.poll();
            if launcher::mister_running_arcade_core()
                && launch_started.elapsed() > Duration::from_millis(500)
            {
                println!("arcade core running — handing off to MiSTer");
                std::process::exit(0);
            } else if launch_started.elapsed() > Duration::from_secs(90) {
                eprintln!("game launch timed out");
                recover_launcher_ui(f, ui, &mut launch_spawned_mister);
                std::process::exit(1);
            }
        }

        if launching {
            window.request_redraw();
        }
        let active_arcade_games = if !launching && nav.screen == Screen::Arcade {
            active_system_game_slice(&catalog, &nav)
        } else {
            &[]
        };
        if dirty_opt && !launching && nav.screen == Screen::Arcade {
            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
            if schedule_arcade_preview_window(
                &bridge,
                active_arcade_games,
                nav.arcade.selected,
                &mut preview,
            ) {
                window.request_redraw();
            }
        }
        if !launching && apply_ready_preview(&app, &mut preview) {
            window.request_redraw();
        }

        let frame_t0 = Instant::now();
        let prepare_us = (frame_t0 - loop_start).as_micros();
        update_slint_animations(animation_clock);
        let frame_t1 = Instant::now();
        let mut this_rect: Option<DirtyRect> = None;
        window.draw_if_needed(|renderer| {
            let region = target.render(renderer, ui);
            this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
        });
        let frame_t2 = Instant::now();
        let custom_draw_start = Instant::now();
        let arcade_list_rect = if !preview_stress && !launching && nav.screen == Screen::Arcade {
            let force_arcade_redraw = this_rect.is_some_and(|rect| {
                rect.intersection(ArcadeListRenderer::dirty_rect())
                    .is_some()
            });
            arcade_list_renderer.draw(
                active_arcade_games,
                nav.arcade.visual_index,
                force_arcade_redraw,
            )
        } else {
            None
        };
        let (raw_preview_rect, preview_transition_trace) = blit_raw_preview_if_needed(
            target,
            ui,
            &mut preview,
            &mut preview_transition,
            loop_start.duration_since(run_start),
            this_rect,
        );
        if preview_transition_trace.active {
            window.request_redraw();
        }
        let effect_label_rect = effect_label_overlay
            .as_mut()
            .map(|overlay| overlay.draw(target, ui, preview_transition_trace.effect.label()));
        let custom_draw_done = Instant::now();
        if !first_render_logged {
            first_render_logged = true;
            boot_analytics::event(
                "first_render",
                format!("frame={frames} dirty_rect={}", format_dirty_rect(this_rect)),
            );
        }
        let pace = if first_visible_copy_done {
            let pace = pacer.wait();
            let frame_t3 = Instant::now();
            (Some(pace), frame_t3)
        } else {
            (None, Instant::now())
        };
        let frame_t3 = pace.1;
        if !first_vsync_logged
            && pace
                .0
                .as_ref()
                .is_some_and(|p| p.source == VsyncPaceSource::Vsync)
        {
            first_vsync_logged = true;
            boot_analytics::event("first_vsync", format!("frame={frames}"));
        }
        let mut copied_rows = 0u32;
        let mut cached_present_frame_us = 0u128;
        if launching {
            let cached_copy_start = Instant::now();
            copied_rows = target.present_rows(f, disp, ui, 0, ui.render_h());
            cached_present_frame_us = cached_copy_start.elapsed().as_micros();
        } else if let Some(rect) = this_rect {
            let cached_copy_start = Instant::now();
            copied_rows = target.present_rect(f, disp, ui, rect);
            cached_present_frame_us = cached_copy_start.elapsed().as_micros();
        }
        if let Some(rect) = raw_preview_rect {
            let cached_copy_start = Instant::now();
            copied_rows += target.present_rect(f, disp, ui, rect);
            cached_present_frame_us += cached_copy_start.elapsed().as_micros();
        }
        if let Some(rect) = effect_label_rect {
            if !this_rect.is_some_and(|slint_rect| slint_rect.contains(rect)) {
                let cached_copy_start = Instant::now();
                copied_rows += target.present_rect(f, disp, ui, rect);
                cached_present_frame_us += cached_copy_start.elapsed().as_micros();
            }
        }
        let arcade_update_label = match arcade_list_rect.as_ref() {
            Some(ArcadeListUpdate::Full(_)) => "full".to_string(),
            Some(ArcadeListUpdate::Scroll { delta_y }) => format!("scroll:{delta_y}"),
            None => "none".to_string(),
        };
        let mut overlay_present_frame_us = 0u128;
        if let Some(update) = arcade_list_rect {
            let overlay_copy_start = Instant::now();
            copied_rows +=
                copy_arcade_list_update(target, disp, ui, &mut arcade_list_renderer, update);
            overlay_present_frame_us = overlay_copy_start.elapsed().as_micros();
        }
        let mut present_probe_frame_us = 0u128;
        if let Some(probe) = present_probe.as_mut() {
            let probe_copy_start = Instant::now();
            copied_rows += probe.present(disp, frames);
            present_probe_frame_us = probe_copy_start.elapsed().as_micros();
        }
        let frame_t4 = Instant::now();
        if let Some(file) = preview_scroll_trace.as_mut() {
            let cache_state = preview.trace_cache_state();
            let _ = std::io::Write::write_fmt(
                file,
                format_args!(
                    "{}\t{}\t{}\t{:.6}\t{}\t{}\t{:.3}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    frames,
                    loop_start.duration_since(run_start).as_micros(),
                    nav.arcade.selected,
                    nav.arcade.visual_index,
                    cache_state,
                    preview_transition_trace.effect.label(),
                    preview_transition_trace.progress,
                    arcade_update_label,
                    copied_rows,
                    prepare_us,
                    (frame_t2 - frame_t1).as_micros(),
                    (custom_draw_done - custom_draw_start).as_micros(),
                    (frame_t3 - custom_draw_done).as_micros(),
                    (frame_t4 - frame_t3).as_micros(),
                    cached_present_frame_us,
                    overlay_present_frame_us,
                    present_probe_frame_us,
                    (frame_t4 - loop_start).as_micros()
                ),
            );
        }
        if copied_rows > 0 && !first_copy_logged {
            first_copy_logged = true;
            boot_analytics::event(
                if first_visible_copy_done {
                    "first_copy"
                } else {
                    "first_copy_immediate"
                },
                format!(
                    "frame={frames} rows={copied_rows} dirty_rect={}",
                    format_dirty_rect(this_rect)
                ),
            );
            disp.record_visual_sample("after_first_copy");
        }
        if copied_rows > 0 {
            first_visible_copy_done = true;
        }
        launcher_fps_frames += 1;
        launcher_prepare_us += prepare_us;
        launcher_render_us += (frame_t2 - frame_t1).as_micros();
        launcher_custom_draw_us += (custom_draw_done - custom_draw_start).as_micros();
        launcher_vsync_us += (frame_t3 - custom_draw_done).as_micros();
        launcher_copy_us += (frame_t4 - frame_t3).as_micros();
        launcher_cached_present_us += cached_present_frame_us;
        launcher_overlay_present_us += overlay_present_frame_us;
        launcher_rows += copied_rows as u128;
        if launcher_fps_window_start.elapsed() >= Duration::from_secs(1) {
            let n = launcher_fps_frames.max(1) as u128;
            println!(
                "launcher fps ~ {} prepare {}us slint-render {}us custom-draw {}us vsync-wait {}us fb-present {}us cached-present {}us overlay-present {}us ({} rows avg)",
                launcher_fps_frames,
                launcher_prepare_us / n,
                launcher_render_us / n,
                launcher_custom_draw_us / n,
                launcher_vsync_us / n,
                launcher_copy_us / n,
                launcher_cached_present_us / n,
                launcher_overlay_present_us / n,
                launcher_rows / n
            );
            launcher_fps_window_start = Instant::now();
            launcher_fps_frames = 0;
            launcher_prepare_us = 0;
            launcher_render_us = 0;
            launcher_custom_draw_us = 0;
            launcher_vsync_us = 0;
            launcher_copy_us = 0;
            launcher_cached_present_us = 0;
            launcher_overlay_present_us = 0;
            launcher_rows = 0;
        }
        if frames == 30 && !stable_frame_logged {
            stable_frame_logged = true;
            boot_analytics::event("stable_frame", "frame=30");
            disp.record_visual_sample("stable_frame_30");
        } else if frames == 120 {
            disp.record_visual_sample("sample_frame_120");
        } else if frames == 240 {
            disp.record_visual_sample("sample_frame_240");
        }
        let reasserted = false;
        if boot_frame_profile
            .as_ref()
            .is_some_and(|profile| !profile.should_record(frames))
        {
            boot_frame_profile = None;
        }
        if let Some(profile) = boot_frame_profile.as_mut() {
            let (edge1_hash, edge1_nonzero) = disp.right_edge_signature(1);
            let (edge8_hash, edge8_nonzero) = disp.right_edge_signature(8);
            let (left8_hash, left8_nonzero) = disp.left_edge_signature(8);
            let (top8_hash, top8_nonzero) = disp.top_edge_signature(8);
            let (bottom8_hash, bottom8_nonzero) = disp.bottom_edge_signature(8);
            let (full_sample_hash, full_sample_nonzero) = disp.sampled_signature();
            profile.record(
                frames,
                (frame_t1 - frame_t0).as_micros() as u64,
                (frame_t2 - frame_t1).as_micros() as u64,
                (frame_t3 - frame_t2).as_micros() as u64,
                (frame_t4 - frame_t3).as_micros() as u64,
                copied_rows,
                reasserted,
                edge1_hash,
                edge1_nonzero,
                edge8_hash,
                edge8_nonzero,
                left8_hash,
                left8_nonzero,
                top8_hash,
                top8_nonzero,
                bottom8_hash,
                bottom8_nonzero,
                full_sample_hash,
                full_sample_nonzero,
            );
        }
        if !first_frame_logged {
            first_frame_logged = true;
            boot_analytics::event("first_frame", format!("catalog_ready={catalog_ready}"));
            print_startup_event(
                start,
                "first_frame",
                format!("catalog_ready={catalog_ready}"),
            );
        }
        if last_status_write.elapsed() >= Duration::from_secs(1) {
            let fps_estimate = if run_start.elapsed().as_secs_f64() > 0.0 {
                frames as f64 / run_start.elapsed().as_secs_f64()
            } else {
                0.0
            };
            runtime_status::write_launcher_status(LauncherStatus {
                scene: launcher_mode.label(),
                screen: screen_label(nav.screen),
                frames,
                fps_estimate,
                last_frame_ms_ago: 0,
                catalog_ready,
                catalog_games: catalog.len(),
                catalog_systems: catalog.systems.len(),
                catalog_refresh_done,
                launch_state: if launching { "launching" } else { "idle" },
                loading_title: &loading_title,
                input_pad_count: pad.len(),
                active_pad_index: pad.active_idx(),
            });
            last_status_write = Instant::now();
        }
        frames += 1;
    }
    let elapsed = run_start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Err(e) = cpu_profile::finish(cpu) {
        eprintln!("{e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_half_target_allows_640x448_at_native_scale() {
        let ui = UiDisplay::for_framebuffer(1920, 1080);
        let target = EffectTarget::new(EffectFill::Half, EffectSize { w: 640, h: 448 }, &ui)
            .expect("640x448 should fit in half-fill benchmark mode");

        assert_eq!(target.physical_w, 640);
        assert_eq!(target.physical_h, 448);
        assert_eq!(target.render_w, 640);
        assert_eq!(target.render_h, 448);
        assert_eq!(target.scale, 1);
    }
}
