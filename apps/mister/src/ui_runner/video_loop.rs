// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

use crate::spring_animation::{SpringAnimation, SpringConfiguration};

pub(super) const VIDEO_IMAGE_RECT: DirtyRect = DirtyRect {
    x0: 40,
    y0: 30,
    x1: 680,
    y1: 510,
};

const VIDEO_SOURCE_W: usize = crate::video_player::CANONICAL_VIDEO_WIDTH as usize;
const VIDEO_SOURCE_H: usize = crate::video_player::CANONICAL_VIDEO_HEIGHT as usize;
const VIDEO_SCALE_ANIMATION_RESPONSE: Duration = Duration::from_millis(200);

fn video_scale_spring_configuration() -> SpringConfiguration {
    SpringConfiguration::smooth_with_response(VIDEO_SCALE_ANIMATION_RESPONSE)
}

pub(super) fn video_frame_rect(width: usize, height: usize) -> DirtyRect {
    DirtyRect {
        x0: VIDEO_IMAGE_RECT.x0,
        y0: VIDEO_IMAGE_RECT.y0,
        x1: (VIDEO_IMAGE_RECT.x0 + width).min(VIDEO_IMAGE_RECT.x1),
        y1: (VIDEO_IMAGE_RECT.y0 + height).min(VIDEO_IMAGE_RECT.y1),
    }
}

#[derive(Clone, Copy, Debug)]
struct VideoSizeAnimation {
    spring: SpringAnimation,
    updated_at: Duration,
}

impl VideoSizeAnimation {
    fn new(doubled: bool) -> Self {
        let progress = if doubled { 1.0 } else { 0.0 };
        Self {
            spring: SpringAnimation::new(progress, video_scale_spring_configuration()),
            updated_at: Duration::ZERO,
        }
    }

    fn update(&mut self, now: Duration) -> bool {
        let was_active = self.is_active();
        let elapsed = now.saturating_sub(self.updated_at);
        self.updated_at = now;
        if was_active {
            self.spring.advance(elapsed);
        }
        was_active && !self.is_active()
    }

    fn toggle(&mut self, now: Duration) {
        self.update(now);
        self.spring.set_target(if self.spring.target() >= 0.5 {
            0.0
        } else {
            1.0
        });
    }

    fn is_active(self) -> bool {
        !self.spring.is_settled()
    }

    fn dimensions(self) -> (usize, usize) {
        let progress = self.spring.value().clamp(0.0, 1.0);
        let interpolated = (VIDEO_SOURCE_W as f64 * (1.0 + progress)).round() as usize;
        let width = ((interpolated + 2) / 4) * 4;
        let height = width * VIDEO_SOURCE_H / VIDEO_SOURCE_W;
        (width.clamp(VIDEO_SOURCE_W, VIDEO_SOURCE_W * 2), height)
    }

    fn target_dimensions(self) -> (usize, usize) {
        if self.spring.target() >= 0.5 {
            (VIDEO_SOURCE_W * 2, VIDEO_SOURCE_H * 2)
        } else {
            (VIDEO_SOURCE_W, VIDEO_SOURCE_H)
        }
    }
}

#[derive(Default)]
struct VideoButtonEdge {
    previous_a: bool,
}

struct VideoAutoToggle {
    interval: Option<Duration>,
    next: Duration,
}

impl VideoAutoToggle {
    fn from_env() -> Self {
        let interval = std::env::var("MISTER_VIDEO_AUTO_TOGGLE_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|millis| *millis > 0)
            .map(Duration::from_millis);
        Self {
            next: interval.unwrap_or(Duration::ZERO),
            interval,
        }
    }

    fn due(&mut self, now: Duration) -> bool {
        let Some(interval) = self.interval else {
            return false;
        };
        if now < self.next {
            return false;
        }
        self.next += interval;
        while self.next <= now {
            self.next += interval;
        }
        true
    }
}

impl VideoButtonEdge {
    fn a_pressed(&mut self, current_a: bool) -> bool {
        let pressed = current_a && !self.previous_a;
        self.previous_a = current_a;
        pressed
    }
}

struct VideoRgb565Scaler {
    pixels: Vec<u16>,
    x_map: Vec<usize>,
    y_map: Vec<usize>,
    mapped_source_width: usize,
    mapped_source_height: usize,
    mapped_width: usize,
    mapped_height: usize,
}

impl Default for VideoRgb565Scaler {
    fn default() -> Self {
        Self {
            pixels: Vec::with_capacity(VIDEO_SOURCE_W * 2 * VIDEO_SOURCE_H * 2),
            x_map: Vec::with_capacity(VIDEO_SOURCE_W * 2),
            y_map: Vec::with_capacity(VIDEO_SOURCE_H * 2),
            mapped_source_width: 0,
            mapped_source_height: 0,
            mapped_width: 0,
            mapped_height: 0,
        }
    }
}

impl VideoRgb565Scaler {
    fn scale<'a>(
        &'a mut self,
        frame: &'a crate::video_player::VideoRgb565Frame,
        width: usize,
        height: usize,
    ) -> &'a [u16] {
        let source_w = frame.width as usize;
        let source_h = frame.height as usize;
        debug_assert_eq!(frame.pixels.len(), source_w * source_h);
        if width == source_w && height == source_h {
            return &frame.pixels;
        }

        self.pixels.resize(width * height, 0);
        if width == source_w * 2 && height == source_h * 2 {
            for source_y in 0..source_h {
                let source = &frame.pixels[source_y * source_w..(source_y + 1) * source_w];
                let first_row = source_y * 2 * width;
                let target = &mut self.pixels[first_row..first_row + width];
                for (pair, pixel) in target.chunks_exact_mut(2).zip(source.iter().copied()) {
                    pair.fill(pixel);
                }
                self.pixels
                    .copy_within(first_row..first_row + width, first_row + width);
            }
            return &self.pixels;
        }

