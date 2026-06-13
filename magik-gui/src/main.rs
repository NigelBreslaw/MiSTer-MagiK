//! Native MiSTer MagiK framebuffer frontend.
//!
//! Subcommands:
//!   Production:
//!     ui [scene] [secs]  Slint UI (default `launcher`, infinite when secs=0)
//!     input              gamepad log / sniff / calibrate
//!     audio-tone         play a 48 kHz stereo sine wave through /dev/MrAudio
//!   Diagnostics:
//!     read               print live video mode + fb params
//!     route              route the current /dev/fb0 buffer 0 to HDMI
//!     fb                 paint + optionally route current fb size
//!     fb-current         compatibility alias for `fb`
//!     vsync-probe        print per-frame vsync/fallback pacing diagnostics
//!     cpu-profile-smoke  burn CPU and verify profiler SVG output
//!     library-refresh    build/update the SQLite library cache
//!     library-sql        inspect the SQLite library cache without sqlite3(1)
//!   Benchmarks:
//!     scenes             list Slint scene names
//!     effects            list framebuffer effect benchmark names
//!     camera-effects     list classic camera/background effect labels
//!     sprite-effects     list classic sprite/object effect labels
//!     text-effects       list classic game/Amiga text effect labels
//!     raster-effects     list classic raster/palette effect labels
//!     transition-effects list classic screen transition effect labels
//!     preview-transitions list screenshot transition labels
//!     effect-bench       run framebuffer effect benchmarks
//!     library-scan-bench benchmark cold scan, import, cached load, no-op rescan
//!
//! Core handoff argv (`.rbf` paths) re-execs `/media/fat/MiSTer_MagiK`.
//!
//! See AGENTS.md for display routing, boot handoff, and toolchain details.

use std::ffi::CString;

mod arcade_list_renderer;
mod bitmap_text;
mod boot_analytics;
mod cpu_profile;
mod display_config;
mod fb;
mod fpga;
#[cfg(mister_bench_scenes)]
mod frame_profile;
mod input;
mod launcher;
mod mr_audio;
mod preview_state;
mod runtime_status;
mod screenshot_transitions;
mod setup_nav;
mod ui_blend_velocity;
mod ui_display;
mod ui_effect_bench;
mod ui_runner;
#[cfg(feature = "video")]
mod video_player;
mod vt;

pub use mister_magik_fb::fb_format;
pub use mister_magik_fb::{
    arcade_catalog, command_args, controller_db, input_repeat, input_state, library_db,
    preview_worker,
};

use fb::{Display, Pixel, VsyncPacer};
use fpga::{Fpga, Mode, UIO_GET_FB_PAR, UIO_GET_VRES};
use mister_magik_fb::fb_format::FramebufferFormat;
use slint::platform::software_renderer::{Rgb565Pixel, TargetPixel};
use ui_display::{UI_FB_H, UI_FB_W};
use ui_runner::ui_boot::ui_fpga_scaled_mode;

