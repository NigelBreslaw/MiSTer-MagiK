// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_particles::cabinet::Rgb565Pixel;
#[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "arm")))]
use mister_magik_startup_particle_lab::LiveParticleRenderer;
use mister_magik_startup_particle_lab::{
    DEFAULT_HEIGHT, DEFAULT_WIDTH, FocusedParticleRenderer, read_effect_recipe,
};
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const FRAME_RATE: u64 = 60;
const FRAME_DURATION: Duration = Duration::from_nanos(1_000_000_000 / FRAME_RATE);

fn main() -> Result<(), String> {
    let options = Options::parse(env::args().skip(1))?;
    if options.check {
        let recipe = read_effect_recipe(&options.recipe)?;
        let renderer = FocusedParticleRenderer::new(DEFAULT_WIDTH, DEFAULT_HEIGHT, recipe)?;
        println!(
            "startup-particle-lab check passed effect={}",
            renderer.kind().label()
        );
        return Ok(());
    }
    if let Some(output) = options.output.as_deref() {
        return render_headless(
            &options.recipe,
            options
                .time_ms
                .expect("option parser requires time with output"),
            output,
        );
    }
    run_window(options.recipe)
}

fn render_headless(recipe_path: &Path, time_ms: u64, output: &Path) -> Result<(), String> {
    let recipe = read_effect_recipe(recipe_path)?;
    let mut renderer = FocusedParticleRenderer::new(DEFAULT_WIDTH, DEFAULT_HEIGHT, recipe)?;
    let mut pixels = vec![Rgb565Pixel(0); DEFAULT_WIDTH * DEFAULT_HEIGHT];
    let target = Duration::from_millis(time_ms);
    let mut elapsed = Duration::ZERO;
    loop {
        let stats = renderer.render(&mut pixels, elapsed)?;
        if elapsed >= target {
            write_ppm(output, &pixels)?;
            println!(
                "capture={} effect={} time_ms={} hash={:016x}",
                output.display(),
                stats.effect.label(),
                time_ms,
                frame_hash(&pixels)
            );
            return Ok(());
        }
        elapsed = (elapsed + FRAME_DURATION).min(target);
    }
}

