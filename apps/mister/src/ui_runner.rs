// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared vsync render loop and Slint bench scene dispatch.
#![cfg_attr(mister_ui_scope_launcher, allow(dead_code))]

use crate::fpga::Fpga;
use crate::vt::VtGraphicsGuard;
use mister_magik_fb::framebuffer::format::production_label;
use mister_magik_fb::framebuffer::mapped::MappedRgb565Framebuffer;
use mister_magik_fb::framebuffer::vsync::VsyncPaceSource;
use mister_magik_fb::framebuffer::vsync::VsyncPacer;
use slint::platform::WindowAdapter;
use slint::platform::software_renderer::{RepaintBufferType, Rgb565Pixel, TargetPixel};
use slint::{ComponentHandle, Model, ModelRc, PhysicalSize, SharedString, VecModel};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mister_magik_ui as slint_ui;

use crate::arcade_catalog::{
    self, ARCADE_LIST_VISIBLE_H, ARCADE_ROW_HEIGHT, ArcadeCatalog, ArcadeGameView,
    HOME_LIST_VISIBLE_W, HOME_TILE_GAP, HOME_TILE_WIDTH, LaunchTarget,
};
use crate::arcade_list_renderer::{
    ARCADE_LIST_H, ARCADE_LIST_W, ARCADE_LIST_X, ARCADE_LIST_Y, ArcadeListGeometry, ArcadeListItem,
    ArcadeListRenderer, ArcadeListUpdate,
};
use crate::boot_analytics;
use crate::controller_db::ControllerDb;
use crate::cpu_profile;
use crate::display_config::{DisplayConfig, detect_runtime_display_geometry};
use crate::frame_profile::{FrameProfiler, FrameRect, FrameSample, VideoFrameProfile};
use crate::input::{PadInfo, PadPool};
use crate::launcher::{self, LauncherAction, LauncherNav, Screen};
use crate::library_db;
use crate::preview_state::{
    ARCADE_PREVIEW_BOX_H, ARCADE_PREVIEW_BOX_W, ARCADE_PREVIEW_BOX_X, ARCADE_PREVIEW_BOX_Y,
    PreviewPresentationState, PreviewPresentationTarget, PreviewRawFrame, PreviewRawFrameStatus,
    PreviewRawPixels, PreviewRawTransitionFrame, PreviewState, apply_ready_preview,
    preview_visual_pct, prewarm_arcade_selected_preview, request_arcade_preview_window,
    schedule_arcade_preview_window,
};
use crate::return_catalog_capsule;
use crate::runtime_status::{self, LauncherStatus};
use crate::screenshot_transitions::{
    PreviewFadePath, PreviewFadeTrace, PreviewTransitionDemo, PreviewTransitionEffect,
    PreviewTransitionTrace,
};
use crate::setup_nav::{SetupAction, SetupNav, SetupPhase};
use crate::ui_display::{
    CrtUiMetrics, RuntimeDisplayGeometry, UiDisplay, UiDisplayPlan, UiPixelSize,
};
#[cfg(mister_experiments)]
use mister_magik_fb::experiments::effects::framebuffer_effects::{
    EFFECT_SIZES, EffectKind, EffectSize,
};
use mister_magik_fb::framebuffer::full_frame_latch::{
    LatchCopyPath, LatchFrameBuffers, LatchHardware, wait_for_latch_completion,
};
use mister_magik_fb::framebuffer::present::{
    copy_cached_rect_565, copy_cached_rows_565, copy_direct_preview_rect_565,
    copy_direct_preview_rect_to_hidden,
};
use mister_magik_fb::framebuffer::route::LauncherFramebufferRoute;
use mister_magik_fb::framebuffer::target::{
    CachedFrameView, DirectPreviewView, DirtyRect, DirtyRectList, FramebufferTargetGeometry,
    UiFrameTarget, blend_565, build_launcher_present_plan_from_layers, dirty_rect, dirty_rects,
    format_dirty_rect,
};
use mister_magik_fb::framebuffer::{
    format::rgb565_stride_bytes,
    scanout_slots::{HiddenRgb565BufferIndex, ScanoutSlotsRgb565Framebuffer},
};
use mister_magik_fb::launcher_presentation::LauncherBridgePresenter;
use mister_magik_fb::launcher_runtime::catalog::*;
use mister_magik_fb::launcher_runtime::composition::*;
use mister_magik_fb::launcher_runtime::lifecycle::*;
use mister_magik_fb::launcher_runtime::media::*;
use mister_magik_fb::launcher_runtime::navigation_transition::*;
use mister_magik_fb::launcher_runtime::settings::{FileSettingsStore, SettingsStore};
use slint_ui::launcher::PreviewStatus;
use std::path::PathBuf;
use std::sync::{OnceLock, mpsc};

