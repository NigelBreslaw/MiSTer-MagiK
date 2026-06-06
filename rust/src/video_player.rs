//! FFmpeg-backed media pump for the Slint video benchmark.

use ffmpeg::codec;
use ffmpeg::format;
use ffmpeg::media;
use ffmpeg::software::scaling::{context::Context as ScalingContext, flag::Flags};
use ffmpeg::util::format::pixel::Pixel as FfmpegPixel;
use ffmpeg::util::frame::video::Video;
use ffmpeg_the_third as ffmpeg;
use slint::{Image, Rgb8Pixel, SharedPixelBuffer};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

pub const DEFAULT_VIDEO_PATH: &str = "/media/fat/mister-magic/mslug3.mov";
const AUDIO_RATE: u32 = 48_000;
const OUTPUT_AUDIO_CHANNELS: usize = 2;

pub struct VideoPlayer {
    path: String,
    input: format::context::Input,
    video_stream_index: usize,
    audio_stream_index: usize,
    video_decoder: ffmpeg::decoder::Video,
    scaler: ScalingContext,
    frame_interval: Duration,
    audio_rate: u32,
    audio_channels: u32,
    queued_audio: Vec<i16>,
    audio_start: usize,
    loop_count: u64,
}

pub struct PlaybackFrame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    pub audio: Vec<i16>,
    pub audio_requested_frames: usize,
    pub loop_count: u64,
}

#[derive(Default)]
struct RecycledFrame {
    rgb: Vec<u8>,
    audio: Vec<i16>,
}

pub struct VideoFrameWorker {
    rx: mpsc::Receiver<Result<PlaybackFrame, String>>,
    recycle_tx: mpsc::SyncSender<RecycledFrame>,
    frame_interval: Duration,
}

