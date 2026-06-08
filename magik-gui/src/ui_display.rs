//! HDMI framebuffer vs Slint render buffer.
//!
//! Slint layouts stay at **`MisterUi.scale = 1`**. The normal UI path uses a
//! small 960x540 framebuffer and lets the MiSTer FPGA scale it to HDMI.

/// Slint global — always 1; layout math uses base units only.
pub const SLINT_UI_SCALE: i32 = 1;

pub const UI_FB_W: usize = 960;
pub const UI_FB_H: usize = 540;
pub const UI_HDMI_W: u16 = 1920;
pub const UI_HDMI_H: u16 = 1080;

pub struct UiDisplay {
    fb_w: usize,
    fb_h: usize,
    render_w: usize,
    render_h: usize,
}

impl UiDisplay {
    pub fn for_framebuffer(fb_w: usize, fb_h: usize) -> Self {
        Self {
            fb_w,
            fb_h,
            render_w: fb_w,
            render_h: fb_h,
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
        1
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

#[cfg(test)]
mod tests {
    use super::UiDisplay;

    #[test]
    fn uses_framebuffer_size_for_1080p_modes() {
        let ui = UiDisplay::for_framebuffer(1920, 1080);
        assert_eq!(ui.render_w(), 1920);
        assert_eq!(ui.render_h(), 1080);
        assert_eq!(ui.fb_scale(), 1);
    }

    #[test]
    fn uses_native_size_for_non_1080p_modes() {
        let ui = UiDisplay::for_framebuffer(960, 540);
        assert_eq!(ui.render_w(), 960);
        assert_eq!(ui.render_h(), 540);
        assert_eq!(ui.fb_scale(), 1);
    }
}
