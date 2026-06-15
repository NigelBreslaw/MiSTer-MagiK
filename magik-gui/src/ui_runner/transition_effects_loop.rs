use super::*;
use crate::input::PadPool;
use crate::input_repeat::RepeatNav;
use crate::preview_worker;
use mister_magik_fb::camera_effects::{CameraImage, CameraPixel};
use mister_magik_fb::raw565;
use mister_magik_fb::transition_effects::{
    render_transition_effect_frame, synthetic_transition_images, TransitionEffectKind,
    TransitionEffectRenderState,
};
use std::fs::File;
use std::io::Write;
use std::path::Path;

pub(super) fn print_transition_effects() {
    println!("{}", TransitionEffectKind::labels());
}

const CONTROLLER_EXIT_GRACE: Duration = Duration::from_millis(500);

struct TransitionEffectsConfig {
    effects: Vec<TransitionEffectKind>,
    segment: Duration,
    cache_cap: usize,
    auto: bool,
    hud: bool,
    trace: Option<File>,
}

impl TransitionEffectsConfig {
    fn from_env() -> Self {
        let effects = parse_effects_env();
        let segment_secs = std::env::var("MISTER_TRANSITION_EFFECTS_SEGMENT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(20)
            .max(1);
        let cache_cap = std::env::var("MISTER_TRANSITION_EFFECTS_CACHE_CAP")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(64)
            .clamp(1, 512);
        let auto = env_truthy("MISTER_TRANSITION_EFFECTS_AUTO");
        let hud = env_truthy("MISTER_TRANSITION_EFFECTS_HUD");
        let trace = std::env::var("MISTER_TRANSITION_EFFECTS_TRACE")
            .ok()
            .and_then(|path| {
                let mut f = File::create(&path)
                    .map_err(|e| eprintln!("transition effects trace: create {path} failed: {e}"))
                    .ok()?;
                f.write_all(
                    b"effect\tframe\telapsed_us\twall_us\tcpu_us\tcpu_pct\tdraw_us\tpresent_us\tvsync_us\tclear_us\tbackground_us\tprojection_us\timage_blit_us\tsprite_us\tpost_us\thud_us\tmask_cell_count\trevealed_pixel_count\thidden_pixel_count\tsource_a_pixel_count\tsource_b_pixel_count\tshake_offset_px\tflash_pixel_count\twarp_sample_count\tghost_pixel_count\tglitch_band_count\tvsync_source\tvsync_period_us\tvsync_miss_streak\n",
                )
                .ok()?;
                println!("transition_effects_trace={path}");
                Some(f)
            });
        Self {
            effects,
            segment: Duration::from_secs(segment_secs),
            cache_cap,
            auto,
            hud,
            trace,
        }
    }

    fn effect_at(&self, elapsed: Duration, selected_idx: usize) -> TransitionEffectKind {
        if self.auto {
            let idx = ((elapsed.as_micros() / self.segment.as_micros().max(1)) as usize)
                % self.effects.len().max(1);
            self.effects
                .get(idx)
                .copied()
                .unwrap_or(TransitionEffectKind::VenetianBlindsWipe)
        } else {
            self.effects
                .get(selected_idx % self.effects.len().max(1))
                .copied()
                .unwrap_or(TransitionEffectKind::VenetianBlindsWipe)
        }
    }
}

pub(super) fn run_transition_effects_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    _fb_format: FramebufferFormat,
) {
    let mut cfg = TransitionEffectsConfig::from_env();
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    let images = load_transition_effect_images(&arcade_root, cfg.cache_cap);
    println!(
        "transition-effects effects={} auto={} hud={} segment_secs={} cache_cap={} images={}",
        cfg.effects
            .iter()
            .map(|effect| effect.label())
            .collect::<Vec<_>>()
            .join(","),
        cfg.auto,
        cfg.hud,
        cfg.segment.as_secs(),
        cfg.cache_cap,
        images.len()
    );

    let mut pad = if cfg.auto {
        None
    } else {
        match PadPool::open_all() {
            Ok(pad) => Some(pad),
            Err(e) => {
                eprintln!("pad: unavailable for transition-effects picker: {e}");
                None
            }
        }
    };
    let mut repeat = RepeatNav::default();
    let mut selected_idx = 0usize;
    let mut backbuffer = vec![CameraPixel(0); ui.render_w() * ui.render_h()];
    let mut render_state = TransitionEffectRenderState::new(ui.render_w(), ui.render_h());
    let mut pacer = VsyncPacer::from_env();
    let start = Instant::now();
    let mut frame = 0u64;
    let mut last_effect = TransitionEffectKind::VenetianBlindsWipe;
    let mut exit_was_held = false;

    loop {
        let frame_start = Instant::now();
        let cpu_start = process_cpu_us();
        let elapsed = start.elapsed();
        if secs > 0 && elapsed >= Duration::from_secs(secs) {
            break;
        }

        if let Some(pad) = pad.as_mut() {
            let _ = pad.poll();
            let state = pad.state();
            let now = Instant::now();
            if repeat.tick_left(state.dpad_left, now) && !cfg.effects.is_empty() {
                selected_idx = selected_idx.wrapping_add(cfg.effects.len()).wrapping_sub(1)
                    % cfg.effects.len();
                println!(
                    "transition_effect_selected={}",
                    cfg.effects[selected_idx].label()
                );
            }
            if repeat.tick_right(state.dpad_right, now) && !cfg.effects.is_empty() {
                selected_idx = (selected_idx + 1) % cfg.effects.len();
                println!(
                    "transition_effect_selected={}",
                    cfg.effects[selected_idx].label()
                );
            }
            let exit_held = state.btn_b || state.btn_start;
            if elapsed >= CONTROLLER_EXIT_GRACE && exit_held && !exit_was_held {
                println!("transition_effects_exit=controller");
                break;
            }
            exit_was_held = exit_held;
        }

        let effect = cfg.effect_at(elapsed, selected_idx);
        if effect != last_effect {
            frame = 0;
            last_effect = effect;
        }
        let effect_idx = cfg
            .effects
            .iter()
            .position(|candidate| *candidate == effect)
            .unwrap_or(selected_idx);
        let hud_text = cfg.hud.then(|| {
            format!(
                "{} {}/{}",
                effect.label(),
                effect_idx + 1,
                cfg.effects.len()
            )
        });
        let stats = render_transition_effect_frame(
            &mut backbuffer,
            &mut render_state,
            ui.render_w(),
            ui.render_h(),
            &images,
            effect,
            frame,
            hud_text.as_deref(),
        );
        let draw_us = stats.draw_us();

        let vsync = pacer.wait();
        let present_start = Instant::now();
        disp.copy_rows_camera_565(&backbuffer, 0, ui.render_h());
        let present_us = present_start.elapsed().as_micros() as u64;
        let wall_us = frame_start.elapsed().as_micros() as u64;
        let cpu_us = process_cpu_us().saturating_sub(cpu_start);
        let cpu_pct = if wall_us == 0 {
            0
        } else {
            ((cpu_us as u128 * 100) / wall_us as u128) as u64
        };

        if let Some(trace) = cfg.trace.as_mut() {
            let _ = writeln!(
                trace,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                effect.label(),
                frame,
                elapsed.as_micros(),
                wall_us,
                cpu_us,
                cpu_pct,
                draw_us,
                present_us,
                vsync.wait_us,
                stats.clear_us,
                stats.background_us,
                stats.projection_us,
                stats.image_blit_us,
                stats.sprite_us,
                stats.post_us,
                stats.hud_us,
                stats.mask_cell_count,
                stats.revealed_pixel_count,
                stats.hidden_pixel_count,
                stats.source_a_pixel_count,
                stats.source_b_pixel_count,
                stats.shake_offset_px,
                stats.flash_pixel_count,
                stats.warp_sample_count,
                stats.ghost_pixel_count,
                stats.glitch_band_count,
                vsync.source.label(),
                vsync.period_us,
                vsync.miss_streak
            );
        }

        frame = frame.wrapping_add(1);
    }
}