const MISTER_BIN: &str = "/media/fat/MiSTer_MagiK";
fn main() {
    let args: Vec<String> = std::env::args().collect();
    boot_analytics::event("process_start", format!("args={}", args.join(" ")));

    if args.len() >= 2 {
        if command_args::should_handoff_to_mister(&args[1]) {
            exec_mister(&args);
        }
    }

    let cmd = command_args::resolve_command(&args);

    if cmd != "library-sql" {
        println!("mister-magik-fb [{cmd}] (arch={})", std::env::consts::ARCH);
    }

    if cmd == "vsync-probe" {
        run_vsync_probe();
        return;
    }

    if cmd == "cpu-profile-smoke" {
        run_cpu_profile_smoke();
        return;
    }

    if cmd == "library-refresh" {
        run_library_refresh();
        return;
    }

    if cmd == "library-sql" {
        run_library_sql();
        return;
    }

    if cmd == "preview-transitions" {
        print_preview_transitions();
        return;
    }

    if cmd == "camera-effects" {
        ui_runner::print_camera_effects();
        return;
    }

    if cmd == "sprite-effects" {
        ui_runner::print_sprite_effects();
        return;
    }

    if cmd == "text-effects" {
        ui_runner::print_text_effects();
        return;
    }

    if cmd == "raster-effects" {
        ui_runner::print_raster_effects();
        return;
    }

    if cmd == "transition-effects" {
        ui_runner::print_transition_effects();
        return;
    }

    let mut f = match Fpga::open() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("failed to open FPGA (/dev/mem): {e}");
            std::process::exit(1);
        }
    };

    match cmd.as_str() {
        "read" => read_mode(&mut f),
        "route" => route_framebuffer(&mut f),
        "fb" => fb_current_probe(&mut f),
        "fb-current" => fb_current_probe(&mut f),
        "fb-format-smoke" => fb_format_smoke(&mut f),
        "early-black" => early_black_route(&mut f),
        "ui" => ui_runner::run_ui(&mut f),
        "scenes" => ui_runner::print_scenes(),
        "effects" => ui_runner::print_effects(),
        "effect-bench" => ui_effect_bench::run_effect_bench(&mut f),
        "input" => run_input(),
        "library-scan-bench" => library_db::run_scan_bench(),
        "audio-tone" => run_audio_tone(&mut f),
        other => {
            eprintln!(
                "unknown command '{other}' (use: {})",
                command_args::COMMANDS.join(" | ")
            );
            std::process::exit(2);
        }
    }
}

fn print_preview_transitions() {
    println!(
        "{}",
        screenshot_transitions::PreviewTransitionEffect::labels()
    );
}

fn run_library_refresh() {
    let mut progress = |title: &str, detail: &str| {
        println!("library_refresh\tprogress\t{title}\t{detail}");
    };
    match library_db::refresh_default_sqlite_database(Some(&mut progress)) {
        Ok(summary) => {
            println!(
                "library_refresh\tdone\tskipped={} bytes={} scan_us={} import_us={} discoveries={} normal_files={} containers={} entries={}",
                summary.skipped,
                summary.bytes,
                summary.scan_us,
                summary.import_us,
                summary.discoveries,
                summary.normal_files,
                summary.containers,
                summary.entries
            );
        }
        Err(e) => {
            eprintln!("library_refresh\tfailed\t{e}");
            std::process::exit(1);
        }
    }
}

fn run_library_sql() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    match library_db::run_sqlite_inspect_cli(&args) {
        Ok(output) => {
            print!("{output}");
            if !output.ends_with('\n') {
                println!();
            }
        }
        Err(e) => {
            eprintln!("library_sql\tfailed\t{e}");
            std::process::exit(1);
        }
    }
}

