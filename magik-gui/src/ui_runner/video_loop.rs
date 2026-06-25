use super::*;

pub(super) const VIDEO_IMAGE_RECT: DirtyRect = DirtyRect {
    x0: 40,
    y0: 158,
    x1: 360,
    y1: 382,
};

#[cfg(all(feature = "video", mister_bench_scenes))]
#[derive(Default)]
pub(super) struct VideoFramePhases {
    frame_updated: bool,
    decode_us: u64,
    recv_us: u64,
    image_us: u64,
    blit_us: u64,
    audio_us: u64,
}

#[cfg(all(feature = "video", mister_bench_scenes))]
#[derive(Default)]
pub(super) struct VideoWindowTotals {
    frames: u64,
    video_frames: u64,
    decode_us: u128,
    recv_us: u128,
    image_us: u128,
    blit_us: u128,
    audio_us: u128,
    render_us: u128,
    vsync_us: u128,
    copy_us: u128,
    copy_rows: u128,
    copy_px: u128,
}

#[cfg(all(feature = "video", mister_bench_scenes))]
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
        self.decode_us += phases.decode_us as u128;
        self.recv_us += phases.recv_us as u128;
        self.image_us += phases.image_us as u128;
        self.blit_us += phases.blit_us as u128;
        self.audio_us += phases.audio_us as u128;
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

#[cfg(all(feature = "video", mister_bench_scenes))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VideoRenderMode {
    SlintImage,
    DirectBlit,
}

#[cfg(all(feature = "video", mister_bench_scenes))]
impl VideoRenderMode {
    pub(super) fn from_env() -> Self {
        match std::env::var("MISTER_VIDEO_RENDER_MODE")
            .unwrap_or_else(|_| "slint-image".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "direct" | "direct-blit" | "direct_blit" => Self::DirectBlit,
            _ => Self::SlintImage,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::SlintImage => "slint-image",
            Self::DirectBlit => "direct-blit",
        }
    }
}

#[cfg(all(feature = "video", mister_bench_scenes))]
pub(super) fn video_copy_rect(
    dirty: DirtyRect,
    video_dirty_clip_ready: bool,
    frame_updated: bool,
) -> DirtyRect {
    if video_dirty_clip_ready && frame_updated {
        dirty.intersection(VIDEO_IMAGE_RECT).unwrap_or(dirty)
    } else {
        dirty
    }
}

#[cfg(all(feature = "video", mister_bench_scenes))]
pub(super) fn direct_video_copy_rect(
    dirty: Option<DirtyRect>,
    video_dirty_clip_ready: bool,
) -> DirtyRect {
    let Some(dirty) = dirty else {
        return VIDEO_IMAGE_RECT;
    };
    if !video_dirty_clip_ready {
        return dirty.union(VIDEO_IMAGE_RECT);
    }
    dirty
        .intersection(VIDEO_IMAGE_RECT)
        .unwrap_or(dirty.union(VIDEO_IMAGE_RECT))
}

#[cfg(all(feature = "video", mister_bench_scenes))]
pub(super) fn blit_video_frame_to_cached(
    frame: &SharedPixelBuffer<Rgb8Pixel>,
    cached: &mut [Rgb565Pixel],
    render_w: usize,
) {
    let src_w = frame.width() as usize;
    let src_h = frame.height() as usize;
    let bytes = frame.as_bytes();
    let dst_x = VIDEO_IMAGE_RECT.x0;
    let dst_y = VIDEO_IMAGE_RECT.y0;
    for y in 0..src_h {
        let src = &bytes[y * src_w * 3..(y + 1) * src_w * 3];
        let dst =
            &mut cached[(dst_y + y) * render_w + dst_x..(dst_y + y) * render_w + dst_x + src_w];
        unsafe {
            let mut src = src.as_ptr();
            let mut dst = dst.as_mut_ptr();
            for _ in 0..src_w {
                dst.write(<Rgb565Pixel as TargetPixel>::from_rgb(
                    *src,
                    *src.add(1),
                    *src.add(2),
                ));
                src = src.add(3);
                dst = dst.add(1);
            }
        }
    }
}

