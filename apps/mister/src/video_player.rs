// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! FFmpeg-backed media pump for the Slint video benchmark.

use ffmpeg::ChannelLayout;
use ffmpeg::codec;
use ffmpeg::format;
use ffmpeg::media;
use ffmpeg::software::resampling::Context as ResamplingContext;
use ffmpeg::util::format::pixel::Pixel as FfmpegPixel;
use ffmpeg::util::format::sample::{Sample, Type as SampleType};
use ffmpeg::util::frame::audio::Audio;
use ffmpeg::util::frame::video::Video;
use ffmpeg::{codec::packet::Packet, util::error::EAGAIN};
use ffmpeg_next as ffmpeg;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};

use crate::video_i420::{convert_i420_to_rgb565, convert_i420_to_rgb565_2x_rust_optimized};

const DEFAULT_VIDEO_MAX_W: u32 = 640;
const DEFAULT_VIDEO_MAX_H: u32 = 480;
pub const CANONICAL_VIDEO_WIDTH: u32 = 320;
pub const CANONICAL_VIDEO_HEIGHT: u32 = 240;
const AUDIO_RATE: u32 = 48_000;
const OUTPUT_AUDIO_CHANNELS: usize = 2;
const VIDEO_QUEUE_DEPTH: usize = 2;

pub fn video_paths_from_env() -> Result<Vec<String>, String> {
    if let Some(path) = std::env::var("MISTER_VIDEO_PATH")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return Ok(vec![path]);
    }
    if let Some(dir) = std::env::var("MISTER_VIDEO_DIR")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return sorted_mp4_files(PathBuf::from(dir));
    }
    let default_dir = mister_magik_catalog::device_layout::current_app_path("video-snaps/neogeo");
    if default_dir.is_dir() {
        let paths = sorted_mp4_files(default_dir)?;
        if !paths.is_empty() {
            return Ok(paths);
        }
    }
    Ok(vec![
        mister_magik_catalog::device_layout::current_app_path("mslug3.mov")
            .display()
            .to_string(),
    ])
}

fn sorted_mp4_files(dir: PathBuf) -> Result<Vec<String>, String> {
    let mut paths: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| format!("read video dir {}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("mp4"))
                .unwrap_or(false)
        })
        .map(|path| path.display().to_string())
        .collect();
    paths.sort_by_key(|path| path.to_ascii_lowercase());
    if paths.is_empty() {
        return Err(format!("{}: no .mp4 files", dir.display()));
    }
    Ok(paths)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoScaleMode {
    Source,
    Double,
}

impl VideoScaleMode {
    fn from_env() -> Result<Self, String> {
        match std::env::var("MISTER_VIDEO_SCALE")
            .unwrap_or_else(|_| "source".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "source" => Ok(Self::Source),
            "2x" | "double" => Ok(Self::Double),
            other => Err(format!(
                "video: unknown MISTER_VIDEO_SCALE={other:?}; use source|2x"
            )),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Double => "2x",
        }
    }

    fn output_dimensions(self, src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
        match self {
            Self::Double => (
                src_w.saturating_mul(2).min(max_w),
                src_h.saturating_mul(2).min(max_h),
            ),
            Self::Source => (src_w.min(max_w), src_h.min(max_h)),
        }
    }
}

pub fn video_starts_doubled_from_env() -> Result<bool, String> {
    VideoScaleMode::from_env().map(|mode| mode == VideoScaleMode::Double)
}

pub struct VideoPlayer {
    playlist: Vec<String>,
    playlist_index: usize,
    path: String,
    input: format::context::Input,
    video_stream_index: usize,
    audio_stream_index: usize,
    video_decoder: ffmpeg::decoder::Video,
    audio_decoder: ffmpeg::decoder::Audio,
    audio_resampler: ResamplingContext,
    frame_interval: Duration,
    audio_rate: u32,
    audio_channels: u16,
    queued_audio: Vec<i16>,
    audio_start: usize,
    pending_video_packets: VecDeque<Packet>,
    pending_audio_decode_us: u64,
    pending_audio_resample_us: u64,
    loop_count: u64,
    video_codec: String,
    audio_codec: String,
    scale_mode: VideoScaleMode,
    output_width: u32,
    output_height: u32,
}

