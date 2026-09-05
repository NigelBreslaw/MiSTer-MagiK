//! A single latest-frame slot isolates rendering from a stalled local receiver.
use mister_magik_framebuffer_stream::{
    FrameGeometry, FrameHeader, FrameKind, FrameRect, write_frame as write_preview_frame,
};
use slint::platform::software_renderer::Rgb565Pixel;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Packet = (FrameHeader, Vec<u8>);
pub struct PreviewProducer {
    state_root: PathBuf,
    last_preview: Instant,
    sequence: u64,
    pending: Arc<Mutex<Option<Packet>>>,
}
impl PreviewProducer {
    pub fn new() -> Self {
        let state_root = std::env::var("MISTER_MAGIK2_STATE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| "/tmp/mister-magik2".into());
        Self::with_root(state_root)
    }
    fn with_root(state_root: PathBuf) -> Self {
        let pending = Arc::new(Mutex::new(None::<Packet>));
        let worker_slot = Arc::downgrade(&pending);
        let socket_path = state_root.join("probe-frames.sock");
        std::thread::spawn(move || {
            loop {
                let Some(slot) = worker_slot.upgrade() else {
                    break;
                };
                let packet = slot.lock().expect("preview slot").take();
                drop(slot);
                if let Some((header, bytes)) = packet
                    && let Ok(mut socket) = UnixStream::connect(&socket_path)
                {
                    let _ = socket.set_write_timeout(Some(Duration::from_millis(100)));
                    let _ = write_preview_frame(&mut socket, header, &bytes);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        Self {
            state_root,
            last_preview: Instant::now() - Duration::from_secs(1),
            sequence: 0,
            pending,
        }
    }
    pub fn publish_if_watched(
        &mut self,
        pixels: &[Rgb565Pixel],
        width: usize,
        height: usize,
        elapsed: Duration,
    ) {
        if self.last_preview.elapsed() < Duration::from_millis(200) || !self.viewer_is_active() {
            return;
        }
        self.last_preview = Instant::now();
        let (Ok(width), Ok(height), Ok(raw_bytes)) = (
            u32::try_from(width),
            u32::try_from(height),
            u32::try_from(pixels.len().saturating_mul(2)),
        ) else {
            return;
        };
        let Ok(mut slot) = self.pending.try_lock() else {
            return;
        };
        let mut bytes = Vec::with_capacity(raw_bytes as usize);
        for pixel in pixels {
            bytes.extend_from_slice(&pixel.0.to_le_bytes());
        }
        self.sequence += 1;
        let geometry = FrameGeometry {
            width,
            height,
            stride_pixels: width,
        };
        let header = FrameHeader {
            kind: FrameKind::Keyframe,
            flags: 0,
            sequence: self.sequence,
            timestamp_us: elapsed.as_micros() as u64,
            geometry,
            rect: FrameRect::full(geometry),
            raw_bytes,
            payload_bytes: raw_bytes,
        };
        *slot = Some((header, bytes)); // Replace, never queue behind an old preview.
    }
    fn viewer_is_active(&self) -> bool {
        let Ok(deadline) = std::fs::read_to_string(self.state_root.join("viewer-lease")) else {
            return false;
        };
        let Ok(deadline) = deadline.trim().parse::<u128>() else {
            return false;
        };
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .is_ok_and(|now| now.as_millis() < deadline)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stalled_unix_receiver_does_not_block_render_publication() {
        let root =
            std::env::temp_dir().join(format!("magik2-preview-socket-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("viewer-lease"), u128::MAX.to_string()).unwrap();
        let listener =
            std::os::unix::net::UnixListener::bind(root.join("probe-frames.sock")).unwrap();
        let (accepted, ready) = std::sync::mpsc::channel();
        let receiver = std::thread::spawn(move || {
            let (_socket, _) = listener.accept().unwrap();
            accepted.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(500)); // Deliberately never read.
        });
        let mut producer = PreviewProducer::with_root(root.clone());
        let pixels = vec![Rgb565Pixel(0); 960 * 540];
        producer.publish_if_watched(&pixels, 960, 540, Duration::ZERO);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        for _ in 0..10 {
            producer.last_preview = Instant::now() - Duration::from_secs(1);
            producer.publish_if_watched(&pixels, 960, 540, Duration::ZERO);
        }
        assert!(started.elapsed() < Duration::from_millis(200));
        assert!(producer.sequence > 1);
        drop(producer);
        receiver.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn held_sender_slot_does_not_block_the_producer() {
        let root = std::env::temp_dir().join(format!("magik2-preview-slot-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("viewer-lease"), u128::MAX.to_string()).unwrap();
        let pending = Arc::new(Mutex::new(None));
        let held = pending.lock().unwrap();
        let mut producer = PreviewProducer {
            state_root: root.clone(),
            last_preview: Instant::now() - Duration::from_secs(1),
            sequence: 0,
            pending: pending.clone(),
        };
        let start = Instant::now();
        producer.publish_if_watched(&[Rgb565Pixel(0); 4], 2, 2, Duration::ZERO);
        assert!(start.elapsed() < Duration::from_millis(50));
        drop(held);
        std::fs::remove_dir_all(root).unwrap();
    }
}