impl VideoFrameWorker {
    pub fn start(path: String) -> Result<Self, String> {
        let (tx, rx) = mpsc::sync_channel(2);
        let (recycle_tx, recycle_rx) = mpsc::sync_channel::<RecycledFrame>(2);
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("video-decode".to_string())
            .spawn(move || {
                lower_decode_thread_priority();
                let mut player = match VideoPlayer::open(&path) {
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
                    let buffers = recycle_rx.try_recv().unwrap_or_default();
                    let audio_frames = audio_pacer.next_frames(frame_interval);
                    let frame = player.next_frame_into(audio_frames, buffers.rgb, buffers.audio);
                    let failed = frame.is_err();
                    if tx.send(frame).is_err() || failed {
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
        })
    }

    pub fn frame_interval(&self) -> Duration {
        self.frame_interval
    }

    pub fn try_recv(&self) -> Result<Option<PlaybackFrame>, String> {
        match self.rx.try_recv() {
            Ok(Ok(frame)) => Ok(Some(frame)),
            Ok(Err(e)) => Err(e),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err("video decode worker stopped".into()),
        }
    }

    pub fn recycle(&self, mut frame: PlaybackFrame) {
        frame.rgb.clear();
        frame.audio.clear();
        let _ = self.recycle_tx.try_send(RecycledFrame {
            rgb: frame.rgb,
            audio: frame.audio,
        });
    }
}

fn lower_decode_thread_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 5);
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
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        ffmpeg::init().map_err(|e| format!("ffmpeg init failed: {e}"))?;
        let path = path.as_ref().display().to_string();
        let player = Self::open_inner(path)?;
        println!(
            "video: opened {} ({}x{}, frame_interval={}us; audio={}Hz {}ch pcm_s16le)",
            player.path,
            player.video_decoder.width(),
            player.video_decoder.height(),
            player.frame_interval.as_micros(),
            player.audio_rate,
            player.audio_channels
        );
        Ok(player)
    }

    pub fn frame_interval(&self) -> Duration {
        self.frame_interval
    }

    fn open_inner(path: String) -> Result<Self, String> {
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

        let video_context = codec::context::Context::from_parameters(video_stream.parameters())
            .map_err(|e| format!("decoder parameters: {e}"))?;
        let video_decoder = video_context
            .decoder()
            .video()
            .map_err(|e| format!("open video decoder: {e}"))?;

        let audio_parameters = audio_stream.parameters();
        validate_audio_parameters(&path, &audio_parameters)?;
        let audio_rate = audio_parameters.sample_rate();
        let audio_channels = audio_parameters.ch_layout().channels();

        let scaler = ScalingContext::get(
            video_decoder.format(),
            video_decoder.width(),
            video_decoder.height(),
            FfmpegPixel::RGB24,
            video_decoder.width(),
            video_decoder.height(),
            Flags::BILINEAR,
        )
        .map_err(|e| format!("create RGB scaler: {e}"))?;

        Ok(Self {
            path,
            input,
            video_stream_index,
            audio_stream_index,
            video_decoder,
            scaler,
            frame_interval,
            audio_rate,
            audio_channels,
            queued_audio: Vec::new(),
            audio_start: 0,
            loop_count: 0,
        })
    }

    fn rewind(&mut self) -> Result<(), String> {
        let path = self.path.clone();
        let loop_count = self.loop_count + 1;
        *self = Self::open_inner(path)?;
        self.loop_count = loop_count;
        Ok(())
    }

    fn next_frame_into(
        &mut self,
        audio_frames: usize,
        mut rgb: Vec<u8>,
        mut audio: Vec<i16>,
    ) -> Result<PlaybackFrame, String> {
        for _ in 0..2 {
            if let Some(frame) = self.next_frame_until_eof_into(audio_frames, rgb, audio)? {
                return Ok(frame);
            }
            rgb = Vec::new();
            audio = Vec::new();
            self.rewind()?;
        }
        Err("media decode reached EOF twice without a video frame".into())
    }

    fn next_frame_until_eof_into(
        &mut self,
        audio_frames: usize,
        mut rgb: Vec<u8>,
        mut audio: Vec<i16>,
    ) -> Result<Option<PlaybackFrame>, String> {
        if let Some((width, height)) =
            receive_rgb_frame_into(&mut self.video_decoder, &mut self.scaler, &mut rgb)?
        {
            self.ensure_audio(audio_frames)?;
            self.take_audio_into(audio_frames, &mut audio);
            return Ok(Some(PlaybackFrame {
                width,
                height,
                rgb,
                audio,
                audio_requested_frames: audio_frames,
                loop_count: self.loop_count,
            }));
        }

        for item in self.input.packets() {
            let (stream, packet) = item.map_err(|e| format!("read video packet: {e}"))?;
            let stream_index = stream.index();
            if stream_index == self.video_stream_index {
                self.video_decoder
                    .send_packet(&packet)
                    .map_err(|e| format!("send video packet: {e}"))?;
                if let Some((width, height)) =
                    receive_rgb_frame_into(&mut self.video_decoder, &mut self.scaler, &mut rgb)?
                {
                    self.ensure_audio(audio_frames)?;
                    self.take_audio_into(audio_frames, &mut audio);
                    return Ok(Some(PlaybackFrame {
                        width,
                        height,
                        rgb,
                        audio,
                        audio_requested_frames: audio_frames,
                        loop_count: self.loop_count,
                    }));
                }
            } else if stream_index == self.audio_stream_index {
                append_pcm_audio_packet(&packet, self.audio_channels, &mut self.queued_audio)?;
            }
        }

        let _ = self.video_decoder.send_eof();
        if let Some((width, height)) =
            receive_rgb_frame_into(&mut self.video_decoder, &mut self.scaler, &mut rgb)?
        {
            self.ensure_audio(audio_frames)?;
            self.take_audio_into(audio_frames, &mut audio);
            return Ok(Some(PlaybackFrame {
                width,
                height,
                rgb,
                audio,
                audio_requested_frames: audio_frames,
                loop_count: self.loop_count,
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
            let (stream, packet) = item.map_err(|e| format!("read audio packet: {e}"))?;
            let stream_index = stream.index();
            if stream_index == self.audio_stream_index {
                append_pcm_audio_packet(&packet, self.audio_channels, &mut self.queued_audio)?;
                if self.queued_audio.len().saturating_sub(self.audio_start) >= target_samples {
                    return Ok(());
                }
            } else if stream_index == self.video_stream_index {
                self.video_decoder
                    .send_packet(&packet)
                    .map_err(|e| format!("send video packet while filling audio: {e}"))?;
            }
        }
        Ok(())
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
}

fn receive_rgb_frame_into(
    decoder: &mut ffmpeg::decoder::Video,
    scaler: &mut ScalingContext,
    rgb_buf: &mut Vec<u8>,
) -> Result<Option<(u32, u32)>, String> {
    let mut decoded = Video::empty();
    match decoder.receive_frame(&mut decoded) {
        Ok(()) => {
            let mut rgb = Video::empty();
            scaler
                .run(&decoded, &mut rgb)
                .map_err(|e| format!("scale video frame: {e}"))?;
            Ok(Some(rgb_frame_to_vec_into(&rgb, rgb_buf)))
        }
        Err(_) => Ok(None),
    }
}

fn append_pcm_audio_packet(
    packet: &ffmpeg::codec::packet::Packet,
    input_channels: u32,
    queued_audio: &mut Vec<i16>,
) -> Result<(), String> {
    let Some(data) = packet.data() else {
        return Ok(());
    };
    let input_channels = input_channels as usize;
    let input_frame_bytes = input_channels * std::mem::size_of::<i16>();
    if input_frame_bytes == 0 || data.len() % input_frame_bytes != 0 {
        return Err(format!(
            "pcm_s16le packet has {} bytes, expected a multiple of {input_frame_bytes} for {input_channels}ch frames",
            data.len(),
        ));
    }
    queued_audio.reserve(data.len() / input_frame_bytes * OUTPUT_AUDIO_CHANNELS);
    match input_channels {
        1 => {
            for chunk in data.chunks_exact(2) {
                let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                queued_audio.push(sample);
                queued_audio.push(sample);
            }
        }
        2 => {
            for chunk in data.chunks_exact(2) {
                queued_audio.push(i16::from_le_bytes([chunk[0], chunk[1]]));
            }
        }
        _ => {
            return Err(format!(
                "pcm_s16le audio must be mono or stereo, got {input_channels} channels"
            ));
        }
    }
    Ok(())
}

fn validate_audio_parameters(
    path: &str,
    parameters: &ffmpeg::codec::ParametersRef<'_>,
) -> Result<(), String> {
    if parameters.id() != ffmpeg::codec::Id::PCM_S16LE {
        return Err(format!(
            "{path}: audio must be pcm_s16le, got {:?}",
            parameters.id()
        ));
    }
    if parameters.sample_rate() != AUDIO_RATE {
        return Err(format!(
            "{path}: audio must be {AUDIO_RATE}Hz PCM, got {}Hz",
            parameters.sample_rate()
        ));
    }
    let channels = parameters.ch_layout().channels();
    if channels != 1 && channels != 2 {
        return Err(format!(
            "{path}: audio must be mono or stereo PCM, got {} channels",
            channels
        ));
    }
    Ok(())
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

fn rgb_frame_to_vec_into(frame: &Video, rgb: &mut Vec<u8>) -> (u32, u32) {
    let width = frame.width();
    let height = frame.height();
    let stride = frame.stride(0);
    let row_len = width as usize * 3;
    let data = frame.data(0);
    rgb.resize(row_len * height as usize, 0);
    for y in 0..height as usize {
        let src = &data[y * stride..y * stride + row_len];
        let dst = &mut rgb[y * row_len..(y + 1) * row_len];
        dst.copy_from_slice(src);
    }
    (width, height)
}

pub fn rgb_image(width: u32, height: u32, rgb: &[u8]) -> Image {
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(rgb);
    Image::from_rgb8(buffer)
}
