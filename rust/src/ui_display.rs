//! HDMI framebuffer size and Slint `ui-scale` from the environment.
//!
//! Layout math lives in `.slint` (`960 * ui-scale`, etc.). Rust only reads
//! `MISTER_RENDER_SCALE` and applies fb upscale when scale=1 (960→1920).

pub const FB_W: usize = 1920;
pub const FB_H: usize = 1080;

/// Slint `ui-scale` (1 = 960×540, 2 = 1920×1080). Env: `MISTER_RENDER_SCALE`.
pub fn ui_scale_from_env() -> i32 {
    std::env::var("MISTER_RENDER_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n == 1 || n == 2)
        .unwrap_or(1)
}

pub struct UiDisplay {
    pub scale: i32,
}

impl UiDisplay {
    pub fn from_env() -> Self {
        Self {
            scale: ui_scale_from_env(),
        }
    }

    pub fn render_w(&self) -> usize {
        960 * self.scale as usize
    }

    pub fn render_h(&self) -> usize {
        540 * self.scale as usize
    }

    /// Nearest upscale to `/dev/fb0` when render buffer is smaller than HDMI.
    pub fn fb_scale(&self) -> usize {
        FB_W / self.render_w()
    }

    pub fn log_line(&self) -> String {
        format!(
            "ui-scale={} render={}x{} fb={FB_W}x{FB_H} fb_scale={}",
            self.scale,
            self.render_w(),
            self.render_h(),
            self.fb_scale()
        )
    }
}
