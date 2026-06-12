use super::*;

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

pub(crate) fn ui_fpga_scaled_mode() -> Mode {
    Mode {
        hact: UI_HDMI_W,
        hbp: 3,
        vact: UI_HDMI_H,
        vbp: 2,
    }
}
