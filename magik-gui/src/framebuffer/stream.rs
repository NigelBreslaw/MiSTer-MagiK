use mister_magik_framebuffer_stream::{
    write_frame, FrameGeometry, FrameHeader, FrameKind, FrameRect, FLAG_LZ4_SIZE_PREPENDED,
};
use slint::platform::software_renderer::Rgb565Pixel;
use std::io;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

pub const PRODUCER_STREAM_PORT: u16 = 7499;
const KEYFRAME_INTERVAL_FRAMES: u64 = 60;

static STARTED_AT: OnceLock<Instant> = OnceLock::new();
static LISTENER_STARTED: OnceLock<()> = OnceLock::new();
static WORKER_QUEUE: OnceLock<Arc<WorkerQueue>> = OnceLock::new();
static SUBSCRIBER_ACTIVE: AtomicBool = AtomicBool::new(false);
static NEEDS_KEYFRAME: AtomicBool = AtomicBool::new(true);
static PENDING_FRAME: AtomicBool = AtomicBool::new(false);
static COALESCED_FRAMES: AtomicU64 = AtomicU64::new(0);

struct ProducerState {
    latest_geometry: Option<FrameGeometry>,
    latest_pixels: Vec<Rgb565Pixel>,
    has_full_frame_base: bool,
}

impl ProducerState {
    fn new() -> Self {
        Self {
            latest_geometry: None,
            latest_pixels: Vec::new(),
            has_full_frame_base: false,
        }
    }

    fn remember_rect_pixels(
        &mut self,
        geometry: FrameGeometry,
        rect: FrameRect,
        rect_pixels: &[Rgb565Pixel],
    ) {
        let len = geometry
            .stride_pixels
            .checked_mul(geometry.height)
            .map(|pixels| pixels as usize)
            .unwrap_or(0);
        if self.latest_geometry != Some(geometry) || self.latest_pixels.len() != len {
            self.latest_pixels.clear();
            self.latest_pixels.resize(len, Rgb565Pixel(0));
            self.latest_geometry = Some(geometry);
            self.has_full_frame_base = false;
        }
        copy_rect_into_latest(&mut self.latest_pixels, geometry, rect, rect_pixels);
        if is_full_frame(geometry, rect) {
            self.has_full_frame_base = true;
        }
    }
}

impl Default for ProducerState {
    fn default() -> Self {
        Self::new()
    }
}

struct FrameUpdate {
    geometry: FrameGeometry,
    rect: FrameRect,
    pixels: Vec<Rgb565Pixel>,
    captured_at_us: u64,
}

enum WorkerEvent {
    Subscriber(TcpStream),
    Frame(FrameUpdate),
    Timeout,
    Disconnected,
}

struct WorkerQueue {
    state: Mutex<WorkerQueueState>,
    ready: Condvar,
}

#[derive(Default)]
struct WorkerQueueState {
    subscriber: Option<TcpStream>,
    frame: Option<FrameUpdate>,
}

impl WorkerQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(WorkerQueueState::default()),
            ready: Condvar::new(),
        }
    }

    fn push_subscriber(&self, stream: TcpStream) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        state.subscriber = Some(stream);
        self.ready.notify_one();
        true
    }

    fn push_frame(&self, frame: FrameUpdate) -> bool {
        if PENDING_FRAME.swap(true, Ordering::AcqRel) {
            COALESCED_FRAMES.fetch_add(1, Ordering::Relaxed);
            NEEDS_KEYFRAME.store(true, Ordering::Release);
            return true;
        }
        let Ok(mut state) = self.state.lock() else {
            PENDING_FRAME.store(false, Ordering::Release);
            return false;
        };
        if state.frame.replace(frame).is_some() {
            COALESCED_FRAMES.fetch_add(1, Ordering::Relaxed);
            NEEDS_KEYFRAME.store(true, Ordering::Release);
        }
        self.ready.notify_one();
        true
    }

    fn recv_timeout(&self, timeout: Duration) -> WorkerEvent {
        let Ok(mut state) = self.state.lock() else {
            return WorkerEvent::Disconnected;
        };
        while state.subscriber.is_none() && state.frame.is_none() {
            let Ok((next_state, wait_result)) = self.ready.wait_timeout(state, timeout) else {
                return WorkerEvent::Disconnected;
            };
            state = next_state;
            if wait_result.timed_out() {
                return WorkerEvent::Timeout;
            }
        }
        if let Some(stream) = state.subscriber.take() {
            return WorkerEvent::Subscriber(stream);
        }
        if let Some(frame) = state.frame.take() {
            PENDING_FRAME.store(false, Ordering::Release);
            return WorkerEvent::Frame(frame);
        }
        WorkerEvent::Timeout
    }
}

