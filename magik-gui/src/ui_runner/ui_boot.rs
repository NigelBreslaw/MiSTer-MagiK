use super::*;
use mister_magik_fb::framebuffer::mode::{fb_mode_action, FbModeAction, FbModeGuard};
use mister_magik_fb::framebuffer::route::LauncherFramebufferRoute;

const DEFAULT_BOOT_BLACK_SETTLE_FRAMES: u32 = 4;
const MAX_BOOT_BLACK_SETTLE_FRAMES: u32 = 60;

pub(crate) struct UiBootFramebufferSession {
    pub(crate) ui: UiDisplay,
    pub(crate) disp: MappedRgb565Framebuffer,
    pub(crate) _fb_mode_guard: Option<FbModeGuard>,
}

impl UiBootFramebufferSession {
    pub(crate) fn start_ui_or_exit(f: &mut Fpga) -> Self {
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
            production_label(),
            display_plan.output_w,
            display_plan.output_h,
            display_plan.scan_w,
            display_plan.scan_h
        );

        let current_fb = match MappedRgb565Framebuffer::current_info() {
            Ok(info) => info,
            Err(e) => {
                eprintln!("failed to read current framebuffer mode for FPGA-scaled UI: {e}");
                std::process::exit(1);
            }
        };
        let fb_mode_action = fb_mode_action(current_fb, display_plan.fb_w, display_plan.fb_h);
        println!("fb_mode_action={}", fb_mode_action.label());
        boot_analytics::event("fb_mode_action", fb_mode_action.label());
        let fb_mode_guard = match fb_mode_action {
            FbModeAction::AdoptCurrent => None,
            FbModeAction::WriteMode => {
                match FbModeGuard::set_temporary(display_plan.fb_w, display_plan.fb_h) {
                    Ok(guard) => Some(guard),
                    Err(e) => {
                        eprintln!(
                            "failed to set temporary framebuffer mode for FPGA-scaled UI: {e}"
                        );
                        std::process::exit(1);
                    }
                }
            }
        };

        println!("display-open-path=temporary-fb-fpga-scale");
        let mut disp =
            match MappedRgb565Framebuffer::open_rgb565(display_plan.fb_w, display_plan.fb_h) {
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
                production_label(),
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

        let route = LauncherFramebufferRoute::for_scan(ui.scan_w(), ui.scan_h(), ui.direct_video());
        boot_analytics::event(
            "initial_fb_enable_direct_attempt",
            format!(
                "w={} h={} mode=fpga-scale-scan scan={}x{} set_vga_fb={}",
                disp.width(),
                disp.height(),
                ui.scan_w(),
                ui.scan_h(),
                route.set_vga_fb()
            ),
        );
        let support_flag =
            match f.enable_launcher_framebuffer_route(route, disp.width(), disp.height()) {
                Ok(flag) => flag,
                Err(e) => {
                    eprintln!("failed to route framebuffer for Slint UI: {e}");
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
            settle_boot_black_frame("ui-startup", &mut disp, f, route);
        }
        disp.record_visual_sample("after_initial_route_before_slint_draw");
        println!(
            "fb routed (support_flag={support_flag}); Slint software renderer (vsync, dirty-row copy, fpga_scale=true)"
        );

        Self {
            ui,
            disp,
            _fb_mode_guard: fb_mode_guard,
        }
    }
}

pub(crate) fn settle_boot_black_frame(
    label: &str,
    disp: &mut MappedRgb565Framebuffer,
    f: &mut Fpga,
    route: LauncherFramebufferRoute,
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
        match f.enable_launcher_framebuffer_route(route, disp.width(), disp.height()) {
            Ok(flag) => {
                routed += 1;
                last_flag = flag;
            }
            Err(e) => {
                eprintln!(
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
            route.mode().hact,
            route.mode().vact
        ),
    );
}

pub(crate) fn detect_runtime_display_geometry_for_plan(
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

fn boot_black_settle_frames() -> u32 {
    std::env::var("MISTER_BOOT_BLACK_SETTLE_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(DEFAULT_BOOT_BLACK_SETTLE_FRAMES)
        .min(MAX_BOOT_BLACK_SETTLE_FRAMES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_display::UiDisplayPlan;

    #[test]
    fn launcher_framebuffer_route_for_plan_uses_scan_dimensions_and_direct_video() {
        let plan =
            UiDisplayPlan::from_mister_ini_text("[Menu]\nvideo_mode=8\n[MiSTer]\ndirect_video=1\n")
                .expect("plan");

        let route = LauncherFramebufferRoute::for_scan(plan.scan_w, plan.scan_h, plan.direct_video);

        assert_eq!(route.mode().hact, plan.scan_w);
        assert_eq!(route.mode().vact, plan.scan_h);
        assert!(route.set_vga_fb());
    }
}
