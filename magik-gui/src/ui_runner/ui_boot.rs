use super::*;

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

pub(crate) fn fb_mode_action(
    current: crate::fb::FbInfo,
    plan: crate::ui_display::UiDisplayPlan,
    format: FramebufferFormat,
) -> FbModeAction {
    let expected_bpp = (format.bytes_per_pixel() * 8) as u32;
    let expected_stride = format.stride_bytes(plan.fb_w);
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

pub(crate) fn boot_framebuffer_format() -> FramebufferFormat {
    FramebufferFormat::production_default()
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
    route_mode: Mode,
    set_vga_fb: bool,
    format: FramebufferFormat,
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
        match f.fb_enable_format(
            0,
            disp.width() as u16,
            disp.height() as u16,
            route_mode,
            Some(0),
            Some(0),
            set_vga_fb,
            format,
        ) {
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
            format.label(),
            disp.width(),
            disp.height(),
            route_mode.hact,
            route_mode.vact
        ),
    );
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

        assert_eq!(
            fb_mode_action(current, plan, FramebufferFormat::Rgb565),
            FbModeAction::AdoptCurrent
        );
        assert_eq!(FbModeAction::AdoptCurrent.label(), "adopt");
    }

    #[test]
    fn mismatched_framebuffer_mode_is_rewritten() {
        let plan = UiDisplayPlan::from_mister_ini_text("[Menu]\nvideo_mode=8\n").expect("plan");

        assert_eq!(
            fb_mode_action(
                fb_info(1920, 1080, 3840, 16),
                plan,
                FramebufferFormat::Rgb565
            ),
            FbModeAction::WriteMode
        );
        assert_eq!(
            fb_mode_action(fb_info(960, 540, 3840, 32), plan, FramebufferFormat::Rgb565),
            FbModeAction::WriteMode
        );
        assert_eq!(FbModeAction::WriteMode.label(), "write");
    }

    #[test]
    fn boot_framebuffer_format_ignores_diagnostic_format_override_policy() {
        assert_eq!(
            FramebufferFormat::from_label("8888"),
            Some(FramebufferFormat::Xrgb8888)
        );
        assert_eq!(boot_framebuffer_format(), FramebufferFormat::Rgb565);
    }
}
