// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use slint::platform::software_renderer::{RenderingRotation, RepaintBufferType, SoftwareRenderer};
use slint::platform::{Platform, WindowAdapter};
use slint::{LogicalPosition, LogicalRect, LogicalSize, PhysicalSize, Window};
use std::cell::Cell;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

pub struct MisterSoftwareWindow {
    window: Window,
    renderer: SoftwareRenderer,
    redraw_pending: Cell<bool>,
    size: Cell<PhysicalSize>,
}

impl MisterSoftwareWindow {
    pub fn new(repaint_buffer_type: RepaintBufferType) -> Rc<Self> {
        let window = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            window: Window::new(weak.clone()),
            renderer: SoftwareRenderer::new_with_repaint_buffer_type(repaint_buffer_type),
            redraw_pending: Cell::new(false),
            size: Cell::new(PhysicalSize::default()),
        });
        crate::bitmap_font_resource::register_bitmap_fonts(&window.renderer);
        window
    }

    pub fn redraw_pending(&self) -> bool {
        self.redraw_pending.get()
    }

    pub fn draw_if_needed(&self, render_callback: impl FnOnce(&SoftwareRenderer)) -> bool {
        if self.redraw_pending.replace(false) {
            render_callback(&self.renderer);
            true
        } else {
            false
        }
    }

    pub fn draw_full_frame_if_needed(
        &self,
        render_callback: impl FnOnce(&SoftwareRenderer),
    ) -> bool {
        if !self.redraw_pending.replace(false) {
            return false;
        }
        let previous = self.renderer.repaint_buffer_type();
        self.renderer
            .set_repaint_buffer_type(RepaintBufferType::NewBuffer);
        render_callback(&self.renderer);
        self.renderer.set_repaint_buffer_type(previous);
        true
    }

    pub fn draw_full_frame_reused_if_needed(
        &self,
        logical_width: usize,
        logical_height: usize,
        render_callback: impl FnOnce(&SoftwareRenderer),
    ) -> bool {
        use i_slint_core::renderer::RendererSealed;

        if !self.redraw_pending.replace(false) {
            return false;
        }
        self.renderer.mark_dirty_region(
            LogicalRect::new(
                LogicalPosition::default(),
                LogicalSize::new(logical_width as f32, logical_height as f32),
            )
            .into(),
        );
        render_callback(&self.renderer);
        true
    }

    pub fn set_size(&self, size: impl Into<slint::WindowSize>) {
        self.window.set_size(size);
    }

    pub fn set_rendering_rotation(&self, rotation: RenderingRotation) {
        self.renderer.set_rendering_rotation(rotation);
    }

    pub fn rendering_rotation(&self) -> RenderingRotation {
        self.renderer.rendering_rotation()
    }
}

impl WindowAdapter for MisterSoftwareWindow {
    fn window(&self) -> &Window {
        &self.window
    }

    fn renderer(&self) -> &dyn slint::platform::Renderer {
        &self.renderer
    }

    fn size(&self) -> PhysicalSize {
        self.size.get()
    }

    fn set_size(&self, size: slint::WindowSize) {
        let scale_factor = self.window.scale_factor();
        self.size.set(size.to_physical(scale_factor));
        self.window
            .dispatch_event(slint::platform::WindowEvent::Resized {
                size: size.to_logical(scale_factor),
            });
    }

    fn request_redraw(&self) {
        self.redraw_pending.set(true);
    }
}

impl std::ops::Deref for MisterSoftwareWindow {
    type Target = Window;

    fn deref(&self) -> &Self::Target {
        &self.window
    }
}

pub struct MisterPlatform {
    window: Rc<MisterSoftwareWindow>,
    start: Instant,
    fixed_time: Option<Rc<Cell<Duration>>>,
}

impl MisterPlatform {
    pub fn new(window: Rc<MisterSoftwareWindow>, fixed_time: Option<Rc<Cell<Duration>>>) -> Self {
        Self {
            window,
            start: Instant::now(),
            fixed_time,
        }
    }
}

