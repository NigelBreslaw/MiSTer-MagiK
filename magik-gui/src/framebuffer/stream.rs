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
    latest_geometry: Option<FrameGeometry>,
    latest_pixels: Vec<Rgb565Pixel>,
}

impl ProducerState {
    fn new() -> Self {
        Self {
            subscriber: None,
            sequence: 0,
            needs_keyframe: true,
            latest_geometry: None,
            latest_pixels: Vec::new(),
        }
    }

    fn remember_rect(
        &mut self,
        geometry: FrameGeometry,
        rect: FrameRect,
        pixels: &[Rgb565Pixel],
        src_stride: usize,
        src_x: usize,
        src_y: usize,
    ) {
        let Some(rect_pixels) = collect_strided_rect(pixels, src_stride, src_x, src_y, rect) else {
            return;
        };
        let len = geometry
            .stride_pixels
            .checked_mul(geometry.height)
            .map(|pixels| pixels as usize)
            .unwrap_or(0);
        if self.latest_geometry != Some(geometry) || self.latest_pixels.len() != len {
            self.latest_pixels.clear();
            self.latest_pixels.resize(len, Rgb565Pixel(0));
            self.latest_geometry = Some(geometry);
        }
        copy_rect_into_latest(&mut self.latest_pixels, geometry, rect, &rect_pixels);
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
    state.remember_rect(
        geometry,
        rect,
        pixels,
        geometry.stride_pixels as usize,
        rect.x as usize,
        rect.y as usize,
    );
    if state.subscriber.is_none() {
        return;
    }
    if state.needs_keyframe {
        publish_latest_keyframe_locked(&mut state);
        return;
    }
    publish_rect_locked(
        &mut state,
        geometry,
        rect,
        FrameKind::RectDelta,
        pixels,
        geometry.stride_pixels as usize,
        rect.x as usize,
        rect.y as usize,
    );
}

pub fn publish_dense_rect(geometry: FrameGeometry, rect: FrameRect, pixels: &[Rgb565Pixel]) {
    let Ok(mut state) = state().lock() else {
        return;
    };
    state.remember_rect(geometry, rect, pixels, rect.width as usize, 0, 0);
    if state.subscriber.is_none() {
        return;
    }
    if state.needs_keyframe {
        publish_latest_keyframe_locked(&mut state);
        return;
    }
    publish_rect_locked(
        &mut state,
        geometry,
        rect,
        FrameKind::RectDelta,
        pixels,
        rect.width as usize,
        0,
        0,
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
    state.remember_rect(geometry, rect, pixels, src_stride, src_x, src_y);
    if state.subscriber.is_none() {
        return;
    }
    if state.needs_keyframe {
        publish_latest_keyframe_locked(&mut state);
        return;
    };
    publish_rect_locked(
        &mut state,
        geometry,
        rect,
        FrameKind::RectDelta,
        pixels,
        src_stride,
        src_x,
        src_y,
    );
}

fn install_subscriber(stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
    if let Ok(mut state) = state().lock() {
        state.subscriber = Some(stream);
        state.needs_keyframe = true;
        publish_latest_keyframe_locked(&mut state);
        crate::ui_logln!("framebuffer stream producer subscriber connected");
    }
}

fn publish_latest_keyframe_locked(state: &mut ProducerState) {
    let Some(geometry) = state.latest_geometry else {
        return;
    };
    let pixels = state.latest_pixels.clone();
    publish_rect_locked(
        state,
        geometry,
        FrameRect::full(geometry),
        FrameKind::Keyframe,
        &pixels,
        geometry.stride_pixels as usize,
        0,
        0,
    );
}

fn publish_rect_locked(
    state: &mut ProducerState,
    geometry: FrameGeometry,
    rect: FrameRect,
    kind: FrameKind,
    pixels: &[Rgb565Pixel],
    stride_pixels: usize,
    src_x: usize,
    src_y: usize,
) {
    let raw = rgb565_rect_bytes(pixels, rect, stride_pixels, src_x, src_y);
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

fn copy_rect_into_latest(
    latest: &mut [Rgb565Pixel],
    geometry: FrameGeometry,
    rect: FrameRect,
    rect_pixels: &[Rgb565Pixel],
) {
    let width = rect.width as usize;
    let height = rect.height as usize;
    let stride = geometry.stride_pixels as usize;
    for row in 0..height {
        let src = row.saturating_mul(width);
        let dst = (rect.y as usize + row)
            .saturating_mul(stride)
            .saturating_add(rect.x as usize);
        let Some(src_row) = rect_pixels.get(src..src.saturating_add(width)) else {
            return;
        };
        let Some(dst_row) = latest.get_mut(dst..dst.saturating_add(width)) else {
            return;
        };
        dst_row.copy_from_slice(src_row);
    }
}

fn rgb565_rect_bytes(
    pixels: &[Rgb565Pixel],
    rect: FrameRect,
    stride_pixels: usize,
    src_x: usize,
    src_y: usize,
) -> Vec<u8> {
    let width = rect.width as usize;
    let height = rect.height as usize;
    let mut out = Vec::with_capacity(width.saturating_mul(height).saturating_mul(2));
    for row in 0..height {
        let start = src_y
            .saturating_add(row)
            .saturating_mul(stride_pixels)
            .saturating_add(src_x);
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
            rgb565_rect_bytes(&pixels, rect, 2, 0, 0),
            vec![0x34, 0x12, 0xcd, 0xab]
        );
    }

    #[test]
    fn rgb565_rect_bytes_use_source_offset() {
        let pixels = (0..12).map(Rgb565Pixel).collect::<Vec<_>>();
        let rect = FrameRect {
            x: 1,
            y: 2,
            width: 2,
            height: 2,
        };

        assert_eq!(
            rgb565_rect_bytes(&pixels, rect, 4, 1, 1),
            vec![5, 0, 6, 0, 9, 0, 10, 0]
        );
    }

    #[test]
    fn producer_state_remembers_dirty_rects_for_keyframe() {
        let geometry = FrameGeometry {
            width: 4,
            height: 3,
            stride_pixels: 4,
        };
        let mut state = ProducerState::new();
        let full = (0..12).map(Rgb565Pixel).collect::<Vec<_>>();
        state.remember_rect(
            geometry,
            FrameRect {
                x: 0,
                y: 0,
                width: 4,
                height: 3,
            },
            &full,
            4,
            0,
            0,
        );
        let patch = [
            Rgb565Pixel(100),
            Rgb565Pixel(101),
            Rgb565Pixel(102),
            Rgb565Pixel(103),
        ];
        state.remember_rect(
            geometry,
            FrameRect {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            },
            &patch,
            2,
            0,
            0,
        );

        assert_eq!(
            state.latest_pixels,
            vec![
                Rgb565Pixel(0),
                Rgb565Pixel(1),
                Rgb565Pixel(2),
                Rgb565Pixel(3),
                Rgb565Pixel(4),
                Rgb565Pixel(100),
                Rgb565Pixel(101),
                Rgb565Pixel(7),
                Rgb565Pixel(8),
                Rgb565Pixel(102),
                Rgb565Pixel(103),
                Rgb565Pixel(11),
            ]
        );
    }
}