pub struct PlaybackFrame {
    pub frame: VideoRgb565Frame,
    pub audio: Vec<i16>,
    pub audio_requested_frames: usize,
    pub loop_count: u64,
    pub metrics: PlaybackMetrics,
}

#[derive(Clone, Debug)]
pub struct VideoRgb565Frame {
    pub pixels: Vec<u16>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Default)]
pub struct PlaybackMetrics {
    pub video_pipeline: String,
    pub video_decode_us: u64,
    pub video_convert_us: u64,
    pub video_scale_2x_us: u64,
    pub video_fused_us: u64,
    pub video_pipeline_us: u64,
    pub video_worker_us: u64,
    pub video_scale_us: u64,
    pub audio_decode_us: u64,
    pub audio_resample_us: u64,
    pub audio_buffer_frames: u32,
    pub video_file: String,
    pub video_width: u32,
    pub video_height: u32,
    pub video_codec: String,
    pub audio_codec: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct VideoDecodeMetrics {
    decode_us: u64,
    convert_us: u64,
    scale_2x_us: u64,
    fused_us: u64,
    scale_us: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct AudioDecodeMetrics {
    decode_us: u64,
    resample_us: u64,
}

#[derive(Default)]
struct RecycledFrame {
    audio: Vec<i16>,
    pixels: Vec<u16>,
}

pub struct VideoFrameWorker {
    rx: mpsc::Receiver<Result<PlaybackFrame, String>>,
    recycle_tx: mpsc::SyncSender<RecycledFrame>,
    frame_interval: Duration,
    pending_frames: Arc<AtomicU32>,
}

impl VideoFrameWorker {
    pub fn start(paths: Vec<String>) -> Result<Self, String> {
        let (tx, rx) = mpsc::sync_channel(VIDEO_QUEUE_DEPTH);
        let (recycle_tx, recycle_rx) = mpsc::sync_channel::<RecycledFrame>(VIDEO_QUEUE_DEPTH);
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let pending_frames = Arc::new(AtomicU32::new(0));
        let decode_pending_frames = Arc::clone(&pending_frames);
        std::thread::Builder::new()
            .name("video-decode".to_string())
            .spawn(move || {
                apply_runtime_thread_policy(RuntimeThreadRole::VideoDecode);
                let mut player = match VideoPlayer::open(paths) {
                    Ok(player) => {
                        let _ = init_tx.send(Ok(player.frame_interval()));
                        player
                    }
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                        return;
                    }
                };
                let frame_interval = player.frame_interval();
                let mut audio_pacer = AudioFramePacer::new();
                loop {
                    let mut buffers = RecycledFrame::default();
                    while let Ok(mut recycled) = recycle_rx.try_recv() {
                        if buffers.audio.is_empty() {
                            buffers.audio = std::mem::take(&mut recycled.audio);
                        }
                        if buffers.pixels.is_empty() {
                            buffers.pixels = std::mem::take(&mut recycled.pixels);
                        }
                    }
                    let audio_frames = audio_pacer.next_frames(frame_interval);
                    let t0 = Instant::now();
                    let frame = player
                        .next_frame_into(audio_frames, buffers.audio, buffers.pixels)
                        .map(|mut frame| {
                            frame.metrics.video_worker_us = t0.elapsed().as_micros() as u64;
                            frame
                        });
                    let failed = frame.is_err();
                    decode_pending_frames.fetch_add(1, Ordering::Relaxed);
                    if tx.send(frame).is_err() || failed {
                        decode_pending_frames.fetch_sub(1, Ordering::Relaxed);
                        break;
                    }
                }
            })
            .map_err(|e| format!("spawn video-decode: {e}"))?;
        let frame_interval = init_rx
            .recv()
            .map_err(|_| "video decode worker failed to start".to_string())??;
        Ok(Self {
            rx,
            recycle_tx,
            frame_interval,
            pending_frames,
        })
    }

    pub fn frame_interval(&self) -> Duration {
        self.frame_interval
    }

    pub fn try_recv(&self) -> Result<Option<PlaybackFrame>, String> {
        match self.rx.try_recv() {
            Ok(Ok(frame)) => {
                self.pending_frames.fetch_sub(1, Ordering::Relaxed);
                Ok(Some(frame))
            }
            Ok(Err(e)) => {
                self.pending_frames.fetch_sub(1, Ordering::Relaxed);
                Err(e)
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err("video decode worker stopped".into()),
        }
    }

    pub fn queue_depth(&self) -> u32 {
        self.pending_frames.load(Ordering::Relaxed)
    }

    pub fn recycle_audio(&self, mut audio: Vec<i16>) {
        audio.clear();
        let _ = self.recycle_tx.try_send(RecycledFrame {
            audio,
            pixels: Vec::new(),
        });
    }

    pub fn recycle_pixels(&self, mut pixels: Vec<u16>) {
        pixels.clear();
        let _ = self.recycle_tx.try_send(RecycledFrame {
            audio: Vec::new(),
            pixels,
        });
    }
}

struct AudioFramePacer {
    nanos_remainder: u128,
}

impl AudioFramePacer {
    fn new() -> Self {
        Self { nanos_remainder: 0 }
    }

    fn next_frames(&mut self, interval: Duration) -> usize {
        let total = AUDIO_RATE as u128 * interval.as_nanos() + self.nanos_remainder;
        let frames = total / 1_000_000_000;
        self.nanos_remainder = total % 1_000_000_000;
        frames as usize
    }
}

impl VideoPlayer {
    pub fn open(paths: Vec<String>) -> Result<Self, String> {
        ffmpeg::init().map_err(|e| format!("ffmpeg init failed: {e}"))?;
        if paths.is_empty() {
            return Err("video playlist is empty".into());
        }
        let player = Self::open_inner(paths, 0, 0)?;
        crate::ui_logln!(
            "video: opened {} (playlist={} file=1; {}x{}, video={}, frame_interval={}us; audio={} {}Hz {}ch {} -> 48000Hz stereo s16; scale={} pipeline={})",
            player.path,
            player.playlist.len(),
            player.video_decoder.width(),
            player.video_decoder.height(),
            player.video_codec,
            player.frame_interval.as_micros(),
            player.audio_codec,
            player.audio_rate,
            player.audio_channels,
            player.audio_decoder.format().name(),
            player.scale_mode.label(),
            player.video_pipeline_label()
        );
        Ok(player)
    }

    pub fn frame_interval(&self) -> Duration {
        self.frame_interval
    }

    fn open_inner(
        playlist: Vec<String>,
        playlist_index: usize,
        loop_count: u64,
    ) -> Result<Self, String> {
        let path = playlist
            .get(playlist_index)
            .ok_or_else(|| format!("playlist index {playlist_index} out of range"))?
            .clone();
        let input = format::input(&path).map_err(|e| format!("open {path}: {e}"))?;
        let video_stream = input
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| format!("{path}: no video stream"))?;
        let audio_stream = input
            .streams()
            .best(media::Type::Audio)
            .ok_or_else(|| format!("{path}: no audio stream"))?;

        let video_stream_index = video_stream.index();
        let audio_stream_index = audio_stream.index();
        let frame_interval = stream_frame_interval(&video_stream);
        let video_codec = video_stream.parameters().id().name().to_string();
        let audio_codec = audio_stream.parameters().id().name().to_string();

        let video_context = codec::context::Context::from_parameters(video_stream.parameters())
            .map_err(|e| format!("decoder parameters: {e}"))?;
        let video_decoder = video_context
            .decoder()
            .video()
            .map_err(|e| format!("open video decoder: {e}"))?;
        if video_decoder.width() != CANONICAL_VIDEO_WIDTH
            || video_decoder.height() != CANONICAL_VIDEO_HEIGHT
        {
            return Err(format!(
                "{path}: expected canonical {}x{} video, got {}x{}",
                CANONICAL_VIDEO_WIDTH,
                CANONICAL_VIDEO_HEIGHT,
                video_decoder.width(),
                video_decoder.height()
            ));
        }

        let audio_context = codec::context::Context::from_parameters(audio_stream.parameters())
            .map_err(|e| format!("audio decoder parameters: {e}"))?;
        let audio_decoder = audio_context
            .decoder()
            .audio()
            .map_err(|e| format!("open audio decoder: {e}"))?;
        let audio_layout = decoder_audio_layout(&audio_decoder)?;
        let audio_rate = audio_decoder.rate();
        let audio_channels = audio_decoder.channels();
        let audio_resampler = ResamplingContext::get(
            audio_decoder.format(),
            audio_layout,
            audio_rate,
            Sample::I16(SampleType::Packed),
            ChannelLayout::STEREO,
            AUDIO_RATE,
        )
        .map_err(|e| format!("create audio resampler: {e}"))?;

        // Presentation size is animated by the UI loop. Keep decode/conversion at the
        // canonical source geometry so toggling never restarts FFmpeg or changes its load.
        let scale_mode = VideoScaleMode::Source;
        let (output_w, output_h) = scale_mode.output_dimensions(
            video_decoder.width(),
            video_decoder.height(),
            DEFAULT_VIDEO_MAX_W,
            DEFAULT_VIDEO_MAX_H,
        );

        Ok(Self {
            playlist,
            playlist_index,
            path,
            input,
            video_stream_index,
            audio_stream_index,
            video_decoder,
            audio_decoder,
            audio_resampler,
            frame_interval,
            audio_rate,
            audio_channels,
            queued_audio: Vec::new(),
            audio_start: 0,
            pending_video_packets: VecDeque::new(),
            pending_audio_decode_us: 0,
            pending_audio_resample_us: 0,
            loop_count,
            video_codec,
            audio_codec,
            scale_mode,
            output_width: output_w,
            output_height: output_h,
        })
    }

    fn rewind(&mut self) -> Result<(), String> {
        let playlist = self.playlist.clone();
        let playlist_index = (self.playlist_index + 1) % playlist.len();
        let loop_count = self.loop_count + 1;
        *self = Self::open_inner(playlist, playlist_index, loop_count)?;
        crate::ui_logln!(
            "video: advanced to {} (file={} playlist={} loops={})",
            self.path,
            self.playlist_index + 1,
            self.playlist.len(),
            self.loop_count
        );
        self.loop_count = loop_count;
        Ok(())
    }

    fn next_frame_into(
        &mut self,
        audio_frames: usize,
        mut audio: Vec<i16>,
        mut pixels: Vec<u16>,
    ) -> Result<PlaybackFrame, String> {
        for _ in 0..2 {
            if let Some(frame) = self.next_frame_until_eof_into(audio_frames, audio, pixels)? {
                return Ok(frame);
            }
            audio = Vec::new();
            pixels = Vec::new();
            self.rewind()?;
        }
        Err("media decode reached EOF twice without a video frame".into())
    }

    fn next_frame_until_eof_into(
        &mut self,
        audio_frames: usize,
        mut audio: Vec<i16>,
        mut pixels: Vec<u16>,
    ) -> Result<Option<PlaybackFrame>, String> {
        if let Some((frame, video_metrics)) = receive_rgb565_frame(
            &mut self.video_decoder,
            self.scale_mode,
            self.output_width,
            self.output_height,
            pixels,
        )? {
            self.ensure_audio(audio_frames)?;
            self.take_audio_into(audio_frames, &mut audio);
            return Ok(Some(PlaybackFrame {
                frame,
                audio,
                audio_requested_frames: audio_frames,
                loop_count: self.loop_count,
                metrics: self.take_playback_metrics(video_metrics),
            }));
        }
        pixels = Vec::new();

        while let Some(packet) = self.pending_video_packets.pop_front() {
            if let Some((frame, video_metrics)) = self.send_video_packet(packet, pixels)? {
                self.ensure_audio(audio_frames)?;
                self.take_audio_into(audio_frames, &mut audio);
                return Ok(Some(PlaybackFrame {
                    frame,
                    audio,
                    audio_requested_frames: audio_frames,
                    loop_count: self.loop_count,
                    metrics: self.take_playback_metrics(video_metrics),
                }));
            }
            pixels = Vec::new();
        }

        loop {
            let Some((stream, packet)) = self.input.packets().next() else {
                break;
            };
            let stream_index = stream.index();
            if stream_index == self.video_stream_index {
                if let Some((frame, video_metrics)) = self.send_video_packet(packet, pixels)? {
                    self.ensure_audio(audio_frames)?;
                    self.take_audio_into(audio_frames, &mut audio);
                    return Ok(Some(PlaybackFrame {
                        frame,
                        audio,
                        audio_requested_frames: audio_frames,
                        loop_count: self.loop_count,
                        metrics: self.take_playback_metrics(video_metrics),
                    }));
                }
                pixels = Vec::new();
            } else if stream_index == self.audio_stream_index {
                let mut metrics = AudioDecodeMetrics::default();
                append_decoded_audio_packet(
                    &packet,
                    &mut self.audio_decoder,
                    &mut self.audio_resampler,
                    &mut self.queued_audio,
                    &mut metrics,
                )?;
                self.pending_audio_decode_us = self
                    .pending_audio_decode_us
                    .saturating_add(metrics.decode_us);
                self.pending_audio_resample_us = self
                    .pending_audio_resample_us
                    .saturating_add(metrics.resample_us);
            }
        }

        let _ = self.video_decoder.send_eof();
        let _ = self.audio_decoder.send_eof();
        let mut metrics = AudioDecodeMetrics::default();
        drain_decoded_audio(
            &mut self.audio_decoder,
            &mut self.audio_resampler,
            &mut self.queued_audio,
            &mut metrics,
        )?;
        self.add_pending_audio_metrics(metrics);
        if let Some((frame, video_metrics)) = receive_rgb565_frame(
            &mut self.video_decoder,
            self.scale_mode,
            self.output_width,
            self.output_height,
            pixels,
        )? {
            self.ensure_audio(audio_frames)?;
            self.take_audio_into(audio_frames, &mut audio);
            return Ok(Some(PlaybackFrame {
                frame,
                audio,
                audio_requested_frames: audio_frames,
                loop_count: self.loop_count,
                metrics: self.take_playback_metrics(video_metrics),
            }));
        }
        Ok(None)
    }

    fn ensure_audio(&mut self, frames: usize) -> Result<(), String> {
        let target_samples = frames * OUTPUT_AUDIO_CHANNELS;
        if self.queued_audio.len().saturating_sub(self.audio_start) >= target_samples {
            return Ok(());
        }

        for item in self.input.packets() {
            let (stream, packet) = item;
            let stream_index = stream.index();
            if stream_index == self.audio_stream_index {
                let mut metrics = AudioDecodeMetrics::default();
                append_decoded_audio_packet(
                    &packet,
                    &mut self.audio_decoder,
                    &mut self.audio_resampler,
                    &mut self.queued_audio,
                    &mut metrics,
                )?;
                self.pending_audio_decode_us = self
                    .pending_audio_decode_us
                    .saturating_add(metrics.decode_us);
                self.pending_audio_resample_us = self
                    .pending_audio_resample_us
                    .saturating_add(metrics.resample_us);
                if self.queued_audio.len().saturating_sub(self.audio_start) >= target_samples {
                    return Ok(());
                }
            } else if stream_index == self.video_stream_index {
                self.pending_video_packets.push_back(packet);
            }
        }
        Ok(())
    }

    fn send_video_packet(
        &mut self,
        packet: Packet,
        pixels: Vec<u16>,
    ) -> Result<Option<(VideoRgb565Frame, VideoDecodeMetrics)>, String> {
        match self.video_decoder.send_packet(&packet) {
            Ok(()) => receive_rgb565_frame(
                &mut self.video_decoder,
                self.scale_mode,
                self.output_width,
                self.output_height,
                pixels,
            ),
            Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => {
                let frame = receive_rgb565_frame(
                    &mut self.video_decoder,
                    self.scale_mode,
                    self.output_width,
                    self.output_height,
                    pixels,
                )?;
                self.pending_video_packets.push_front(packet);
                Ok(frame)
            }
            Err(e) => Err(format!("send video packet: {e}")),
        }
    }

    fn take_audio_into(&mut self, frames: usize, out: &mut Vec<i16>) {
        let samples = frames * OUTPUT_AUDIO_CHANNELS;
        let available = self.queued_audio.len().saturating_sub(self.audio_start);
        let n = samples.min(available);
        out.clear();
        out.extend_from_slice(&self.queued_audio[self.audio_start..self.audio_start + n]);
        self.audio_start += n;
        if self.audio_start > 8192 && self.audio_start * 2 > self.queued_audio.len() {
            self.queued_audio.drain(..self.audio_start);
            self.audio_start = 0;
        }
    }

    fn add_pending_audio_metrics(&mut self, metrics: AudioDecodeMetrics) {
        self.pending_audio_decode_us = self
            .pending_audio_decode_us
            .saturating_add(metrics.decode_us);
        self.pending_audio_resample_us = self
            .pending_audio_resample_us
            .saturating_add(metrics.resample_us);
    }

    fn take_playback_metrics(&mut self, video: VideoDecodeMetrics) -> PlaybackMetrics {
        let audio_decode_us = std::mem::take(&mut self.pending_audio_decode_us);
        let audio_resample_us = std::mem::take(&mut self.pending_audio_resample_us);
        PlaybackMetrics {
            video_pipeline: self.video_pipeline_label().into(),
            video_decode_us: video.decode_us,
            video_convert_us: video.convert_us,
            video_scale_2x_us: video.scale_2x_us,
            video_fused_us: video.fused_us,
            video_pipeline_us: if video.fused_us > 0 {
                video.fused_us
            } else {
                video.convert_us.saturating_add(video.scale_2x_us)
            },
            video_worker_us: 0,
            video_scale_us: video.scale_us,
            audio_decode_us,
            audio_resample_us,
            audio_buffer_frames: (self.queued_audio.len().saturating_sub(self.audio_start)
                / OUTPUT_AUDIO_CHANNELS) as u32,
            video_file: self.path.clone(),
            video_width: self.video_decoder.width(),
            video_height: self.video_decoder.height(),
            video_codec: self.video_codec.clone(),
            audio_codec: self.audio_codec.clone(),
        }
    }

    fn video_pipeline_label(&self) -> &'static str {
        match self.scale_mode {
            VideoScaleMode::Source => "rust-source",
            VideoScaleMode::Double => "rust-fused-2x",
        }
    }
}