fn parse_effects_env() -> Vec<TransitionEffectKind> {
    let spec = std::env::var("MISTER_TRANSITION_EFFECTS").unwrap_or_else(|_| "mega".into());
    if matches!(
        spec.trim().to_ascii_lowercase().as_str(),
        "" | "mega" | "all" | "demo"
    ) {
        return TransitionEffectKind::all().to_vec();
    }
    let mut effects = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(effect) = TransitionEffectKind::parse(part) {
            effects.push(effect);
        } else {
            eprintln!("transition-effects: unknown effect {part:?}");
        }
    }
    if effects.is_empty() {
        vec![TransitionEffectKind::VenetianBlindsWipe]
    } else {
        effects
    }
}

fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn process_cpu_us() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    if rc == 0 {
        (ts.tv_sec as u64)
            .saturating_mul(1_000_000)
            .saturating_add((ts.tv_nsec as u64) / 1_000)
    } else {
        0
    }
}

fn load_transition_effect_images(root: &str, cap: usize) -> Vec<CameraImage> {
    let resize = preview_worker::PreviewResizeSpec::from_env();
    let mut paths = Vec::new();
    if let Ok(loaded) = library_db::load_arcade_catalog_from_sqlite(root) {
        paths.extend(
            loaded
                .catalog
                .games
                .iter()
                .filter(|game| game.has_image && !game.image_path.is_empty())
                .map(|game| game.image_path.clone())
                .take(cap * 4),
        );
    }
    let mut images = Vec::new();
    for path in paths {
        if images.len() >= cap {
            break;
        }
        let cache = preview_worker::raw565_preview_cache_path(&path, resize);
        if let Some(img) = read_raw565_image(&cache) {
            images.push(img);
        }
    }
    if images.is_empty() {
        synthetic_transition_images(cap.min(8).max(2))
    } else {
        images
    }
}

fn read_raw565_image(path: &Path) -> Option<CameraImage> {
    let data = std::fs::read(path).ok()?;
    let image = raw565::decode_raw565(&data).ok()?;
    let pixels = image.words.into_iter().map(CameraPixel).collect();
    Some(CameraImage {
        pixels,
        w: image.width,
        h: image.height,
        stride: image.stride_words,
    })
}
