use crate::framebuffer::format::{rgb565_stride_bytes, RGB565_BITS_PER_PIXEL};
use crate::framebuffer::mapped::{FbInfo, MappedRgb565Framebuffer};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FbModeAction {
    AdoptCurrent,
    WriteMode,
}

impl FbModeAction {
    pub const fn label(self) -> &'static str {
        match self {
            Self::AdoptCurrent => "adopt",
            Self::WriteMode => "write",
        }
    }
}

pub struct FbModeGuard {
    previous: FbInfo,
    active: bool,
}

impl FbModeGuard {
    #[allow(dead_code)]
    pub fn set_temporary(w: usize, h: usize) -> std::io::Result<Self> {
        let previous = MappedRgb565Framebuffer::current_info()?;
        MappedRgb565Framebuffer::write_mister_mode_rgb565(w, h, rgb565_stride_bytes(w))?;
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
        match MappedRgb565Framebuffer::restore_mister_mode(self.previous) {
            Ok(()) => {
                clear_fb_mode_for_exit();
                self.active = false;
            }
            Err(e) => {
                crate::ui_errln!("warning: failed to restore framebuffer mode: {e}");
            }
        }
    }
}

impl Drop for FbModeGuard {
    fn drop(&mut self) {
        self.restore_now();
    }
}

pub fn fb_mode_action(current: FbInfo, fb_w: usize, fb_h: usize) -> FbModeAction {
    let expected_bpp = RGB565_BITS_PER_PIXEL;
    let expected_stride = rgb565_stride_bytes(fb_w);
    if current.visible_w == fb_w
        && current.visible_h == fb_h
        && current.virtual_w == fb_w
        && current.virtual_h == fb_h
        && current.stride_bytes == expected_stride
        && current.bits_per_pixel == expected_bpp
    {
        FbModeAction::AdoptCurrent
    } else {
        FbModeAction::WriteMode
    }
}

static FB_MODE_RESTORE: Mutex<Option<FbInfo>> = Mutex::new(None);
static FB_MODE_RESTORE_ATEXIT: OnceLock<()> = OnceLock::new();

fn remember_fb_mode_for_exit(previous: FbInfo) {
    FB_MODE_RESTORE_ATEXIT.get_or_init(|| unsafe {
        // SAFETY: restore_fb_mode_at_exit is an extern "C" function with no
        // captured state. It performs best-effort cleanup and does not unwind.
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
        let _ = MappedRgb565Framebuffer::restore_mister_mode(previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let current = fb_info(960, 540, 1920, 16);

        assert_eq!(
            fb_mode_action(current, 960, 540),
            FbModeAction::AdoptCurrent
        );
        assert_eq!(
            fb_mode_action(fb_info(1280, 720, 2560, 16), 1280, 720),
            FbModeAction::AdoptCurrent
        );
        assert_eq!(FbModeAction::AdoptCurrent.label(), "adopt");
    }

    #[test]
    fn mismatched_framebuffer_mode_is_rewritten() {
        assert_eq!(
            fb_mode_action(fb_info(1920, 1080, 3840, 16), 960, 540),
            FbModeAction::WriteMode
        );
        assert_eq!(
            fb_mode_action(fb_info(960, 540, 3840, 32), 960, 540),
            FbModeAction::WriteMode
        );
        assert_eq!(
            fb_mode_action(fb_info(1280, 720, 3840, 16), 1280, 720),
            FbModeAction::WriteMode
        );
        assert_eq!(FbModeAction::WriteMode.label(), "write");
    }
}