fn receive_rgb565_frame(
    decoder: &mut ffmpeg::decoder::Video,
    scale_mode: VideoScaleMode,
    output_width: u32,
    output_height: u32,
    pixels: Vec<u16>,
) -> Result<Option<(VideoRgb565Frame, VideoDecodeMetrics)>, String> {
    let mut decoded = Video::empty();
    let decode_t0 = Instant::now();
    match decoder.receive_frame(&mut decoded) {
        Ok(()) => {
            let decode_us = decode_t0.elapsed().as_micros() as u64;
            let mut convert_us = 0;
            let mut fused_us = 0;
            let scale_us = 0;
            let frame = rgb565_frame_from_i420(
                &decoded,
                scale_mode,
                output_width,
                output_height,
                pixels,
                &mut convert_us,
                &mut fused_us,
            )?;
            Ok(Some((
                frame,
                VideoDecodeMetrics {
                    decode_us,
                    convert_us,
                    scale_2x_us: 0,
                    fused_us,
                    scale_us,
                },
            )))
        }
        Err(_) => Ok(None),
    }
}

fn rgb565_frame_from_i420(
    frame: &Video,
    scale_mode: VideoScaleMode,
    output_width: u32,
    output_height: u32,
    pixels: Vec<u16>,
    convert_us: &mut u64,
    fused_us: &mut u64,
) -> Result<VideoRgb565Frame, String> {
    if frame.format() != FfmpegPixel::YUV420P {
        return Err(format!(
            "video frame format {:?} at {}x{} is unsupported; production video requires YUV420P",
            frame.format(),
            frame.width(),
            frame.height()
        ));
    }

    let width = frame.width();
    let height = frame.height();
    let expected =
        scale_mode.output_dimensions(width, height, DEFAULT_VIDEO_MAX_W, DEFAULT_VIDEO_MAX_H);
    if expected != (output_width, output_height) {
        return Err(format!(
            "video output geometry mismatch: expected {}x{}, got {}x{}",
            expected.0, expected.1, output_width, output_height
        ));
    }

    if scale_mode == VideoScaleMode::Double {
        let mut pixels =
            recycled_rgb565_pixels(pixels, output_width as usize * output_height as usize);
        let started = Instant::now();
        convert_i420_to_rgb565_2x_rust_optimized(
            frame.data(0),
            frame_stride_usize(frame, 0),
            frame.data(1),
            frame_stride_usize(frame, 1),
            frame.data(2),
            frame_stride_usize(frame, 2),
            &mut pixels,
            output_width as usize,
            width as usize,
            height as usize,
        )?;
        *fused_us = started.elapsed().as_micros() as u64;
        return Ok(VideoRgb565Frame {
            pixels,
            width: output_width,
            height: output_height,
        });
    }

    let mut pixels = recycled_rgb565_pixels(pixels, width as usize * height as usize);
    let convert_t0 = Instant::now();
    convert_i420_to_rgb565(
        frame.data(0),
        frame_stride_usize(frame, 0),
        frame.data(1),
        frame_stride_usize(frame, 1),
        frame.data(2),
        frame_stride_usize(frame, 2),
        &mut pixels,
        width as usize,
        width as usize,
        height as usize,
    )?;
    *convert_us = convert_t0.elapsed().as_micros() as u64;

    Ok(VideoRgb565Frame {
        pixels,
        width: output_width,
        height: output_height,
    })
}

