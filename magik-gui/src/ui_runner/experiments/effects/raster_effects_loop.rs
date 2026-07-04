use super::effect_loop_support::{run_effect_picker_loop, EffectLoopConfig, EffectTraceRow};
use super::*;
use mister_magik_fb::experiments::effects::raster_effects::{
    render_raster_effect_frame, synthetic_raster_images, RasterEffectFrameStats, RasterEffectKind,
    RasterEffectRenderState,
};
use std::fs::File;
use std::io::Write;

const CONTROLLER_EXIT_GRACE: Duration = Duration::from_millis(500);
const TRACE_HEADER: &[u8] = b"effect\tframe\telapsed_us\twall_us\tcpu_us\tcpu_pct\tdraw_us\tpresent_us\tvsync_us\tclear_us\tbackground_us\tprojection_us\timage_blit_us\tsprite_us\tpost_us\thud_us\tpalette_step_count\tlut_lookup_count\trow_op_count\tdither_pixel_count\tflash_pixel_count\ttrail_pixel_count\tindexed_pixel_count\treflection_row_count\tvsync_source\tvsync_period_us\tvsync_miss_streak\n";

pub(in crate::ui_runner) fn print_raster_effects() {
    crate::ui_logln!("{}", RasterEffectKind::labels());
}

pub(in crate::ui_runner) fn run_raster_effects_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
) {
    run_effect_picker_loop(
        secs,
        ui,
        disp,
        EffectLoopConfig {
            family_label: "raster-effects",
            effects_env: "MISTER_RASTER_EFFECTS",
            segment_env: "MISTER_RASTER_EFFECTS_SEGMENT_SECS",
            cache_cap_env: "MISTER_RASTER_EFFECTS_CACHE_CAP",
            auto_env: "MISTER_RASTER_EFFECTS_AUTO",
            hud_env: "MISTER_RASTER_EFFECTS_HUD",
            trace_env: "MISTER_RASTER_EFFECTS_TRACE",
            trace_header: TRACE_HEADER,
            all_effects: RasterEffectKind::all(),
            parse_effect: RasterEffectKind::parse,
            label_effect: RasterEffectKind::label,
            default_effect: RasterEffectKind::PaletteCyclingLavaWaterNeon,
            synthetic_min: 2,
            synthetic_max: 8,
            synthetic_images: synthetic_raster_images,
            new_state: RasterEffectRenderState::new,
            render: render_raster_effect_frame,
            draw_us: |stats| (*stats).draw_us(),
            write_trace_row,
            controller_exit_grace: Some(CONTROLLER_EXIT_GRACE),
        },
    );
}

fn write_trace_row(
    trace: &mut File,
    row: &EffectTraceRow<RasterEffectKind>,
    stats: &RasterEffectFrameStats,
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
        stats.palette_step_count,
        stats.lut_lookup_count,
        stats.row_op_count,
        stats.dither_pixel_count,
        stats.flash_pixel_count,
        stats.trail_pixel_count,
        stats.indexed_pixel_count,
        stats.reflection_row_count,
        row.vsync.source.label(),
        row.vsync.period_us,
        row.vsync.miss_streak
    );
}
