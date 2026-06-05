//! Native MiSTer frontend — spike stage.
//!
//! Subcommands:
//!   read      read & print the live video mode + fb params (UIO_GET_VRES/FB_PAR)
//!   ui [scene] [secs]  Slint UI (default `launcher`, infinite when secs=0)
//!   scenes    list Slint scene names
//!   fb        paint a geometry test pattern to /dev/fb0 and route buffer 0 to HDMI
//!   input     gamepad log / sniff / calibrate
//!   catalog-bench  benchmark arcade catalog pipeline phases
//!   preview-bench  benchmark arcade preview image read/decode
//!
//! When installed as `main=` in MiSTer.ini, boots straight into the launcher.
//! Core handoff argv (`.rbf` paths) re-execs stock `/media/fat/MiSTer`.
//!
//! See AGENTS.md §9.5 (spike) and §12 (toolchain).

use std::ffi::CString;

mod arcade_catalog;
mod controller_db;
mod cpu_profile;
mod fb;
mod fpga;
mod frame_profile;
mod input;
mod input_repeat;
mod launcher;
mod preview_bench;
mod preview_worker;
mod setup_nav;
mod ui_display;
mod ui_runner;
mod vt;

use fb::{Display, Pixel};
use fpga::{Fpga, Mode, MODE_1080P60, UIO_GET_FB_PAR, UIO_GET_VRES};

const W: usize = 1920;
const H: usize = 1080;

const MISTER_BIN: &str = "/media/fat/MiSTer";

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() >= 2 {
        if should_handoff_to_mister(&args[1]) {
            exec_mister(&args);
        }
    }

    let cmd = resolve_command(&args);

    println!("mister-magic-fb [{cmd}] (arch={})", std::env::consts::ARCH);

    let mut f = match Fpga::open() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("failed to open FPGA (/dev/mem): {e}");
            std::process::exit(1);
        }
    };

    match cmd.as_str() {
        "read" => read_mode(&mut f),
        "fb" => fb_test(&mut f),
        "ui" => ui_runner::run_ui(&mut f),
        "scenes" => ui_runner::print_scenes(),
        "input" => run_input(),
        "catalog-bench" => run_catalog_bench(),
        "preview-bench" => preview_bench::run(),
        other => {
            eprintln!(
                "unknown command '{other}' \
                 (use: read | fb | ui | scenes | input | catalog-bench | preview-bench)"
            );
            std::process::exit(2);
        }
    }
}

/// Boot via `main=` in MiSTer.ini, or reset back to menu — run the Slint launcher.
fn resolve_command(args: &[String]) -> String {
    match args.get(1).map(|s| s.as_str()) {
        None => "ui".into(),
        Some("") => "ui".into(),
        Some(arg1) if is_launcher_boot(arg1) => "ui".into(),
        Some(arg1) => arg1.to_string(),
    }
}

fn is_launcher_boot(arg: &str) -> bool {
    arg.ends_with("menu.rbf") || arg.ends_with("/menu.rbf")
}

/// MiSTer re-exec'd us with a core path — hand off to stock Main so gameplay works.
fn should_handoff_to_mister(arg: &str) -> bool {
    if matches!(
        arg,
        "read" | "fb" | "ui" | "scenes" | "input" | "catalog-bench" | "preview-bench"
    ) {
        return false;
    }
    if arg.ends_with("menu.rbf") {
        return false;
    }
    arg.ends_with(".rbf") || arg.ends_with(".mra") || arg.ends_with(".mgl")
}

fn exec_mister(args: &[String]) {
    println!("core handoff → {MISTER_BIN} {}", args[1..].join(" "));
    let c_path = CString::new(MISTER_BIN).expect("CString");
    let c_args: Vec<CString> = std::iter::once(c_path.clone())
        .chain(args[1..].iter().map(|s| CString::new(s.as_str()).expect("CString")))
        .collect();
    let ptrs: Vec<*const libc::c_char> = c_args.iter().map(|s| s.as_ptr()).chain([std::ptr::null()]).collect();
    let err = unsafe { libc::execv(c_path.as_ptr(), ptrs.as_ptr()) };
    eprintln!("execv({MISTER_BIN}) failed: {err}");
    std::process::exit(1);
}

