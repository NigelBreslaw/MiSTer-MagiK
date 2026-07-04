//! Shared vsync render loop and Slint bench scene dispatch.
#![cfg_attr(mister_ui_scope_launcher, allow(dead_code))]

use crate::fpga::Fpga;
use crate::vt::VtGraphicsGuard;
use mister_magik_fb::framebuffer::format::production_label;
use mister_magik_fb::framebuffer::mapped::MappedRgb565Framebuffer;
use mister_magik_fb::framebuffer::vsync::VsyncPaceSource;
use mister_magik_fb::framebuffer::vsync::VsyncPacer;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel, TargetPixel,
};
use slint::platform::{Platform, WindowAdapter};
#[cfg(all(mister_video_scene, feature = "video-lab"))]
use slint::Rgb8Pixel;
use slint::{
    ComponentHandle, ModelRc, PhysicalSize, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mister_magik_ui as slint_ui;

use crate::arcade_catalog::{
    self, ArcadeCatalog, ArcadeGameView, LaunchTarget, ARCADE_LIST_VISIBLE_H, ARCADE_ROW_HEIGHT,
    HOME_LIST_VISIBLE_W, HOME_TILE_GAP, HOME_TILE_WIDTH,
};
use crate::arcade_list_renderer::{
    ArcadeListGeometry, ArcadeListItem, ArcadeListRenderer, ArcadeListUpdate, ARCADE_LIST_H,
    ARCADE_LIST_W, ARCADE_LIST_X, ARCADE_LIST_Y,
};
use crate::boot_analytics;
use crate::controller_db::ControllerDb;
use crate::cpu_profile;
use crate::display_config::{detect_runtime_display_geometry, DisplayConfig};
#[cfg(any(mister_bench_scenes, mister_video_scene))]
use crate::frame_profile::FrameRect;
#[cfg(mister_video_scene)]
use crate::frame_profile::{FrameProfiler, FrameSample, VideoFrameProfile};
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
use mister_magik_fb::experiments::effects::framebuffer_effects::{
    EffectKind, EffectSize, EFFECT_SIZES,
};
use mister_magik_fb::framebuffer::route::LauncherFramebufferRoute;
#[cfg(mister_experiments)]
use mister_magik_fb::framebuffer::target::{blend_565, brighten_565};
use mister_magik_fb::framebuffer::target::{
    build_launcher_present_plan, copy_cached_rect_565, dirty_rect, format_dirty_rect, DirtyRect,
    DirtyRectList, FramebufferTargetGeometry, UiFrameTarget,
};
use slint_ui::launcher::PreviewStatus;
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::{mpsc, OnceLock};

mod catalog_worker;
mod controller_loop;
mod controller_setup_input_session;
#[cfg(mister_experiments)]
mod experiments;
mod launch_handoff_session;
mod launcher_bench;
mod launcher_bridge;
mod launcher_catalog_session;
mod launcher_composition;
mod launcher_compositor;
mod launcher_frame_accounting;
mod launcher_lifecycle;
mod launcher_loop;
mod launcher_scheduler;
mod launcher_worker_intents;
mod media_worker;
mod raw565_preview_renderer;
mod screenshot_media_update_session;
pub(crate) mod ui_boot;
pub(crate) mod ui_frame_target;
pub(crate) mod ui_platform;
#[cfg(mister_video_scene)]
mod video_loop;

use catalog_worker::*;
use controller_loop::*;
use controller_setup_input_session::*;
#[cfg(mister_experiments)]
use experiments::effects::{
    run_camera_effects_loop, run_raster_effects_loop, run_screensaver_loop,
    run_sprite_effects_loop, run_text_effects_loop, run_transition_effects_loop,
};
use launch_handoff_session::*;
use launcher_bench::*;
use launcher_bridge::*;
use launcher_catalog_session::*;
use launcher_composition::*;
use launcher_compositor::*;
use launcher_frame_accounting::*;
use launcher_lifecycle::*;
use launcher_loop::*;
use launcher_scheduler::*;
use media_worker::*;
use raw565_preview_renderer::*;
use screenshot_media_update_session::*;
use ui_boot::*;
use ui_frame_target::*;
use ui_platform::*;
#[cfg(mister_video_scene)]
use video_loop::*;

const AUTO_CONTROLLER_SETUP_ENABLED: bool = false;
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
    "controller_test",
    #[cfg(mister_video_scene)]
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
    experiments::effects::print_camera_effects();
}

#[cfg(mister_experiments)]
pub fn print_sprite_effects() {
    experiments::effects::print_sprite_effects();
}

#[cfg(mister_experiments)]
pub fn print_text_effects() {
    experiments::effects::print_text_effects();
}

#[cfg(mister_experiments)]
pub fn print_raster_effects() {
    experiments::effects::print_raster_effects();
}

#[cfg(mister_experiments)]
pub fn print_transition_effects() {
    experiments::effects::print_transition_effects();
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
    let UiBootFramebufferSession {
        ui,
        mut disp,
        _fb_mode_guard,
    } = UiBootFramebufferSession::start_ui_or_exit(f);

    match f.set_audio_volume(0) {
        Ok(()) => boot_analytics::event("set_audio_volume", "attenuation=0"),
        Err(e) => {
            eprintln!("warning: failed to set FPGA audio volume: {e}");
            boot_analytics::event("set_audio_volume_failed", format!("error={e}"));
        }
    }
    #[cfg(mister_experiments)]
    if scene == "screensaver" {
        run_screensaver_loop(secs, &ui, &mut disp);
        return;
    }

    #[cfg(mister_experiments)]
    if scene == "camera-effects" {
        run_camera_effects_loop(secs, &ui, &mut disp);
        return;
    }

    #[cfg(mister_experiments)]
    if scene == "sprite-effects" {
        run_sprite_effects_loop(secs, &ui, &mut disp);
        return;
    }

    #[cfg(mister_experiments)]
    if scene == "text-effects" {
        run_text_effects_loop(secs, &ui, &mut disp);
        return;
    }

    #[cfg(mister_experiments)]
    if scene == "raster-effects" {
        run_raster_effects_loop(secs, &ui, &mut disp);
        return;
    }

    #[cfg(mister_experiments)]
    if scene == "transition-effects" {
        run_transition_effects_loop(secs, &ui, &mut disp);
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
        #[cfg(mister_video_scene)]
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
            with_scene_app!(launcher::Launcher, &ui, &window, app, {
                boot_analytics::event("app_show_attempt", "scene=launcher");
                app.show().expect("show");
                boot_analytics::event("app_show", "scene=launcher ok=1");
                window.request_redraw();
                let mut target = UiFrameTarget::open(frame_target_geometry(&ui));
                let pad = open_pads();
                init_launcher_bridge(&app, &pad);
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