#[cfg(all(feature = "video", mister_bench_scenes))]
pub(super) fn run_video_playback_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    window: &Rc<MinimalSoftwareWindow>,
    app: slint_ui::video_playback::VideoPlayback,
    animation_clock: &AnimationClock,
) {
    let path = std::env::var("MISTER_VIDEO_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::video_player::DEFAULT_VIDEO_PATH.to_string());
    let frame_worker = match crate::video_player::VideoFrameWorker::start(path.clone()) {
        Ok(worker) => worker,
        Err(e) => {
            eprintln!("video_playback: {e}");
            std::process::exit(1);
        }
    };
    let mut audio_sink = match crate::mr_audio::MrAudioSink::open_default() {
        Ok(sink) => sink,
        Err(e) => {
            eprintln!("video_playback audio: {e}");
            std::process::exit(1);
        }
    };

    let mut cached = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];
    let start = Instant::now();
    let mut next_video_at = Duration::ZERO;
    let frame_interval = frame_worker.frame_interval();
    let mut frames = 0u64;
    let mut profiler = FrameProfiler::from_env();
    let cpu = cpu_profile::start();
    let profile_on = profiler.enabled();
    let frame_order = FrameOrder::from_env();
    let render_mode = VideoRenderMode::from_env();
    let mut pacer = VsyncPacer::from_env();

    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut video_totals = VideoWindowTotals::default();
    let mut audio_stats = AudioWindowStats::default();
    let mut video_dirty_clip_ready = false;

    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "video_playback running {label} path={path} frame-order={} animation-clock={} video-render-mode={}",
        frame_order.label(),
        animation_clock.label(),
        render_mode.label()
    );
    println!("video_render_mode={}", render_mode.label());
    println!(
        "video_dirty_clip=on rect={}x{}+{},{}",
        VIDEO_IMAGE_RECT.width(),
        VIDEO_IMAGE_RECT.rows(),
        VIDEO_IMAGE_RECT.x0,
        VIDEO_IMAGE_RECT.y0
    );

    while secs == 0 || start.elapsed().as_secs() < secs {
        let frame_start = Instant::now();
        let t0 = Instant::now();
        let mut this_rect: Option<DirtyRect> = None;
        let mut phases = VideoFramePhases::default();
        let mut direct_frame: Option<SharedPixelBuffer<Rgb8Pixel>> = None;

        match frame_order {
            FrameOrder::RenderThenVsync => {
                update_slint_animations(animation_clock);
                let now = start.elapsed();
                if now >= next_video_at {
                    let recv_t0 = Instant::now();
                    match frame_worker.try_recv() {
                        Ok(Some(frame)) => {
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                            let crate::video_player::PlaybackFrame {
                                pixel_buffer,
                                audio,
                                audio_requested_frames,
                                loop_count,
                                decode_us,
                            } = frame;
                            phases.frame_updated = true;
                            phases.decode_us = decode_us;
                            match render_mode {
                                VideoRenderMode::SlintImage => {
                                    let image_t0 = Instant::now();
                                    app.set_frame(slint::Image::from_rgb8(pixel_buffer));
                                    phases.image_us = image_t0.elapsed().as_micros() as u64;
                                    window.request_redraw();
                                }
                                VideoRenderMode::DirectBlit => {
                                    direct_frame = Some(pixel_buffer);
                                }
                            }
                            let audio_t0 = Instant::now();
                            match audio_sink.write_frames(&audio) {
                                Ok(written) => {
                                    phases.audio_us = audio_t0.elapsed().as_micros() as u64;
                                    audio_stats.add(
                                        Duration::from_micros(phases.audio_us),
                                        audio_requested_frames,
                                        written,
                                        loop_count,
                                    );
                                }
                                Err(e) => {
                                    eprintln!("video_playback audio: {e}");
                                    break;
                                }
                            }
                            frame_worker.recycle_audio(audio);
                        }
                        Ok(None) => {
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                        }
                        Err(e) => {
                            eprintln!("video_playback: {e}");
                            break;
                        }
                    }
                    next_video_at += frame_interval;
                    while next_video_at < now {
                        next_video_at += frame_interval;
                    }
                }
                let t1 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let region = renderer.render(&mut cached, ui.render_w());
                    this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                });
                let t2 = Instant::now();
                if let Some(frame) = direct_frame.as_ref() {
                    let blit_t0 = Instant::now();
                    blit_video_frame_to_cached(frame, &mut cached, ui.render_w());
                    phases.blit_us = blit_t0.elapsed().as_micros() as u64;
                }
                let pace = pacer.wait();
                let t3 = Instant::now();
                let mut copied_rect = None;
                let rows = if direct_frame.is_some() {
                    let rect = direct_video_copy_rect(this_rect, video_dirty_clip_ready);
                    copy_cached_rect_565(disp, ui, &cached, rect);
                    copied_rect = Some(rect);
                    rect.rows()
                } else if let Some(rect) = this_rect {
                    let rect = video_copy_rect(rect, video_dirty_clip_ready, phases.frame_updated);
                    copy_cached_rect_565(disp, ui, &cached, rect);
                    copied_rect = Some(rect);
                    rect.rows()
                } else {
                    0
                };
                if rows > 0 {
                    video_dirty_clip_ready = true;
                }
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
                );
            }
            FrameOrder::VsyncThenRender => {
                let pace = pacer.wait();
                let t1 = Instant::now();
                update_slint_animations(animation_clock);
                let now = start.elapsed();
                if now >= next_video_at {
                    let recv_t0 = Instant::now();
                    match frame_worker.try_recv() {
                        Ok(Some(frame)) => {
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                            let crate::video_player::PlaybackFrame {
                                pixel_buffer,
                                audio,
                                audio_requested_frames,
                                loop_count,
                                decode_us,
                            } = frame;
                            phases.frame_updated = true;
                            phases.decode_us = decode_us;
                            match render_mode {
                                VideoRenderMode::SlintImage => {
                                    let image_t0 = Instant::now();
                                    app.set_frame(slint::Image::from_rgb8(pixel_buffer));
                                    phases.image_us = image_t0.elapsed().as_micros() as u64;
                                    window.request_redraw();
                                }
                                VideoRenderMode::DirectBlit => {
                                    direct_frame = Some(pixel_buffer);
                                }
                            }
                            let audio_t0 = Instant::now();
                            match audio_sink.write_frames(&audio) {
                                Ok(written) => {
                                    phases.audio_us = audio_t0.elapsed().as_micros() as u64;
                                    audio_stats.add(
                                        Duration::from_micros(phases.audio_us),
                                        audio_requested_frames,
                                        written,
                                        loop_count,
                                    );
                                }
                                Err(e) => {
                                    eprintln!("video_playback audio: {e}");
                                    break;
                                }
                            }
                            frame_worker.recycle_audio(audio);
                        }
                        Ok(None) => {
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                        }
                        Err(e) => {
                            eprintln!("video_playback: {e}");
                            break;
                        }
                    }
                    next_video_at += frame_interval;
                    while next_video_at < now {
                        next_video_at += frame_interval;
                    }
                }
                let t2 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let region = renderer.render(&mut cached, ui.render_w());
                    this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                });
                let t3 = Instant::now();
                if let Some(frame) = direct_frame.as_ref() {
                    let blit_t0 = Instant::now();
                    blit_video_frame_to_cached(frame, &mut cached, ui.render_w());
                    phases.blit_us = blit_t0.elapsed().as_micros() as u64;
                }
                let mut copied_rect = None;
                let rows = if direct_frame.is_some() {
                    let rect = direct_video_copy_rect(this_rect, video_dirty_clip_ready);
                    copy_cached_rect_565(disp, ui, &cached, rect);
                    copied_rect = Some(rect);
                    rect.rows()
                } else if let Some(rect) = this_rect {
                    let rect = video_copy_rect(rect, video_dirty_clip_ready, phases.frame_updated);
                    copy_cached_rect_565(disp, ui, &cached, rect);
                    copied_rect = Some(rect);
                    rect.rows()
                } else {
                    0
                };
                if rows > 0 {
                    video_dirty_clip_ready = true;
                }
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
                );
            }
        }
        frames += 1;
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Ok(status) = crate::mr_audio::read_status() {
        print!("video_playback audio status: {status}");
    }
    if profile_on {
        profiler.finish();
    }
    if let Err(e) = cpu_profile::finish(cpu) {
        eprintln!("{e}");
    }
}

