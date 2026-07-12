use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};

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
    bridge.set_screen_mode(if state == "settings" { 3 } else { 4 });
    bridge.set_licenses_selected(1);
    bridge.set_licenses_expanded(expanded);
    bridge.set_licenses_text(
        "Made with Slint\n\nMiSTer MagiK uses Slint 1.17.0 under the GPL-3.0-only option.\n\nGNU GENERAL PUBLIC LICENSE\nVersion 3, 29 June 2007\n\nThis is representative long legal text used to verify wrapping and clipping."
            .into(),
    );
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
