//! Native MiSTer frontend — spike stage.
//!
//! Subcommands:
//!   read      read & print the live video mode + fb params (UIO_GET_VRES/FB_PAR)
//!   route     route the current /dev/fb0 buffer 0 to HDMI
//!   ui [scene] [secs]  Slint UI (default `launcher`, infinite when secs=0)
//!   scenes    list Slint scene names
//!   effects   list framebuffer effect benchmark names
//!   effect-bench [effect|all] [secs] [raw|overlay|both] [WIDTHxHEIGHT]
//!   vsync-probe [frames]  print per-frame vsync/fallback pacing diagnostics
//!   cpu-profile-smoke [secs]  burn CPU and verify profiler SVG output
//!   fb [secs] [normal|direct|none]  paint + optionally route current fb size
//!   fb-current [secs] [normal|direct|none]  compatibility alias for `fb`
//!   input     gamepad log / sniff / calibrate
//!   library-scan-bench  benchmark cold scan, import, cached load, and no-op rescan
//!   preview-cache build --filter nearest|box|lanczos --format derived-png|raw-rgb --max 320x320
//!   audio-tone  play a 48 kHz stereo sine wave through /dev/MrAudio
//!
//! Core handoff argv (`.rbf` paths) re-execs `/media/fat/MiSTer_MagiK`.
//!
//! See AGENTS.md §9.5 (spike) and §12 (toolchain).

use std::ffi::CString;

mod boot_analytics;
mod cpu_profile;
mod display_config;
mod fb;
mod fpga;
#[cfg_attr(mister_ui_scope_launcher, allow(dead_code))]
mod frame_profile;
mod input;
mod launcher;
mod mr_audio;
mod runtime_status;
mod setup_nav;
mod ui_display;
mod ui_runner;
#[cfg(feature = "video")]
mod video_player;
mod vt;

pub use mister_magik_fb::{
    arcade_catalog, command_args, controller_db, input_repeat, input_state, library_bench,
    preview_worker,
};

