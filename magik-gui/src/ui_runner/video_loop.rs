use super::*;

pub(super) const VIDEO_IMAGE_RECT: DirtyRect = DirtyRect {
    x0: 40,
    y0: 30,
    x1: 680,
    y1: 510,
};

#[cfg(mister_video_scene)]
pub(super) fn video_frame_rect(frame: &crate::video_player::VideoRgb565Frame) -> DirtyRect {
    DirtyRect {
        x0: VIDEO_IMAGE_RECT.x0,
        y0: VIDEO_IMAGE_RECT.y0,
        x1: (VIDEO_IMAGE_RECT.x0 + frame.width as usize).min(VIDEO_IMAGE_RECT.x1),
        y1: (VIDEO_IMAGE_RECT.y0 + frame.height as usize).min(VIDEO_IMAGE_RECT.y1),
    }
}

#[cfg(mister_video_scene)]
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

#[cfg(mister_video_scene)]
fn present_video_frame_direct(
    disp: &mut MappedRgb565Framebuffer,
    frame: &crate::video_player::VideoRgb565Frame,
    rect: DirtyRect,
) {
    let src = rgb565_words_as_pixels(&frame.pixels);
    if let Err(e) = disp.present_rect_565_strided(
        rect.x0,
        rect.y0,
        rect.width(),
        rect.rows() as usize,
        src,
        frame.width as usize,
        0,
        0,
    ) {
        crate::ui_errln!("framebuffer present video direct failed: {e}");
    }
}

#[cfg(mister_video_scene)]
fn present_direct_video_frame(
    disp: &mut MappedRgb565Framebuffer,
    ui: &UiDisplay,
    cached: &[Rgb565Pixel],
    frame: &crate::video_player::VideoRgb565Frame,
    dirty: Option<DirtyRect>,
    video_dirty_clip_ready: bool,
) -> (u32, Option<DirtyRect>) {
    let video_rect = video_frame_rect(frame);
    let mut rows = 0u32;
    let mut copied_rect = None;

    if let Some(dirty) = dirty {
        if !video_dirty_clip_ready || dirty.intersection(video_rect).is_none() {
            if let Some(copied) = copy_cached_rect_565(
                disp,
                CachedFrameView::new(cached, ui.render_w(), ui.render_h()),
                dirty,
            ) {
                rows = rows.saturating_add(copied.rows());
                copied_rect = Some(copied);
            }
        }
    }

    present_video_frame_direct(disp, frame, video_rect);
    rows = rows.saturating_add(video_rect.rows());
    copied_rect = Some(copied_rect.map_or(video_rect, |rect: DirtyRect| rect.union(video_rect)));
    (rows, copied_rect)
}

#[cfg(mister_video_scene)]
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
    audio_underrun: bool,
}

#[cfg(mister_video_scene)]
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
    render_us: u128,
    vsync_us: u128,
    copy_us: u128,
    copy_rows: u128,
    copy_px: u128,
}

#[cfg(mister_video_scene)]
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

#[cfg(mister_video_scene)]
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