        if self.mapped_source_width != source_w
            || self.mapped_source_height != source_h
            || self.mapped_width != width
            || self.mapped_height != height
        {
            self.x_map.resize(width, 0);
            for (target_x, source_x) in self.x_map.iter_mut().enumerate() {
                *source_x = target_x * source_w / width;
            }
            self.y_map.resize(height, 0);
            for (target_y, source_y) in self.y_map.iter_mut().enumerate() {
                *source_y = target_y * source_h / height;
            }
            self.mapped_source_width = source_w;
            self.mapped_source_height = source_h;
            self.mapped_width = width;
            self.mapped_height = height;
        }
        let mut previous_source_y = None;
        for target_y in 0..height {
            let source_y = self.y_map[target_y];
            let target_row = target_y * width;
            if previous_source_y == Some(source_y) {
                self.pixels
                    .copy_within(target_row - width..target_row, target_row);
            } else {
                let source_row = source_y * source_w;
                for target_x in 0..width {
                    self.pixels[target_row + target_x] =
                        frame.pixels[source_row + self.x_map[target_x]];
                }
            }
            previous_source_y = Some(source_y);
        }
        &self.pixels
    }
}

fn rgb565_words_as_pixels(words: &[u16]) -> &[Rgb565Pixel] {
    debug_assert_eq!(
        std::mem::size_of::<Rgb565Pixel>(),
        std::mem::size_of::<u16>()
    );
    debug_assert_eq!(
        std::mem::align_of::<Rgb565Pixel>(),
        std::mem::align_of::<u16>()
    );
    // SAFETY: Rgb565Pixel is layout-compatible with u16; the size/alignment
    // assumptions are asserted above before casting the shared slice.
    unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<Rgb565Pixel>(), words.len()) }
}

fn present_video_frame_direct(
    disp: &mut MappedRgb565Framebuffer,
    pixels: &[u16],
    stride: usize,
    rect: DirtyRect,
) {
    let src = rgb565_words_as_pixels(pixels);
    if let Err(e) = disp.present_rect_565_strided(
        rect.x0,
        rect.y0,
        rect.width(),
        rect.rows() as usize,
        src,
        stride,
        0,
        0,
    ) {
        crate::ui_errln!("framebuffer present video direct failed: {e}");
    }
}

fn present_direct_video_frame(
    disp: &mut MappedRgb565Framebuffer,
    ui: &UiDisplay,
    cached: &[Rgb565Pixel],
    pixels: &[u16],
    width: usize,
    height: usize,
    dirty: Option<DirtyRect>,
    previous_video_rect: Option<DirtyRect>,
    geometry_changed: bool,
) -> (u32, Option<DirtyRect>) {
    let video_rect = video_frame_rect(width, height);
    let mut rows = 0u32;
    let mut copied_rect = None;

    let mut restore = |background_dirty: DirtyRect| {
        if let Some(copied) = copy_cached_rect_565(
            disp,
            CachedFrameView::new(cached, ui.render_w(), ui.render_h()),
            background_dirty,
        ) {
            rows = rows.saturating_add(copied.rows());
            copied_rect = Some(copied_rect.map_or(copied, |rect: DirtyRect| rect.union(copied)));
        }
    };
    if let Some(dirty) = dirty {
        restore(dirty);
    }
    if geometry_changed {
        for exposed in previous_video_rect
            .map(|previous| video_exposed_background_rects(previous, video_rect))
            .unwrap_or([None, None])
            .into_iter()
            .flatten()
        {
            restore(exposed);
        }
    }

    present_video_frame_direct(disp, pixels, width, video_rect);
    rows = rows.saturating_add(video_rect.rows());
    copied_rect = Some(copied_rect.map_or(video_rect, |rect: DirtyRect| rect.union(video_rect)));
    (rows, copied_rect)
}

fn video_exposed_background_rects(
    previous: DirtyRect,
    current: DirtyRect,
) -> [Option<DirtyRect>; 2] {
    let right = (current.x1 < previous.x1).then_some(DirtyRect {
        x0: current.x1,
        y0: previous.y0,
        x1: previous.x1,
        y1: previous.y1,
    });
    let bottom = (current.y1 < previous.y1).then_some(DirtyRect {
        x0: previous.x0,
        y0: current.y1,
        x1: current.x1.min(previous.x1),
        y1: previous.y1,
    });
    [right, bottom]
}

#[cfg(test)]
fn exposed_background_union(previous: DirtyRect, current: DirtyRect) -> Option<DirtyRect> {
    video_exposed_background_rects(previous, current)
        .into_iter()
        .flatten()
        .reduce(DirtyRect::union)
}

#[cfg(test)]
fn growing_video_exposes_no_background(previous: DirtyRect, current: DirtyRect) -> bool {
    if current.x1 >= previous.x1 && current.y1 >= previous.y1 {
        exposed_background_union(previous, current).is_none()
    } else {
        false
    }
}

#[derive(Default)]
pub(super) struct VideoFramePhases {
    frame_updated: bool,
    video_decode_us: u64,
    video_scale_us: u64,
    recv_us: u64,
    image_us: u64,
    blit_us: u64,
    audio_decode_us: u64,
    audio_resample_us: u64,
    audio_write_us: u64,
    audio_buffer_frames: u32,
    queue_depth: u32,
    missed_deadlines: u32,
    audio_underrun: bool,
}

#[derive(Default)]
pub(super) struct VideoWindowTotals {
    frames: u64,
    video_frames: u64,
    video_decode_us: u128,
    video_scale_us: u128,
    recv_us: u128,
    image_us: u128,
    blit_us: u128,
    audio_decode_us: u128,
    audio_resample_us: u128,
    audio_write_us: u128,
    audio_underruns: u64,
    missed_deadlines: u64,
    render_us: u128,
    vsync_us: u128,
    copy_us: u128,
    copy_rows: u128,
    copy_px: u128,
}