fn recycled_rgb565_pixels(mut pixels: Vec<u16>, len: usize) -> Vec<u16> {
    pixels.resize(len, 0);
    pixels
}

fn frame_stride_usize(frame: &Video, plane: usize) -> usize {
    frame.stride(plane)
}

fn append_decoded_audio_packet(
    packet: &ffmpeg::codec::packet::Packet,
    decoder: &mut ffmpeg::decoder::Audio,
    resampler: &mut ResamplingContext,
    queued_audio: &mut Vec<i16>,
    metrics: &mut AudioDecodeMetrics,
) -> Result<(), String> {
    let decode_t0 = Instant::now();
    decoder
        .send_packet(packet)
        .map_err(|e| format!("send audio packet: {e}"))?;
    metrics.decode_us = metrics
        .decode_us
        .saturating_add(decode_t0.elapsed().as_micros() as u64);
    drain_decoded_audio(decoder, resampler, queued_audio, metrics)
}

fn drain_decoded_audio(
    decoder: &mut ffmpeg::decoder::Audio,
    resampler: &mut ResamplingContext,
    queued_audio: &mut Vec<i16>,
    metrics: &mut AudioDecodeMetrics,
) -> Result<(), String> {
    let mut decoded = Audio::empty();
    loop {
        let decode_t0 = Instant::now();
        let received = decoder.receive_frame(&mut decoded);
        metrics.decode_us = metrics
            .decode_us
            .saturating_add(decode_t0.elapsed().as_micros() as u64);
        if received.is_err() {
            break;
        }
        let mut resampled = Audio::empty();
        let resample_t0 = Instant::now();
        resampler
            .run(&decoded, &mut resampled)
            .map_err(|e| format!("resample audio frame: {e}"))?;
        metrics.resample_us = metrics
            .resample_us
            .saturating_add(resample_t0.elapsed().as_micros() as u64);
        append_resampled_stereo_i16(&resampled, queued_audio)?;
    }
    Ok(())
}

