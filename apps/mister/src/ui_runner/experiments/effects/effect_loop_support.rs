// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::input::PadPool;
use crate::input_repeat::RepeatNav;
use crate::preview_worker;
use mister_magik_fb::experiments::effects::camera_effects::{CameraImage, CameraPixel};
use mister_magik_fb::framebuffer::mapped::MappedRgb565Framebuffer;
use mister_magik_fb::framebuffer::vsync::VsyncPace;
use slint::platform::software_renderer::Rgb565Pixel;
use std::fs::File;
use std::io::Write;

pub(super) struct EffectLoopConfig<Kind, State, Stats>
where
    Kind: Copy + Eq + 'static,
{
    pub(super) family_label: &'static str,
    pub(super) effects_env: &'static str,
    pub(super) segment_env: &'static str,
    pub(super) cache_cap_env: &'static str,
    pub(super) auto_env: &'static str,
    pub(super) hud_env: &'static str,
    pub(super) trace_env: &'static str,
    pub(super) trace_header: &'static [u8],
    pub(super) all_effects: &'static [Kind],
    pub(super) parse_effect: fn(&str) -> Option<Kind>,
    pub(super) label_effect: fn(Kind) -> &'static str,
    pub(super) default_effect: Kind,
    pub(super) synthetic_min: usize,
    pub(super) synthetic_max: usize,
    pub(super) synthetic_images: fn(usize) -> Vec<CameraImage>,
    pub(super) new_state: fn(usize, usize) -> State,
    pub(super) render: fn(
        &mut [CameraPixel],
        &mut State,
        usize,
        usize,
        &[CameraImage],
        Kind,
        u64,
        Option<&str>,
    ) -> Stats,
    pub(super) draw_us: fn(&Stats) -> u64,
    pub(super) write_trace_row: fn(&mut File, &EffectTraceRow<Kind>, &Stats),
    pub(super) controller_exit_grace: Option<Duration>,
}

pub(super) struct EffectTraceRow<Kind> {
    pub(super) effect: Kind,
    pub(super) frame: u64,
    pub(super) elapsed_us: u128,
    pub(super) wall_us: u64,
    pub(super) cpu_us: u64,
    pub(super) cpu_pct: u64,
    pub(super) draw_us: u64,
    pub(super) present_us: u64,
    pub(super) vsync: VsyncPace,
}