#[cfg(all(feature = "video", mister_bench_scenes))]
#[derive(Default)]
pub(super) struct AudioWindowStats {
    write_us: u128,
    requested_frames: u128,
    written_frames: u128,
    underruns: u64,
    loop_count: u64,
}

#[cfg(all(feature = "video", mister_bench_scenes))]
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

#[cfg(all(feature = "video", mister_bench_scenes))]
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
) {
    if profiler.enabled() {
        profiler.record(sample);
        return;
    }

    *fps_frames += 1;
    totals.record(phases, sample, copy_rect);
    if fps_window_start.elapsed().as_millis() >= 1000 {
        let video_nn = totals.video_frames.max(1);
        println!(
            "  fps ~ {}  | video-frames {} recv {}us decode-worker {}us/frame image-update {}us/frame blit {}us/frame slint-render {}us vsync-wait {}us fb-present {}us ({} logical rows avg, {} px avg) audio-write {}us/frame audio {}/{}f underruns {} loops {}",
            *fps_frames,
            totals.video_frames,
            VideoWindowTotals::avg_per_frame(totals.recv_us, *fps_frames),
            VideoWindowTotals::avg_per_video_frame(totals.decode_us, video_nn),
            VideoWindowTotals::avg_per_video_frame(totals.image_us, video_nn),
            VideoWindowTotals::avg_per_video_frame(totals.blit_us, video_nn),
            VideoWindowTotals::avg_per_frame(totals.render_us, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.vsync_us, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.copy_us, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.copy_rows, *fps_frames),
            VideoWindowTotals::avg_per_frame(totals.copy_px, *fps_frames),
            VideoWindowTotals::avg_per_video_frame(audio_stats.write_us, video_nn),
            audio_stats.written_frames,
            audio_stats.requested_frames,
            audio_stats.underruns,
            audio_stats.loop_count
        );
        *fps_frames = 0;
        totals.reset();
        audio_stats.reset();
        *fps_window_start = Instant::now();
    }
}
