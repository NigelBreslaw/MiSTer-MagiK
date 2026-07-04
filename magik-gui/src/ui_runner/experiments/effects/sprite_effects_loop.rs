use super::effect_loop_support::{run_effect_picker_loop, EffectLoopConfig, EffectTraceRow};
use super::*;
use mister_magik_fb::experiments::effects::sprite_effects::{
    render_sprite_effect_frame, synthetic_sprite_images, SpriteEffectFrameStats, SpriteEffectKind,
    SpriteEffectRenderState,
};
use std::fs::File;
use std::io::Write;

const CONTROLLER_EXIT_GRACE: Duration = Duration::from_millis(500);
const TRACE_HEADER: &[u8] = b"effect\tframe\telapsed_us\twall_us\tcpu_us\tcpu_pct\tdraw_us\tpresent_us\tvsync_us\tclear_us\tbackground_us\tprojection_us\timage_blit_us\tsprite_us\tpost_us\thud_us\tsprite_count\tsprite_pixels\tparticle_count\tflicker_skip_count\tvsync_source\tvsync_period_us\tvsync_miss_streak\n";

pub(in crate::ui_runner) fn print_sprite_effects() {
    crate::ui_logln!("{}", SpriteEffectKind::labels());
}

pub(in crate::ui_runner) fn run_sprite_effects_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
) {
    run_effect_picker_loop(
        secs,
        ui,
        disp,
        EffectLoopConfig {
            family_label: "sprite-effects",
            effects_env: "MISTER_SPRITE_EFFECTS",
            segment_env: "MISTER_SPRITE_EFFECTS_SEGMENT_SECS",
            cache_cap_env: "MISTER_SPRITE_EFFECTS_CACHE_CAP",
            auto_env: "MISTER_SPRITE_EFFECTS_AUTO",
            hud_env: "MISTER_SPRITE_EFFECTS_HUD",
            trace_env: "MISTER_SPRITE_EFFECTS_TRACE",
            trace_header: TRACE_HEADER,
            all_effects: SpriteEffectKind::all(),
            parse_effect: SpriteEffectKind::parse,
            label_effect: SpriteEffectKind::label,
            default_effect: SpriteEffectKind::SpriteZoomTowardCamera,
            synthetic_min: 8,
            synthetic_max: 24,
            synthetic_images: synthetic_sprite_images,
            new_state: SpriteEffectRenderState::new,
            render: render_sprite_effect_frame,
            draw_us: |stats| (*stats).draw_us(),
            write_trace_row,
            controller_exit_grace: Some(CONTROLLER_EXIT_GRACE),
        },
    );
}

fn write_trace_row(
    trace: &mut File,
    row: &EffectTraceRow<SpriteEffectKind>,
    stats: &SpriteEffectFrameStats,
) {
    let _ = writeln!(
        trace,
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
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
        stats.sprite_count,
        stats.sprite_pixels,
        stats.particle_count,
        stats.flicker_skip_count,
        row.vsync.source.label(),
        row.vsync.period_us,
        row.vsync.miss_streak
    );
}
