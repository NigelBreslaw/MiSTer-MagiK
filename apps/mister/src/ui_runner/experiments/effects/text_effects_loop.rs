// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::effect_loop_support::{run_effect_picker_loop, EffectLoopConfig, EffectTraceRow};
use super::*;
use mister_magik_fb::experiments::effects::text_effects::{
    render_text_effect_frame, synthetic_text_images, TextEffectFrameStats, TextEffectKind,
    TextEffectRenderState,
};
use std::fs::File;
use std::io::Write;

const CONTROLLER_EXIT_GRACE: Duration = Duration::from_millis(500);
const TRACE_HEADER: &[u8] = b"effect\tframe\telapsed_us\twall_us\tcpu_us\tcpu_pct\tdraw_us\tpresent_us\tvsync_us\tclear_us\tbackground_us\tprojection_us\timage_blit_us\tsprite_us\tpost_us\thud_us\tglyph_count\tglyph_pixels\ttile_count\tvector_segment_count\tbob_count\tpalette_step_count\thidden_glyph_count\tscroll_offset\tvsync_source\tvsync_period_us\tvsync_miss_streak\n";

pub(in crate::ui_runner) fn print_text_effects() {
    crate::ui_logln!("{}", TextEffectKind::labels());
}

pub(in crate::ui_runner) fn run_text_effects_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
) {
    run_effect_picker_loop(
        secs,
        ui,
        disp,
        EffectLoopConfig {
            family_label: "text-effects",
            effects_env: "MISTER_TEXT_EFFECTS",
            segment_env: "MISTER_TEXT_EFFECTS_SEGMENT_SECS",
            cache_cap_env: "MISTER_TEXT_EFFECTS_CACHE_CAP",
            auto_env: "MISTER_TEXT_EFFECTS_AUTO",
            hud_env: "MISTER_TEXT_EFFECTS_HUD",
            trace_env: "MISTER_TEXT_EFFECTS_TRACE",
            trace_header: TRACE_HEADER,
            all_effects: TextEffectKind::all(),
            parse_effect: TextEffectKind::parse,
            label_effect: TextEffectKind::label,
            default_effect: TextEffectKind::InsertCoinBlinkCadence,
            synthetic_min: 2,
            synthetic_max: 8,
            synthetic_images: synthetic_text_images,
            new_state: TextEffectRenderState::new,
            render: render_text_effect_frame,
            draw_us: |stats| (*stats).draw_us(),
            write_trace_row,
            controller_exit_grace: Some(CONTROLLER_EXIT_GRACE),
        },
    );
}

fn write_trace_row(
    trace: &mut File,
    row: &EffectTraceRow<TextEffectKind>,
    stats: &TextEffectFrameStats,
) {
    let _ = writeln!(
        trace,
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        row.effect.label(),
        row.frame,
        row.elapsed_us,
        row.wall_us,
        row.cpu_us,
        row.cpu_pct,
        row.draw_us,
        row.present_us,
        row.vsync.wait_us,
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
        row.vsync.source.label(),
        row.vsync.period_us,
        row.vsync.miss_streak
    );
}
