//! Shared vsync render loop and Slint bench scene dispatch.

use crate::fb::{Display, Pixel};
use crate::fpga::{Fpga, MODE_1080P60};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};
use std::rc::Rc;
use std::time::Instant;

mod slint_ui {
    #![allow(clippy::all, unused_imports)]
    include!(concat!(env!("OUT_DIR"), "/app.rs"));
    include!(concat!(env!("OUT_DIR"), "/full_motion.rs"));
    include!(concat!(env!("OUT_DIR"), "/static_ui.rs"));
    include!(concat!(env!("OUT_DIR"), "/local_motion.rs"));
    include!(concat!(env!("OUT_DIR"), "/text_heavy.rs"));
    include!(concat!(env!("OUT_DIR"), "/solid_fill.rs"));
    include!(concat!(env!("OUT_DIR"), "/list_scroll.rs"));
}

pub const UI_SCENES: &[&str] = &[
    "demo",
    "full_motion",
    "static_ui",
    "local_motion",
    "text_heavy",
    "solid_fill",
    "list_scroll",
];

const W: usize = 1920;
const H: usize = 1080;

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

/// `ui [scene] [secs]` — scene defaults to `demo`; secs defaults to 20.
pub fn parse_ui_args() -> (String, u64) {
    let a2 = std::env::args().nth(2);
    let a3 = std::env::args().nth(3);
    match (a2.as_deref(), a3.as_deref()) {
        (Some(s), Some(t)) if t.parse::<u64>().is_ok() => (normalize_scene(s), t.parse().unwrap()),
        (Some(s), None) if s.parse::<u64>().is_ok() => ("demo".into(), s.parse().unwrap()),
        (Some(s), Some(t)) => (
            normalize_scene(s),
            t.parse::<u64>().unwrap_or(20),
        ),
        (Some(s), None) => (normalize_scene(s), 20),
        _ => ("demo".into(), 20),
    }
}

fn normalize_scene(s: &str) -> String {
    if UI_SCENES.contains(&s) {
        s.to_string()
    } else {
        eprintln!("unknown scene '{s}' (use: {})", UI_SCENES.join(" | "));
        std::process::exit(2);
    }
}

pub fn print_scenes() {
    println!("Slint UI scenes (1920x1080):");
    for s in UI_SCENES {
        println!("  {s}");
    }
}

pub fn run_ui(f: &mut Fpga) {
    let (scene, secs) = parse_ui_args();
    println!("ui scene={scene} secs={secs}");

    let mut disp = match Display::open(W, H) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open display (/dev/fb0): {e}");
            std::process::exit(1);
        }
    };
    let flag = f.fb_enable_direct(0, W as u16, H as u16, MODE_1080P60, Some(0), Some(0));
    println!("fb routed (support_flag={flag}); Slint software renderer (vsync, dirty-row copy)");

    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(MisterPlatform {
        window: window.clone(),
        start: Instant::now(),
    }))
    .expect("set_platform");
    window.set_size(PhysicalSize::new(W as u32, H as u32));

    match scene.as_str() {
        "demo" => {
            slint_ui::AppWindow::new().expect("AppWindow::new").show().expect("show");
            run_frame_loop(secs, &mut disp, &window);
        }
        "full_motion" => {
            slint_ui::FullMotion::new()
                .expect("FullMotion::new")
                .show()
                .expect("show");
            run_frame_loop(secs, &mut disp, &window);
        }
        "static_ui" => {
            slint_ui::StaticUi::new()
                .expect("StaticUi::new")
                .show()
                .expect("show");
            run_frame_loop(secs, &mut disp, &window);
        }
        "local_motion" => {
            slint_ui::LocalMotion::new()
                .expect("LocalMotion::new")
                .show()
                .expect("show");
            run_frame_loop(secs, &mut disp, &window);
        }
        "text_heavy" => {
            slint_ui::TextHeavy::new()
                .expect("TextHeavy::new")
                .show()
                .expect("show");
            run_frame_loop(secs, &mut disp, &window);
        }
        "solid_fill" => {
            slint_ui::SolidFill::new()
                .expect("SolidFill::new")
                .show()
                .expect("show");
            run_frame_loop(secs, &mut disp, &window);
        }
        "list_scroll" => {
            slint_ui::ListScroll::new()
                .expect("ListScroll::new")
                .show()
                .expect("show");
            run_frame_loop(secs, &mut disp, &window);
        }
        _ => unreachable!(),
    }
}

fn run_frame_loop(secs: u64, disp: &mut Display, window: &Rc<MinimalSoftwareWindow>) {
    let mut cached = vec![Pixel(0); W * H];
    let start = Instant::now();
    let mut frames = 0u64;
    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut render_us = 0u128;
    let mut vsync_us = 0u128;
    let mut copy_us = 0u128;
    let mut copy_rows_acc = 0u128;

    println!("bench scene running {secs}s (vsync-locked, dirty-row copy)...");
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
        if let Some((y0, y1)) = this_rows {
            disp.copy_rows(&cached, y0, y1);
            copy_rows_acc += (y1 - y0) as u128;
        }
        let t3 = Instant::now();
        render_us += (t1 - t0).as_micros();
        vsync_us += (t2 - t1).as_micros();
        copy_us += (t3 - t2).as_micros();
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
