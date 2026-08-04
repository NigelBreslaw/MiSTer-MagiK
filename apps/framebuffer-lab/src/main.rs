// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_framebuffer_lab::Rgb565Pixel;
use mister_magik_framebuffer_lab::particles::showcase::{
    ParticleDemoKind, ParticleShowcaseConfig, ParticleShowcaseRenderer,
};
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const WIDTH: usize = 960;
const HEIGHT: usize = 540;
const SEED: u64 = 827_141_709_451;
const FRAME_RATE: u64 = 60;
const FRAME_DURATION: Duration = Duration::from_nanos(1_000_000_000 / FRAME_RATE);

fn main() -> Result<(), String> {
    let options = Options::parse(env::args().skip(1))?;
    let demo = ParticleDemoKind::parse(&options.demo)
        .ok_or_else(|| format!("unknown particle demo {:?}", options.demo))?;
    if options.check {
        let _ = renderer(demo, options.family.as_deref(), false, WIDTH, HEIGHT)?;
        println!("particle-lab check passed demo={}", demo.telemetry_label());
        return Ok(());
    }
    if let Some(output) = options.output.as_deref() {
        return render_headless(
            demo,
            options.family.as_deref(),
            options
                .time_ms
                .expect("option parser requires time with output"),
            output,
        );
    }
    run_window(demo, options.family)
}

fn renderer(
    demo: ParticleDemoKind,
    family: Option<&Path>,
    live: bool,
    width: usize,
    height: usize,
) -> Result<ParticleShowcaseRenderer, String> {
    let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
        width,
        height,
        seed: SEED,
        initial_demo: demo,
    })?;
    if let Some(path) = family {
        if live {
            renderer.enable_live_family(path.to_path_buf())?;
        } else {
            renderer.load_family_file(path)?;
        }
    }
    renderer.configure_capture_hud(false);
    Ok(renderer)
}

fn render_headless(
    demo: ParticleDemoKind,
    family: Option<&Path>,
    time_ms: u64,
    output: &Path,
) -> Result<(), String> {
    let mut renderer = renderer(demo, family, false, WIDTH, HEIGHT)?;
    let mut slots = [
        vec![Rgb565Pixel(0); WIDTH * HEIGHT],
        vec![Rgb565Pixel(0); WIDTH * HEIGHT],
    ];
    let target = Duration::from_millis(time_ms);
    let mut elapsed = Duration::ZERO;
    let mut frame = 0_u64;
    loop {
        let slot = frame as usize & 1;
        renderer.render(&mut slots[slot], (slot + 1) as u8, elapsed)?;
        if elapsed >= target {
            write_ppm(output, &slots[slot])?;
            println!(
                "capture={} demo={} time_ms={} hash={:016x}",
                output.display(),
                demo.telemetry_label(),
                time_ms,
                frame_hash(&slots[slot])
            );
            return Ok(());
        }
        elapsed = (elapsed + FRAME_DURATION).min(target);
        frame = frame.saturating_add(1);
    }
}

