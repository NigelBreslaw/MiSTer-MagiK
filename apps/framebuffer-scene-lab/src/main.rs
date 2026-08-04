// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

mod card_flip;
#[cfg(any(all(target_os = "linux", target_arch = "arm"), test))]
mod card_flip_neon;

use card_flip::{CardFlip, Direction as CardFlipDirection, RasterPath as CardFlipRasterPath};
#[cfg(any(target_os = "linux", test))]
use mister_magik_core::input_state::{DirectionalEdges, DirectionalState};
#[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "arm")))]
use mister_magik_framebuffer_scene_lab::LiveParticleRenderer;
use mister_magik_framebuffer_scene_lab::{
    CABINET_LAB_MAX_PARTICLES, DEFAULT_HEIGHT, DEFAULT_WIDTH, EffectKind, FocusedParticleRenderer,
    NavigationFixture, NavigationFixtureScene, read_effect_recipe,
};
use mister_magik_particles::cabinet::{
    CabinetColorMode, CabinetCreativeMode, CabinetRenderOptions, Rgb565Pixel,
};
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(all(feature = "profile", target_os = "linux", target_arch = "arm"))]
mod cpu_profile;

const FRAME_RATE: u64 = 60;
const FRAME_DURATION: Duration = Duration::from_nanos(1_000_000_000 / FRAME_RATE);
#[cfg(all(target_os = "linux", target_arch = "arm"))]
const CARD_FLIP_PROFILE_DURATION: Duration = Duration::from_secs(30);
const CABINET_DEFAULT_PARTICLES: usize = 39_936;
const CABINET_MIN_PARTICLES: usize = 1_024;
const CABINET_PARTICLE_STEP: usize = 1_024;

