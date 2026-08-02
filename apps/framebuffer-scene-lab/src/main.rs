// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "arm")))]
use mister_magik_framebuffer_scene_lab::LiveParticleRenderer;
use mister_magik_framebuffer_scene_lab::{
    DEFAULT_HEIGHT, DEFAULT_WIDTH, EffectKind, FocusedParticleRenderer, NavigationFixture,
    NavigationFixtureScene, read_effect_recipe,
};
use mister_magik_particles::cabinet::Rgb565Pixel;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const FRAME_RATE: u64 = 60;
const FRAME_DURATION: Duration = Duration::from_nanos(1_000_000_000 / FRAME_RATE);

fn main() -> Result<(), String> {
    let options = Options::parse(env::args().skip(1))?;
    if options.scene == EffectKind::NavigationTransition {
        let fixture = options
            .fixture
            .expect("navigation option validation requires a fixture");
        if options.check {
            let _scene = NavigationFixtureScene::new(fixture);
            println!(
                "framebuffer-scene-lab check passed scene={} fixture={}",
                options.scene.label(),
                fixture.label()
            );
            return Ok(());
        }
        if let Some(output) = options.output.as_deref() {
            return render_navigation_headless(
                fixture,
                options
                    .time_ms
                    .expect("option parser requires time with output"),
                output,
            );
        }
        return run_window(SceneSource::Navigation(fixture), options.destination);
    }
    let recipe_path = options
        .recipe
        .as_deref()
        .expect("particle option validation requires a recipe");
    let selected = read_effect_recipe(recipe_path)?;
    if selected.kind() != options.scene {
        return Err(format!(
            "--scene {} does not match the {} recipe",
            options.scene.label(),
            selected.kind().label()
        ));
    }
    if options.check {
        let renderer = FocusedParticleRenderer::new(DEFAULT_WIDTH, DEFAULT_HEIGHT, selected)?;
        println!(
            "framebuffer-scene-lab check passed effect={}",
            renderer.kind().label()
        );
        return Ok(());
    }
    if let Some(output) = options.output.as_deref() {
        return render_headless(
            recipe_path,
            options
                .time_ms
                .expect("option parser requires time with output"),
            output,
        );
    }
    run_window(
        SceneSource::Particle(recipe_path.to_path_buf()),
        options.destination,
    )
}

#[derive(Clone, Debug)]
enum SceneSource {
    Particle(PathBuf),
    Navigation(NavigationFixture),
}

fn render_headless(recipe_path: &Path, time_ms: u64, output: &Path) -> Result<(), String> {
    let recipe = read_effect_recipe(recipe_path)?;
    let mut renderer =
        FocusedParticleRenderer::new_synchronous(DEFAULT_WIDTH, DEFAULT_HEIGHT, recipe)?;
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

fn render_navigation_headless(
    fixture: NavigationFixture,
    time_ms: u64,
    output: &Path,
) -> Result<(), String> {
    let mut renderer = NavigationFixtureScene::new(fixture);
    let mut pixels = vec![Rgb565Pixel(0); DEFAULT_WIDTH * DEFAULT_HEIGHT];
    let stats = renderer.render(&mut pixels, Duration::from_millis(time_ms))?;
    write_ppm(output, &pixels)?;
    println!(
        "capture={} scene={} fixture={} time_ms={} hash={:016x}",
        output.display(),
        stats.effect.label(),
        fixture.label(),
        time_ms,
        frame_hash(&pixels)
    );
    Ok(())
}

#[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "arm")))]
enum LabScene {
    Particle(LiveParticleRenderer),
    Navigation(NavigationFixtureScene),
}

#[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "arm")))]
impl LabScene {
    fn start(source: SceneSource) -> Result<Self, String> {
        match source {
            SceneSource::Particle(recipe) => Ok(Self::Particle(LiveParticleRenderer::start(
                DEFAULT_WIDTH,
                DEFAULT_HEIGHT,
                recipe.clone(),
                status_path(&recipe),
            )?)),
            SceneSource::Navigation(fixture) => {
                Ok(Self::Navigation(NavigationFixtureScene::new(fixture)))
            }
        }
    }

    fn render_buffer(
        &mut self,
        destination: &mut [Rgb565Pixel],
        buffer_id: u8,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
    ) -> Result<mister_magik_framebuffer_scene_lab::FrameStats, String> {
        match self {
            Self::Particle(renderer) => {
                renderer.render_buffer(destination, buffer_id, elapsed, next_elapsed)
            }
            Self::Navigation(renderer) => renderer.render(destination, elapsed),
        }
    }

    #[cfg(target_os = "macos")]
    fn effect(&self) -> EffectKind {
        match self {
            Self::Particle(renderer) => renderer.effect(),
            Self::Navigation(_) => EffectKind::NavigationTransition,
        }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::Particle(renderer) => renderer.generation(),
            Self::Navigation(_) => 0,
        }
    }

    fn state_label(&self) -> String {
        match self {
            Self::Particle(renderer) => format!("{:?}", renderer.status_state()),
            Self::Navigation(renderer) => format!("fixture:{}", renderer.fixture().label()),
        }
    }

    fn last_error(&self) -> Option<&str> {
        match self {
            Self::Particle(renderer) => renderer.last_error(),
            Self::Navigation(_) => None,
        }
    }
}