#[derive(Clone)]
pub struct AnimationClock {
    fixed_time: Option<Rc<Cell<Duration>>>,
    fixed_step: Duration,
}

const ANIMATION_CLOCK_ENV: &str = "MISTER_ANIMATION_CLOCK";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimationClockConfig {
    mode: Option<String>,
}

impl AnimationClockConfig {
    pub fn capture_with(value: Option<&str>) -> Self {
        Self {
            mode: value.map(str::to_owned),
        }
    }

    pub fn capture_environment_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        Self::capture_with(get(ANIMATION_CLOCK_ENV))
    }
}

impl AnimationClock {
    pub fn from_env() -> Self {
        Self::from_env_with_fixed_step(Duration::from_nanos(16_666_667))
    }

    pub fn from_env_with_fixed_step(fixed_step: Duration) -> Self {
        Self::from_config_with_fixed_step(
            &AnimationClockConfig::capture_with(std::env::var(ANIMATION_CLOCK_ENV).ok().as_deref()),
            fixed_step,
        )
    }

    pub fn from_config_with_fixed_step(
        config: &AnimationClockConfig,
        fixed_step: Duration,
    ) -> Self {
        match config
            .mode
            .as_deref()
            .map(|s| s.to_ascii_lowercase().replace('_', "-"))
            .as_deref()
        {
            None | Some("") | Some("fixed60") | Some("fixed-60") | Some("frame")
            | Some("frame-clock") => Self {
                fixed_time: Some(Rc::new(Cell::new(Duration::ZERO))),
                fixed_step,
            },
            Some("wall") | Some("wall-clock") => Self {
                fixed_time: None,
                fixed_step,
            },
            other => {
                crate::ui_errln!("ui: unknown MISTER_ANIMATION_CLOCK={other:?}; use wall|fixed60");
                Self {
                    fixed_time: None,
                    fixed_step,
                }
            }
        }
    }

    pub fn platform_time(&self) -> Option<Rc<Cell<Duration>>> {
        self.fixed_time.clone()
    }

    #[cfg(any(mister_bench_scenes, all(target_os = "linux", target_arch = "arm")))]
    pub fn label(&self) -> &'static str {
        if self.fixed_time.is_some() {
            "fixed60"
        } else {
            "wall"
        }
    }

    pub fn advance(&self) {
        if let Some(t) = &self.fixed_time {
            t.set(t.get() + self.fixed_step);
        }
    }
}

pub fn update_slint_animations(animation_clock: &AnimationClock) {
    animation_clock.advance();
    slint::platform::update_timers_and_animations();
}

const PRESENT_DELAY_ENV: &str = "MISTER_FB_PRESENT_DELAY_US";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOrder {
    RenderThenVsync,
    VsyncThenRender,
}