pub fn start() {
    let _ = STARTED_AT.get_or_init(Instant::now);
    let _ = worker_queue();
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
        return;
    }
    let (rect, src_x, src_y) = if NEEDS_KEYFRAME.load(Ordering::Acquire) {
        (FrameRect::full(geometry), 0, 0)
    } else {
        (rect, rect.x as usize, rect.y as usize)
    };
    publish_rect(
        geometry,
        rect,
        pixels,
        geometry.stride_pixels as usize,
        src_x,
        src_y,
    );
}

pub fn publish_dense_rect(geometry: FrameGeometry, rect: FrameRect, pixels: &[Rgb565Pixel]) {
    if !SUBSCRIBER_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    publish_rect(geometry, rect, pixels, rect.width as usize, 0, 0);
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
    publish_rect(geometry, rect, pixels, src_stride, src_x, src_y);
}

fn publish_rect(
    geometry: FrameGeometry,
    rect: FrameRect,
    pixels: &[Rgb565Pixel],
    src_stride: usize,
    src_x: usize,
    src_y: usize,
) {
    if PENDING_FRAME.load(Ordering::Acquire) {
        COALESCED_FRAMES.fetch_add(1, Ordering::Relaxed);
        NEEDS_KEYFRAME.store(true, Ordering::Release);
        return;
    }
    let Some(pixels) = collect_strided_rect(pixels, src_stride, src_x, src_y, rect) else {
        return;
    };
    if !worker_queue().push_frame(FrameUpdate {
        geometry,
        rect,
        pixels,
        captured_at_us: timestamp_us(),
    }) {
        SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
        NEEDS_KEYFRAME.store(true, Ordering::Release);
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
    if !worker_queue().push_subscriber(stream) {
        SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
        NEEDS_KEYFRAME.store(true, Ordering::Release);
        return;
    }
    crate::ui_logln!("framebuffer stream producer subscriber connected");
}

fn is_full_frame(geometry: FrameGeometry, rect: FrameRect) -> bool {
    rect.x == 0 && rect.y == 0 && rect.width == geometry.width && rect.height == geometry.height
}

fn worker_queue() -> &'static Arc<WorkerQueue> {
    WORKER_QUEUE.get_or_init(|| {
        let queue = Arc::new(WorkerQueue::new());
        let worker_queue = Arc::clone(&queue);
        thread::Builder::new()
            .name("fb-stream-producer".to_string())
            .spawn(move || run_worker(worker_queue))
            .expect("spawn framebuffer stream worker");
        queue
    })
}

