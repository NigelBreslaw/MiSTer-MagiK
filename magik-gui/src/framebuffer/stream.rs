use mister_magik_framebuffer_stream::{
    write_frame, FrameGeometry, FrameHeader, FrameKind, FrameRect, FLAG_LZ4_SIZE_PREPENDED,
};
use slint::platform::software_renderer::Rgb565Pixel;
use std::io;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

pub const PRODUCER_STREAM_PORT: u16 = 7499;

static STATE: OnceLock<Mutex<ProducerState>> = OnceLock::new();
static STARTED_AT: OnceLock<Instant> = OnceLock::new();
static LISTENER_STARTED: OnceLock<()> = OnceLock::new();
static WORKER_TX: OnceLock<SyncSender<WorkerCommand>> = OnceLock::new();
static SUBSCRIBER_ACTIVE: AtomicBool = AtomicBool::new(false);
static NEEDS_KEYFRAME: AtomicBool = AtomicBool::new(true);

struct ProducerState {
    latest_geometry: Option<FrameGeometry>,
    latest_pixels: Vec<Rgb565Pixel>,
}

impl ProducerState {
    fn new() -> Self {
        Self {
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

struct FrameJob {
    geometry: FrameGeometry,
    rect: FrameRect,
    kind: FrameKind,
    raw: Vec<u8>,
}

enum WorkerCommand {
    Subscriber(TcpStream),
    Frame(FrameJob),
}

pub fn start() {
    let _ = STARTED_AT.get_or_init(Instant::now);
    let _ = STATE.get_or_init(|| Mutex::new(ProducerState::new()));
    let _ = worker_tx();
    let _ = LISTENER_STARTED.get_or_init(|| {
        thread::spawn(move || {
            let listener = match bind_listener() {
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

fn bind_listener() -> io::Result<TcpListener> {
    let mut last_error = None;
    for _ in 0..20 {
        match TcpListener::bind(("127.0.0.1", PRODUCER_STREAM_PORT)) {
            Ok(listener) => return Ok(listener),
            Err(err) if err.kind() == ErrorKind::AddrInUse => {
                last_error = Some(err);
                thread::sleep(Duration::from_millis(100));
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_error.unwrap_or_else(|| ErrorKind::AddrInUse.into()))
}

pub fn publish_cached_rect(geometry: FrameGeometry, rect: FrameRect, pixels: &[Rgb565Pixel]) {
    if !SUBSCRIBER_ACTIVE.load(Ordering::Acquire) {
        remember_startup_keyframe(geometry, rect, pixels);
        return;
    }
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
    let job = if NEEDS_KEYFRAME.swap(false, Ordering::AcqRel) {
        latest_keyframe_job(&state)
    } else {
        Some(frame_job(
            geometry,
            rect,
            FrameKind::RectDelta,
            pixels,
            geometry.stride_pixels as usize,
            rect.x as usize,
            rect.y as usize,
        ))
    };
    drop(state);
    if let Some(job) = job {
        enqueue_frame(job);
    }
}

pub fn publish_dense_rect(geometry: FrameGeometry, rect: FrameRect, pixels: &[Rgb565Pixel]) {
    if !SUBSCRIBER_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let Ok(mut state) = state().lock() else {
        return;
    };
    state.remember_rect(geometry, rect, pixels, rect.width as usize, 0, 0);
    let job = if NEEDS_KEYFRAME.swap(false, Ordering::AcqRel) {
        latest_keyframe_job(&state)
    } else {
        Some(frame_job(
            geometry,
            rect,
            FrameKind::RectDelta,
            pixels,
            rect.width as usize,
            0,
            0,
        ))
    };
    drop(state);
    if let Some(job) = job {
        enqueue_frame(job);
    }
}

pub fn publish_strided_rect(
    geometry: FrameGeometry,
    rect: FrameRect,
    pixels: &[Rgb565Pixel],
    src_stride: usize,
    src_x: usize,
    src_y: usize,
) {
    if !SUBSCRIBER_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let Ok(mut state) = state().lock() else {
        return;
    };
    state.remember_rect(geometry, rect, pixels, src_stride, src_x, src_y);
    let job = if NEEDS_KEYFRAME.swap(false, Ordering::AcqRel) {
        latest_keyframe_job(&state)
    } else {
        Some(frame_job(
            geometry,
            rect,
            FrameKind::RectDelta,
            pixels,
            src_stride,
            src_x,
            src_y,
        ))
    };
    drop(state);
    if let Some(job) = job {
        enqueue_frame(job);
    }
}

fn install_subscriber(stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
    if SUBSCRIBER_ACTIVE.swap(true, Ordering::AcqRel) {
        crate::ui_errln!("framebuffer stream producer rejected extra subscriber");
        return;
    }
    NEEDS_KEYFRAME.store(true, Ordering::Release);
    if worker_tx().send(WorkerCommand::Subscriber(stream)).is_err() {
        SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
        return;
    }
    if let Ok(state) = state().lock() {
        if let Some(job) = latest_keyframe_job(&state) {
            NEEDS_KEYFRAME.store(false, Ordering::Release);
            enqueue_frame(job);
        }
    }
    crate::ui_logln!("framebuffer stream producer subscriber connected");
}

fn remember_startup_keyframe(geometry: FrameGeometry, rect: FrameRect, pixels: &[Rgb565Pixel]) {
    if !is_full_frame(geometry, rect) {
        return;
    }
    let Ok(mut state) = state().lock() else {
        return;
    };
    if state.latest_geometry.is_some() {
        return;
    }
    state.remember_rect(
        geometry,
        rect,
        pixels,
        geometry.stride_pixels as usize,
        rect.x as usize,
        rect.y as usize,
    );
}

fn is_full_frame(geometry: FrameGeometry, rect: FrameRect) -> bool {
    rect.x == 0 && rect.y == 0 && rect.width == geometry.width && rect.height == geometry.height
}

fn latest_keyframe_job(state: &ProducerState) -> Option<FrameJob> {
    let Some(geometry) = state.latest_geometry else {
        return None;
    };
    Some(frame_job(
        geometry,
        FrameRect::full(geometry),
        FrameKind::Keyframe,
        &state.latest_pixels,
        geometry.stride_pixels as usize,
        0,
        0,
    ))
}

fn frame_job(
    geometry: FrameGeometry,
    rect: FrameRect,
    kind: FrameKind,
    pixels: &[Rgb565Pixel],
    stride_pixels: usize,
    src_x: usize,
    src_y: usize,
) -> FrameJob {
    let raw = rgb565_rect_bytes(pixels, rect, stride_pixels, src_x, src_y);
    FrameJob {
        kind,
        geometry,
        rect,
        raw,
    }
}

fn enqueue_frame(job: FrameJob) {
    if job.raw.is_empty() {
        if matches!(job.kind, FrameKind::Keyframe) {
            NEEDS_KEYFRAME.store(true, Ordering::Release);
        }
        return;
    }
    match worker_tx().try_send(WorkerCommand::Frame(job)) {
        Ok(()) => {}
        Err(TrySendError::Full(WorkerCommand::Frame(job))) => {
            if SUBSCRIBER_ACTIVE.load(Ordering::Acquire) {
                NEEDS_KEYFRAME.store(true, Ordering::Release);
                if matches!(job.kind, FrameKind::Keyframe) {
                    crate::ui_errln!("framebuffer stream producer dropped keyframe");
                }
            }
        }
        Err(TrySendError::Disconnected(_)) => {
            SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
            NEEDS_KEYFRAME.store(true, Ordering::Release);
        }
        Err(TrySendError::Full(WorkerCommand::Subscriber(_))) => unreachable!(),
    }
}

fn worker_tx() -> &'static SyncSender<WorkerCommand> {
    WORKER_TX.get_or_init(|| {
        let (tx, rx) = sync_channel::<WorkerCommand>(1);
        thread::spawn(move || {
            let mut subscriber: Option<TcpStream> = None;
            let mut sequence = 0_u64;
            loop {
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(WorkerCommand::Subscriber(stream)) => {
                        subscriber = Some(stream);
                        sequence = 0;
                    }
                    Ok(WorkerCommand::Frame(job)) => {
                        let Some(stream) = subscriber.as_mut() else {
                            SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
                            NEEDS_KEYFRAME.store(true, Ordering::Release);
                            continue;
                        };
                        sequence = sequence.saturating_add(1);
                        if let Err(err) = write_job(stream, sequence, job) {
                            crate::ui_errln!("framebuffer stream producer write failed: {err}");
                            subscriber = None;
                            SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
                            NEEDS_KEYFRAME.store(true, Ordering::Release);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(stream) = subscriber.as_mut() {
                            if let Err(err) = write_heartbeat(stream) {
                                crate::ui_errln!(
                                    "framebuffer stream producer heartbeat failed: {err}"
                                );
                                subscriber = None;
                                SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
                                NEEDS_KEYFRAME.store(true, Ordering::Release);
                            }
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
            NEEDS_KEYFRAME.store(true, Ordering::Release);
        });
        tx
    })
}

fn write_heartbeat(stream: &mut TcpStream) -> io::Result<()> {
    let header = FrameHeader {
        kind: FrameKind::Heartbeat,
        flags: 0,
        sequence: 0,
        timestamp_us: timestamp_us(),
        geometry: FrameGeometry {
            width: 0,
            height: 0,
            stride_pixels: 0,
        },
        rect: FrameRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        raw_bytes: 0,
        payload_bytes: 0,
    };
    write_frame(stream, header, &[])
}

fn write_job(stream: &mut TcpStream, sequence: u64, job: FrameJob) -> io::Result<()> {
    let payload = lz4_flex::compress_prepend_size(&job.raw);
    let header = FrameHeader {
        kind: job.kind,
        flags: FLAG_LZ4_SIZE_PREPENDED,
        sequence,
        timestamp_us: timestamp_us(),
        geometry: job.geometry,
        rect: job.rect,
        raw_bytes: job.raw.len() as u32,
        payload_bytes: payload.len() as u32,
    };
    write_frame(stream, header, &payload)
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
