//! HDMI framebuffer vs Slint render buffer.
//!
//! Slint layouts stay at **`MisterUi.scale = 1`** and render 1:1 to the current
//! MiSTer framebuffer. The framebuffer size is discovered at runtime.

/// Slint global — always 1; layout math uses base units only.
pub const SLINT_UI_SCALE: i32 = 1;

pub struct UiDisplay {
    fb_w: usize,
    fb_h: usize,
    render_w: usize,
    render_h: usize,
    fb_scale: usize,
}

impl UiDisplay {
    pub fn for_framebuffer(fb_w: usize, fb_h: usize) -> Self {
        Self {
            fb_w,
            fb_h,
            render_w: fb_w,
            render_h: fb_h,
            fb_scale: 1,
        }
    }

    pub fn render_w(&self) -> usize {
        self.render_w
    }

    pub fn render_h(&self) -> usize {
        self.render_h
    }

    pub fn fb_w(&self) -> usize {
        self.fb_w
    }

    pub fn fb_h(&self) -> usize {
        self.fb_h
    }

    pub fn fb_scale(&self) -> usize {
        self.fb_scale
    }

    pub fn log_line(&self) -> String {
        format!(
            "slint-scale={SLINT_UI_SCALE} render={}x{} fb={}x{} fb_scale={}",
            self.render_w(),
            self.render_h(),
            self.fb_w,
            self.fb_h,
            self.fb_scale()
        )
    }
}