fn run_worker(queue: Arc<WorkerQueue>) {
    mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
        mister_magik_catalog::runtime_thread::RuntimeThreadRole::FramebufferStream,
    );
    let mut subscriber: Option<TcpStream> = None;
    let mut producer_state = ProducerState::new();
    let mut encoder = StreamEncoder::new();
    let mut sequence = 0_u64;
    loop {
        match queue.recv_timeout(Duration::from_millis(500)) {
            WorkerEvent::Subscriber(stream) => {
                subscriber = Some(stream);
                sequence = 0;
                NEEDS_KEYFRAME.store(true, Ordering::Release);
            }
            WorkerEvent::Frame(update) => {
                producer_state.remember_rect_pixels(update.geometry, update.rect, &update.pixels);
                let Some(stream) = subscriber.as_mut() else {
                    SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
                    NEEDS_KEYFRAME.store(true, Ordering::Release);
                    continue;
                };
                let force_keyframe = NEEDS_KEYFRAME.swap(false, Ordering::AcqRel)
                    || sequence == 0
                    || sequence.is_multiple_of(KEYFRAME_INTERVAL_FRAMES);
                let job = if force_keyframe && producer_state.has_full_frame_base {
                    producer_state.latest_geometry.map(|geometry| {
                        (
                            geometry,
                            FrameRect::full(geometry),
                            FrameKind::Keyframe,
                            producer_state.latest_pixels.as_slice(),
                            geometry.stride_pixels as usize,
                        )
                    })
                } else {
                    None
                };
                let (geometry, rect, kind, pixels, stride_pixels) = match job {
                    Some(job) => job,
                    None => (
                        update.geometry,
                        update.rect,
                        FrameKind::RectDelta,
                        update.pixels.as_slice(),
                        update.rect.width as usize,
                    ),
                };
                let raw = rgb565_rect_bytes(pixels, rect, stride_pixels, 0, 0);
                if raw.as_ref().is_empty() {
                    if force_keyframe {
                        NEEDS_KEYFRAME.store(true, Ordering::Release);
                    }
                    continue;
                }
                sequence = sequence.saturating_add(1);
                if let Err(err) = write_job(
                    stream,
                    sequence,
                    geometry,
                    rect,
                    kind,
                    update.captured_at_us,
                    raw.as_ref(),
                    &mut encoder,
                ) {
                    crate::ui_errln!("framebuffer stream producer write failed: {err}");
                    subscriber = None;
                    SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
                    NEEDS_KEYFRAME.store(true, Ordering::Release);
                }
            }
            WorkerEvent::Timeout => {
                if let Some(stream) = subscriber.as_mut() {
                    if let Err(err) = write_heartbeat(stream) {
                        crate::ui_errln!("framebuffer stream producer heartbeat failed: {err}");
                        subscriber = None;
                        SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
                        NEEDS_KEYFRAME.store(true, Ordering::Release);
                    }
                }
            }
            WorkerEvent::Disconnected => break,
        }
    }
    PENDING_FRAME.store(false, Ordering::Release);
    SUBSCRIBER_ACTIVE.store(false, Ordering::Release);
    NEEDS_KEYFRAME.store(true, Ordering::Release);
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

#[allow(clippy::too_many_arguments)]
fn write_job(
    stream: &mut TcpStream,
    sequence: u64,
    geometry: FrameGeometry,
    rect: FrameRect,
    kind: FrameKind,
    captured_at_us: u64,
    raw: &[u8],
    encoder: &mut StreamEncoder,
) -> io::Result<()> {
    let payload = encoder.compress_size_prepended(raw)?;
    let header = FrameHeader {
        kind,
        flags: FLAG_LZ4_SIZE_PREPENDED,
        sequence,
        timestamp_us: captured_at_us,
        geometry,
        rect,
        raw_bytes: raw.len() as u32,
        payload_bytes: payload.len() as u32,
    };
    write_frame(stream, header, payload)
}

struct StreamEncoder {
    table: lz4_flex::block::CompressTable,
    output: Vec<u8>,
}

impl StreamEncoder {
    fn new() -> Self {
        Self {
            table: lz4_flex::block::CompressTable::large(),
            output: Vec::new(),
        }
    }

    fn compress_size_prepended(&mut self, raw: &[u8]) -> io::Result<&[u8]> {
        let capacity = 4usize
            .checked_add(lz4_flex::block::get_maximum_output_size(raw.len()))
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "LZ4 payload too large"))?;
        if self.output.len() < capacity {
            self.output.resize(capacity, 0);
        }
        let raw_len = u32::try_from(raw.len())
            .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "LZ4 input too large"))?;
        self.output[..4].copy_from_slice(&raw_len.to_le_bytes());
        let compressed = lz4_flex::block::compress_into_with_table(
            raw,
            &mut self.output[4..capacity],
            &mut self.table,
        )
        .map_err(|err| io::Error::new(ErrorKind::InvalidData, err))?;
        Ok(&self.output[..4 + compressed])
    }
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
    if width == 0
        || height == 0
        || src_stride < width
        || src_x.checked_add(width).is_none_or(|end| end > src_stride)
    {
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

enum Rgb565Bytes<'a> {
    Borrowed(&'a [u8]),
    Packed(Vec<u8>),
}

impl AsRef<[u8]> for Rgb565Bytes<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Packed(bytes) => bytes,
        }
    }
}

