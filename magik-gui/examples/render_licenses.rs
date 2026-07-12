use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, ModelRc, PhysicalSize, SharedString, VecModel};

struct SnapshotPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for SnapshotPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "licenses.ppm".into());
    let state = std::env::args().nth(2).unwrap_or_else(|| "list".into());
    let expanded = state == "expanded";
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(SnapshotPlatform(window.clone())))
        .expect("set snapshot platform");
    window.set_size(PhysicalSize::new(960, 540));

    let app = mister_magik_ui::launcher::Launcher::new().expect("create launcher");
    let bridge = app.global::<mister_magik_ui::launcher::MisterBridge>();
    bridge.set_startup_visible(false);
    bridge.set_screen_mode(match state.as_str() {
        "settings" => 3,
        "about" => 4,
        "info" => 6,
        _ => 5,
    });
    bridge.set_build_label("Build 42 | 2026-07-12 12:00 UTC".into());
    bridge.set_present_mode_label("RGB565 /dev/fb0".into());
    bridge.set_info_database_build("1,284 ms (scan 1,107 ms, save 177 ms)".into());
    bridge.set_info_kernel_version("Kernel version detected at launcher startup".into());
    bridge.set_slint_logo_image(
        slint::Image::load_from_path(std::path::Path::new(
            "magik-gui/ui/art/slint-logo-pixel.png",
        ))
        .expect("load Slint logo snapshot asset"),
    );
    bridge.set_licenses_selected(1);
    bridge.set_licenses_expanded(expanded);
    bridge.set_license_lines(ModelRc::new(VecModel::from(
        [
            "FFmpeg 8.1.2",
            "",
            "MiSTer MagiK statically links selected FFmpeg libraries.",
            "",
            "GNU LESSER GENERAL PUBLIC LICENSE",
            "Version 2.1, February 1999",
            "",
            "This is representative legal text used to verify the virtualized line layout.",
        ]
        .map(SharedString::from)
        .to_vec(),
    )));
    app.show().expect("show launcher");
    window.request_redraw();

    let mut pixels = vec![Rgb565Pixel::default(); 960 * 540];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 960);
    });
    let mut ppm = format!("P6\n960 540\n255\n").into_bytes();
    for pixel in pixels {
        let value = pixel.0;
        let red = ((value >> 11) & 0x1f) as u8;
        let green = ((value >> 5) & 0x3f) as u8;
        let blue = (value & 0x1f) as u8;
        ppm.extend_from_slice(&[
            (red << 3) | (red >> 2),
            (green << 2) | (green >> 4),
            (blue << 3) | (blue >> 2),
        ]);
    }
    std::fs::write(output, ppm).expect("write PPM");
}
