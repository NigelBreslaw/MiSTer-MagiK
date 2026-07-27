// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[cfg(target_os = "macos")]
mod macos {
    use mister_magik_fb::visual_platform::{MisterPlatform, MisterSoftwareWindow};
    use mister_magik_ui::launcher::{Launcher, MisterBridge, MisterUi};
    use slint::platform::software_renderer::{RepaintBufferType, Rgb565Pixel};
    use slint::{ComponentHandle, PhysicalSize};
    use softbuffer::{Context, Surface};
    use std::error::Error;
    use std::num::NonZeroU32;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::window::{Window, WindowId};

    const FRAME_WIDTH: usize = 960;
    const FRAME_HEIGHT: usize = 540;
    const FRAME_PERIOD: Duration = Duration::from_nanos(16_666_667);

    pub fn run() -> Result<(), Box<dyn Error>> {
        let slint_window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(MisterPlatform::new(
            Rc::clone(&slint_window),
            None,
        )))?;
        slint_window.set_size(PhysicalSize::new(FRAME_WIDTH as u32, FRAME_HEIGHT as u32));

        let launcher = Launcher::new()?;
        let ui = launcher.global::<MisterUi>();
        ui.set_window_width(FRAME_WIDTH as i32);
        ui.set_window_height(FRAME_HEIGHT as i32);
        ui.set_crt_layout(false);
        let bridge = launcher.global::<MisterBridge>();
        bridge.set_startup_visible(false);
        bridge.set_effective_view("home".into());
        bridge.set_screen_mode(0);
        bridge.set_menu_title("MiSTer MagiK".into());
        bridge.set_clock_text("12:34".into());
        launcher.show()?;
        slint_window.request_redraw();

        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + FRAME_PERIOD));
        let mut application = PreviewApplication::new(launcher, slint_window);
        event_loop.run_app(&mut application)?;
        Ok(())
    }

    struct PreviewApplication {
        _launcher: Launcher,
        slint_window: Rc<MisterSoftwareWindow>,
        native_window: Option<Arc<Window>>,
        surface: Option<Surface<Arc<Window>, Arc<Window>>>,
        rgb565: Vec<Rgb565Pixel>,
        xrgb8888: Vec<u32>,
    }

    impl PreviewApplication {
        fn new(launcher: Launcher, slint_window: Rc<MisterSoftwareWindow>) -> Self {
            Self {
                _launcher: launcher,
                slint_window,
                native_window: None,
                surface: None,
                rgb565: vec![Rgb565Pixel(0); FRAME_WIDTH * FRAME_HEIGHT],
                xrgb8888: Vec::new(),
            }
        }

        fn create_window(&mut self, event_loop: &ActiveEventLoop) {
            let attributes = Window::default_attributes()
                .with_title("MiSTer MagiK UI Preview — Home")
                .with_inner_size(LogicalSize::new(FRAME_WIDTH as f64, FRAME_HEIGHT as f64))
                .with_min_inner_size(LogicalSize::new(
                    (FRAME_WIDTH / 2) as f64,
                    (FRAME_HEIGHT / 2) as f64,
                ));
            let window = Arc::new(
                event_loop
                    .create_window(attributes)
                    .expect("create preview window"),
            );
            let context = Context::new(Arc::clone(&window)).expect("create softbuffer context");
            let surface =
                Surface::new(&context, Arc::clone(&window)).expect("create preview window surface");
            self.native_window = Some(window);
            self.surface = Some(surface);
        }

        fn render(&mut self) {
            slint::platform::update_timers_and_animations();
            self.slint_window.request_redraw();
            self.slint_window.draw_if_needed(|renderer| {
                renderer.render(&mut self.rgb565, FRAME_WIDTH);
            });

            let Some(window) = self.native_window.as_ref() else {
                return;
            };
            let Some(surface) = self.surface.as_mut() else {
                return;
            };
            let size = window.inner_size();
            let Some(width) = NonZeroU32::new(size.width) else {
                return;
            };
            let Some(height) = NonZeroU32::new(size.height) else {
                return;
            };
            surface
                .resize(width, height)
                .expect("resize preview surface");

            let output_len = size.width as usize * size.height as usize;
            self.xrgb8888.resize(output_len, 0);
            scale_rgb565_nearest(
                &self.rgb565,
                FRAME_WIDTH,
                FRAME_HEIGHT,
                &mut self.xrgb8888,
                size.width as usize,
                size.height as usize,
            );
            let mut buffer = surface.buffer_mut().expect("map preview surface");
            buffer.copy_from_slice(&self.xrgb8888);
            buffer.present().expect("present preview surface");
        }
    }

    impl ApplicationHandler for PreviewApplication {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.native_window.is_none() {
                self.create_window(event_loop);
            }
            if let Some(window) = self.native_window.as_ref() {
                window.request_redraw();
            }
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            window_id: WindowId,
            event: WindowEvent,
        ) {
            if self.native_window.as_ref().map(|window| window.id()) != Some(window_id) {
                return;
            }
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::RedrawRequested => self.render(),
                WindowEvent::Resized(_) => {
                    if let Some(window) = self.native_window.as_ref() {
                        window.request_redraw();
                    }
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + FRAME_PERIOD));
            if let Some(window) = self.native_window.as_ref() {
                window.request_redraw();
            }
        }
    }

    fn scale_rgb565_nearest(
        source: &[Rgb565Pixel],
        source_width: usize,
        source_height: usize,
        destination: &mut [u32],
        destination_width: usize,
        destination_height: usize,
    ) {
        if source_width == 0
            || source_height == 0
            || destination_width == 0
            || destination_height == 0
        {
            return;
        }
        let scale = (destination_width / source_width)
            .min(destination_height / source_height)
            .max(1);
        let content_width = (source_width * scale).min(destination_width);
        let content_height = (source_height * scale).min(destination_height);
        let offset_x = (destination_width - content_width) / 2;
        let offset_y = (destination_height - content_height) / 2;
        destination.fill(0);
        for destination_y in 0..content_height {
            let source_y = destination_y * source_height / content_height;
            for destination_x in 0..content_width {
                let source_x = destination_x * source_width / content_width;
                destination
                    [(offset_y + destination_y) * destination_width + offset_x + destination_x] =
                    rgb565_to_xrgb8888(source[source_y * source_width + source_x]);
            }
        }
    }

    fn rgb565_to_xrgb8888(pixel: Rgb565Pixel) -> u32 {
        let value = pixel.0;
        let red = u32::from((value >> 11) & 0x1f);
        let green = u32::from((value >> 5) & 0x3f);
        let blue = u32::from(value & 0x1f);
        let red = (red << 3) | (red >> 2);
        let green = (green << 2) | (green >> 4);
        let blue = (blue << 3) | (blue >> 2);
        (red << 16) | (green << 8) | blue
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn rgb565_primary_channels_expand_to_xrgb8888() {
            assert_eq!(rgb565_to_xrgb8888(Rgb565Pixel(0xf800)), 0x00ff0000);
            assert_eq!(rgb565_to_xrgb8888(Rgb565Pixel(0x07e0)), 0x0000ff00);
            assert_eq!(rgb565_to_xrgb8888(Rgb565Pixel(0x001f)), 0x000000ff);
        }
    }
}

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    macos::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("mister-magik-ui-preview is available on macOS only");
}