mod arcade_drawer;
mod catalog_worker;
mod controller_loop;
mod controller_setup_input_session;
mod crt_trial_loop;
#[cfg(mister_experiments)]
mod experiments;
mod latch_v5_qualification;
mod launch_handoff_session;
mod launcher_automation;
mod launcher_bench;
mod launcher_bridge;
mod launcher_catalog_publication_test;
mod launcher_catalog_session;
mod launcher_compositor;
pub(crate) mod launcher_display_session;
mod launcher_frame_accounting;
mod launcher_loop;
mod launcher_pacing;
mod launcher_present;
mod launcher_readiness;
mod launcher_scheduler;
mod launcher_screensaver;
mod launcher_screensaver_pipeline;
mod launcher_startup_intro;
mod launcher_worker_intents;
mod raw565_preview_renderer;
mod screenshot_media_update_session;
mod tear_pattern_loop;
pub(crate) mod ui_boot;
pub(crate) mod ui_frame_target;
pub(crate) mod ui_platform;
mod update_checker;
#[cfg(all(target_os = "linux", target_arch = "arm"))]
mod video_loop;

use catalog_worker::*;
use controller_loop::*;
use controller_setup_input_session::*;
use crt_trial_loop::*;
#[cfg(mister_experiments)]
use experiments::effects::{
    run_camera_effects_loop, run_raster_effects_loop, run_sprite_effects_loop,
    run_text_effects_loop, run_transition_effects_loop,
};
use latch_v5_qualification::*;
use launch_handoff_session::*;
use launcher_automation::*;
use launcher_bench::*;
use launcher_bridge::*;
use launcher_catalog_publication_test::*;
use launcher_catalog_session::*;
use launcher_compositor::*;
use launcher_display_session::*;
use launcher_frame_accounting::*;
use launcher_loop::*;
use launcher_present::*;
use launcher_scheduler::*;
use launcher_screensaver::{LauncherScreensaver, LauncherScreensaverLoader};
use launcher_screensaver_pipeline::{RenderAheadPoll, ScreensaverRenderAhead};
use launcher_startup_intro::*;
use mister_magik_mister_runtime::framebuffer::latch_state::{
    DirectLayerState, LatchFramePlan as LauncherFramePlan, LatchPresentPlan, TwoBufferLatchState,
};
use raw565_preview_renderer::*;
use screenshot_media_update_session::*;
use tear_pattern_loop::*;
use ui_boot::*;
use ui_frame_target::*;
use ui_platform::*;
use update_checker::*;
#[cfg(all(target_os = "linux", target_arch = "arm"))]
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
        Screen::About => "about",
        Screen::Licenses => "licenses",
        Screen::Info => "info",
        Screen::Screensaver => "screensaver-settings",
    }
}

pub const UI_SCENES: &[&str] = &[
    "launcher",
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
    "crt_probe", // Bounded attended slot diagnostics; never a production launcher mode.
    "crt_trial",
    "tear_pattern",
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
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
        crate::ui_errln!("unknown scene '{s}' (use: {})", UI_SCENES.join(" | "));
        std::process::exit(2);
    }
}

#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
pub fn print_scenes() {
    crate::ui_logln!("Slint UI scenes (runtime framebuffer sized):");
    for s in UI_SCENES {
        crate::ui_logln!("  {s}");
    }
}