#[cfg(target_os = "macos")]
fn run_window(demo: ParticleDemoKind, family: Option<PathBuf>) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::new()
        .map_err(|error| format!("create particle-lab event loop: {error}"))?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let mut application = macos::ParticleLabApplication::new(demo, family)?;
    event_loop
        .run_app(&mut application)
        .map_err(|error| format!("run particle-lab window: {error}"))
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn run_window(demo: ParticleDemoKind, family: Option<PathBuf>) -> Result<(), String> {
    use mister_magik_mister_runtime::display_plan::detect_runtime_display_plan;
    use mister_magik_mister_runtime::fpga::Fpga;
    use mister_magik_mister_runtime::framebuffer::damage::{DirtyRect, DirtyRectList};
    use mister_magik_mister_runtime::framebuffer::hidden_latch::CachedHiddenLatchPresenter;
    use std::time::Instant;

    let mut fpga = Fpga::open().map_err(|error| format!("open FPGA display detector: {error}"))?;
    let runtime = detect_runtime_display_plan(&mut fpga)
        .map_err(|error| format!("resolve Main display plan: {error}"))?;
    drop(fpga);
    let plan = runtime.plan;
    let width = plan.render_w;
    let height = plan.render_h;
    let mut renderer = renderer(demo, family.as_deref(), true, width, height)?;
    let mut presenter = CachedHiddenLatchPresenter::open(plan)
        .map_err(|error| format!("open cached RGB565 latch presenter: {error}"))?;
    let mut render_slots = [
        vec![Rgb565Pixel(0); width * height],
        vec![Rgb565Pixel(0); width * height],
    ];
    let full_damage = DirtyRectList::from_one(DirtyRect {
        x0: 0,
        y0: 0,
        x1: width,
        y1: height,
    });
    println!(
        "particle-lab render={}x{} framebuffer={}x{} scan={}x{} output={}x{} format=rgb565",
        width, height, plan.fb_w, plan.fb_h, plan.scan_w, plan.scan_h, plan.output_w, plan.output_h,
    );
    let started = Instant::now();
    let mut next_frame = started;
    let mut status_started = started;
    let mut status_frames = 0_u64;
    loop {
        presenter
            .settle_pending()
            .map_err(|error| format!("settle hidden RGB565 particle frame: {error}"))?;
        let elapsed = Instant::now().saturating_duration_since(started);
        let slot = presenter.writable_slot_index();
        let slot_offset = usize::from(slot - 1);
        let stats = renderer.render(&mut render_slots[slot_offset], slot, elapsed)?;
        presenter
            .prepare_cached(&render_slots[slot_offset], &full_damage)
            .map_err(|error| format!("prepare hidden RGB565 particle frame: {error}"))?;
        let post = presenter
            .post_prepared()
            .map_err(|error| format!("post hidden RGB565 particle frame: {error}"))?;
        status_frames = status_frames.saturating_add(1);
        if status_started.elapsed() >= Duration::from_secs(1) {
            let seconds = status_started.elapsed().as_secs_f64();
            let reload = renderer
                .live_reload_status_label()
                .unwrap_or_else(|| "embedded".into());
            let error = renderer.live_reload_error().unwrap_or("none");
            println!(
                "particle-lab demo={} generation={} fps={:.1} visible={} slot={} sequence={} reload_error={}",
                stats.demo.telemetry_label(),
                reload,
                status_frames as f64 / seconds,
                stats.visible,
                post.slot_index,
                post.sequence,
                error
            );
            status_started = Instant::now();
            status_frames = 0;
        }
        next_frame += FRAME_DURATION;
        if let Some(remaining) = next_frame.checked_duration_since(Instant::now()) {
            std::thread::sleep(remaining);
        } else {
            next_frame = Instant::now();
        }
    }
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_arch = "arm"))))]
fn run_window(_demo: ParticleDemoKind, _family: Option<PathBuf>) -> Result<(), String> {
    Err("interactive particle lab requires macOS or the ARM MiSTer target".into())
}

struct Options {
    demo: String,
    family: Option<PathBuf>,
    time_ms: Option<u64>,
    output: Option<PathBuf>,
    check: bool,
}

impl Options {
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut demo = None;
        let mut family = None;
        let mut time_ms = None;
        let mut output = None;
        let mut check = false;
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--demo" => demo = arguments.next(),
                "--family" => family = arguments.next().map(PathBuf::from),
                "--time-ms" => {
                    let value = arguments.next().ok_or("--time-ms requires milliseconds")?;
                    time_ms = Some(
                        value
                            .parse()
                            .map_err(|_| format!("invalid time in milliseconds {value:?}"))?,
                    );
                }
                "--output" => output = arguments.next().map(PathBuf::from),
                "--check" | "--validate-only" => check = true,
                "--help" | "-h" => return Err(usage().into()),
                other => return Err(format!("unknown particle-lab argument {other:?}")),
            }
        }
        let options = Self {
            demo: demo.ok_or("--demo is required")?,
            family,
            time_ms,
            output,
            check,
        };
        options.validate()?;
        Ok(options)
    }

    fn validate(&self) -> Result<(), String> {
        if self.check && (self.time_ms.is_some() || self.output.is_some()) {
            return Err("--check cannot be combined with --time-ms or --output".into());
        }
        if self.output.is_some() != self.time_ms.is_some() {
            return Err("deterministic capture requires both --time-ms and --output".into());
        }
        Ok(())
    }
}

fn usage() -> &'static str {
    "usage:\n  mister-magik-particle-lab --demo ID [--family FILE.json]\n  mister-magik-particle-lab --demo ID [--family FILE.json] --time-ms N --output FILE.ppm\n  mister-magik-particle-lab --demo ID [--family FILE.json] --check"
}

