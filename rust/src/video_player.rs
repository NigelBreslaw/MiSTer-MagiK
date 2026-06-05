//! FFmpeg-backed video frame pump for the Slint video benchmark.

use ffmpeg::codec;
use ffmpeg::format;
use ffmpeg::media;
use ffmpeg::software::scaling::{context::Context as ScalingContext, flag::Flags};
use ffmpeg::util::format::pixel::Pixel as FfmpegPixel;
use ffmpeg::util::frame::video::Video;
use ffmpeg_the_third as ffmpeg;
use slint::{Image, Rgb8Pixel, SharedPixelBuffer};
use std::path::Path;
use std::time::Duration;

pub const DEFAULT_VIDEO_PATH: &str = "/media/fat/mister-magic/mslug3.mp4";

pub struct VideoPlayer {
    path: String,
    input: format::context::Input,
    stream_index: usize,
    decoder: ffmpeg::decoder::Video,
    scaler: ScalingContext,
    frame_interval: Duration,
}

impl VideoPlayer {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        ffmpeg::init().map_err(|e| format!("ffmpeg init failed: {e}"))?;
        let path = path.as_ref().display().to_string();
        let player = Self::open_inner(path)?;
        println!(
            "video: opened {} ({}x{}, frame_interval={}us)",
            player.path,
            player.decoder.width(),
            player.decoder.height(),
            player.frame_interval.as_micros()
        );
        Ok(player)
    }

    pub fn frame_interval(&self) -> Duration {
        self.frame_interval
    }

    pub fn next_image(&mut self) -> Result<Image, String> {
        for _ in 0..2 {
            if let Some(image) = self.next_image_until_eof()? {
                return Ok(image);
            }
            self.rewind()?;
        }
        Err("video decode reached EOF twice without a frame".into())
    }

    fn open_inner(path: String) -> Result<Self, String> {
        let input = format::input(&path).map_err(|e| format!("open {path}: {e}"))?;
        let stream = input
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| format!("{path}: no video stream"))?;
        let stream_index = stream.index();
        let frame_interval = stream_frame_interval(&stream);
        let context = codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| format!("decoder parameters: {e}"))?;
        let decoder = context
            .decoder()
            .video()
            .map_err(|e| format!("open video decoder: {e}"))?;
        let scaler = ScalingContext::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            FfmpegPixel::RGB24,
            decoder.width(),
            decoder.height(),
            Flags::BILINEAR,
        )
        .map_err(|e| format!("create RGB scaler: {e}"))?;

        Ok(Self {
            path,
            input,
            stream_index,
            decoder,
            scaler,
            frame_interval,
        })
    }

    fn rewind(&mut self) -> Result<(), String> {
        let path = self.path.clone();
        *self = Self::open_inner(path)?;
        Ok(())
    }

    fn next_image_until_eof(&mut self) -> Result<Option<Image>, String> {
        if let Some(image) = receive_image(&mut self.decoder, &mut self.scaler)? {
            return Ok(Some(image));
        }

        let stream_index = self.stream_index;
        let decoder = &mut self.decoder;
        let scaler = &mut self.scaler;
        for item in self.input.packets() {
            let (stream, packet) = item.map_err(|e| format!("read video packet: {e}"))?;
            if stream.index() != stream_index {
                continue;
            }
            decoder
                .send_packet(&packet)
                .map_err(|e| format!("send video packet: {e}"))?;
            if let Some(image) = receive_image(decoder, scaler)? {
                return Ok(Some(image));
            }
        }

        let _ = decoder.send_eof();
        receive_image(decoder, scaler)
    }
}

fn receive_image(
    decoder: &mut ffmpeg::decoder::Video,
    scaler: &mut ScalingContext,
) -> Result<Option<Image>, String> {
    let mut decoded = Video::empty();
    match decoder.receive_frame(&mut decoded) {
        Ok(()) => {
            let mut rgb = Video::empty();
            scaler
                .run(&decoded, &mut rgb)
                .map_err(|e| format!("scale video frame: {e}"))?;
            Ok(Some(rgb_frame_to_slint_image(&rgb)))
        }
        Err(_) => Ok(None),
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

fn rgb_frame_to_slint_image(frame: &Video) -> Image {
    let width = frame.width();
    let height = frame.height();
    let stride = frame.stride(0);
    let row_len = width as usize * 3;
    let data = frame.data(0);
    let mut rgb = vec![0u8; row_len * height as usize];
    for y in 0..height as usize {
        let src = &data[y * stride..y * stride + row_len];
        let dst = &mut rgb[y * row_len..(y + 1) * row_len];
        dst.copy_from_slice(src);
    }
    let buffer = SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(&rgb, width, height);
    Image::from_rgb8(buffer)
}