use fb::{Display, Pixel, VsyncPacer};
use fpga::{Fpga, Mode, UIO_GET_FB_PAR, UIO_GET_VRES};

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

    println!("mister-magik-fb [{cmd}] (arch={})", std::env::consts::ARCH);

    if cmd == "vsync-probe" {
        run_vsync_probe();
        return;
    }

    if cmd == "cpu-profile-smoke" {
        run_cpu_profile_smoke();
        return;
    }

    if cmd == "preview-cache" {
        run_preview_cache();
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
        "ui" => ui_runner::run_ui(&mut f),
        "scenes" => ui_runner::print_scenes(),
        "effects" => ui_runner::print_effects(),
        "effect-bench" => ui_runner::run_effect_bench(&mut f),
        "input" => run_input(),
        "library-scan-bench" => library_bench::run_scan_bench(),
        "preview-cache" => run_preview_cache(),
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

fn run_preview_cache() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let sub = args.first().map(String::as_str).unwrap_or("help");
    if sub != "build" {
        eprintln!(
            "usage: preview-cache build [--filter nearest|box|lanczos] [--format derived-png|raw-rgb] [--max 320x320] [--root /media/fat/_Arcade] [--limit N]"
        );
        std::process::exit(2);
    }

    let mut filter = preview_worker::PreviewResizeFilter::Lanczos;
    let mut format = preview_worker::PreviewStorageFormat::RawRgb;
    let mut max_w = 320u32;
    let mut max_h = 320u32;
    let mut root = std::path::PathBuf::from(arcade_catalog::DEFAULT_ARCADE_ROOT);
    let mut limit: Option<usize> = None;

    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--filter" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("preview-cache: --filter needs a value");
                    std::process::exit(2);
                };
                filter = preview_worker::PreviewResizeFilter::from_label(value);
                if filter == preview_worker::PreviewResizeFilter::Off {
                    eprintln!("preview-cache: filter must be nearest, box, or lanczos");
                    std::process::exit(2);
                }
                i += 2;
            }
            "--format" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("preview-cache: --format needs a value");
                    std::process::exit(2);
                };
                format = preview_worker::PreviewStorageFormat::from_label(value);
                if format == preview_worker::PreviewStorageFormat::Png {
                    eprintln!("preview-cache: format must be derived-png or raw-rgb");
                    std::process::exit(2);
                }
                i += 2;
            }
            "--max" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("preview-cache: --max needs a value");
                    std::process::exit(2);
                };
                let Some((w, h)) = parse_size_arg(value) else {
                    eprintln!("preview-cache: --max must look like 320x320");
                    std::process::exit(2);
                };
                max_w = w.max(1);
                max_h = h.max(1);
                i += 2;
            }
            "--root" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("preview-cache: --root needs a value");
                    std::process::exit(2);
                };
                root = std::path::PathBuf::from(value);
                i += 2;
            }
            "--limit" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("preview-cache: --limit needs a value");
                    std::process::exit(2);
                };
                limit = value.parse::<usize>().ok();
                if limit.is_none() {
                    eprintln!("preview-cache: --limit must be an integer");
                    std::process::exit(2);
                }
                i += 2;
            }
            other => {
                eprintln!("preview-cache: unknown argument {other:?}");
                std::process::exit(2);
            }
        }
    }

    let resize = preview_worker::PreviewResizeSpec {
        filter,
        max_w,
        max_h,
    };
    let screenshot_dir = if root.file_name().and_then(|s| s.to_str()) == Some("screenshot") {
        root
    } else {
        root.join("media").join("screenshot")
    };
    let mut paths: Vec<_> = match std::fs::read_dir(&screenshot_dir) {
        Ok(read_dir) => read_dir
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().and_then(|s| s.to_str()) == Some("png")
                    && !path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|name| name.starts_with("._"))
            })
            .collect(),
        Err(e) => {
            eprintln!("preview-cache: read {}: {e}", screenshot_dir.display());
            std::process::exit(1);
        }
    };
    paths.sort();
    if let Some(limit) = limit {
        paths.truncate(limit);
    }

    println!(
        "preview_cache_build format={} filter={} max={}x{} input_dir={} count={}",
        format.label(),
        filter.label(),
        max_w,
        max_h,
        screenshot_dir.display(),
        paths.len()
    );
    println!(
        "file\tsource_w\tsource_h\toutput_w\toutput_h\tread_us\tdecode_us\tresize_us\tencode_write_us\ttotal_us\tinput_bytes\toutput_bytes\tout"
    );

    let total_t = std::time::Instant::now();
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut output_bytes = 0usize;
    let mut read_us = 0u128;
    let mut decode_us = 0u128;
    let mut resize_us = 0u128;
    let mut write_us = 0u128;
    for path in paths {
        let path_str = path.to_string_lossy().into_owned();
        match preview_worker::write_preview_cache(&path_str, format, resize) {
            Ok(result) => {
                ok += 1;
                output_bytes += result.output_bytes;
                read_us += result.read_us as u128;
                decode_us += result.decode_us as u128;
                resize_us += result.resize_us as u128;
                write_us += result.encode_write_us as u128;
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    std::path::Path::new(&result.input_path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&result.input_path),
                    result.source_width,
                    result.source_height,
                    result.output_width,
                    result.output_height,
                    result.read_us,
                    result.decode_us,
                    result.resize_us,
                    result.encode_write_us,
                    result.total_us,
                    result.input_bytes,
                    result.output_bytes,
                    result.output_path.display()
                );
            }
            Err(e) => {
                failed += 1;
                eprintln!("preview-cache: {path_str}: {e}");
            }
        }
    }
    let n = ok.max(1) as u128;
    println!(
        "preview_cache_summary ok={} failed={} elapsed_ms={} output_bytes={} avg_read_us={} avg_decode_us={} avg_resize_us={} avg_encode_write_us={}",
        ok,
        failed,
        total_t.elapsed().as_millis(),
        output_bytes,
        read_us / n,
        decode_us / n,
        resize_us / n,
        write_us / n
    );
}

fn parse_size_arg(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x').or_else(|| s.split_once('X'))?;
    Some((w.parse().ok()?, h.parse().ok()?))
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
