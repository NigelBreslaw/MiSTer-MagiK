//! Shared vsync render loop and Slint bench scene dispatch.

use crate::fb::{Display, Pixel, VsyncPacer};
use crate::fpga::{Fpga, Mode};
use crate::vt::VtGraphicsGuard;
use mister_magik_fb::vsync_pacer::VsyncPaceSource;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{
    ComponentHandle, Image, ModelRc, PhysicalSize, Rgb8Pixel, SharedPixelBuffer, SharedString,
    VecModel,
};
use std::rc::Rc;
use std::time::{Duration, Instant};

mod slint_ui {
    #![allow(clippy::all, unused_imports)]
    pub mod app {
        include!(concat!(env!("OUT_DIR"), "/app.rs"));
    }
    #[cfg(not(mister_ui_scope_launcher))]
    pub mod full_motion {
        include!(concat!(env!("OUT_DIR"), "/full_motion.rs"));
    }
    #[cfg(not(mister_ui_scope_launcher))]
    pub mod static_ui {
        include!(concat!(env!("OUT_DIR"), "/static_ui.rs"));
    }
    #[cfg(not(mister_ui_scope_launcher))]
    pub mod local_motion {
        include!(concat!(env!("OUT_DIR"), "/local_motion.rs"));
    }
    #[cfg(not(mister_ui_scope_launcher))]
    pub mod console_scroll {
        include!(concat!(env!("OUT_DIR"), "/console_scroll.rs"));
    }
    #[cfg(not(mister_ui_scope_launcher))]
    pub mod effect_hud {
        include!(concat!(env!("OUT_DIR"), "/effect_hud.rs"));
    }
    #[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
    pub mod video_playback {
        include!(concat!(env!("OUT_DIR"), "/video_playback.rs"));
    }
    pub mod controller {
        include!(concat!(env!("OUT_DIR"), "/controller_test.rs"));
    }
    pub mod launcher {
        include!(concat!(env!("OUT_DIR"), "/launcher.rs"));
    }
    pub mod arcade_page {
        include!(concat!(env!("OUT_DIR"), "/arcade_page.rs"));
    }
}

use crate::arcade_catalog::{
    self, ArcadeCatalog, ArcadeGameEntry, ARCADE_LIST_VISIBLE_H, ARCADE_ROW_HEIGHT,
    HOME_LIST_VISIBLE_W, HOME_TILE_GAP, HOME_TILE_WIDTH,
};
use crate::boot_analytics;
use crate::controller_db::ControllerDb;
use crate::cpu_profile;
use crate::display_config::DisplayConfig;
use crate::frame_profile::{FrameProfiler, FrameRect, FrameSample};
use crate::input::{PadInfo, PadPool};
use crate::launcher::{self, LauncherAction, LauncherNav, Screen};
use crate::library_bench;
use crate::preview_worker::PreviewWorker;
use crate::runtime_status::{self, LauncherStatus};
use crate::setup_nav::{SetupAction, SetupNav, SetupPhase};
use crate::ui_display::{UiDisplay, SLINT_UI_SCALE, UI_FB_H, UI_FB_W, UI_HDMI_H, UI_HDMI_W};
use mister_magik_fb::effects::{EffectKind, EffectSize, EffectState, EFFECT_SIZES};
use slint::platform::software_renderer::PhysicalRegion;
use slint_ui::launcher::PreviewStatus;
use std::cell::Cell;
use std::collections::HashMap;
use std::collections::VecDeque;
#[cfg(not(mister_ui_scope_launcher))]
use std::fs::File;
#[cfg(not(mister_ui_scope_launcher))]
use std::io::Write;
use std::path::PathBuf;
use std::sync::{mpsc, Mutex, OnceLock};

const AUTO_CONTROLLER_SETUP_ENABLED: bool = false;
const DEFAULT_DIRTY_RECT_BROAD_PCT: usize = 85;
const PREVIEW_MAX_AREA: u32 = (UI_FB_W as u32 * UI_FB_H as u32 * 40) / 100;
const ARCADE_PREVIEW_BOX_X: usize = 12;
const ARCADE_PREVIEW_BOX_Y: usize = 16;
const ARCADE_PREVIEW_BOX_W: u32 = 456;
const ARCADE_PREVIEW_BOX_H: u32 = 444;

fn screen_label(screen: Screen) -> &'static str {
    match screen {
        Screen::Home => "home",
        Screen::Controller => "controller",
        Screen::Arcade => "arcade",
        Screen::Settings => "settings",
    }
}

pub const UI_SCENES: &[&str] = &[
    "launcher",
    "arcade_page",
    "blend_velocity",
    "demo",
    "controller_test",
    #[cfg(not(mister_ui_scope_launcher))]
    "full_motion",
    #[cfg(not(mister_ui_scope_launcher))]
    "static_ui",
    #[cfg(not(mister_ui_scope_launcher))]
    "local_motion",
    #[cfg(not(mister_ui_scope_launcher))]
    "console_scroll",
    #[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
    "video_playback",
];

struct MisterPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
    fixed_time: Option<Rc<Cell<Duration>>>,
}

#[derive(Clone)]
struct AnimationClock {
    fixed_time: Option<Rc<Cell<Duration>>>,
    fixed_step: Duration,
}