#[cfg(target_os = "macos")]
fn run_window(source: SceneSource, _destination: Option<(u16, u16)>) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::new()
        .map_err(|error| format!("create framebuffer-scene-lab event loop: {error}"))?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let mut application = macos::ParticleLabApplication::new(source)?;
    event_loop
        .run_app(&mut application)
        .map_err(|error| format!("run framebuffer-scene-lab window: {error}"))
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn run_window(source: SceneSource, destination: Option<(u16, u16)>) -> Result<(), String> {
    use mister_magik_mister_runtime::framebuffer::hidden_latch::HiddenLatchPresenter;
    use std::time::Instant;

    let mut renderer = LabScene::start(source)?;
    let (destination_width, destination_height) = destination
        .ok_or("MiSTer startup particle preview requires an explicit scanout destination")?;
    let mut presenter = HiddenLatchPresenter::open_scaled(
        DEFAULT_WIDTH as u16,
        DEFAULT_HEIGHT as u16,
        destination_width,
        destination_height,
    )
    .map_err(|error| format!("open scaled hidden RGB565 latch presenter: {error}"))?;
    if presenter.stride_pixels() != DEFAULT_WIDTH {
        return Err(format!(
            "framebuffer scene lab requires a packed {DEFAULT_WIDTH}-pixel stride, received {}",
            presenter.stride_pixels()
        ));
    }
    println!(
        "framebuffer-scene-lab source={}x{} destination={}x{} format=rgb565",
        presenter.width(),
        presenter.height(),
        presenter.destination_width(),
        presenter.destination_height(),
    );
    let started = Instant::now();
    let mut next_frame = started;
    let mut status_started = started;
    let mut cpu_started = process_cpu_time();
    let mut status_frames = 0_u64;
    let mut render_samples_us = Vec::with_capacity(64);
    let mut clear_samples_us = Vec::with_capacity(64);
    let mut simulation_samples_us = Vec::with_capacity(64);
    let mut projection_samples_us = Vec::with_capacity(64);
    let mut raster_samples_us = Vec::with_capacity(64);
    let mut last_sequence = None;
    let mut repeated_presentations = 0_u64;
    loop {
        let elapsed = Instant::now().saturating_duration_since(started);
        let writable_slot = presenter.writable_slot_index();
        let pixels = presenter.pixels_mut();
        // SAFETY: both pixel types are repr(transparent) wrappers around u16,
        // have identical length/alignment, and accept every u16 bit pattern.
        let pixels = unsafe {
            std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<Rgb565Pixel>(), pixels.len())
        };
        let render_started = Instant::now();
        let stats = renderer.render_buffer(
            pixels,
            writable_slot - 1,
            elapsed,
            Some(elapsed.saturating_add(FRAME_DURATION)),
        )?;
        render_samples_us.push(render_started.elapsed().as_micros() as u64);
        if let Some(stages) = stats.magik_stages {
            clear_samples_us.push(stages.clear_us);
            simulation_samples_us.push(stages.simulation_us);
            projection_samples_us.push(stages.projection_us);
            raster_samples_us.push(stages.raster_us);
        } else {
            clear_samples_us.clear();
            simulation_samples_us.clear();
            projection_samples_us.clear();
            raster_samples_us.clear();
        }
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
            let (render_average_us, render_p99_us, render_max_us) =
                sample_summary(&mut render_samples_us);
            let (clear_average_us, clear_p99_us, _) = sample_summary(&mut clear_samples_us);
            let (simulation_average_us, simulation_p99_us, _) =
                sample_summary(&mut simulation_samples_us);
            let (projection_average_us, projection_p99_us, _) =
                sample_summary(&mut projection_samples_us);
            let (raster_average_us, raster_p99_us, _) = sample_summary(&mut raster_samples_us);
            let error = renderer.last_error().unwrap_or("none");
            let magik_stages = if stats.effect != EffectKind::Magik || clear_samples_us.is_empty() {
                "magik_stages=not_applicable".to_owned()
            } else {
                format!(
                    "magik_clear_avg_us={clear_average_us} magik_clear_p99_us={clear_p99_us} magik_simulation_avg_us={simulation_average_us} magik_simulation_p99_us={simulation_p99_us} magik_projection_avg_us={projection_average_us} magik_projection_p99_us={projection_p99_us} magik_raster_avg_us={raster_average_us} magik_raster_p99_us={raster_p99_us}"
                )
            };
            println!(
                "framebuffer-scene-lab effect={} generation={} state={} fps={:.1} cpu_pct={:.1} render_avg_us={} render_p99_us={} render_max_us={} {} visible={} simulation_backend={} projection_backend={} slot={} sequence={} repeated_presentations={} reload_error={}",
                stats.effect.label(),
                renderer.generation(),
                renderer.state_label(),
                status_frames as f64 / seconds,
                cpu_percent,
                render_average_us,
                render_p99_us,
                render_max_us,
                magik_stages,
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
            clear_samples_us.clear();
            simulation_samples_us.clear();
            projection_samples_us.clear();
            raster_samples_us.clear();
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
fn sample_summary(samples: &mut [u64]) -> (u64, u64, u64) {
    samples.sort_unstable();
    let average = if samples.is_empty() {
        0
    } else {
        samples.iter().sum::<u64>() / samples.len() as u64
    };
    let p99 = samples
        .get(
            samples
                .len()
                .saturating_mul(99)
                .div_ceil(100)
                .saturating_sub(1),
        )
        .copied()
        .unwrap_or_default();
    let maximum = samples.last().copied().unwrap_or_default();
    (average, p99, maximum)
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
fn run_window(_source: SceneSource, _destination: Option<(u16, u16)>) -> Result<(), String> {
    Err("interactive startup particle preview requires macOS or ARM MiSTer".into())
}

struct Options {
    scene: EffectKind,
    recipe: Option<PathBuf>,
    fixture: Option<NavigationFixture>,
    time_ms: Option<u64>,
    output: Option<PathBuf>,
    check: bool,
    destination: Option<(u16, u16)>,
}

impl Options {
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut recipe = None;
        let mut fixture = None;
        let mut scene = None;
        let mut time_ms = None;
        let mut output = None;
        let mut check = false;
        let mut destination_width = None;
        let mut destination_height = None;
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--scene" => {
                    let value = arguments.next().ok_or("--scene requires a scene name")?;
                    scene = EffectKind::parse(&value);
                    if scene.is_none() {
                        return Err(format!(
                            "invalid scene {value:?}; expected magik, cabinet, or navigation-transition"
                        ));
                    }
                }
                "--recipe" => recipe = arguments.next().map(PathBuf::from),
                "--fixture" => {
                    let value = arguments.next().ok_or("--fixture requires a name")?;
                    fixture = NavigationFixture::parse(&value);
                    if fixture.is_none() {
                        return Err(format!(
                            "invalid fixture {value:?}; expected home-arcade, home-consoles, or consoles-system"
                        ));
                    }
                }
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
                "--destination-width" => {
                    destination_width =
                        Some(parse_dimension("--destination-width", arguments.next())?);
                }
                "--destination-height" => {
                    destination_height =
                        Some(parse_dimension("--destination-height", arguments.next())?);
                }
                "--help" | "-h" => return Err(usage().into()),
                other => return Err(format!("unknown framebuffer-scene-lab argument {other:?}")),
            }
        }
        let destination = match (destination_width, destination_height) {
            (Some(width), Some(height)) => Some((width, height)),
            (None, None) => None,
            _ => return Err("scanout destination requires both width and height".into()),
        };
        let options = Self {
            scene: scene.ok_or("--scene is required")?,
            recipe,
            fixture,
            time_ms,
            output,
            check,
            destination,
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
        if self.destination.is_some() && (self.check || self.output.is_some()) {
            return Err("scanout destination is only valid for an interactive preview".into());
        }
        match self.scene {
            EffectKind::Magik | EffectKind::Cabinet => {
                if self.recipe.is_none() {
                    return Err("particle scenes require --recipe".into());
                }
                if self.fixture.is_some() {
                    return Err("particle scenes do not accept --fixture".into());
                }
            }
            EffectKind::NavigationTransition => {
                if self.fixture.is_none() {
                    return Err("navigation-transition requires --fixture".into());
                }
                if self.recipe.is_some() {
                    return Err("navigation-transition does not accept --recipe".into());
                }
            }
        }
        Ok(())
    }
}