fn run_cpu_profile_smoke() {
    let secs = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(3);
    if std::env::var("MISTER_PPROF").ok().as_deref() != Some("1") {
        eprintln!("cpu-profile-smoke requires MISTER_PPROF=1");
        std::process::exit(2);
    }
    println!("cpu_profile_smoke: burning CPU for {secs}s");
    let cpu = cpu_profile::start();
    if cpu.is_none() {
        eprintln!("cpu_profile_smoke: profiler did not start");
        std::process::exit(1);
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let mut state = 0x1234_5678_9abc_def0_u64;
    let mut rounds = 0_u64;
    while std::time::Instant::now() < until {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1)
            .rotate_left((state & 31) as u32);
        std::hint::black_box(state);
        rounds = rounds.wrapping_add(1);
    }
    println!("cpu_profile_smoke: rounds={rounds} state={state:#018x}");
    match cpu_profile::finish(cpu) {
        Ok(Some(summary)) if summary.sample_hits > 0 && summary.bytes > 0 => {
            println!(
                "cpu_profile_smoke: ok samples={} stacks={} duration={:.1}s hz={} bytes={} out={}",
                summary.sample_hits,
                summary.sample_stacks,
                summary.duration_secs,
                summary.hz,
                summary.bytes,
                summary.out_path
            );
        }
        Ok(Some(summary)) => {
            eprintln!(
                "cpu_profile_smoke: profiler produced unusable output samples={} bytes={}",
                summary.sample_hits, summary.bytes
            );
            std::process::exit(1);
        }
        Ok(None) => {
            eprintln!("cpu_profile_smoke: profiling feature is not enabled");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn run_vsync_probe() {
    let frames = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(300);
    let mut pacer = VsyncPacer::from_env();
    println!("frame\tsource\twait_us\tperiod_us\tinferred_hz\tmiss_streak\tmessage");
    for frame in 0..frames {
        let pace = pacer.wait();
        println!(
            "{frame}\t{}\t{}\t{}\t{:.2}\t{}\t{}",
            pace.source.label(),
            pace.wait_us,
            pace.period_us,
            1_000_000.0 / pace.period_us as f64,
            pace.miss_streak,
            pace.message.as_deref().unwrap_or("")
        );
    }
    println!(
        "vsync_probe_summary frames={frames} hits={} timeouts={} fallback_frames={} errors={} max_miss_streak={} inferred_hz={:.2}",
        pacer.hits(),
        pacer.timeouts(),
        pacer.fallback_frames(),
        pacer.errors(),
        pacer.max_miss_streak(),
        1_000_000.0 / pacer.period_us() as f64
    );
    if pacer.errors() > 0 {
        std::process::exit(1);
    }
}

fn run_audio_tone(f: &mut Fpga) {
    if let Err(e) = f.set_audio_volume(0) {
        eprintln!("warning: failed to set FPGA audio volume: {e}");
    }
    let args: Vec<String> = std::env::args().skip(2).collect();
    if let Err(e) = mr_audio::run_tone_from_args(&args) {
        eprintln!("audio-tone failed: {e}");
        std::process::exit(1);
    }
}

fn exec_mister(args: &[String]) {
    println!("core handoff → {MISTER_BIN} {}", args[1..].join(" "));
    let c_path = CString::new(MISTER_BIN).expect("CString");
    let c_args: Vec<CString> = std::iter::once(c_path.clone())
        .chain(
            args[1..]
                .iter()
                .map(|s| CString::new(s.as_str()).expect("CString")),
        )
        .collect();
    let ptrs: Vec<*const libc::c_char> = c_args
        .iter()
        .map(|s| s.as_ptr())
        .chain([std::ptr::null()])
        .collect();
    let err = unsafe { libc::execv(c_path.as_ptr(), ptrs.as_ptr()) };
    eprintln!("execv({MISTER_BIN}) failed: {err}");
    std::process::exit(1);
}

fn route_framebuffer(f: &mut Fpga) {
    let disp = match Display::open_current() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open current display (/dev/fb0): {e}");
            std::process::exit(1);
        }
    };
    let w = disp.width();
    let h = disp.height();
    let flag = match f.fb_enable(
        0,
        w as u16,
        h as u16,
        Mode::framebuffer_sized(w as u16, h as u16),
        Some(0),
        Some(0),
        std::env::var_os("MISTER_DIRECT_VIDEO").is_some(),
    ) {
        Ok(flag) => flag,
        Err(e) => {
            eprintln!("failed to route current fb to HDMI: {e}");
            std::process::exit(1);
        }
    };
    println!("route: fb0 {w}x{h} -> HDMI support_flag={flag}");
}

fn early_black_route(f: &mut Fpga) {
    let format = FramebufferFormat::from_env();
    if let Err(e) =
        Display::write_mister_mode_format(format, UI_FB_W, UI_FB_H, format.stride_bytes(UI_FB_W))
    {
        eprintln!("early-black: failed to set framebuffer mode: {e}");
        std::process::exit(1);
    }

    let mut disp = match Display::open_with_format(UI_FB_W, UI_FB_H, format) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("early-black: failed to open /dev/fb0: {e}");
            std::process::exit(1);
        }
    };

    disp.clear_black();
    boot_analytics::event(
        "early_black_route_frame_copied",
        format!(
            "format={} w={} h={}",
            format.label(),
            disp.width(),
            disp.height()
        ),
    );

    let route_mode = ui_fpga_scaled_mode();
    let flag = match f.fb_enable_format(
        0,
        disp.width() as u16,
        disp.height() as u16,
        route_mode,
        Some(0),
        Some(0),
        std::env::var_os("MISTER_DIRECT_VIDEO").is_some(),
        format,
    ) {
        Ok(flag) => flag,
        Err(e) => {
            eprintln!("early-black: failed to route framebuffer: {e}");
            std::process::exit(1);
        }
    };
    boot_analytics::event(
        "early_black_route_completed",
        format!(
            "format={} w={} h={} scan={}x{} support_flag={flag}",
            format.label(),
            disp.width(),
            disp.height(),
            route_mode.hact,
            route_mode.vact
        ),
    );
    println!(
        "early-black: routed {} {}x{} -> {}x{} support_flag={flag}",
        format.label(),
        disp.width(),
        disp.height(),
        route_mode.hact,
        route_mode.vact
    );
}