impl AnimationClock {
    fn from_env() -> Self {
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

    fn platform_time(&self) -> Option<Rc<Cell<Duration>>> {
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

fn update_slint_animations(animation_clock: &AnimationClock) {
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
struct DirtyRect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl DirtyRect {
    fn rows(self) -> u32 {
        (self.y1 - self.y0) as u32
    }

    fn width(self) -> usize {
        self.x1 - self.x0
    }

    fn is_full_width(self, render_w: usize) -> bool {
        self.x0 == 0 && self.x1 >= render_w
    }

    fn intersection(self, other: DirtyRect) -> Option<DirtyRect> {
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

fn preview_trace_enabled() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        matches!(
            std::env::var("MISTER_PREVIEW_TRACE").as_deref(),
            Ok("1") | Ok("on") | Ok("true") | Ok("yes")
        )
    })
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectBenchMode {
    Raw,
    Overlay,
}

impl EffectBenchMode {
    fn label(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Overlay => "overlay",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectFill {
    Full,
    Half,
    Native,
    FpgaHalf,
}

impl EffectFill {
    fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Half => "half",
            Self::Native => "native",
            Self::FpgaHalf => "fpga-half",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "full" => Some(Self::Full),
            "half" => Some(Self::Half),
            "native" => Some(Self::Native),
            "fpga-half" => Some(Self::FpgaHalf),
            _ => None,
        }
    }

    fn uses_fpga_scaler(self) -> bool {
        matches!(self, Self::FpgaHalf)
    }
}

#[derive(Clone, Copy, Debug)]
struct EffectTarget {
    physical_x: usize,
    physical_y: usize,
    physical_w: usize,
    physical_h: usize,
    render_w: usize,
    render_h: usize,
    scale: usize,
}

impl EffectTarget {
    fn new(fill: EffectFill, size: EffectSize, ui: &UiDisplay) -> Option<Self> {
        let (physical_w, physical_h, scale) = match fill {
            EffectFill::Full => (1920, 1080, size.scale_to_1080p()?),
            EffectFill::Half => (960, 540, size.scale_to_half_1080p()?),
            EffectFill::Native => (size.w, size.h, 1),
            EffectFill::FpgaHalf => {
                if size.w != 480 || size.h != 270 {
                    return None;
                }
                (960, 540, 2)
            }
        };
        if !fill.uses_fpga_scaler() && (physical_w > ui.fb_w() || physical_h > ui.fb_h()) {
            return None;
        }
        Some(Self {
            physical_x: if fill.uses_fpga_scaler() {
                480
            } else {
                (ui.fb_w() - physical_w) / 2
            },
            physical_y: if fill.uses_fpga_scaler() {
                270
            } else {
                (ui.fb_h() - physical_h) / 2
            },
            physical_w,
            physical_h,
            render_w: if fill.uses_fpga_scaler() {
                size.w
            } else {
                physical_w / ui.fb_scale()
            },
            render_h: if fill.uses_fpga_scaler() {
                size.h
            } else {
                physical_h / ui.fb_scale()
            },
            scale,
        })
    }
}

struct FbModeGuard {
    previous: crate::fb::FbInfo,
    active: bool,
}

impl FbModeGuard {
    fn set_temporary(w: usize, h: usize) -> std::io::Result<Self> {
        let previous = Display::current_info()?;
        Display::write_mister_mode(w, h, w * 4)?;
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

fn parse_effect_bench_args() -> (
    Vec<EffectKind>,
    u64,
    Vec<EffectBenchMode>,
    EffectSize,
    EffectFill,
) {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let effect_arg = args.first().map(String::as_str).unwrap_or("all");
    let effects = if effect_arg == "all" {
        EffectKind::all().to_vec()
    } else {
        match EffectKind::parse(effect_arg) {
            Some(kind) => vec![kind],
            None => {
                eprintln!("unknown effect '{effect_arg}' (use `effects` to list names)");
                std::process::exit(2);
            }
        }
    };
    let secs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let modes = match args.get(2).map(String::as_str).unwrap_or("both") {
        "raw" => vec![EffectBenchMode::Raw],
        "overlay" => vec![EffectBenchMode::Overlay],
        "both" => vec![EffectBenchMode::Raw, EffectBenchMode::Overlay],
        other => {
            eprintln!("unknown effect-bench mode '{other}' (use raw|overlay|both)");
            std::process::exit(2);
        }
    };
    let size = match args.get(3).map(String::as_str) {
        Some(s) => match EffectSize::parse(s) {
            Some(size) => size,
            None => {
                eprintln!("unsupported effect size '{s}' (use `effects` to list supported sizes)");
                std::process::exit(2);
            }
        },
        None => EffectSize { w: 480, h: 270 },
    };
    let fill = match args.get(4).map(String::as_str) {
        Some(s) => EffectFill::parse(s).unwrap_or_else(|| {
            eprintln!("unknown effect fill '{s}' (use full|half|native|fpga-half)");
            std::process::exit(2);
        }),
        None => EffectFill::Full,
    };
    if fill == EffectFill::FpgaHalf && modes.iter().any(|m| *m != EffectBenchMode::Raw) {
        eprintln!("effect fill fpga-half supports raw mode only");
        std::process::exit(2);
    }
    (effects, secs, modes, size, fill)
}

#[derive(Default)]
struct EffectBenchTotals {
    frames: u64,
    effect_us: u128,
    slint_us: u128,
    scale_copy_us: u128,
    vsync_us: u128,
    wall_us: u128,
    slow_frames: u64,
}

impl EffectBenchTotals {
    fn record(
        &mut self,
        effect_us: u64,
        slint_us: u64,
        scale_copy_us: u64,
        vsync_us: u64,
        wall_us: u64,
    ) {
        self.frames += 1;
        self.effect_us += effect_us as u128;
        self.slint_us += slint_us as u128;
        self.scale_copy_us += scale_copy_us as u128;
        self.vsync_us += vsync_us as u128;
        self.wall_us += wall_us as u128;
        if wall_us >= 16_667 {
            self.slow_frames += 1;
        }
    }

    fn avg(v: u128, frames: u64) -> u64 {
        if frames == 0 {
            0
        } else {
            (v / frames as u128) as u64
        }
    }
}

fn scale_effect_to_pixels_fit(
    src: &[u32],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    dst: &mut [Pixel],
) {
    assert!(dst.len() >= dst_w * dst_h);
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return;
    }
    for y in 0..dst_h {
        let sy = (y * src_h / dst_h).min(src_h - 1);
        let src_row = &src[sy * src_w..(sy + 1) * src_w];
        let dst_row = &mut dst[y * dst_w..(y + 1) * dst_w];
        for (x, p) in dst_row.iter_mut().enumerate() {
            let sx = (x * src_w / dst_w).min(src_w - 1);
            *p = Pixel(src_row[sx]);
        }
    }
}

#[cfg(not(mister_ui_scope_launcher))]
pub fn run_effect_bench(f: &mut Fpga) {
    let (effects, secs, modes, size, fill) = parse_effect_bench_args();
    println!(
        "effect-bench effects={} secs={} modes={} fill={} internal={}x{}",
        effects
            .iter()
            .map(|k| k.name())
            .collect::<Vec<_>>()
            .join(","),
        secs,
        modes
            .iter()
            .map(|m| m.label())
            .collect::<Vec<_>>()
            .join(","),
        fill.label(),
        size.w,
        size.h
    );

    let _vt = VtGraphicsGuard::enter_or_warn();
    if fill == EffectFill::FpgaHalf && (size.w != 480 || size.h != 270) {
        eprintln!(
            "effect fill fpga-half supports only 480x270, got {}x{}",
            size.w, size.h
        );
        std::process::exit(2);
    }
    let _fb_mode_guard = if fill == EffectFill::FpgaHalf {
        println!("effect-bench-fb-mode=temporary 480x270 stride=1920 restore=on-drop");
        match FbModeGuard::set_temporary(size.w, size.h) {
            Ok(guard) => Some(guard),
            Err(e) => {
                eprintln!("failed to set temporary framebuffer mode for fpga-half: {e}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };
    let mut disp = if fill == EffectFill::FpgaHalf {
        match Display::open(size.w, size.h) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("failed to open temporary display (/dev/fb0): {e}");
                std::process::exit(1);
            }
        }
    } else {
        match Display::open_current_boot() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("failed to open display (/dev/fb0): {e}");
                std::process::exit(1);
            }
        }
    };
    let ui = UiDisplay::for_framebuffer(disp.width(), disp.height());
    let target = EffectTarget::new(fill, size, &ui).unwrap_or_else(|| {
        eprintln!(
            "effect size {}x{} cannot fill={} on framebuffer {}x{}",
            size.w,
            size.h,
            fill.label(),
            disp.width(),
            disp.height()
        );
        std::process::exit(2);
    });
    println!("{}", ui.log_line());
    let display_config = DisplayConfig::detect(f, disp.info(), &ui);
    println!("{}", display_config.log_line());
    let route_mode = if fill == EffectFill::FpgaHalf {
        Mode {
            hact: target.physical_w as u16,
            hbp: 3,
            vact: target.physical_h as u16,
            vbp: 2,
        }
    } else {
        Mode::framebuffer_sized(disp.width() as u16, disp.height() as u16)
    };
    let (xoff, yoff) = if fill == EffectFill::FpgaHalf {
        (
            Some(target.physical_x as i32),
            Some(target.physical_y as i32),
        )
    } else {
        (Some(0), Some(0))
    };
    let flag = f.fb_enable(
        0,
        disp.width() as u16,
        disp.height() as u16,
        route_mode,
        xoff,
        yoff,
        std::env::var_os("MISTER_DIRECT_VIDEO").is_some(),
    );
    f.set_audio_volume(0);
    println!(
        "fb routed (support_flag={flag}); native retro effect benchmark fpga_scale={}",
        fill.uses_fpga_scaler()
    );

    let needs_overlay = modes.contains(&EffectBenchMode::Overlay);
    let mut overlay_ctx = if needs_overlay {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let animation_clock = AnimationClock::from_env();
        slint::platform::set_platform(Box::new(MisterPlatform {
            window: window.clone(),
            start: Instant::now(),
            fixed_time: animation_clock.platform_time(),
        }))
        .expect("set_platform");
        let app = slint_ui::effect_hud::EffectHud::new().expect("EffectHud");
        let mister_ui = app.global::<slint_ui::effect_hud::MisterUi>();
        mister_ui.set_scale(SLINT_UI_SCALE);
        mister_ui.set_window_width(target.render_w as i32);
        mister_ui.set_window_height(target.render_h as i32);
        window.set_size(PhysicalSize::new(
            target.render_w as u32,
            target.render_h as u32,
        ));
        app.show().expect("show");
        Some((
            window,
            app,
            animation_clock,
            vec![Pixel(0); target.render_w * target.render_h],
        ))
    } else {
        None
    };

    let mut low = vec![0u32; size.w * size.h];
    for &kind in &effects {
        for &mode in &modes {
            run_one_effect_bench(
                &mut disp,
                &mut overlay_ctx,
                kind,
                mode,
                fill,
                size,
                target,
                secs,
                &mut low,
                fill == EffectFill::FpgaHalf,
            );
        }
    }
}

#[cfg(mister_ui_scope_launcher)]
pub fn run_effect_bench(_f: &mut Fpga) {
    eprintln!("effect-bench is unavailable in launcher-only UI builds");
    std::process::exit(2);
}

#[cfg(not(mister_ui_scope_launcher))]
fn run_one_effect_bench(
    disp: &mut Display,
    overlay_ctx: &mut Option<(
        Rc<MinimalSoftwareWindow>,
        slint_ui::effect_hud::EffectHud,
        AnimationClock,
        Vec<Pixel>,
    )>,
    kind: EffectKind,
    mode: EffectBenchMode,
    fill: EffectFill,
    size: EffectSize,
    target: EffectTarget,
    secs: u64,
    low: &mut [u32],
    direct_to_fb: bool,
) {
    let mut state = EffectState::new(kind, size);
    disp.clear(Pixel(0));
    let start = Instant::now();
    let mut frame = 0u64;
    let mut totals = EffectBenchTotals::default();
    let mut live_start = Instant::now();
    let mut live_frames = 0u64;
    let mut pacer = VsyncPacer::from_env();

    println!(
        "effect bench running {} mode={} fill={} internal={}x{} target={}x{}+{},{} scale={} secs={}...",
        kind.name(),
        mode.label(),
        fill.label(),
        size.w,
        size.h,
        target.physical_w,
        target.physical_h,
        target.physical_x,
        target.physical_y,
        target.scale,
        secs
    );
    while secs == 0 || start.elapsed().as_secs() < secs {
        let wall_start = Instant::now();
        let effect_us;
        let vsync_us;
        if direct_to_fb {
            let v0 = Instant::now();
            let _pace = pacer.wait();
            vsync_us = v0.elapsed().as_micros() as u64;
            let t0 = Instant::now();
            state.render(frame, disp.buffer_u32_mut());
            effect_us = t0.elapsed().as_micros() as u64;
        } else {
            let t0 = Instant::now();
            state.render(frame, low);
            effect_us = t0.elapsed().as_micros() as u64;
            let v0 = Instant::now();
            let _pace = pacer.wait();
            vsync_us = v0.elapsed().as_micros() as u64;
        }
        let mut slint_us = 0;
        let scale_copy_us;
        match mode {
            EffectBenchMode::Raw => {
                if direct_to_fb {
                    scale_copy_us = 0;
                } else {
                    let c0 = Instant::now();
                    disp.copy_u32_rect_scaled_at(
                        target.physical_x,
                        target.physical_y,
                        target.scale,
                        low,
                        size.w,
                        size.h,
                    );
                    scale_copy_us = c0.elapsed().as_micros() as u64;
                }
            }
            EffectBenchMode::Overlay => {
                let Some((window, app, animation_clock, full)) = overlay_ctx.as_mut() else {
                    eprintln!("effect-bench internal error: overlay context missing");
                    std::process::exit(1);
                };
                let c0 = Instant::now();
                scale_effect_to_pixels_fit(
                    low,
                    size.w,
                    size.h,
                    target.render_w,
                    target.render_h,
                    full,
                );
                let mut copy_acc = c0.elapsed().as_micros() as u64;
                app.set_effect_name(kind.name().into());
                app.set_mode_label("overlay".into());
                app.set_fps_label(format!("fps {live_frames}").into());
                app.set_timing_label(format!("fx {effect_us}us").into());
                app.set_frame_phase((frame % 16) as i32);
                update_slint_animations(animation_clock);
                window.request_redraw();
                let s0 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let _ = renderer.render(full, target.render_w);
                });
                slint_us = s0.elapsed().as_micros() as u64;
                let c1 = Instant::now();
                disp.copy_rect_scaled_at(
                    target.physical_x,
                    target.physical_y,
                    UiDisplay::for_framebuffer(disp.width(), disp.height()).fb_scale(),
                    full,
                    target.render_w,
                    target.render_h,
                );
                copy_acc += c1.elapsed().as_micros() as u64;
                scale_copy_us = copy_acc;
            }
        }
        let wall_us = wall_start.elapsed().as_micros() as u64;
        totals.record(effect_us, slint_us, scale_copy_us, vsync_us, wall_us);
        frame += 1;
        live_frames += 1;
        if live_start.elapsed().as_millis() >= 1000 {
            let nn = live_frames.max(1) as u128;
            println!(
                "  fps ~ {live_frames}  | effect {}us  slint {}us  scale-copy {}us  vsync-wait {}us  vsync hits={} timeouts={} fallback={} errors={} hz={:.2}",
                totals.effect_us / totals.frames.max(1) as u128,
                totals.slint_us / totals.frames.max(1) as u128,
                totals.scale_copy_us / totals.frames.max(1) as u128,
                totals.vsync_us / totals.frames.max(1) as u128,
                pacer.hits(),
                pacer.timeouts(),
                pacer.fallback_frames(),
                pacer.errors(),
                1_000_000.0 / pacer.period_us() as f64
            );
            let _ = nn;
            live_frames = 0;
            live_start = Instant::now();
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    let fps = if elapsed > 0.0 {
        totals.frames as f64 / elapsed
    } else {
        0.0
    };
    println!(
        "effect_bench_result\t{}\t{}\t{}\t{}\t{}x{}\t{}\t{}\t{:.1}\t{}\t{}\t{}\t{}\t{}",
        std::env::var("MISTER_EFFECT_BENCH_LABEL").unwrap_or_else(|_| "manual".into()),
        kind.name(),
        mode.label(),
        fill.label(),
        size.w,
        size.h,
        target.scale,
        totals.frames,
        fps,
        EffectBenchTotals::avg(totals.effect_us, totals.frames),
        EffectBenchTotals::avg(totals.slint_us, totals.frames),
        EffectBenchTotals::avg(totals.scale_copy_us, totals.frames),
        EffectBenchTotals::avg(totals.vsync_us, totals.frames),
        EffectBenchTotals::avg(totals.wall_us, totals.frames)
    );
    println!(
        "effect_bench_summary effect={} mode={} fill={} slow_frames={} elapsed={elapsed:.1}s vsync_hits={} vsync_timeouts={} fallback_frames={} vsync_errors={} max_miss_streak={} inferred_hz={:.2}",
        kind.name(),
        mode.label(),
        fill.label(),
        totals.slow_frames,
        pacer.hits(),
        pacer.timeouts(),
        pacer.fallback_frames(),
        pacer.errors(),
        pacer.max_miss_streak(),
        1_000_000.0 / pacer.period_us() as f64
    );
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

struct DirectBackbuffer {
    buffer: crate::fbwc::FbwcBuffer,
    routed: bool,
}

struct UiFrameTarget {
    cached: Vec<Pixel>,
    direct: Option<DirectBackbuffer>,
}

impl UiFrameTarget {
    fn cached(ui: &UiDisplay) -> Self {
        Self {
            cached: vec![Pixel(0); ui.render_w() * ui.render_h()],
            direct: None,
        }
    }

    fn open(ui: &UiDisplay) -> Self {
        if !crate::fbwc::requested_direct_mode() {
            println!("slint-render-target=cached");
            return Self::cached(ui);
        }

        match Self::open_direct(ui) {
            Ok(direct) => {
                println!("slint-render-target=fbwc-direct");
                Self {
                    cached: vec![Pixel(0); ui.render_w() * ui.render_h()],
                    direct: Some(direct),
                }
            }
            Err(e) => {
                eprintln!("fbwc-direct unavailable: {e}; using cached /dev/fb0 path");
                println!("slint-render-target=cached fallback=fbwc-direct");
                Self::cached(ui)
            }
        }
    }

    fn open_direct(ui: &UiDisplay) -> Result<DirectBackbuffer, String> {
        crate::fbwc::ensure_loaded()?;
        let mut buffer = crate::fbwc::FbwcBuffer::open_pixels(ui.render_w() * ui.render_h())
            .map_err(|e| format!("mmap {}: {e}", crate::fbwc::DEVICE_PATH))?;
        buffer.clear(Pixel(0));
        Ok(DirectBackbuffer {
            buffer,
            routed: false,
        })
    }

    fn label(&self) -> &'static str {
        match self.direct {
            Some(_) => "fbwc-direct",
            None => "cached",
        }
    }

    fn render_buffer_mut(&mut self) -> &mut [Pixel] {
        match self.direct.as_mut() {
            Some(direct) => direct.buffer.buffer_mut(),
            None => &mut self.cached,
        }
    }

    fn present_rect(
        &mut self,
        f: &mut Fpga,
        disp: &mut Display,
        ui: &UiDisplay,
        rect: DirtyRect,
    ) -> u32 {
        match self.direct.as_mut() {
            Some(direct) => {
                route_direct_backbuffer(f, ui, direct);
                rect.rows()
            }
            None => {
                copy_cached_rect(disp, ui, &self.cached, rect);
                rect.rows()
            }
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
        match self.direct.as_mut() {
            Some(direct) => {
                route_direct_backbuffer(f, ui, direct);
                y1.saturating_sub(y0) as u32
            }
            None => {
                copy_cached_rows(disp, ui, &self.cached, y0, y1);
                y1.saturating_sub(y0) as u32
            }
        }
    }

    fn copy_rect_from(
        &mut self,
        disp: &mut Display,
        ui: &UiDisplay,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Pixel],
    ) {
        match self.direct.as_mut() {
            Some(direct) => {
                direct
                    .buffer
                    .copy_rect_from(ui.render_w(), ui.render_h(), x, y, w, h, src);
            }
            None => disp.copy_rect_from(x, y, w, h, src),
        }
    }

    fn route_if_direct(&mut self, f: &mut Fpga, ui: &UiDisplay) {
        if let Some(direct) = self.direct.as_mut() {
            route_direct_backbuffer(f, ui, direct);
        }
    }

    fn shutdown_direct(&mut self, f: &mut Fpga, disp: &mut Display, ui: &UiDisplay) {
        let Some(direct) = self.direct.take() else {
            return;
        };
        let copy_len = self.cached.len().min(direct.buffer.buffer().len());
        self.cached[..copy_len].copy_from_slice(&direct.buffer.buffer()[..copy_len]);
        let flag = f.fb_enable(
            0,
            ui.fb_w() as u16,
            ui.fb_h() as u16,
            ui_fpga_scaled_mode(),
            Some(0),
            Some(0),
            std::env::var_os("MISTER_DIRECT_VIDEO").is_some(),
        );
        println!("fbwc-direct restored fb0 route support_flag={flag}");
        copy_cached_rows(disp, ui, &self.cached, 0, ui.render_h());
        drop(direct);
        crate::fbwc::unload_or_warn();
    }
}

fn route_direct_backbuffer(f: &mut Fpga, ui: &UiDisplay, direct: &mut DirectBackbuffer) {
    if direct.routed {
        return;
    }
    let flag = f.fb_enable(
        1,
        ui.fb_w() as u16,
        ui.fb_h() as u16,
        ui_fpga_scaled_mode(),
        Some(0),
        Some(0),
        std::env::var_os("MISTER_DIRECT_VIDEO").is_some(),
    );
    println!("fbwc-direct routed buffer 1 support_flag={flag}");
    direct.routed = true;
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
    if crate::fbwc::requested_direct_mode() {
        let probe = crate::fbwc::support_probe();
        println!("{}", probe.log_line());
        if !probe.ok {
            eprintln!(
                "fbwc-direct requested but unsupported; falling back to cached /dev/fb0 path"
            );
        } else {
            println!("fbwc-direct requested; target will load after display init");
        }
    } else {
        println!("ui_render_mode=cached");
    }

    let _vt = VtGraphicsGuard::enter_or_warn();

    println!("ui-fb-mode=temporary {UI_FB_W}x{UI_FB_H} fpga-scale=1920x1080 restore=on-drop");
    let _fb_mode_guard = match FbModeGuard::set_temporary(UI_FB_W, UI_FB_H) {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("failed to set temporary framebuffer mode for FPGA-scaled UI: {e}");
            std::process::exit(1);
        }
    };

    println!("display-open-path=temporary-fb-fpga-scale");
    let mut disp = match Display::open(UI_FB_W, UI_FB_H) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open display (/dev/fb0): {e}");
            std::process::exit(1);
        }
    };
    let ui = UiDisplay::for_framebuffer(disp.width(), disp.height());
    println!("{}", ui.log_line());
    disp.record_visual_sample("after_display_open_before_initial_route");
    let display_config = DisplayConfig::detect(f, disp.info(), &ui);
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
    let flag = f.fb_enable(
        0,
        disp.width() as u16,
        disp.height() as u16,
        route_mode,
        Some(0),
        Some(0),
        set_vga_fb,
    );
    boot_analytics::event(
        "initial_fb_enable_direct_done",
        format!("support_flag={flag}"),
    );
    disp.record_visual_sample("after_initial_route_before_slint_draw");
    f.set_audio_volume(0);
    boot_analytics::event("set_audio_volume", "attenuation=0");
    println!(
        "fb routed (support_flag={flag}); Slint software renderer (vsync, dirty-row copy, fpga_scale=true)"
    );

    if scene == "blend_velocity" {
        run_blend_velocity_loop(secs, &mut disp);
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
                target.shutdown_direct(f, &mut disp, &ui);
            });
        }
        #[cfg(not(mister_ui_scope_launcher))]
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
                target.shutdown_direct(f, &mut disp, &ui);
            });
        }
        #[cfg(not(mister_ui_scope_launcher))]
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
                target.shutdown_direct(f, &mut disp, &ui);
            });
        }
        #[cfg(not(mister_ui_scope_launcher))]
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
                target.shutdown_direct(f, &mut disp, &ui);
            });
        }
        #[cfg(not(mister_ui_scope_launcher))]
        "console_scroll" => {
            with_scene_app!(console_scroll::ConsoleScroll, &ui, &window, app, {
                app.show().expect("show");
                run_console_scroll_loop(secs, &ui, &mut disp, &window, app, &animation_clock);
            });
        }
        #[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
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
        "arcade_page" => {
            let pad = open_pads();
            with_scene_app!(arcade_page::ArcadePage, &ui, &window, app, {
                boot_analytics::event("app_show_attempt", "scene=arcade_page");
                app.show().expect("show");
                boot_analytics::event("app_show", "scene=arcade_page ok=1");
                window.request_redraw();
                run_arcade_page_loop(secs, &ui, &mut disp, f, &window, pad, app, &animation_clock);
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
                );
                target.shutdown_direct(f, &mut disp, &ui);
            });
        }
        _ => unreachable!(),
    }
}