fn parse_dimension(label: &str, value: Option<String>) -> Result<u16, String> {
    let value = value.ok_or_else(|| format!("{label} requires pixels"))?;
    value
        .parse::<u16>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("invalid {label} value {value:?}"))
}

fn usage() -> &'static str {
    "usage:\n  mister-magik-framebuffer-scene-lab --scene magik|cabinet --recipe FILE.json [--destination-width W --destination-height H]\n  mister-magik-framebuffer-scene-lab --scene navigation-transition --fixture home-arcade|home-consoles|consoles-system [--destination-width W --destination-height H]\n  mister-magik-framebuffer-scene-lab --scene SCENE (--recipe FILE.json|--fixture FIXTURE) --time-ms N --output FILE.ppm\n  mister-magik-framebuffer-scene-lab --scene SCENE (--recipe FILE.json|--fixture FIXTURE) --check"
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
        renderer: LabScene,
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
        pub(super) fn new(source: SceneSource) -> Result<Self, String> {
            let renderer = LabScene::start(source)?;
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
                    .expect("create framebuffer-scene-lab window"),
            );
            let context = Context::new(Arc::clone(&window))
                .expect("create framebuffer-scene-lab softbuffer context");
            let surface = Surface::new(&context, Arc::clone(&window))
                .expect("create framebuffer-scene-lab softbuffer surface");
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
                "MiSTer MagiK Framebuffer Scenes — {} — generation {} {} — {:.1} fps{error}",
                self.renderer.effect().label(),
                self.renderer.generation(),
                self.renderer.state_label(),
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
            let elapsed = self.epoch.elapsed();
            if let Err(error) = self.renderer.render_buffer(
                &mut self.pixels,
                0,
                elapsed,
                Some(elapsed.saturating_add(FRAME_DURATION)),
            ) {
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
                .expect("resize framebuffer-scene-lab surface");
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
                .expect("map framebuffer-scene-lab surface");
            buffer.copy_from_slice(&self.xrgb8888);
            buffer
                .present()
                .expect("present framebuffer-scene-lab surface");
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
    use mister_magik_framebuffer_scene_lab::EffectRecipe;
    use mister_magik_particles::recipes::{embedded_cabinet_recipe, embedded_magik_recipe};

    #[test]
    fn parses_interactive_capture_and_check_contracts() {
        let interactive =
            Options::parse(["--scene", "magik", "--recipe", "magik.json"].map(String::from))
                .unwrap();
        assert_eq!(interactive.recipe, Some(PathBuf::from("magik.json")));
        assert!(interactive.output.is_none());

        let navigation = Options::parse(
            [
                "--scene",
                "navigation-transition",
                "--fixture",
                "home-arcade",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(navigation.fixture, Some(NavigationFixture::HomeArcade));
        assert!(navigation.recipe.is_none());

        let device = Options::parse(
            [
                "--scene",
                "magik",
                "--recipe",
                "magik.json",
                "--destination-width",
                "1920",
                "--destination-height",
                "1080",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(device.destination, Some((1920, 1080)));

        let capture = Options::parse(
            [
                "--scene",
                "cabinet",
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

        let check = Options::parse(
            ["--scene", "magik", "--recipe", "magik.json", "--check"].map(String::from),
        )
        .unwrap();
        assert!(check.check);
    }

    #[test]
    fn rejects_partial_or_conflicting_modes() {
        assert!(
            Options::parse(
                [
                    "--scene",
                    "magik",
                    "--recipe",
                    "magik.json",
                    "--output",
                    "x.ppm",
                ]
                .map(String::from),
            )
            .is_err()
        );
        assert!(
            Options::parse(
                [
                    "--scene",
                    "navigation-transition",
                    "--fixture",
                    "home-arcade",
                    "--recipe",
                    "magik.json",
                ]
                .map(String::from),
            )
            .is_err()
        );
        assert!(Options::parse(["--scene", "navigation-transition"].map(String::from)).is_err());
        assert!(
            Options::parse(
                [
                    "--scene",
                    "magik",
                    "--recipe",
                    "magik.json",
                    "--destination-width",
                    "1920",
                ]
                .map(String::from)
            )
            .is_err()
        );
        assert!(
            Options::parse(
                [
                    "--scene",
                    "magik",
                    "--recipe",
                    "magik.json",
                    "--check",
                    "--time-ms",
                    "1",
                ]
                .map(String::from)
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

    #[test]
    fn fixed_time_particle_frames_match_pre_scene_extraction_baselines() {
        for (recipe, expected) in [
            (
                EffectRecipe::Magik(embedded_magik_recipe().unwrap()),
                0xaa21_3d52_dc7e_eeef,
            ),
            (
                EffectRecipe::Cabinet(embedded_cabinet_recipe().unwrap()),
                0x0193_8d06_cd43_78ff,
            ),
        ] {
            let mut renderer =
                FocusedParticleRenderer::new_synchronous(DEFAULT_WIDTH, DEFAULT_HEIGHT, recipe)
                    .unwrap();
            let mut pixels = vec![Rgb565Pixel(0); DEFAULT_WIDTH * DEFAULT_HEIGHT];
            let target = Duration::from_millis(5_000);
            let mut elapsed = Duration::ZERO;
            loop {
                renderer.render(&mut pixels, elapsed).unwrap();
                if elapsed >= target {
                    break;
                }
                elapsed = (elapsed + FRAME_DURATION).min(target);
            }
            assert_eq!(frame_hash(&pixels), expected);
        }
    }
}