pub(super) fn run_effect_picker_loop<Kind, State, Stats>(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
    spec: EffectLoopConfig<Kind, State, Stats>,
) where
    Kind: Copy + Eq + 'static,
{
    let effects = parse_effects_env(
        spec.effects_env,
        spec.family_label,
        spec.all_effects,
        spec.parse_effect,
        spec.default_effect,
    );
    let segment = segment_from_env(spec.segment_env, 20);
    let cache_cap = cache_cap_from_env(spec.cache_cap_env, 64);
    let auto = env_truthy(spec.auto_env);
    let hud = env_truthy(spec.hud_env);
    let mut trace = create_trace(spec.trace_env, spec.family_label, spec.trace_header);
    let arcade_root = arcade_root_from_env();
    let images = load_effect_images(
        &arcade_root,
        cache_cap,
        spec.synthetic_min,
        spec.synthetic_max,
        spec.synthetic_images,
    );
    crate::ui_logln!(
        "{} effects={} auto={} hud={} segment_secs={} cache_cap={} images={}",
        spec.family_label,
        effects
            .iter()
            .map(|effect| (spec.label_effect)(*effect))
            .collect::<Vec<_>>()
            .join(","),
        auto,
        hud,
        segment.as_secs(),
        cache_cap,
        images.len()
    );

    let mut pad = if auto {
        None
    } else {
        match PadPool::open_all() {
            Ok(pad) => Some(pad),
            Err(e) => {
                crate::ui_errln!("pad: unavailable for {} picker: {e}", spec.family_label);
                None
            }
        }
    };
    let mut repeat = RepeatNav::default();
    let mut selected_idx = 0usize;
    let mut backbuffer = vec![CameraPixel(0); ui.render_w() * ui.render_h()];
    let mut present_buffer = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];
    let mut render_state = (spec.new_state)(ui.render_w(), ui.render_h());
    let mut pacer = VsyncPacer::from_env();
    let start = Instant::now();
    let mut frame = 0u64;
    let mut last_effect = spec.default_effect;
    let mut exit_was_held = false;
    let selected_log = format!(
        "{}_effect_selected",
        spec.family_label
            .trim_end_matches("-effects")
            .replace('-', "_")
    );
    let exit_log = format!("{}_exit=controller", spec.family_label.replace('-', "_"));

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
            if repeat.tick_left(state.dpad_left, now) && !effects.is_empty() {
                selected_idx =
                    selected_idx.wrapping_add(effects.len()).wrapping_sub(1) % effects.len();
                crate::ui_logln!(
                    "{}={}",
                    selected_log,
                    (spec.label_effect)(effects[selected_idx])
                );
            }
            if repeat.tick_right(state.dpad_right, now) && !effects.is_empty() {
                selected_idx = (selected_idx + 1) % effects.len();
                crate::ui_logln!(
                    "{}={}",
                    selected_log,
                    (spec.label_effect)(effects[selected_idx])
                );
            }
            let exit_held = state.btn_b || state.btn_start;
            let exit_requested = match spec.controller_exit_grace {
                Some(grace) => elapsed >= grace && exit_held && !exit_was_held,
                None => exit_held,
            };
            if exit_requested {
                crate::ui_logln!("{exit_log}");
                break;
            }
            exit_was_held = exit_held;
        }

        let effect = selected_effect(
            &effects,
            auto,
            segment,
            elapsed,
            selected_idx,
            spec.default_effect,
        );
        if effect != last_effect {
            frame = 0;
            last_effect = effect;
        }
        let effect_idx = effects
            .iter()
            .position(|candidate| *candidate == effect)
            .unwrap_or(selected_idx);
        let hud_text = hud.then(|| {
            format!(
                "{} {}/{}",
                (spec.label_effect)(effect),
                effect_idx + 1,
                effects.len()
            )
        });
        let stats = (spec.render)(
            &mut backbuffer,
            &mut render_state,
            ui.render_w(),
            ui.render_h(),
            &images,
            effect,
            frame,
            hud_text.as_deref(),
        );
        let draw_us = (spec.draw_us)(&stats);

        let vsync = pacer.wait();
        let present_start = Instant::now();
        present_camera_pixels_565(disp, &backbuffer, &mut present_buffer, 0, ui.render_h());
        let present_us = present_start.elapsed().as_micros() as u64;
        let wall_us = frame_start.elapsed().as_micros() as u64;
        let cpu_us = process_cpu_us().saturating_sub(cpu_start);
        let cpu_pct = if wall_us == 0 {
            0
        } else {
            ((cpu_us as u128 * 100) / wall_us as u128) as u64
        };

        if let Some(trace) = trace.as_mut() {
            let row = EffectTraceRow {
                effect,
                frame,
                elapsed_us: elapsed.as_micros(),
                wall_us,
                cpu_us,
                cpu_pct,
                draw_us,
                present_us,
                vsync,
            };
            (spec.write_trace_row)(trace, &row, &stats);
        }

        frame = frame.wrapping_add(1);
    }
}

pub(super) fn parse_effects_env<T: Copy>(
    env_name: &str,
    family_label: &str,
    all: &[T],
    parse: fn(&str) -> Option<T>,
    default: T,
) -> Vec<T> {
    let spec = std::env::var(env_name).unwrap_or_else(|_| "mega".into());
    if matches!(
        spec.trim().to_ascii_lowercase().as_str(),
        "" | "mega" | "all" | "demo"
    ) {
        return all.to_vec();
    }
    let mut effects = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(effect) = parse(part) {
            effects.push(effect);
        } else {
            crate::ui_errln!("{family_label}: unknown effect {part:?}");
        }
    }
    if effects.is_empty() {
        vec![default]
    } else {
        effects
    }
}

pub(super) fn selected_effect<T: Copy>(
    effects: &[T],
    auto: bool,
    segment: Duration,
    elapsed: Duration,
    selected_idx: usize,
    default: T,
) -> T {
    if effects.is_empty() {
        return default;
    }
    let idx = if auto {
        (elapsed.as_micros() / segment.as_micros().max(1)) as usize % effects.len()
    } else {
        selected_idx % effects.len()
    };
    effects.get(idx).copied().unwrap_or(default)
}

pub(super) fn segment_from_env(name: &str, default_secs: u64) -> Duration {
    Duration::from_secs(
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(default_secs)
            .max(1),
    )
}

pub(super) fn cache_cap_from_env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(1, 512)
}

pub(super) fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub(super) fn present_camera_pixels_565(
    disp: &mut MappedRgb565Framebuffer,
    src: &[CameraPixel],
    scratch: &mut [Rgb565Pixel],
    y0: usize,
    y1: usize,
) {
    debug_assert!(scratch.len() >= src.len());
    for (dst, src) in scratch.iter_mut().zip(src.iter()) {
        *dst = Rgb565Pixel(src.0);
    }
    if let Err(e) = disp.present_rows_565(scratch, y0, y1) {
        crate::ui_errln!("effect present failed: {e}");
    }
}

