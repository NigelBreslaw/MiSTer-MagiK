// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Native framebuffer effect benchmark scene.
#![cfg_attr(not(mister_experiments), allow(unused_imports, dead_code))]

use crate::display_config::DisplayConfig;
use crate::fpga::Fpga;
use crate::ui_display::UiDisplay;
use crate::ui_runner::ui_platform::{
    AnimationClock, MisterPlatform, MisterSoftwareWindow, update_slint_animations,
};
use crate::vt::VtGraphicsGuard;
use mister_magik_fb::experiments::effects::framebuffer_effects::{
    EffectKind, EffectSize, EffectState,
};
use mister_magik_fb::framebuffer::mapped::MappedRgb565Framebuffer;
use mister_magik_fb::framebuffer::route::LauncherFramebufferRoute;
use mister_magik_fb::framebuffer::vsync::VsyncPacer;
use slint::platform::software_renderer::{RepaintBufferType, Rgb565Pixel, TargetPixel};
use slint::{ComponentHandle, PhysicalSize};
use std::rc::Rc;
use std::time::Instant;

use mister_magik_ui as slint_ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(mister_experiments), allow(dead_code))]
enum EffectBenchMode {
    Raw,
    Overlay,
}

#[cfg_attr(not(mister_experiments), allow(dead_code))]
impl EffectBenchMode {
    fn label(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Overlay => "overlay",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(mister_experiments), allow(dead_code))]
pub enum EffectFill {
    Full,
    Half,
    Double,
    Native,
}

#[cfg_attr(not(mister_experiments), allow(dead_code))]
impl EffectFill {
    fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Half => "half",
            Self::Double => "2x",
            Self::Native => "native",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "full" => Some(Self::Full),
            "half" => Some(Self::Half),
            "2x" | "double" => Some(Self::Double),
            "native" => Some(Self::Native),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(mister_experiments), allow(dead_code))]
pub struct EffectTarget {
    pub physical_x: usize,
    pub physical_y: usize,
    pub physical_w: usize,
    pub physical_h: usize,
    pub render_w: usize,
    pub render_h: usize,
    pub scale: usize,
}

#[cfg_attr(not(mister_experiments), allow(dead_code))]
impl EffectTarget {
    pub fn new(fill: EffectFill, size: EffectSize, ui: &UiDisplay) -> Option<Self> {
        let (physical_w, physical_h, scale) = match fill {
            EffectFill::Full => (1920, 1080, size.scale_to_1080p()?),
            EffectFill::Half => match size.scale_to_half_1080p() {
                Some(scale) => (960, 540, scale),
                None if size.w <= 960 && size.h <= 540 => (size.w, size.h, 1),
                None => return None,
            },
            EffectFill::Double => (size.w.checked_mul(2)?, size.h.checked_mul(2)?, 2),
            EffectFill::Native => (size.w, size.h, 1),
        };
        if physical_w > ui.fb_w() || physical_h > ui.fb_h() {
            return None;
        }
        Some(Self {
            physical_x: (ui.fb_w() - physical_w) / 2,
            physical_y: (ui.fb_h() - physical_h) / 2,
            physical_w,
            physical_h,
            render_w: physical_w,
            render_h: physical_h,
            scale,
        })
    }
}