#[cfg(target_os = "macos")]
fn run_window(recipe: PathBuf) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::new()
        .map_err(|error| format!("create startup-particle-lab event loop: {error}"))?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let mut application = macos::ParticleLabApplication::new(recipe)?;
    event_loop
        .run_app(&mut application)
        .map_err(|error| format!("run startup-particle-lab window: {error}"))
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn run_window(recipe: PathBuf) -> Result<(), String> {
    use mister_magik_mister_runtime::framebuffer::hidden_latch::HiddenLatchPresenter;
    use std::time::Instant;

    let mut renderer = LiveParticleRenderer::start(
        DEFAULT_WIDTH,
        DEFAULT_HEIGHT,
        recipe.clone(),
        status_path(&recipe),
    )?;
    let mut presenter = HiddenLatchPresenter::open(DEFAULT_WIDTH as u16, DEFAULT_HEIGHT as u16)
        .map_err(|error| format!("open hidden RGB565 latch presenter: {error}"))?;
    if presenter.stride_pixels() != DEFAULT_WIDTH {
        return Err(format!(
            "startup particle lab requires a packed {DEFAULT_WIDTH}-pixel stride, received {}",
            presenter.stride_pixels()
        ));
    }
    let started = Instant::now();
    let mut next_frame = started;
    let mut status_started = started;
    let mut cpu_started = process_cpu_time();
    let mut status_frames = 0_u64;
    let mut render_samples_us = Vec::with_capacity(64);
    let mut last_sequence = None;
    let mut repeated_presentations = 0_u64;
    loop {
        let elapsed = Instant::now().saturating_duration_since(started);
        let pixels = presenter.pixels_mut();
        // SAFETY: both pixel types are repr(transparent) wrappers around u16,
        // have identical length/alignment, and accept every u16 bit pattern.
        let pixels = unsafe {
            std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<Rgb565Pixel>(), pixels.len())
        };
        let render_started = Instant::now();
        let stats = renderer.render(pixels, elapsed)?;
        render_samples_us.push(render_started.elapsed().as_micros() as u64);
        let receipt = presenter
            .present()
            .map_err(|error| format!("present hidden RGB565 startup particle frame: {error}"))?;
        if last_sequence.is_some_and(|sequence| receipt.sequence <= sequence) {
            repeated_presentations = repeated_presentations.saturating_add(1);
        }
        last_sequence = Some(receipt.sequence);
        status_frames = status_frames.saturating_add(1);
        if status_started.elapsed() >= Duration::from_secs(1) {
            let seconds = status_started.elapsed().as_secs_f64();
            let cpu_now = process_cpu_time();
            let cpu_percent = cpu_now.saturating_sub(cpu_started).as_secs_f64() / seconds * 100.0;
            render_samples_us.sort_unstable();
            let render_p99_us = render_samples_us
                .get(
                    render_samples_us
                        .len()
                        .saturating_mul(99)
                        .div_ceil(100)
                        .saturating_sub(1),
                )
                .copied()
                .unwrap_or_default();
            let render_max_us = render_samples_us.last().copied().unwrap_or_default();
            let render_average_us = if render_samples_us.is_empty() {
                0
            } else {
                render_samples_us.iter().sum::<u64>() / render_samples_us.len() as u64
            };
            let error = renderer.last_error().unwrap_or("none");
            println!(
                "startup-particle-lab effect={} generation={} state={:?} fps={:.1} cpu_pct={:.1} render_avg_us={} render_p99_us={} render_max_us={} visible={} simulation_backend={} projection_backend={} slot={} sequence={} repeated_presentations={} reload_error={}",
                stats.effect.label(),
                renderer.generation(),
                renderer.status_state(),
                status_frames as f64 / seconds,
                cpu_percent,
                render_average_us,
                render_p99_us,
                render_max_us,
                stats.visible,
                stats.simulation_backend,
                stats.projection_backend,
                receipt.slot_index,
                receipt.sequence,
                repeated_presentations,
                error,
            );
            status_started = Instant::now();
            cpu_started = cpu_now;
            status_frames = 0;
            render_samples_us.clear();
            repeated_presentations = 0;
        }
        next_frame += FRAME_DURATION;
        if let Some(remaining) = next_frame.checked_duration_since(Instant::now()) {
            std::thread::sleep(remaining);
        } else {
            next_frame = Instant::now();
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn process_cpu_time() -> Duration {
    let mut value = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: clock_gettime initializes the supplied timespec on success and
    // CLOCK_PROCESS_CPUTIME_ID requires no external resources or ownership.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, value.as_mut_ptr()) };
    if result != 0 {
        return Duration::ZERO;
    }
    // SAFETY: the successful call above initialized every field.
    let value = unsafe { value.assume_init() };
    Duration::new(value.tv_sec as u64, value.tv_nsec as u32)
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_arch = "arm"))))]
fn run_window(_recipe: PathBuf) -> Result<(), String> {
    Err("interactive startup particle preview requires macOS or ARM MiSTer".into())
}

struct Options {
    recipe: PathBuf,
    time_ms: Option<u64>,
    output: Option<PathBuf>,
    check: bool,
}

impl Options {
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut recipe = None;
        let mut time_ms = None;
        let mut output = None;
        let mut check = false;
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--recipe" => recipe = arguments.next().map(PathBuf::from),
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
                other => return Err(format!("unknown startup-particle-lab argument {other:?}")),
            }
        }
        let options = Self {
            recipe: recipe.ok_or("--recipe is required")?,
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
    "usage:\n  mister-magik-startup-particle-lab --recipe FILE.json\n  mister-magik-startup-particle-lab --recipe FILE.json --time-ms N --output FILE.ppm\n  mister-magik-startup-particle-lab --recipe FILE.json --check"
}

fn status_path(recipe: &Path) -> PathBuf {
    recipe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("status.json")
}