fn fb_current_probe(f: &mut Fpga) {
    let _vt = vt::VtGraphicsGuard::enter_or_warn();
    let secs = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60);
    let route = std::env::args().nth(3).unwrap_or_else(|| "normal".into());
    let mut disp = match Display::open_current() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open current display (/dev/fb0): {e}");
            std::process::exit(1);
        }
    };
    let w = disp.width();
    let h = disp.height();
    paint_pattern(disp.buffer_mut(), w, h);
    println!("painted current {w}x{h} test pattern");

    match route.as_str() {
        "normal" => {
            let flag = match f.fb_enable(
                0,
                w as u16,
                h as u16,
                Mode::framebuffer_sized(w as u16, h as u16),
                Some(0),
                Some(0),
                false,
            ) {
                Ok(flag) => flag,
                Err(e) => {
                    eprintln!("failed to route current fb via SET_FBUF: {e}");
                    std::process::exit(1);
                }
            };
            println!("routed current fb via SET_FBUF only support_flag={flag}");
        }
        "direct" => {
            let flag = match f.fb_enable_direct(
                0,
                w as u16,
                h as u16,
                Mode::framebuffer_sized(w as u16, h as u16),
                Some(0),
                Some(0),
            ) {
                Ok(flag) => flag,
                Err(e) => {
                    eprintln!("failed to route current fb via SET_FBUF + set_vga_fb: {e}");
                    std::process::exit(1);
                }
            };
            println!("routed current fb via SET_FBUF + set_vga_fb support_flag={flag}");
        }
        "none" => {
            println!("route skipped; expecting another owner to scan /dev/fb0");
        }
        other => {
            eprintln!("unknown fb-current route '{other}' (use normal|direct|none)");
            std::process::exit(2);
        }
    }

    let params = match f.read_fb_params() {
        Ok(params) => params,
        Err(e) => {
            eprintln!("failed to read framebuffer params after route: {e}");
            std::process::exit(1);
        }
    };
    println!("after route: {}", params.log_line());
    if secs == 0 {
        println!("holding forever — stop this process or reboot when done");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
    println!("holding {secs}s — check HDMI for bordered colour test pattern...");
    std::thread::sleep(std::time::Duration::from_secs(secs));
}

