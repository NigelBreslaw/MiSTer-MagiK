// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) fn run_bench_frame(
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
    _f: &mut Fpga,
    target: &mut UiFrameTarget,
    window: &Rc<MisterSoftwareWindow>,
    frame_order: FrameOrder,
    animation_clock: &AnimationClock,
    pacer: &mut VsyncPacer,
    present_timing: PresentTiming,
) -> FrameSample {
    let frame_start = Instant::now();
    let t0 = Instant::now();
    let mut this_rect: Option<DirtyRect> = None;

    match frame_order {
        FrameOrder::RenderThenVsync => {
            update_slint_animations(animation_clock);
            let t1 = Instant::now();
            window.draw_if_needed(|renderer| {
                let region = target.render(renderer);
                this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
            });
            let t2 = Instant::now();
            let pace = pacer.wait();
            let vsync_done = Instant::now();
            present_timing.wait_until_present_time(vsync_done);
            let t3 = Instant::now();
            let mut copy_us = 0;
            let mut present_rect = None;
            let rows = match this_rect {
                Some(rect) => {
                    let c0 = Instant::now();
                    let copied = copy_cached_rect_565(disp, target.cached_frame_view(), rect);
                    copy_us += c0.elapsed().as_micros() as u64;
                    present_rect = copied.map(frame_rect);
                    copied.map_or(0, DirtyRect::rows)
                }
                None => 0,
            };
            FrameSample {
                prepare_us: 0,
                anim_us: (t1 - t0).as_micros() as u64,
                slint_render_us: (t2 - t1).as_micros() as u64,
                custom_draw_us: 0,
                vsync_us: (t3 - t2).as_micros() as u64,
                fb_present_us: copy_us,
                cached_present_us: copy_us,
                arcade_list_present_us: 0,
                rows,
                present_rect,
                wall_us: frame_start.elapsed().as_micros() as u64,
                vsync_source: pace.source,
                vsync_period_us: pace.period_us,
                vsync_miss_streak: pace.miss_streak,
                video: crate::frame_profile::VideoFrameProfile::default(),
            }
        }
        FrameOrder::VsyncThenRender => {
            let pace = pacer.wait();
            let vsync_done = Instant::now();
            let t1 = vsync_done;
            update_slint_animations(animation_clock);
            let t2 = Instant::now();
            window.draw_if_needed(|renderer| {
                let region = target.render(renderer);
                this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
            });
            present_timing.wait_until_present_time(vsync_done);
            let t3 = Instant::now();
            let mut copy_us = 0;
            let mut present_rect = None;
            let rows = match this_rect {
                Some(rect) => {
                    let c0 = Instant::now();
                    let copied = copy_cached_rect_565(disp, target.cached_frame_view(), rect);
                    copy_us += c0.elapsed().as_micros() as u64;
                    present_rect = copied.map(frame_rect);
                    copied.map_or(0, DirtyRect::rows)
                }
                None => 0,
            };
            FrameSample {
                prepare_us: 0,
                anim_us: (t2 - t1).as_micros() as u64,
                slint_render_us: (t3 - t2).as_micros() as u64,
                custom_draw_us: 0,
                vsync_us: (t1 - t0).as_micros() as u64,
                fb_present_us: copy_us,
                cached_present_us: copy_us,
                arcade_list_present_us: 0,
                rows,
                present_rect,
                wall_us: frame_start.elapsed().as_micros() as u64,
                vsync_source: pace.source,
                vsync_period_us: pace.period_us,
                vsync_miss_streak: pace.miss_streak,
                video: crate::frame_profile::VideoFrameProfile::default(),
            }
        }
    }
}

pub(super) fn run_frame_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
    f: &mut Fpga,
    window: &Rc<MisterSoftwareWindow>,
    target: &mut UiFrameTarget,
    animation_clock: &AnimationClock,
) {
    let start = Instant::now();
    let mut frames = 0u64;
    let mut profiler = FrameProfiler::from_env();
    let cpu = cpu_profile::start();
    let profile_on = profiler.enabled();

    // Legacy 1 Hz line (no anim column) when frame profiling is disabled.
    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut render_us = 0u128;
    let mut vsync_us = 0u128;
    let mut copy_us = 0u128;
    let mut copy_rows_acc = 0u128;
    let configured_frame_order = FrameOrder::from_env();
    let frame_order = configured_frame_order;
    let present_timing = PresentTiming::from_env();
    let mut pacer = VsyncPacer::from_env();

    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    crate::ui_logln!(
        "bench scene running {label} (vsync-locked, dirty-row copy, frame-order={}, render-mode={}, animation-clock={}, present-delay-us={})...",
        frame_order.label(),
        target.label(),
        animation_clock.label(),
        present_timing.delay_us()
    );
    crate::ui_logln!(
        "slint-render-mode={} frame-order={} requested-frame-order={}",
        target.label(),
        frame_order.label(),
        configured_frame_order.label()
    );
    while secs == 0 || start.elapsed().as_secs() < secs {
        let sample = run_bench_frame(
            ui,
            disp,
            f,
            target,
            window,
            frame_order,
            animation_clock,
            &mut pacer,
            present_timing,
        );
        frames += 1;

        if profiler.enabled() {
            profiler.record(sample);
        } else {
            fps_frames += 1;
            render_us += sample.slint_render_us as u128;
            vsync_us += sample.vsync_us as u128;
            copy_us += sample.fb_present_us as u128;
            copy_rows_acc += sample.rows as u128;
            if fps_window_start.elapsed().as_millis() >= 1000 {
                let nn = fps_frames.max(1) as u128;
                crate::ui_logln!(
                    "  fps ~ {fps_frames}  | slint-render {}us  vsync-wait {}us  fb-present {}us ({} logical rows avg)  vsync hits={} timeouts={} fallback={} errors={} hz={:.2}",
                    render_us / nn,
                    vsync_us / nn,
                    copy_us / nn,
                    copy_rows_acc / nn,
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
                copy_rows_acc = 0;
                fps_window_start = Instant::now();
            }
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    crate::ui_logln!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if profile_on {
        profiler.finish();
    }
    if let Err(e) = cpu_profile::finish(cpu) {
        crate::ui_errln!("{e}");
    }
}
