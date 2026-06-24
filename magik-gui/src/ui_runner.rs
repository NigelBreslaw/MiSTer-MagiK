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
use slint::{ComponentHandle, ModelRc, PhysicalSize, SharedString, VecModel};
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
use crate::display_config::{detect_runtime_display_geometry, DisplayConfig};
#[cfg(mister_bench_scenes)]
use crate::frame_profile::{FrameProfiler, FrameRect, FrameSample};
use crate::input::{PadInfo, PadPool};
use crate::launcher::{self, LauncherAction, LauncherNav, Screen};
use crate::library_db;
use crate::preview_state::{
    apply_ready_preview, preview_visual_pct, request_arcade_preview_window,
    schedule_arcade_preview_window, PreviewRawFrame, PreviewRawFrameStatus, PreviewRawPixels,
    PreviewRawTransitionFrame, PreviewState, ARCADE_PREVIEW_BOX_H, ARCADE_PREVIEW_BOX_W,
    ARCADE_PREVIEW_BOX_X, ARCADE_PREVIEW_BOX_Y,
};
use crate::runtime_status::{self, LauncherStatus};
use crate::screenshot_transitions::{
    PreviewTransitionDemo, PreviewTransitionEffect, PreviewTransitionTrace,
};
use crate::setup_nav::{SetupAction, SetupNav, SetupPhase};
use crate::ui_display::{RuntimeDisplayGeometry, UiDisplay, UiDisplayPlan, SLINT_UI_SCALE};
#[cfg(mister_experiments)]
use mister_magik_fb::effects::{EffectKind, EffectSize, EFFECT_SIZES};
use slint::platform::software_renderer::PhysicalRegion;
use slint_ui::launcher::PreviewStatus;
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::{mpsc, Mutex, OnceLock};

#[cfg(mister_experiments)]
mod camera_effects_loop;
mod catalog_worker;
mod controller_loop;
#[cfg(mister_experiments)]
mod effect_loop_support;
mod launcher_bench;
mod launcher_bridge;
mod launcher_catalog_session;
mod launcher_compositor;
mod launcher_frame_accounting;
mod launcher_loop;
mod launcher_worker_intents;
mod media_worker;
#[cfg(mister_experiments)]
mod raster_effects_loop;
mod raw565_preview_renderer;
#[cfg(mister_experiments)]
mod screensaver_loop;
mod screenshot_media_update_session;
#[cfg(mister_experiments)]
mod sprite_effects_loop;
#[cfg(mister_experiments)]
mod text_effects_loop;
#[cfg(mister_experiments)]
mod transition_effects_loop;
pub(crate) mod ui_boot;
#[cfg(mister_bench_scenes)]
mod ui_frame_loop;
pub(crate) mod ui_frame_target;
pub(crate) mod ui_platform;
#[cfg(all(feature = "video", mister_bench_scenes))]
mod video_loop;

#[cfg(mister_experiments)]
use camera_effects_loop::run_camera_effects_loop;
use catalog_worker::*;
use controller_loop::*;
use launcher_bench::*;
use launcher_bridge::*;
use launcher_catalog_session::*;
use launcher_compositor::*;
use launcher_frame_accounting::*;
use launcher_loop::*;
use media_worker::*;
#[cfg(mister_experiments)]
use raster_effects_loop::run_raster_effects_loop;
use raw565_preview_renderer::*;
#[cfg(mister_experiments)]
use screensaver_loop::*;
use screenshot_media_update_session::*;
#[cfg(mister_experiments)]
use sprite_effects_loop::run_sprite_effects_loop;
#[cfg(mister_experiments)]
use text_effects_loop::run_text_effects_loop;
#[cfg(mister_experiments)]
use transition_effects_loop::run_transition_effects_loop;
use ui_boot::*;
#[cfg(mister_bench_scenes)]
use ui_frame_loop::*;
use ui_frame_target::*;
use ui_platform::*;
#[cfg(all(feature = "video", mister_bench_scenes))]
use video_loop::*;

const AUTO_CONTROLLER_SETUP_ENABLED: bool = false;
const DEFAULT_DIRTY_RECT_BROAD_PCT: usize = 85;
const FIRST_LIBRARY_SCAN_MESSAGE: &str =
    "Scanning for games. This only happens the first time you start MiSTer MagiK";