impl VideoWindowTotals {
    pub(super) fn record(
        &mut self,
        phases: VideoFramePhases,
        sample: FrameSample,
        copy_rect: Option<DirtyRect>,
    ) {
        self.frames += 1;
        if phases.frame_updated {
            self.video_frames += 1;
        }
        self.video_decode_us += phases.video_decode_us as u128;
        self.video_scale_us += phases.video_scale_us as u128;
        self.recv_us += phases.recv_us as u128;
        self.image_us += phases.image_us as u128;
        self.blit_us += phases.blit_us as u128;
        self.audio_decode_us += phases.audio_decode_us as u128;
        self.audio_resample_us += phases.audio_resample_us as u128;
        self.audio_write_us += phases.audio_write_us as u128;
        if phases.audio_underrun {
            self.audio_underruns += 1;
        }
        self.missed_deadlines += u64::from(phases.missed_deadlines);
        self.render_us += sample.slint_render_us as u128;
        self.vsync_us += sample.vsync_us as u128;
        self.copy_us += sample.fb_present_us as u128;
        self.copy_rows += sample.rows as u128;
        if let Some(rect) = copy_rect {
            self.copy_px += rect.width() as u128 * rect.rows() as u128;
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn avg_per_frame(value: u128, frames: u64) -> u128 {
        if frames == 0 {
            0
        } else {
            value / frames as u128
        }
    }

    pub(super) fn avg_per_video_frame(value: u128, video_frames: u64) -> u128 {
        if video_frames == 0 {
            0
        } else {
            value / video_frames as u128
        }
    }
}

pub(super) fn run_video_playback_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
    window: &Rc<MisterSoftwareWindow>,
    mut pad: PadPool,
    _app: slint_ui::video_playback::VideoPlayback,
    animation_clock: &AnimationClock,
    profiles: &mister_magik_fb::process_config::ProfileProcessConfig,
) {
    let initial_doubled = match crate::video_player::video_starts_doubled_from_env() {
        Ok(doubled) => doubled,
        Err(e) => {
            crate::ui_errln!("video_playback: {e}");
            std::process::exit(2);
        }
    };
    let paths = match crate::video_player::video_paths_from_env() {
        Ok(paths) => paths,
        Err(e) => {
            crate::ui_errln!("video_playback: {e}");
            std::process::exit(1);
        }
    };
    let playlist_label = if paths.len() == 1 {
        paths[0].clone()
    } else {
        format!("{} files starting {}", paths.len(), paths[0])
    };
    let frame_worker = match crate::video_player::VideoFrameWorker::start(paths.clone()) {
        Ok(worker) => worker,
        Err(e) => {
            crate::ui_errln!("video_playback: {e}");
            std::process::exit(1);
        }
    };
    let audio_writer = match AudioWriteWorker::start() {
        Ok(worker) => worker,
        Err(e) => {
            crate::ui_errln!("video_playback audio: {e}");
            std::process::exit(1);
        }
    };

    let mut cached = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];
    let start = Instant::now();
    let mut next_video_at = Duration::ZERO;
    let frame_interval = frame_worker.frame_interval();
    let mut frames = 0u64;
    let mut profiler = FrameProfiler::from_config(profiles.frame().clone());
    let cpu = cpu_profile::start(profiles.cpu());
    let profile_on = profiler.enabled();
    let frame_order = if std::env::var_os("MISTER_FRAME_ORDER").is_some() {
        FrameOrder::from_env()
    } else {
        FrameOrder::VsyncThenRender
    };
    let mut pacer = VsyncPacer::from_env();

    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut video_totals = VideoWindowTotals::default();
    let mut audio_stats = AudioWindowStats::default();
    let mut video_cpu = VideoCpuSampler::new();
    let mut size_animation = VideoSizeAnimation::new(initial_doubled);
    let mut button_edge = VideoButtonEdge::default();
    let mut auto_toggle = VideoAutoToggle::from_env();
    let mut retained_frame: Option<crate::video_player::VideoRgb565Frame> = None;
    let mut scaler = VideoRgb565Scaler::default();
    let mut previous_video_rect: Option<DirtyRect> = None;
    let mut playback_started = false;

    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    crate::ui_logln!(
        "video_playback running {label} playlist={playlist_label} frame-order={} animation-clock={} video-render-mode=direct-blit",
        frame_order.label(),
        animation_clock.label()
    );
    crate::ui_logln!("video_render_mode=direct-blit");
    crate::ui_logln!(
        "video_controls queue_depth=2 scale={} a=toggle-320x240-640x480 animation=spring-smooth response_ms={} profile={}",
        std::env::var("MISTER_VIDEO_SCALE").unwrap_or_else(|_| "source".into()),
        VIDEO_SCALE_ANIMATION_RESPONSE.as_millis(),
        std::env::var("MISTER_VIDEO_PROFILE")
            .or_else(|_| std::env::var("MISTER_PROFILE"))
            .unwrap_or_else(|_| "off".into())
    );
    crate::ui_logln!(
        "video_dirty_clip=on rect={}x{}+{},{}",
        VIDEO_IMAGE_RECT.width(),
        VIDEO_IMAGE_RECT.rows(),
        VIDEO_IMAGE_RECT.x0,
        VIDEO_IMAGE_RECT.y0
    );

