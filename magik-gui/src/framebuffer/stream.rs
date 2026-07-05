use mister_magik_framebuffer_stream::{
    write_frame, FrameGeometry, FrameHeader, FrameKind, FrameRect, FLAG_LZ4_SIZE_PREPENDED,
};
use slint::platform::software_renderer::Rgb565Pixel;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

pub const PRODUCER_STREAM_PORT: u16 = 7499;

static STATE: OnceLock<Mutex<ProducerState>> = OnceLock::new();
static STARTED_AT: OnceLock<Instant> = OnceLock::new();
static LISTENER_STARTED: OnceLock<()> = OnceLock::new();

struct ProducerState {
    subscriber: Option<TcpStream>,
    sequence: u64,
    needs_keyframe: bool,
}

impl ProducerState {
    fn new() -> Self {
        Self {
            subscriber: None,
            sequence: 0,
            needs_keyframe: true,
        }
    }
}

pub fn start() {
    let _ = STARTED_AT.get_or_init(Instant::now);
    let _ = STATE.get_or_init(|| Mutex::new(ProducerState::new()));
    let _ = LISTENER_STARTED.get_or_init(|| {
        thread::spawn(move || {
            let listener = match TcpListener::bind(("127.0.0.1", PRODUCER_STREAM_PORT)) {
                Ok(listener) => listener,
                Err(err) => {
                    crate::ui_errln!("framebuffer stream producer bind failed: {err}");
                    return;
                }
            };
            crate::ui_logln!("framebuffer stream producer listening port={PRODUCER_STREAM_PORT}");
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => install_subscriber(stream),
                    Err(err) => {
                        crate::ui_errln!("framebuffer stream producer accept failed: {err}")
                    }
                }
            }
        });
    });
}

pub fn publish_cached_rect(geometry: FrameGeometry, rect: FrameRect, pixels: &[Rgb565Pixel]) {
    let Ok(mut state) = state().lock() else {
        return;
    };
    if state.subscriber.is_none() {
        return;
    }
    let rect = if state.needs_keyframe {
        FrameRect::full(geometry)
    } else {
        rect
    };
    let kind = if state.needs_keyframe {
        FrameKind::Keyframe
    } else {
        FrameKind::RectDelta
    };
    publish_rect_locked(
        &mut state,
        geometry,
        rect,
        kind,
        pixels,
        geometry.stride_pixels as usize,
    );
}

pub fn publish_dense_rect(geometry: FrameGeometry, rect: FrameRect, pixels: &[Rgb565Pixel]) {
    let Ok(mut state) = state().lock() else {
        return;
    };
    if state.subscriber.is_none() || state.needs_keyframe {
        return;
    }
    publish_rect_locked(
        &mut state,
        geometry,
        rect,
        FrameKind::RectDelta,
        pixels,
        rect.width as usize,
    );
}

pub fn publish_strided_rect(
    geometry: FrameGeometry,
    rect: FrameRect,
    pixels: &[Rgb565Pixel],
    src_stride: usize,
    src_x: usize,
    src_y: usize,
) {
    let Ok(mut state) = state().lock() else {
        return;
    };
    if state.subscriber.is_none() || state.needs_keyframe {
        return;
    }
    let Some(rect_pixels) = collect_strided_rect(pixels, src_stride, src_x, src_y, rect) else {
        return;
    };
    publish_rect_locked(
        &mut state,
        geometry,
        rect,
        FrameKind::RectDelta,
        &rect_pixels,
        rect.width as usize,
    );
}

fn install_subscriber(stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
    if let Ok(mut state) = state().lock() {
        state.subscriber = Some(stream);
        state.needs_keyframe = true;
        crate::ui_logln!("framebuffer stream producer subscriber connected");
    }
}

fn publish_rect_locked(
    state: &mut ProducerState,
    geometry: FrameGeometry,
    rect: FrameRect,
    kind: FrameKind,
    pixels: &[Rgb565Pixel],
    stride_pixels: usize,
) {
    let raw = rgb565_rect_bytes(pixels, rect, stride_pixels);
    if raw.is_empty() {
        return;
    }
    let payload = lz4_flex::compress_prepend_size(&raw);
    state.sequence = state.sequence.saturating_add(1);
    let header = FrameHeader {
        kind,
        flags: FLAG_LZ4_SIZE_PREPENDED,
        sequence: state.sequence,
        timestamp_us: timestamp_us(),
        geometry,
        rect,
        raw_bytes: raw.len() as u32,
        payload_bytes: payload.len() as u32,
    };
    if let Err(err) = write_to_subscriber(state, header, &payload) {
        crate::ui_errln!("framebuffer stream producer write failed: {err}");
        state.subscriber = None;
        state.needs_keyframe = true;
        return;
    }
    state.needs_keyframe = false;
}

fn write_to_subscriber(
    state: &mut ProducerState,
    header: FrameHeader,
    payload: &[u8],
) -> io::Result<()> {
    let Some(stream) = state.subscriber.as_mut() else {
        return Ok(());
    };
    write_frame(stream, header, payload)
}

fn state() -> &'static Mutex<ProducerState> {
    STATE.get_or_init(|| Mutex::new(ProducerState::new()))
}

fn timestamp_us() -> u64 {
    STARTED_AT.get_or_init(Instant::now).elapsed().as_micros() as u64
}

fn collect_strided_rect(
    pixels: &[Rgb565Pixel],
    src_stride: usize,
    src_x: usize,
    src_y: usize,
    rect: FrameRect,
) -> Option<Vec<Rgb565Pixel>> {
    let width = rect.width as usize;
    let height = rect.height as usize;
    if width == 0 || height == 0 || src_stride < width || src_x > src_stride {
        return None;
    }
    let mut out = Vec::with_capacity(width.checked_mul(height)?);
    for row in 0..height {
        let src_row = src_y.checked_add(row)?;
        let start = src_row.checked_mul(src_stride)?.checked_add(src_x)?;
        let end = start.checked_add(width)?;
        out.extend_from_slice(pixels.get(start..end)?);
    }
    Some(out)
}

fn rgb565_rect_bytes(pixels: &[Rgb565Pixel], rect: FrameRect, stride_pixels: usize) -> Vec<u8> {
    let width = rect.width as usize;
    let height = rect.height as usize;
    let mut out = Vec::with_capacity(width.saturating_mul(height).saturating_mul(2));
    for row in 0..height {
        let start = row.saturating_mul(stride_pixels);
        let end = start.saturating_add(width);
        let Some(row_pixels) = pixels.get(start..end) else {
            return Vec::new();
        };
        for pixel in row_pixels {
            out.extend_from_slice(&pixel.0.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strided_rect_collection_extracts_rows() {
        let pixels = (0..12).map(Rgb565Pixel).collect::<Vec<_>>();
        let rect = FrameRect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };

        let out = collect_strided_rect(&pixels, 4, 1, 1, rect).expect("strided rect");

        assert_eq!(
            out,
            vec![
                Rgb565Pixel(5),
                Rgb565Pixel(6),
                Rgb565Pixel(9),
                Rgb565Pixel(10)
            ]
        );
    }

    #[test]
    fn rgb565_rect_bytes_are_little_endian() {
        let pixels = [Rgb565Pixel(0x1234), Rgb565Pixel(0xabcd)];
        let rect = FrameRect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };

        assert_eq!(
            rgb565_rect_bytes(&pixels, rect, 2),
            vec![0x34, 0x12, 0xcd, 0xab]
        );
    }
}
