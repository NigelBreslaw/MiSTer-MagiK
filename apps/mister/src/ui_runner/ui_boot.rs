// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use mister_magik_fb::framebuffer::mode::{FbModeAction, FbModeGuard, fb_mode_action};

const DEFAULT_BOOT_BLACK_SETTLE_FRAMES: u32 = 4;
const MAX_BOOT_BLACK_SETTLE_FRAMES: u32 = 60;

pub struct UiBootFramebufferSession {
    pub ui: UiDisplay,
    pub disp: MappedRgb565Framebuffer,
    pub display_session: LauncherDisplaySession,
    pub _fb_mode_guard: Option<FbModeGuard>,
}

impl UiBootFramebufferSession {
    pub fn start_ui_or_exit(
        f: &mut Fpga,
        config: &mister_magik_fb::process_config::DisplayPacingConfig,
    ) -> Self {
        let runtime_geometry = detect_runtime_display_geometry_for_plan(f, "ui");
        let display_plan = UiDisplayPlan::from_runtime_or_mister_ini_file_with_inputs(
            runtime_geometry,
            config.display_inputs(),
        );
        crate::ui_logln!("{}", display_plan.log_line());
        if display_plan.fallback {
            boot_analytics::event("display_plan_fallback", display_plan.log_line());
        }
        crate::ui_logln!(
            "ui-fb-mode=temporary {}x{} format={} output={}x{} scan={}x{} restore=on-drop",
            display_plan.fb_w,
            display_plan.fb_h,
            production_label(),
            display_plan.output_w,
            display_plan.output_h,
            display_plan.scan_w,
            display_plan.scan_h
        );

        let current_fb = match MappedRgb565Framebuffer::current_info() {
            Ok(info) => info,
            Err(e) => {
                crate::ui_errln!("failed to read current framebuffer mode for FPGA-scaled UI: {e}");
                std::process::exit(1);
            }
        };
        let fb_mode_action = fb_mode_action(current_fb, display_plan.fb_w, display_plan.fb_h);
        crate::ui_logln!("fb_mode_action={}", fb_mode_action.label());
        boot_analytics::event("fb_mode_action", fb_mode_action.label());
        let fb_mode_guard = match fb_mode_action {
            FbModeAction::AdoptCurrent => None,
            FbModeAction::WriteMode => {
                match FbModeGuard::set_temporary(display_plan.fb_w, display_plan.fb_h) {
                    Ok(guard) => Some(guard),
                    Err(e) => {
                        crate::ui_errln!(
                            "failed to set temporary framebuffer mode for FPGA-scaled UI: {e}"
                        );
                        std::process::exit(1);
                    }
                }
            }
        };

        crate::ui_logln!("display-open-path=temporary-fb-fpga-scale");
        let mut disp =
            match MappedRgb565Framebuffer::open_rgb565(display_plan.fb_w, display_plan.fb_h) {
                Ok(d) => d,
                Err(e) => {
                    crate::ui_errln!("failed to open display (/dev/fb0): {e}");
                    std::process::exit(1);
                }
            };
        let ui = UiDisplay::for_plan(display_plan)
            .with_crt_font_experiment(config.display_inputs().crt_font_experiment());
        crate::ui_logln!("{}", ui.log_line());
        disp.clear_black();
        boot_analytics::event(
            "ui_black_frame_copied",
            format!(
                "format={} w={} h={}",
                production_label(),
                disp.width(),
                disp.height()
            ),
        );
        disp.record_visual_sample("after_ui_black_frame_before_initial_route");
        match DisplayConfig::detect(f, disp.info(), &ui) {
            Ok(config) => {
                crate::ui_logln!("{}", config.log_line());
                boot_analytics::event("display_config_detected", config.boot_analytics_detail());
            }
            Err(e) => {
                crate::ui_errln!("warning: failed to read display configuration from FPGA: {e}");
                boot_analytics::event("display_config_detect_failed", format!("error={e}"));
            }
        }
        if std::env::var_os("MISTER_MAGIK_PARENT").is_some() {
            crate::ui_logln!("MiSTer_MagiK parent detected; Slint reasserting framebuffer route");
        }

        let mut display_session = LauncherDisplaySession::with_guard(
            &ui,
            mister_magik_fb::framebuffer::ownership::FramebufferRouteGuard::new(
                config.route_reassert_frames(),
            ),
        );
        let route = display_session.route();
        boot_analytics::event(
            "initial_fb_enable_direct_attempt",
            format!(
                "w={} h={} mode=fpga-scale-scan scan={}x{} direct_video={}",
                disp.width(),
                disp.height(),
                ui.scan_w(),
                ui.scan_h(),
                route.direct_video()
            ),
        );
        let support_flag = match display_session.enable_initial(f) {
            Ok(flag) => flag,
            Err(e) => {
                crate::ui_errln!("failed to route framebuffer for Slint UI: {e}");
                std::process::exit(1);
            }
        };
        boot_analytics::event(
            "initial_fb_enable_direct_done",
            format!("support_flag={support_flag}"),
        );
        boot_analytics::event(
            "rust_framebuffer_route_completed",
            format!(
                "format={} w={} h={} output={}x{} scan={}x{} support_flag={support_flag}",
                production_label(),
                disp.width(),
                disp.height(),
                ui.output_w(),
                ui.output_h(),
                ui.scan_w(),
                ui.scan_h()
            ),
        );
        if fb_mode_action == FbModeAction::WriteMode {
            settle_boot_black_frame("ui-startup", &mut disp, f, &mut display_session);
        }
        disp.record_visual_sample("after_initial_route_before_slint_draw");
        crate::ui_logln!(
            "fb routed (support_flag={support_flag}); Slint software renderer (vsync, dirty-row copy, fpga_scale=true)"
        );

        Self {
            ui,
            disp,
            display_session,
            _fb_mode_guard: fb_mode_guard,
        }
    }
}

