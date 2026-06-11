//! Native framebuffer effect benchmark scene.
#![cfg_attr(not(mister_bench_scenes), allow(unused_imports, dead_code))]

use crate::display_config::DisplayConfig;
use crate::fb::{Display, Pixel, VsyncPacer};
use crate::fpga::{Fpga, Mode};
use crate::ui_display::{UiDisplay, SLINT_UI_SCALE};
use crate::ui_runner::ui_boot::FbModeGuard;
use crate::ui_runner::ui_platform::{update_slint_animations, AnimationClock, MisterPlatform};
use crate::vt::VtGraphicsGuard;
use mister_magik_fb::effects::{EffectKind, EffectSize, EffectState};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::{ComponentHandle, PhysicalSize};
use std::rc::Rc;
use std::time::Instant;

use mister_magik_ui as slint_ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
enum EffectBenchMode {
    Raw,
    Overlay,
}

#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
impl EffectBenchMode {
    fn label(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Overlay => "overlay",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
enum EffectFill {
    Full,
    Half,
    Double,
    Native,
    FpgaHalf,
}

#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
impl EffectFill {
    fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Half => "half",
            Self::Double => "2x",
            Self::Native => "native",
            Self::FpgaHalf => "fpga-half",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "full" => Some(Self::Full),
            "half" => Some(Self::Half),
            "2x" | "double" => Some(Self::Double),
            "native" => Some(Self::Native),
            "fpga-half" => Some(Self::FpgaHalf),
            _ => None,
        }
    }

    fn uses_fpga_scaler(self) -> bool {
        matches!(self, Self::FpgaHalf)
    }
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
struct EffectTarget {
    physical_x: usize,
    physical_y: usize,
    physical_w: usize,
    physical_h: usize,
    render_w: usize,
    render_h: usize,
    scale: usize,
}

#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
impl EffectTarget {
    fn new(fill: EffectFill, size: EffectSize, ui: &UiDisplay) -> Option<Self> {
        let (physical_w, physical_h, scale) = match fill {
            EffectFill::Full => (1920, 1080, size.scale_to_1080p()?),
            EffectFill::Half => match size.scale_to_half_1080p() {
                Some(scale) => (960, 540, scale),
                None if size.w <= 960 && size.h <= 540 => (size.w, size.h, 1),
                None => return None,
            },
            EffectFill::Double => (size.w.checked_mul(2)?, size.h.checked_mul(2)?, 2),
            EffectFill::Native => (size.w, size.h, 1),
            EffectFill::FpgaHalf => {
                if size.w != 480 || size.h != 270 {
                    return None;
                }
                (960, 540, 2)
            }
        };
        if !fill.uses_fpga_scaler() && (physical_w > ui.fb_w() || physical_h > ui.fb_h()) {
            return None;
        }
        Some(Self {
            physical_x: if fill.uses_fpga_scaler() {
                480
            } else {
                (ui.fb_w() - physical_w) / 2
            },
            physical_y: if fill.uses_fpga_scaler() {
                270
            } else {
                (ui.fb_h() - physical_h) / 2
            },
            physical_w,
            physical_h,
            render_w: if fill.uses_fpga_scaler() {
                size.w
            } else {
                physical_w / ui.fb_scale()
            },
            render_h: if fill.uses_fpga_scaler() {
                size.h
            } else {
                physical_h / ui.fb_scale()
            },
            scale,
        })
    }
}

#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
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
                eprintln!("unknown effect '{effect_arg}' (use `effects` to list names)");
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
            eprintln!("unknown effect-bench mode '{other}' (use raw|overlay|both)");
            std::process::exit(2);
        }
    };
    let size = match args.get(3).map(String::as_str) {
        Some(s) => match EffectSize::parse(s) {
            Some(size) => size,
            None => {
                eprintln!("unsupported effect size '{s}' (use `effects` to list supported sizes)");
                std::process::exit(2);
            }
        },
        None => EffectSize { w: 480, h: 270 },
    };
    let fill = match args.get(4).map(String::as_str) {
        Some(s) => EffectFill::parse(s).unwrap_or_else(|| {
            eprintln!("unknown effect fill '{s}' (use full|half|native|fpga-half)");
            std::process::exit(2);
        }),
        None => EffectFill::Full,
    };
    if fill == EffectFill::FpgaHalf && modes.iter().any(|m| *m != EffectBenchMode::Raw) {
        eprintln!("effect fill fpga-half supports raw mode only");
        std::process::exit(2);
    }
    (effects, secs, modes, size, fill)
}

#[derive(Default)]
#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
struct EffectBenchTotals {
    frames: u64,
    effect_us: u128,
    slint_us: u128,
    scale_copy_us: u128,
    vsync_us: u128,
    wall_us: u128,
    slow_frames: u64,
}

#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
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

#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
fn scale_effect_to_pixels_fit(
    src: &[u32],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    dst: &mut [Pixel],
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
            *p = Pixel(src_row[sx]);
        }
    }
}