fn write_ppm(path: &Path, pixels: &[Rgb565Pixel]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(format!("P6\n{WIDTH} {HEIGHT}\n255\n").as_bytes())
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    for &pixel in pixels {
        file.write_all(&rgb565_to_rgb888(pixel))
            .map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    Ok(())
}

fn rgb565_to_rgb888(pixel: Rgb565Pixel) -> [u8; 3] {
    let red = ((pixel.0 >> 11) & 0x1f) as u8;
    let green = ((pixel.0 >> 5) & 0x3f) as u8;
    let blue = (pixel.0 & 0x1f) as u8;
    [
        (u16::from(red) * 255 / 31) as u8,
        (u16::from(green) * 255 / 63) as u8,
        (u16::from(blue) * 255 / 31) as u8,
    ]
}

fn frame_hash(pixels: &[Rgb565Pixel]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for pixel in pixels {
        for byte in pixel.0.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use softbuffer::{Context, Surface};
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::time::Instant;
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::{ElementState, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, ControlFlow};
    use winit::keyboard::{KeyCode, PhysicalKey};
    use winit::window::{Window, WindowId};

    pub(super) struct ParticleLabApplication {
        renderer: ParticleShowcaseRenderer,
        demo: ParticleDemoKind,
        window: Option<Arc<Window>>,
        surface: Option<Surface<Arc<Window>, Arc<Window>>>,
        slots: [Vec<Rgb565Pixel>; 2],
        xrgb8888: Vec<u32>,
        epoch: Instant,
        frame: u64,
        next_frame: Instant,
        fps_started: Instant,
        fps_frames: u64,
        fps: f64,
        last_title: String,
        render_error: Option<String>,
    }

    impl ParticleLabApplication {
        pub(super) fn new(demo: ParticleDemoKind, family: Option<PathBuf>) -> Result<Self, String> {
            let renderer = renderer(demo, family.as_deref(), true, WIDTH, HEIGHT)?;
            let now = Instant::now();
            Ok(Self {
                renderer,
                demo,
                window: None,
                surface: None,
                slots: [
                    vec![Rgb565Pixel(0); WIDTH * HEIGHT],
                    vec![Rgb565Pixel(0); WIDTH * HEIGHT],
                ],
                xrgb8888: Vec::new(),
                epoch: now,
                frame: 0,
                next_frame: now,
                fps_started: now,
                fps_frames: 0,
                fps: 0.0,
                last_title: String::new(),
                render_error: None,
            })
        }

        fn create_window(&mut self, event_loop: &ActiveEventLoop) {
            let attributes = Window::default_attributes()
                .with_title(self.title())
                .with_inner_size(LogicalSize::new(WIDTH as f64, HEIGHT as f64))
                .with_min_inner_size(LogicalSize::new((WIDTH / 2) as f64, (HEIGHT / 2) as f64));
            let window = Arc::new(
                event_loop
                    .create_window(attributes)
                    .expect("create particle-lab window"),
            );
            let context =
                Context::new(Arc::clone(&window)).expect("create particle-lab softbuffer context");
            let surface = Surface::new(&context, Arc::clone(&window))
                .expect("create particle-lab softbuffer surface");
            self.window = Some(window);
            self.surface = Some(surface);
        }

        fn title(&self) -> String {
            let recipes = self
                .renderer
                .live_reload_status_label()
                .unwrap_or_else(|| "recipes:embedded:0".into());
            let error = self
                .render_error
                .as_deref()
                .or_else(|| self.renderer.live_reload_error());
            let error = error.map_or_else(String::new, |error| format!(" — error: {error}"));
            format!(
                "MiSTer MagiK Particle Lab — {:02}/36 {} — {recipes} — {:.1} fps{error}",
                self.demo.number(),
                self.demo.label(),
                self.fps,
            )
        }

        fn update_title(&mut self) {
            let title = self.title();
            if title != self.last_title {
                if let Some(window) = self.window.as_ref() {
                    window.set_title(&title);
                }
                self.last_title = title;
            }
        }

        fn render(&mut self) {
            let Some(window) = self.window.as_ref() else {
                return;
            };
            let slot = self.frame as usize & 1;
            let elapsed = self.epoch.elapsed();
            if let Err(error) =
                self.renderer
                    .render(&mut self.slots[slot], (slot + 1) as u8, elapsed)
            {
                self.render_error = Some(error);
                self.update_title();
                return;
            }
            self.frame = self.frame.saturating_add(1);
            self.fps_frames = self.fps_frames.saturating_add(1);
            let fps_elapsed = self.fps_started.elapsed();
            if fps_elapsed >= Duration::from_secs(1) {
                self.fps = self.fps_frames as f64 / fps_elapsed.as_secs_f64();
                self.fps_started = Instant::now();
                self.fps_frames = 0;
            }

            let size = window.inner_size();
            let Some(width) = NonZeroU32::new(size.width) else {
                return;
            };
            let Some(height) = NonZeroU32::new(size.height) else {
                return;
            };
            let Some(surface) = self.surface.as_mut() else {
                return;
            };
            surface
                .resize(width, height)
                .expect("resize particle-lab surface");
            self.xrgb8888
                .resize(size.width as usize * size.height as usize, 0);
            scale_rgb565_nearest(
                &self.slots[slot],
                &mut self.xrgb8888,
                size.width as usize,
                size.height as usize,
            );
            let mut buffer = surface.buffer_mut().expect("map particle-lab surface");
            buffer.copy_from_slice(&self.xrgb8888);
            buffer.present().expect("present particle-lab surface");
            self.update_title();
        }
    }

    impl ApplicationHandler for ParticleLabApplication {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_none() {
                self.create_window(event_loop);
            }
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            window_id: WindowId,
            event: WindowEvent,
        ) {
            if self.window.as_ref().map(|window| window.id()) != Some(window_id) {
                return;
            }
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::RedrawRequested => self.render(),
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed
                        && matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape)) =>
                {
                    event_loop.exit();
                }
                WindowEvent::Resized(_) => {
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            let now = Instant::now();
            if now >= self.next_frame {
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
                while self.next_frame <= now {
                    self.next_frame += FRAME_DURATION;
                }
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
        }
    }

    fn scale_rgb565_nearest(
        source: &[Rgb565Pixel],
        destination: &mut [u32],
        destination_width: usize,
        destination_height: usize,
    ) {
        if destination_width == 0 || destination_height == 0 {
            return;
        }
        let scale = (destination_width / WIDTH)
            .min(destination_height / HEIGHT)
            .max(1);
        let content_width = (WIDTH * scale).min(destination_width);
        let content_height = (HEIGHT * scale).min(destination_height);
        let offset_x = (destination_width - content_width) / 2;
        let offset_y = (destination_height - content_height) / 2;
        destination.fill(0);
        for destination_y in 0..content_height {
            let source_y = destination_y * HEIGHT / content_height;
            for destination_x in 0..content_width {
                let source_x = destination_x * WIDTH / content_width;
                let [red, green, blue] = rgb565_to_rgb888(source[source_y * WIDTH + source_x]);
                destination
                    [(offset_y + destination_y) * destination_width + offset_x + destination_x] =
                    u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_interactive_capture_and_check_contracts() {
        let interactive = Options::parse(
            ["--demo", "grid-flocking", "--family", "procedural.json"].map(String::from),
        )
        .unwrap();
        assert_eq!(interactive.demo, "grid-flocking");
        assert_eq!(interactive.family, Some(PathBuf::from("procedural.json")));
        assert!(interactive.output.is_none());

        let capture = Options::parse(
            ["--demo", "13", "--time-ms", "15000", "--output", "fire.ppm"].map(String::from),
        )
        .unwrap();
        assert_eq!(capture.time_ms, Some(15_000));
        assert_eq!(capture.output, Some(PathBuf::from("fire.ppm")));

        let check = Options::parse(["--demo", "13", "--check"].map(String::from)).unwrap();
        assert!(check.check);
    }

    #[test]
    fn rejects_partial_or_conflicting_modes() {
        assert!(Options::parse(["--demo", "13", "--output", "x.ppm"].map(String::from)).is_err());
        assert!(
            Options::parse(["--demo", "13", "--check", "--time-ms", "1"].map(String::from))
                .is_err()
        );
    }

    #[test]
    fn rgb565_primary_channels_expand_to_rgb888() {
        assert_eq!(rgb565_to_rgb888(Rgb565Pixel(0xf800)), [255, 0, 0]);
        assert_eq!(rgb565_to_rgb888(Rgb565Pixel(0x07e0)), [0, 255, 0]);
        assert_eq!(rgb565_to_rgb888(Rgb565Pixel(0x001f)), [0, 0, 255]);
    }
}
