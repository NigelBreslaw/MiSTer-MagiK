//! Shared vsync render loop and Slint bench scene dispatch.

use crate::fb::{Display, Pixel};
use crate::fpga::{Fpga, MODE_1080P60};
use crate::vt::VtGraphicsGuard;
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
    pub mod controller {
        include!(concat!(env!("OUT_DIR"), "/controller_test.rs"));
    }
    pub mod launcher {
        include!(concat!(env!("OUT_DIR"), "/launcher.rs"));
    }
}

use crate::input::{PadInfo, PadReader};
use crate::launcher::{self, LauncherNav, Screen};

pub const UI_SCENES: &[&str] = &[
    "launcher",
    "demo",
    "controller_test",
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

/// `ui [scene] [secs]` — scene defaults to `launcher`; secs defaults to 0 (infinite).
pub fn parse_ui_args() -> (String, u64) {
    let a2 = std::env::args().nth(2);
    let a3 = std::env::args().nth(3);
    match (a2.as_deref(), a3.as_deref()) {
        (Some(s), Some(t)) if t.parse::<u64>().is_ok() => (normalize_scene(s), t.parse().unwrap()),
        (Some(s), None) if s.parse::<u64>().is_ok() => ("launcher".into(), s.parse().unwrap()),
        (Some(s), Some(t)) => (
            normalize_scene(s),
            t.parse::<u64>().unwrap_or(0),
        ),
        (Some(s), None) => (normalize_scene(s), 0),
        _ => ("launcher".into(), 0),
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

    let _vt = VtGraphicsGuard::enter_or_warn();

    let mut disp = match Display::open_boot(W, H) {
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
        "controller_test" => {
            let pad = open_pad();
            let app = slint_ui::controller::ControllerTest::new().expect("ControllerTest::new");
            sync_bridge(&app, &pad);
            app.show().expect("show");
            window.request_redraw();
            run_controller_loop(secs, &mut disp, &window, pad, app);
        }
        "launcher" => {
            let pad = open_pad();
            let app = slint_ui::launcher::Launcher::new().expect("Launcher::new");
            init_launcher_bridge(&app, &pad);
            app.show().expect("show");
            window.request_redraw();
            run_launcher_loop(secs, &mut disp, f, &window, pad, app);
        }
        _ => unreachable!(),
    }
}

fn open_pad() -> PadReader {
    for attempt in 0..60 {
        match PadReader::open() {
            Ok(p) => {
                if attempt > 0 {
                    println!("gamepad open ok after {attempt} retries");
                }
                return p;
            }
            Err(e) => {
                if attempt == 0 || attempt % 10 == 0 {
                    eprintln!("gamepad open attempt {attempt}: {e}");
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
    eprintln!("failed to open gamepad after 30s");
    std::process::exit(1);
}

fn init_launcher_bridge(app: &slint_ui::launcher::Launcher, pad: &PadReader) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_screen_mode(0);
    bridge.set_selected_index(0);
    sync_bridge_pad_launcher(&bridge, pad);
}

fn sync_bridge(app: &slint_ui::controller::ControllerTest, pad: &PadReader) {
    sync_bridge_pad_controller(&app.global::<slint_ui::controller::MisterBridge>(), pad);
}

fn sync_bridge_launcher(app: &slint_ui::launcher::Launcher, pad: &PadReader, nav: &LauncherNav) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    sync_bridge_pad_launcher(&bridge, pad);
    bridge.set_screen_mode(match nav.screen {
        Screen::Home => 0,
        Screen::Controller => 1,
    });
    bridge.set_selected_index(nav.selected as i32);
}

fn sync_bridge_pad_controller(
    bridge: &slint_ui::controller::MisterBridge,
    pad: &PadReader,
) {
    let state = pad.state();
    let info = pad.info();
    bridge.set_dpad_up(state.dpad_up);
    bridge.set_dpad_down(state.dpad_down);
    bridge.set_dpad_left(state.dpad_left);
    bridge.set_dpad_right(state.dpad_right);
    bridge.set_btn_a(state.btn_a);
    bridge.set_btn_b(state.btn_b);
    bridge.set_btn_x(state.btn_x);
    bridge.set_btn_y(state.btn_y);
    bridge.set_btn_l(state.btn_l);
    bridge.set_btn_r(state.btn_r);
    bridge.set_btn_zl(state.btn_zl);
    bridge.set_btn_zr(state.btn_zr);
    bridge.set_btn_select(state.btn_select);
    bridge.set_btn_start(state.btn_start);
    bridge.set_btn_l3(state.btn_l3);
    bridge.set_btn_r3(state.btn_r3);
    bridge.set_btn_home(state.btn_home);
    bridge.set_btn_capture(state.btn_capture);
    bridge.set_capture_available(info.capture_available);
    bridge.set_left_x(state.left_x);
    bridge.set_left_y(state.left_y);
    bridge.set_right_x(state.right_x);
    bridge.set_right_y(state.right_y);
    sync_device_info_controller(bridge, info, &pad.path);
    bridge.set_pressed_now(state.pressed_now.clone().into());
    bridge.set_last_event_label(state.last_event_label.clone().into());
    bridge.set_last_raw_event(state.last_raw.clone().into());
}

fn sync_bridge_pad_launcher(bridge: &slint_ui::launcher::MisterBridge, pad: &PadReader) {
    let state = pad.state();
    let info = pad.info();
    bridge.set_dpad_up(state.dpad_up);
    bridge.set_dpad_down(state.dpad_down);
    bridge.set_dpad_left(state.dpad_left);
    bridge.set_dpad_right(state.dpad_right);
    bridge.set_btn_a(state.btn_a);
    bridge.set_btn_b(state.btn_b);
    bridge.set_btn_x(state.btn_x);
    bridge.set_btn_y(state.btn_y);
    bridge.set_btn_l(state.btn_l);
    bridge.set_btn_r(state.btn_r);
    bridge.set_btn_zl(state.btn_zl);
    bridge.set_btn_zr(state.btn_zr);
    bridge.set_btn_select(state.btn_select);
    bridge.set_btn_start(state.btn_start);
    bridge.set_btn_l3(state.btn_l3);
    bridge.set_btn_r3(state.btn_r3);
    bridge.set_btn_home(state.btn_home);
    bridge.set_btn_capture(state.btn_capture);
    bridge.set_capture_available(info.capture_available);
    bridge.set_left_x(state.left_x);
    bridge.set_left_y(state.left_y);
    bridge.set_right_x(state.right_x);
    bridge.set_right_y(state.right_y);
    sync_device_info_launcher(bridge, info, &pad.path);
    bridge.set_pressed_now(state.pressed_now.clone().into());
    bridge.set_last_event_label(state.last_event_label.clone().into());
    bridge.set_last_raw_event(state.last_raw.clone().into());
}

fn sync_device_info_controller(
    bridge: &slint_ui::controller::MisterBridge,
    info: &PadInfo,
    js_path: &str,
) {
    bridge.set_device_label(js_path.into());
    bridge.set_device_name(info.name.clone().into());
    bridge.set_usb_port(info.usb_port.clone().into());
    bridge.set_usb_id(format!("{}:{}", info.vendor_id, info.product_id).into());
    bridge.set_serial_id(if info.serial.is_empty() {
        "(no serial)".into()
    } else {
        info.serial.clone().into()
    });
    bridge.set_js_counts(format!(
        "js API: {} buttons, {} axes · evdev: {} keys, {} abs axes",
        info.js_buttons, info.js_axes, info.evdev_key_count, info.evdev_abs_count
    ).into());
}

fn sync_device_info_launcher(
    bridge: &slint_ui::launcher::MisterBridge,
    info: &PadInfo,
    js_path: &str,
) {
    bridge.set_device_label(js_path.into());
    bridge.set_device_name(info.name.clone().into());
    bridge.set_usb_port(info.usb_port.clone().into());
    bridge.set_usb_id(format!("{}:{}", info.vendor_id, info.product_id).into());
    bridge.set_serial_id(if info.serial.is_empty() {
        "(no serial)".into()
    } else {
        info.serial.clone().into()
    });
    bridge.set_js_counts(format!(
        "js API: {} buttons, {} axes · evdev: {} keys, {} abs axes",
        info.js_buttons, info.js_axes, info.evdev_key_count, info.evdev_abs_count
    ).into());
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

fn run_controller_loop(
    secs: u64,
    disp: &mut Display,
    window: &Rc<MinimalSoftwareWindow>,
    mut pad: PadReader,
    app: slint_ui::controller::ControllerTest,
) {
    let mut cached = vec![Pixel(0); W * H];
    let start = Instant::now();
    let mut frames = 0u64;
    let label = if secs == 0 { "forever".to_string() } else { format!("{secs}s") };
    println!("controller_test running {label} — press buttons on the pad...");
    while secs == 0 || start.elapsed().as_secs() < secs {
        if pad.poll() {
            sync_bridge(&app, &pad);
            window.request_redraw();
        }
        slint::platform::update_timers_and_animations();
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
        disp.wait_vsync();
        if let Some((y0, y1)) = this_rows {
            disp.copy_rows(&cached, y0, y1);
        }
        frames += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
}

fn run_launcher_loop(
    secs: u64,
    disp: &mut Display,
    f: &mut Fpga,
    window: &Rc<MinimalSoftwareWindow>,
    mut pad: PadReader,
    app: slint_ui::launcher::Launcher,
) {
    let mut cached = vec![Pixel(0); W * H];
    let start = Instant::now();
    let mut frames = 0u64;
    let mut nav = LauncherNav::new();
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!("launcher running {label} — D-pad to move, A to select, Home to go back...");
    sync_bridge_launcher(&app, &pad, &nav);
    window.request_redraw();
    while secs == 0 || start.elapsed().as_secs() < secs {
        let mut changed = pad.poll();
        if changed {
            let state = pad.state();
            if let Some(mra) = nav.handle_input(&state) {
                launcher::launch_mra(mra);
            }
            changed = true;
        }
        if changed {
            sync_bridge_launcher(&app, &pad, &nav);
            window.request_redraw();
        }
        slint::platform::update_timers_and_animations();
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
        disp.wait_vsync();
        if frames % 60 == 0 {
            f.fb_enable_direct(0, W as u16, H as u16, MODE_1080P60, Some(0), Some(0));
        }
        if let Some((y0, y1)) = this_rows {
            disp.copy_rows(&cached, y0, y1);
        }
        frames += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
}