#[cfg(mister_video_scene)]
pub(super) fn run_video_playback_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
    window: &Rc<MinimalSoftwareWindow>,
    _app: slint_ui::video_playback::VideoPlayback,
    animation_clock: &AnimationClock,
) {
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
    let mut profiler = FrameProfiler::from_env();
    let cpu = cpu_profile::start();
    let profile_on = profiler.enabled();
    let frame_order = FrameOrder::from_env();
    let mut pacer = VsyncPacer::from_env();

    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut video_totals = VideoWindowTotals::default();
    let mut audio_stats = AudioWindowStats::default();
    let mut video_cpu = VideoCpuSampler::new();
    let mut video_dirty_clip_ready = false;

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
        "video_controls queue_depth=2 scale={} profile={}",
        std::env::var("MISTER_VIDEO_SCALE").unwrap_or_else(|_| "source".into()),
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
        let mut direct_frame: Option<crate::video_player::VideoRgb565Frame> = None;

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
                                frame,
                                audio,
                                audio_requested_frames,
                                loop_count,
                                decode_us: _decode_us,
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
                            direct_frame = Some(frame);
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
                        }
                        Err(e) => {
                            crate::ui_errln!("video_playback: {e}");
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
                let pace = pacer.wait();
                let t3 = Instant::now();
                let mut copied_rect = None;
                let rows = if let Some(frame) = direct_frame.as_ref() {
                    let (rows, rect) = present_direct_video_frame(
                        disp,
                        ui,
                        &cached,
                        frame,
                        this_rect,
                        video_dirty_clip_ready,
                    );
                    copied_rect = rect;
                    rows
                } else if let Some(rect) = this_rect {
                    let rect = video_copy_rect(rect, video_dirty_clip_ready, phases.frame_updated);
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
                    let recv_t0 = Instant::now();
                    match frame_worker.try_recv() {
                        Ok(Some(frame)) => {
                            phases.recv_us = recv_t0.elapsed().as_micros() as u64;
                            let crate::video_player::PlaybackFrame {
                                frame,
                                audio,
                                audio_requested_frames,
                                loop_count,
                                decode_us: _decode_us,
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
                            direct_frame = Some(frame);
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
                        }
                        Err(e) => {
                            crate::ui_errln!("video_playback: {e}");
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
                let mut copied_rect = None;
                let rows = if let Some(frame) = direct_frame.as_ref() {
                    let (rows, rect) = present_direct_video_frame(
                        disp,
                        ui,
                        &cached,
                        frame,
                        this_rect,
                        video_dirty_clip_ready,
                    );
                    copied_rect = rect;
                    rows
                } else if let Some(rect) = this_rect {
                    let rect = video_copy_rect(rect, video_dirty_clip_ready, phases.frame_updated);
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

    let _ = drain_audio_write_results(&audio_writer, &frame_worker, &mut audio_stats);
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

#[cfg(mister_video_scene)]
#[derive(Default)]
pub(super) struct AudioWindowStats {
    write_us: u128,
    requested_frames: u128,
    written_frames: u128,
    underruns: u64,
    loop_count: u64,
}

#[cfg(mister_video_scene)]
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

#[cfg(mister_video_scene)]
struct AudioWriteJob {
    audio: Vec<i16>,
    requested_frames: usize,
    loop_count: u64,
}

#[cfg(mister_video_scene)]
struct AudioWriteResult {
    audio: Vec<i16>,
    requested_frames: usize,
    written_frames: usize,
    loop_count: u64,
    write_us: u64,
    error: Option<String>,
}

#[cfg(mister_video_scene)]
struct AudioWriteWorker {
    tx: mpsc::SyncSender<AudioWriteJob>,
    rx: mpsc::Receiver<AudioWriteResult>,
}

#[cfg(mister_video_scene)]
impl AudioWriteWorker {
    fn start() -> Result<Self, String> {
        let mut sink = crate::mr_audio::MrAudioSink::open_default()?;
        let (tx, job_rx) = mpsc::sync_channel::<AudioWriteJob>(4);
        let (result_tx, rx) = mpsc::channel::<AudioWriteResult>();
        std::thread::Builder::new()
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
        Ok(Self { tx, rx })
    }

    fn try_send(&self, job: AudioWriteJob) -> Result<(), mpsc::TrySendError<AudioWriteJob>> {
        self.tx.try_send(job)
    }

    fn try_recv(&self) -> Result<AudioWriteResult, mpsc::TryRecvError> {
        self.rx.try_recv()
    }
}

#[cfg(mister_video_scene)]
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

#[cfg(mister_video_scene)]
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

#[cfg(mister_video_scene)]
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
            "  fps ~ {}  | video-frames {} recv {}us video-decode {}us/frame video-scale {}us/frame image-update {}us/frame blit {}us/frame slint-render {}us vsync-wait {}us fb-present {}us ({} logical rows avg, {} px avg) audio-decode {}us/frame audio-resample {}us/frame audio-write {}us/frame audio {}/{}f underruns {} loops {}",
            *fps_frames,
            totals.video_frames,
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

#[cfg(mister_video_scene)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct VideoCpuTicks {
    process: u64,
    main: u64,
    decode: u64,
}

#[cfg(mister_video_scene)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct VideoCpuSample {
    main_pct: f64,
    decode_pct: f64,
    process_pct: f64,
}

#[cfg(mister_video_scene)]
pub(super) struct VideoCpuSampler {
    ticks_per_sec: f64,
    start: Option<(Instant, VideoCpuTicks)>,
    last: Option<(Instant, VideoCpuTicks)>,
}

#[cfg(mister_video_scene)]
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

#[cfg(mister_video_scene)]
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

#[cfg(mister_video_scene)]
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

#[cfg(mister_video_scene)]
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

#[cfg(mister_video_scene)]
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

#[cfg(mister_video_scene)]
fn read_stat_ticks(path: impl AsRef<std::path::Path>) -> Option<u64> {
    let stat = std::fs::read_to_string(path).ok()?;
    let after_comm = stat.rsplit_once(") ")?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let utime = fields.get(11)?.parse::<u64>().ok()?;
    let stime = fields.get(12)?.parse::<u64>().ok()?;
    Some(utime + stime)
}
