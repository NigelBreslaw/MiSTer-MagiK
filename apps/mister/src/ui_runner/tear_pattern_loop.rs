// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) fn run_tear_pattern_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
    window: &Rc<MisterSoftwareWindow>,
    animation_clock: &AnimationClock,
) {
    let mut cached = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];
    let start = Instant::now();
    let mut frames = 0u64;
    let mut pacer = VsyncPacer::from_env();
    let frame_order = FrameOrder::from_env();
    let present_timing = PresentTiming::from_env();
    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut render_us = 0u128;
    let mut vsync_us = 0u128;
    let mut copy_us = 0u128;
    let mut rows_acc = 0u128;
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    crate::ui_logln!(
        "tear_pattern running {label} (horizontal motion, vsync-locked, dirty-row copy, frame-order={}, present-delay-us={})...",
        frame_order.label(),
        present_timing.delay_us()
    );

    while secs == 0 || start.elapsed().as_secs() < secs {
        let mut this_rect: Option<DirtyRect> = None;
        let (slint_render_us, frame_vsync_us) = match frame_order {
            FrameOrder::RenderThenVsync => {
                update_slint_animations(animation_clock);
                let t1 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let region = renderer.render(&mut cached, ui.render_w());
                    this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                });
                let t2 = Instant::now();
                pacer.wait();
                let vsync_done = Instant::now();
                present_timing.wait_until_present_time(vsync_done);
                let t3 = Instant::now();
                ((t2 - t1).as_micros() as u64, (t3 - t2).as_micros() as u64)
            }
            FrameOrder::VsyncThenRender => {
                let t0 = Instant::now();
                pacer.wait();
                let vsync_done = Instant::now();
                update_slint_animations(animation_clock);
                let t1 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let region = renderer.render(&mut cached, ui.render_w());
                    this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                });
                let t2 = Instant::now();
                present_timing.wait_until_present_time(vsync_done);
                let t3 = Instant::now();
                (
                    (t2 - t1).as_micros() as u64,
                    (vsync_done - t0).as_micros() as u64 + (t3 - t2).as_micros() as u64,
                )
            }
        };
        let rows = if let Some(rect) = this_rect {
            let c0 = Instant::now();
            let copied = copy_cached_rect_565(
                disp,
                CachedFrameView::new(&cached, ui.render_w(), ui.render_h()),
                rect,
            );
            copy_us += c0.elapsed().as_micros();
            copied.map_or(0, |rect| rect.y1.saturating_sub(rect.y0))
        } else {
            0
        };

        frames += 1;
        fps_frames += 1;
        render_us += slint_render_us as u128;
        vsync_us += frame_vsync_us as u128;
        rows_acc += rows as u128;
        if fps_window_start.elapsed().as_millis() >= 1000 {
            let nn = fps_frames.max(1) as u128;
            crate::ui_logln!(
                "  fps ~ {fps_frames}  | slint-render {}us  vsync-wait {}us  fb-present {}us ({} logical rows avg)  vsync hits={} timeouts={} fallback={} errors={} hz={:.2}",
                render_us / nn,
                vsync_us / nn,
                copy_us / nn,
                rows_acc / nn,
                pacer.hits(),
                pacer.timeouts(),
                pacer.fallback_frames(),
                pacer.errors(),
                1_000_000.0 / pacer.period_us() as f64
            );
            fps_frames = 0;
            render_us = 0;
            vsync_us = 0;
            copy_us = 0;
            rows_acc = 0;
            fps_window_start = Instant::now();
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    crate::ui_logln!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
}
