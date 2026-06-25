use super::*;
use mister_magik_fb::fb_format::{rgb565_stride_bytes, RGB565_BITS_PER_PIXEL};

const DEFAULT_BOOT_BLACK_SETTLE_FRAMES: u32 = 4;
const MAX_BOOT_BLACK_SETTLE_FRAMES: u32 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FbModeAction {
    AdoptCurrent,
    WriteMode,
}

impl FbModeAction {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::AdoptCurrent => "adopt",
            Self::WriteMode => "write",
        }
    }
}

pub(crate) struct FbModeGuard {
    previous: crate::fb::FbInfo,
    active: bool,
}

impl FbModeGuard {
    #[allow(dead_code)]
    pub(crate) fn set_temporary(w: usize, h: usize) -> std::io::Result<Self> {
        let previous = Display::current_info()?;
        Display::write_mister_mode_rgb565(w, h, rgb565_stride_bytes(w))?;
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

pub(crate) fn fb_mode_action(
    current: crate::fb::FbInfo,
    plan: crate::ui_display::UiDisplayPlan,
) -> FbModeAction {
    let expected_bpp = RGB565_BITS_PER_PIXEL;
    let expected_stride = rgb565_stride_bytes(plan.fb_w);
    if current.visible_w == plan.fb_w
        && current.visible_h == plan.fb_h
        && current.virtual_w == plan.fb_w
        && current.virtual_h == plan.fb_h
        && current.stride_bytes == expected_stride
        && current.bits_per_pixel == expected_bpp
    {
        FbModeAction::AdoptCurrent
    } else {
        FbModeAction::WriteMode
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LauncherFramebufferRoute {
    mode: Mode,
    set_vga_fb: bool,
}

impl LauncherFramebufferRoute {
    pub(crate) fn for_ui(ui: &UiDisplay) -> Self {
        Self::for_scan(ui.scan_w(), ui.scan_h(), ui.direct_video())
    }

    pub(crate) fn for_plan(plan: crate::ui_display::UiDisplayPlan) -> Self {
        Self::for_scan(plan.scan_w, plan.scan_h, plan.direct_video)
    }

    fn for_scan(scan_w: u16, scan_h: u16, set_vga_fb: bool) -> Self {
        Self {
            mode: ui_fpga_scaled_mode(scan_w, scan_h),
            set_vga_fb,
        }
    }

    pub(crate) fn enable(
        self,
        f: &mut Fpga,
        fb_width: usize,
        fb_height: usize,
    ) -> std::io::Result<u16> {
        f.fb_enable_rgb565(
            0,
            fb_width as u16,
            fb_height as u16,
            self.mode,
            self.set_vga_fb,
        )
    }

    pub(crate) fn mode(self) -> Mode {
        self.mode
    }

    pub(crate) fn set_vga_fb(self) -> bool {
        self.set_vga_fb
    }
}

pub(crate) struct UiBootFramebufferSession {
    pub(crate) ui: UiDisplay,
    pub(crate) disp: Display,
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

        let current_fb = match Display::current_info() {
            Ok(info) => info,
            Err(e) => {
                eprintln!("failed to read current framebuffer mode for FPGA-scaled UI: {e}");
                std::process::exit(1);
            }
        };
        let fb_mode_action = fb_mode_action(current_fb, display_plan);
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
        let mut disp = match Display::open_rgb565(display_plan.fb_w, display_plan.fb_h) {
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

        let route = LauncherFramebufferRoute::for_ui(&ui);
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
        let support_flag = match route.enable(f, disp.width(), disp.height()) {
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

pub(crate) fn ui_fpga_scaled_mode(scan_w: u16, scan_h: u16) -> Mode {
    Mode {
        hact: scan_w,
        hbp: 3,
        vact: scan_h,
        vbp: 2,
    }
}

pub(crate) fn settle_boot_black_frame(
    label: &str,
    disp: &mut Display,
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
        match route.enable(f, disp.width(), disp.height()) {
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
    use crate::fb::FbInfo;
    use crate::ui_display::UiDisplayPlan;

    fn fb_info(w: usize, h: usize, stride_bytes: usize, bits_per_pixel: u32) -> FbInfo {
        FbInfo {
            visible_w: w,
            visible_h: h,
            virtual_w: w,
            virtual_h: h,
            stride_bytes,
            bits_per_pixel,
            red_offset: 11,
            green_offset: 5,
            blue_offset: 0,
            transp_offset: 0,
        }
    }

    #[test]
    fn matching_rgb565_framebuffer_mode_is_adopted() {
        let plan = UiDisplayPlan::from_mister_ini_text("[Menu]\nvideo_mode=8\n").expect("plan");
        let current = fb_info(960, 540, 1920, 16);

        assert_eq!(fb_mode_action(current, plan), FbModeAction::AdoptCurrent);
        assert_eq!(FbModeAction::AdoptCurrent.label(), "adopt");
    }

    #[test]
    fn mismatched_framebuffer_mode_is_rewritten() {
        let plan = UiDisplayPlan::from_mister_ini_text("[Menu]\nvideo_mode=8\n").expect("plan");

        assert_eq!(
            fb_mode_action(fb_info(1920, 1080, 3840, 16), plan),
            FbModeAction::WriteMode
        );
        assert_eq!(
            fb_mode_action(fb_info(960, 540, 3840, 32), plan),
            FbModeAction::WriteMode
        );
        assert_eq!(FbModeAction::WriteMode.label(), "write");
    }

    #[test]
    fn launcher_framebuffer_route_for_plan_uses_scan_dimensions_and_direct_video() {
        let plan =
            UiDisplayPlan::from_mister_ini_text("[Menu]\nvideo_mode=8\n[MiSTer]\ndirect_video=1\n")
                .expect("plan");

        let route = LauncherFramebufferRoute::for_plan(plan);

        assert_eq!(route.mode().hact, plan.scan_w);
        assert_eq!(route.mode().vact, plan.scan_h);
        assert!(route.set_vga_fb());
    }
}