fn rgb565_rect_bytes(
    pixels: &[Rgb565Pixel],
    rect: FrameRect,
    stride_pixels: usize,
    src_x: usize,
    src_y: usize,
) -> Rgb565Bytes<'_> {
    let width = rect.width as usize;
    let height = rect.height as usize;
    if src_x == 0
        && src_y == 0
        && width == stride_pixels
        && pixels.len() == width.saturating_mul(height)
    {
        #[cfg(target_endian = "little")]
        return Rgb565Bytes::Borrowed(bytemuck::cast_slice(pixels));
    }
    let mut out = Vec::with_capacity(width.saturating_mul(height).saturating_mul(2));
    for row in 0..height {
        let start = src_y
            .saturating_add(row)
            .saturating_mul(stride_pixels)
            .saturating_add(src_x);
        let end = start.saturating_add(width);
        let Some(row_pixels) = pixels.get(start..end) else {
            return Rgb565Bytes::Packed(Vec::new());
        };
        for pixel in row_pixels {
            out.extend_from_slice(&pixel.0.to_le_bytes());
        }
    }
    Rgb565Bytes::Packed(out)
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
    fn strided_rect_collection_rejects_rows_that_overrun_stride() {
        let pixels = (0..9).map(Rgb565Pixel).collect::<Vec<_>>();
        let rect = FrameRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        };

        assert_eq!(collect_strided_rect(&pixels, 3, 2, 0, rect), None);
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
            rgb565_rect_bytes(&pixels, rect, 2, 0, 0).as_ref(),
            &[0x34, 0x12, 0xcd, 0xab]
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
            rgb565_rect_bytes(&pixels, rect, 4, 1, 1).as_ref(),
            &[5, 0, 6, 0, 9, 0, 10, 0]
        );
    }

    #[test]
    fn stream_encoder_reuses_size_prepended_lz4_format() {
        let raw = (0..4096).map(|value| value as u8).collect::<Vec<_>>();
        let mut encoder = StreamEncoder::new();

        let payload = encoder
            .compress_size_prepended(&raw)
            .expect("compress RGB565 bytes")
            .to_vec();

        assert_eq!(
            lz4_flex::decompress_size_prepended(&payload).expect("decode payload"),
            raw
        );
        let capacity = encoder.output.capacity();
        let _ = encoder
            .compress_size_prepended(&[7; 128])
            .expect("reuse encoder");
        assert_eq!(encoder.output.capacity(), capacity);
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
        state.remember_rect_pixels(
            geometry,
            FrameRect {
                x: 0,
                y: 0,
                width: 4,
                height: 3,
            },
            &full,
        );
        let patch = [
            Rgb565Pixel(100),
            Rgb565Pixel(101),
            Rgb565Pixel(102),
            Rgb565Pixel(103),
        ];
        state.remember_rect_pixels(
            geometry,
            FrameRect {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            },
            &patch,
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

    #[test]
    fn worker_queue_drops_frame_when_pending_and_requests_keyframe() {
        PENDING_FRAME.store(false, Ordering::Release);
        NEEDS_KEYFRAME.store(false, Ordering::Release);
        let queue = WorkerQueue::new();
        let geometry = FrameGeometry {
            width: 2,
            height: 1,
            stride_pixels: 2,
        };
        let first = FrameUpdate {
            geometry,
            rect: FrameRect::full(geometry),
            pixels: vec![Rgb565Pixel(1), Rgb565Pixel(2)],
            captured_at_us: 1,
        };
        let second = FrameUpdate {
            geometry,
            rect: FrameRect::full(geometry),
            pixels: vec![Rgb565Pixel(3), Rgb565Pixel(4)],
            captured_at_us: 2,
        };

        assert!(queue.push_frame(first));
        assert!(queue.push_frame(second));

        assert!(NEEDS_KEYFRAME.load(Ordering::Acquire));
        let state = queue.state.lock().expect("queue state");
        let pending = state.frame.as_ref().expect("latest frame remains queued");
        assert_eq!(pending.pixels, vec![Rgb565Pixel(1), Rgb565Pixel(2)]);
        PENDING_FRAME.store(false, Ordering::Release);
    }
}
