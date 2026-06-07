//! HDMI framebuffer vs Slint render buffer.
//!
//! Slint layouts stay at **`MisterUi.scale = 1`**. The framebuffer size is
//! discovered at runtime; when the current framebuffer is exactly 1920x1080, we
//! keep the 960x540 pixel-art render surface and copy it at 2x.

/// Slint global — always 1; layout math uses base units only.
pub const SLINT_UI_SCALE: i32 = 1;

const PIXEL_ART_W: usize = 960;
const PIXEL_ART_H: usize = 540;
const PIXEL_ART_SCALE: usize = 2;

pub struct UiDisplay {
    fb_w: usize,
    fb_h: usize,
    render_w: usize,
    render_h: usize,
    fb_scale: usize,
}

impl UiDisplay {
    pub fn for_framebuffer(fb_w: usize, fb_h: usize) -> Self {
        if fb_w == PIXEL_ART_W * PIXEL_ART_SCALE && fb_h == PIXEL_ART_H * PIXEL_ART_SCALE {
            return Self {
                fb_w,
                fb_h,
                render_w: PIXEL_ART_W,
                render_h: PIXEL_ART_H,
                fb_scale: PIXEL_ART_SCALE,
            };
        }

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

#[cfg(test)]
mod tests {
    use super::UiDisplay;

    #[test]
    fn pixel_doubles_at_1080p() {
        let ui = UiDisplay::for_framebuffer(1920, 1080);
        assert_eq!(ui.render_w(), 960);
        assert_eq!(ui.render_h(), 540);
        assert_eq!(ui.fb_scale(), 2);
    }

    #[test]
    fn uses_native_size_for_non_1080p_modes() {
        let ui = UiDisplay::for_framebuffer(960, 540);
        assert_eq!(ui.render_w(), 960);
        assert_eq!(ui.render_h(), 540);
        assert_eq!(ui.fb_scale(), 1);
    }
}
