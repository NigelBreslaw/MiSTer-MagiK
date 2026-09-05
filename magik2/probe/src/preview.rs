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
        let socket_path = state_root.join("probe-frames.sock");
        Self::with_sender(state_root, move |(header, bytes)| {
            if let Ok(mut socket) = UnixStream::connect(&socket_path) {
                let _ = socket.set_write_timeout(Some(Duration::from_millis(100)));
                let _ = write_preview_frame(&mut socket, header, &bytes);
            }
        })
    }
    fn with_sender(state_root: PathBuf, mut send: impl FnMut(Packet) + Send + 'static) -> Self {
        let pending = Arc::new(Mutex::new(None::<Packet>));
        let worker_slot = Arc::downgrade(&pending);
        std::thread::spawn(move || {
            loop {
                let Some(slot) = worker_slot.upgrade() else {
                    break;
                };
                let packet = slot.lock().expect("preview slot").take();
                drop(slot);
                if let Some(packet) = packet {
                    send(packet);
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
        use std::io::Write;
        use std::sync::mpsc;
        let (mut writer, receiver) = UnixStream::pair().unwrap();
        // Fill the socket before publication so transport cannot finish until
        // this test releases the receiver, regardless of runner CPU speed.
        writer.set_nonblocking(true).unwrap();
        loop {
            match writer.write(&[0; 64 * 1024]) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("cannot fill preview socket: {error}"),
            }
        }
        writer.set_nonblocking(false).unwrap();
        let (entered, sending) = mpsc::channel();
        let (sent, send_finished) = mpsc::channel();
        let mut producer = PreviewProducer::with_sender(root.clone(), move |(header, bytes)| {
            entered.send(()).unwrap();
            let _ = write_preview_frame(&mut writer, header, &bytes);
            let _ = sent.send(());
        });
        let (proceed, continue_publication) = mpsc::channel();
        let (complete, published) = mpsc::channel();
        let publisher = std::thread::spawn(move || {
            let pixels = vec![Rgb565Pixel(0); 960 * 540];
            producer.publish_if_watched(&pixels, 960, 540, Duration::ZERO);
            if continue_publication
                .recv_timeout(Duration::from_secs(10))
                .is_err()
            {
                return;
            }
            for _ in 0..10 {
                producer.last_preview = Instant::now() - Duration::from_secs(1);
                producer.publish_if_watched(&pixels, 960, 540, Duration::ZERO);
            }
            let latest = producer
                .pending
                .lock()
                .unwrap()
                .as_ref()
                .map(|(header, _)| header.sequence);
            let _ = complete.send((producer.sequence, latest));
        });
        let started = sending.recv_timeout(Duration::from_secs(10));
        let _ = proceed.send(());
        let result = published.recv_timeout(Duration::from_secs(10));
        let transport_blocked = matches!(send_finished.try_recv(), Err(mpsc::TryRecvError::Empty));
        // Always release I/O and join before asserting, including failure paths.
        drop(receiver);
        publisher.join().unwrap();
        assert!(started.is_ok(), "preview worker did not start");
        assert!(
            transport_blocked,
            "socket unexpectedly completed without a reader"
        );
        assert_eq!(result.unwrap(), (11, Some(11)));
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
        let (complete, published) = std::sync::mpsc::channel();
        let publisher = std::thread::spawn(move || {
            producer.publish_if_watched(&[Rgb565Pixel(0); 4], 2, 2, Duration::ZERO);
            let _ = complete.send(producer.sequence);
        });
        let result = published.recv_timeout(Duration::from_secs(10));
        drop(held);
        publisher.join().unwrap();
        assert_eq!(result.unwrap(), 0, "busy slot should skip the preview");
        std::fs::remove_dir_all(root).unwrap();
    }
}