fn fb_format_smoke(f: &mut Fpga) {
    let _vt = vt::VtGraphicsGuard::enter_or_warn();
    let format_arg = std::env::args().nth(2).unwrap_or_else(|| "8888".into());
    let format = match FramebufferFormat::from_label(&format_arg) {
        Some(format) => format,
        None => {
            eprintln!("fb-format-smoke format must be 8888 or 565");
            std::process::exit(2);
        }
    };
    let secs = std::env::args()
        .nth(3)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(10);
    let route = std::env::args().nth(4).unwrap_or_else(|| "normal".into());
    let previous = match Display::current_info() {
        Ok(info) => info,
        Err(e) => {
            eprintln!("failed to read current framebuffer mode: {e}");
            std::process::exit(1);
        }
    };
    const W: usize = 960;
    const H: usize = 540;
    const HDMI_W: u16 = 1920;
    const HDMI_H: u16 = 1080;
    let scan_mode = Mode::framebuffer_sized(HDMI_W, HDMI_H);
    if let Err(e) = Display::write_mister_mode_format(format, W, H, format.stride_bytes(W)) {
        eprintln!("failed to set framebuffer mode for smoke: {e}");
        std::process::exit(1);
    }
    let restore = || {
        if let Err(e) = Display::restore_mister_mode(previous) {
            eprintln!("warning: failed to restore framebuffer mode: {e}");
        }
    };
    let mut disp = match Display::open_with_format(W, H, format) {
        Ok(d) => d,
        Err(e) => {
            restore();
            eprintln!("failed to open smoke framebuffer: {e}");
            std::process::exit(1);
        }
    };
    match format {
        FramebufferFormat::Xrgb8888 => paint_pattern(disp.buffer_mut(), W, H),
        FramebufferFormat::Rgb565 => paint_pattern_565(disp.buffer_565_mut(), W, H),
    }
    let route_res = match route.as_str() {
        "normal" => f.fb_enable_format(
            0,
            W as u16,
            H as u16,
            scan_mode,
            Some(0),
            Some(0),
            false,
            format,
        ),
        "direct" => f.fb_enable_format(
            0,
            W as u16,
            H as u16,
            scan_mode,
            Some(0),
            Some(0),
            true,
            format,
        ),
        "none" => Ok(0),
        other => {
            restore();
            eprintln!("unknown fb-format-smoke route '{other}' (use normal|direct|none)");
            std::process::exit(2);
        }
    };
    match route_res {
        Ok(flag) => println!(
            "fb-format-smoke: format={} rb={} source={}x{} scan={}x{} stride={} route={} support_flag={flag}",
            format.label(),
            if format.rb_from_env() { 1 } else { 0 },
            W,
            H,
            HDMI_W,
            HDMI_H,
            format.stride_bytes(W),
            route
        ),
        Err(e) => {
            restore();
            eprintln!("failed to route smoke framebuffer: {e}");
            std::process::exit(1);
        }
    }
    if secs == 0 {
        println!("holding forever - stop this process or reboot when done");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
    println!("holding {secs}s - check HDMI for RGB/color-ramp smoke pattern...");
    std::thread::sleep(std::time::Duration::from_secs(secs));
    restore();
}

fn paint_pattern(buf: &mut [Pixel], w: usize, h: usize) {
    const RED: u32 = 0x00FF_0000;
    const GREEN: u32 = 0x0000_FF00;
    const BLUE: u32 = 0x0000_00FF;
    const YELLOW: u32 = 0x00FF_FF00;
    const WHITE: u32 = 0x00FF_FFFF;
    const BLACK: u32 = 0x0000_0000;
    fill_rect_strided(buf, w, 0, 0, w, h, BLACK);
    fill_rect_strided(buf, w, 0, 0, w / 2, h / 2, RED);
    fill_rect_strided(buf, w, w / 2, 0, w, h / 2, GREEN);
    fill_rect_strided(buf, w, 0, h / 2, w / 2, h, BLUE);
    fill_rect_strided(buf, w, w / 2, h / 2, w, h, YELLOW);

    let b = (w.min(h) / 90).clamp(2, 8);
    fill_rect_strided(buf, w, 0, 0, w, b, WHITE);
    fill_rect_strided(buf, w, 0, h.saturating_sub(b), w, h, WHITE);
    fill_rect_strided(buf, w, 0, 0, b, h, WHITE);
    fill_rect_strided(buf, w, w.saturating_sub(b), 0, w, h, WHITE);
    fill_rect_strided(buf, w, 0, h / 2 - b / 2, w, h / 2 + b / 2, WHITE);
    fill_rect_strided(buf, w, w / 2 - b / 2, 0, w / 2 + b / 2, h, WHITE);
}

fn paint_pattern_565(buf: &mut [Rgb565Pixel], w: usize, h: usize) {
    let mut tmp = vec![Pixel(0); w * h];
    paint_pattern(&mut tmp, w, h);
    for (dst, src) in buf.iter_mut().zip(tmp) {
        let p = src.0 & 0x00ff_ffff;
        *dst = <Rgb565Pixel as TargetPixel>::from_rgb((p >> 16) as u8, (p >> 8) as u8, p as u8);
    }
}

fn fill_rect_strided(
    buf: &mut [Pixel],
    stride: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    c: u32,
) {
    for y in y0..y1 {
        for x in x0..x1 {
            buf[y * stride + x] = Pixel(c);
        }
    }
}

fn read_mode(f: &mut Fpga) {
    println!("\n=== UIO_GET_VRES (0x23) ===");
    let cmd = match f.cmd_capture(UIO_GET_VRES) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("failed to issue UIO_GET_VRES: {e}");
            std::process::exit(1);
        }
    };
    print_word("  cmd", cmd);
    let mut vres = [(0u16, 0u16); 16];
    for w in vres.iter_mut() {
        *w = match f.spi_capture(0) {
            Ok(w) => w,
            Err(e) => {
                f.disable_io();
                eprintln!("failed to read UIO_GET_VRES word: {e}");
                std::process::exit(1);
            }
        };
    }
    f.disable_io();
    for (i, w) in vres.iter().enumerate() {
        print_word(&format!("  w{i:<2}"), *w);
    }
    let lo = |i: usize| vres[i].1 as u32;
    println!(
        "  -> width={} height={}",
        lo(1) | (lo(2) << 16),
        lo(3) | (lo(4) << 16)
    );

    println!("\n=== UIO_GET_FB_PAR (0x40) ===");
    let cmd = match f.cmd_capture(UIO_GET_FB_PAR) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("failed to issue UIO_GET_FB_PAR: {e}");
            std::process::exit(1);
        }
    };
    print_word("  cmd(crc)", cmd);
    let mut fbp = [(0u16, 0u16); 6];
    for w in fbp.iter_mut() {
        *w = match f.spi_capture(0) {
            Ok(w) => w,
            Err(e) => {
                f.disable_io();
                eprintln!("failed to read UIO_GET_FB_PAR word: {e}");
                std::process::exit(1);
            }
        };
    }
    f.disable_io();
    for (i, w) in fbp.iter().enumerate() {
        print_word(&format!("  w{i:<2}"), *w);
    }
    println!(
        "  -> arx={} ary={} fb_fmt=0x{:04x} fb_w={} fb_h={} fb_en={}",
        fbp[0].1,
        fbp[1].1,
        fbp[2].1,
        fbp[3].1,
        fbp[4].1,
        fbp[2].1 & 0x40 != 0
    );
}