const UPDATING_LIBRARY_SCAN_MESSAGE: &str = "Updating Library";

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
    #[cfg(mister_experiments)]
    "screensaver",
    #[cfg(mister_experiments)]
    "camera-effects",
    #[cfg(mister_experiments)]
    "sprite-effects",
    #[cfg(mister_experiments)]
    "text-effects",
    #[cfg(mister_experiments)]
    "raster-effects",
    #[cfg(mister_experiments)]
    "transition-effects",
    #[cfg(all(not(mister_ui_scope_launcher), mister_bench_scenes))]
    "demo",
    "controller_test",
    #[cfg(mister_bench_scenes)]
    "full_motion",
    #[cfg(mister_bench_scenes)]
    "static_ui",
    #[cfg(mister_bench_scenes)]
    "local_motion",
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

#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
pub fn print_scenes() {
    println!("Slint UI scenes (runtime framebuffer sized, ui-scale {SLINT_UI_SCALE}):");
    for s in UI_SCENES {
        println!("  {s}");
    }
}

#[cfg(mister_experiments)]
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

#[cfg(mister_experiments)]
pub fn print_camera_effects() {
    camera_effects_loop::print_camera_effects();
}

#[cfg(mister_experiments)]
pub fn print_sprite_effects() {
    sprite_effects_loop::print_sprite_effects();
}

#[cfg(mister_experiments)]
pub fn print_text_effects() {
    text_effects_loop::print_text_effects();
}

#[cfg(mister_experiments)]
pub fn print_raster_effects() {
    raster_effects_loop::print_raster_effects();
}