#[cfg(mister_experiments)]
pub fn print_effects() {
    crate::ui_logln!("Framebuffer effects:");
    for &kind in EffectKind::all() {
        crate::ui_logln!("  {}", kind.name());
    }
    crate::ui_logln!("Supported internal sizes:");
    for &(w, h) in EFFECT_SIZES {
        let scale = EffectSize { w, h }.scale_to_1080p().unwrap_or(0);
        if scale > 0 {
            crate::ui_logln!("  {w}x{h} ({scale}x to 1920x1080)");
        } else {
            crate::ui_logln!("  {w}x{h}");
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
        mister_ui.set_window_width($ui.render_w() as i32);
        mister_ui.set_window_height($ui.render_h() as i32);
        mister_ui.set_crt_layout($ui.output_route().is_crt());
        // HDMI keeps the legacy shared metrics; CRT profiles are route-owned.
        if $ui.output_route().is_crt() {
            let crt_metrics = CrtUiMetrics::for_display($ui);
            let content = $ui.content_rect();
            mister_ui.set_crt_grid_x(crt_metrics.grid_x);
            mister_ui.set_crt_grid_y(crt_metrics.grid_y);
            mister_ui.set_crt_border_x(crt_metrics.border_x);
            mister_ui.set_crt_border_y(crt_metrics.border_y);
            mister_ui.set_crt_font_family(crt_metrics.font_family.label().into());
            let pixel_text_size = |size| match size {
                UiPixelSize::Px8 => slint_ui::$module::PixelTextSize::Px8,
                UiPixelSize::Px16 => slint_ui::$module::PixelTextSize::Px16,
                UiPixelSize::Px24 => slint_ui::$module::PixelTextSize::Px24,
                UiPixelSize::Px32 => slint_ui::$module::PixelTextSize::Px32,
            };
            mister_ui.set_crt_body_font(pixel_text_size(crt_metrics.body_font));
            mister_ui.set_crt_heading_font(pixel_text_size(crt_metrics.heading_font));
            mister_ui.set_crt_card_title_font(pixel_text_size(crt_metrics.card_title_font));
            mister_ui.set_crt_card_detail_font(pixel_text_size(crt_metrics.card_detail_font));
            mister_ui.set_crt_header_height(crt_metrics.header_height);
            mister_ui.set_crt_footer_height(crt_metrics.footer_height);
            mister_ui.set_crt_game_row_height(crt_metrics.game_row_height);
            mister_ui.set_crt_content_x(content.x as i32);
            mister_ui.set_crt_content_y(content.y as i32);
            mister_ui.set_crt_content_width(content.width as i32);
            mister_ui.set_crt_content_height(content.height as i32);
        }
        configure_window($ui, $window);
        $body
    }};
}

pub fn run_ui(f: &mut Fpga, launch_return_cpu_profile: Option<cpu_profile::CpuProfiler>) {
    crate::launch_preparation::cleanup_archive_launch_staging();
    let (scene, secs) = parse_ui_args();
    boot_analytics::event("run_ui_start", format!("scene={scene} secs={secs}"));
    crate::ui_logln!("ui scene={scene} secs={secs}");
    crate::ui_logln!("ui_render_mode=cached");
    mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
        mister_magik_catalog::runtime_thread::RuntimeThreadRole::LauncherUi,
    );

    mister_magik_fb::framebuffer::stream::start();

    let _vt = VtGraphicsGuard::enter_or_warn();
    let UiBootFramebufferSession {
        ui,
        mut disp,
        mut display_session,
        _fb_mode_guard,
    } = UiBootFramebufferSession::start_ui_or_exit(f);

    match f.set_audio_volume(0) {
        Ok(()) => boot_analytics::event("set_audio_volume", "attenuation=0"),
        Err(e) => {
            crate::ui_errln!("warning: failed to set FPGA audio volume: {e}");
            boot_analytics::event("set_audio_volume_failed", format!("error={e}"));
        }
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

    if scene == "crt_probe" {
        run_crt_probe_loop(secs, &ui, f, &mut display_session);
        return;
    }

    if scene == "crt_trial" {
        run_crt_trial_loop(secs, &ui, f, &mut display_session);
        return;
    }

    let window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    let animation_clock = AnimationClock::from_env_with_fixed_step(
        ui.output_route()
            .nominal_period_us()
            .map(Duration::from_micros)
            .unwrap_or(Duration::from_nanos(16_666_667)),
    );
    slint::platform::set_platform(Box::new(MisterPlatform::new(
        window.clone(),
        animation_clock.platform_time(),
    )))
    .expect("set_platform");
    boot_analytics::event("slint_platform_set", "ok=1");

    match scene.as_str() {
        #[cfg(all(target_os = "linux", target_arch = "arm"))]
        "video_playback" => {
            let pad = open_pads();
            with_scene_app!(video_playback::VideoPlayback, &ui, &window, app, {
                app.show().expect("show");
                window.request_redraw();
                run_video_playback_loop(secs, &ui, &mut disp, &window, pad, app, &animation_clock);
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
        "tear_pattern" => {
            with_scene_app!(tear_pattern::TearPattern, &ui, &window, app, {
                app.show().expect("show");
                window.request_redraw();
                run_tear_pattern_loop(secs, &ui, &mut disp, &window, &animation_clock);
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
                    &mut display_session,
                    &window,
                    &mut target,
                    pad,
                    app,
                    &animation_clock,
                    launch_return_cpu_profile,
                );
            });
        }
        _ => unreachable!(),
    }
}