#[cfg_attr(not(mister_experiments), allow(dead_code))]
fn parse_effect_bench_args() -> (
    Vec<EffectKind>,
    u64,
    Vec<EffectBenchMode>,
    EffectSize,
    EffectFill,
) {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let effect_arg = args.first().map(String::as_str).unwrap_or("all");
    let effects = if effect_arg == "all" {
        EffectKind::all().to_vec()
    } else {
        match EffectKind::parse(effect_arg) {
            Some(kind) => vec![kind],
            None => {
                crate::ui_errln!("unknown effect '{effect_arg}' (use `effects` to list names)");
                std::process::exit(2);
            }
        }
    };
    let secs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let modes = match args.get(2).map(String::as_str).unwrap_or("both") {
        "raw" => vec![EffectBenchMode::Raw],
        "overlay" => vec![EffectBenchMode::Overlay],
        "both" => vec![EffectBenchMode::Raw, EffectBenchMode::Overlay],
        other => {
            crate::ui_errln!("unknown effect-bench mode '{other}' (use raw|overlay|both)");
            std::process::exit(2);
        }
    };
    let size = match args.get(3).map(String::as_str) {
        Some(s) => match EffectSize::parse(s) {
            Some(size) => size,
            None => {
                crate::ui_errln!(
                    "unsupported effect size '{s}' (use `effects` to list supported sizes)"
                );
                std::process::exit(2);
            }
        },
        None => EffectSize { w: 480, h: 270 },
    };
    let fill = match args.get(4).map(String::as_str) {
        Some(s) => EffectFill::parse(s).unwrap_or_else(|| {
            crate::ui_errln!("unknown effect fill '{s}' (use full|half|2x|native)");
            std::process::exit(2);
        }),
        None => EffectFill::Full,
    };
    (effects, secs, modes, size, fill)
}

#[derive(Default)]
#[cfg_attr(not(mister_experiments), allow(dead_code))]
struct EffectBenchTotals {
    frames: u64,
    effect_us: u128,
    slint_us: u128,
    scale_copy_us: u128,
    vsync_us: u128,
    wall_us: u128,
    slow_frames: u64,
}

#[cfg_attr(not(mister_experiments), allow(dead_code))]
impl EffectBenchTotals {
    fn record(
        &mut self,
        effect_us: u64,
        slint_us: u64,
        scale_copy_us: u64,
        vsync_us: u64,
        wall_us: u64,
    ) {
        self.frames += 1;
        self.effect_us += effect_us as u128;
        self.slint_us += slint_us as u128;
        self.scale_copy_us += scale_copy_us as u128;
        self.vsync_us += vsync_us as u128;
        self.wall_us += wall_us as u128;
        if wall_us >= 16_667 {
            self.slow_frames += 1;
        }
    }

    fn avg(v: u128, frames: u64) -> u64 {
        if frames == 0 {
            0
        } else {
            (v / frames as u128) as u64
        }
    }
}

#[cfg_attr(not(mister_experiments), allow(dead_code))]
fn rgb565_from_rgb888_word(pixel: u32) -> Rgb565Pixel {
    let p = pixel & 0x00ff_ffff;
    <Rgb565Pixel as TargetPixel>::from_rgb((p >> 16) as u8, (p >> 8) as u8, p as u8)
}

#[cfg_attr(not(mister_experiments), allow(dead_code))]
fn scale_effect_to_rgb565_fit(
    src: &[u32],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    dst: &mut [Rgb565Pixel],
) {
    assert!(dst.len() >= dst_w * dst_h);
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return;
    }
    for y in 0..dst_h {
        let sy = (y * src_h / dst_h).min(src_h - 1);
        let src_row = &src[sy * src_w..(sy + 1) * src_w];
        let dst_row = &mut dst[y * dst_w..(y + 1) * dst_w];
        for (x, p) in dst_row.iter_mut().enumerate() {
            let sx = (x * src_w / dst_w).min(src_w - 1);
            *p = rgb565_from_rgb888_word(src_row[sx]);
        }
    }
}

#[cfg(mister_experiments)]
fn present_effect_target(
    disp: &mut MappedRgb565Framebuffer,
    target: EffectTarget,
    pixels: &[Rgb565Pixel],
) {
    if let Err(e) = disp.present_rect_565(
        target.physical_x,
        target.physical_y,
        target.render_w,
        target.render_h,
        pixels,
    ) {
        crate::ui_errln!("effect-bench present failed: {e}");
    }
}

