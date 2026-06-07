//! HDMI framebuffer vs Slint render buffer.
//!
//! Slint layouts stay at **`MisterUi.scale = 1`** (960×540 design). Rust always
//! renders that size and nearest-upscales to 1920×1080 on `/dev/fb0`.
//!
//! `MISTER_RENDER_SCALE` / `MISTER_PIXEL_SCALE` are kept for bench labelling and
//! a future native-1080p path; they do not multiply Slint layout today.

pub const FB_W: usize = 1920;
pub const FB_H: usize = 1080;
pub const UI_BASE_W: usize = 960;
pub const UI_BASE_H: usize = 540;

/// Slint global — always 1; layout math uses base units only.
pub const SLINT_UI_SCALE: i32 = 1;

/// Bench label from env (1 = half-res render path, 2 = native 1080p — not enabled yet).
pub fn bench_render_scale_from_env() -> usize {
    if let Ok(v) = std::env::var("MISTER_RENDER_SCALE") {
        if let Ok(n) = v.parse::<usize>() {
            if n == 1 || n == 2 {
                return n;
            }
        }
    }
    if let Ok(v) = std::env::var("MISTER_PIXEL_SCALE") {
        if let Ok(n) = v.parse::<usize>() {
            return match n {
                1 => 2,
                2 => 1,
                _ => 1,
            };
        }
    }
    1
}

pub struct UiDisplay {
    /// From env — for logs/TSV only until native 1080p render is wired up.
    pub bench_render_scale: usize,
}

impl UiDisplay {
    pub fn from_env() -> Self {
        Self {
            bench_render_scale: bench_render_scale_from_env(),
        }
    }

    pub fn render_w(&self) -> usize {
        UI_BASE_W
    }

    pub fn render_h(&self) -> usize {
        UI_BASE_H
    }

    pub fn fb_scale(&self) -> usize {
        FB_W / self.render_w()
    }

    pub fn log_line(&self) -> String {
        format!(
            "slint-scale={SLINT_UI_SCALE} render={}x{} fb={FB_W}x{FB_H} fb_scale={} bench_render_scale={}",
            self.render_w(),
            self.render_h(),
            self.fb_scale(),
            self.bench_render_scale
        )
    }
}