fn fb_test(f: &mut Fpga) {
    let _vt = vt::VtGraphicsGuard::enter_or_warn();
    let mut disp = match Display::open(W, H) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open display (/dev/fb0 + /dev/mem): {e}");
            std::process::exit(1);
        }
    };
    const N: u32 = 0; // paint + route buffer 0 (the write-combined /dev/fb0 buffer)

    // Geometry-revealing pattern: four solid quadrants, a 6px white border that
    // must touch all four screen edges, and a white cross-hair through centre.
    // Any horizontal-total mismatch shears this into diagonal columns; any
    // offset pulls the border off an edge.
    const RED: u32 = 0x00FF_0000;
    const GREEN: u32 = 0x0000_FF00;
    const BLUE: u32 = 0x0000_00FF;
    const YELLOW: u32 = 0x00FF_FF00;
    const WHITE: u32 = 0x00FF_FFFF;

    let buf = disp.buffer_mut();
    fill_rect(buf, 0, 0, W / 2, H / 2, RED);
    fill_rect(buf, W / 2, 0, W, H / 2, GREEN);
    fill_rect(buf, 0, H / 2, W / 2, H, BLUE);
    fill_rect(buf, W / 2, H / 2, W, H, YELLOW);

    let b = 6;
    fill_rect(buf, 0, 0, W, b, WHITE); // top
    fill_rect(buf, 0, H - b, W, H, WHITE); // bottom
    fill_rect(buf, 0, 0, b, H, WHITE); // left
    fill_rect(buf, W - b, 0, W, H, WHITE); // right
    fill_rect(buf, 0, H / 2 - b / 2, W, H / 2 + b / 2, WHITE); // horizontal centre
    fill_rect(buf, W / 2 - b / 2, 0, W / 2 + b / 2, H, WHITE); // vertical centre

    println!("painted {W}x{H} test pattern to buffer {N}");

    // Empirically, the stock menu's HPS buffer 0 (direct_video=1) sits at the
    // visible-area origin: xoff=yoff=0. That's the direct_video formula
    // (xoff=item[4]-FB_DV_LBRD) evaluated with the *running* menu mode, whose
    // porches are already the tiny border constants (item[4]=3, item[8]=2) -> 0.
    // Hardcoding the original mode-8 porches (148/36) put us ~145px too far
    // right. The real frontend must derive xoff/yoff from the LIVE mode (read via
    // UIO_GET_VRES + the mode table) so it adapts to other resolutions / CRT.
    // Override on the CLI for tuning: `fb <xoff> <yoff>`.
    let xo = Some(std::env::args().nth(2).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0));
    let yo = Some(std::env::args().nth(3).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0));

    let mode: Mode = MODE_1080P60;
    let flag = f.fb_enable_direct(N, W as u16, H as u16, mode, xo, yo);
    let xoff = xo.unwrap_or(mode.hbp as i32 - fpga::FB_DV_LBRD);
    let yoff = yo.unwrap_or(mode.vbp as i32 - fpga::FB_DV_UBRD);
    println!(
        "fb_enable_direct(n={N}): support_flag={flag}; scaled L={} R={} T={} B={} stride={}",
        xoff,
        xoff + mode.hact as i32 - 1,
        yoff,
        yoff + mode.vact as i32 - 1,
        W * 4
    );

    // Hold so the (SIGSTOPped) menu can't repaint while you look at HDMI.
    let secs = 12;
    println!("holding {secs}s — check HDMI for a clean 4-quadrant pattern...");
    std::thread::sleep(std::time::Duration::from_secs(secs));
    println!("done (MiSTer will resume after this process exits)");
}

fn fill_rect(buf: &mut [Pixel], x0: usize, y0: usize, x1: usize, y1: usize, c: u32) {
    for y in y0..y1 {
        for x in x0..x1 {
            buf[y * W + x] = Pixel(c);
        }
    }
}

fn read_mode(f: &mut Fpga) {
    println!("\n=== UIO_GET_VRES (0x23) ===");
    let cmd = f.cmd_capture(UIO_GET_VRES);
    print_word("  cmd", cmd);
    let mut vres = [(0u16, 0u16); 16];
    for w in vres.iter_mut() {
        *w = f.spi_capture(0);
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
    let cmd = f.cmd_capture(UIO_GET_FB_PAR);
    print_word("  cmd(crc)", cmd);
    let mut fbp = [(0u16, 0u16); 6];
    for w in fbp.iter_mut() {
        *w = f.spi_capture(0);
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
    println!("{label} hi=0x{:04x} ({:5})   lo=0x{:04x} ({:5})", w.0, w.0, w.1, w.1);
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

fn run_catalog_bench() {
    let args: Vec<String> = std::env::args().collect();
    let mut sample = 10usize;
    if let Some(i) = args.iter().position(|a| a == "--sample-images") {
        if let Some(n) = args.get(i + 1).and_then(|s| s.parse().ok()) {
            sample = n;
        }
    }

    let root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    println!("catalog-bench root={root} sample_images={sample}");
    let (catalog, timings) = arcade_catalog::build_with_options(
        &root,
        arcade_catalog::BuildOptions {
            sample_image_decodes: sample,
        },
        None,
    );
    timings.print_summary();
    println!("games={}", catalog.len());
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