    while secs == 0 || start.elapsed().as_secs() < secs {
        if !drain_audio_write_results(&audio_writer, &frame_worker, &mut audio_stats) {
            break;
        }
        let frame_start = Instant::now();
        let t0 = Instant::now();
        let mut this_rect: Option<DirtyRect> = None;
        let mut phases = VideoFramePhases::default();
        phases.queue_depth = frame_worker.queue_depth();
        let mut video_profile = VideoFrameProfile {
            video_queue_depth: phases.queue_depth,
            ..Default::default()
        };
        let now = start.elapsed();
        let _ = pad.poll();
        let controller_toggle = button_edge.a_pressed(pad.state().btn_a);
        let automatic_toggle = auto_toggle.due(now);
        if controller_toggle || automatic_toggle {
            size_animation.toggle(now);
            let (width, height) = size_animation.dimensions();
            let (target_width, target_height) = size_animation.target_dimensions();
            crate::ui_logln!(
                "video_size_transition trigger={} start={}x{} target={}x{} animation=spring-smooth response_ms={}",
                if controller_toggle {
                    "controller-a"
                } else {
                    "automatic"
                },
                width,
                height,
                target_width,
                target_height,
                VIDEO_SCALE_ANIMATION_RESPONSE.as_millis()
            );
        }
        if size_animation.update(now) {
            let (width, height) = size_animation.dimensions();
            crate::ui_logln!("video_size_transition complete={}x{}", width, height);
        }
        let (presentation_width, presentation_height) = size_animation.dimensions();
        let presentation_rect = video_frame_rect(presentation_width, presentation_height);
        let geometry_changed = previous_video_rect != Some(presentation_rect);
        video_profile.video_present_width = presentation_width as u32;
        video_profile.video_present_height = presentation_height as u32;
        video_profile.video_size_animating = size_animation.is_active();

        match frame_order {
            FrameOrder::RenderThenVsync => {
                update_slint_animations(animation_clock);
                let now = start.elapsed();
                if now >= next_video_at {
                    let playback_was_started = playback_started;
                    let recv_t0 = Instant::now();
                    match frame_worker.try_recv() {
                        Ok(Some(frame)) => {
                            playback_started = true;
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                            let crate::video_player::PlaybackFrame {
                                frame,
                                audio,
                                audio_requested_frames,
                                loop_count,
                                metrics,
                            } = frame;
                            phases.frame_updated = true;
                            phases.video_decode_us = metrics.video_decode_us;
                            phases.video_scale_us = metrics.video_scale_us;
                            phases.audio_decode_us = metrics.audio_decode_us;
                            phases.audio_resample_us = metrics.audio_resample_us;
                            phases.audio_buffer_frames = metrics.audio_buffer_frames;
                            phases.queue_depth = frame_worker.queue_depth();
                            video_profile = VideoFrameProfile {
                                video_decode_us: phases.video_decode_us,
                                video_scale_us: phases.video_scale_us,
                                video_recv_us: phases.recv_us,
                                video_frame_updated: true,
                                video_queue_depth: phases.queue_depth,
                                audio_decode_us: phases.audio_decode_us,
                                audio_resample_us: phases.audio_resample_us,
                                audio_buffer_frames: phases.audio_buffer_frames,
                                video_file: metrics.video_file,
                                video_width: metrics.video_width,
                                video_height: metrics.video_height,
                                video_codec: metrics.video_codec,
                                audio_codec: metrics.audio_codec,
                                ..Default::default()
                            };
                            if let Some(previous) = retained_frame.replace(frame) {
                                frame_worker.recycle_pixels(previous.pixels);
                            }
                            if !enqueue_audio_write(
                                &audio_writer,
                                &frame_worker,
                                &mut audio_stats,
                                audio,
                                audio_requested_frames,
                                loop_count,
                                &mut phases,
                                &mut video_profile,
                            ) {
                                break;
                            }
                        }
                        Ok(None) => {
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                            if playback_was_started {
                                phases.missed_deadlines = phases.missed_deadlines.saturating_add(1);
                            }
                        }
                        Err(e) => {
                            crate::ui_errln!("video_playback: {e}");
                            break;
                        }
                    }
                    if playback_started && !playback_was_started {
                        next_video_at = now + frame_interval;
                    } else if playback_started {
                        next_video_at += frame_interval;
                        while next_video_at < now {
                            next_video_at += frame_interval;
                            phases.missed_deadlines = phases.missed_deadlines.saturating_add(1);
                        }
                    }
                    video_profile.video_missed_deadlines = phases.missed_deadlines;
                }
                let t1 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let region = renderer.render(&mut cached, ui.render_w());
                    this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                });
                let t2 = Instant::now();
                let pace = pacer.wait();
                let t3 = Instant::now();
                let mut copied_rect = None;
                let should_present_video = retained_frame.is_some()
                    && (phases.frame_updated || geometry_changed || this_rect.is_some());
                let rows = if should_present_video {
                    let frame = retained_frame.as_ref().expect("checked above");
                    let scale_started = Instant::now();
                    let pixels = scaler.scale(frame, presentation_width, presentation_height);
                    phases.video_scale_us = phases
                        .video_scale_us
                        .saturating_add(scale_started.elapsed().as_micros() as u64);
                    let (rows, rect) = present_direct_video_frame(
                        disp,
                        ui,
                        &cached,
                        pixels,
                        presentation_width,
                        presentation_height,
                        this_rect,
                        previous_video_rect,
                        geometry_changed,
                    );
                    copied_rect = rect;
                    previous_video_rect = Some(presentation_rect);
                    rows
                } else if let Some(rect) = this_rect {
                    let copied = copy_cached_rect_565(
                        disp,
                        CachedFrameView::new(&cached, ui.render_w(), ui.render_h()),
                        rect,
                    );
                    copied_rect = copied;
                    copied.map_or(0, DirtyRect::rows)
                } else {
                    0
                };
                video_profile.video_scale_us = phases.video_scale_us;
                video_profile.video_present_width = presentation_width as u32;
                video_profile.video_present_height = presentation_height as u32;
                video_profile.video_size_animating = size_animation.is_active();
                video_profile.video_missed_deadlines = phases.missed_deadlines;
                let t4 = Instant::now();
                let sample = FrameSample {
                    prepare_us: 0,
                    anim_us: (t1 - t0).as_micros() as u64,
                    slint_render_us: (t2 - t1).as_micros() as u64,
                    custom_draw_us: 0,
                    vsync_us: (t3 - t2).as_micros() as u64,
                    fb_present_us: (t4 - t3).as_micros() as u64,
                    cached_present_us: (t4 - t3).as_micros() as u64,
                    arcade_list_present_us: 0,
                    rows,
                    present_rect: copied_rect.map(frame_rect),
                    wall_us: frame_start.elapsed().as_micros() as u64,
                    vsync_source: pace.source,
                    vsync_period_us: pace.period_us,
                    vsync_miss_streak: pace.miss_streak,
                    video: video_profile,
                };
                record_video_sample(
                    phases,
                    sample,
                    copied_rect,
                    &mut profiler,
                    &mut fps_window_start,
                    &mut fps_frames,
                    &mut video_totals,
                    &mut audio_stats,
                    &mut video_cpu,
                );
            }
            FrameOrder::VsyncThenRender => {
                let pace = pacer.wait();
                let t1 = Instant::now();
                update_slint_animations(animation_clock);
                let now = start.elapsed();
                if now >= next_video_at {
                    let playback_was_started = playback_started;
                    let recv_t0 = Instant::now();
                    match frame_worker.try_recv() {
                        Ok(Some(frame)) => {
                            playback_started = true;
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                            let crate::video_player::PlaybackFrame {
                                frame,
                                audio,
                                audio_requested_frames,
                                loop_count,
                                metrics,
                            } = frame;
                            phases.frame_updated = true;
                            phases.video_decode_us = metrics.video_decode_us;
                            phases.video_scale_us = metrics.video_scale_us;
                            phases.audio_decode_us = metrics.audio_decode_us;
                            phases.audio_resample_us = metrics.audio_resample_us;
                            phases.audio_buffer_frames = metrics.audio_buffer_frames;
                            phases.queue_depth = frame_worker.queue_depth();
                            video_profile = VideoFrameProfile {
                                video_decode_us: phases.video_decode_us,
                                video_scale_us: phases.video_scale_us,
                                video_recv_us: phases.recv_us,
                                video_frame_updated: true,
                                video_queue_depth: phases.queue_depth,
                                audio_decode_us: phases.audio_decode_us,
                                audio_resample_us: phases.audio_resample_us,
                                audio_buffer_frames: phases.audio_buffer_frames,
                                video_file: metrics.video_file,
                                video_width: metrics.video_width,
                                video_height: metrics.video_height,
                                video_codec: metrics.video_codec,
                                audio_codec: metrics.audio_codec,
                                ..Default::default()
                            };
                            if let Some(previous) = retained_frame.replace(frame) {
                                frame_worker.recycle_pixels(previous.pixels);
                            }
                            if !enqueue_audio_write(
                                &audio_writer,
                                &frame_worker,
                                &mut audio_stats,
                                audio,
                                audio_requested_frames,
                                loop_count,
                                &mut phases,
                                &mut video_profile,
                            ) {
                                break;
                            }
                        }
                        Ok(None) => {
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                            if playback_was_started {
                                phases.missed_deadlines = phases.missed_deadlines.saturating_add(1);
                            }
                        }
                        Err(e) => {
                            crate::ui_errln!("video_playback: {e}");
                            break;
                        }
                    }
                    if playback_started && !playback_was_started {
                        next_video_at = now + frame_interval;
                    } else if playback_started {
                        next_video_at += frame_interval;
                        while next_video_at < now {
                            next_video_at += frame_interval;
                            phases.missed_deadlines = phases.missed_deadlines.saturating_add(1);
                        }
                    }
                    video_profile.video_missed_deadlines = phases.missed_deadlines;
                }
                let t2 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let region = renderer.render(&mut cached, ui.render_w());
                    this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                });
                let t3 = Instant::now();
                let mut copied_rect = None;
                let should_present_video = retained_frame.is_some()
                    && (phases.frame_updated || geometry_changed || this_rect.is_some());
                let rows = if should_present_video {
                    let frame = retained_frame.as_ref().expect("checked above");
                    let scale_started = Instant::now();
                    let pixels = scaler.scale(frame, presentation_width, presentation_height);
                    phases.video_scale_us = phases
                        .video_scale_us
                        .saturating_add(scale_started.elapsed().as_micros() as u64);
                    let (rows, rect) = present_direct_video_frame(
                        disp,
                        ui,
                        &cached,
                        pixels,
                        presentation_width,
                        presentation_height,
                        this_rect,
                        previous_video_rect,
                        geometry_changed,
                    );
                    copied_rect = rect;
                    previous_video_rect = Some(presentation_rect);
                    rows
                } else if let Some(rect) = this_rect {
                    let copied = copy_cached_rect_565(
                        disp,
                        CachedFrameView::new(&cached, ui.render_w(), ui.render_h()),
                        rect,
                    );
                    copied_rect = copied;
                    copied.map_or(0, DirtyRect::rows)
                } else {
                    0
                };
                video_profile.video_scale_us = phases.video_scale_us;
                video_profile.video_present_width = presentation_width as u32;
                video_profile.video_present_height = presentation_height as u32;
                video_profile.video_size_animating = size_animation.is_active();
                video_profile.video_missed_deadlines = phases.missed_deadlines;
                let t4 = Instant::now();
                let sample = FrameSample {
                    prepare_us: 0,
                    anim_us: (t2 - t1).as_micros() as u64,
                    slint_render_us: (t3 - t2).as_micros() as u64,
                    custom_draw_us: 0,
                    vsync_us: (t1 - t0).as_micros() as u64,
                    fb_present_us: (t4 - t3).as_micros() as u64,
                    cached_present_us: (t4 - t3).as_micros() as u64,
                    arcade_list_present_us: 0,
                    rows,
                    present_rect: copied_rect.map(frame_rect),
                    wall_us: frame_start.elapsed().as_micros() as u64,
                    vsync_source: pace.source,
                    vsync_period_us: pace.period_us,
                    vsync_miss_streak: pace.miss_streak,
                    video: video_profile,
                };
                record_video_sample(
                    phases,
                    sample,
                    copied_rect,
                    &mut profiler,
                    &mut fps_window_start,
                    &mut fps_frames,
                    &mut video_totals,
                    &mut audio_stats,
                    &mut video_cpu,
                );
            }
        }
        frames += 1;
    }

    if let Some(frame) = retained_frame.take() {
        frame_worker.recycle_pixels(frame.pixels);
    }
    finish_audio_writer(audio_writer, &frame_worker, &mut audio_stats);
    let elapsed = start.elapsed().as_secs_f64();
    crate::ui_logln!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Ok(status) = crate::mr_audio::read_status() {
        crate::ui_log!("video_playback audio status: {status}");
    }
    if let Some(cpu) = video_cpu.final_sample() {
        crate::ui_logln!(
            "video_cpu final main={:.1}% decode={:.1}% process={:.1}%",
            cpu.main_pct,
            cpu.decode_pct,
            cpu.process_pct
        );
    }
    if profile_on {
        profiler.finish();
    }
    if let Err(e) = cpu_profile::finish(cpu) {
        crate::ui_errln!("{e}");
    }
}