fn write_ppm(path: &Path, pixels: &[Rgb565Pixel]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(format!("P6\n{DEFAULT_WIDTH} {DEFAULT_HEIGHT}\n255\n").as_bytes())
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
        renderer: LiveParticleRenderer,
        window: Option<Arc<Window>>,
        surface: Option<Surface<Arc<Window>, Arc<Window>>>,
        pixels: Vec<Rgb565Pixel>,
        xrgb8888: Vec<u32>,
        epoch: Instant,
        next_frame: Instant,
        fps_started: Instant,
        fps_frames: u64,
        fps: f64,
        last_title: String,
        render_error: Option<String>,
    }

    impl ParticleLabApplication {
        pub(super) fn new(recipe: PathBuf) -> Result<Self, String> {
            let renderer = LiveParticleRenderer::start(
                DEFAULT_WIDTH,
                DEFAULT_HEIGHT,
                recipe.clone(),
                status_path(&recipe),
            )?;
            let now = Instant::now();
            Ok(Self {
                renderer,
                window: None,
                surface: None,
                pixels: vec![Rgb565Pixel(0); DEFAULT_WIDTH * DEFAULT_HEIGHT],
                xrgb8888: Vec::new(),
                epoch: now,
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
                .with_inner_size(LogicalSize::new(
                    DEFAULT_WIDTH as f64,
                    DEFAULT_HEIGHT as f64,
                ))
                .with_min_inner_size(LogicalSize::new(
                    (DEFAULT_WIDTH / 2) as f64,
                    (DEFAULT_HEIGHT / 2) as f64,
                ));
            let window = Arc::new(
                event_loop
                    .create_window(attributes)
                    .expect("create startup-particle-lab window"),
            );
            let context = Context::new(Arc::clone(&window))
                .expect("create startup-particle-lab softbuffer context");
            let surface = Surface::new(&context, Arc::clone(&window))
                .expect("create startup-particle-lab softbuffer surface");
            self.window = Some(window);
            self.surface = Some(surface);
        }

        fn title(&self) -> String {
            let error = self
                .render_error
                .as_deref()
                .or_else(|| self.renderer.last_error())
                .map_or_else(String::new, |error| format!(" — error: {error}"));
            format!(
                "MiSTer MagiK Startup Particles — {} — generation {} {:?} — {:.1} fps{error}",
                self.renderer.effect().label(),
                self.renderer.generation(),
                self.renderer.status_state(),
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
            if let Err(error) = self.renderer.render(&mut self.pixels, self.epoch.elapsed()) {
                self.render_error = Some(error);
                self.update_title();
                return;
            }
            self.render_error = None;
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
                .expect("resize startup-particle-lab surface");
            self.xrgb8888
                .resize(size.width as usize * size.height as usize, 0);
            scale_rgb565_nearest(
                &self.pixels,
                &mut self.xrgb8888,
                size.width as usize,
                size.height as usize,
            );
            let mut buffer = surface
                .buffer_mut()
                .expect("map startup-particle-lab surface");
            buffer.copy_from_slice(&self.xrgb8888);
            buffer
                .present()
                .expect("present startup-particle-lab surface");
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
        let content_scale = (destination_width as f64 / DEFAULT_WIDTH as f64)
            .min(destination_height as f64 / DEFAULT_HEIGHT as f64);
        let content_width = (DEFAULT_WIDTH as f64 * content_scale).round() as usize;
        let content_height = (DEFAULT_HEIGHT as f64 * content_scale).round() as usize;
        let offset_x = (destination_width - content_width) / 2;
        let offset_y = (destination_height - content_height) / 2;
        destination.fill(0);
        for destination_y in 0..content_height {
            let source_y = destination_y * DEFAULT_HEIGHT / content_height;
            for destination_x in 0..content_width {
                let source_x = destination_x * DEFAULT_WIDTH / content_width;
                let [red, green, blue] =
                    rgb565_to_rgb888(source[source_y * DEFAULT_WIDTH + source_x]);
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
        let interactive = Options::parse(["--recipe", "magik.json"].map(String::from)).unwrap();
        assert_eq!(interactive.recipe, PathBuf::from("magik.json"));
        assert!(interactive.output.is_none());

        let capture = Options::parse(
            [
                "--recipe",
                "cabinet.json",
                "--time-ms",
                "5000",
                "--output",
                "cabinet.ppm",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(capture.time_ms, Some(5_000));
        assert_eq!(capture.output, Some(PathBuf::from("cabinet.ppm")));

        let check =
            Options::parse(["--recipe", "magik.json", "--check"].map(String::from)).unwrap();
        assert!(check.check);
    }

    #[test]
    fn rejects_partial_or_conflicting_modes() {
        assert!(
            Options::parse(["--recipe", "magik.json", "--output", "x.ppm"].map(String::from))
                .is_err()
        );
        assert!(
            Options::parse(
                ["--recipe", "magik.json", "--check", "--time-ms", "1"].map(String::from)
            )
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
