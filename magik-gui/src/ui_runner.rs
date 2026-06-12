//! Shared vsync render loop and Slint bench scene dispatch.
#![cfg_attr(mister_ui_scope_launcher, allow(dead_code))]

use crate::fb::{Display, VsyncPacer};
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
use crate::frame_profile::FrameRect;
#[cfg(mister_bench_scenes)]
use crate::frame_profile::{FrameProfiler, FrameSample};
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

mod catalog_worker;
#[cfg(mister_bench_scenes)]
mod console_scroll_loop;
mod controller_loop;
mod launcher_bench;
mod launcher_bridge;
mod launcher_loop;
mod screensaver_loop;
pub(crate) mod ui_boot;
#[cfg(mister_bench_scenes)]
mod ui_frame_loop;
pub(crate) mod ui_frame_target;
pub(crate) mod ui_platform;
#[cfg(all(feature = "video", mister_bench_scenes))]
mod video_loop;

use catalog_worker::*;
#[cfg(mister_bench_scenes)]
use console_scroll_loop::*;
use controller_loop::*;
use launcher_bench::*;
use launcher_bridge::*;
use launcher_loop::*;
use screensaver_loop::*;
use ui_boot::*;
#[cfg(mister_bench_scenes)]
use ui_frame_loop::*;
use ui_frame_target::*;
use ui_platform::*;
#[cfg(all(feature = "video", mister_bench_scenes))]
use video_loop::*;

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
    "screensaver",
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
    disp.clear_black();
    boot_analytics::event(
        "early_black_frame_copied",
        format!(
            "format={} w={} h={}",
            fb_format.label(),
            disp.width(),
            disp.height()
        ),
    );
    disp.record_visual_sample("after_early_black_frame_before_initial_route");
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
    boot_analytics::event(
        "rust_framebuffer_route_completed",
        format!(
            "format={} w={} h={} scan={}x{} support_flag={flag}",
            fb_format.label(),
            disp.width(),
            disp.height(),
            UI_HDMI_W,
            UI_HDMI_H
        ),
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

    if scene == "screensaver" {
        run_screensaver_loop(secs, &ui, &mut disp, fb_format);
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