#[cfg(mister_experiments)]
pub fn print_transition_effects() {
    transition_effects_loop::print_transition_effects();
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

fn detect_runtime_display_geometry_for_plan(
    f: &mut Fpga,
    label: &str,
) -> Option<RuntimeDisplayGeometry> {
    match detect_runtime_display_geometry(f) {
        Ok(detected) => {
            println!("runtime-video-info[{label}]: {}", detected.video.log_line());
            match detected.geometry {
                Some(geometry) => {
                    boot_analytics::event(
                        "runtime_display_geometry_detected",
                        format!(
                            "label={label} output={}x{} scan={}x{}",
                            geometry.output_w, geometry.output_h, geometry.scan_w, geometry.scan_h
                        ),
                    );
                    Some(geometry)
                }
                None => {
                    eprintln!(
                        "warning: runtime display geometry invalid for {label}; falling back to MiSTer.ini"
                    );
                    boot_analytics::event(
                        "runtime_display_geometry_invalid",
                        format!("label={label} {}", detected.video.log_line()),
                    );
                    None
                }
            }
        }
        Err(e) => {
            eprintln!("warning: failed to detect runtime display geometry for {label}: {e}");
            boot_analytics::event(
                "runtime_display_geometry_detect_failed",
                format!("label={label} error={e}"),
            );
            None
        }
    }
}

pub fn run_ui(f: &mut Fpga) {
    let (scene, secs) = parse_ui_args();
    boot_analytics::event("run_ui_start", format!("scene={scene} secs={secs}"));
    println!("ui scene={scene} secs={secs}");
    println!("ui_render_mode=cached");

    let _vt = VtGraphicsGuard::enter_or_warn();

    let fb_format = boot_framebuffer_format();
    let runtime_geometry = detect_runtime_display_geometry_for_plan(f, "ui");
    let display_plan = UiDisplayPlan::from_runtime_or_mister_ini_file(runtime_geometry);
    println!("{}", display_plan.log_line());
    if display_plan.fallback {
        boot_analytics::event("display_plan_fallback", display_plan.log_line());
    }
    println!(
        "ui-fb-mode=temporary {}x{} format={} output={}x{} scan={}x{} restore=on-drop",
        display_plan.fb_w,
        display_plan.fb_h,
        fb_format.label(),
        display_plan.output_w,
        display_plan.output_h,
        display_plan.scan_w,
        display_plan.scan_h
    );
    let current_fb = match Display::current_info() {
        Ok(info) => info,
        Err(e) => {
            eprintln!("failed to read current framebuffer mode for FPGA-scaled UI: {e}");
            std::process::exit(1);
        }
    };
    let fb_mode_action = fb_mode_action(current_fb, display_plan, fb_format);
    println!("fb_mode_action={}", fb_mode_action.label());
    boot_analytics::event("fb_mode_action", fb_mode_action.label());
    let _fb_mode_guard = match fb_mode_action {
        FbModeAction::AdoptCurrent => None,
        FbModeAction::WriteMode => {
            match FbModeGuard::set_temporary_format(display_plan.fb_w, display_plan.fb_h, fb_format)
            {
                Ok(guard) => Some(guard),
                Err(e) => {
                    eprintln!("failed to set temporary framebuffer mode for FPGA-scaled UI: {e}");
                    std::process::exit(1);
                }
            }
        }
    };

    println!("display-open-path=temporary-fb-fpga-scale");
    let mut disp = match Display::open_with_format(display_plan.fb_w, display_plan.fb_h, fb_format)
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open display (/dev/fb0): {e}");
            std::process::exit(1);
        }
    };
    let ui = UiDisplay::for_plan(display_plan);
    println!("{}", ui.log_line());
    disp.clear_black();
    boot_analytics::event(
        "ui_black_frame_copied",
        format!(
            "format={} w={} h={}",
            fb_format.label(),
            disp.width(),
            disp.height()
        ),
    );
    disp.record_visual_sample("after_ui_black_frame_before_initial_route");
    match DisplayConfig::detect(f, disp.info(), &ui) {
        Ok(config) => {
            println!("{}", config.log_line());
            boot_analytics::event("display_config_detected", config.boot_analytics_detail());
        }
        Err(e) => {
            eprintln!("warning: failed to read display configuration from FPGA: {e}");
            boot_analytics::event("display_config_detect_failed", format!("error={e}"));
        }
    }
    if std::env::var_os("MISTER_MAGIK_PARENT").is_some() {
        println!("MiSTer_MagiK parent detected; Slint reasserting framebuffer route");
    }
    let route_mode = ui_fpga_scaled_mode(ui.scan_w(), ui.scan_h());
    let route_mode_label = "fpga-scale-scan";
    let set_vga_fb = ui.direct_video();
    boot_analytics::event(
        "initial_fb_enable_direct_attempt",
        format!(
            "w={} h={} mode={route_mode_label} scan={}x{} set_vga_fb={set_vga_fb}",
            disp.width(),
            disp.height(),
            ui.scan_w(),
            ui.scan_h()
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
            "format={} w={} h={} output={}x{} scan={}x{} support_flag={flag}",
            fb_format.label(),
            disp.width(),
            disp.height(),
            ui.output_w(),
            ui.output_h(),
            ui.scan_w(),
            ui.scan_h()
        ),
    );
    if fb_mode_action == FbModeAction::WriteMode {
        settle_boot_black_frame(
            "ui-startup",
            &mut disp,
            f,
            route_mode,
            set_vga_fb,
            fb_format,
        );
    }
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

    #[cfg(mister_experiments)]
    if scene == "screensaver" {
        run_screensaver_loop(secs, &ui, &mut disp, fb_format);
        return;
    }

    #[cfg(mister_experiments)]
    if scene == "camera-effects" {
        run_camera_effects_loop(secs, &ui, &mut disp, fb_format);
        return;
    }

    #[cfg(mister_experiments)]
    if scene == "sprite-effects" {
        run_sprite_effects_loop(secs, &ui, &mut disp, fb_format);
        return;
    }

    #[cfg(mister_experiments)]
    if scene == "text-effects" {
        run_text_effects_loop(secs, &ui, &mut disp, fb_format);
        return;
    }

    #[cfg(mister_experiments)]
    if scene == "raster-effects" {
        run_raster_effects_loop(secs, &ui, &mut disp, fb_format);
        return;
    }

    #[cfg(mister_experiments)]
    if scene == "transition-effects" {
        run_transition_effects_loop(secs, &ui, &mut disp, fb_format);
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
        #[cfg(all(not(mister_ui_scope_launcher), mister_bench_scenes))]
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
        "launcher" => {
            let pad = open_pads();
            with_scene_app!(launcher::Launcher, &ui, &window, app, {
                init_launcher_bridge(&app, &pad);
                boot_analytics::event("app_show_attempt", "scene=launcher");
                app.show().expect("show");
                boot_analytics::event("app_show", "scene=launcher ok=1");
                window.request_redraw();
                let mut target = UiFrameTarget::open(&ui);
                present_launcher_startup_frame(
                    Instant::now(),
                    &ui,
                    &mut disp,
                    f,
                    &window,
                    &mut target,
                );
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
            });
        }
        _ => unreachable!(),
    }
}
