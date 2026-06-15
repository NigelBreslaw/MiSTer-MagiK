use super::*;
use crate::input::PadPool;
use crate::input_repeat::RepeatNav;
use crate::preview_worker;
use mister_magik_fb::camera_effects::{CameraImage, CameraPixel};
use mister_magik_fb::raw565;
use mister_magik_fb::text_effects::{
    pixel_to_rgb888, render_text_effect_frame, synthetic_text_images, TextEffectKind,
    TextEffectRenderState,
};
use std::fs::File;
use std::io::Write;
use std::path::Path;

pub(super) fn print_text_effects() {
    println!("{}", TextEffectKind::labels());
}

const CONTROLLER_EXIT_GRACE: Duration = Duration::from_millis(500);

struct TextEffectsConfig {
    effects: Vec<TextEffectKind>,
    segment: Duration,
    cache_cap: usize,
    auto: bool,
    hud: bool,
    trace: Option<File>,
}

impl TextEffectsConfig {
    fn from_env() -> Self {
        let effects = parse_effects_env();
        let segment_secs = std::env::var("MISTER_TEXT_EFFECTS_SEGMENT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(20)
            .max(1);
        let cache_cap = std::env::var("MISTER_TEXT_EFFECTS_CACHE_CAP")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(64)
            .clamp(1, 512);
        let auto = env_truthy("MISTER_TEXT_EFFECTS_AUTO");
        let hud = env_truthy("MISTER_TEXT_EFFECTS_HUD");
        let trace = std::env::var("MISTER_TEXT_EFFECTS_TRACE")
            .ok()
            .and_then(|path| {
                let mut f = File::create(&path)
                    .map_err(|e| eprintln!("text effects trace: create {path} failed: {e}"))
                    .ok()?;
                f.write_all(
                    b"effect\tframe\telapsed_us\twall_us\tcpu_us\tcpu_pct\tdraw_us\tpresent_us\tvsync_us\tclear_us\tbackground_us\tprojection_us\timage_blit_us\tsprite_us\tpost_us\thud_us\tglyph_count\tglyph_pixels\ttile_count\tvector_segment_count\tbob_count\tpalette_step_count\thidden_glyph_count\tscroll_offset\tvsync_source\tvsync_period_us\tvsync_miss_streak\n",
                )
                .ok()?;
                println!("text_effects_trace={path}");
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

    fn effect_at(&self, elapsed: Duration, selected_idx: usize) -> TextEffectKind {
        if self.auto {
            let idx = ((elapsed.as_micros() / self.segment.as_micros().max(1)) as usize)
                % self.effects.len().max(1);
            self.effects
                .get(idx)
                .copied()
                .unwrap_or(TextEffectKind::InsertCoinBlinkCadence)
        } else {
            self.effects
                .get(selected_idx % self.effects.len().max(1))
                .copied()
                .unwrap_or(TextEffectKind::InsertCoinBlinkCadence)
        }
    }
}

pub(super) fn run_text_effects_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    fb_format: FramebufferFormat,
) {
    let mut cfg = TextEffectsConfig::from_env();
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    let images = load_text_effect_images(&arcade_root, cfg.cache_cap);
    println!(
        "text-effects effects={} auto={} hud={} segment_secs={} cache_cap={} images={}",
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
                eprintln!("pad: unavailable for text-effects picker: {e}");
                None
            }
        }
    };
    let mut repeat = RepeatNav::default();
    let mut selected_idx = 0usize;
    let mut backbuffer = vec![CameraPixel(0); ui.render_w() * ui.render_h()];
    let mut render_state = TextEffectRenderState::new(ui.render_w(), ui.render_h());
    let mut xrgb_scratch =
        (fb_format == FramebufferFormat::Xrgb8888).then(|| vec![Pixel(0); backbuffer.len()]);
    let mut pacer = VsyncPacer::from_env();
    let start = Instant::now();
    let mut frame = 0u64;
    let mut last_effect = TextEffectKind::InsertCoinBlinkCadence;
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
                println!("text_effect_selected={}", cfg.effects[selected_idx].label());
            }
            if repeat.tick_right(state.dpad_right, now) && !cfg.effects.is_empty() {
                selected_idx = (selected_idx + 1) % cfg.effects.len();
                println!("text_effect_selected={}", cfg.effects[selected_idx].label());
            }
            let exit_held = state.btn_b || state.btn_start;
            if elapsed >= CONTROLLER_EXIT_GRACE && exit_held && !exit_was_held {
                println!("text_effects_exit=controller");
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
        let stats = render_text_effect_frame(
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
        match fb_format {
            FramebufferFormat::Rgb565 => {
                disp.copy_rows_camera_565(&backbuffer, 0, ui.render_h());
            }
            FramebufferFormat::Xrgb8888 => {
                let scratch = xrgb_scratch.as_mut().expect("xrgb scratch");
                for (dst, src) in scratch.iter_mut().zip(backbuffer.iter().copied()) {
                    *dst = Pixel(pixel_to_rgb888(src));
                }
                disp.copy_rows(scratch, 0, ui.render_h());
            }
        }
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
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
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
                stats.glyph_count,
                stats.glyph_pixels,
                stats.tile_count,
                stats.vector_segment_count,
                stats.bob_count,
                stats.palette_step_count,
                stats.hidden_glyph_count,
                stats.scroll_offset,
                vsync.source.label(),
                vsync.period_us,
                vsync.miss_streak
            );
        }

        frame = frame.wrapping_add(1);
    }
}

fn parse_effects_env() -> Vec<TextEffectKind> {
    let spec = std::env::var("MISTER_TEXT_EFFECTS").unwrap_or_else(|_| "mega".into());
    if matches!(
        spec.trim().to_ascii_lowercase().as_str(),
        "" | "mega" | "all" | "demo"
    ) {
        return TextEffectKind::all().to_vec();
    }
    let mut effects = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(effect) = TextEffectKind::parse(part) {
            effects.push(effect);
        } else {
            eprintln!("text-effects: unknown effect {part:?}");
        }
    }
    if effects.is_empty() {
        vec![TextEffectKind::InsertCoinBlinkCadence]
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

fn load_text_effect_images(root: &str, cap: usize) -> Vec<CameraImage> {
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
        synthetic_text_images(cap.min(8).max(2))
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
