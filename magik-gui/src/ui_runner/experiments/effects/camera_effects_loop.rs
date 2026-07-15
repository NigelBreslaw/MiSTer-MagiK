// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::effect_loop_support::{run_effect_picker_loop, EffectLoopConfig, EffectTraceRow};
use super::*;
use mister_magik_fb::experiments::effects::camera_effects::{
    render_camera_effect_frame, synthetic_images, CameraEffectFrameStats, CameraEffectKind,
    CameraEffectRenderState,
};
use std::fs::File;
use std::io::Write;

const TRACE_HEADER: &[u8] = b"effect\tframe\telapsed_us\twall_us\tcpu_us\tcpu_pct\tdraw_us\tpresent_us\tvsync_us\tclear_us\tbackground_us\tprojection_us\timage_blit_us\tsprite_us\tpost_us\thud_us\tvsync_source\tvsync_period_us\tvsync_miss_streak\n";

pub(in crate::ui_runner) fn print_camera_effects() {
    crate::ui_logln!("{}", CameraEffectKind::labels());
}

pub(in crate::ui_runner) fn run_camera_effects_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
) {
    run_effect_picker_loop(
        secs,
        ui,
        disp,
        EffectLoopConfig {
            family_label: "camera-effects",
            effects_env: "MISTER_CAMERA_EFFECTS",
            segment_env: "MISTER_CAMERA_EFFECTS_SEGMENT_SECS",
            cache_cap_env: "MISTER_CAMERA_EFFECTS_CACHE_CAP",
            auto_env: "MISTER_CAMERA_EFFECTS_AUTO",
            hud_env: "MISTER_CAMERA_EFFECTS_HUD",
            trace_env: "MISTER_CAMERA_EFFECTS_TRACE",
            trace_header: TRACE_HEADER,
            all_effects: CameraEffectKind::all(),
            parse_effect: CameraEffectKind::parse,
            label_effect: CameraEffectKind::label,
            default_effect: CameraEffectKind::MultiLayerParallax,
            synthetic_min: 8,
            synthetic_max: 24,
            synthetic_images,
            new_state: CameraEffectRenderState::new,
            render: render_camera_effect_frame,
            draw_us: |stats| (*stats).draw_us(),
            write_trace_row,
            controller_exit_grace: None,
        },
    );
}

fn write_trace_row(
    trace: &mut File,
    row: &EffectTraceRow<CameraEffectKind>,
    stats: &CameraEffectFrameStats,
) {
    let _ = writeln!(
        trace,
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
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
        row.vsync.source.label(),
        row.vsync.period_us,
        row.vsync.miss_streak
    );
}
