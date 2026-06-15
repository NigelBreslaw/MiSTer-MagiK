use super::effect_loop_support::{
    arcade_root_from_env, cache_cap_from_env, create_trace, env_truthy, load_effect_images,
    parse_effects_env, process_cpu_us, segment_from_env, selected_effect,
};
use super::*;
use crate::input::PadPool;
use crate::input_repeat::RepeatNav;
use mister_magik_fb::camera_effects::{CameraImage, CameraPixel};
use mister_magik_fb::sprite_effects::{
    render_sprite_effect_frame, synthetic_sprite_images, SpriteEffectKind, SpriteEffectRenderState,
};
use std::fs::File;
use std::io::Write;

pub(super) fn print_sprite_effects() {
    println!("{}", SpriteEffectKind::labels());
}

const CONTROLLER_EXIT_GRACE: Duration = Duration::from_millis(500);

struct SpriteEffectsConfig {
    effects: Vec<SpriteEffectKind>,
    segment: Duration,
    cache_cap: usize,
    auto: bool,
    hud: bool,
    trace: Option<File>,
}

impl SpriteEffectsConfig {
    fn from_env() -> Self {
        let effects = parse_effects_env(
            "MISTER_SPRITE_EFFECTS",
            "sprite-effects",
            SpriteEffectKind::all(),
            SpriteEffectKind::parse,
            SpriteEffectKind::SpriteZoomTowardCamera,
        );
        let segment = segment_from_env("MISTER_SPRITE_EFFECTS_SEGMENT_SECS", 20);
        let cache_cap = cache_cap_from_env("MISTER_SPRITE_EFFECTS_CACHE_CAP", 64);
        let auto = env_truthy("MISTER_SPRITE_EFFECTS_AUTO");
        let hud = env_truthy("MISTER_SPRITE_EFFECTS_HUD");
        let trace = create_trace(
            "MISTER_SPRITE_EFFECTS_TRACE",
            "sprite-effects",
            b"effect\tframe\telapsed_us\twall_us\tcpu_us\tcpu_pct\tdraw_us\tpresent_us\tvsync_us\tclear_us\tbackground_us\tprojection_us\timage_blit_us\tsprite_us\tpost_us\thud_us\tsprite_count\tsprite_pixels\tparticle_count\tflicker_skip_count\tvsync_source\tvsync_period_us\tvsync_miss_streak\n",
        );
        Self {
            effects,
            segment,
            cache_cap,
            auto,
            hud,
            trace,
        }
    }

    fn effect_at(&self, elapsed: Duration, selected_idx: usize) -> SpriteEffectKind {
        selected_effect(
            &self.effects,
            self.auto,
            self.segment,
            elapsed,
            selected_idx,
            SpriteEffectKind::SpriteZoomTowardCamera,
        )
    }
}

pub(super) fn run_sprite_effects_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    _fb_format: FramebufferFormat,
) {
    let mut cfg = SpriteEffectsConfig::from_env();
    let arcade_root = arcade_root_from_env();
    let images = load_sprite_effect_images(&arcade_root, cfg.cache_cap);
    println!(
        "sprite-effects effects={} auto={} hud={} segment_secs={} cache_cap={} images={}",
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
                eprintln!("pad: unavailable for sprite-effects picker: {e}");
                None
            }
        }
    };
    let mut repeat = RepeatNav::default();
    let mut selected_idx = 0usize;
    let mut backbuffer = vec![CameraPixel(0); ui.render_w() * ui.render_h()];
    let mut render_state = SpriteEffectRenderState::new(ui.render_w(), ui.render_h());
    let mut pacer = VsyncPacer::from_env();
    let start = Instant::now();
    let mut frame = 0u64;
    let mut last_effect = SpriteEffectKind::SpriteZoomTowardCamera;
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
                    "sprite_effect_selected={}",
                    cfg.effects[selected_idx].label()
                );
            }
            if repeat.tick_right(state.dpad_right, now) && !cfg.effects.is_empty() {
                selected_idx = (selected_idx + 1) % cfg.effects.len();
                println!(
                    "sprite_effect_selected={}",
                    cfg.effects[selected_idx].label()
                );
            }
            let exit_held = state.btn_b || state.btn_start;
            if elapsed >= CONTROLLER_EXIT_GRACE && exit_held && !exit_was_held {
                println!("sprite_effects_exit=controller");
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
        let stats = render_sprite_effect_frame(
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
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
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
                stats.sprite_count,
                stats.sprite_pixels,
                stats.particle_count,
                stats.flicker_skip_count,
                vsync.source.label(),
                vsync.period_us,
                vsync.miss_streak
            );
        }

        frame = frame.wrapping_add(1);
    }
}

fn load_sprite_effect_images(root: &str, cap: usize) -> Vec<CameraImage> {
    load_effect_images(root, cap, 8, 24, synthetic_sprite_images)
}