#[derive(Default)]
pub(super) struct AudioWindowStats {
    write_us: u128,
    requested_frames: u128,
    written_frames: u128,
    underruns: u64,
    loop_count: u64,
}

impl AudioWindowStats {
    pub(super) fn add(
        &mut self,
        write_elapsed: Duration,
        requested_frames: usize,
        written_frames: usize,
        loop_count: u64,
    ) {
        self.write_us += write_elapsed.as_micros();
        self.requested_frames += requested_frames as u128;
        self.written_frames += written_frames as u128;
        if written_frames < requested_frames {
            self.underruns += 1;
        }
        self.loop_count = self.loop_count.max(loop_count);
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}

struct AudioWriteJob {
    audio: Vec<i16>,
    requested_frames: usize,
    loop_count: u64,
}

struct AudioWriteResult {
    audio: Vec<i16>,
    requested_frames: usize,
    written_frames: usize,
    loop_count: u64,
    write_us: u64,
    error: Option<String>,
}

struct AudioWriteWorker {
    tx: Option<mpsc::SyncSender<AudioWriteJob>>,
    rx: mpsc::Receiver<AudioWriteResult>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl AudioWriteWorker {
    fn start() -> Result<Self, String> {
        let mut sink = crate::mr_audio::MrAudioSink::open_default()?;
        let (tx, job_rx) = mpsc::sync_channel::<AudioWriteJob>(4);
        let (result_tx, rx) = mpsc::channel::<AudioWriteResult>();
        let join = std::thread::Builder::new()
            .name("video-audio-write".to_string())
            .spawn(move || {
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::VideoAudio,
                );
                while let Ok(job) = job_rx.recv() {
                    let t0 = Instant::now();
                    let result = match sink.write_frames(&job.audio) {
                        Ok(written) => AudioWriteResult {
                            audio: job.audio,
                            requested_frames: job.requested_frames,
                            written_frames: written,
                            loop_count: job.loop_count,
                            write_us: t0.elapsed().as_micros() as u64,
                            error: None,
                        },
                        Err(e) => AudioWriteResult {
                            audio: job.audio,
                            requested_frames: job.requested_frames,
                            written_frames: 0,
                            loop_count: job.loop_count,
                            write_us: t0.elapsed().as_micros() as u64,
                            error: Some(e),
                        },
                    };
                    let failed = result.error.is_some();
                    if result_tx.send(result).is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(|e| format!("spawn video-audio-write: {e}"))?;
        Ok(Self {
            tx: Some(tx),
            rx,
            join: Some(join),
        })
    }

    fn try_send(&self, job: AudioWriteJob) -> Result<(), mpsc::TrySendError<AudioWriteJob>> {
        match &self.tx {
            Some(tx) => tx.try_send(job),
            None => Err(mpsc::TrySendError::Disconnected(job)),
        }
    }

    fn try_recv(&self) -> Result<AudioWriteResult, mpsc::TryRecvError> {
        self.rx.try_recv()
    }

    fn finish(mut self) -> Result<Vec<AudioWriteResult>, String> {
        self.tx.take();
        let mut results = Vec::new();
        while let Ok(result) = self.rx.recv() {
            results.push(result);
        }
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| "video audio writer panicked during shutdown".to_string())?;
        }
        Ok(results)
    }
}

fn drain_audio_write_results(
    audio_writer: &AudioWriteWorker,
    frame_worker: &crate::video_player::VideoFrameWorker,
    audio_stats: &mut AudioWindowStats,
) -> bool {
    loop {
        match audio_writer.try_recv() {
            Ok(result) => {
                if let Some(e) = result.error {
                    crate::ui_errln!("video_playback audio: {e}");
                    frame_worker.recycle_audio(result.audio);
                    return false;
                }
                audio_stats.add(
                    Duration::from_micros(result.write_us),
                    result.requested_frames,
                    result.written_frames,
                    result.loop_count,
                );
                frame_worker.recycle_audio(result.audio);
            }
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => {
                crate::ui_errln!("video_playback audio: writer stopped");
                return false;
            }
        }
    }
}

fn finish_audio_writer(
    audio_writer: AudioWriteWorker,
    frame_worker: &crate::video_player::VideoFrameWorker,
    audio_stats: &mut AudioWindowStats,
) {
    match audio_writer.finish() {
        Ok(results) => {
            let mut requested = 0usize;
            let mut written = 0usize;
            for result in results {
                if let Some(e) = result.error {
                    crate::ui_errln!("video_playback audio shutdown: {e}");
                }
                requested = requested.saturating_add(result.requested_frames);
                written = written.saturating_add(result.written_frames);
                audio_stats.add(
                    Duration::from_micros(result.write_us),
                    result.requested_frames,
                    result.written_frames,
                    result.loop_count,
                );
                frame_worker.recycle_audio(result.audio);
            }
            crate::ui_logln!(
                "video_playback audio shutdown requested={} written={} lost={} underruns={}",
                requested,
                written,
                requested.saturating_sub(written),
                audio_stats.underruns
            );
        }
        Err(e) => crate::ui_errln!("video_playback audio shutdown: {e}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn enqueue_audio_write(
    audio_writer: &AudioWriteWorker,
    frame_worker: &crate::video_player::VideoFrameWorker,
    audio_stats: &mut AudioWindowStats,
    audio: Vec<i16>,
    requested_frames: usize,
    loop_count: u64,
    phases: &mut VideoFramePhases,
    video_profile: &mut VideoFrameProfile,
) -> bool {
    let audio_t0 = Instant::now();
    let job = AudioWriteJob {
        audio,
        requested_frames,
        loop_count,
    };
    match audio_writer.try_send(job) {
        Ok(()) => {
            phases.audio_write_us = audio_t0.elapsed().as_micros() as u64;
            video_profile.audio_write_us = phases.audio_write_us;
            true
        }
        Err(mpsc::TrySendError::Full(job)) => {
            phases.audio_write_us = audio_t0.elapsed().as_micros() as u64;
            phases.audio_underrun = true;
            video_profile.audio_write_us = phases.audio_write_us;
            video_profile.audio_underrun = true;
            audio_stats.add(
                Duration::from_micros(phases.audio_write_us),
                job.requested_frames,
                0,
                job.loop_count,
            );
            frame_worker.recycle_audio(job.audio);
            true
        }
        Err(mpsc::TrySendError::Disconnected(job)) => {
            crate::ui_errln!("video_playback audio: writer stopped");
            frame_worker.recycle_audio(job.audio);
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record_video_sample(
    phases: VideoFramePhases,
    sample: FrameSample,
    copy_rect: Option<DirtyRect>,
    profiler: &mut FrameProfiler,
    fps_window_start: &mut Instant,
    fps_frames: &mut u64,
    totals: &mut VideoWindowTotals,
    audio_stats: &mut AudioWindowStats,
    cpu_sampler: &mut VideoCpuSampler,
) {
    let cpu_window = cpu_sampler.window_sample();
    if profiler.enabled() {
        profiler.record(sample);
        if let Some(cpu) = cpu_window {
            crate::ui_logln!(
                "  video-cpu | main={:.1}% decode={:.1}% process={:.1}%",
                cpu.main_pct,
                cpu.decode_pct,
                cpu.process_pct
            );
        }
        return;
    }

    *fps_frames += 1;
    totals.record(phases, sample, copy_rect);
    if fps_window_start.elapsed().as_millis() >= 1000 {
        let video_nn = totals.video_frames.max(1);
        crate::ui_logln!(
            "  fps ~ {}  | video-frames {} missed-deadlines {} recv {}us video-decode {}us/frame video-scale {}us/frame image-update {}us/frame blit {}us/frame slint-render {}us vsync-wait {}us fb-present {}us ({} logical rows avg, {} px avg) audio-decode {}us/frame audio-resample {}us/frame audio-write {}us/frame audio {}/{}f underruns {} loops {}",
            *fps_frames,
            totals.video_frames,
            totals.missed_deadlines,
            VideoWindowTotals::avg_per_frame(totals.recv_us, *fps_frames),
            VideoWindowTotals::avg_per_video_frame(totals.video_decode_us, video_nn),
            VideoWindowTotals::avg_per_video_frame(totals.video_scale_us, video_nn),
            VideoWindowTotals::avg_per_video_frame(totals.image_us, video_nn),
            VideoWindowTotals::avg_per_video_frame(totals.blit_us, video_nn),
            VideoWindowTotals::avg_per_frame(totals.render_us, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.vsync_us, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.copy_us, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.copy_rows, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.copy_px, *fps_frames),
            VideoWindowTotals::avg_per_video_frame(totals.audio_decode_us, video_nn),
            VideoWindowTotals::avg_per_video_frame(totals.audio_resample_us, video_nn),
            VideoWindowTotals::avg_per_video_frame(totals.audio_write_us, video_nn),
            audio_stats.written_frames,
            audio_stats.requested_frames,
            totals.audio_underruns,
            audio_stats.loop_count
        );
        if let Some(cpu) = cpu_window {
            crate::ui_logln!(
                "  video-cpu | main={:.1}% decode={:.1}% process={:.1}%",
                cpu.main_pct,
                cpu.decode_pct,
                cpu.process_pct
            );
        }
        *fps_frames = 0;
        totals.reset();
        audio_stats.reset();
        *fps_window_start = Instant::now();
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct VideoCpuTicks {
    process: u64,
    main: u64,
    decode: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct VideoCpuSample {
    main_pct: f64,
    decode_pct: f64,
    process_pct: f64,
}

pub(super) struct VideoCpuSampler {
    ticks_per_sec: f64,
    start: Option<(Instant, VideoCpuTicks)>,
    last: Option<(Instant, VideoCpuTicks)>,
}

impl VideoCpuSampler {
    pub(super) fn new() -> Self {
        let ticks_per_sec = linux_ticks_per_sec();
        let now = Instant::now();
        let ticks = read_video_cpu_ticks();
        Self {
            ticks_per_sec,
            start: ticks.map(|ticks| (now, ticks)),
            last: ticks.map(|ticks| (now, ticks)),
        }
    }

    pub(super) fn window_sample(&mut self) -> Option<VideoCpuSample> {
        let (last_instant, last_ticks) = self.last?;
        if last_instant.elapsed().as_millis() < 1000 {
            return None;
        }
        let now = Instant::now();
        let ticks = read_video_cpu_ticks()?;
        self.last = Some((now, ticks));
        Some(cpu_sample_between(
            last_instant,
            now,
            last_ticks,
            ticks,
            self.ticks_per_sec,
        ))
    }

    pub(super) fn final_sample(&self) -> Option<VideoCpuSample> {
        let (start_instant, start_ticks) = self.start?;
        let now = Instant::now();
        let ticks = read_video_cpu_ticks()?;
        Some(cpu_sample_between(
            start_instant,
            now,
            start_ticks,
            ticks,
            self.ticks_per_sec,
        ))
    }
}

fn cpu_sample_between(
    start: Instant,
    end: Instant,
    a: VideoCpuTicks,
    b: VideoCpuTicks,
    ticks_per_sec: f64,
) -> VideoCpuSample {
    let secs = end.duration_since(start).as_secs_f64().max(0.001);
    let pct = |delta: u64| (delta as f64 / ticks_per_sec) * 100.0 / secs;
    VideoCpuSample {
        main_pct: pct(b.main.saturating_sub(a.main)),
        decode_pct: pct(b.decode.saturating_sub(a.decode)),
        process_pct: pct(b.process.saturating_sub(a.process)),
    }
}

fn linux_ticks_per_sec() -> f64 {
    #[cfg(target_os = "linux")]
    // SAFETY: sysconf(_SC_CLK_TCK) does not dereference Rust memory.
    unsafe {
        let value = libc::sysconf(libc::_SC_CLK_TCK);
        if value > 0 {
            return value as f64;
        }
    }
    100.0
}

fn read_video_cpu_ticks() -> Option<VideoCpuTicks> {
    let process = read_stat_ticks("/proc/self/stat")?;
    let main = read_stat_ticks(format!("/proc/self/task/{}/stat", std::process::id()))?;
    let decode = find_thread_ticks("video-decode").unwrap_or(0);
    Some(VideoCpuTicks {
        process,
        main,
        decode,
    })
}

fn find_thread_ticks(name: &str) -> Option<u64> {
    let tasks = std::fs::read_dir("/proc/self/task").ok()?;
    for task in tasks.flatten() {
        let task_path = task.path();
        let comm = std::fs::read_to_string(task_path.join("comm")).ok()?;
        if comm.trim() == name {
            return read_stat_ticks(task_path.join("stat"));
        }
    }
    None
}

fn read_stat_ticks(path: impl AsRef<std::path::Path>) -> Option<u64> {
    let stat = std::fs::read_to_string(path).ok()?;
    let after_comm = stat.rsplit_once(") ")?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let utime = fields.get(11)?.parse::<u64>().ok()?;
    let stime = fields.get(12)?.parse::<u64>().ok()?;
    Some(utime + stime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_animation_uses_a_200ms_smooth_spring_response() {
        let mut animation = VideoSizeAnimation::new(false);
        animation.toggle(Duration::ZERO);

        assert!(!animation.update(Duration::from_millis(100)));
        let halfway = animation.dimensions();
        assert!(halfway.0 > 320 && halfway.0 < 640);
        assert_eq!(halfway.0 * 3, halfway.1 * 4);
        assert!(animation.is_active());

        assert!(!animation.update(Duration::from_millis(200)));
        assert!(animation.dimensions().0 >= 632);
        assert_eq!(
            animation.spring.configuration().damping_ratio(),
            SpringConfiguration::smooth().damping_ratio()
        );
        assert!(animation.update(Duration::from_millis(400)));
        assert_eq!(animation.dimensions(), (640, 480));
        assert!(!animation.is_active());
    }

    #[test]
    fn reversing_mid_animation_keeps_current_size_and_spring_velocity() {
        let mut animation = VideoSizeAnimation::new(false);
        animation.toggle(Duration::ZERO);
        animation.update(Duration::from_millis(100));
        let midpoint = animation.dimensions();
        let velocity = animation.spring.velocity();

        animation.toggle(Duration::from_millis(100));
        assert_eq!(animation.dimensions(), midpoint);
        assert_eq!(animation.spring.velocity(), velocity);
        assert_eq!(animation.target_dimensions(), (320, 240));
        assert!(animation.update(Duration::from_millis(500)));
        assert_eq!(animation.dimensions(), (320, 240));
    }

    #[test]
    fn a_button_is_rising_edge_triggered() {
        let mut edge = VideoButtonEdge::default();
        assert!(!edge.a_pressed(false));
        assert!(edge.a_pressed(true));
        assert!(!edge.a_pressed(true));
        assert!(!edge.a_pressed(false));
        assert!(edge.a_pressed(true));
    }

    #[test]
    fn automatic_toggle_fires_at_each_configured_interval() {
        let mut toggle = VideoAutoToggle {
            interval: Some(Duration::from_millis(500)),
            next: Duration::from_millis(500),
        };
        assert!(!toggle.due(Duration::from_millis(499)));
        assert!(toggle.due(Duration::from_millis(500)));
        assert!(!toggle.due(Duration::from_millis(999)));
        assert!(toggle.due(Duration::from_millis(1_100)));
        assert_eq!(toggle.next, Duration::from_millis(1_500));
    }

    #[test]
    fn scaler_uses_nearest_neighbour_for_intermediate_and_exact_2x_sizes() {
        let frame = crate::video_player::VideoRgb565Frame {
            pixels: vec![1, 2, 3, 4],
            width: 2,
            height: 2,
        };
        let mut scaler = VideoRgb565Scaler::default();

        assert_eq!(scaler.scale(&frame, 3, 3), &[1, 1, 2, 1, 1, 2, 3, 3, 4]);
        assert_eq!(
            scaler.scale(&frame, 4, 4),
            &[1, 1, 2, 2, 1, 1, 2, 2, 3, 3, 4, 4, 3, 3, 4, 4]
        );
    }

    #[test]
    fn scaler_reuses_its_output_allocation() {
        let frame = crate::video_player::VideoRgb565Frame {
            pixels: vec![7; 4],
            width: 2,
            height: 2,
        };
        let mut scaler = VideoRgb565Scaler::default();
        assert!(scaler.pixels.capacity() >= VIDEO_SOURCE_W * 2 * VIDEO_SOURCE_H * 2);
        assert!(scaler.x_map.capacity() >= VIDEO_SOURCE_W * 2);
        assert!(scaler.y_map.capacity() >= VIDEO_SOURCE_H * 2);
        scaler.scale(&frame, 4, 4);
        let allocation = scaler.pixels.as_ptr();
        let x_mapping = scaler.x_map.as_ptr();
        let y_mapping = scaler.y_map.as_ptr();
        scaler.scale(&frame, 3, 3);
        assert_eq!(scaler.pixels.as_ptr(), allocation);
        assert_eq!(scaler.x_map.as_ptr(), x_mapping);
        assert_eq!(scaler.y_map.as_ptr(), y_mapping);
    }

    #[test]
    fn shrinking_restores_only_exposed_background_bands() {
        let old = video_frame_rect(640, 480);
        let new = video_frame_rect(320, 240);
        let [right, bottom] = video_exposed_background_rects(old, new);
        assert_eq!(
            right,
            Some(DirtyRect {
                x0: 360,
                y0: 30,
                x1: 680,
                y1: 510,
            })
        );
        assert_eq!(
            bottom,
            Some(DirtyRect {
                x0: 40,
                y0: 270,
                x1: 360,
                y1: 510,
            })
        );
        assert_eq!(exposed_background_union(old, new), Some(old));
        assert!(growing_video_exposes_no_background(new, old));
    }
}
