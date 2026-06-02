//! Native MiSTer frontend — spike stage.
//!
//! Subcommands (run on-device with the stock menu SIGSTOPped so we own the bus):
//!   read   read & print the live video mode + fb params (UIO_GET_VRES/FB_PAR)
//!   fb     paint a geometry test pattern to /dev/fb0 and route buffer 0 to HDMI
//!          via the ported video_fb_enable (direct_video positioning)
//!
//! See AGENTS.md §9.5 (spike) and §12 (toolchain).

mod fb;
mod fpga;

use fb::{Display, Pixel};
use fpga::{Fpga, Mode, MODE_1080P60, UIO_GET_FB_PAR, UIO_GET_VRES};
use std::rc::Rc;
use std::time::Instant;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::PhysicalSize;

slint::include_modules!();

const W: usize = 1920;
const H: usize = 1080;

/// Minimal Slint platform: one software-rendered window, time from a monotonic
/// clock. We drive the event loop ourselves (no run_event_loop), pacing on vsync.
struct MisterPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for MisterPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> core::time::Duration {
        self.start.elapsed()
    }
}

fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "fb".to_string());
    println!("mister-slint-fb [{cmd}] (arch={})", std::env::consts::ARCH);

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
        "ui" => run_ui(&mut f),
        other => {
            eprintln!("unknown command '{other}' (use: read | fb | ui)");
            std::process::exit(2);
        }
    }
}

fn run_ui(f: &mut Fpga) {
    // Optional run duration (seconds); default 20 so the SIGSTOPped menu resumes.
    let secs: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    let mut disp = match Display::open(W, H) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open display (/dev/fb0): {e}");
            std::process::exit(1);
        }
    };
    // Route buffer 0 to HDMI once (xoff=yoff=0 for this direct_video menu, §9.6).
    let flag = f.fb_enable_direct(0, W as u16, H as u16, MODE_1080P60, Some(0), Some(0));
    println!("fb routed (support_flag={flag}); Slint software renderer (vsync, dirty-row copy)");

    // ReusedBuffer: Slint renders into ONE cached buffer that stays fully current,
    // redrawing only this frame's dirty region (fast, ~2.3ms — cached RAM). After
    // vsync we copy just those rows into the write-combined framebuffer.
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(MisterPlatform {
        window: window.clone(),
        start: Instant::now(),
    }))
    .expect("set_platform");
    window.set_size(PhysicalSize::new(W as u32, H as u32));

    let ui = AppWindow::new().expect("AppWindow::new");
    ui.show().expect("show");

    let mut cached = vec![Pixel(0); W * H];
    let start = Instant::now();
    let mut frames = 0u64;
    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut render_us = 0u128;
    let mut vsync_us = 0u128;
    let mut copy_us = 0u128;
    let mut copy_rows_acc = 0u128;

    println!("running UI for {secs}s (vsync-locked, dirty-row copy)...");
    while start.elapsed().as_secs() < secs {
        slint::platform::update_timers_and_animations();
        let t0 = Instant::now();
        let mut this_rows: Option<(usize, usize)> = None;
        window.draw_if_needed(|renderer| {
            let region = renderer.render(&mut cached, W);
            let o = region.bounding_box_origin();
            let s = region.bounding_box_size();
            if s.width > 0 && s.height > 0 {
                let y0 = o.y.max(0) as usize;
                let y1 = ((o.y + s.height as i32) as usize).min(H);
                if y1 > y0 {
                    this_rows = Some((y0, y1));
                }
            }
        });
        let t1 = Instant::now();
        disp.wait_vsync();
        let t2 = Instant::now();
        let mut copied_rows = 0usize;
        if let Some((y0, y1)) = this_rows {
            disp.copy_rows(&cached, y0, y1);
            copied_rows = y1 - y0;
        }
        let t3 = Instant::now();
        render_us += (t1 - t0).as_micros();
        vsync_us += (t2 - t1).as_micros();
        copy_us += (t3 - t2).as_micros();
        copy_rows_acc += copied_rows as u128;
        frames += 1;
        fps_frames += 1;
        if fps_window_start.elapsed().as_millis() >= 1000 {
            let nn = fps_frames.max(1) as u128;
            println!(
                "  fps ~ {fps_frames}  | render {}us  vsync-wait {}us  copy {}us ({} rows avg)",
                render_us / nn,
                vsync_us / nn,
                copy_us / nn,
                copy_rows_acc / nn
            );
            fps_frames = 0;
            render_us = 0;
            vsync_us = 0;
            copy_us = 0;
            copy_rows_acc = 0;
            fps_window_start = Instant::now();
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
}

fn fb_test(f: &mut Fpga) {
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
