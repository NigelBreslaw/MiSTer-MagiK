//! HDMI framebuffer vs Slint render buffer.
//!
//! Slint layouts stay at **`MisterUi.scale = 1`**. The legacy launcher path keeps
//! the 960×540 design and nearest-upscales to 1920×1080. Benchmark scenes can
//! render 1:1 to the current framebuffer when the MiSTer mode is already low-res.
//!
//! `MISTER_RENDER_SCALE` / `MISTER_PIXEL_SCALE` are kept for bench labelling.

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
    fb_w: usize,
    fb_h: usize,
    render_w: usize,
    render_h: usize,
    fb_scale: usize,
    pub bench_render_scale: usize,
}

impl UiDisplay {
    pub fn from_env() -> Self {
        Self {
            fb_w: FB_W,
            fb_h: FB_H,
            render_w: UI_BASE_W,
            render_h: UI_BASE_H,
            fb_scale: 2,
            bench_render_scale: bench_render_scale_from_env(),
        }
    }

    pub fn for_framebuffer(fb_w: usize, fb_h: usize, legacy_1080p: bool) -> Self {
        let bench_render_scale = bench_render_scale_from_env();
        if legacy_1080p || (fb_w == FB_W && fb_h == FB_H) {
            return Self {
                fb_w,
                fb_h,
                render_w: UI_BASE_W,
                render_h: UI_BASE_H,
                fb_scale: 2,
                bench_render_scale,
            };
        }
        Self {
            fb_w,
            fb_h,
            render_w: fb_w,
            render_h: fb_h,
            fb_scale: 1,
            bench_render_scale,
        }
    }

    pub fn render_w(&self) -> usize {
        self.render_w
    }

    pub fn render_h(&self) -> usize {
        self.render_h
    }

    pub fn fb_scale(&self) -> usize {
        self.fb_scale
    }

    pub fn log_line(&self) -> String {
        format!(
            "slint-scale={SLINT_UI_SCALE} render={}x{} fb={}x{} fb_scale={} bench_render_scale={}",
            self.render_w(),
            self.render_h(),
            self.fb_w,
            self.fb_h,
            self.fb_scale(),
            self.bench_render_scale
        )
    }
}