fn print_word(label: &str, w: (u16, u16)) {
    println!(
        "{label} hi=0x{:04x} ({:5})   lo=0x{:04x} ({:5})",
        w.0, w.0, w.1, w.1
    );
}

fn run_input() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let sub = args.first().map(|s| s.as_str()).unwrap_or("log");
    match sub {
        "calibrate" => {
            let path = args.get(1).map(|s| s.as_str());
            if let Err(e) = input::calibrate(path) {
                eprintln!("input calibrate failed: {e}");
                std::process::exit(1);
            }
        }
        "log" => {
            let (path, secs) = parse_input_log_args(&args[1..]);
            if let Err(e) = input::log_js_events(path, secs) {
                eprintln!("input log failed: {e}");
                std::process::exit(1);
            }
        }
        "sniff" => {
            let (path, secs) = parse_input_log_args(&args[1..]);
            if let Err(e) = input::sniff(path, secs) {
                eprintln!("input sniff failed: {e}");
                std::process::exit(1);
            }
        }
        other => {
            eprintln!(
                "unknown input subcommand '{other}' \
                 (use: input log [path] [secs] | input sniff [path] [secs] | input calibrate [path])"
            );
            std::process::exit(2);
        }
    }
}

fn parse_input_log_args(args: &[String]) -> (Option<&str>, u64) {
    match args.len() {
        0 => (None, 120),
        1 => {
            if let Ok(secs) = args[0].parse::<u64>() {
                (None, secs)
            } else {
                (Some(args[0].as_str()), 30)
            }
        }
        _ => {
            if args[1].parse::<u64>().is_ok() {
                (Some(args[0].as_str()), args[1].parse().unwrap())
            } else {
                (None, args[0].parse().unwrap_or(30))
            }
        }
    }
}
