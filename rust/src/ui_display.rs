//! Slint render vs HDMI framebuffer sizing (P2 pixel-scale experiment).

pub const FB_W: usize = 1920;
pub const FB_H: usize = 1080;

/// Integer upscale from logical render buffer to `/dev/fb0` (nearest-neighbor).
/// Set via `MISTER_PIXEL_SCALE` (default **2** for 960×540 → 1080p).
pub fn pixel_scale() -> usize {
    std::env::var("MISTER_PIXEL_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| (1..=4).contains(&n) && FB_W.is_multiple_of(n) && FB_H.is_multiple_of(n))
        .unwrap_or(2)
}

pub struct UiDisplay {
    pub pixel_scale: usize,
    pub render_w: usize,
    pub render_h: usize,
}

impl UiDisplay {
    pub fn from_env() -> Self {
        let pixel_scale = pixel_scale();
        Self {
            render_w: FB_W / pixel_scale,
            render_h: FB_H / pixel_scale,
            pixel_scale,
        }
    }

    pub fn log_line(&self) -> String {
        if self.pixel_scale > 1 {
            format!(
                "pixel_scale={} render={}x{} fb={}x{} font=PressStart2P",
                self.pixel_scale, self.render_w, self.render_h, FB_W, FB_H
            )
        } else {
            format!("pixel_scale=1 render={}x{} fb={}x{}", self.render_w, self.render_h, FB_W, FB_H)
        }
    }
}
