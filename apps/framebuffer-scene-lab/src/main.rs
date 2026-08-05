// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[cfg(any(test, all(target_os = "linux", target_arch = "arm")))]
mod card_assessment;
mod card_flip;
mod card_flip_neon;
#[cfg(all(target_os = "linux", target_arch = "arm"))]
mod vsync_observer;

#[cfg(all(target_os = "linux", target_arch = "arm"))]
use card_assessment::{
    CardFrameDetails, FrameEvidence, PresentationTelemetrySnapshot, ScreenshotFrameDetails,
    apply_authoritative_presentation_telemetry, confirmation_sequence_is_contiguous,
    summarize_cadence, summarize_presentation_telemetry, summarize_vsync_observer,
};
use card_flip::{CardFlip, Direction as CardFlipDirection, RasterPath as CardFlipRasterPath};
#[cfg(any(target_os = "linux", test))]
use mister_magik_core::input_state::{DirectionalEdges, DirectionalState};
#[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "arm")))]
use mister_magik_framebuffer_scene_lab::LiveParticleRenderer;
use mister_magik_framebuffer_scene_lab::{
    CABINET_LAB_MAX_PARTICLES, DEFAULT_HEIGHT, DEFAULT_WIDTH, EffectKind, FocusedParticleRenderer,
    NavigationFixture, NavigationFixtureScene, read_effect_recipe,
};
use mister_magik_framebuffer_scenes::SceneGeometry;
use mister_magik_particles::cabinet::{
    CabinetColorMode, CabinetCreativeMode, CabinetRenderOptions, Rgb565Pixel,
};
use mister_magik_screenshot_parade::{
    ScreenshotParade, ScreenshotParadeConfig, ScreenshotParadeReplacementMode,
    ScreenshotParadeStartup, ScreenshotParadeStats, ScreenshotPhaseGeneration,
    ScreenshotSamplingProfile,
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
const CABINET_DEFAULT_PARTICLES: usize = 39_936;
const CABINET_MIN_PARTICLES: usize = 1_024;
const CABINET_PARTICLE_STEP: usize = 1_024;
const DEFAULT_SCREENSHOT_SEED: u64 = 0x4d61_6769_4b54_696c;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BoundedRun {
    duration: Duration,
    warmup: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MeasurementPass {
    Cadence,
    Profile,
}

#[cfg_attr(not(all(target_os = "linux", target_arch = "arm")), allow(dead_code))]
impl MeasurementPass {
    const fn label(self) -> &'static str {
        match self {
            Self::Cadence => "cadence",
            Self::Profile => "profile",
        }
    }

    const fn profiler_enabled(self) -> bool {
        matches!(self, Self::Profile)
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(not(all(target_os = "linux", target_arch = "arm")), allow(dead_code))]
struct MeasurementEvidence {
    pass: MeasurementPass,
    evidence_dir: PathBuf,
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
struct PendingFrameEvidence {
    frame: FrameEvidence,
    frame_started: std::time::Instant,
    frame_cpu_started: Duration,
    post_completed: std::time::Instant,
    pipeline_before:
        mister_magik_mister_runtime::framebuffer::hidden_latch::HiddenLatchPipelineStats,
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn finish_frame_evidence(
    mut pending: PendingFrameEvidence,
    receipt: mister_magik_mister_runtime::framebuffer::hidden_latch::HiddenLatchPresentReceipt,
    pipeline_after: mister_magik_mister_runtime::framebuffer::hidden_latch::HiddenLatchPipelineStats,
    settle_wall_us: u64,
    settle_cpu_us: u64,
    previous: Option<&FrameEvidence>,
) -> FrameEvidence {
    let completed_at = std::time::Instant::now();
    pending.frame.settle_wall_us = settle_wall_us;
    pending.frame.settle_cpu_us = settle_cpu_us;
    pending.frame.post_to_confirm_wall_us = completed_at
        .duration_since(pending.post_completed)
        .as_micros() as u64;
    pending.frame.frame_to_confirm_wall_us = completed_at
        .duration_since(pending.frame_started)
        .as_micros() as u64;
    pending.frame.process_cpu_us = process_cpu_time()
        .saturating_sub(pending.frame_cpu_started)
        .as_micros() as u64;
    pending.frame.completion_monotonic_us = monotonic_time_us();
    pending.frame.completion_interval_us = previous.map_or(0, |frame| {
        pending
            .frame
            .completion_monotonic_us
            .saturating_sub(frame.completion_monotonic_us)
    });
    pending.frame.slot_index = receipt.slot_index;
    pending.frame.sequence = receipt.sequence;
    pending.frame.sequence_delta = previous
        .map(|frame| receipt.sequence.wrapping_sub(frame.sequence))
        .unwrap_or(0);
    pending.frame.flip_count = receipt.flip_count;
    pending.frame.flip_delta = previous
        .map(|frame| receipt.flip_count.wrapping_sub(frame.flip_count))
        .unwrap_or(0);
    pending.frame.post_count = receipt.post_count;
    pending.frame.post_delta = previous
        .map(|frame| receipt.post_count.wrapping_sub(frame.post_count))
        .unwrap_or(0);
    pending.frame.latch_drop_count = receipt.drop_count;
    pending.frame.latch_drop_delta = previous
        .map(|frame| receipt.drop_count.wrapping_sub(frame.latch_drop_count))
        .unwrap_or(0);
    pending.frame.status_reads = pipeline_after
        .status_reads
        .saturating_sub(pending.pipeline_before.status_reads);
    pending.frame.poll_reads = pipeline_after
        .poll_reads
        .saturating_sub(pending.pipeline_before.poll_reads);
    pending.frame
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn write_measurement_evidence(
    assessment: &MeasurementEvidence,
    frames: &[FrameEvidence],
    vsync_events: &[card_assessment::VsyncEvent],
    cadence: &card_assessment::CadenceSummary,
    plan: mister_magik_core::display::ResolvedDisplayPlan,
    scene: &'static str,
    card_geometry: Option<card_flip::CardGeometry>,
    process_cpu_pct: f64,
    rss_samples_kib: &[u64],
) -> Result<(), String> {
    std::fs::create_dir_all(&assessment.evidence_dir).map_err(|error| {
        format!(
            "create scene measurement directory {}: {error}",
            assessment.evidence_dir.display()
        )
    })?;
    let frames_path = assessment.evidence_dir.join("frames.jsonl");
    let mut frame_bytes = Vec::with_capacity(frames.len().saturating_mul(768));
    for frame in frames {
        serde_json::to_writer(&mut frame_bytes, frame).map_err(|error| error.to_string())?;
        frame_bytes.push(b'\n');
    }
    std::fs::write(&frames_path, frame_bytes)
        .map_err(|error| format!("write {}: {error}", frames_path.display()))?;
    let vsync_path = assessment.evidence_dir.join("vsync.jsonl");
    let mut vsync_bytes = Vec::with_capacity(vsync_events.len().saturating_mul(160));
    for event in vsync_events {
        serde_json::to_writer(&mut vsync_bytes, event).map_err(|error| error.to_string())?;
        vsync_bytes.push(b'\n');
    }
    std::fs::write(&vsync_path, vsync_bytes)
        .map_err(|error| format!("write {}: {error}", vsync_path.display()))?;
    let screenshot_frames = frames
        .iter()
        .filter_map(|frame| frame.screenshot.as_ref())
        .collect::<Vec<_>>();
    let screenshot_summary = (!screenshot_frames.is_empty()).then(|| {
        serde_json::json!({
            "samples": screenshot_frames.len(),
            "phase_bank_resident_bytes": screenshot_frames
                .iter()
                .map(|frame| frame.phase_bank_resident_bytes)
                .max()
                .unwrap_or(0),
            "scale_count_start": screenshot_frames.first().map_or(0, |frame| frame.scale_count),
            "scale_count_end": screenshot_frames.last().map_or(0, |frame| frame.scale_count),
            "scale_total_us_start": screenshot_frames.first().map_or(0, |frame| frame.scale_total_us),
            "scale_total_us_end": screenshot_frames.last().map_or(0, |frame| frame.scale_total_us),
            "scale_total_us_delta": screenshot_frames.last().map_or(0, |frame| frame.scale_total_us)
                .saturating_sub(screenshot_frames.first().map_or(0, |frame| frame.scale_total_us)),
            "scale_max_us": screenshot_frames.iter().map(|frame| frame.scale_max_us).max().unwrap_or(0),
            "phase_count_start": screenshot_frames.first().map_or(0, |frame| frame.phase_count),
            "phase_count_end": screenshot_frames.last().map_or(0, |frame| frame.phase_count),
            "phase_total_us_start": screenshot_frames.first().map_or(0, |frame| frame.phase_total_us),
            "phase_total_us_end": screenshot_frames.last().map_or(0, |frame| frame.phase_total_us),
            "phase_total_us_delta": screenshot_frames.last().map_or(0, |frame| frame.phase_total_us)
                .saturating_sub(screenshot_frames.first().map_or(0, |frame| frame.phase_total_us)),
            "phase_max_us": screenshot_frames.iter().map(|frame| frame.phase_max_us).max().unwrap_or(0),
            "max_preparation_queue_depth": screenshot_frames
                .iter()
                .map(|frame| frame.preparation_queue_depth)
                .max()
                .unwrap_or(0),
            "max_raster_held_cards": screenshot_frames
                .iter()
                .map(|frame| frame.raster_held_cards)
                .max()
                .unwrap_or(0),
            "max_raster_moved_cards": screenshot_frames
                .iter()
                .map(|frame| frame.raster_moved_cards)
                .max()
                .unwrap_or(0),
            "raster_hold_layer_mask": screenshot_frames
                .iter()
                .fold(0_u8, |mask, frame| mask | frame.raster_hold_layer_mask),
            "raster_visible_layer_mask": screenshot_frames
                .iter()
                .fold(0_u8, |mask, frame| mask | frame.raster_visible_layer_mask),
            "sixteenth_phase_layer_mask": screenshot_frames
                .iter()
                .fold(0_u8, |mask, frame| mask | frame.sixteenth_phase_layer_mask),
        })
    });
    let summary = serde_json::json!({
        "schema": "mister-magik-scene-lab-measurement-pass-v3",
        "scene": scene,
        "pass": assessment.pass.label(),
        "profiler_enabled": assessment.pass.profiler_enabled(),
        "process_cpu_pct_of_one_core": process_cpu_pct,
        "display": {
            "render_w": plan.render_w,
            "render_h": plan.render_h,
            "framebuffer_w": plan.fb_w,
            "framebuffer_h": plan.fb_h,
            "scan_w": plan.scan_w,
            "scan_h": plan.scan_h,
            "output_w": plan.output_w,
            "output_h": plan.output_h,
            "route": plan.output_route.label(),
        },
        "card": card_geometry.map(|geometry| serde_json::json!({
            "x": geometry.card_x,
            "y": geometry.card_y,
            "width": geometry.card_width,
            "height": geometry.card_height,
        })),
        "rss_kib": {
            "samples": rss_samples_kib.len(),
            "mean": rss_samples_kib.iter().copied().sum::<u64>()
                .checked_div(rss_samples_kib.len() as u64).unwrap_or(0),
            "max": rss_samples_kib.iter().copied().max().unwrap_or(0),
        },
        "cadence": cadence,
        "vsync_observer": summarize_vsync_observer(vsync_events),
        "screenshot": screenshot_summary,
    });
    let summary_path = assessment.evidence_dir.join("summary.json");
    let mut summary_bytes =
        serde_json::to_vec_pretty(&summary).map_err(|error| error.to_string())?;
    summary_bytes.push(b'\n');
    std::fs::write(&summary_path, summary_bytes)
        .map_err(|error| format!("write {}: {error}", summary_path.display()))
}

fn main() -> Result<(), String> {
    let options = Options::parse(env::args().skip(1))?;
    if options.scene == EffectKind::ScreenshotScreensaver {
        let archive = options
            .archive
            .as_deref()
            .expect("screenshot option validation requires an archive");
        if options.check {
            let scene = screenshot_scene(
                archive,
                options.seed,
                options.sampling_profile,
                options.phase_generation,
                options.replacement_mode,
                ScreenshotParadeStartup::Prepared,
                DEFAULT_WIDTH,
                DEFAULT_HEIGHT,
            )?;
            println!(
                "framebuffer-scene-lab check passed scene={} entries={} pack_bytes={} seed=0x{:016x}",
                options.scene.label(),
                scene.asset_count(),
                scene.compressed_bytes(),
                options.seed
            );
            return Ok(());
        }
        if let Some(output) = options.output.as_deref() {
            return render_screenshot_headless(
                archive,
                options.seed,
                options.sampling_profile,
                options.phase_generation,
                options.replacement_mode,
                options
                    .time_ms
                    .expect("option parser requires time with output"),
                output,
            );
        }
        return run_window(
            SceneSource::Screenshot {
                archive: archive.to_path_buf(),
                seed: options.seed,
                sampling_profile: options.sampling_profile,
                phase_generation: options.phase_generation,
                replacement_mode: options.replacement_mode,
            },
            None,
            options.profile,
            options.measurement_evidence(),
            options.bounded_run(),
        );
    }
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
        return run_window(
            SceneSource::CardFlip(duration),
            None,
            options.profile,
            options.measurement_evidence(),
            options.bounded_run(),
        );
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
        return run_window(
            SceneSource::Navigation(fixture),
            None,
            options.profile,
            options.measurement_evidence(),
            options.bounded_run(),
        );
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
        options.measurement_evidence(),
        options.bounded_run(),
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
    Screenshot {
        archive: PathBuf,
        seed: u64,
        sampling_profile: ScreenshotSamplingProfile,
        phase_generation: ScreenshotPhaseGeneration,
        replacement_mode: ScreenshotParadeReplacementMode,
    },
}

fn screenshot_scene(
    archive_path: &Path,
    seed: u64,
    sampling_profile: ScreenshotSamplingProfile,
    phase_generation: ScreenshotPhaseGeneration,
    replacement_mode: ScreenshotParadeReplacementMode,
    startup: ScreenshotParadeStartup,
    width: usize,
    height: usize,
) -> Result<ScreenshotParade, String> {
    let archive = mister_magik_catalog::preview_worker::ResidentPreviewArchive::open(archive_path)?;
    let geometry = SceneGeometry::new(width, height, width).map_err(|error| error.to_string())?;
    ScreenshotParade::new(
        archive,
        ScreenshotParadeConfig {
            geometry,
            seed,
            sampling_profile,
            phase_generation,
            startup,
            replacement_mode,
            worker_start: None,
        },
    )
}

fn render_screenshot_headless(
    archive_path: &Path,
    seed: u64,
    sampling_profile: ScreenshotSamplingProfile,
    phase_generation: ScreenshotPhaseGeneration,
    replacement_mode: ScreenshotParadeReplacementMode,
    time_ms: u64,
    output: &Path,
) -> Result<(), String> {
    let mut renderer = screenshot_scene(
        archive_path,
        seed,
        sampling_profile,
        phase_generation,
        replacement_mode,
        ScreenshotParadeStartup::Prepared,
        DEFAULT_WIDTH,
        DEFAULT_HEIGHT,
    )?;
    let mut pixels = vec![Rgb565Pixel(0); DEFAULT_WIDTH * DEFAULT_HEIGHT];
    let target = Duration::from_millis(time_ms);
    let mut elapsed = Duration::ZERO;
    let stats = loop {
        let stats = renderer.render_at(&mut pixels, elapsed)?;
        if elapsed >= target {
            break stats;
        }
        elapsed = (elapsed + FRAME_DURATION).min(target);
    };
    write_ppm(output, &pixels)?;
    println!(
        "capture={} scene=screenshot-screensaver time_ms={} seed=0x{:016x} cards={} hash={:016x}",
        output.display(),
        time_ms,
        seed,
        stats.cards_drawn,
        frame_hash(&pixels)
    );
    Ok(())
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
    Screenshot(Box<ScreenshotParade>),
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
            SceneSource::Screenshot {
                archive,
                seed,
                sampling_profile,
                phase_generation,
                replacement_mode,
            } => screenshot_scene(
                &archive,
                seed,
                sampling_profile,
                phase_generation,
                replacement_mode,
                match replacement_mode {
                    ScreenshotParadeReplacementMode::Prepare => ScreenshotParadeStartup::Streaming,
                    ScreenshotParadeReplacementMode::Recycle => ScreenshotParadeStartup::Prepared,
                },
                width,
                height,
            )
            .map(Box::new)
            .map(Self::Screenshot),
        }
    }

    fn render_buffer(
        &mut self,
        destination: &mut [Rgb565Pixel],
        buffer_id: u8,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
        presentation_tick: Option<u64>,
    ) -> Result<mister_magik_framebuffer_scene_lab::FrameStats, String> {
        let pixel_count = destination.len();
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
            Self::Screenshot(renderer) => {
                let stats = match presentation_tick {
                    Some(tick) => renderer.render_at_presentation_tick(destination, tick),
                    None => renderer.render_at(destination, elapsed),
                }?;
                Ok(screenshot_frame_stats(stats, pixel_count))
            }
        }
    }

    fn effect(&self) -> EffectKind {
        match self {
            Self::Particle(renderer) => renderer.effect(),
            Self::Focused(renderer) => renderer.kind(),
            Self::Navigation(_) => EffectKind::NavigationTransition,
            Self::CardFlip(_) => EffectKind::CardFlip,
            Self::Screenshot(_) => EffectKind::ScreenshotScreensaver,
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    fn measurement_ready(&self) -> bool {
        match self {
            Self::Screenshot(renderer) => renderer.is_ready(),
            _ => true,
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
            Self::Screenshot(_) => 0,
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
            Self::Screenshot(renderer) => format!(
                "{}:{}",
                if renderer.is_ready() {
                    "ready"
                } else {
                    "loading"
                },
                renderer.active_card_count()
            ),
        }
    }

    fn last_error(&self) -> Option<&str> {
        match self {
            Self::Particle(renderer) => renderer.last_error(),
            Self::Focused(_) => None,
            Self::Navigation(_) => None,
            Self::CardFlip(_) => None,
            Self::Screenshot(_) => None,
        }
    }
}

fn screenshot_frame_stats(
    stats: ScreenshotParadeStats,
    pixel_count: usize,
) -> mister_magik_framebuffer_scene_lab::FrameStats {
    mister_magik_framebuffer_scene_lab::FrameStats {
        effect: EffectKind::ScreenshotScreensaver,
        particles: 0,
        projected_particles: 0,
        projection_cohorts: 5,
        visible: stats.cards_drawn,
        pixel_writes: pixel_count,
        simulation_backend: "elapsed-time-schedule",
        projection_backend: "rgb565-lanczos-parade",
        magik_stages: None,
        cabinet_stages: None,
        intro_stages: None,
        screenshot: Some(mister_magik_framebuffer_scene_lab::ScreenshotFrameStats {
            raster_held_cards: stats.raster_held_cards,
            raster_moved_cards: stats.raster_moved_cards,
            raster_hold_layer_mask: stats.raster_hold_layer_mask,
            raster_visible_layer_mask: stats.raster_visible_layer_mask,
            sixteenth_phase_layer_mask: stats.sixteenth_phase_layer_mask,
            phase_bank_resident_bytes: stats.phase_bank_resident_bytes,
            scale_count: stats.scale_count,
            scale_total_us: stats.scale_total_us.min(u128::from(u64::MAX)) as u64,
            scale_max_us: stats.scale_max_us.min(u128::from(u64::MAX)) as u64,
            phase_count: stats.phase_count,
            phase_total_us: stats.phase_total_us.min(u128::from(u64::MAX)) as u64,
            phase_max_us: stats.phase_max_us.min(u128::from(u64::MAX)) as u64,
            preparation_queue_depth: stats.queue_depth,
        }),
        cue_id: "screenshot-screensaver",
        cue_index: 0,
        cue_start_ms: 0,
        previous_cue_start_ms: 0,
        cue_elapsed_ms: 0,
        cue_duration_ms: 0,
        total_ms: 0,
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
        screenshot: None,
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
        if button_a && button_b {
            return None;
        }
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
    _measurement_evidence: Option<MeasurementEvidence>,
    _bounded: Option<BoundedRun>,
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
fn run_window(
    source: SceneSource,
    case: Option<CabinetCase>,
    profile: bool,
    measurement_evidence: Option<MeasurementEvidence>,
    bounded: Option<BoundedRun>,
) -> Result<(), String> {
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
        return run_card_flip_mister(*duration, plan, profile, measurement_evidence, bounded);
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
    let mut presentation_tick = 0_u64;
    let mut render_samples_us = Vec::with_capacity(64);
    let mut clear_samples_us = Vec::with_capacity(64);
    let mut simulation_samples_us = Vec::with_capacity(64);
    let mut projection_samples_us = Vec::with_capacity(64);
    let mut ordering_samples_us = Vec::with_capacity(64);
    let mut raster_samples_us = Vec::with_capacity(64);
    let mut worker_wait_samples_us = Vec::with_capacity(64);
    let mut prepared_age_samples_us = Vec::with_capacity(64);
    let mut last_sequence = None;
    let mut confirmation_sequence_failures = 0_u64;
    let mut latch_drop_count = 0_u16;
    let mut warmup_last_flip_count = None;
    let mut warmup_unit_flip_streak = 0_u8;
    let mut warmup_started = None;
    let mut measurement_started = None;
    let mut presentation_telemetry_start = None;
    let mut measurement_cpu_started = Duration::ZERO;
    let mut frame_evidence = Vec::with_capacity(bounded.map_or(0, |run| {
        run.duration.as_secs() as usize * FRAME_RATE as usize + 4
    }));
    let mut pending_evidence: Option<PendingFrameEvidence> = None;
    let mut rss_samples_kib = Vec::new();
    let mut next_rss_sample = None;
    let mut vsync_observer = None;
    let profiler_requested = profile
        || measurement_evidence
            .as_ref()
            .is_some_and(|evidence| evidence.pass.profiler_enabled());
    #[cfg(feature = "profile")]
    let mut profiler = None;
    #[cfg(not(feature = "profile"))]
    if profiler_requested {
        return Err("scene profiling requires a release-device-profile build".into());
    }
    loop {
        let settle_started = Instant::now();
        let settle_cpu_started = process_cpu_time();
        let settled = presenter
            .settle_pending()
            .map_err(|error| format!("settle hidden RGB565 startup particle frame: {error}"))?;
        let settle_wall_us = settle_started.elapsed().as_micros() as u64;
        let settle_cpu_us = process_cpu_time()
            .saturating_sub(settle_cpu_started)
            .as_micros() as u64;
        if let Some(receipt) = settled {
            if last_sequence.is_some_and(|sequence| {
                !confirmation_sequence_is_contiguous(sequence, receipt.sequence)
            }) {
                confirmation_sequence_failures = confirmation_sequence_failures.saturating_add(1);
            }
            last_sequence = Some(receipt.sequence);
            latch_drop_count = receipt.drop_count;
            if let Some(pending) = pending_evidence.take() {
                let evidence = finish_frame_evidence(
                    pending,
                    receipt,
                    presenter.pipeline_stats(),
                    settle_wall_us,
                    settle_cpu_us,
                    frame_evidence.last(),
                );
                frame_evidence.push(evidence);
            }
            presentation_tick = presentation_tick.saturating_add(1);
            if let Some(bounded) = bounded
                && measurement_started.is_none()
                && renderer.measurement_ready()
            {
                let flip_delta = warmup_last_flip_count
                    .map(|previous: u16| receipt.flip_count.wrapping_sub(previous))
                    .unwrap_or(0);
                warmup_last_flip_count = Some(receipt.flip_count);
                warmup_unit_flip_streak = if flip_delta == 1 {
                    warmup_unit_flip_streak.saturating_add(1)
                } else {
                    0
                };
                if warmup_unit_flip_streak >= 3 {
                    let ready_at = *warmup_started.get_or_insert_with(Instant::now);
                    if ready_at.elapsed() >= bounded.warmup {
                        let raw_telemetry_start =
                            presenter.presentation_telemetry().map_err(|error| {
                                format!("read start FPGA presentation telemetry: {error}")
                            })?;
                        let telemetry_start = PresentationTelemetrySnapshot {
                            owned_vblank_count: raw_telemetry_start.owned_vblank_count,
                            presented_vblank_count: raw_telemetry_start.presented_vblank_count,
                            repeated_vblank_count: raw_telemetry_start.repeated_vblank_count,
                            ownership_loss_count: raw_telemetry_start.ownership_loss_count,
                            active_sequence: raw_telemetry_start.active_sequence,
                            flags: raw_telemetry_start.flags,
                        };
                        if !telemetry_start.magik_ownership() || telemetry_start.pending() {
                            return Err(
                                "start FPGA presentation telemetry is not owned and settled".into(),
                            );
                        }
                        let measured_at = Instant::now();
                        measurement_started = Some(measured_at);
                        presentation_telemetry_start = Some(telemetry_start);
                        status_started = measured_at;
                        measurement_cpu_started = process_cpu_time();
                        cpu_started = measurement_cpu_started;
                        status_frames = 0;
                        render_samples_us.clear();
                        clear_samples_us.clear();
                        simulation_samples_us.clear();
                        projection_samples_us.clear();
                        ordering_samples_us.clear();
                        raster_samples_us.clear();
                        worker_wait_samples_us.clear();
                        prepared_age_samples_us.clear();
                        confirmation_sequence_failures = 0;
                        frame_evidence.clear();
                        pending_evidence = None;
                        rss_samples_kib.clear();
                        next_rss_sample = Some(measured_at);
                        if measurement_evidence.is_some() {
                            vsync_observer = Some(vsync_observer::VsyncObserver::start()?);
                        }
                        #[cfg(feature = "profile")]
                        if profiler_requested {
                            profiler = Some(cpu_profile::start(renderer.effect().label())?);
                        }
                        println!(
                            "scene-lab-measurement state=started scene={} seconds={} warmup_seconds={}",
                            renderer.effect().label(),
                            bounded.duration.as_secs(),
                            bounded.warmup.as_secs(),
                        );
                    }
                }
            }
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
        let render_cpu_started = process_cpu_time();
        let stats = renderer.render_buffer(
            pixels,
            writable_slot - 1,
            elapsed,
            Some(elapsed.saturating_add(FRAME_DURATION)),
            Some(presentation_tick),
        )?;
        if let Some(controls) = controls.as_ref() {
            controls.draw_hud(pixels, plan.render_w, plan.render_h);
        }
        let render_wall_us = render_started.elapsed().as_micros() as u64;
        let render_cpu_us = process_cpu_time()
            .saturating_sub(render_cpu_started)
            .as_micros() as u64;
        render_samples_us.push(render_wall_us);
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
        let transfer_started = Instant::now();
        let transfer_cpu_started = process_cpu_time();
        let copy = presenter
            .prepare_cached(cached, &full_damage)
            .map_err(|error| format!("copy cached startup particle frame: {error}"))?;
        let transfer_wall_us = transfer_started.elapsed().as_micros() as u64;
        let transfer_cpu_us = process_cpu_time()
            .saturating_sub(transfer_cpu_started)
            .as_micros() as u64;
        let post_started = Instant::now();
        let post_cpu_started = process_cpu_time();
        let post = presenter
            .post_prepared()
            .map_err(|error| format!("post hidden RGB565 startup particle frame: {error}"))?;
        let post_wall_us = post_started.elapsed().as_micros() as u64;
        let post_cpu_us = process_cpu_time()
            .saturating_sub(post_cpu_started)
            .as_micros() as u64;
        if measurement_started.is_some() {
            let mut evidence = FrameEvidence::new(
                renderer.effect().label(),
                frame_evidence.len() as u64 + 1,
                profiler_requested,
            );
            evidence.render_wall_us = render_wall_us;
            evidence.render_cpu_us = render_cpu_us;
            evidence.transfer_wall_us = transfer_wall_us;
            evidence.transfer_cpu_us = transfer_cpu_us;
            evidence.post_wall_us = post_wall_us;
            evidence.post_cpu_us = post_cpu_us;
            evidence.source_rect_count = copy.source_rect_count;
            evidence.destination_rect_count = copy.destination_rect_count;
            evidence.source_bytes = copy.source_bytes;
            evidence.destination_bytes = copy.destination_bytes;
            evidence.full_restore = copy.full_restore;
            evidence.visible_count = Some(stats.visible);
            evidence.screenshot = stats.screenshot.map(|screenshot| ScreenshotFrameDetails {
                raster_held_cards: screenshot.raster_held_cards,
                raster_moved_cards: screenshot.raster_moved_cards,
                raster_hold_layer_mask: screenshot.raster_hold_layer_mask,
                raster_visible_layer_mask: screenshot.raster_visible_layer_mask,
                sixteenth_phase_layer_mask: screenshot.sixteenth_phase_layer_mask,
                phase_bank_resident_bytes: screenshot.phase_bank_resident_bytes,
                scale_count: screenshot.scale_count,
                scale_total_us: screenshot.scale_total_us,
                scale_max_us: screenshot.scale_max_us,
                phase_count: screenshot.phase_count,
                phase_total_us: screenshot.phase_total_us,
                phase_max_us: screenshot.phase_max_us,
                preparation_queue_depth: screenshot.preparation_queue_depth,
            });
            pending_evidence = Some(PendingFrameEvidence {
                frame: evidence,
                frame_started: render_started,
                frame_cpu_started: render_cpu_started,
                post_completed: Instant::now(),
                pipeline_before: presenter.pipeline_stats(),
            });
        }
        if next_rss_sample.is_some_and(|sample_at| Instant::now() >= sample_at) {
            if let Some(rss_kib) = process_rss_kib() {
                rss_samples_kib.push(rss_kib);
            }
            next_rss_sample = Some(Instant::now() + Duration::from_secs(1));
        }
        status_frames = status_frames.saturating_add(1);
        if bounded.is_some_and(|bounded| {
            measurement_started.is_some_and(|started| started.elapsed() >= bounded.duration)
        }) {
            let settle_started = Instant::now();
            let settle_cpu_started = process_cpu_time();
            let receipt = presenter
                .settle_pending()
                .map_err(|error| format!("settle final hidden RGB565 scene frame: {error}"))?
                .ok_or("final scene presentation did not produce a latch confirmation")?;
            let settle_wall_us = settle_started.elapsed().as_micros() as u64;
            let settle_cpu_us = process_cpu_time()
                .saturating_sub(settle_cpu_started)
                .as_micros() as u64;
            if last_sequence.is_some_and(|sequence| {
                !confirmation_sequence_is_contiguous(sequence, receipt.sequence)
            }) {
                confirmation_sequence_failures = confirmation_sequence_failures.saturating_add(1);
            }
            latch_drop_count = receipt.drop_count;
            if let Some(pending) = pending_evidence.take() {
                let evidence = finish_frame_evidence(
                    pending,
                    receipt,
                    presenter.pipeline_stats(),
                    settle_wall_us,
                    settle_cpu_us,
                    frame_evidence.last(),
                );
                frame_evidence.push(evidence);
            }
            let raw_telemetry_end = presenter
                .presentation_telemetry()
                .map_err(|error| format!("read end FPGA presentation telemetry: {error}"))?;
            let presentation_telemetry_end = PresentationTelemetrySnapshot {
                owned_vblank_count: raw_telemetry_end.owned_vblank_count,
                presented_vblank_count: raw_telemetry_end.presented_vblank_count,
                repeated_vblank_count: raw_telemetry_end.repeated_vblank_count,
                ownership_loss_count: raw_telemetry_end.ownership_loss_count,
                active_sequence: raw_telemetry_end.active_sequence,
                flags: raw_telemetry_end.flags,
            };
            let telemetry_elapsed_us = measurement_started
                .expect("bounded scene completes only after measurement begins")
                .elapsed()
                .as_micros() as u64;
            if let Some(observer) = vsync_observer.as_ref() {
                observer.request_stop();
            }
            #[cfg(feature = "profile")]
            if let Some(active_profiler) = profiler.take() {
                cpu_profile::finish(active_profiler)?;
            }
            let vsync_events = vsync_observer
                .take()
                .map(vsync_observer::VsyncObserver::finish)
                .transpose()?
                .unwrap_or_default();
            let measured_started =
                measurement_started.expect("bounded scene completes only after measurement begins");
            let seconds = measured_started.elapsed().as_secs_f64();
            let cpu_percent = process_cpu_time()
                .saturating_sub(measurement_cpu_started)
                .as_secs_f64()
                / seconds
                * 100.0;
            let (refresh_period_us, refresh_period_source) = plan
                .output_route
                .nominal_period_us()
                .map_or((16_667, "hdmi-60hz-contract"), |period| {
                    (period, "resolved-output-route")
                });
            let telemetry = summarize_presentation_telemetry(
                presentation_telemetry_start
                    .expect("bounded measurement starts with FPGA telemetry"),
                presentation_telemetry_end,
                telemetry_elapsed_us,
                refresh_period_us,
            )?;
            let cadence = apply_authoritative_presentation_telemetry(
                summarize_cadence(&frame_evidence, refresh_period_us, refresh_period_source)?,
                telemetry,
            );
            println!(
                "scene-lab-cadence scene={} profiler_enabled={} authoritative={} source={} refresh_period_us={} confirmed_frames={} expected_refresh_intervals={} unique_latch_flips={} dropped_frames={} software_estimated_dropped_frames={} ownership_losses={} telemetry_invariant={} telemetry_plausible={} confirmation_sequence_failures={} latch_drop_delta={} completion_failures={} long_completion_intervals={} max_completion_interval_us={} unique_fps={:.3}",
                renderer.effect().label(),
                cadence.profiler_enabled,
                cadence.cadence_authoritative,
                cadence.cadence_source,
                cadence.refresh_period_us,
                cadence.confirmed_frames,
                cadence.expected_refresh_intervals,
                cadence.unique_latch_flips,
                cadence.dropped_frames,
                cadence.software_estimated_dropped_frames,
                cadence
                    .presentation_telemetry
                    .as_ref()
                    .map_or(0, |telemetry| telemetry.ownership_loss_delta),
                cadence
                    .presentation_telemetry
                    .as_ref()
                    .is_some_and(|telemetry| telemetry.lifetime_invariant_valid
                        && telemetry.delta_invariant_valid),
                cadence
                    .presentation_telemetry
                    .as_ref()
                    .is_some_and(|telemetry| telemetry.plausible),
                cadence.confirmation_sequence_failures,
                cadence.latch_drop_delta,
                cadence.completion_failures,
                cadence.long_completion_intervals,
                cadence.max_completion_interval_us,
                cadence.unique_fps,
            );
            if let Some(evidence) = measurement_evidence.as_ref() {
                write_measurement_evidence(
                    evidence,
                    &frame_evidence,
                    &vsync_events,
                    &cadence,
                    plan,
                    renderer.effect().label(),
                    None,
                    cpu_percent,
                    &rss_samples_kib,
                )?;
            }
            if let Some(case) = case {
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
                    "cabinet-case name={} particles={} projected_particles={} projection_cohorts={} visible={} pixel_writes={} mode={} seconds={:.3} frames={} fps={:.3} cpu_pct={:.2} render_avg_us={} render_p99_us={} render_max_us={} clear_avg_us={} clear_p99_us={} projection_avg_us={} projection_p99_us={} ordering_avg_us={} ordering_p99_us={} raster_avg_us={} raster_p99_us={} worker_wait_avg_us={} worker_wait_p99_us={} prepared_age_avg_us={} prepared_age_p99_us={} confirmation_sequence_failures={} projection_backend={}",
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
                    confirmation_sequence_failures,
                    stats.projection_backend,
                );
            } else {
                println!(
                    "scene-lab-measurement state=complete scene={} seconds={:.3} frames={} cpu_pct={:.2} confirmation_sequence_failures={} latch_drop_count={} rss_mean_kib={} rss_max_kib={}",
                    renderer.effect().label(),
                    seconds,
                    status_frames,
                    cpu_percent,
                    confirmation_sequence_failures,
                    latch_drop_count,
                    rss_samples_kib
                        .iter()
                        .copied()
                        .sum::<u64>()
                        .checked_div(rss_samples_kib.len() as u64)
                        .unwrap_or(0),
                    rss_samples_kib.iter().copied().max().unwrap_or(0),
                );
            }
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
            let screenshot_metrics = stats.screenshot.map_or_else(
                || "screenshot_metrics=not_applicable".to_owned(),
                |screenshot| {
                    format!(
                        "screenshot_held={} screenshot_moved={} screenshot_hold_mask=0b{:05b} screenshot_visible_mask=0b{:05b} screenshot_sixteenth_mask=0b{:05b} screenshot_phase_bank_bytes={}",
                        screenshot.raster_held_cards,
                        screenshot.raster_moved_cards,
                        screenshot.raster_hold_layer_mask,
                        screenshot.raster_visible_layer_mask,
                        screenshot.sixteenth_phase_layer_mask,
                        screenshot.phase_bank_resident_bytes,
                    )
                },
            );
            let stage_metrics = if clear_samples_us.is_empty() {
                "scene_stages=not_applicable".to_owned()
            } else {
                format!(
                    "scene_clear_avg_us={clear_average_us} scene_clear_p99_us={clear_p99_us} scene_simulation_avg_us={simulation_average_us} scene_simulation_p99_us={simulation_p99_us} scene_projection_avg_us={projection_average_us} scene_projection_p99_us={projection_p99_us} scene_raster_avg_us={raster_average_us} scene_raster_p99_us={raster_p99_us}"
                )
            };
            println!(
                "framebuffer-scene-lab effect={} generation={} state={} cue={} cue_elapsed_ms={} total_ms={} particles={} fps={:.1} cpu_pct={:.1} render_avg_us={} render_p99_us={} render_max_us={} {} {} visible={} simulation_backend={} projection_backend={} slot={} sequence={} confirmation_sequence_failures={} latch_drop_count={} latch_status_reads={} latch_poll_reads={} latch_settle_us={} reload_error={}",
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
                screenshot_metrics,
                stats.visible,
                stats.simulation_backend,
                stats.projection_backend,
                post.slot_index,
                post.sequence,
                confirmation_sequence_failures,
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
            confirmation_sequence_failures = 0;
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
    assessment: Option<MeasurementEvidence>,
    bounded: Option<BoundedRun>,
) -> Result<(), String> {
    use mister_magik_mister_runtime::framebuffer::damage::{DirtyRect, DirtyRectList};
    use mister_magik_mister_runtime::framebuffer::hidden_latch::CachedHiddenLatchPresenter;
    use mister_magik_mister_runtime::framebuffer::rgb565::Rgb565;
    use mister_magik_mister_runtime::lab_input::FramebufferLabInput;
    use std::time::Instant;

    let mut presenter = CachedHiddenLatchPresenter::open(plan)
        .map_err(|error| format!("open plan-aware hidden RGB565 card presenter: {error}"))?;

    let mut renderer = CardFlip::new_device(plan.render_w, plan.render_h)
        .map_err(|error| format!("resolve card geometry: {error}"))?;
    renderer.set_duration(duration);
    let mut staging = vec![Rgb565Pixel(0); plan.render_w * plan.render_h];
    let geometry = renderer.geometry();
    let card_damage = DirtyRectList::from_one(DirtyRect {
        x0: geometry.card_x,
        y0: geometry.card_y,
        x1: geometry.card_x + geometry.card_width,
        y1: geometry.card_y + geometry.card_height,
    });
    let mut controls = CardFlipLabControls::default();
    let mut input = FramebufferLabInput::open();
    let started = Instant::now();
    let mut next_frame = started;
    let mut status_started = started;
    let mut cpu_started = process_cpu_time();
    let mut rendered_frames = 0_u64;
    let mut frame_to_present_samples_us = Vec::with_capacity(128);
    let mut render_samples_us = Vec::with_capacity(128);
    let mut transfer_samples_us = Vec::with_capacity(128);
    let mut present_samples_us = Vec::with_capacity(128);
    let mut pending_frame_started: Option<Instant> = None;
    let mut pending_present_started: Option<Instant> = None;
    let mut last_sequence = None;
    let mut confirmation_sequence_failures = 0_u64;
    let mut latch_drop_count = 0_u16;
    let mut profile_frame_to_present_samples_us = Vec::with_capacity(2048);
    let mut profile_render_samples_us = Vec::with_capacity(2048);
    let mut profile_transfer_samples_us = Vec::with_capacity(2048);
    let mut profile_present_samples_us = Vec::with_capacity(2048);
    let profile_cpu_started = process_cpu_time();
    let mut profile_frames = 0_u64;
    let mut profile_confirmation_sequence_failures = 0_u64;
    let mut transfer_source_rects = 0_u64;
    let mut transfer_destination_rects = 0_u64;
    let mut transfer_source_bytes = 0_u64;
    let mut transfer_destination_bytes = 0_u64;
    let mut full_slot_restores = 0_u64;
    let mut profile_transfer_source_rects = 0_u64;
    let mut profile_transfer_destination_rects = 0_u64;
    let mut profile_transfer_source_bytes = 0_u64;
    let mut profile_transfer_destination_bytes = 0_u64;
    let mut profile_full_slot_restores = 0_u64;
    let mut profile_frame_evidence = Vec::with_capacity(2048);
    let mut pending_card_evidence: Option<PendingFrameEvidence> = None;
    let profiler_requested = profile
        || assessment
            .as_ref()
            .is_some_and(|assessment| assessment.pass.profiler_enabled());
    let bounded_run = bounded.is_some();
    let mut measurement_started = None;
    let mut presentation_telemetry_start = None;
    let mut measurement_cpu_started = profile_cpu_started;
    let mut warmup_full_slot_restores = 0_u64;
    let mut warmup_last_flip_count = None;
    let mut warmup_unit_flip_streak = 0_u8;
    let mut warmup_started = None;
    let mut rss_samples_kib = Vec::new();
    let mut next_rss_sample = None;
    let mut vsync_observer = None;
    let mut automatic_direction = CardFlipDirection::Reverse;
    let mut next_automatic_flip = duration;
    if bounded_run {
        renderer.play(CardFlipDirection::Forward, Duration::ZERO);
    }
    #[cfg(feature = "profile")]
    let mut profiler = None;
    #[cfg(not(feature = "profile"))]
    if profiler_requested {
        return Err("card flip profiling requires a profiled-capable device build".into());
    }

    println!(
        "framebuffer-scene-lab scene=card-flip render={}x{} framebuffer={}x{} scan={}x{} output={}x{} route={} card={}x{} format=rgb565 raster=armv7-fixed-q16 transfer=cached-prepare-only present=post-to-latch-confirmed",
        plan.render_w,
        plan.render_h,
        plan.fb_w,
        plan.fb_h,
        plan.scan_w,
        plan.scan_h,
        plan.output_w,
        plan.output_h,
        plan.output_route.label(),
        geometry.card_width,
        geometry.card_height,
    );

    loop {
        let settle_started = Instant::now();
        let settle_cpu_started = process_cpu_time();
        let settled = presenter
            .settle_pending()
            .map_err(|error| format!("settle hidden RGB565 card frame: {error}"))?;
        let settle_wall_us = settle_started.elapsed().as_micros() as u64;
        let settle_cpu_us = process_cpu_time()
            .saturating_sub(settle_cpu_started)
            .as_micros() as u64;
        if let Some(receipt) = settled {
            if let Some(present_started) = pending_present_started.take() {
                let present_us = present_started.elapsed().as_micros() as u64;
                present_samples_us.push(present_us);
                profile_present_samples_us.push(present_us);
            }
            if let Some(frame_started) = pending_frame_started.take() {
                let frame_to_present_us = frame_started.elapsed().as_micros() as u64;
                frame_to_present_samples_us.push(frame_to_present_us);
                profile_frame_to_present_samples_us.push(frame_to_present_us);
            }
            if last_sequence.is_some_and(|sequence| {
                !confirmation_sequence_is_contiguous(sequence, receipt.sequence)
            }) {
                confirmation_sequence_failures = confirmation_sequence_failures.saturating_add(1);
            }
            last_sequence = Some(receipt.sequence);
            latch_drop_count = receipt.drop_count;
            if bounded_run && measurement_started.is_none() {
                let flip_delta = warmup_last_flip_count
                    .map(|previous: u16| receipt.flip_count.wrapping_sub(previous))
                    .unwrap_or(0);
                warmup_last_flip_count = Some(receipt.flip_count);
                warmup_unit_flip_streak = if flip_delta == 1 {
                    warmup_unit_flip_streak.saturating_add(1)
                } else {
                    0
                };
                if warmup_full_slot_restores >= 2 && warmup_unit_flip_streak >= 3 {
                    let bounded = bounded.expect("bounded card run has duration");
                    let ready_at = *warmup_started.get_or_insert_with(Instant::now);
                    if ready_at.elapsed() >= bounded.warmup {
                        let raw_telemetry_start =
                            presenter.presentation_telemetry().map_err(|error| {
                                format!("read start FPGA presentation telemetry: {error}")
                            })?;
                        let telemetry_start = PresentationTelemetrySnapshot {
                            owned_vblank_count: raw_telemetry_start.owned_vblank_count,
                            presented_vblank_count: raw_telemetry_start.presented_vblank_count,
                            repeated_vblank_count: raw_telemetry_start.repeated_vblank_count,
                            ownership_loss_count: raw_telemetry_start.ownership_loss_count,
                            active_sequence: raw_telemetry_start.active_sequence,
                            flags: raw_telemetry_start.flags,
                        };
                        if !telemetry_start.magik_ownership() || telemetry_start.pending() {
                            return Err(
                                "start FPGA presentation telemetry is not owned and settled".into(),
                            );
                        }
                        let measured_at = Instant::now();
                        measurement_started = Some(measured_at);
                        presentation_telemetry_start = Some(telemetry_start);
                        measurement_cpu_started = process_cpu_time();
                        profile_frame_to_present_samples_us.clear();
                        profile_render_samples_us.clear();
                        profile_transfer_samples_us.clear();
                        profile_present_samples_us.clear();
                        profile_frame_evidence.clear();
                        profile_frames = 0;
                        profile_confirmation_sequence_failures = 0;
                        profile_transfer_source_rects = 0;
                        profile_transfer_destination_rects = 0;
                        profile_transfer_source_bytes = 0;
                        profile_transfer_destination_bytes = 0;
                        profile_full_slot_restores = 0;
                        rss_samples_kib.clear();
                        next_rss_sample = Some(measured_at);
                        if assessment.is_some() {
                            vsync_observer = Some(vsync_observer::VsyncObserver::start()?);
                        }
                        #[cfg(feature = "profile")]
                        if profiler_requested {
                            profiler = Some(cpu_profile::start("card-flip")?);
                        }
                        println!(
                            "card-flip-measurement pass={} state=measuring seconds={} warmup_seconds={} warmup_full_slot_restores={} warmup_unit_flip_streak={}",
                            assessment
                                .as_ref()
                                .map_or("profile", |assessment| assessment.pass.label()),
                            bounded.duration.as_secs(),
                            bounded.warmup.as_secs(),
                            warmup_full_slot_restores,
                            warmup_unit_flip_streak,
                        );
                        status_started = measured_at;
                        cpu_started = measurement_cpu_started;
                    }
                }
            }
            if let Some(pending) = pending_card_evidence.take() {
                let evidence = finish_frame_evidence(
                    pending,
                    receipt,
                    presenter.pipeline_stats(),
                    settle_wall_us,
                    settle_cpu_us,
                    profile_frame_evidence.last(),
                );
                profile_frame_evidence.push(evidence);
            }
        }

        let elapsed = started.elapsed();
        let state = input.poll_state();
        if let Some(direction) = controls.poll(state.button_a, state.button_b) {
            renderer.play(direction, elapsed);
            next_frame = Instant::now();
        }
        if bounded_run && elapsed >= next_automatic_flip {
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
            let frame_started = Instant::now();
            let frame_cpu_started = process_cpu_time();
            let render_started = frame_started;
            let render_cpu_started = frame_cpu_started;
            let stats = renderer
                .render(&mut staging, elapsed)
                .map_err(str::to_owned)?;
            let render_us = render_started.elapsed().as_micros() as u64;
            let render_cpu_us = process_cpu_time()
                .saturating_sub(render_cpu_started)
                .as_micros() as u64;
            if stats.changed {
                // SAFETY: both RGB565 wrappers are transparent over u16 and
                // every u16 bit pattern is valid for either representation.
                let cached = unsafe {
                    std::slice::from_raw_parts(staging.as_ptr().cast::<Rgb565>(), staging.len())
                };
                let transfer_started = Instant::now();
                let transfer_cpu_started = process_cpu_time();
                let copy = presenter
                    .prepare_cached(cached, &card_damage)
                    .map_err(|error| format!("copy cached card frame: {error}"))?;
                let transfer_us = transfer_started.elapsed().as_micros() as u64;
                let transfer_cpu_us = process_cpu_time()
                    .saturating_sub(transfer_cpu_started)
                    .as_micros() as u64;
                let present_started = Instant::now();
                let post_cpu_started = process_cpu_time();
                let post = presenter
                    .post_prepared()
                    .map_err(|error| format!("post hidden RGB565 card frame: {error}"))?;
                let post_wall_us = present_started.elapsed().as_micros() as u64;
                let post_cpu_us = process_cpu_time()
                    .saturating_sub(post_cpu_started)
                    .as_micros() as u64;
                let post_completed = Instant::now();
                pending_frame_started = Some(frame_started);
                pending_present_started = Some(present_started);
                debug_assert_eq!(post.slot_index, copy.slot_index);
                if bounded_run && measurement_started.is_none() && copy.full_restore {
                    warmup_full_slot_restores = warmup_full_slot_restores.saturating_add(1);
                }
                if measurement_started.is_some() {
                    let mut evidence = FrameEvidence::new(
                        "card-flip",
                        profile_frame_evidence.len() as u64 + 1,
                        profiler_requested,
                    );
                    evidence.card = Some(CardFrameDetails {
                        progress_q16: stats.progress_q16,
                        face: if stats.progress_q16 < u16::MAX / 2 {
                            "front"
                        } else {
                            "back"
                        },
                        direction: stats.direction.label(),
                    });
                    evidence.render_wall_us = render_us;
                    evidence.render_cpu_us = render_cpu_us;
                    evidence.transfer_wall_us = transfer_us;
                    evidence.transfer_cpu_us = transfer_cpu_us;
                    evidence.post_wall_us = post_wall_us;
                    evidence.post_cpu_us = post_cpu_us;
                    evidence.source_rect_count = copy.source_rect_count;
                    evidence.destination_rect_count = copy.destination_rect_count;
                    evidence.source_bytes = copy.source_bytes;
                    evidence.destination_bytes = copy.destination_bytes;
                    evidence.full_restore = copy.full_restore;
                    pending_card_evidence = Some(PendingFrameEvidence {
                        frame: evidence,
                        frame_started,
                        frame_cpu_started,
                        post_completed,
                        pipeline_before: presenter.pipeline_stats(),
                    });
                }
                rendered_frames = rendered_frames.saturating_add(1);
                render_samples_us.push(render_us);
                transfer_samples_us.push(transfer_us);
                transfer_source_rects =
                    transfer_source_rects.saturating_add(u64::from(copy.source_rect_count));
                transfer_destination_rects = transfer_destination_rects
                    .saturating_add(u64::from(copy.destination_rect_count));
                transfer_source_bytes =
                    transfer_source_bytes.saturating_add(copy.source_bytes as u64);
                transfer_destination_bytes =
                    transfer_destination_bytes.saturating_add(copy.destination_bytes as u64);
                full_slot_restores =
                    full_slot_restores.saturating_add(u64::from(copy.full_restore));
                if measurement_started.is_some() {
                    profile_render_samples_us.push(render_us);
                    profile_transfer_samples_us.push(transfer_us);
                    profile_transfer_source_rects = profile_transfer_source_rects
                        .saturating_add(u64::from(copy.source_rect_count));
                    profile_transfer_destination_rects = profile_transfer_destination_rects
                        .saturating_add(u64::from(copy.destination_rect_count));
                    profile_transfer_source_bytes =
                        profile_transfer_source_bytes.saturating_add(copy.source_bytes as u64);
                    profile_transfer_destination_bytes = profile_transfer_destination_bytes
                        .saturating_add(copy.destination_bytes as u64);
                    profile_full_slot_restores =
                        profile_full_slot_restores.saturating_add(u64::from(copy.full_restore));
                    profile_frames = profile_frames.saturating_add(1);
                }
            }
            next_frame += FRAME_DURATION;
            if next_frame <= Instant::now() {
                next_frame = Instant::now();
            }
        }

        if next_rss_sample.is_some_and(|sample_at| Instant::now() >= sample_at) {
            if let Some(rss_kib) = process_rss_kib() {
                rss_samples_kib.push(rss_kib);
            }
            next_rss_sample = Some(Instant::now() + Duration::from_secs(1));
        }

        if status_started.elapsed() >= Duration::from_secs(1) {
            let seconds = status_started.elapsed().as_secs_f64();
            let cpu_now = process_cpu_time();
            let cpu_percent = cpu_now.saturating_sub(cpu_started).as_secs_f64() / seconds * 100.0;
            let (frame_to_present_average_us, frame_to_present_p99_us, frame_to_present_max_us) =
                sample_summary(&mut frame_to_present_samples_us);
            let (render_average_us, render_p99_us, render_max_us) =
                sample_summary(&mut render_samples_us);
            let (transfer_average_us, transfer_p99_us, transfer_max_us) =
                sample_summary(&mut transfer_samples_us);
            let (present_average_us, present_p99_us, present_max_us) =
                sample_summary(&mut present_samples_us);
            println!(
                "card-flip frames={} fps={:.1} cpu_pct={:.1} active={} progress_q16={} direction={} frame_to_present_avg_us={} frame_to_present_p99_us={} frame_to_present_max_us={} render_avg_us={} render_p99_us={} render_max_us={} transfer_avg_us={} transfer_p99_us={} transfer_max_us={} present_avg_us={} present_p99_us={} present_max_us={} present_samples={} transfer_source_rects={} transfer_destination_rects={} transfer_source_bytes={} transfer_destination_bytes={} full_slot_restores={} confirmation_sequence_failures={} latch_drop_count={}",
                rendered_frames,
                rendered_frames as f64 / seconds,
                cpu_percent,
                renderer.is_active(),
                renderer.progress_q16(),
                renderer.direction().label(),
                frame_to_present_average_us,
                frame_to_present_p99_us,
                frame_to_present_max_us,
                render_average_us,
                render_p99_us,
                render_max_us,
                transfer_average_us,
                transfer_p99_us,
                transfer_max_us,
                present_average_us,
                present_p99_us,
                present_max_us,
                present_samples_us.len(),
                transfer_source_rects,
                transfer_destination_rects,
                transfer_source_bytes,
                transfer_destination_bytes,
                full_slot_restores,
                confirmation_sequence_failures,
                latch_drop_count,
            );
            status_started = Instant::now();
            cpu_started = cpu_now;
            rendered_frames = 0;
            frame_to_present_samples_us.clear();
            render_samples_us.clear();
            transfer_samples_us.clear();
            present_samples_us.clear();
            transfer_source_rects = 0;
            transfer_destination_rects = 0;
            transfer_source_bytes = 0;
            transfer_destination_bytes = 0;
            full_slot_restores = 0;
            profile_confirmation_sequence_failures = profile_confirmation_sequence_failures
                .saturating_add(confirmation_sequence_failures);
            confirmation_sequence_failures = 0;
        }

        if bounded_run
            && measurement_started.is_some_and(|started| {
                bounded.is_some_and(|bounded| started.elapsed() >= bounded.duration)
            })
        {
            let settle_started = Instant::now();
            let settle_cpu_started = process_cpu_time();
            let settled = presenter
                .settle_pending()
                .map_err(|error| format!("settle final hidden RGB565 card frame: {error}"))?;
            let settle_wall_us = settle_started.elapsed().as_micros() as u64;
            let settle_cpu_us = process_cpu_time()
                .saturating_sub(settle_cpu_started)
                .as_micros() as u64;
            if let Some(receipt) = settled {
                if let Some(present_started) = pending_present_started.take() {
                    let present_us = present_started.elapsed().as_micros() as u64;
                    present_samples_us.push(present_us);
                    profile_present_samples_us.push(present_us);
                }
                if let Some(frame_started) = pending_frame_started.take() {
                    let frame_to_present_us = frame_started.elapsed().as_micros() as u64;
                    frame_to_present_samples_us.push(frame_to_present_us);
                    profile_frame_to_present_samples_us.push(frame_to_present_us);
                }
                if last_sequence.is_some_and(|sequence| {
                    !confirmation_sequence_is_contiguous(sequence, receipt.sequence)
                }) {
                    confirmation_sequence_failures =
                        confirmation_sequence_failures.saturating_add(1);
                }
                latch_drop_count = receipt.drop_count;
                if let Some(pending) = pending_card_evidence.take() {
                    let evidence = finish_frame_evidence(
                        pending,
                        receipt,
                        presenter.pipeline_stats(),
                        settle_wall_us,
                        settle_cpu_us,
                        profile_frame_evidence.last(),
                    );
                    profile_frame_evidence.push(evidence);
                }
            }
            let raw_telemetry_end = presenter
                .presentation_telemetry()
                .map_err(|error| format!("read end FPGA presentation telemetry: {error}"))?;
            let presentation_telemetry_end = PresentationTelemetrySnapshot {
                owned_vblank_count: raw_telemetry_end.owned_vblank_count,
                presented_vblank_count: raw_telemetry_end.presented_vblank_count,
                repeated_vblank_count: raw_telemetry_end.repeated_vblank_count,
                ownership_loss_count: raw_telemetry_end.ownership_loss_count,
                active_sequence: raw_telemetry_end.active_sequence,
                flags: raw_telemetry_end.flags,
            };
            let telemetry_elapsed_us = measurement_started
                .expect("bounded card run completes only after measurement begins")
                .elapsed()
                .as_micros() as u64;
            if let Some(observer) = vsync_observer.as_ref() {
                observer.request_stop();
            }
            let measured_started = measurement_started
                .expect("bounded card run completes only after measurement begins");
            let seconds = measured_started.elapsed().as_secs_f64();
            let cpu_percent = process_cpu_time()
                .saturating_sub(measurement_cpu_started)
                .as_secs_f64()
                / seconds
                * 100.0;
            #[cfg(feature = "profile")]
            if let Some(active_profiler) = profiler.take() {
                cpu_profile::finish(active_profiler)?;
            }
            let vsync_events = vsync_observer
                .take()
                .map(vsync_observer::VsyncObserver::finish)
                .transpose()?
                .unwrap_or_default();
            let (frame_to_present_average_us, frame_to_present_p99_us, frame_to_present_max_us) =
                sample_summary(&mut profile_frame_to_present_samples_us);
            let (render_average_us, render_p99_us, render_max_us) =
                sample_summary(&mut profile_render_samples_us);
            let (transfer_average_us, transfer_p99_us, transfer_max_us) =
                sample_summary(&mut profile_transfer_samples_us);
            let (present_average_us, present_p99_us, present_max_us) =
                sample_summary(&mut profile_present_samples_us);
            println!(
                "card-flip-profile seconds={:.3} frames={} fps={:.3} cpu_pct={:.2} frame_to_present_avg_us={} frame_to_present_p99_us={} frame_to_present_max_us={} render_avg_us={} render_p99_us={} render_max_us={} transfer_avg_us={} transfer_p99_us={} transfer_max_us={} present_avg_us={} present_p99_us={} present_max_us={} present_samples={} transfer_source_rects={} transfer_destination_rects={} transfer_source_bytes={} transfer_destination_bytes={} full_slot_restores={} confirmation_sequence_failures={} latch_drop_count={}",
                seconds,
                profile_frames,
                profile_frames as f64 / seconds,
                cpu_percent,
                frame_to_present_average_us,
                frame_to_present_p99_us,
                frame_to_present_max_us,
                render_average_us,
                render_p99_us,
                render_max_us,
                transfer_average_us,
                transfer_p99_us,
                transfer_max_us,
                present_average_us,
                present_p99_us,
                present_max_us,
                profile_present_samples_us.len(),
                profile_transfer_source_rects,
                profile_transfer_destination_rects,
                profile_transfer_source_bytes,
                profile_transfer_destination_bytes,
                profile_full_slot_restores,
                profile_confirmation_sequence_failures
                    .saturating_add(confirmation_sequence_failures),
                latch_drop_count,
            );
            let (refresh_period_us, refresh_period_source) = plan
                .output_route
                .nominal_period_us()
                .map_or((16_667, "hdmi-60hz-contract"), |period| {
                    (period, "resolved-output-route")
                });
            let telemetry = summarize_presentation_telemetry(
                presentation_telemetry_start
                    .expect("bounded card measurement starts with FPGA telemetry"),
                presentation_telemetry_end,
                telemetry_elapsed_us,
                refresh_period_us,
            )?;
            let cadence = apply_authoritative_presentation_telemetry(
                summarize_cadence(
                    &profile_frame_evidence,
                    refresh_period_us,
                    refresh_period_source,
                )?,
                telemetry,
            );
            println!(
                "card-flip-cadence profiler_enabled={} authoritative={} source={} refresh_period_us={} confirmed_frames={} expected_refresh_intervals={} unique_latch_flips={} dropped_frames={} software_estimated_dropped_frames={} confirmation_sequence_failures={} latch_drop_delta={} completion_failures={} long_completion_intervals={} max_completion_interval_us={} unique_fps={:.3}",
                cadence.profiler_enabled,
                cadence.cadence_authoritative,
                cadence.cadence_source,
                cadence.refresh_period_us,
                cadence.confirmed_frames,
                cadence.expected_refresh_intervals,
                cadence.unique_latch_flips,
                cadence.dropped_frames,
                cadence.software_estimated_dropped_frames,
                cadence.confirmation_sequence_failures,
                cadence.latch_drop_delta,
                cadence.completion_failures,
                cadence.long_completion_intervals,
                cadence.max_completion_interval_us,
                cadence.unique_fps,
            );
            if let Some(assessment) = assessment.as_ref() {
                write_measurement_evidence(
                    assessment,
                    &profile_frame_evidence,
                    &vsync_events,
                    &cadence,
                    plan,
                    "card-flip",
                    Some(geometry),
                    cpu_percent,
                    &rss_samples_kib,
                )?;
                println!(
                    "card-flip-assessment pass={} state=complete evidence_dir={}",
                    assessment.pass.label(),
                    assessment.evidence_dir.display(),
                );
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

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn process_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?.trim();
        value.split_ascii_whitespace().next()?.parse().ok()
    })
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn monotonic_time_us() -> u64 {
    let mut value = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: clock_gettime initializes the supplied timespec on success and
    // CLOCK_MONOTONIC is a process-independent monotonic clock.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, value.as_mut_ptr()) };
    if result != 0 {
        return 0;
    }
    // SAFETY: the successful call above initialized every field.
    let value = unsafe { value.assume_init() };
    (value.tv_sec as u64)
        .saturating_mul(1_000_000)
        .saturating_add(value.tv_nsec as u64 / 1_000)
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_arch = "arm"))))]
fn run_window(
    _source: SceneSource,
    _case: Option<CabinetCase>,
    _profile: bool,
    _measurement_evidence: Option<MeasurementEvidence>,
    _bounded: Option<BoundedRun>,
) -> Result<(), String> {
    Err("interactive startup particle preview requires macOS or ARM MiSTer".into())
}

struct Options {
    scene: EffectKind,
    recipe: Option<PathBuf>,
    archive: Option<PathBuf>,
    fixture: Option<NavigationFixture>,
    seed: u64,
    seed_requested: bool,
    sampling_profile: ScreenshotSamplingProfile,
    sampling_profile_requested: bool,
    phase_generation: ScreenshotPhaseGeneration,
    phase_generation_requested: bool,
    replacement_mode: ScreenshotParadeReplacementMode,
    replacement_mode_requested: bool,
    time_ms: Option<u64>,
    output: Option<PathBuf>,
    check: bool,
    case: Option<CabinetCase>,
    seconds: Option<u64>,
    warmup_seconds: u64,
    profile: bool,
    assessment_pass: Option<MeasurementPass>,
    evidence_dir: Option<PathBuf>,
    duration_ms: Option<u64>,
    direction: CardFlipDirection,
    direction_requested: bool,
}

impl Options {
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut recipe = None;
        let mut archive = None;
        let mut fixture = None;
        let mut seed = DEFAULT_SCREENSHOT_SEED;
        let mut seed_requested = false;
        let mut sampling_profile = ScreenshotSamplingProfile::HdmiLegacyHalf;
        let mut sampling_profile_requested = false;
        let mut phase_generation = ScreenshotPhaseGeneration::LinearLanczos3;
        let mut phase_generation_requested = false;
        let mut replacement_mode = ScreenshotParadeReplacementMode::Prepare;
        let mut replacement_mode_requested = false;
        let mut scene = None;
        let mut time_ms = None;
        let mut output = None;
        let mut check = false;
        let mut case = None;
        let mut seconds = None;
        let mut warmup_seconds = 0;
        let mut profile = false;
        let mut assessment_pass = None;
        let mut evidence_dir = None;
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
                            "invalid scene {value:?}; expected magik, cabinet, intro, navigation-transition, card-flip, or screenshot-screensaver"
                        ));
                    }
                }
                "--recipe" => recipe = arguments.next().map(PathBuf::from),
                "--archive" => archive = arguments.next().map(PathBuf::from),
                "--seed" => {
                    let value = arguments.next().ok_or("--seed requires an integer")?;
                    seed = parse_seed(&value)?;
                    seed_requested = true;
                }
                "--sampling-profile" => {
                    let value = arguments
                        .next()
                        .ok_or("--sampling-profile requires legacy-half or sixteenth")?;
                    sampling_profile = match value.as_str() {
                        "legacy-half" | "hdmi" => ScreenshotSamplingProfile::HdmiLegacyHalf,
                        "sixteenth" | "crt" => ScreenshotSamplingProfile::CrtSixteenth,
                        _ => {
                            return Err(format!(
                                "invalid sampling profile {value:?}; expected legacy-half or sixteenth"
                            ));
                        }
                    };
                    sampling_profile_requested = true;
                }
                "--phase-generation" => {
                    let value = arguments
                        .next()
                        .ok_or(
                            "--phase-generation requires two-tap, linear-lanczos3, or linear-lanczos3-neon",
                        )?;
                    phase_generation = match value.as_str() {
                        "two-tap" => ScreenshotPhaseGeneration::Rgb565TwoTap,
                        "linear-lanczos3" => ScreenshotPhaseGeneration::LinearLanczos3,
                        "linear-lanczos3-neon" => ScreenshotPhaseGeneration::LinearLanczos3Neon,
                        _ => {
                            return Err(format!(
                                "invalid phase generation {value:?}; expected two-tap, linear-lanczos3, or linear-lanczos3-neon"
                            ));
                        }
                    };
                    phase_generation_requested = true;
                }
                "--replacement-mode" => {
                    let value = arguments
                        .next()
                        .ok_or("--replacement-mode requires prepare or recycle")?;
                    replacement_mode = match value.as_str() {
                        "prepare" => ScreenshotParadeReplacementMode::Prepare,
                        "recycle" => ScreenshotParadeReplacementMode::Recycle,
                        _ => {
                            return Err(format!(
                                "invalid replacement mode {value:?}; expected prepare or recycle"
                            ));
                        }
                    };
                    replacement_mode_requested = true;
                }
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
                "--seconds" => {
                    let value = arguments.next().ok_or("--seconds requires a value")?;
                    seconds = Some(
                        value
                            .parse::<u64>()
                            .ok()
                            .filter(|value| (1..=600).contains(value))
                            .ok_or_else(|| {
                                format!("invalid --seconds value {value:?}; expected 1..=600")
                            })?,
                    );
                }
                "--warmup-seconds" => {
                    let value = arguments
                        .next()
                        .ok_or("--warmup-seconds requires a value")?;
                    warmup_seconds = value
                        .parse::<u64>()
                        .ok()
                        .filter(|value| *value <= 600)
                        .ok_or_else(|| {
                            format!("invalid --warmup-seconds value {value:?}; expected 0..=600")
                        })?;
                }
                "--profile" => profile = true,
                "--assessment-pass" => {
                    let value = arguments
                        .next()
                        .ok_or("--assessment-pass requires cadence or profile")?;
                    assessment_pass = Some(match value.as_str() {
                        "cadence" => MeasurementPass::Cadence,
                        "profile" => MeasurementPass::Profile,
                        _ => {
                            return Err(format!(
                                "invalid assessment pass {value:?}; expected cadence or profile"
                            ));
                        }
                    });
                }
                "--evidence-dir" => evidence_dir = arguments.next().map(PathBuf::from),
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
            archive,
            fixture,
            seed,
            seed_requested,
            sampling_profile,
            sampling_profile_requested,
            phase_generation,
            phase_generation_requested,
            replacement_mode,
            replacement_mode_requested,
            time_ms,
            output,
            check,
            case,
            seconds,
            warmup_seconds,
            profile,
            assessment_pass,
            evidence_dir,
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
        if self.case.is_some() && self.seconds.is_none() {
            return Err("--case requires --seconds".into());
        }
        if self.warmup_seconds > 0 && self.seconds.is_none() {
            return Err("--warmup-seconds requires --seconds".into());
        }
        if (self.profile || self.assessment_pass.is_some()) && self.seconds.is_none() {
            return Err("profiling and assessment require --seconds".into());
        }
        if self.assessment_pass.is_some() != self.evidence_dir.is_some() {
            return Err("measurement requires both --assessment-pass and --evidence-dir".into());
        }
        if self.assessment_pass.is_some() && (self.profile || self.check || self.output.is_some()) {
            return Err("--assessment-pass requires an interactive run without --profile".into());
        }
        if self.scene != EffectKind::CardFlip
            && (self.duration_ms.is_some() || self.direction_requested)
        {
            return Err("--duration-ms and --direction are valid only for card-flip".into());
        }
        if self.scene != EffectKind::ScreenshotScreensaver
            && (self.archive.is_some()
                || self.seed_requested
                || self.sampling_profile_requested
                || self.phase_generation_requested
                || self.replacement_mode_requested)
        {
            return Err(
                "--archive, --seed, --sampling-profile, --phase-generation, and --replacement-mode are valid only for screenshot-screensaver".into(),
            );
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
            EffectKind::ScreenshotScreensaver => {
                if self.archive.is_none() {
                    return Err("screenshot-screensaver requires --archive".into());
                }
                if self.recipe.is_some() || self.fixture.is_some() {
                    return Err(
                        "screenshot-screensaver accepts no recipe or fixture options".into(),
                    );
                }
                if self.case.is_some() || self.duration_ms.is_some() || self.direction_requested {
                    return Err(
                        "screenshot-screensaver rejects cabinet and card-only options".into(),
                    );
                }
            }
        }
        Ok(())
    }

    fn card_duration(&self) -> Duration {
        Duration::from_millis(self.duration_ms.unwrap_or(440))
    }

    fn bounded_run(&self) -> Option<BoundedRun> {
        self.seconds.map(|seconds| BoundedRun {
            duration: Duration::from_secs(seconds),
            warmup: Duration::from_secs(self.warmup_seconds),
        })
    }

    fn measurement_evidence(&self) -> Option<MeasurementEvidence> {
        self.assessment_pass.map(|pass| MeasurementEvidence {
            pass,
            evidence_dir: self
                .evidence_dir
                .clone()
                .expect("option validation pairs assessment pass and evidence directory"),
        })
    }
}

fn usage() -> &'static str {
    "usage:\n  mister-magik-framebuffer-scene-lab --scene magik|cabinet|intro --recipe FILE.json\n  mister-magik-framebuffer-scene-lab --scene cabinet --recipe FILE.json --case NAME --seconds N [--profile]\n  mister-magik-framebuffer-scene-lab --scene navigation-transition --fixture home-arcade|home-consoles|consoles-system\n  mister-magik-framebuffer-scene-lab --scene card-flip [--duration-ms N]\n  mister-magik-framebuffer-scene-lab --scene SCENE --seconds N [--warmup-seconds N] [--profile]\n  mister-magik-framebuffer-scene-lab --scene SCENE --seconds N --assessment-pass cadence|profile --evidence-dir DIR\n  mister-magik-framebuffer-scene-lab --scene card-flip --direction forward|reverse --time-ms N --output FILE.ppm\n  mister-magik-framebuffer-scene-lab --scene screenshot-screensaver --archive FILE [--seed SEED] [--sampling-profile legacy-half|sixteenth] [--phase-generation two-tap|linear-lanczos3] [--replacement-mode prepare|recycle]\n  mister-magik-framebuffer-scene-lab --scene SCENE (--recipe FILE.json|--fixture FIXTURE|--archive FILE) --time-ms N --output FILE.ppm\n  mister-magik-framebuffer-scene-lab --scene SCENE --check"
}

fn parse_seed(value: &str) -> Result<u64, String> {
    let value = value.trim();
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or_else(
            || value.parse::<u64>(),
            |digits| u64::from_str_radix(digits, 16),
        )
        .map_err(|_| format!("invalid screenshot seed {value:?}"))
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
                None,
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
                "--seconds",
                "30",
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

        let assessment = Options::parse(
            [
                "--scene",
                "card-flip",
                "--seconds",
                "30",
                "--assessment-pass",
                "cadence",
                "--evidence-dir",
                "/tmp/card-assessment",
            ]
            .map(String::from),
        )
        .unwrap();
        let assessment = assessment.measurement_evidence().unwrap();
        assert_eq!(assessment.pass, MeasurementPass::Cadence);
        assert_eq!(
            assessment.evidence_dir,
            PathBuf::from("/tmp/card-assessment")
        );

        for (profile, expected) in [
            ("legacy-half", ScreenshotSamplingProfile::HdmiLegacyHalf),
            ("hdmi", ScreenshotSamplingProfile::HdmiLegacyHalf),
            ("sixteenth", ScreenshotSamplingProfile::CrtSixteenth),
            ("crt", ScreenshotSamplingProfile::CrtSixteenth),
        ] {
            let screenshot = Options::parse(
                [
                    "--scene",
                    "screenshot-screensaver",
                    "--archive",
                    "screenshots.mmlz4b",
                    "--seed",
                    "0x1234",
                    "--sampling-profile",
                    profile,
                ]
                .map(String::from),
            )
            .unwrap();
            assert_eq!(
                screenshot.archive,
                Some(PathBuf::from("screenshots.mmlz4b"))
            );
            assert_eq!(screenshot.seed, 0x1234);
            assert_eq!(screenshot.sampling_profile, expected, "profile={profile}");
            assert_eq!(
                screenshot.phase_generation,
                ScreenshotPhaseGeneration::LinearLanczos3
            );
        }
        let two_tap = Options::parse(
            [
                "--scene",
                "screenshot-screensaver",
                "--archive",
                "screenshots.mmlz4b",
                "--sampling-profile",
                "sixteenth",
                "--phase-generation",
                "two-tap",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(
            two_tap.phase_generation,
            ScreenshotPhaseGeneration::Rgb565TwoTap
        );
        let neon = Options::parse(
            [
                "--scene",
                "screenshot-screensaver",
                "--archive",
                "screenshots.mmlz4b",
                "--sampling-profile",
                "sixteenth",
                "--phase-generation",
                "linear-lanczos3-neon",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(
            neon.phase_generation,
            ScreenshotPhaseGeneration::LinearLanczos3Neon
        );
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
                ["--scene", "card-flip", "--assessment-pass", "profile"].map(String::from)
            )
            .is_err()
        );
        assert!(
            Options::parse(
                [
                    "--scene",
                    "card-flip",
                    "--assessment-pass",
                    "profile",
                    "--evidence-dir",
                    "/tmp/card-assessment",
                    "--profile",
                ]
                .map(String::from)
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
        assert!(Options::parse(["--scene", "screenshot-screensaver"].map(String::from)).is_err());
        assert!(
            Options::parse(
                [
                    "--scene",
                    "screenshot-screensaver",
                    "--archive",
                    "screenshots.mmlz4b",
                    "--recipe",
                    "magik.json",
                ]
                .map(String::from),
            )
            .is_err()
        );
        assert!(
            Options::parse(
                ["--scene", "magik", "--recipe", "magik.json", "--seed", "7",].map(String::from),
            )
            .is_err()
        );
    }

    #[test]
    fn bounded_measurement_options_are_generic_and_bounded() {
        let screenshot = Options::parse(
            [
                "--scene",
                "screenshot-screensaver",
                "--archive",
                "screenshots.mmlz4b",
                "--seconds",
                "90",
                "--warmup-seconds",
                "3",
                "--assessment-pass",
                "profile",
                "--evidence-dir",
                "/tmp/screenshot-profile",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(screenshot.seconds, Some(90));
        assert_eq!(screenshot.warmup_seconds, 3);

        let card = Options::parse(
            [
                "--scene",
                "card-flip",
                "--seconds",
                "5",
                "--duration-ms",
                "700",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(card.card_duration(), Duration::from_millis(700));
        for arguments in [
            vec!["--scene", "card-flip", "--seconds", "0"],
            vec!["--scene", "card-flip", "--seconds", "601"],
            vec!["--scene", "card-flip", "--warmup-seconds", "1"],
            vec![
                "--scene",
                "card-flip",
                "--seconds",
                "1",
                "--warmup-seconds",
                "601",
            ],
            vec!["--scene", "card-flip", "--profile"],
        ] {
            assert!(Options::parse(arguments.into_iter().map(String::from)).is_err());
        }
    }

    #[test]
    fn screenshot_seed_accepts_decimal_and_prefixed_hex() {
        assert_eq!(parse_seed("42").unwrap(), 42);
        assert_eq!(parse_seed("0x2a").unwrap(), 42);
        assert_eq!(parse_seed("0X2A").unwrap(), 42);
        assert!(parse_seed("0xnope").is_err());
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
}