fn open_pads() -> PadPool {
    for attempt in 0..60 {
        match PadPool::open_all() {
            Ok(p) => {
                if attempt > 0 {
                    println!("gamepad open ok after {attempt} retries");
                }
                return p;
            }
            Err(e) => {
                if attempt == 0 || attempt % 10 == 0 {
                    eprintln!("gamepad open attempt {attempt}: {e}");
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
    eprintln!("failed to open gamepad after 30s");
    std::process::exit(1);
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
    bridge.set_arcade_preview_box_x(ARCADE_PREVIEW_BOX_X as i32);
    bridge.set_arcade_preview_box_y(ARCADE_PREVIEW_BOX_Y as i32);
    bridge.set_arcade_preview_box_width(ARCADE_PREVIEW_BOX_W as i32);
    bridge.set_arcade_preview_box_height(ARCADE_PREVIEW_BOX_H as i32);
}

fn sync_arcade_page_geometry_bridge(bridge: &slint_ui::arcade_page::MisterBridge) {
    bridge.set_arcade_list_x(ARCADE_LIST_X as i32);
    bridge.set_arcade_list_y(ARCADE_LIST_Y as i32);
    bridge.set_arcade_list_width(ARCADE_LIST_W as i32);
    bridge.set_arcade_list_height(ARCADE_LIST_H as i32);
    bridge.set_arcade_preview_box_x(ARCADE_PREVIEW_BOX_X as i32);
    bridge.set_arcade_preview_box_y(ARCADE_PREVIEW_BOX_Y as i32);
    bridge.set_arcade_preview_box_width(ARCADE_PREVIEW_BOX_W as i32);
    bridge.set_arcade_preview_box_height(ARCADE_PREVIEW_BOX_H as i32);
}

fn sync_bridge(app: &slint_ui::controller::ControllerTest, pad: &PadPool) {
    sync_bridge_pad_controller(&app.global::<slint_ui::controller::MisterBridge>(), pad);
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
    if let Some(catalog) = catalog {
        let games = active_system_games(catalog, nav);
        let title = active_system(catalog, nav)
            .map(|system| system.title.clone())
            .unwrap_or_else(|| "Games".to_string());
        bridge.set_game_systems(slint_game_systems(&catalog.systems));
        bridge.set_active_system_title(title.into());
        bridge.set_arcade_games(slint_arcade_games(&games));
    }
    bridge.set_confirm_visible(nav.confirm_action.is_some());
    bridge.set_confirm_selected(nav.confirm_selected as i32);
    match nav.confirm_action {
        Some(launcher::ConfirmAction::ExitToMister) => {
            bridge.set_confirm_title("Exit to Mister".into());
            bridge.set_confirm_message("Use the stock MiSTer menu until reboot.".into());
            bridge.set_confirm_left_label("Exit to Mister".into());
            bridge.set_confirm_right_label("Return to Magik".into());
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
    bridge.set_loading_message(loading_message.into());
    bridge.set_loading_detail(loading_detail.into());
    if nav.screen == Screen::Arcade {
        let games = catalog
            .map(|catalog| active_system_games(catalog, nav))
            .unwrap_or_default();
        request_arcade_preview(&bridge, &games, nav.arcade.selected, preview);
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
    bridge.set_loading_message(loading_message.into());
    bridge.set_loading_detail(loading_detail.into());
    if nav.screen == Screen::Arcade {
        schedule_arcade_preview_for_game(&bridge, active_arcade_game(catalog, nav), preview);
    } else {
        preview.clear(&bridge);
    }
    bridge.set_setup_visible(setup.is_active());
}

fn png_to_slint_image(width: u32, height: u32, rgb: Vec<u8>) -> Image {
    let buffer = SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(&rgb, width, height);
    Image::from_rgb8(buffer)
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

const PREVIEW_IMAGE_CACHE_CAP: usize = 16;

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
}

impl PreviewImageCache {
    fn get(&mut self, path: &str) -> Option<PreviewImage> {
        let idx = self.entries.iter().position(|(p, _)| p == path)?;
        let (_, image) = self.entries.remove(idx)?;
        let out = image.clone();
        self.entries.push_back((path.to_string(), image));
        Some(out)
    }

    fn insert(&mut self, path: String, image: PreviewImage) {
        if let Some(idx) = self.entries.iter().position(|(p, _)| p == &path) {
            self.entries.remove(idx);
        }
        self.entries.push_back((path, image));
        while self.entries.len() > PREVIEW_IMAGE_CACHE_CAP {
            self.entries.pop_front();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewDisplaySize {
    w: u32,
    h: u32,
}

fn gcd_u32(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a.max(1)
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
    let g = gcd_u32(source_w, source_h);
    let base_w = source_w / g;
    let base_h = source_h / g;
    let base_area = base_w.saturating_mul(base_h).max(1);
    let by_w = pane_w / base_w.max(1);
    let by_h = pane_h / base_h.max(1);
    let by_area = ((max_area as f64) / (base_area as f64)).sqrt().floor() as u32;
    let max_n = by_w.min(by_h).min(by_area).max(1);
    let n = if max_n >= g {
        ((max_n / g).max(1)) * g
    } else {
        max_n
    };
    PreviewDisplaySize {
        w: base_w * n,
        h: base_h * n,
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

struct PreviewState {
    worker: PreviewWorker,
    last_preview_path: Option<String>,
    current_generation: u64,
    cache: PreviewImageCache,
    has_visible_preview: bool,
    visible_path: String,
}

impl PreviewState {
    fn new() -> Self {
        Self {
            worker: PreviewWorker::new(),
            last_preview_path: None,
            current_generation: 0,
            cache: PreviewImageCache::default(),
            has_visible_preview: false,
            visible_path: String::new(),
        }
    }

    fn clear(&mut self, bridge: &slint_ui::launcher::MisterBridge) {
        if self.last_preview_path.is_some()
            || self.current_generation != 0
            || self.has_visible_preview
        {
            self.last_preview_path = None;
            self.current_generation = 0;
            self.has_visible_preview = false;
            self.visible_path.clear();
            bridge.set_arcade_preview_has_image(false);
            bridge.set_arcade_preview_placeholder_visible(true);
            bridge.set_arcade_preview_status(PreviewStatus::Empty);
            bridge.set_arcade_preview_title("".into());
            clear_preview_image_bridge(bridge);
        }
    }
}

fn request_arcade_preview(
    bridge: &slint_ui::launcher::MisterBridge,
    games: &[ArcadeGameEntry],
    selected: usize,
    preview: &mut PreviewState,
) {
    request_arcade_preview_for_game(bridge, games.get(selected), preview);
}

fn request_arcade_preview_for_game(
    bridge: &slint_ui::launcher::MisterBridge,
    game: Option<&ArcadeGameEntry>,
    preview: &mut PreviewState,
) {
    let Some(game) = game else {
        preview.last_preview_path = None;
        preview.current_generation = 0;
        preview.has_visible_preview = false;
        preview.visible_path.clear();
        bridge.set_arcade_preview_placeholder_visible(true);
        bridge.set_arcade_preview_status(PreviewStatus::Empty);
        bridge.set_arcade_preview_title("".into());
        clear_preview_image_bridge(bridge);
        return;
    };
    bridge.set_arcade_preview_placeholder_visible(true);
    if preview.last_preview_path.as_deref() == Some(game.mra_path.as_str()) {
        return;
    }
    preview.last_preview_path = Some(game.mra_path.clone());

    bridge.set_arcade_preview_title(game.title.clone().into());
    if game.has_image {
        if let Some(image) = preview.cache.get(&game.image_path) {
            preview.current_generation = 0;
            preview.has_visible_preview = true;
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace cache_hit title={} path={}",
                    game.title, game.image_path
                );
            }
            if preview.visible_path != game.image_path {
                preview.visible_path = game.image_path.clone();
                apply_preview_image_bridge(bridge, &image);
            } else {
                apply_preview_image_bridge(bridge, &image);
            }
            return;
        }
        preview.current_generation = preview
            .worker
            .request(game.title.clone(), game.image_path.clone());
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
        return;
    }
    preview.current_generation = 0;
    preview.has_visible_preview = false;
    preview.visible_path.clear();
    bridge.set_arcade_preview_placeholder_visible(true);
    clear_preview_image_bridge(bridge);
    bridge.set_arcade_preview_status(PreviewStatus::Empty);
}

fn schedule_arcade_preview_for_game(
    bridge: &slint_ui::launcher::MisterBridge,
    game: Option<&ArcadeGameEntry>,
    preview: &mut PreviewState,
) -> bool {
    let Some(game) = game else {
        preview.clear(bridge);
        return true;
    };
    if preview.last_preview_path.as_deref() == Some(game.mra_path.as_str()) {
        return false;
    }
    request_arcade_preview_for_game(bridge, Some(game), preview);
    true
}

fn apply_ready_preview(app: &slint_ui::launcher::Launcher, preview: &mut PreviewState) -> bool {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let mut dirty = false;
    for result in preview.worker.drain() {
        if result.generation != preview.current_generation {
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace stale_result generation={} current_generation={} path={}",
                    result.generation, preview.current_generation, result.image_path
                );
            }
            continue;
        }
        bridge.set_arcade_preview_title(result.title.into());
        if let Some(image) = result.image {
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace apply generation={} age_us={} total_us={} read_us={} decode_us={} encoded_bytes={} decoded_bytes={} path={}",
                    result.generation,
                    result.request_age_us,
                    result.total_us,
                    result.read_us,
                    result.decode_us,
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
            let image = PreviewImage {
                image: png_to_slint_image(source_w, source_h, image.rgb),
                source_w,
                source_h,
                display_w: display.w,
                display_h: display.h,
            };
            let image_path = result.image_path;
            preview.cache.insert(image_path.clone(), image.clone());
            preview.has_visible_preview = true;
            preview.visible_path = image_path;
            apply_preview_image_bridge(&bridge, &image);
        } else {
            preview.has_visible_preview = false;
            preview.visible_path.clear();
            clear_preview_image_bridge(&bridge);
            bridge.set_arcade_preview_status(PreviewStatus::Empty);
        }
        dirty = true;
    }
    dirty
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

fn slint_game_systems(
    systems: &[arcade_catalog::GameSystemEntry],
) -> ModelRc<slint_ui::launcher::GameSystem> {
    let rows: Vec<slint_ui::launcher::GameSystem> = systems
        .iter()
        .map(|system| slint_ui::launcher::GameSystem {
            id: system.id.clone().into(),
            title: system.title.clone().into(),
            count: system.count as i32,
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn empty_arcade_catalog(root: &str) -> ArcadeCatalog {
    ArcadeCatalog {
        root: PathBuf::from(root),
        games: Vec::new(),
        systems: Vec::new(),
    }
}

fn active_system<'a>(
    catalog: &'a ArcadeCatalog,
    nav: &LauncherNav,
) -> Option<&'a arcade_catalog::GameSystemEntry> {
    catalog.systems.get(nav.selected)
}

fn active_system_games(catalog: &ArcadeCatalog, nav: &LauncherNav) -> Vec<ArcadeGameEntry> {
    active_system(catalog, nav)
        .map(|system| catalog.system_games(&system.id))
        .unwrap_or_default()
}

fn active_arcade_game<'a>(
    catalog: &'a ArcadeCatalog,
    nav: &LauncherNav,
) -> Option<&'a ArcadeGameEntry> {
    let system = active_system(catalog, nav)?;
    catalog.system_game_at(&system.id, nav.arcade.selected)
}

fn start_library_catalog_worker(
    root: String,
    cached_catalog_ready: bool,
) -> mpsc::Receiver<CatalogWorkerMessage> {
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
            let summary = match library_bench::refresh_default_sqlite_database(Some(&mut progress))
            {
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
            match library_bench::load_arcade_catalog_from_sqlite(&root) {
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
        summary: Option<library_bench::LibraryRefreshSummary>,
        load_us: u64,
    },
    Unchanged {
        summary: library_bench::LibraryRefreshSummary,
    },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LauncherBenchScenario {
    Idle,
    HomeNav,
    ListScroll,
    QuickTap,
    RapidTaps,
    HeldScroll,
    TurboHold,
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
            "quick-tap" | "quick_tap" => Some(Self::QuickTap),
            "rapid-taps" | "rapid_taps" => Some(Self::RapidTaps),
            "held-scroll" | "held_scroll" => Some(Self::HeldScroll),
            "turbo-hold" | "turbo_hold" => Some(Self::TurboHold),
            "preview" | "preview-changes" | "preview_changes" => Some(Self::PreviewChanges),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::HomeNav => "home-nav",
            Self::ListScroll => "list-scroll",
            Self::QuickTap => "quick-tap",
            Self::RapidTaps => "rapid-taps",
            Self::HeldScroll => "held-scroll",
            Self::TurboHold => "turbo-hold",
            Self::PreviewChanges => "preview-changes",
        }
    }

    fn period(self) -> Duration {
        match self {
            Self::Idle => Duration::MAX,
            Self::HomeNav => Duration::from_millis(300),
            Self::ListScroll => Duration::from_millis(120),
            Self::QuickTap | Self::RapidTaps | Self::HeldScroll | Self::TurboHold => Duration::ZERO,
            Self::PreviewChanges => Duration::from_millis(500),
        }
    }
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
        LauncherBenchScenario::ListScroll | LauncherBenchScenario::PreviewChanges => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            let selected = step % count;
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
    Some(catalog.system_game_count(&system.id))
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
                let region = renderer.render(target.render_buffer_mut(), ui.render_w());
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
                let region = renderer.render(target.render_buffer_mut(), ui.render_w());
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlendVelocityVariant {
    Baseline,
    CopyOnly,
    NoFade,
}

impl BlendVelocityVariant {
    fn from_env() -> Self {
        match std::env::var("MISTER_BLEND_BENCH_VARIANT")
            .unwrap_or_else(|_| "baseline".into())
            .to_ascii_lowercase()
            .replace('_', "-")
            .as_str()
        {
            "copy-only" | "copy" => Self::CopyOnly,
            "no-fade" | "nofade" | "body-only" => Self::NoFade,
            _ => Self::Baseline,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::CopyOnly => "copy-only",
            Self::NoFade => "no-fade",
        }
    }
}

#[derive(Default)]
struct BlendVelocityTotals {
    frames: u64,
    surface_us: u128,
    fade_blend_us: u128,
    fade_copy_us: u128,
    body_copy_us: u128,
    selection_copy_us: u128,
    vsync_us: u128,
    wall_us: u128,
    rows: u128,
    px: u128,
}

impl BlendVelocityTotals {
    fn record(&mut self, sample: BlendVelocitySample) {
        self.frames += 1;
        self.surface_us += sample.surface_us as u128;
        self.fade_blend_us += sample.fade_blend_us as u128;
        self.fade_copy_us += sample.fade_copy_us as u128;
        self.body_copy_us += sample.body_copy_us as u128;
        self.selection_copy_us += sample.selection_copy_us as u128;
        self.vsync_us += sample.vsync_us as u128;
        self.wall_us += sample.wall_us as u128;
        self.rows += sample.rows as u128;
        self.px += sample.px as u128;
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn avg(value: u128, frames: u64) -> u128 {
        if frames == 0 {
            0
        } else {
            value / frames as u128
        }
    }
}

#[derive(Clone, Copy)]
struct BlendVelocitySample {
    surface_us: u64,
    fade_blend_us: u64,
    fade_copy_us: u64,
    body_copy_us: u64,
    selection_copy_us: u64,
    vsync_us: u64,
    wall_us: u64,
    rows: u32,
    px: u32,
}

struct BlendVelocityBench {
    variant: BlendVelocityVariant,
    surface: Vec<Pixel>,
    fade_scratch: Vec<Pixel>,
    selection_horizontal: Vec<Pixel>,
    selection_vertical: Vec<Pixel>,
    surface_y: usize,
    visual_px: i32,
    px_per_frame: i32,
}

impl BlendVelocityBench {
    fn new(variant: BlendVelocityVariant) -> Self {
        let mut this = Self {
            variant,
            surface: vec![Pixel(0); ARCADE_LIST_W * ARCADE_LIST_H],
            fade_scratch: vec![Pixel(0); ARCADE_LIST_W * ARCADE_LIST_FADE_H],
            selection_horizontal: Vec::new(),
            selection_vertical: Vec::new(),
            surface_y: 0,
            visual_px: 0,
            px_per_frame: std::env::var("MISTER_BLEND_BENCH_PX_PER_FRAME")
                .ok()
                .and_then(|s| s.parse::<i32>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(6),
        };
        this.draw_full_surface();
        this
    }

    fn run_frame(&mut self, disp: &mut Display, pacer: &mut VsyncPacer) -> BlendVelocitySample {
        let frame_start = Instant::now();
        let surface_start = Instant::now();
        self.advance_surface();
        let surface_us = surface_start.elapsed().as_micros() as u64;

        let pace = pacer.wait();
        let vsync_us = pace.wait_us;

        let mut rows = 0u32;
        let mut px = 0u32;

        let fade_blend_start = Instant::now();
        let fade_blend_us = if self.variant == BlendVelocityVariant::Baseline {
            self.prepare_fade_bands()
        } else {
            0
        };
        let measured_fade_blend_us = fade_blend_start.elapsed().as_micros() as u64;
        let fade_blend_us = fade_blend_us.max(measured_fade_blend_us);

        let fade_copy_start = Instant::now();
        let mut fade_copy_us = 0u64;
        if self.variant != BlendVelocityVariant::NoFade {
            let fade_h = ARCADE_LIST_FADE_H.min(ARCADE_LIST_H / 2);
            let top_px = self.copy_top_fade_to_display(disp, fade_h);
            let bottom_px = self.copy_bottom_fade_to_display(disp, fade_h);
            rows += (fade_h * 2) as u32;
            px += top_px + bottom_px;
            fade_copy_us = fade_copy_start.elapsed().as_micros() as u64;
        }

        let body_copy_start = Instant::now();
        let fade_h = ARCADE_LIST_FADE_H.min(ARCADE_LIST_H / 2);
        let body_y = if self.variant == BlendVelocityVariant::NoFade {
            0
        } else {
            fade_h
        };
        let body_h = if self.variant == BlendVelocityVariant::NoFade {
            ARCADE_LIST_H
        } else {
            ARCADE_LIST_H - fade_h * 2
        };
        self.copy_viewport_band_to_display(disp, body_y, body_h);
        let body_copy_us = body_copy_start.elapsed().as_micros() as u64;
        rows += body_h as u32;
        px += (ARCADE_LIST_W * body_h) as u32;

        let selection_copy_start = Instant::now();
        self.copy_selection_frame_to_display(disp);
        let selection_copy_us = selection_copy_start.elapsed().as_micros() as u64;
        rows += (ARCADE_ROW_HEIGHT as u32) + 6;
        px += (ARCADE_LIST_W * 6 + ARCADE_ROW_HEIGHT as usize * 6) as u32;

        let wall_us = frame_start.elapsed().as_micros() as u64;
        BlendVelocitySample {
            surface_us,
            fade_blend_us,
            fade_copy_us,
            body_copy_us,
            selection_copy_us,
            vsync_us,
            wall_us,
            rows,
            px,
        }
    }

    fn advance_surface(&mut self) {
        let d = self.px_per_frame as usize;
        self.visual_px += self.px_per_frame;
        self.surface_y = (self.surface_y + d) % ARCADE_LIST_H;
        self.draw_band(ARCADE_LIST_H - d.min(ARCADE_LIST_H), d.min(ARCADE_LIST_H));
    }

    fn draw_full_surface(&mut self) {
        self.surface_y = 0;
        self.draw_band(0, ARCADE_LIST_H);
    }

    fn draw_band(&mut self, band_y: usize, band_h: usize) {
        if band_h == 0 {
            return;
        }
        let band_h = band_h.min(ARCADE_LIST_H - band_y);
        for row in 0..band_h {
            let viewport_y = band_y + row;
            let world_y = self.visual_px + viewport_y as i32;
            let row_idx = world_y.div_euclid(ARCADE_ROW_HEIGHT);
            let src_y = (world_y.rem_euclid(ARCADE_ROW_HEIGHT)) as usize;
            let bg = if row_idx % 2 == 0 {
                Pixel(0x001a1424)
            } else {
                Pixel(0x00150f20)
            };
            let border = Pixel(0x00251c34);
            let dst_y = (self.surface_y + viewport_y) % ARCADE_LIST_H;
            let line = &mut self.surface[dst_y * ARCADE_LIST_W..(dst_y + 1) * ARCADE_LIST_W];
            line.fill(bg);
            if src_y == 0 || src_y == ARCADE_ROW_HEIGHT as usize - 1 {
                line.fill(border);
            }
            for x in 12..ARCADE_LIST_W.min(320) {
                if (x + row_idx as usize * 17 + src_y) % 37 == 0 {
                    line[x] = Pixel(0x00e8e0f0);
                }
            }
        }
    }

    fn prepare_fade_bands(&mut self) -> u64 {
        let start = Instant::now();
        let fade_h = ARCADE_LIST_FADE_H.min(ARCADE_LIST_H / 2);
        self.fade_scratch
            .resize(ARCADE_LIST_W * fade_h * 2, Pixel(0));
        let surface = &self.surface;
        let surface_y = self.surface_y;
        let fade_scratch = &mut self.fade_scratch;
        for row in 0..fade_h {
            let alpha = fade_alpha(row, fade_h);
            let src_y = (surface_y + row) % ARCADE_LIST_H;
            let src = src_y * ARCADE_LIST_W;
            blend_row_towards(
                &surface[src..src + ARCADE_LIST_W],
                &mut fade_scratch[row * ARCADE_LIST_W..(row + 1) * ARCADE_LIST_W],
                alpha,
                ARCADE_LIST_FADE_COLOR,
            );
        }
        for row in 0..fade_h {
            let viewport_y = ARCADE_LIST_H - fade_h + row;
            let alpha = fade_alpha(fade_h - 1 - row, fade_h);
            let dst_row = fade_h + row;
            let src_y = (surface_y + viewport_y) % ARCADE_LIST_H;
            let src = src_y * ARCADE_LIST_W;
            blend_row_towards(
                &surface[src..src + ARCADE_LIST_W],
                &mut fade_scratch[dst_row * ARCADE_LIST_W..(dst_row + 1) * ARCADE_LIST_W],
                alpha,
                ARCADE_LIST_FADE_COLOR,
            );
        }
        start.elapsed().as_micros() as u64
    }

    fn copy_top_fade_to_display(&mut self, disp: &mut Display, fade_h: usize) -> u32 {
        if self.variant == BlendVelocityVariant::CopyOnly {
            self.copy_viewport_band_to_display(disp, 0, fade_h);
        } else {
            disp.copy_rect_from(
                ARCADE_LIST_X,
                ARCADE_LIST_Y,
                ARCADE_LIST_W,
                fade_h,
                &self.fade_scratch[..ARCADE_LIST_W * fade_h],
            );
        }
        (ARCADE_LIST_W * fade_h) as u32
    }

    fn copy_bottom_fade_to_display(&mut self, disp: &mut Display, fade_h: usize) -> u32 {
        if self.variant == BlendVelocityVariant::CopyOnly {
            self.copy_viewport_band_to_display(disp, ARCADE_LIST_H - fade_h, fade_h);
        } else {
            let offset = ARCADE_LIST_W * fade_h;
            disp.copy_rect_from(
                ARCADE_LIST_X,
                ARCADE_LIST_Y + ARCADE_LIST_H - fade_h,
                ARCADE_LIST_W,
                fade_h,
                &self.fade_scratch[offset..offset + ARCADE_LIST_W * fade_h],
            );
        }
        (ARCADE_LIST_W * fade_h) as u32
    }

    fn copy_viewport_band_to_display(&self, disp: &mut Display, viewport_y: usize, h: usize) {
        if h == 0 || viewport_y >= ARCADE_LIST_H {
            return;
        }
        let h = h.min(ARCADE_LIST_H - viewport_y);
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % ARCADE_LIST_H;
            let copy_h = (h - copied).min(ARCADE_LIST_H - src_y);
            let src = src_y * ARCADE_LIST_W;
            disp.copy_rect_from(
                ARCADE_LIST_X,
                ARCADE_LIST_Y + viewport_y + copied,
                ARCADE_LIST_W,
                copy_h,
                &self.surface[src..src + copy_h * ARCADE_LIST_W],
            );
            copied += copy_h;
        }
    }

    fn copy_selection_frame_to_display(&mut self, disp: &mut Display) {
        let rect = ArcadeListRenderer::selection_rect();
        let color = Pixel(0x0006d6a0);
        let thickness = 3usize;
        let h = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.selection_horizontal
            .resize(ARCADE_LIST_W * thickness, color);
        self.selection_horizontal.fill(color);
        disp.copy_rect_from(
            rect.x0,
            rect.y0,
            ARCADE_LIST_W,
            thickness,
            &self.selection_horizontal,
        );
        disp.copy_rect_from(
            rect.x0,
            rect.y1.saturating_sub(thickness),
            ARCADE_LIST_W,
            thickness,
            &self.selection_horizontal,
        );
        self.selection_vertical.resize(thickness * h, color);
        self.selection_vertical.fill(color);
        disp.copy_rect_from(rect.x0, rect.y0, thickness, h, &self.selection_vertical);
        disp.copy_rect_from(
            rect.x1.saturating_sub(thickness),
            rect.y0,
            thickness,
            h,
            &self.selection_vertical,
        );
    }
}

fn run_blend_velocity_loop(secs: u64, disp: &mut Display) {
    let variant = BlendVelocityVariant::from_env();
    let mut bench = BlendVelocityBench::new(variant);
    let mut pacer = VsyncPacer::from_env();
    let cpu = cpu_profile::start();
    let start = Instant::now();
    let mut frames = 0u64;
    let mut totals = BlendVelocityTotals::default();
    let mut window_totals = BlendVelocityTotals::default();
    let mut fps_window_start = Instant::now();
    let trace_path = std::env::var("MISTER_BLEND_BENCH_TRACE").ok();
    let mut trace = trace_path.as_ref().and_then(|path| {
        let mut file = std::fs::File::create(path)
            .map_err(|e| eprintln!("blend_velocity trace: create {path} failed: {e}"))
            .ok()?;
        std::io::Write::write_all(
            &mut file,
            b"frame\telapsed_us\tvariant\tvisual_px\tpx_per_frame\tsurface_us\tfade_blend_us\tfade_copy_us\tbody_copy_us\tselection_copy_us\tvsync_us\twall_us\trows\tpx\n",
        )
        .map_err(|e| eprintln!("blend_velocity trace: header write failed: {e}"))
        .ok()?;
        println!("blend_velocity_trace={path}");
        Some(file)
    });

    println!(
        "blend_velocity running variant={} px_per_frame={} trace={} secs={}",
        variant.label(),
        bench.px_per_frame,
        trace_path.as_deref().unwrap_or("off"),
        secs
    );

    while secs == 0 || start.elapsed().as_secs() < secs {
        let sample = bench.run_frame(disp, &mut pacer);
        frames += 1;
        totals.record(sample);
        window_totals.record(sample);
        if let Some(file) = trace.as_mut() {
            let _ = std::io::Write::write_fmt(
                file,
                format_args!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    frames,
                    start.elapsed().as_micros(),
                    variant.label(),
                    bench.visual_px,
                    bench.px_per_frame,
                    sample.surface_us,
                    sample.fade_blend_us,
                    sample.fade_copy_us,
                    sample.body_copy_us,
                    sample.selection_copy_us,
                    sample.vsync_us,
                    sample.wall_us,
                    sample.rows,
                    sample.px
                ),
            );
        }

        if fps_window_start.elapsed().as_millis() >= 1000 {
            let n = window_totals.frames.max(1);
            println!(
                "blend_velocity fps ~ {} variant={} surface {}us fade-blend {}us fade-copy {}us body-copy {}us selection-copy {}us vsync {}us wall {}us rows {} px {}",
                window_totals.frames,
                variant.label(),
                BlendVelocityTotals::avg(window_totals.surface_us, n),
                BlendVelocityTotals::avg(window_totals.fade_blend_us, n),
                BlendVelocityTotals::avg(window_totals.fade_copy_us, n),
                BlendVelocityTotals::avg(window_totals.body_copy_us, n),
                BlendVelocityTotals::avg(window_totals.selection_copy_us, n),
                BlendVelocityTotals::avg(window_totals.vsync_us, n),
                BlendVelocityTotals::avg(window_totals.wall_us, n),
                BlendVelocityTotals::avg(window_totals.rows, n),
                BlendVelocityTotals::avg(window_totals.px, n),
            );
            window_totals.reset();
            fps_window_start = Instant::now();
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    let n = totals.frames.max(1);
    println!(
        "blend_velocity_result variant={} frames={} elapsed={elapsed:.1}s fps={:.1} surface_us={} fade_blend_us={} fade_copy_us={} body_copy_us={} selection_copy_us={} vsync_us={} wall_us={} rows={} px={}",
        variant.label(),
        frames,
        frames as f64 / elapsed,
        BlendVelocityTotals::avg(totals.surface_us, n),
        BlendVelocityTotals::avg(totals.fade_blend_us, n),
        BlendVelocityTotals::avg(totals.fade_copy_us, n),
        BlendVelocityTotals::avg(totals.body_copy_us, n),
        BlendVelocityTotals::avg(totals.selection_copy_us, n),
        BlendVelocityTotals::avg(totals.vsync_us, n),
        BlendVelocityTotals::avg(totals.wall_us, n),
        BlendVelocityTotals::avg(totals.rows, n),
        BlendVelocityTotals::avg(totals.px, n),
    );
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Err(e) = cpu_profile::finish(cpu) {
        eprintln!("{e}");
    }
}

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
const VIDEO_IMAGE_RECT: DirtyRect = DirtyRect {
    x0: 40,
    y0: 158,
    x1: 360,
    y1: 382,
};

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
#[derive(Default)]
struct VideoFramePhases {
    frame_updated: bool,
    decode_us: u64,
    recv_us: u64,
    image_us: u64,
    blit_us: u64,
    audio_us: u64,
}

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
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

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
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

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoRenderMode {
    SlintImage,
    DirectBlit,
}

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
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

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
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

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
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

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
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

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
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

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
#[derive(Default)]
struct AudioWindowStats {
    write_us: u128,
    requested_frames: u128,
    written_frames: u128,
    underruns: u64,
    loop_count: u64,
}

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
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

#[cfg(all(feature = "video", not(mister_ui_scope_launcher)))]
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

#[cfg(not(mister_ui_scope_launcher))]
const CONSOLE_LIST_X: usize = 40;
#[cfg(not(mister_ui_scope_launcher))]
const CONSOLE_LIST_Y: usize = 116;
#[cfg(not(mister_ui_scope_launcher))]
const CONSOLE_LIST_W: usize = 880;
#[cfg(not(mister_ui_scope_launcher))]
const CONSOLE_LIST_H: usize = 356;
#[cfg(not(mister_ui_scope_launcher))]
const CONSOLE_ROW_H: usize = 44;
#[cfg(not(mister_ui_scope_launcher))]
const CONSOLE_FONT_PX: f32 = 16.0;
#[cfg(not(mister_ui_scope_launcher))]
const CONSOLE_TRACE_DEFAULT_PATH: &str = "/tmp/mister-magik-console-scroll-trace.tsv";

#[cfg(not(mister_ui_scope_launcher))]
struct ConsoleScrollTrace {
    file: File,
    start: Instant,
    frame: u64,
    fb_sample_step: usize,
    copy_budget_us: u64,
}

#[cfg(not(mister_ui_scope_launcher))]
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

#[cfg(not(mister_ui_scope_launcher))]
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

#[cfg(not(mister_ui_scope_launcher))]
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
}

#[cfg(not(mister_ui_scope_launcher))]
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

#[cfg(not(mister_ui_scope_launcher))]
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

#[cfg(not(mister_ui_scope_launcher))]
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

#[cfg(not(mister_ui_scope_launcher))]
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

struct ConsoleGlyph {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    advance: i32,
    data: Vec<u8>,
}

struct ConsoleFont {
    font: swash::FontRef<'static>,
    scale_context: swash::scale::ScaleContext,
    glyphs: HashMap<char, ConsoleGlyph>,
    pixel_size: f32,
    units_per_em: f32,
}

impl ConsoleFont {
    fn new(pixel_size: f32) -> Self {
        let data = include_bytes!("../ui/fonts/PressStart2P-Regular.ttf");
        let font = swash::FontRef::from_index(data, 0).expect("PressStart2P-Regular.ttf");
        let units_per_em = font.metrics(&[]).units_per_em as f32;
        Self {
            font,
            scale_context: swash::scale::ScaleContext::new(),
            glyphs: HashMap::new(),
            pixel_size,
            units_per_em,
        }
    }

    fn glyph(&mut self, ch: char) -> Option<&ConsoleGlyph> {
        if !self.glyphs.contains_key(&ch) {
            let glyph_id = self.font.charmap().map(ch);
            let advance = if glyph_id == 0 {
                (self.pixel_size * 0.75) as i32
            } else {
                let scale = self.pixel_size / self.units_per_em;
                (self.font.glyph_metrics(&[]).advance_width(glyph_id) * scale) as i32
            };
            let glyph = if glyph_id == 0 || ch == ' ' {
                ConsoleGlyph {
                    left: 0,
                    top: 0,
                    width: 0,
                    height: 0,
                    advance,
                    data: Vec::new(),
                }
            } else {
                let mut scaler = self
                    .scale_context
                    .builder(self.font)
                    .size(self.pixel_size)
                    .build();
                let image = swash::scale::Render::new(&[swash::scale::Source::Outline])
                    .format(swash::zeno::Format::Alpha)
                    .render(&mut scaler, glyph_id)?;
                ConsoleGlyph {
                    left: image.placement.left,
                    top: image.placement.top,
                    width: image.placement.width as usize,
                    height: image.placement.height as usize,
                    advance,
                    data: image.data,
                }
            };
            self.glyphs.insert(ch, glyph);
        }
        self.glyphs.get(&ch)
    }

    fn draw_text_clipped(
        &mut self,
        dst: &mut [Pixel],
        stride: usize,
        clip_w: usize,
        clip_y: usize,
        clip_h: usize,
        x: isize,
        baseline_y: isize,
        text: &str,
        color: Pixel,
    ) {
        let mut pen_x = x;
        for ch in text.chars() {
            let Some(glyph) = self.glyph(ch) else {
                continue;
            };
            let gx0 = pen_x + glyph.left as isize;
            let gy0 = baseline_y - glyph.top as isize;
            for gy in 0..glyph.height {
                let dy = gy0 + gy as isize;
                if dy < clip_y as isize || dy >= (clip_y + clip_h) as isize {
                    continue;
                }
                for gx in 0..glyph.width {
                    let dx = gx0 + gx as isize;
                    if dx < 0 || dx >= clip_w as isize {
                        continue;
                    }
                    let alpha = glyph.data[gy * glyph.width + gx];
                    if alpha >= 128 {
                        dst[dy as usize * stride + dx as usize] = color;
                    }
                }
            }
            pen_x += glyph.advance as isize;
        }
    }
}

const ARCADE_LIST_X: usize = 8;
const ARCADE_LIST_Y: usize = 56;
const ARCADE_LIST_W: usize = 464;
const ARCADE_LIST_H: usize = 384;
const ARCADE_LIST_FONT_PX: f32 = 16.0;
const ARCADE_LIST_META_FONT_PX: f32 = 8.0;
const ARCADE_LIST_FADE_H: usize = 48;
const ARCADE_LIST_FADE_MAX_ALPHA: u32 = 256;
const ARCADE_LIST_FADE_COLOR: Pixel = Pixel(0x00120d1a);

struct ArcadeListRenderer {
    title_font: ConsoleFont,
    meta_font: ConsoleFont,
    row_cache: HashMap<usize, CachedArcadeRow>,
    surface: Vec<Pixel>,
    band_scratch: Vec<Pixel>,
    fade_scratch: Vec<Pixel>,
    selection_horizontal: Vec<Pixel>,
    selection_vertical: Vec<Pixel>,
    surface_y: usize,
    last_draw: Option<ArcadeListDrawKey>,
}

struct CachedArcadeRow {
    title: String,
    pixels: Vec<Pixel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArcadeListDrawKey {
    len: usize,
    visual_px: i32,
    anchor_system_id: String,
    anchor_mra_path: String,
    anchor_title: String,
}

enum ArcadeListUpdate {
    Full(DirtyRect),
}

impl ArcadeListRenderer {
    fn new() -> Self {
        Self {
            title_font: ConsoleFont::new(ARCADE_LIST_FONT_PX),
            meta_font: ConsoleFont::new(ARCADE_LIST_META_FONT_PX),
            row_cache: HashMap::new(),
            surface: vec![Pixel(0); ARCADE_LIST_W * ARCADE_LIST_H],
            band_scratch: Vec::new(),
            fade_scratch: Vec::new(),
            selection_horizontal: Vec::new(),
            selection_vertical: Vec::new(),
            surface_y: 0,
            last_draw: None,
        }
    }

    fn dirty_rect() -> DirtyRect {
        DirtyRect {
            x0: ARCADE_LIST_X,
            y0: ARCADE_LIST_Y,
            x1: ARCADE_LIST_X + ARCADE_LIST_W,
            y1: ARCADE_LIST_Y + ARCADE_LIST_H,
        }
    }

    fn draw(
        &mut self,
        games: &[ArcadeGameEntry],
        visual_index: f32,
        force: bool,
    ) -> Option<ArcadeListUpdate> {
        let visual_px = (visual_index * ARCADE_ROW_HEIGHT as f32).round() as i32;
        let anchor = visual_index
            .round()
            .clamp(0.0, games.len().saturating_sub(1) as f32) as usize;
        let previous = self.last_draw.clone();
        let key = ArcadeListDrawKey {
            len: games.len(),
            visual_px,
            anchor_system_id: games
                .get(anchor)
                .map(|game| game.system_id.clone())
                .unwrap_or_default(),
            anchor_mra_path: games
                .get(anchor)
                .map(|game| game.mra_path.clone())
                .unwrap_or_default(),
            anchor_title: games
                .get(anchor)
                .map(|game| game.title.clone())
                .unwrap_or_default(),
        };
        if !force && self.last_draw.as_ref() == Some(&key) {
            return None;
        }
        let same_game_set = previous
            .as_ref()
            .is_some_and(|previous| previous.len == key.len);
        self.last_draw = Some(key);
        let content_delta = previous
            .as_ref()
            .map(|previous| previous.visual_px - visual_px)
            .unwrap_or(0);
        if force || previous.is_none() || !same_game_set || games.is_empty() {
            self.surface_y = 0;
            self.draw_content_band(games, visual_index, 0, ARCADE_LIST_H);
        } else if content_delta == 0 {
        } else if content_delta.unsigned_abs() as usize >= ARCADE_LIST_H {
            self.surface_y = 0;
            self.draw_content_band(games, visual_index, 0, ARCADE_LIST_H);
        } else if content_delta < 0 {
            let d = content_delta.unsigned_abs() as usize;
            self.surface_y = (self.surface_y + d) % ARCADE_LIST_H;
            self.draw_content_band(games, visual_index, ARCADE_LIST_H - d, d);
        } else {
            let d = content_delta as usize;
            self.surface_y = (self.surface_y + ARCADE_LIST_H - d) % ARCADE_LIST_H;
            self.draw_content_band(games, visual_index, 0, d);
        }
        if force {
            return Some(ArcadeListUpdate::Full(Self::dirty_rect()));
        }
        if previous.is_none() {
            return Some(ArcadeListUpdate::Full(Self::dirty_rect()));
        }
        if !same_game_set {
            return Some(ArcadeListUpdate::Full(Self::dirty_rect()));
        }
        if content_delta == 0 || content_delta.unsigned_abs() as usize >= ARCADE_LIST_H {
            return Some(ArcadeListUpdate::Full(Self::dirty_rect()));
        }
        Some(ArcadeListUpdate::Full(Self::dirty_rect()))
    }

    fn selection_rect() -> DirtyRect {
        let y = Self::selection_y();
        DirtyRect {
            x0: ARCADE_LIST_X,
            y0: ARCADE_LIST_Y + y,
            x1: ARCADE_LIST_X + ARCADE_LIST_W,
            y1: ARCADE_LIST_Y + y + ARCADE_ROW_HEIGHT as usize,
        }
    }

    fn selection_y() -> usize {
        let row_h = ARCADE_ROW_HEIGHT as usize;
        let visible_rows = (ARCADE_LIST_H / row_h).max(1);
        (visible_rows / 2) * row_h
    }

    fn draw_content_band(
        &mut self,
        games: &[ArcadeGameEntry],
        visual_index: f32,
        band_y: usize,
        band_h: usize,
    ) {
        if band_h == 0 || band_y >= ARCADE_LIST_H {
            return;
        }
        let band_h = band_h.min(ARCADE_LIST_H - band_y);
        let mut band = std::mem::take(&mut self.band_scratch);
        band.resize(ARCADE_LIST_W * band_h, Pixel(0x00120d1a));
        band.fill(Pixel(0x00120d1a));
        if games.is_empty() {
            self.meta_font.draw_text_clipped(
                &mut band,
                ARCADE_LIST_W,
                ARCADE_LIST_W,
                0,
                band_h,
                96,
                (ARCADE_LIST_H / 2).saturating_sub(band_y) as isize,
                "NO GAMES",
                Pixel(0x00706080),
            );
            self.copy_band_to_surface(&band, band_y, band_h);
            self.band_scratch = band;
            return;
        }
        let row_h = ARCADE_ROW_HEIGHT as isize;
        let local_anchor_y = Self::selection_y() as isize;
        let first = ((visual_index.floor() as isize) - 7).max(0) as usize;
        let last = ((visual_index.ceil() as isize) + 8).max(0) as usize;
        let end = last.min(games.len().saturating_sub(1));
        for idx in first..=end {
            let y =
                local_anchor_y + ((idx as f32 - visual_index) * ARCADE_ROW_HEIGHT as f32) as isize;
            let clip_y0 = y.max(band_y as isize);
            let clip_y1 = (y + row_h).min((band_y + band_h) as isize);
            if clip_y1 <= clip_y0 {
                continue;
            }
            self.blit_cached_row_to_band(&mut band, band_h, band_y, &games[idx].title, idx, y);
        }
        self.copy_band_to_surface(&band, band_y, band_h);
        self.band_scratch = band;
    }

    fn blit_cached_row_to_band(
        &mut self,
        band: &mut [Pixel],
        band_h: usize,
        band_y: usize,
        title: &str,
        idx: usize,
        y: isize,
    ) {
        let needs_render = self
            .row_cache
            .get(&idx)
            .is_none_or(|cached| cached.title != title);
        if needs_render {
            if self.row_cache.len() > 128 {
                self.row_cache.clear();
            }
            let row = self.render_row(title, idx);
            self.row_cache.insert(
                idx,
                CachedArcadeRow {
                    title: title.to_string(),
                    pixels: row,
                },
            );
        }
        let row = &self.row_cache.get(&idx).expect("row cache insert").pixels;
        let row_h = ARCADE_ROW_HEIGHT as isize;
        let clip_y0 = y.max(band_y as isize);
        let clip_y1 = (y + row_h).min((band_y + band_h) as isize);
        if clip_y1 <= clip_y0 {
            return;
        }
        let copy_h = (clip_y1 - clip_y0) as usize;
        let src_y = (clip_y0 - y) as usize;
        let dst_y = (clip_y0 as usize).saturating_sub(band_y);
        for row_y in 0..copy_h {
            let src = (src_y + row_y) * ARCADE_LIST_W;
            let dst = (dst_y + row_y) * ARCADE_LIST_W;
            band[dst..dst + ARCADE_LIST_W].copy_from_slice(&row[src..src + ARCADE_LIST_W]);
        }
    }

    fn copy_band_to_surface(&mut self, band: &[Pixel], band_y: usize, band_h: usize) {
        for row in 0..band_h {
            let src = row * ARCADE_LIST_W;
            let dst_y = (self.surface_y + band_y + row) % ARCADE_LIST_H;
            let dst = dst_y * ARCADE_LIST_W;
            self.surface[dst..dst + ARCADE_LIST_W].copy_from_slice(&band[src..src + ARCADE_LIST_W]);
        }
    }

    fn copy_layer_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut Display,
        ui: &UiDisplay,
    ) {
        let fade_h = ARCADE_LIST_FADE_H.min(ARCADE_LIST_H / 2);
        self.copy_fade_to_target(target, disp, ui);
        self.copy_viewport_band_to_target(target, disp, ui, fade_h, ARCADE_LIST_H - fade_h * 2);
        self.copy_selection_frame_to_target(target, disp, ui);
    }

    fn copy_viewport_band_to_target(
        &self,
        target: &mut UiFrameTarget,
        disp: &mut Display,
        ui: &UiDisplay,
        viewport_y: usize,
        h: usize,
    ) {
        if h == 0 || viewport_y >= ARCADE_LIST_H {
            return;
        }
        let h = h.min(ARCADE_LIST_H - viewport_y);
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % ARCADE_LIST_H;
            let copy_h = (h - copied).min(ARCADE_LIST_H - src_y);
            let src = src_y * ARCADE_LIST_W;
            target.copy_rect_from(
                disp,
                ui,
                ARCADE_LIST_X,
                ARCADE_LIST_Y + viewport_y + copied,
                ARCADE_LIST_W,
                copy_h,
                &self.surface[src..src + copy_h * ARCADE_LIST_W],
            );
            copied += copy_h;
        }
    }

    fn surface_row(&self, viewport_y: usize) -> &[Pixel] {
        let src_y = (self.surface_y + viewport_y) % ARCADE_LIST_H;
        let src = src_y * ARCADE_LIST_W;
        &self.surface[src..src + ARCADE_LIST_W]
    }

    fn copy_fade_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut Display,
        ui: &UiDisplay,
    ) {
        let fade_h = ARCADE_LIST_FADE_H.min(ARCADE_LIST_H / 2);
        let mut band = std::mem::take(&mut self.fade_scratch);
        band.resize(ARCADE_LIST_W * fade_h, Pixel(0));
        for row in 0..fade_h {
            let alpha = fade_alpha(row, fade_h);
            blend_row_towards(
                self.surface_row(row),
                &mut band[row * ARCADE_LIST_W..(row + 1) * ARCADE_LIST_W],
                alpha,
                ARCADE_LIST_FADE_COLOR,
            );
        }
        target.copy_rect_from(
            disp,
            ui,
            ARCADE_LIST_X,
            ARCADE_LIST_Y,
            ARCADE_LIST_W,
            fade_h,
            &band,
        );

        for row in 0..fade_h {
            let viewport_y = ARCADE_LIST_H - fade_h + row;
            let alpha = fade_alpha(fade_h - 1 - row, fade_h);
            blend_row_towards(
                self.surface_row(viewport_y),
                &mut band[row * ARCADE_LIST_W..(row + 1) * ARCADE_LIST_W],
                alpha,
                ARCADE_LIST_FADE_COLOR,
            );
        }
        target.copy_rect_from(
            disp,
            ui,
            ARCADE_LIST_X,
            ARCADE_LIST_Y + ARCADE_LIST_H - fade_h,
            ARCADE_LIST_W,
            fade_h,
            &band,
        );
        self.fade_scratch = band;
    }

    fn copy_selection_frame_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut Display,
        ui: &UiDisplay,
    ) {
        let rect = Self::selection_rect();
        let color = Pixel(0x0006d6a0);
        let thickness = 3usize;
        let h = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.selection_horizontal
            .resize(ARCADE_LIST_W * thickness, color);
        self.selection_horizontal.fill(color);
        target.copy_rect_from(
            disp,
            ui,
            rect.x0,
            rect.y0,
            ARCADE_LIST_W,
            thickness,
            &self.selection_horizontal,
        );
        target.copy_rect_from(
            disp,
            ui,
            rect.x0,
            rect.y1.saturating_sub(thickness),
            ARCADE_LIST_W,
            thickness,
            &self.selection_horizontal,
        );
        self.selection_vertical.resize(thickness * h, color);
        self.selection_vertical.fill(color);
        target.copy_rect_from(
            disp,
            ui,
            rect.x0,
            rect.y0,
            thickness,
            h,
            &self.selection_vertical,
        );
        target.copy_rect_from(
            disp,
            ui,
            rect.x1.saturating_sub(thickness),
            rect.y0,
            thickness,
            h,
            &self.selection_vertical,
        );
    }

    fn render_row(&mut self, title: &str, idx: usize) -> Vec<Pixel> {
        let mut row = vec![Pixel(0); ARCADE_LIST_W * ARCADE_ROW_HEIGHT as usize];
        draw_arcade_row_background(&mut row, idx);
        let title = clipped_title(title, 30);
        self.title_font.draw_text_clipped(
            &mut row,
            ARCADE_LIST_W,
            ARCADE_LIST_W,
            0,
            ARCADE_ROW_HEIGHT as usize,
            12,
            30,
            &title,
            Pixel(0x00e8e0f0),
        );
        row
    }
}

fn draw_arcade_row_background(row: &mut [Pixel], idx: usize) {
    let bg = if idx % 2 == 0 {
        Pixel(0x001a1424)
    } else {
        Pixel(0x00150f20)
    };
    let border = Pixel(0x00251c34);
    for row_y in 0..ARCADE_ROW_HEIGHT as isize {
        let dy = row_y as usize;
        let line = &mut row[dy * ARCADE_LIST_W..(dy + 1) * ARCADE_LIST_W];
        for px in line.iter_mut() {
            *px = bg;
        }
        if row_y == 0 || row_y == ARCADE_ROW_HEIGHT as isize - 1 {
            for px in line.iter_mut() {
                *px = border;
            }
        }
    }
}

fn fade_alpha(row_from_edge: usize, fade_h: usize) -> u32 {
    if fade_h <= 1 {
        return ARCADE_LIST_FADE_MAX_ALPHA;
    }
    let inv = (fade_h - 1 - row_from_edge) as u32;
    (ARCADE_LIST_FADE_MAX_ALPHA * inv) / (fade_h - 1) as u32
}

fn blend_row_towards(src: &[Pixel], dst: &mut [Pixel], alpha: u32, color: Pixel) {
    #[cfg(all(target_arch = "arm", target_feature = "neon"))]
    let processed = unsafe { blend_row_towards_neon(src, dst, alpha, color) };
    #[cfg(not(all(target_arch = "arm", target_feature = "neon")))]
    let processed = 0usize;

    let inv = 256 - alpha;
    let cr = (color.0 >> 16) & 0xff;
    let cg = (color.0 >> 8) & 0xff;
    let cb = color.0 & 0xff;
    for (src, dst) in src[processed..].iter().zip(dst[processed..].iter_mut()) {
        let sr = (src.0 >> 16) & 0xff;
        let sg = (src.0 >> 8) & 0xff;
        let sb = src.0 & 0xff;
        let r = (sr * inv + cr * alpha) >> 8;
        let g = (sg * inv + cg * alpha) >> 8;
        let b = (sb * inv + cb * alpha) >> 8;
        *dst = Pixel((r << 16) | (g << 8) | b);
    }
}

#[cfg(all(target_arch = "arm", target_feature = "neon"))]
unsafe fn blend_row_towards_neon(
    src: &[Pixel],
    dst: &mut [Pixel],
    alpha: u32,
    color: Pixel,
) -> usize {
    use core::arch::arm::{
        vaddq_u32, vandq_u32, vdupq_n_u32, vld1q_u32, vmulq_u32, vorrq_u32, vshrq_n_u32, vst1q_u32,
    };

    let len = src.len().min(dst.len());
    let inv = 256 - alpha;
    let rb_mask = vdupq_n_u32(0x00ff00ff);
    let g_mask = vdupq_n_u32(0x0000ff00);
    let inv_v = vdupq_n_u32(inv);
    let alpha_v = vdupq_n_u32(alpha);
    let color_v = vdupq_n_u32(color.0);
    let color_rb = vmulq_u32(vandq_u32(color_v, rb_mask), alpha_v);
    let color_g = vmulq_u32(vandq_u32(color_v, g_mask), alpha_v);
    let src_ptr = src.as_ptr().cast::<u32>();
    let dst_ptr = dst.as_mut_ptr().cast::<u32>();
    let mut i = 0usize;
    while i + 4 <= len {
        let px = vld1q_u32(src_ptr.add(i));
        let rb = vshrq_n_u32(
            vaddq_u32(vmulq_u32(vandq_u32(px, rb_mask), inv_v), color_rb),
            8,
        );
        let g = vshrq_n_u32(
            vaddq_u32(vmulq_u32(vandq_u32(px, g_mask), inv_v), color_g),
            8,
        );
        let out = vorrq_u32(vandq_u32(rb, rb_mask), vandq_u32(g, g_mask));
        vst1q_u32(dst_ptr.add(i), out);
        i += 4;
    }
    i
}

fn clipped_title(title: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in title.chars().take(max_chars) {
        out.push(ch);
    }
    if title.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn fill_cached_rect(
    cached: &mut [Pixel],
    render_w: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: Pixel,
) {
    for row in 0..h {
        let dst = (y + row) * render_w + x;
        cached[dst..dst + w].fill(color);
    }
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
        f.fb_enable(
            0,
            ui.fb_w() as u16,
            ui.fb_h() as u16,
            ui_fpga_scaled_mode(),
            Some(0),
            Some(0),
            std::env::var_os("MISTER_DIRECT_VIDEO").is_some(),
        );
        *spawned_mister = false;
    }
}

fn slint_arcade_page_games(
    games: &[ArcadeGameEntry],
) -> ModelRc<slint_ui::arcade_page::ArcadeGame> {
    let rows: Vec<slint_ui::arcade_page::ArcadeGame> = games
        .iter()
        .map(|g| slint_ui::arcade_page::ArcadeGame {
            title: g.title.clone().into(),
            mra_path: g.mra_path.clone().into(),
            image_path: g.image_path.clone().into(),
            has_image: g.has_image,
            system_id: g.system_id.clone().into(),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn run_arcade_page_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    f: &mut Fpga,
    window: &Rc<MinimalSoftwareWindow>,
    mut pad: PadPool,
    app: slint_ui::arcade_page::ArcadePage,
    animation_clock: &AnimationClock,
) {
    let start = Instant::now();
    let mut frames = 0u64;
    let mut nav = LauncherNav::new();
    nav.screen = Screen::Arcade;
    let launcher_bench_scenario = LauncherBenchScenario::from_env();
    let mut bench_next_step = Instant::now();
    let mut bench_step_idx = 0usize;
    let dirty_opt = launcher_dirty_opt_enabled();
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "arcade_page running {label} — {} pad(s), D-pad up/down to move...",
        pad.len()
    );
    if let Some(scenario) = launcher_bench_scenario {
        println!("launcher_bench_scenario={}", scenario.label());
    }
    println!(
        "launcher_dirty_opt={}",
        if dirty_opt { "on" } else { "off" }
    );

    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    let catalog = library_bench::load_arcade_catalog_from_sqlite(&arcade_root)
        .map(|loaded| loaded.catalog)
        .unwrap_or_else(|e| {
            eprintln!("arcade_page catalog cache load failed: {e}");
            empty_arcade_catalog(&arcade_root)
        });
    let games = active_system_games(&catalog, &nav);
    let active_title = active_system(&catalog, &nav)
        .map(|system| system.title.clone())
        .unwrap_or_else(|| "Arcade".to_string());

    let bridge = app.global::<slint_ui::arcade_page::MisterBridge>();
    bridge.set_screen_mode(Screen::Arcade as i32);
    sync_arcade_page_geometry_bridge(&bridge);
    bridge.set_active_system_title(active_title.into());
    bridge.set_arcade_games(slint_arcade_page_games(&games));
    bridge.set_arcade_selected(0);
    bridge.set_arcade_scroll_y(0);
    bridge.set_arcade_preview_has_image(false);
    bridge.set_arcade_preview_status(slint_ui::arcade_page::PreviewStatus::Empty);
    bridge.set_arcade_preview_title("".into());
    bridge.set_arcade_preview_image(Image::default());
    bridge.set_arcade_preview_source_width(0);
    bridge.set_arcade_preview_source_height(0);
    bridge.set_arcade_preview_display_width(0);
    bridge.set_arcade_preview_display_height(0);

    let mut pacer = VsyncPacer::from_env();
    let mut target = UiFrameTarget::cached(ui);
    let mut arcade_list_renderer = ArcadeListRenderer::new();
    let cpu = cpu_profile::start();
    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut prepare_us = 0u128;
    let mut render_us = 0u128;
    let mut arcade_draw_us = 0u128;
    let mut vsync_us = 0u128;
    let mut copy_us = 0u128;
    let mut cached_present_us = 0u128;
    let mut overlay_present_us = 0u128;
    let mut rows = 0u128;
    let mut last_arcade_selected = nav.arcade.selected;
    let mut last_arcade_scroll_y = nav.arcade.scroll_y;
    let mut last_arcade_visual_index = nav.arcade.visual_index;
    let mut frame_trace = std::env::var("MISTER_ARCADE_FRAME_TRACE")
        .ok()
        .and_then(|path| {
            let mut file = std::fs::File::create(&path)
                .map_err(|e| eprintln!("arcade frame trace: create {path} failed: {e}"))
                .ok()?;
            std::io::Write::write_all(
                &mut file,
                b"frame\telapsed_us\tselected\tvisual_index\tvisual_px\tscroll_y\tupdate\trows\tprepare_us\tslint_render_us\tarcade_draw_us\tvsync_us\tfb_present_us\tcached_present_us\toverlay_present_us\twall_us\n",
            )
            .map_err(|e| eprintln!("arcade frame trace: header write failed: {e}"))
            .ok()?;
            println!("arcade_frame_trace={path}");
            Some(file)
        });

    while secs == 0 || start.elapsed().as_secs() < secs {
        let frame_start = Instant::now();
        if let Some(scenario) = launcher_bench_scenario {
            if bench_next_step.elapsed() >= scenario.period() {
                let _ = launcher_bench_step(
                    scenario,
                    &mut nav,
                    &catalog,
                    Some(games.len()),
                    bench_step_idx,
                    Instant::now(),
                );
                bench_step_idx = bench_step_idx.wrapping_add(1);
                bench_next_step = Instant::now();
            }
        } else {
            let _pad_changed = pad.poll();
            let mut state = pad.state().clone();
            state.btn_a = false;
            state.btn_b = false;
            state.btn_home = false;
            let _ = nav.handle_input(&state, Instant::now(), &catalog);
            if nav.screen != Screen::Arcade {
                nav.screen = Screen::Arcade;
            }
        }
        if nav.arcade.selected != last_arcade_selected
            || nav.arcade.scroll_y != last_arcade_scroll_y
            || (nav.arcade.visual_index - last_arcade_visual_index).abs() > 0.001
        {
            bridge.set_arcade_selected(nav.arcade.selected as i32);
            bridge.set_arcade_scroll_y(nav.arcade.scroll_y);
            last_arcade_selected = nav.arcade.selected;
            last_arcade_scroll_y = nav.arcade.scroll_y;
            last_arcade_visual_index = nav.arcade.visual_index;
        }

        let prepare_done = Instant::now();
        update_slint_animations(animation_clock);
        let frame_t1 = Instant::now();
        let mut this_rect: Option<DirtyRect> = None;
        window.draw_if_needed(|renderer| {
            let region = renderer.render(target.render_buffer_mut(), ui.render_w());
            this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
        });
        let frame_t2 = Instant::now();
        let arcade_draw_start = Instant::now();
        let force_arcade_redraw = this_rect.is_some_and(|rect| {
            rect.intersection(ArcadeListRenderer::dirty_rect())
                .is_some()
        });
        let arcade_list_rect =
            arcade_list_renderer.draw(&games, nav.arcade.visual_index, force_arcade_redraw);
        let arcade_draw_done = Instant::now();
        let pace = pacer.wait();
        let frame_t3 = Instant::now();
        let mut copied_rows = 0u32;
        let mut cached_present_frame_us = 0u128;
        if let Some(rect) = this_rect {
            let cached_copy_start = Instant::now();
            copied_rows = target.present_rect(f, disp, ui, rect);
            cached_present_frame_us = cached_copy_start.elapsed().as_micros();
        }
        let arcade_update_label = match arcade_list_rect.as_ref() {
            Some(ArcadeListUpdate::Full(_)) => "full".to_string(),
            None => "none".to_string(),
        };
        let mut overlay_present_frame_us = 0u128;
        if let Some(update) = arcade_list_rect {
            let overlay_copy_start = Instant::now();
            copied_rows +=
                copy_arcade_list_update(&mut target, disp, ui, &mut arcade_list_renderer, update);
            overlay_present_frame_us = overlay_copy_start.elapsed().as_micros();
        }
        let frame_t4 = Instant::now();
        let _ = pace;
        if let Some(file) = frame_trace.as_mut() {
            let visual_px = (nav.arcade.visual_index * ARCADE_ROW_HEIGHT as f32).round() as i32;
            let _ = std::io::Write::write_fmt(
                file,
                format_args!(
                    "{}\t{}\t{}\t{:.6}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    frames,
                    frame_start.duration_since(start).as_micros(),
                    nav.arcade.selected,
                    nav.arcade.visual_index,
                    visual_px,
                    nav.arcade.scroll_y,
                    arcade_update_label,
                    copied_rows,
                    (prepare_done - frame_start).as_micros(),
                    (frame_t2 - frame_t1).as_micros(),
                    (arcade_draw_done - arcade_draw_start).as_micros(),
                    (frame_t3 - arcade_draw_done).as_micros(),
                    (frame_t4 - frame_t3).as_micros(),
                    cached_present_frame_us,
                    overlay_present_frame_us,
                    (frame_t4 - frame_start).as_micros()
                ),
            );
        }

        frames += 1;
        fps_frames += 1;
        prepare_us += (prepare_done - frame_start).as_micros();
        render_us += (frame_t2 - frame_t1).as_micros();
        arcade_draw_us += (arcade_draw_done - arcade_draw_start).as_micros();
        vsync_us += (frame_t3 - arcade_draw_done).as_micros();
        copy_us += (frame_t4 - frame_t3).as_micros();
        cached_present_us += cached_present_frame_us;
        overlay_present_us += overlay_present_frame_us;
        rows += copied_rows as u128;
        if fps_window_start.elapsed() >= Duration::from_secs(1) {
            let n = fps_frames.max(1) as u128;
            println!(
                "arcade_page fps ~ {} prepare {}us slint-render {}us arcade-draw {}us vsync-wait {}us fb-present {}us cached-present {}us overlay-present {}us ({} rows avg)",
                fps_frames,
                prepare_us / n,
                render_us / n,
                arcade_draw_us / n,
                vsync_us / n,
                copy_us / n,
                cached_present_us / n,
                overlay_present_us / n,
                rows / n
            );
            fps_window_start = Instant::now();
            fps_frames = 0;
            prepare_us = 0;
            render_us = 0;
            arcade_draw_us = 0;
            vsync_us = 0;
            copy_us = 0;
            cached_present_us = 0;
            overlay_present_us = 0;
            rows = 0;
        }
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
) {
    let start = Instant::now();
    let mut frames = 0u64;
    let mut nav = LauncherNav::new();
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
    let mut launcher_fps_window_start = Instant::now();
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
    let mut boot_frame_profile = boot_analytics::LauncherFrameWriter::from_env();
    let mut preview = PreviewState::new();
    let mut arcade_list_renderer = ArcadeListRenderer::new();
    let mut active_arcade_games_cache: Vec<ArcadeGameEntry> = Vec::new();
    let mut active_arcade_games_cache_key: Option<(usize, usize)> = None;
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    let mut catalog = match library_bench::load_arcade_catalog_from_sqlite(&arcade_root) {
        Ok(loaded) => {
            print_startup_event(
                start,
                "catalog_cache_load",
                format!(
                    "ok=1 games={} rows={} load_us={}",
                    loaded.catalog.len(),
                    loaded.rows,
                    loaded.us
                ),
            );
            print_startup_event(
                start,
                "library_db_loaded",
                format!(
                    "games={} rows={} load_us={}",
                    loaded.catalog.len(),
                    loaded.rows,
                    loaded.us
                ),
            );
            loaded.catalog
        }
        Err(e) => {
            print_startup_event(start, "catalog_cache_load", format!("ok=0 error={e}"));
            print_startup_event(start, "library_db_miss", e);
            empty_arcade_catalog(&arcade_root)
        }
    };
    let mut catalog_ready = !catalog.games.is_empty();
    print_startup_event(start, "catalog_worker_start", &arcade_root);
    let catalog_rx = start_library_catalog_worker(arcade_root.clone(), catalog_ready);
    let mut catalog_refresh_done = false;
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_game_systems(slint_game_systems(&catalog.systems));
    bridge.set_catalog_scan_visible(!catalog_ready);
    bridge.set_catalog_scan_title(if catalog_ready {
        "Refreshing library".into()
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
    );
    window.request_redraw();
    let mut first_frame_logged = false;
    let mut first_render_logged = false;
    let mut first_vsync_logged = false;
    let mut first_copy_logged = false;
    let mut first_visible_copy_done = false;
    let mut stable_frame_logged = false;
    while secs == 0 || start.elapsed().as_secs() < secs {
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
            while let Ok(message) = catalog_rx.try_recv() {
                match message {
                    CatalogWorkerMessage::Progress { title, detail } => {
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        let visible = !catalog_ready
                            || title == "Indexing library"
                            || title == "Library changed"
                            || title == "Library scan failed"
                            || title == "Library load failed";
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
                        active_arcade_games_cache_key = None;
                        catalog_ready = true;
                        catalog_refresh_done = true;
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
                        bridge.set_catalog_scan_title("".into());
                        bridge.set_catalog_scan_detail("".into());
                        sync_bridge_launcher(
                            &app,
                            &pad,
                            &nav,
                            &setup,
                            &loading_title,
                            "",
                            Some(&catalog),
                            &mut preview,
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

        if !launching {
            let pad_changed = pad.poll();
            let frame_now = Instant::now();
            let state = pad.state();
            let active_idx = pad.active_idx();
            let info = pad.info();

            if launcher_bench_scenario.is_none() && setup_active {
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
                    if let Some(event) = nav.handle_input(&state, frame_now, &catalog) {
                        match event.action {
                            LauncherAction::ExitToMister => {
                                loading_title = "Exit to Mister".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Return to Magik after reboot",
                                    Some(&catalog),
                                    &mut preview,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region =
                                        renderer.render(target.render_buffer_mut(), ui.render_w());
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                target.shutdown_direct(f, disp, ui);
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
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region =
                                        renderer.render(target.render_buffer_mut(), ui.render_w());
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                target.shutdown_direct(f, disp, ui);
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
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region =
                                        renderer.render(target.render_buffer_mut(), ui.render_w());
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                target.shutdown_direct(f, disp, ui);
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
                        );
                        window.request_redraw();
                        update_slint_animations(animation_clock);
                        window.draw_if_needed(|renderer| {
                            let region = renderer.render(target.render_buffer_mut(), ui.render_w());
                            let _ = region;
                        });
                        let _pace = pacer.wait();
                        target.present_rows(f, disp, ui, 0, ui.render_h());
                        target.shutdown_direct(f, disp, ui);

                        match launcher::execute_game_launch(&mra) {
                            Ok(spawned) => {
                                launch_started = Instant::now();
                                launch_spawned_mister = spawned;
                            }
                            Err(e) => {
                                eprintln!("game launch failed: {e}");
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
                );
                window.request_redraw();
            } else if light_bridge_dirty {
                sync_bridge_launcher_light(
                    &app,
                    &nav,
                    &setup,
                    &loading_title,
                    "",
                    &catalog,
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
        if !launching && nav.screen == Screen::Arcade {
            let cache_key = (nav.selected, catalog.len());
            if active_arcade_games_cache_key != Some(cache_key) {
                active_arcade_games_cache = active_system_games(&catalog, &nav);
                active_arcade_games_cache_key = Some(cache_key);
            }
        }
        if dirty_opt && !launching && nav.screen == Screen::Arcade {
            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
            if schedule_arcade_preview_for_game(
                &bridge,
                active_arcade_games_cache.get(nav.arcade.selected),
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
            let region = renderer.render(target.render_buffer_mut(), ui.render_w());
            this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
        });
        let frame_t2 = Instant::now();
        let custom_draw_start = Instant::now();
        let arcade_list_rect = if !launching && nav.screen == Screen::Arcade {
            let force_arcade_redraw = this_rect.is_some_and(|rect| {
                rect.intersection(ArcadeListRenderer::dirty_rect())
                    .is_some()
            });
            arcade_list_renderer.draw(
                &active_arcade_games_cache,
                nav.arcade.visual_index,
                force_arcade_redraw,
            )
        } else {
            None
        };
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
        let mut overlay_present_frame_us = 0u128;
        if let Some(update) = arcade_list_rect {
            let overlay_copy_start = Instant::now();
            copied_rows +=
                copy_arcade_list_update(target, disp, ui, &mut arcade_list_renderer, update);
            target.route_if_direct(f, ui);
            overlay_present_frame_us = overlay_copy_start.elapsed().as_micros();
        }
        let frame_t4 = Instant::now();
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
            let fps_estimate = if start.elapsed().as_secs_f64() > 0.0 {
                frames as f64 / start.elapsed().as_secs_f64()
            } else {
                0.0
            };
            runtime_status::write_launcher_status(LauncherStatus {
                scene: "launcher",
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
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_size_enlarges_only_by_integer_scale() {
        let size = preview_display_size(100, 50, ARCADE_PREVIEW_BOX_W, ARCADE_PREVIEW_BOX_H);
        assert_eq!(size, PreviewDisplaySize { w: 400, h: 200 });
        assert_eq!(size.w % 100, 0);
        assert_eq!(size.h % 50, 0);
        assert!(size.w * size.h <= PREVIEW_MAX_AREA);
    }

    #[test]
    fn preview_size_shrinks_large_images_without_changing_aspect() {
        let size = preview_display_size(1920, 1080, ARCADE_PREVIEW_BOX_W, ARCADE_PREVIEW_BOX_H);
        assert_eq!(size, PreviewDisplaySize { w: 448, h: 252 });
        assert_eq!(size.w as u64 * 1080, size.h as u64 * 1920);
        assert!(size.w * size.h <= PREVIEW_MAX_AREA);
        assert!(size.w <= ARCADE_PREVIEW_BOX_W);
        assert!(size.h <= ARCADE_PREVIEW_BOX_H);
    }

    #[test]
    fn preview_size_keeps_odd_ratio_integer_dimensions() {
        let size = preview_display_size(321, 225, ARCADE_PREVIEW_BOX_W, ARCADE_PREVIEW_BOX_H);
        assert_eq!(size, PreviewDisplaySize { w: 321, h: 225 });
        assert_eq!(size.w as u64 * 225, size.h as u64 * 321);
        assert!(size.w * size.h <= PREVIEW_MAX_AREA);
    }

    #[test]
    fn preview_size_respects_pane_bounds_and_area() {
        let size = preview_display_size(320, 224, ARCADE_PREVIEW_BOX_W, ARCADE_PREVIEW_BOX_H);
        assert_eq!(size, PreviewDisplaySize { w: 320, h: 224 });
        assert!(size.w * size.h <= PREVIEW_MAX_AREA);
        assert!(size.w <= ARCADE_PREVIEW_BOX_W);
        assert!(size.h <= ARCADE_PREVIEW_BOX_H);
    }
}
