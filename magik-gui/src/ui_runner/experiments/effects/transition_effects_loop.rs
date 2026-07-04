use super::effect_loop_support::{run_effect_picker_loop, EffectLoopConfig, EffectTraceRow};
use super::*;
use mister_magik_fb::experiments::effects::transition_effects::{
    render_transition_effect_frame, synthetic_transition_images, TransitionEffectFrameStats,
    TransitionEffectKind, TransitionEffectRenderState,
};
use std::fs::File;
use std::io::Write;

const CONTROLLER_EXIT_GRACE: Duration = Duration::from_millis(500);
const TRACE_HEADER: &[u8] = b"effect\tframe\telapsed_us\twall_us\tcpu_us\tcpu_pct\tdraw_us\tpresent_us\tvsync_us\tclear_us\tbackground_us\tprojection_us\timage_blit_us\tsprite_us\tpost_us\thud_us\tmask_cell_count\trevealed_pixel_count\thidden_pixel_count\tsource_a_pixel_count\tsource_b_pixel_count\tshake_offset_px\tflash_pixel_count\twarp_sample_count\tghost_pixel_count\tglitch_band_count\tvsync_source\tvsync_period_us\tvsync_miss_streak\n";

pub(in crate::ui_runner) fn print_transition_effects() {
    crate::ui_logln!("{}", TransitionEffectKind::labels());
}

pub(in crate::ui_runner) fn run_transition_effects_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
) {
    run_effect_picker_loop(
        secs,
        ui,
        disp,
        EffectLoopConfig {
            family_label: "transition-effects",
            effects_env: "MISTER_TRANSITION_EFFECTS",
            segment_env: "MISTER_TRANSITION_EFFECTS_SEGMENT_SECS",
            cache_cap_env: "MISTER_TRANSITION_EFFECTS_CACHE_CAP",
            auto_env: "MISTER_TRANSITION_EFFECTS_AUTO",
            hud_env: "MISTER_TRANSITION_EFFECTS_HUD",
            trace_env: "MISTER_TRANSITION_EFFECTS_TRACE",
            trace_header: TRACE_HEADER,
            all_effects: TransitionEffectKind::all(),
            parse_effect: TransitionEffectKind::parse,
            label_effect: TransitionEffectKind::label,
            default_effect: TransitionEffectKind::VenetianBlindsWipe,
            synthetic_min: 2,
            synthetic_max: 8,
            synthetic_images: synthetic_transition_images,
            new_state: TransitionEffectRenderState::new,
            render: render_transition_effect_frame,
            draw_us: |stats| (*stats).draw_us(),
            write_trace_row,
            controller_exit_grace: Some(CONTROLLER_EXIT_GRACE),
        },
    );
}

fn write_trace_row(
    trace: &mut File,
    row: &EffectTraceRow<TransitionEffectKind>,
    stats: &TransitionEffectFrameStats,
) {
    let _ = writeln!(
        trace,
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
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
        row.vsync.source.label(),
        row.vsync.period_us,
        row.vsync.miss_streak
    );
}