#[cfg(mister_bench_scenes)]
pub fn run_effect_bench(f: &mut Fpga) {
    let (effects, secs, modes, size, fill) = parse_effect_bench_args();
    println!(
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
    if fill == EffectFill::FpgaHalf && (size.w != 480 || size.h != 270) {
        eprintln!(
            "effect fill fpga-half supports only 480x270, got {}x{}",
            size.w, size.h
        );
        std::process::exit(2);
    }
    let _fb_mode_guard = if fill == EffectFill::FpgaHalf {
        println!("effect-bench-fb-mode=temporary 480x270 stride=1920 restore=on-drop");
        match FbModeGuard::set_temporary(size.w, size.h) {
            Ok(guard) => Some(guard),
            Err(e) => {
                eprintln!("failed to set temporary framebuffer mode for fpga-half: {e}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };
    let mut disp = if fill == EffectFill::FpgaHalf {
        match Display::open(size.w, size.h) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("failed to open temporary display (/dev/fb0): {e}");
                std::process::exit(1);
            }
        }
    } else {
        match Display::open_current_boot() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("failed to open display (/dev/fb0): {e}");
                std::process::exit(1);
            }
        }
    };
    let ui = UiDisplay::for_framebuffer(disp.width(), disp.height());
    let target = EffectTarget::new(fill, size, &ui).unwrap_or_else(|| {
        eprintln!(
            "effect size {}x{} cannot fill={} on framebuffer {}x{}",
            size.w,
            size.h,
            fill.label(),
            disp.width(),
            disp.height()
        );
        std::process::exit(2);
    });
    println!("{}", ui.log_line());
    let display_config = match DisplayConfig::detect(f, disp.info(), &ui) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("failed to read display configuration from FPGA: {e}");
            std::process::exit(1);
        }
    };
    println!("{}", display_config.log_line());
    let route_mode = if fill == EffectFill::FpgaHalf {
        Mode {
            hact: target.physical_w as u16,
            hbp: 3,
            vact: target.physical_h as u16,
            vbp: 2,
        }
    } else {
        Mode::framebuffer_sized(disp.width() as u16, disp.height() as u16)
    };
    let (xoff, yoff) = if fill == EffectFill::FpgaHalf {
        (
            Some(target.physical_x as i32),
            Some(target.physical_y as i32),
        )
    } else {
        (Some(0), Some(0))
    };
    let flag = match f.fb_enable(
        0,
        disp.width() as u16,
        disp.height() as u16,
        route_mode,
        xoff,
        yoff,
        std::env::var_os("MISTER_DIRECT_VIDEO").is_some(),
    ) {
        Ok(flag) => flag,
        Err(e) => {
            eprintln!("failed to route framebuffer for effect benchmark: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = f.set_audio_volume(0) {
        eprintln!("warning: failed to set FPGA audio volume: {e}");
    }
    println!(
        "fb routed (support_flag={flag}); native retro effect benchmark fpga_scale={}",
        fill.uses_fpga_scaler()
    );

    let needs_overlay = modes.contains(&EffectBenchMode::Overlay);
    let mut overlay_ctx = if needs_overlay {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let animation_clock = AnimationClock::from_env();
        slint::platform::set_platform(Box::new(MisterPlatform {
            window: window.clone(),
            start: Instant::now(),
            fixed_time: animation_clock.platform_time(),
        }))
        .expect("set_platform");
        let app = slint_ui::effect_hud::EffectHud::new().expect("EffectHud");
        let mister_ui = app.global::<slint_ui::effect_hud::MisterUi>();
        mister_ui.set_scale(SLINT_UI_SCALE);
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
            vec![Pixel(0); target.render_w * target.render_h],
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
                fill == EffectFill::FpgaHalf,
            );
        }
    }
}

#[cfg(not(mister_bench_scenes))]
pub fn run_effect_bench(_f: &mut Fpga) {
    eprintln!("effect-bench is unavailable in launcher-only UI builds");
    std::process::exit(2);
}

#[cfg(mister_bench_scenes)]
fn run_one_effect_bench(
    disp: &mut Display,
    overlay_ctx: &mut Option<(
        Rc<MinimalSoftwareWindow>,
        slint_ui::effect_hud::EffectHud,
        AnimationClock,
        Vec<Pixel>,
    )>,
    kind: EffectKind,
    mode: EffectBenchMode,
    fill: EffectFill,
    size: EffectSize,
    target: EffectTarget,
    secs: u64,
    low: &mut [u32],
    direct_to_fb: bool,
) {
    let mut state = EffectState::new(kind, size);
    disp.clear(Pixel(0));
    let start = Instant::now();
    let mut frame = 0u64;
    let mut totals = EffectBenchTotals::default();
    let mut live_start = Instant::now();
    let mut live_frames = 0u64;
    let mut pacer = VsyncPacer::from_env();

    println!(
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
        if direct_to_fb {
            let v0 = Instant::now();
            let _pace = pacer.wait();
            vsync_us = v0.elapsed().as_micros() as u64;
            let t0 = Instant::now();
            state.render(frame, disp.buffer_u32_mut());
            effect_us = t0.elapsed().as_micros() as u64;
        } else {
            let t0 = Instant::now();
            state.render(frame, low);
            effect_us = t0.elapsed().as_micros() as u64;
            let v0 = Instant::now();
            let _pace = pacer.wait();
            vsync_us = v0.elapsed().as_micros() as u64;
        }
        let mut slint_us = 0;
        let scale_copy_us;
        match mode {
            EffectBenchMode::Raw => {
                if direct_to_fb {
                    scale_copy_us = 0;
                } else {
                    let c0 = Instant::now();
                    disp.copy_u32_rect_scaled_at(
                        target.physical_x,
                        target.physical_y,
                        target.scale,
                        low,
                        size.w,
                        size.h,
                    );
                    scale_copy_us = c0.elapsed().as_micros() as u64;
                }
            }
            EffectBenchMode::Overlay => {
                let Some((window, app, animation_clock, full)) = overlay_ctx.as_mut() else {
                    eprintln!("effect-bench internal error: overlay context missing");
                    std::process::exit(1);
                };
                let c0 = Instant::now();
                scale_effect_to_pixels_fit(
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
                disp.copy_rect_scaled_at(
                    target.physical_x,
                    target.physical_y,
                    UiDisplay::for_framebuffer(disp.width(), disp.height()).fb_scale(),
                    full,
                    target.render_w,
                    target.render_h,
                );
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
            println!(
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
    println!(
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
    println!(
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