impl FrameOrder {
    pub fn from_env() -> Self {
        match std::env::var("MISTER_FRAME_ORDER")
            .ok()
            .map(|s| s.to_ascii_lowercase().replace('_', "-"))
            .as_deref()
        {
            None | Some("") | Some("render-then-vsync") | Some("render") => Self::RenderThenVsync,
            Some("vsync-then-render") | Some("vsync-first") | Some("vsync") => {
                Self::VsyncThenRender
            }
            other => {
                crate::ui_errln!(
                    "ui: unknown MISTER_FRAME_ORDER={other:?}; use render-then-vsync|vsync-first"
                );
                Self::RenderThenVsync
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::RenderThenVsync => "render-then-vsync",
            Self::VsyncThenRender => "vsync-first",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentTiming {
    delay_us: u64,
}

impl PresentTiming {
    pub fn from_env() -> Self {
        Self::from_value(std::env::var(PRESENT_DELAY_ENV).ok().as_deref())
    }

    pub fn from_value(value: Option<&str>) -> Self {
        let delay_us = value.and_then(present_delay_from_value);
        Self {
            delay_us: delay_us.unwrap_or(0),
        }
    }

    pub fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        Self::from_value(get(PRESENT_DELAY_ENV))
    }

    pub fn delay_us(self) -> u64 {
        self.delay_us
    }

    pub fn wait_until_present_time(self, vsync_done: std::time::Instant) {
        if self.delay_us == 0 {
            return;
        }
        let target = vsync_done + Duration::from_micros(self.delay_us);
        let now = std::time::Instant::now();
        if target > now {
            std::thread::sleep(target - now);
        }
    }
}

fn present_delay_from_value(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<u64>() {
        Ok(delay) => Some(delay.min(50_000)),
        Err(_) => {
            crate::ui_errln!(
                "ui: ignoring invalid {PRESENT_DELAY_ENV}={value:?}; expected microseconds"
            );
            None
        }
    }
}

impl Platform for MisterPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> core::time::Duration {
        self.fixed_time
            .as_ref()
            .map(|t| t.get())
            .unwrap_or_else(|| self.start.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::ComponentHandle;
    use slint::platform::software_renderer::Rgb565Pixel;

    slint::slint! {
        component ReusedRasterProbe inherits Window {
            in property <length> tile-x;
            in property <bool> tile-visible: true;
            width: 64px;
            height: 48px;
            background: black;

            Rectangle {
                x: root.tile-x;
                y: 8px;
                width: 8px;
                height: 8px;
                visible: root.tile-visible;
                background: red;
            }
        }
    }

    #[test]
    fn software_window_redraw_state_is_authoritative() {
        let window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        assert!(!window.redraw_pending());

        WindowAdapter::request_redraw(window.as_ref());
        assert!(window.redraw_pending());

        let mut rendered = false;
        assert!(window.draw_if_needed(|_| rendered = true));
        assert!(rendered);
        assert!(!window.redraw_pending());
        assert!(!window.draw_if_needed(|_| panic!("idle window rendered")));
    }

    #[test]
    fn reused_full_raster_refreshes_moved_deleted_and_rotated_content() {
        let window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let _ = slint::platform::set_platform(Box::new(MisterPlatform::new(
            window.clone(),
            Some(Rc::new(Cell::new(Duration::ZERO))),
        )));
        let ui = ReusedRasterProbe::new().expect("probe component");
        window.set_size(PhysicalSize::new(64, 48));
        ui.set_tile_x(4.0);
        ui.show().expect("show probe");

        let mut pixels = vec![Rgb565Pixel(0); 64 * 48];
        assert!(window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 64);
        }));
        assert_ne!(pixels[8 * 64 + 4].0, 0);

        ui.set_tile_x(20.0);
        assert!(window.draw_full_frame_reused_if_needed(64, 48, |renderer| {
            assert_eq!(
                renderer.repaint_buffer_type(),
                RepaintBufferType::ReusedBuffer
            );
            renderer.render(&mut pixels, 64);
        }));
        assert_eq!(pixels[8 * 64 + 4].0, 0);
        assert_ne!(pixels[8 * 64 + 20].0, 0);

        ui.set_tile_visible(false);
        assert!(window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 64);
        }));
        assert_eq!(pixels[8 * 64 + 20].0, 0);

        for (rotation, width, height, stride) in [
            (RenderingRotation::NoRotation, 64, 48, 64),
            (RenderingRotation::Rotate90, 48, 64, 48),
            (RenderingRotation::Rotate270, 48, 64, 48),
        ] {
            window.set_rendering_rotation(rotation);
            WindowAdapter::request_redraw(window.as_ref());
            assert!(window.draw_full_frame_reused_if_needed(64, 48, |renderer| {
                let region = renderer.render(&mut pixels, stride);
                assert_eq!(region.bounding_box_size().width, width);
                assert_eq!(region.bounding_box_size().height, height);
            }));
        }
    }

    #[test]
    fn present_delay_parses_microseconds() {
        assert_eq!(present_delay_from_value("2500"), Some(2500));
        assert_eq!(present_delay_from_value(""), None);
    }

    #[test]
    fn present_delay_clamps_extreme_values() {
        assert_eq!(present_delay_from_value("999999"), Some(50_000));
    }

    #[test]
    fn present_delay_rejects_invalid_text() {
        assert_eq!(present_delay_from_value("later"), None);
    }
}