fn append_resampled_stereo_i16(frame: &Audio, queued_audio: &mut Vec<i16>) -> Result<(), String> {
    if frame.format() != Sample::I16(SampleType::Packed) || frame.channels() != 2 {
        return Err(format!(
            "resampler produced {} with {} channels, expected packed stereo s16",
            frame.format().name(),
            frame.channels()
        ));
    }
    queued_audio.reserve(frame.samples() * OUTPUT_AUDIO_CHANNELS);
    for &(left, right) in frame.plane::<(i16, i16)>(0) {
        queued_audio.push(left);
        queued_audio.push(right);
    }
    Ok(())
}

fn decoder_audio_layout(decoder: &ffmpeg::decoder::Audio) -> Result<ChannelLayout, String> {
    let layout = decoder.channel_layout();
    if !layout.is_empty() {
        return Ok(layout);
    }
    match decoder.channels() {
        1 => Ok(ChannelLayout::MONO),
        2 => Ok(ChannelLayout::STEREO),
        channels => Err(format!(
            "audio decoder did not report a channel layout for {channels} channels"
        )),
    }
}

fn stream_frame_interval(stream: &format::stream::Stream<'_>) -> Duration {
    let rate = stream.avg_frame_rate();
    let num = rate.0;
    let den = rate.1;
    if num > 0 && den > 0 {
        let secs = den as f64 / num as f64;
        Duration::from_secs_f64(secs)
    } else {
        Duration::from_nanos(16_666_667)
    }
}