pub fn settle_boot_black_frame(
    label: &str,
    disp: &mut MappedRgb565Framebuffer,
    f: &mut Fpga,
    display_session: &mut LauncherDisplaySession,
) {
    let frames = boot_black_settle_frames();
    if frames == 0 {
        boot_analytics::event(
            "boot_black_settle_skipped",
            format!("label={label} frames=0"),
        );
        return;
    }

    let mut routed = 0u32;
    let mut last_flag = 0u16;
    for _ in 0..frames {
        disp.clear_black();
        match display_session.enable_boot_settle(f) {
            Ok(flag) => {
                routed += 1;
                last_flag = flag;
            }
            Err(e) => {
                crate::ui_errln!(
                    "warning: failed to reassert black framebuffer route during {label}: {e}"
                );
                boot_analytics::event(
                    "boot_black_settle_failed",
                    format!("label={label} frame={routed} error={e}"),
                );
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(17));
    }

    boot_analytics::event(
        "boot_black_settle_completed",
        format!(
            "label={label} frames={frames} routed={routed} support_flag={last_flag} format={} w={} h={} scan={}x{}",
            production_label(),
            disp.width(),
            disp.height(),
            display_session.route().mode().hact,
            display_session.route().mode().vact
        ),
    );
}

pub fn detect_runtime_display_geometry_for_plan(
    f: &mut Fpga,
    label: &str,
) -> Option<RuntimeDisplayGeometry> {
    match detect_runtime_display_geometry(f) {
        Ok(detected) => {
            crate::ui_logln!("runtime-video-info[{label}]: {}", detected.video.log_line());
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
                    crate::ui_errln!(
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
            crate::ui_errln!("warning: failed to detect runtime display geometry for {label}: {e}");
            boot_analytics::event(
                "runtime_display_geometry_detect_failed",
                format!("label={label} error={e}"),
            );
            None
        }
    }
}

fn boot_black_settle_frames() -> u32 {
    std::env::var("MISTER_BOOT_BLACK_SETTLE_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(DEFAULT_BOOT_BLACK_SETTLE_FRAMES)
        .min(MAX_BOOT_BLACK_SETTLE_FRAMES)
}

#[cfg(test)]
mod tests {
    use crate::ui_display::UiDisplayPlan;
    use mister_magik_fb::framebuffer::route::LauncherFramebufferRoute;

    #[test]
    fn launcher_framebuffer_route_for_plan_uses_scan_dimensions_and_direct_video() {
        let plan =
            UiDisplayPlan::from_mister_ini_text("[Menu]\nvideo_mode=8\n[MiSTer]\ndirect_video=1\n")
                .expect("plan");

        let route = LauncherFramebufferRoute::for_scan(plan.scan_w, plan.scan_h, plan.direct_video);

        assert_eq!(route.mode().hact, plan.scan_w);
        assert_eq!(route.mode().vact, plan.scan_h);
        assert!(route.direct_video());
    }

    #[test]
    fn pal_576p_route_uses_native_scanout_geometry() {
        let plan = UiDisplayPlan::from_mister_ini_text(
            "[MiSTer]\ndirect_video=1\nmenu_pal=1\nforced_scandoubler=1\n",
        )
        .expect("plan");

        let route = LauncherFramebufferRoute::for_scan(plan.scan_w, plan.scan_h, plan.direct_video);

        assert_eq!((plan.render_w, plan.render_h), (640, 576));
        assert_eq!((plan.fb_w, plan.fb_h), (640, 576));
        assert_eq!((route.mode().hact, route.mode().vact), (640, 576));
        assert!(route.direct_video());
    }
}