#[cfg(mister_experiments)]
pub fn run_effect_bench(f: &mut Fpga) {
    let (effects, secs, modes, size, fill) = parse_effect_bench_args();
    crate::ui_logln!(
        "effect-bench effects={} secs={} modes={} fill={} internal={}x{}",
        effects
            .iter()
            .map(|k| k.name())
            .collect::<Vec<_>>()
            .join(","),
        secs,
        modes
            .iter()
            .map(|m| m.label())
            .collect::<Vec<_>>()
            .join(","),
        fill.label(),
        size.w,
        size.h
    );

    let _vt = VtGraphicsGuard::enter_or_warn();
    let mut disp = match MappedRgb565Framebuffer::open_current_boot() {
        Ok(d) => d,
        Err(e) => {
            crate::ui_errln!("failed to open display (/dev/fb0): {e}");
            std::process::exit(1);
        }
    };
    let ui = UiDisplay::for_framebuffer(disp.width(), disp.height());
    let target = EffectTarget::new(fill, size, &ui).unwrap_or_else(|| {
        crate::ui_errln!(
            "effect size {}x{} cannot fill={} on framebuffer {}x{}",
            size.w,
            size.h,
            fill.label(),
            disp.width(),
            disp.height()
        );
        std::process::exit(2);
    });
    crate::ui_logln!("{}", ui.log_line());
    let display_config = match DisplayConfig::detect(f, disp.info(), &ui) {
        Ok(config) => config,
        Err(e) => {
            crate::ui_errln!("failed to read display configuration from FPGA: {e}");
            std::process::exit(1);
        }
    };
    crate::ui_logln!("{}", display_config.log_line());
    let route = LauncherFramebufferRoute::for_scan(ui.scan_w(), ui.scan_h(), ui.direct_video());
    let flag = match f.enable_launcher_framebuffer_route(route, disp.width(), disp.height()) {
        Ok(flag) => flag,
        Err(e) => {
            crate::ui_errln!("failed to route framebuffer for effect benchmark: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = f.set_audio_volume(0) {
        crate::ui_errln!("warning: failed to set FPGA audio volume: {e}");
    }
    crate::ui_logln!(
        "fb routed (support_flag={flag}); native retro effect benchmark checked_rgb565_present=true"
    );

    let needs_overlay = modes.contains(&EffectBenchMode::Overlay);
    let mut overlay_ctx = if needs_overlay {
        let window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let animation_clock = AnimationClock::from_env();
        slint::platform::set_platform(Box::new(MisterPlatform::new(
            window.clone(),
            animation_clock.platform_time(),
        )))
        .expect("set_platform");
        let app = slint_ui::effect_hud::EffectHud::new().expect("EffectHud");
        let mister_ui = app.global::<slint_ui::effect_hud::MisterUi>();
        mister_ui.set_window_width(target.render_w as i32);
        mister_ui.set_window_height(target.render_h as i32);
        window.set_size(PhysicalSize::new(
            target.render_w as u32,
            target.render_h as u32,
        ));
        app.show().expect("show");
        Some((
            window,
            app,
            animation_clock,
            vec![Rgb565Pixel(0); target.render_w * target.render_h],
        ))
    } else {
        None
    };

    let mut low = vec![0u32; size.w * size.h];
    for &kind in &effects {
        for &mode in &modes {
            run_one_effect_bench(
                &mut disp,
                &mut overlay_ctx,
                kind,
                mode,
                fill,
                size,
                target,
                secs,
                &mut low,
            );
        }
    }
}

#[cfg(not(mister_experiments))]
pub fn run_effect_bench(_f: &mut Fpga) {
    crate::ui_errln!("effect-bench is unavailable in launcher-only UI builds");
    std::process::exit(2);
}

#[cfg(mister_experiments)]
fn run_one_effect_bench(
    disp: &mut MappedRgb565Framebuffer,
    overlay_ctx: &mut Option<(
        Rc<MisterSoftwareWindow>,
        slint_ui::effect_hud::EffectHud,
        AnimationClock,
        Vec<Rgb565Pixel>,
    )>,
    kind: EffectKind,
    mode: EffectBenchMode,
    fill: EffectFill,
    size: EffectSize,
    target: EffectTarget,
    secs: u64,
    low: &mut [u32],
) {
    let mut state = EffectState::new(kind, size);
    disp.clear_black();
    let start = Instant::now();
    let mut frame = 0u64;
    let mut totals = EffectBenchTotals::default();
    let mut live_start = Instant::now();
    let mut live_frames = 0u64;
    let mut pacer = VsyncPacer::from_env();
    let mut raw_pixels = vec![Rgb565Pixel(0); target.render_w * target.render_h];

    crate::ui_logln!(
        "effect bench running {} mode={} fill={} internal={}x{} target={}x{}+{},{} scale={} secs={}...",
        kind.name(),
        mode.label(),
        fill.label(),
        size.w,
        size.h,
        target.physical_w,
        target.physical_h,
        target.physical_x,
        target.physical_y,
        target.scale,
        secs
    );
    while secs == 0 || start.elapsed().as_secs() < secs {
        let wall_start = Instant::now();
        let effect_us;
        let vsync_us;
        let t0 = Instant::now();
        state.render(frame, low);
        effect_us = t0.elapsed().as_micros() as u64;
        let v0 = Instant::now();
        let _pace = pacer.wait();
        vsync_us = v0.elapsed().as_micros() as u64;
        let mut slint_us = 0;
        let scale_copy_us;
        match mode {
            EffectBenchMode::Raw => {
                let c0 = Instant::now();
                scale_effect_to_rgb565_fit(
                    low,
                    size.w,
                    size.h,
                    target.render_w,
                    target.render_h,
                    &mut raw_pixels,
                );
                present_effect_target(disp, target, &raw_pixels);
                scale_copy_us = c0.elapsed().as_micros() as u64;
            }
            EffectBenchMode::Overlay => {
                let Some((window, app, animation_clock, full)) = overlay_ctx.as_mut() else {
                    crate::ui_errln!("effect-bench internal error: overlay context missing");
                    std::process::exit(1);
                };
                let c0 = Instant::now();
                scale_effect_to_rgb565_fit(
                    low,
                    size.w,
                    size.h,
                    target.render_w,
                    target.render_h,
                    full,
                );
                let mut copy_acc = c0.elapsed().as_micros() as u64;
                app.set_effect_name(kind.name().into());
                app.set_mode_label("overlay".into());
                app.set_fps_label(format!("fps {live_frames}").into());
                app.set_timing_label(format!("fx {effect_us}us").into());
                app.set_frame_phase((frame % 16) as i32);
                update_slint_animations(animation_clock);
                window.request_redraw();
                let s0 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let _ = renderer.render(full, target.render_w);
                });
                slint_us = s0.elapsed().as_micros() as u64;
                let c1 = Instant::now();
                present_effect_target(disp, target, full);
                copy_acc += c1.elapsed().as_micros() as u64;
                scale_copy_us = copy_acc;
            }
        }
        let wall_us = wall_start.elapsed().as_micros() as u64;
        totals.record(effect_us, slint_us, scale_copy_us, vsync_us, wall_us);
        frame += 1;
        live_frames += 1;
        if live_start.elapsed().as_millis() >= 1000 {
            let nn = live_frames.max(1) as u128;
            crate::ui_logln!(
                "  fps ~ {live_frames}  | effect {}us  slint {}us  scale-copy {}us  vsync-wait {}us  vsync hits={} timeouts={} fallback={} errors={} hz={:.2}",
                totals.effect_us / totals.frames.max(1) as u128,
                totals.slint_us / totals.frames.max(1) as u128,
                totals.scale_copy_us / totals.frames.max(1) as u128,
                totals.vsync_us / totals.frames.max(1) as u128,
                pacer.hits(),
                pacer.timeouts(),
                pacer.fallback_frames(),
                pacer.errors(),
                1_000_000.0 / pacer.period_us() as f64
            );
            let _ = nn;
            live_frames = 0;
            live_start = Instant::now();
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    let fps = if elapsed > 0.0 {
        totals.frames as f64 / elapsed
    } else {
        0.0
    };
    crate::ui_logln!(
        "effect_bench_result\t{}\t{}\t{}\t{}\t{}x{}\t{}\t{}\t{:.1}\t{}\t{}\t{}\t{}\t{}",
        std::env::var("MISTER_EFFECT_BENCH_LABEL").unwrap_or_else(|_| "manual".into()),
        kind.name(),
        mode.label(),
        fill.label(),
        size.w,
        size.h,
        target.scale,
        totals.frames,
        fps,
        EffectBenchTotals::avg(totals.effect_us, totals.frames),
        EffectBenchTotals::avg(totals.slint_us, totals.frames),
        EffectBenchTotals::avg(totals.scale_copy_us, totals.frames),
        EffectBenchTotals::avg(totals.vsync_us, totals.frames),
        EffectBenchTotals::avg(totals.wall_us, totals.frames)
    );
    crate::ui_logln!(
        "effect_bench_summary effect={} mode={} fill={} slow_frames={} elapsed={elapsed:.1}s vsync_hits={} vsync_timeouts={} fallback_frames={} vsync_errors={} max_miss_streak={} inferred_hz={:.2}",
        kind.name(),
        mode.label(),
        fill.label(),
        totals.slow_frames,
        pacer.hits(),
        pacer.timeouts(),
        pacer.fallback_frames(),
        pacer.errors(),
        pacer.max_miss_streak(),
        1_000_000.0 / pacer.period_us() as f64
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui() -> UiDisplay {
        UiDisplay::for_framebuffer(1920, 1080)
    }

    #[test]
    fn effect_target_sizes_cover_supported_fills() {
        let size = EffectSize { w: 480, h: 270 };

        let full = EffectTarget::new(EffectFill::Full, size, &ui()).expect("full target");
        assert_eq!(
            (
                full.physical_x,
                full.physical_y,
                full.physical_w,
                full.physical_h
            ),
            (0, 0, 1920, 1080)
        );
        assert_eq!((full.render_w, full.render_h, full.scale), (1920, 1080, 4));

        let half = EffectTarget::new(EffectFill::Half, size, &ui()).expect("half target");
        assert_eq!(
            (
                half.physical_x,
                half.physical_y,
                half.physical_w,
                half.physical_h
            ),
            (480, 270, 960, 540)
        );
        assert_eq!((half.render_w, half.render_h, half.scale), (960, 540, 2));

        let double = EffectTarget::new(EffectFill::Double, size, &ui()).expect("2x target");
        assert_eq!(
            (
                double.physical_x,
                double.physical_y,
                double.physical_w,
                double.physical_h
            ),
            (480, 270, 960, 540)
        );
        assert_eq!(
            (double.render_w, double.render_h, double.scale),
            (960, 540, 2)
        );

        let native = EffectTarget::new(EffectFill::Native, size, &ui()).expect("native target");
        assert_eq!(
            (
                native.physical_x,
                native.physical_y,
                native.physical_w,
                native.physical_h
            ),
            (720, 405, 480, 270)
        );
        assert_eq!(
            (native.render_w, native.render_h, native.scale),
            (480, 270, 1)
        );
    }

    #[test]
    fn scale_effect_to_rgb565_fit_converts_and_scales_pixels() {
        let src = [0x00ff0000, 0x0000ff00, 0x000000ff, 0x00ffffff];
        let mut dst = [Rgb565Pixel(0); 8];

        scale_effect_to_rgb565_fit(&src, 2, 2, 4, 2, &mut dst);

        assert_eq!(
            dst,
            [
                rgb565_from_rgb888_word(0x00ff0000),
                rgb565_from_rgb888_word(0x00ff0000),
                rgb565_from_rgb888_word(0x0000ff00),
                rgb565_from_rgb888_word(0x0000ff00),
                rgb565_from_rgb888_word(0x000000ff),
                rgb565_from_rgb888_word(0x000000ff),
                rgb565_from_rgb888_word(0x00ffffff),
                rgb565_from_rgb888_word(0x00ffffff),
            ]
        );
    }
}