fn main() -> Result<(), String> {
    let options = Options::parse(env::args().skip(1))?;
    if options.scene == EffectKind::CardFlip {
        let duration = options.card_duration();
        if options.check {
            let mut renderer = CardFlip::new(CardFlipRasterPath::Reference);
            renderer.set_duration(duration);
            println!("framebuffer-scene-lab check passed scene=card-flip");
            return Ok(());
        }
        if let Some(output) = options.output.as_deref() {
            return render_card_flip_headless(
                duration,
                options.direction,
                options
                    .time_ms
                    .expect("option parser requires time with output"),
                output,
            );
        }
        return run_window(SceneSource::CardFlip(duration), None, options.profile);
    }
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
        return run_window(SceneSource::Navigation(fixture), None, false);
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
        options.case,
        options.profile,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CabinetCase {
    name: &'static str,
    particles: usize,
    mode: CabinetCreativeMode,
    color: CabinetColorMode,
}

const CABINET_CASES: [CabinetCase; 27] = [
    CabinetCase {
        name: "baseline-24064",
        particles: 24_064,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "baseline-36096",
        particles: 36_096,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "baseline-48128",
        particles: 48_128,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "baseline-60160",
        particles: 60_160,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "baseline-72192",
        particles: 72_192,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "satellites-48128",
        particles: 48_128,
        mode: CabinetCreativeMode::Satellites,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "satellites-72192",
        particles: 72_192,
        mode: CabinetCreativeMode::Satellites,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "history-48128",
        particles: 48_128,
        mode: CabinetCreativeMode::HistoryEcho,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "history-72192",
        particles: 72_192,
        mode: CabinetCreativeMode::HistoryEcho,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "depth-48128",
        particles: 48_128,
        mode: CabinetCreativeMode::DepthPalette,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "depth-72192",
        particles: 72_192,
        mode: CabinetCreativeMode::DepthPalette,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "jitter-48128",
        particles: 48_128,
        mode: CabinetCreativeMode::MicroJitter,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "jitter-72192",
        particles: 72_192,
        mode: CabinetCreativeMode::MicroJitter,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "all-48128",
        particles: 48_128,
        mode: CabinetCreativeMode::All,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "all-72192",
        particles: 72_192,
        mode: CabinetCreativeMode::All,
        color: CabinetColorMode::Origin,
    },
    CabinetCase {
        name: "prism-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::ScreenPrism,
    },
    CabinetCase {
        name: "aurora-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::DiagonalAurora,
    },
    CabinetCase {
        name: "vortex-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::VortexSpectrum,
    },
    CabinetCase {
        name: "studio-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::StudioLights,
    },
    CabinetCase {
        name: "depth-prism-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::DepthPrism,
    },
    CabinetCase {
        name: "motion-heat-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::MotionHeat,
    },
    CabinetCase {
        name: "directional-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::DirectionalMotion,
    },
    CabinetCase {
        name: "phase-story-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::PhaseStory,
    },
    CabinetCase {
        name: "interference-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::InterferenceBands,
    },
    CabinetCase {
        name: "arcade-palettes-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::ArcadePalettes,
    },
    CabinetCase {
        name: "texture-exact-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::TextureExact,
    },
    CabinetCase {
        name: "texture-glow-39936",
        particles: 39_936,
        mode: CabinetCreativeMode::Baseline,
        color: CabinetColorMode::TextureGlow,
    },
];

fn cabinet_case(name: &str) -> Option<CabinetCase> {
    CABINET_CASES.iter().copied().find(|case| case.name == name)
}

#[derive(Clone, Debug)]
enum SceneSource {
    Particle(PathBuf),
    Navigation(NavigationFixture),
    CardFlip(Duration),
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

fn render_card_flip_headless(
    duration: Duration,
    direction: CardFlipDirection,
    time_ms: u64,
    output: &Path,
) -> Result<(), String> {
    let mut renderer = CardFlip::new(CardFlipRasterPath::Reference);
    renderer.set_duration(duration);
    renderer.start_from_endpoint(direction, Duration::ZERO);
    let mut pixels = vec![Rgb565Pixel(0); DEFAULT_WIDTH * DEFAULT_HEIGHT];
    let stats = renderer
        .render(&mut pixels, Duration::from_millis(time_ms))
        .map_err(str::to_owned)?;
    write_ppm(output, &pixels)?;
    println!(
        "capture={} scene=card-flip direction={} time_ms={} progress_q16={} hash={:016x}",
        output.display(),
        direction.label(),
        time_ms,
        stats.progress_q16,
        frame_hash(&pixels)
    );
    Ok(())
}

#[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "arm")))]
enum LabScene {
    Particle(LiveParticleRenderer),
    Focused(FocusedParticleRenderer),
    Navigation(NavigationFixtureScene),
    CardFlip(Box<CardFlip>),
}

#[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "arm")))]
impl LabScene {
    fn start(
        source: SceneSource,
        case: Option<CabinetCase>,
        width: usize,
        height: usize,
    ) -> Result<Self, String> {
        match source {
            SceneSource::Particle(recipe) => {
                if let Some(case) = case {
                    let selected = read_effect_recipe(&recipe)?;
                    let mut renderer = FocusedParticleRenderer::new(width, height, selected)?;
                    renderer.set_cabinet_render_options(CabinetRenderOptions {
                        active_count: case.particles,
                        creative_mode: case.mode,
                        color_mode: case.color,
                    })?;
                    Ok(Self::Focused(renderer))
                } else {
                    Ok(Self::Particle(LiveParticleRenderer::start(
                        width,
                        height,
                        recipe.clone(),
                        status_path(&recipe),
                    )?))
                }
            }
            SceneSource::Navigation(fixture) => Ok(Self::Navigation(
                NavigationFixtureScene::new_with_geometry(fixture, width, height),
            )),
            SceneSource::CardFlip(duration) => {
                let raster_path = if cfg!(all(target_os = "linux", target_arch = "arm")) {
                    CardFlipRasterPath::Device
                } else {
                    CardFlipRasterPath::Reference
                };
                let mut renderer = CardFlip::new(raster_path);
                renderer.set_duration(duration);
                Ok(Self::CardFlip(Box::new(renderer)))
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
            Self::Focused(renderer) => {
                renderer.render_buffer(destination, buffer_id, elapsed, next_elapsed)
            }
            Self::Navigation(renderer) => renderer.render(destination, elapsed),
            Self::CardFlip(renderer) => renderer
                .render(destination, elapsed)
                .map(card_frame_stats)
                .map_err(str::to_owned),
        }
    }

    fn effect(&self) -> EffectKind {
        match self {
            Self::Particle(renderer) => renderer.effect(),
            Self::Focused(renderer) => renderer.kind(),
            Self::Navigation(_) => EffectKind::NavigationTransition,
            Self::CardFlip(_) => EffectKind::CardFlip,
        }
    }

    #[cfg(target_os = "macos")]
    fn play_card(&mut self, direction: CardFlipDirection, at: Duration) {
        if let Self::CardFlip(renderer) = self {
            renderer.play(direction, at);
        }
    }

    #[cfg(target_os = "macos")]
    fn card_needs_frame(&self) -> bool {
        matches!(self, Self::CardFlip(renderer) if renderer.is_dirty())
    }

    fn set_cabinet_controls(&mut self, controls: &CabinetLabControls) -> Result<(), String> {
        match self {
            Self::Particle(renderer) if renderer.effect() == EffectKind::Cabinet => {
                renderer.set_cabinet_render_options(controls.render_options())
            }
            Self::Focused(_) => Ok(()),
            _ => Ok(()),
        }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::Particle(renderer) => renderer.generation(),
            Self::Focused(_) => 0,
            Self::Navigation(_) => 0,
            Self::CardFlip(_) => 0,
        }
    }

    fn state_label(&self) -> String {
        match self {
            Self::Particle(renderer) => format!("{:?}", renderer.status_state()),
            Self::Focused(_) => "locked-case".into(),
            Self::Navigation(renderer) => format!("fixture:{}", renderer.fixture().label()),
            Self::CardFlip(renderer) => format!(
                "{}:{:05}:{}",
                renderer.direction().label(),
                renderer.progress_q16(),
                if renderer.is_active() {
                    "moving"
                } else {
                    "idle"
                }
            ),
        }
    }

    fn last_error(&self) -> Option<&str> {
        match self {
            Self::Particle(renderer) => renderer.last_error(),
            Self::Focused(_) => None,
            Self::Navigation(_) => None,
            Self::CardFlip(_) => None,
        }
    }
}

fn card_frame_stats(
    stats: card_flip::RenderStats,
) -> mister_magik_framebuffer_scene_lab::FrameStats {
    mister_magik_framebuffer_scene_lab::FrameStats {
        effect: EffectKind::CardFlip,
        particles: 0,
        projected_particles: 0,
        projection_cohorts: 1,
        visible: stats.pixel_writes,
        pixel_writes: stats.pixel_writes,
        simulation_backend: "none",
        projection_backend: if cfg!(all(target_os = "linux", target_arch = "arm")) {
            "armv7-fixed-q16"
        } else {
            "reference-f32"
        },
        magik_stages: None,
        cabinet_stages: None,
        intro_stages: None,
        cue_id: "card-flip",
        cue_index: 0,
        cue_start_ms: 0,
        previous_cue_start_ms: 0,
        cue_elapsed_ms: 0,
        cue_duration_ms: card_flip::DEFAULT_DURATION.as_millis() as u64,
        total_ms: card_flip::DEFAULT_DURATION.as_millis() as u64,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LabAction {
    PreviousMode,
    NextMode,
    IncreaseParticles,
    DecreaseParticles,
}

#[cfg(any(all(target_os = "linux", target_arch = "arm"), test))]
#[derive(Default)]
struct CardFlipLabControls {
    previous_a: bool,
    previous_b: bool,
}

#[cfg(any(all(target_os = "linux", target_arch = "arm"), test))]
impl CardFlipLabControls {
    fn poll(&mut self, button_a: bool, button_b: bool) -> Option<CardFlipDirection> {
        let forward = button_a && !self.previous_a;
        let reverse = button_b && !self.previous_b;
        self.previous_a = button_a;
        self.previous_b = button_b;
        match (forward, reverse) {
            (true, false) => Some(CardFlipDirection::Forward),
            (false, true) => Some(CardFlipDirection::Reverse),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum CabinetDemoMode {
    #[default]
    Baseline,
    Satellites,
    HistoryEcho,
    DepthPalette,
    MicroJitter,
    All,
    ScreenPrism,
    DiagonalAurora,
    VortexSpectrum,
    StudioLights,
    DepthPrism,
    MotionHeat,
    DirectionalMotion,
    PhaseStory,
    InterferenceBands,
    ArcadePalettes,
    TextureExact,
    TextureGlow,
}

impl CabinetDemoMode {
    const ALL: [Self; 18] = [
        Self::Baseline,
        Self::Satellites,
        Self::HistoryEcho,
        Self::DepthPalette,
        Self::MicroJitter,
        Self::All,
        Self::ScreenPrism,
        Self::DiagonalAurora,
        Self::VortexSpectrum,
        Self::StudioLights,
        Self::DepthPrism,
        Self::MotionHeat,
        Self::DirectionalMotion,
        Self::PhaseStory,
        Self::InterferenceBands,
        Self::ArcadePalettes,
        Self::TextureExact,
        Self::TextureGlow,
    ];

    const fn index(self) -> usize {
        self as usize
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Baseline => "BASELINE",
            Self::Satellites => "SATELLITES",
            Self::HistoryEcho => "HISTORY ECHO",
            Self::DepthPalette => "DEPTH PALETTE",
            Self::MicroJitter => "MICRO-JITTER",
            Self::All => "ALL",
            Self::ScreenPrism => "SCREEN PRISM",
            Self::DiagonalAurora => "DIAGONAL AURORA",
            Self::VortexSpectrum => "VORTEX SPECTRUM",
            Self::StudioLights => "STUDIO LIGHTS",
            Self::DepthPrism => "DEPTH PRISM",
            Self::MotionHeat => "MOTION HEAT",
            Self::DirectionalMotion => "DIRECTIONAL COLOUR",
            Self::PhaseStory => "PHASE STORY",
            Self::InterferenceBands => "INTERFERENCE BANDS",
            Self::ArcadePalettes => "ARCADE PALETTES",
            Self::TextureExact => "TEXTURE EXACT",
            Self::TextureGlow => "TEXTURE GLOW",
        }
    }

    const fn render_options(self, active_count: usize) -> CabinetRenderOptions {
        let creative_mode = match self {
            Self::Satellites => CabinetCreativeMode::Satellites,
            Self::HistoryEcho => CabinetCreativeMode::HistoryEcho,
            Self::DepthPalette => CabinetCreativeMode::DepthPalette,
            Self::MicroJitter => CabinetCreativeMode::MicroJitter,
            Self::All => CabinetCreativeMode::All,
            Self::Baseline
            | Self::ScreenPrism
            | Self::DiagonalAurora
            | Self::VortexSpectrum
            | Self::StudioLights
            | Self::DepthPrism
            | Self::MotionHeat
            | Self::DirectionalMotion
            | Self::PhaseStory
            | Self::InterferenceBands
            | Self::ArcadePalettes
            | Self::TextureExact
            | Self::TextureGlow => CabinetCreativeMode::Baseline,
        };
        let color_mode = match self {
            Self::ScreenPrism => CabinetColorMode::ScreenPrism,
            Self::DiagonalAurora => CabinetColorMode::DiagonalAurora,
            Self::VortexSpectrum => CabinetColorMode::VortexSpectrum,
            Self::StudioLights => CabinetColorMode::StudioLights,
            Self::DepthPrism => CabinetColorMode::DepthPrism,
            Self::MotionHeat => CabinetColorMode::MotionHeat,
            Self::DirectionalMotion => CabinetColorMode::DirectionalMotion,
            Self::PhaseStory => CabinetColorMode::PhaseStory,
            Self::InterferenceBands => CabinetColorMode::InterferenceBands,
            Self::ArcadePalettes => CabinetColorMode::ArcadePalettes,
            Self::TextureExact => CabinetColorMode::TextureExact,
            Self::TextureGlow => CabinetColorMode::TextureGlow,
            _ => CabinetColorMode::Origin,
        };
        CabinetRenderOptions {
            active_count,
            creative_mode,
            color_mode,
        }
    }
}

struct CabinetLabControls {
    mode: CabinetDemoMode,
    particles: usize,
    #[cfg(any(target_os = "linux", test))]
    previous_direction: DirectionalState,
    hud: CabinetHud,
}

impl CabinetLabControls {
    fn new() -> Self {
        let mode = CabinetDemoMode::Baseline;
        let particles = CABINET_DEFAULT_PARTICLES;
        Self {
            mode,
            particles,
            #[cfg(any(target_os = "linux", test))]
            previous_direction: DirectionalState::default(),
            hud: CabinetHud::new(mode, particles),
        }
    }

    const fn render_options(&self) -> CabinetRenderOptions {
        self.mode.render_options(self.particles)
    }

    #[cfg(any(target_os = "linux", test))]
    fn poll_direction(&mut self, direction: DirectionalState) {
        let edges = DirectionalEdges::rising(direction, self.previous_direction);
        self.previous_direction = direction;
        if edges.left != edges.right {
            self.apply(if edges.left {
                LabAction::PreviousMode
            } else {
                LabAction::NextMode
            });
        }
        if edges.up != edges.down {
            self.apply(if edges.up {
                LabAction::IncreaseParticles
            } else {
                LabAction::DecreaseParticles
            });
        }
    }

    fn apply(&mut self, action: LabAction) {
        match action {
            LabAction::PreviousMode => {
                let index = self.mode.index();
                self.mode = CabinetDemoMode::ALL
                    [(index + CabinetDemoMode::ALL.len() - 1) % CabinetDemoMode::ALL.len()];
            }
            LabAction::NextMode => {
                self.mode =
                    CabinetDemoMode::ALL[(self.mode.index() + 1) % CabinetDemoMode::ALL.len()];
            }
            LabAction::IncreaseParticles => {
                self.particles = self
                    .particles
                    .saturating_add(CABINET_PARTICLE_STEP)
                    .min(CABINET_LAB_MAX_PARTICLES);
            }
            LabAction::DecreaseParticles => {
                self.particles = self
                    .particles
                    .saturating_sub(CABINET_PARTICLE_STEP)
                    .max(CABINET_MIN_PARTICLES);
            }
        }
        self.hud.update(self.mode, self.particles);
    }

    fn draw_hud(&self, pixels: &mut [Rgb565Pixel], width: usize, height: usize) {
        self.hud.draw(pixels, width, height);
    }
}

const HUD_X: usize = 8;
const HUD_Y: usize = 8;
const HUD_WIDTH: usize = 420;
const HUD_HEIGHT: usize = 42;

struct CabinetHud {
    pixels: Vec<Rgb565Pixel>,
}

impl CabinetHud {
    fn new(mode: CabinetDemoMode, particles: usize) -> Self {
        let mut hud = Self {
            pixels: vec![Rgb565Pixel(0); HUD_WIDTH * HUD_HEIGHT],
        };
        hud.update(mode, particles);
        hud
    }

    fn update(&mut self, mode: CabinetDemoMode, particles: usize) {
        self.pixels.fill(Rgb565Pixel(0));
        let mode_line = format!(
            "MODE {}/{}  {}",
            mode.index() + 1,
            CabinetDemoMode::ALL.len(),
            mode.label()
        );
        let count_line = format!("PARTICLES {},{:03}", particles / 1_000, particles % 1_000);
        draw_hud_text(&mut self.pixels, 6, 5, &mode_line, Rgb565Pixel(0xffa0));
        draw_hud_text(&mut self.pixels, 6, 23, &count_line, Rgb565Pixel(0x07ff));
    }

    fn draw(&self, destination: &mut [Rgb565Pixel], width: usize, height: usize) {
        if destination.len() != width.saturating_mul(height) || width <= HUD_X || height <= HUD_Y {
            return;
        }
        let copy_width = HUD_WIDTH.min(width - HUD_X);
        let copy_height = HUD_HEIGHT.min(height - HUD_Y);
        for row in 0..copy_height {
            let source = &self.pixels[row * HUD_WIDTH..row * HUD_WIDTH + copy_width];
            let offset = (HUD_Y + row) * width + HUD_X;
            destination[offset..offset + copy_width].copy_from_slice(source);
        }
    }
}

fn draw_hud_text(
    destination: &mut [Rgb565Pixel],
    origin_x: usize,
    origin_y: usize,
    text: &str,
    color: Rgb565Pixel,
) {
    for (character_index, character) in text.chars().enumerate() {
        let rows = hud_glyph(character);
        let x = origin_x + character_index * 12;
        if x + 9 >= HUD_WIDTH {
            break;
        }
        for (row, bits) in rows.into_iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) == 0 {
                    continue;
                }
                for scale_y in 0..2 {
                    for scale_x in 0..2 {
                        let pixel_x = x + column * 2 + scale_x;
                        let pixel_y = origin_y + row * 2 + scale_y;
                        if pixel_x < HUD_WIDTH && pixel_y < HUD_HEIGHT {
                            destination[pixel_y * HUD_WIDTH + pixel_x] = color;
                        }
                    }
                }
            }
        }
    }
}

#[allow(clippy::unreadable_literal)]
const fn hud_glyph(character: char) -> [u8; 7] {
    match character {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        ',' => [0, 0, 0, 0, 0, 0b00100, 0b01000],
        _ => [0; 7],
    }
}

#[cfg(target_os = "macos")]
fn run_window(
    source: SceneSource,
    case: Option<CabinetCase>,
    _profile: bool,
) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::new()
        .map_err(|error| format!("create framebuffer-scene-lab event loop: {error}"))?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let mut application = macos::ParticleLabApplication::new(source, case)?;
    event_loop
        .run_app(&mut application)
        .map_err(|error| format!("run framebuffer-scene-lab window: {error}"))
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn run_window(source: SceneSource, case: Option<CabinetCase>, profile: bool) -> Result<(), String> {
    use mister_magik_mister_runtime::display_plan::detect_runtime_display_plan;
    use mister_magik_mister_runtime::fpga::Fpga;
    use mister_magik_mister_runtime::framebuffer::damage::{DirtyRect, DirtyRectList};
    use mister_magik_mister_runtime::framebuffer::hidden_latch::CachedHiddenLatchPresenter;
    use mister_magik_mister_runtime::framebuffer::rgb565::Rgb565;
    use mister_magik_mister_runtime::lab_input::FramebufferLabInput;
    use std::time::Instant;

    let mut fpga = Fpga::open().map_err(|error| format!("open FPGA display detector: {error}"))?;
    let runtime = detect_runtime_display_plan(&mut fpga)
        .map_err(|error| format!("resolve Main display plan: {error}"))?;
    drop(fpga);
    let plan = runtime.plan;
    if let SceneSource::CardFlip(duration) = &source {
        return run_card_flip_mister(*duration, plan, profile);
    }
    let mut renderer = LabScene::start(source, case, plan.render_w, plan.render_h)?;
    let mut controls =
        (case.is_none() && renderer.effect() == EffectKind::Cabinet).then(CabinetLabControls::new);
    let mut input = FramebufferLabInput::open();
    let mut presenter = CachedHiddenLatchPresenter::open(plan)
        .map_err(|error| format!("open plan-aware hidden RGB565 presenter: {error}"))?;
    let mut render_slots = [
        vec![Rgb565Pixel(0); plan.render_w * plan.render_h],
        vec![Rgb565Pixel(0); plan.render_w * plan.render_h],
    ];
    let full_damage = DirtyRectList::from_one(DirtyRect {
        x0: 0,
        y0: 0,
        x1: plan.render_w,
        y1: plan.render_h,
    });
    println!(
        "framebuffer-scene-lab render={}x{} framebuffer={}x{} scan={}x{} output={}x{} route={} format=rgb565",
        plan.render_w,
        plan.render_h,
        plan.fb_w,
        plan.fb_h,
        plan.scan_w,
        plan.scan_h,
        plan.output_w,
        plan.output_h,
        plan.output_route.label(),
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
    let mut ordering_samples_us = Vec::with_capacity(64);
    let mut raster_samples_us = Vec::with_capacity(64);
    let mut worker_wait_samples_us = Vec::with_capacity(64);
    let mut prepared_age_samples_us = Vec::with_capacity(64);
    let mut last_sequence = None;
    let mut repeated_presentations = 0_u64;
    let mut latch_drop_count = 0_u16;
    #[cfg(feature = "profile")]
    let profiler = profile.then(cpu_profile::start).transpose()?;
    #[cfg(not(feature = "profile"))]
    if profile {
        return Err("cabinet profiling requires a release-device-profile build".into());
    }
    loop {
        if let Some(receipt) = presenter
            .settle_pending()
            .map_err(|error| format!("settle hidden RGB565 startup particle frame: {error}"))?
        {
            if last_sequence.is_some_and(|sequence| receipt.sequence <= sequence) {
                repeated_presentations = repeated_presentations.saturating_add(1);
            }
            last_sequence = Some(receipt.sequence);
            latch_drop_count = receipt.drop_count;
        }
        let wall_elapsed = Instant::now().saturating_duration_since(started);
        let elapsed = if case.is_some() {
            Duration::from_nanos((1_000_000_000 / FRAME_RATE).saturating_mul(status_frames))
        } else {
            wall_elapsed
        };
        if let Some(controls) = controls.as_mut() {
            controls.poll_direction(input.poll());
            renderer.set_cabinet_controls(controls)?;
        }
        let writable_slot = presenter.writable_slot_index();
        let pixels = &mut render_slots[usize::from(writable_slot - 1)];
        let render_started = Instant::now();
        let stats = renderer.render_buffer(
            pixels,
            writable_slot - 1,
            elapsed,
            Some(elapsed.saturating_add(FRAME_DURATION)),
        )?;
        if let Some(controls) = controls.as_ref() {
            controls.draw_hud(pixels, plan.render_w, plan.render_h);
        }
        render_samples_us.push(render_started.elapsed().as_micros() as u64);
        if let Some(stages) = stats.magik_stages {
            clear_samples_us.push(stages.clear_us);
            simulation_samples_us.push(stages.simulation_us);
            projection_samples_us.push(stages.projection_us);
            ordering_samples_us.push(0);
            raster_samples_us.push(stages.raster_us);
            worker_wait_samples_us.push(0);
            prepared_age_samples_us.push(0);
        } else if let Some(stages) = stats.cabinet_stages {
            clear_samples_us.push(stages.clear_us);
            simulation_samples_us.push(0);
            projection_samples_us.push(stages.projection_us);
            ordering_samples_us.push(stages.ordering_us);
            raster_samples_us.push(stages.raster_us);
            worker_wait_samples_us.push(stages.worker_wait_us);
            prepared_age_samples_us.push(stages.prepared_age_us);
        } else if let Some(stages) = stats.intro_stages {
            clear_samples_us.push(stages.clear_us);
            simulation_samples_us.push(stages.transform_us);
            projection_samples_us.push(stages.projection_us);
            ordering_samples_us.push(0);
            raster_samples_us.push(stages.raster_us);
            worker_wait_samples_us.push(0);
            prepared_age_samples_us.push(0);
        } else {
            clear_samples_us.clear();
            simulation_samples_us.clear();
            projection_samples_us.clear();
            ordering_samples_us.clear();
            raster_samples_us.clear();
            worker_wait_samples_us.clear();
            prepared_age_samples_us.clear();
        }
        // SAFETY: both pixel types are repr(transparent) wrappers around u16,
        // have identical length/alignment, and accept every u16 bit pattern.
        let cached =
            unsafe { std::slice::from_raw_parts(pixels.as_ptr().cast::<Rgb565>(), pixels.len()) };
        presenter
            .prepare_cached(cached, &full_damage)
            .map_err(|error| format!("copy cached startup particle frame: {error}"))?;
        let post = presenter
            .post_prepared()
            .map_err(|error| format!("post hidden RGB565 startup particle frame: {error}"))?;
        status_frames = status_frames.saturating_add(1);
        if let Some(case) = case
            && started.elapsed() >= Duration::from_secs(60)
        {
            #[cfg(feature = "profile")]
            if let Some(profiler) = profiler {
                cpu_profile::finish(profiler)?;
            }
            let seconds = started.elapsed().as_secs_f64();
            let cpu_now = process_cpu_time();
            let cpu_percent = cpu_now.saturating_sub(cpu_started).as_secs_f64() / seconds * 100.0;
            let (render_average_us, render_p99_us, render_max_us) =
                sample_summary(&mut render_samples_us);
            let (clear_average_us, clear_p99_us, _) = sample_summary(&mut clear_samples_us);
            let (projection_average_us, projection_p99_us, _) =
                sample_summary(&mut projection_samples_us);
            let (ordering_average_us, ordering_p99_us, _) =
                sample_summary(&mut ordering_samples_us);
            let (raster_average_us, raster_p99_us, _) = sample_summary(&mut raster_samples_us);
            let (worker_wait_average_us, worker_wait_p99_us, _) =
                sample_summary(&mut worker_wait_samples_us);
            let (prepared_age_average_us, prepared_age_p99_us, _) =
                sample_summary(&mut prepared_age_samples_us);
            println!(
                "cabinet-case name={} particles={} projected_particles={} projection_cohorts={} visible={} pixel_writes={} mode={} seconds={:.3} frames={} fps={:.3} cpu_pct={:.2} render_avg_us={} render_p99_us={} render_max_us={} clear_avg_us={} clear_p99_us={} projection_avg_us={} projection_p99_us={} ordering_avg_us={} ordering_p99_us={} raster_avg_us={} raster_p99_us={} worker_wait_avg_us={} worker_wait_p99_us={} prepared_age_avg_us={} prepared_age_p99_us={} repeated_presentations={} projection_backend={}",
                case.name,
                case.particles,
                stats.projected_particles,
                stats.projection_cohorts,
                stats.visible,
                stats.pixel_writes,
                case.mode.label(),
                seconds,
                status_frames,
                status_frames as f64 / seconds,
                cpu_percent,
                render_average_us,
                render_p99_us,
                render_max_us,
                clear_average_us,
                clear_p99_us,
                projection_average_us,
                projection_p99_us,
                ordering_average_us,
                ordering_p99_us,
                raster_average_us,
                raster_p99_us,
                worker_wait_average_us,
                worker_wait_p99_us,
                prepared_age_average_us,
                prepared_age_p99_us,
                repeated_presentations,
                stats.projection_backend,
            );
            return Ok(());
        }
        if case.is_none() && status_started.elapsed() >= Duration::from_secs(1) {
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
            let stage_metrics = if clear_samples_us.is_empty() {
                "scene_stages=not_applicable".to_owned()
            } else {
                format!(
                    "scene_clear_avg_us={clear_average_us} scene_clear_p99_us={clear_p99_us} scene_simulation_avg_us={simulation_average_us} scene_simulation_p99_us={simulation_p99_us} scene_projection_avg_us={projection_average_us} scene_projection_p99_us={projection_p99_us} scene_raster_avg_us={raster_average_us} scene_raster_p99_us={raster_p99_us}"
                )
            };
            println!(
                "framebuffer-scene-lab effect={} generation={} state={} cue={} cue_elapsed_ms={} total_ms={} particles={} fps={:.1} cpu_pct={:.1} render_avg_us={} render_p99_us={} render_max_us={} {} visible={} simulation_backend={} projection_backend={} slot={} sequence={} repeated_presentations={} latch_drop_count={} latch_status_reads={} latch_poll_reads={} latch_settle_us={} reload_error={}",
                stats.effect.label(),
                renderer.generation(),
                renderer.state_label(),
                stats.cue_id,
                stats.cue_elapsed_ms,
                stats.total_ms,
                stats.particles,
                status_frames as f64 / seconds,
                cpu_percent,
                render_average_us,
                render_p99_us,
                render_max_us,
                stage_metrics,
                stats.visible,
                stats.simulation_backend,
                stats.projection_backend,
                post.slot_index,
                post.sequence,
                repeated_presentations,
                latch_drop_count,
                presenter.pipeline_stats().status_reads,
                presenter.pipeline_stats().poll_reads,
                presenter.pipeline_stats().settle_us,
                error,
            );
            status_started = Instant::now();
            cpu_started = cpu_now;
            status_frames = 0;
            render_samples_us.clear();
            clear_samples_us.clear();
            simulation_samples_us.clear();
            projection_samples_us.clear();
            ordering_samples_us.clear();
            raster_samples_us.clear();
            worker_wait_samples_us.clear();
            prepared_age_samples_us.clear();
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
fn run_card_flip_mister(
    duration: Duration,
    plan: mister_magik_core::display::ResolvedDisplayPlan,
    profile: bool,
) -> Result<(), String> {
    use mister_magik_mister_runtime::framebuffer::damage::{DirtyRect, DirtyRectList};
    use mister_magik_mister_runtime::framebuffer::hidden_latch::CachedHiddenLatchPresenter;
    use mister_magik_mister_runtime::framebuffer::rgb565::Rgb565;
    use mister_magik_mister_runtime::lab_input::FramebufferLabInput;
    use std::time::Instant;

    let mut presenter = CachedHiddenLatchPresenter::open(plan)
        .map_err(|error| format!("open plan-aware hidden RGB565 card presenter: {error}"))?;

    let mut renderer = CardFlip::new(CardFlipRasterPath::Device);
    renderer.set_duration(duration);
    let mut reference = vec![Rgb565Pixel(0); DEFAULT_WIDTH * DEFAULT_HEIGHT];
    let mut staging = vec![Rgb565Pixel(0); plan.render_w * plan.render_h];
    let mut staging_initialized = false;
    let card_rect = scaled_card_rect(plan.render_w, plan.render_h);
    let card_damage = DirtyRectList::from_one(DirtyRect {
        x0: card_rect.0,
        y0: card_rect.1,
        x1: card_rect.0 + card_rect.2,
        y1: card_rect.1 + card_rect.3,
    });
    card_flip_neon::fill_rgb565(&mut reference, Rgb565Pixel(0));
    let mut controls = CardFlipLabControls::default();
    let mut input = FramebufferLabInput::open();
    let started = Instant::now();
    let mut next_frame = started;
    let mut status_started = started;
    let mut cpu_started = process_cpu_time();
    let mut rendered_frames = 0_u64;
    let mut render_samples_us = Vec::with_capacity(64);
    let mut transfer_samples_us = Vec::with_capacity(64);
    let mut last_sequence = None;
    let mut repeated_presentations = 0_u64;
    let mut latch_drop_count = 0_u16;
    let mut profile_render_samples_us = Vec::with_capacity(1024);
    let mut profile_transfer_samples_us = Vec::with_capacity(1024);
    let profile_cpu_started = process_cpu_time();
    let mut profile_frames = 0_u64;
    let mut profile_repeated_presentations = 0_u64;
    let mut automatic_direction = CardFlipDirection::Reverse;
    let mut next_automatic_flip = duration;
    if profile {
        renderer.play(CardFlipDirection::Forward, Duration::ZERO);
    }
    #[cfg(feature = "profile")]
    let profiler = profile.then(cpu_profile::start).transpose()?;
    #[cfg(not(feature = "profile"))]
    if profile {
        return Err("card flip profiling requires a release-device-profile build".into());
    }

    println!(
        "framebuffer-scene-lab scene=card-flip render={}x{} framebuffer={}x{} scan={}x{} output={}x{} route={} card={}x{} format=rgb565 raster=armv7-fixed-q16 transfer=dirty-rect",
        plan.render_w,
        plan.render_h,
        plan.fb_w,
        plan.fb_h,
        plan.scan_w,
        plan.scan_h,
        plan.output_w,
        plan.output_h,
        plan.output_route.label(),
        card_rect.2,
        card_rect.3,
    );

    loop {
        if let Some(receipt) = presenter
            .settle_pending()
            .map_err(|error| format!("settle hidden RGB565 card frame: {error}"))?
        {
            if last_sequence.is_some_and(|sequence| receipt.sequence <= sequence) {
                repeated_presentations = repeated_presentations.saturating_add(1);
            }
            last_sequence = Some(receipt.sequence);
            latch_drop_count = receipt.drop_count;
        }

        let elapsed = started.elapsed();
        let state = input.poll_state();
        if let Some(direction) = controls.poll(state.button_a, state.button_b) {
            renderer.play(direction, elapsed);
            next_frame = Instant::now();
        }
        if profile && elapsed >= next_automatic_flip {
            renderer.play(automatic_direction, elapsed);
            automatic_direction = match automatic_direction {
                CardFlipDirection::Forward => CardFlipDirection::Reverse,
                CardFlipDirection::Reverse => CardFlipDirection::Forward,
            };
            next_automatic_flip = elapsed + duration;
            next_frame = Instant::now();
        }

        let now = Instant::now();
        if renderer.is_dirty() && now >= next_frame {
            let render_started = Instant::now();
            let stats = renderer
                .render(&mut reference, elapsed)
                .map_err(str::to_owned)?;
            let render_us = render_started.elapsed().as_micros() as u64;
            if stats.changed {
                let transfer_started = Instant::now();
                if !staging_initialized {
                    staging.fill(reference[0]);
                    staging_initialized = true;
                }
                scale_card_frame(
                    &reference,
                    &mut staging,
                    plan.render_w,
                    plan.render_h,
                    card_rect,
                );
                // SAFETY: both RGB565 wrappers are transparent over u16 and
                // every u16 bit pattern is valid for either representation.
                let cached = unsafe {
                    std::slice::from_raw_parts(staging.as_ptr().cast::<Rgb565>(), staging.len())
                };
                let copy = presenter
                    .prepare_cached(cached, &card_damage)
                    .map_err(|error| format!("copy cached card frame: {error}"))?;
                let transfer_us = transfer_started.elapsed().as_micros() as u64;
                let post = presenter
                    .post_prepared()
                    .map_err(|error| format!("post hidden RGB565 card frame: {error}"))?;
                debug_assert_eq!(post.slot_index, copy.slot_index);
                rendered_frames = rendered_frames.saturating_add(1);
                render_samples_us.push(render_us);
                transfer_samples_us.push(transfer_us);
                profile_render_samples_us.push(render_us);
                profile_transfer_samples_us.push(transfer_us);
                profile_frames = profile_frames.saturating_add(1);
            }
            next_frame += FRAME_DURATION;
            if next_frame <= Instant::now() {
                next_frame = Instant::now();
            }
        }

        if status_started.elapsed() >= Duration::from_secs(1) {
            let seconds = status_started.elapsed().as_secs_f64();
            let cpu_now = process_cpu_time();
            let cpu_percent = cpu_now.saturating_sub(cpu_started).as_secs_f64() / seconds * 100.0;
            let (render_average_us, render_p99_us, render_max_us) =
                sample_summary(&mut render_samples_us);
            let (transfer_average_us, transfer_p99_us, transfer_max_us) =
                sample_summary(&mut transfer_samples_us);
            println!(
                "card-flip frames={} fps={:.1} cpu_pct={:.1} active={} progress_q16={} direction={} render_avg_us={} render_p99_us={} render_max_us={} transfer_avg_us={} transfer_p99_us={} transfer_max_us={} repeated_presentations={} latch_drop_count={}",
                rendered_frames,
                rendered_frames as f64 / seconds,
                cpu_percent,
                renderer.is_active(),
                renderer.progress_q16(),
                renderer.direction().label(),
                render_average_us,
                render_p99_us,
                render_max_us,
                transfer_average_us,
                transfer_p99_us,
                transfer_max_us,
                repeated_presentations,
                latch_drop_count,
            );
            status_started = Instant::now();
            cpu_started = cpu_now;
            rendered_frames = 0;
            render_samples_us.clear();
            transfer_samples_us.clear();
            profile_repeated_presentations =
                profile_repeated_presentations.saturating_add(repeated_presentations);
            repeated_presentations = 0;
        }

        if profile && started.elapsed() >= CARD_FLIP_PROFILE_DURATION {
            let seconds = started.elapsed().as_secs_f64();
            let cpu_percent = process_cpu_time()
                .saturating_sub(profile_cpu_started)
                .as_secs_f64()
                / seconds
                * 100.0;
            let (render_average_us, render_p99_us, render_max_us) =
                sample_summary(&mut profile_render_samples_us);
            let (transfer_average_us, transfer_p99_us, transfer_max_us) =
                sample_summary(&mut profile_transfer_samples_us);
            println!(
                "card-flip-profile seconds={:.3} frames={} fps={:.3} cpu_pct={:.2} render_avg_us={} render_p99_us={} render_max_us={} transfer_avg_us={} transfer_p99_us={} transfer_max_us={} repeated_presentations={} latch_drop_count={}",
                seconds,
                profile_frames,
                profile_frames as f64 / seconds,
                cpu_percent,
                render_average_us,
                render_p99_us,
                render_max_us,
                transfer_average_us,
                transfer_p99_us,
                transfer_max_us,
                profile_repeated_presentations,
                latch_drop_count,
            );
            #[cfg(feature = "profile")]
            if let Some(profiler) = profiler {
                cpu_profile::finish(profiler)?;
            }
            return Ok(());
        }

        let wait = if renderer.is_dirty() {
            next_frame
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(2))
        } else {
            Duration::from_millis(2)
        };
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
}

#[cfg(any(test, all(target_os = "linux", target_arch = "arm")))]
fn scaled_card_rect(width: usize, height: usize) -> (usize, usize, usize, usize) {
    let card_height = height.saturating_mul(7).div_ceil(10).min(height);
    let card_width = card_height
        .saturating_mul(43)
        .saturating_add(31)
        .saturating_div(63)
        .min(width);
    (
        (width - card_width) / 2,
        (height - card_height) / 2,
        card_width,
        card_height,
    )
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn scale_card_frame(
    reference: &[Rgb565Pixel],
    destination: &mut [Rgb565Pixel],
    destination_width: usize,
    destination_height: usize,
    card_rect: (usize, usize, usize, usize),
) {
    debug_assert_eq!(reference.len(), DEFAULT_WIDTH * DEFAULT_HEIGHT);
    debug_assert_eq!(destination.len(), destination_width * destination_height);
    let (x0, y0, width, height) = card_rect;
    for y in 0..height {
        let source_y = card_flip::CARD_Y + y * card_flip::CARD_HEIGHT / height;
        let source_row = source_y * DEFAULT_WIDTH;
        let destination_row = (y0 + y) * destination_width;
        for x in 0..width {
            let source_x = card_flip::CARD_X + x * card_flip::CARD_WIDTH / width;
            destination[destination_row + x0 + x] = reference[source_row + source_x];
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
fn run_window(
    _source: SceneSource,
    _case: Option<CabinetCase>,
    _profile: bool,
) -> Result<(), String> {
    Err("interactive startup particle preview requires macOS or ARM MiSTer".into())
}

struct Options {
    scene: EffectKind,
    recipe: Option<PathBuf>,
    fixture: Option<NavigationFixture>,
    time_ms: Option<u64>,
    output: Option<PathBuf>,
    check: bool,
    case: Option<CabinetCase>,
    profile: bool,
    duration_ms: Option<u64>,
    direction: CardFlipDirection,
    direction_requested: bool,
}

impl Options {
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut recipe = None;
        let mut fixture = None;
        let mut scene = None;
        let mut time_ms = None;
        let mut output = None;
        let mut check = false;
        let mut case = None;
        let mut profile = false;
        let mut duration_ms = None;
        let mut direction = CardFlipDirection::Forward;
        let mut direction_requested = false;
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--scene" => {
                    let value = arguments.next().ok_or("--scene requires a scene name")?;
                    scene = EffectKind::parse(&value);
                    if scene.is_none() {
                        return Err(format!(
                            "invalid scene {value:?}; expected magik, cabinet, intro, navigation-transition, or card-flip"
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
                "--case" => {
                    let value = arguments.next().ok_or("--case requires a named case")?;
                    case = Some(cabinet_case(&value).ok_or_else(|| {
                        format!(
                            "unknown cabinet case {value:?}; use one of the closed case registry"
                        )
                    })?);
                }
                "--profile" => profile = true,
                "--duration-ms" => {
                    let value = arguments
                        .next()
                        .ok_or("--duration-ms requires milliseconds")?;
                    duration_ms = Some(
                        value
                            .parse::<u64>()
                            .ok()
                            .filter(|value| (100..=10_000).contains(value))
                            .ok_or_else(|| {
                                format!(
                                    "invalid --duration-ms value {value:?}; expected 100..=10000"
                                )
                            })?,
                    );
                }
                "--direction" => {
                    let value = arguments
                        .next()
                        .ok_or("--direction requires forward or reverse")?;
                    direction = match value.as_str() {
                        "forward" => CardFlipDirection::Forward,
                        "reverse" => CardFlipDirection::Reverse,
                        _ => {
                            return Err(format!(
                                "invalid card direction {value:?}; expected forward or reverse"
                            ));
                        }
                    };
                    direction_requested = true;
                }
                "--help" | "-h" => return Err(usage().into()),
                other => return Err(format!("unknown framebuffer-scene-lab argument {other:?}")),
            }
        }
        let options = Self {
            scene: scene.ok_or("--scene is required")?,
            recipe,
            fixture,
            time_ms,
            output,
            check,
            case,
            profile,
            duration_ms,
            direction,
            direction_requested,
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
        if self.case.is_some()
            && (self.scene != EffectKind::Cabinet || self.check || self.output.is_some())
        {
            return Err("--case requires an interactive cabinet device run".into());
        }
        if self.profile && self.case.is_none() && self.scene != EffectKind::CardFlip {
            return Err("--profile requires card-flip or a closed cabinet --case".into());
        }
        if self.scene != EffectKind::CardFlip
            && (self.duration_ms.is_some() || self.direction_requested)
        {
            return Err("--duration-ms and --direction are valid only for card-flip".into());
        }
        match self.scene {
            EffectKind::Magik | EffectKind::Cabinet | EffectKind::Intro => {
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
            EffectKind::CardFlip => {
                if self.recipe.is_some() || self.fixture.is_some() {
                    return Err(
                        "card-flip is self-contained and accepts no recipe or fixture".into(),
                    );
                }
                if self.case.is_some() {
                    return Err("card-flip does not accept cabinet case options".into());
                }
            }
        }
        Ok(())
    }

    fn card_duration(&self) -> Duration {
        Duration::from_millis(self.duration_ms.unwrap_or(440))
    }
}

fn usage() -> &'static str {
    "usage:\n  mister-magik-framebuffer-scene-lab --scene magik|cabinet|intro --recipe FILE.json\n  mister-magik-framebuffer-scene-lab --scene cabinet --recipe FILE.json --case NAME [--profile]\n  mister-magik-framebuffer-scene-lab --scene navigation-transition --fixture home-arcade|home-consoles|consoles-system\n  mister-magik-framebuffer-scene-lab --scene card-flip [--duration-ms N]\n  mister-magik-framebuffer-scene-lab --scene card-flip --direction forward|reverse --time-ms N --output FILE.ppm\n  mister-magik-framebuffer-scene-lab --scene SCENE (--recipe FILE.json|--fixture FIXTURE) --time-ms N --output FILE.ppm\n  mister-magik-framebuffer-scene-lab --scene SCENE --check"
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

    #[derive(Clone, Copy)]
    enum IntroAction {
        Pause,
        ScrubBack,
        ScrubForward,
        PreviousCue,
        NextCue,
        Restart,
        ToggleLoop,
    }

    struct IntroTransport {
        playhead: Duration,
        last_tick: Instant,
        paused: bool,
        looping: bool,
        total: Duration,
    }

    impl IntroTransport {
        fn new(now: Instant) -> Self {
            Self {
                playhead: Duration::ZERO,
                last_tick: now,
                paused: false,
                looping: true,
                total: Duration::from_secs(20),
            }
        }

        fn tick(&mut self, now: Instant) -> Duration {
            if !self.paused {
                self.playhead = self
                    .playhead
                    .saturating_add(now.duration_since(self.last_tick));
            }
            self.last_tick = now;
            if self.looping && !self.total.is_zero() {
                self.playhead = Duration::from_nanos(
                    self.playhead.as_nanos().rem_euclid(self.total.as_nanos()) as u64,
                );
            } else {
                self.playhead = self.playhead.min(self.total);
            }
            self.playhead
        }

        fn apply(
            &mut self,
            action: IntroAction,
            stats: Option<mister_magik_framebuffer_scene_lab::FrameStats>,
        ) {
            match action {
                IntroAction::Pause => self.paused = !self.paused,
                IntroAction::ScrubBack => {
                    self.playhead = self.playhead.saturating_sub(Duration::from_millis(100));
                }
                IntroAction::ScrubForward => {
                    self.playhead = self
                        .playhead
                        .saturating_add(Duration::from_millis(100))
                        .min(self.total);
                }
                IntroAction::PreviousCue => {
                    if let Some(stats) = stats {
                        self.playhead = Duration::from_millis(stats.previous_cue_start_ms);
                    }
                }
                IntroAction::NextCue => {
                    if let Some(stats) = stats {
                        self.playhead = Duration::from_millis(
                            stats.cue_start_ms.saturating_add(stats.cue_duration_ms),
                        )
                        .min(self.total);
                    }
                }
                IntroAction::Restart => self.playhead = Duration::ZERO,
                IntroAction::ToggleLoop => self.looping = !self.looping,
            }
            self.last_tick = Instant::now();
        }
    }

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
        controls: Option<CabinetLabControls>,
        intro_transport: Option<IntroTransport>,
        last_stats: Option<mister_magik_framebuffer_scene_lab::FrameStats>,
    }

    impl ParticleLabApplication {
        pub(super) fn new(source: SceneSource, case: Option<CabinetCase>) -> Result<Self, String> {
            let renderer = LabScene::start(source, case, DEFAULT_WIDTH, DEFAULT_HEIGHT)?;
            let controls = (case.is_none() && renderer.effect() == EffectKind::Cabinet)
                .then(CabinetLabControls::new);
            let now = Instant::now();
            let intro_transport =
                (renderer.effect() == EffectKind::Intro).then(|| IntroTransport::new(now));
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
                controls,
                intro_transport,
                last_stats: None,
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
            let cue = self.last_stats.as_ref().map_or_else(String::new, |stats| {
                if stats.effect == EffectKind::Intro {
                    format!(
                        " — {} {:05}/{:05}ms particles={} cohorts={}",
                        stats.cue_id,
                        stats.cue_elapsed_ms,
                        stats.total_ms,
                        stats.particles,
                        stats.projection_cohorts
                    )
                } else {
                    String::new()
                }
            });
            format!(
                "MiSTer MagiK Framebuffer Scenes — {} — generation {} {} — {:.1} fps{cue}{error}",
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
            let now = Instant::now();
            let elapsed = self
                .intro_transport
                .as_mut()
                .map_or_else(|| self.epoch.elapsed(), |transport| transport.tick(now));
            if let Some(controls) = self.controls.as_ref()
                && let Err(error) = self.renderer.set_cabinet_controls(controls)
            {
                self.render_error = Some(error);
                self.update_title();
                return;
            }
            let stats = match self.renderer.render_buffer(
                &mut self.pixels,
                0,
                elapsed,
                Some(elapsed.saturating_add(FRAME_DURATION)),
            ) {
                Ok(stats) => stats,
                Err(error) => {
                    self.render_error = Some(error);
                    self.update_title();
                    return;
                }
            };
            if let Some(transport) = self.intro_transport.as_mut() {
                transport.total = Duration::from_millis(stats.total_ms);
            }
            self.last_stats = Some(stats);
            if let Some(controls) = self.controls.as_ref() {
                controls.draw_hud(&mut self.pixels, DEFAULT_WIDTH, DEFAULT_HEIGHT);
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
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed && !event.repeat =>
                {
                    let card_direction = match event.physical_key {
                        PhysicalKey::Code(KeyCode::KeyA | KeyCode::Enter) => {
                            Some(CardFlipDirection::Forward)
                        }
                        PhysicalKey::Code(KeyCode::KeyB | KeyCode::Backspace) => {
                            Some(CardFlipDirection::Reverse)
                        }
                        _ => None,
                    };
                    if self.renderer.effect() == EffectKind::CardFlip
                        && let Some(direction) = card_direction
                    {
                        self.renderer.play_card(direction, self.epoch.elapsed());
                        self.next_frame = Instant::now();
                        if let Some(window) = self.window.as_ref() {
                            window.request_redraw();
                        }
                        return;
                    }
                    let intro_action = match event.physical_key {
                        PhysicalKey::Code(KeyCode::Space) => Some(IntroAction::Pause),
                        PhysicalKey::Code(KeyCode::ArrowLeft) => Some(IntroAction::ScrubBack),
                        PhysicalKey::Code(KeyCode::ArrowRight) => Some(IntroAction::ScrubForward),
                        PhysicalKey::Code(KeyCode::ArrowUp) => Some(IntroAction::PreviousCue),
                        PhysicalKey::Code(KeyCode::ArrowDown) => Some(IntroAction::NextCue),
                        PhysicalKey::Code(KeyCode::Home) => Some(IntroAction::Restart),
                        PhysicalKey::Code(KeyCode::KeyL) => Some(IntroAction::ToggleLoop),
                        _ => None,
                    };
                    if let (Some(transport), Some(action)) =
                        (self.intro_transport.as_mut(), intro_action)
                    {
                        transport.apply(action, self.last_stats);
                        if let Some(window) = self.window.as_ref() {
                            window.request_redraw();
                        }
                        return;
                    }
                    let action = match event.physical_key {
                        PhysicalKey::Code(KeyCode::ArrowLeft) => Some(LabAction::PreviousMode),
                        PhysicalKey::Code(KeyCode::ArrowRight) => Some(LabAction::NextMode),
                        PhysicalKey::Code(KeyCode::ArrowUp) => Some(LabAction::IncreaseParticles),
                        PhysicalKey::Code(KeyCode::ArrowDown) => Some(LabAction::DecreaseParticles),
                        _ => None,
                    };
                    if let (Some(controls), Some(action)) = (self.controls.as_mut(), action) {
                        controls.apply(action);
                        if let Some(window) = self.window.as_ref() {
                            window.request_redraw();
                        }
                    }
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
            if self.renderer.effect() == EffectKind::CardFlip && !self.renderer.card_needs_frame() {
                event_loop.set_control_flow(ControlFlow::Wait);
                return;
            }
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

        let case = Options::parse(
            [
                "--scene",
                "cabinet",
                "--recipe",
                "cabinet.json",
                "--case",
                "all-72192",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(case.case, cabinet_case("all-72192"));

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

        let card = Options::parse(
            [
                "--scene",
                "card-flip",
                "--direction",
                "reverse",
                "--duration-ms",
                "600",
                "--time-ms",
                "300",
                "--output",
                "card.ppm",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(card.direction, CardFlipDirection::Reverse);
        assert_eq!(card.card_duration(), Duration::from_millis(600));
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
                    "cabinet",
                    "--recipe",
                    "cabinet.json",
                    "--case",
                    "unknown",
                ]
                .map(String::from)
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
        assert!(
            Options::parse(["--scene", "card-flip", "--recipe", "card.json"].map(String::from))
                .is_err()
        );
        assert!(
            Options::parse(
                [
                    "--scene",
                    "magik",
                    "--recipe",
                    "magik.json",
                    "--direction",
                    "forward",
                ]
                .map(String::from)
            )
            .is_err()
        );
    }

    #[test]
    fn card_controls_use_a_and_b_rising_edges() {
        let mut controls = CardFlipLabControls::default();
        assert_eq!(controls.poll(true, false), Some(CardFlipDirection::Forward));
        assert_eq!(controls.poll(true, false), None);
        assert_eq!(controls.poll(false, false), None);
        assert_eq!(controls.poll(false, true), Some(CardFlipDirection::Reverse));
        assert_eq!(controls.poll(true, true), None);
    }

    #[test]
    fn rgb565_primary_channels_expand_to_rgb888() {
        assert_eq!(rgb565_to_rgb888(Rgb565Pixel(0xf800)), [255, 0, 0]);
        assert_eq!(rgb565_to_rgb888(Rgb565Pixel(0x07e0)), [0, 255, 0]);
        assert_eq!(rgb565_to_rgb888(Rgb565Pixel(0x001f)), [0, 0, 255]);
    }

    #[test]
    fn cabinet_controls_use_rising_edges_exact_steps_and_wrap_modes() {
        let mut controls = CabinetLabControls::new();
        controls.poll_direction(DirectionalState {
            up: true,
            right: true,
            ..DirectionalState::default()
        });
        assert_eq!(controls.particles, 40_960);
        assert_eq!(controls.mode, CabinetDemoMode::Satellites);

        controls.poll_direction(DirectionalState {
            up: true,
            right: true,
            ..DirectionalState::default()
        });
        assert_eq!(controls.particles, 40_960);
        assert_eq!(controls.mode, CabinetDemoMode::Satellites);

        controls.poll_direction(DirectionalState::default());
        controls.apply(LabAction::PreviousMode);
        assert_eq!(controls.mode, CabinetDemoMode::Baseline);
        controls.apply(LabAction::PreviousMode);
        assert_eq!(controls.mode, CabinetDemoMode::TextureGlow);
    }

    #[test]
    fn cabinet_controls_clamp_to_interactive_capacity() {
        let mut controls = CabinetLabControls::new();
        for _ in 0..100 {
            controls.apply(LabAction::IncreaseParticles);
        }
        assert_eq!(controls.particles, CABINET_LAB_MAX_PARTICLES);
        for _ in 0..100 {
            controls.apply(LabAction::DecreaseParticles);
        }
        assert_eq!(controls.particles, CABINET_MIN_PARTICLES);
    }

    #[test]
    fn cabinet_hud_draws_inside_the_top_left_panel() {
        let hud = CabinetHud::new(CabinetDemoMode::MicroJitter, 48_128);
        let mut pixels = vec![Rgb565Pixel(0x1234); DEFAULT_WIDTH * DEFAULT_HEIGHT];
        hud.draw(&mut pixels, DEFAULT_WIDTH, DEFAULT_HEIGHT);
        assert_eq!(pixels[HUD_Y * DEFAULT_WIDTH + HUD_X], Rgb565Pixel(0));
        assert!(pixels.contains(&Rgb565Pixel(0xffa0)));
        assert!(pixels.contains(&Rgb565Pixel(0x07ff)));
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
                0x27f1_8935_5c84_8f09,
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

    #[test]
    fn card_geometry_tracks_render_height_and_preserves_aspect_ratio() {
        assert_eq!(scaled_card_rect(960, 600), (336, 90, 287, 420));
        assert_eq!(scaled_card_rect(640, 480), (205, 72, 229, 336));
    }
}