pub(super) fn create_trace(
    env_name: &str,
    family_label: &str,
    header: &'static [u8],
) -> Option<File> {
    std::env::var(env_name).ok().and_then(|path| {
        let mut f = File::create(&path)
            .map_err(|e| crate::ui_errln!("{family_label} trace: create {path} failed: {e}"))
            .ok()?;
        f.write_all(header).ok()?;
        crate::ui_logln!("{}_trace={path}", family_label.replace('-', "_"));
        Some(f)
    })
}

pub(super) fn process_cpu_us() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts points to initialized writable storage for the duration of the
    // syscall; failures are converted to 0.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    if rc == 0 {
        (ts.tv_sec as u64)
            .saturating_mul(1_000_000)
            .saturating_add((ts.tv_nsec as u64) / 1_000)
    } else {
        0
    }
}

pub(super) fn arcade_root_from_env() -> String {
    std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string())
}

pub(super) fn load_effect_images(
    root: &str,
    cap: usize,
    synthetic_min: usize,
    synthetic_max: usize,
    synthetic: fn(usize) -> Vec<CameraImage>,
) -> Vec<CameraImage> {
    let mut assets = Vec::new();
    if let Ok(loaded) = crate::library_db::load_arcade_catalog_from_sqlite(root) {
        assets.extend(
            loaded
                .catalog
                .games
                .iter()
                .filter(|game| {
                    game.has_preview
                        && !game.preview_archive_path.is_empty()
                        && !game.preview_asset_key.is_empty()
                })
                .map(|game| {
                    (
                        game.preview_archive_path.to_string(),
                        game.preview_asset_key.to_string(),
                    )
                })
                .take(cap * 4),
        );
    }
    let mut images = Vec::new();
    for (archive_path, asset_key) in assets {
        if images.len() >= cap {
            break;
        }
        if let Ok(image) = preview_worker::load_preview_asset_pixels(&archive_path, &asset_key) {
            let image = preview_pixels_to_camera_image(image);
            images.push(image);
        }
    }
    if images.is_empty() {
        synthetic(cap.min(synthetic_max).max(synthetic_min))
    } else {
        images
    }
}

fn preview_pixels_to_camera_image(image: preview_worker::PreviewPixels) -> CameraImage {
    match image {
        preview_worker::PreviewPixels::Rgb565 {
            width,
            height,
            stride_bytes,
            words,
        } => CameraImage {
            pixels: words.iter().copied().map(CameraPixel).collect(),
            w: width as usize,
            h: height as usize,
            stride: stride_bytes as usize / 2,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FakeEffect {
        First,
        Second,
    }

    fn parse_fake(label: &str) -> Option<FakeEffect> {
        match label {
            "first" => Some(FakeEffect::First),
            "second" => Some(FakeEffect::Second),
            _ => None,
        }
    }

    fn fake_synthetic(count: usize) -> Vec<CameraImage> {
        (0..count)
            .map(|idx| CameraImage {
                pixels: vec![CameraPixel(idx as u16)],
                w: 1,
                h: 1,
                stride: 1,
            })
            .collect()
    }

    #[test]
    fn parse_effects_defaults_to_all_for_mega() {
        let parsed = parse_effects_env(
            "MISTER_TEST_EFFECTS_UNSET",
            "test-effects",
            &[FakeEffect::First, FakeEffect::Second],
            parse_fake,
            FakeEffect::First,
        );

        assert_eq!(parsed, vec![FakeEffect::First, FakeEffect::Second]);
    }

    #[test]
    fn selected_effect_uses_auto_segment_or_manual_index() {
        let effects = [FakeEffect::First, FakeEffect::Second];

        assert_eq!(
            selected_effect(
                &effects,
                true,
                Duration::from_secs(2),
                Duration::from_secs(3),
                0,
                FakeEffect::First,
            ),
            FakeEffect::Second
        );
        assert_eq!(
            selected_effect(
                &effects,
                false,
                Duration::from_secs(2),
                Duration::from_secs(3),
                1,
                FakeEffect::First,
            ),
            FakeEffect::Second
        );
    }

    #[test]
    fn load_effect_images_falls_back_to_synthetic_when_cache_is_empty() {
        let images = load_effect_images("/path/that/does/not/exist", 64, 2, 8, fake_synthetic);

        assert_eq!(images.len(), 8);
        assert_eq!(images[0].pixels[0], CameraPixel(0));
    }
}
