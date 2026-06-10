//! Background arcade preview image loader.

use crate::arcade_catalog::{self, DecodedImage};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct PreviewRequest {
    pub generation: u64,
    pub title: String,
    pub image_path: String,
    pub requested_at: Instant,
}

#[derive(Clone, Debug)]
pub struct PreviewResult {
    pub generation: u64,
    pub title: String,
    pub image_path: String,
    pub image: Option<DecodedImage>,
    pub request_age_us: u64,
    pub read_us: u64,
    pub decode_us: u64,
    pub total_us: u64,
    pub encoded_bytes: usize,
    pub decoded_bytes: usize,
}

pub struct PreviewWorker {
    tx: mpsc::Sender<PreviewRequest>,
    rx: mpsc::Receiver<PreviewResult>,
    next_generation: u64,
}

impl PreviewWorker {
    pub fn new() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<PreviewRequest>();
        let (res_tx, res_rx) = mpsc::channel::<PreviewResult>();
        std::thread::Builder::new()
            .name("preview-loader".to_string())
            .spawn(move || preview_thread(req_rx, res_tx))
            .expect("spawn preview-loader");
        Self {
            tx: req_tx,
            rx: res_rx,
            next_generation: 1,
        }
    }

    pub fn request(&mut self, title: String, image_path: String) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        let _ = self.tx.send(PreviewRequest {
            generation,
            title,
            image_path,
            requested_at: Instant::now(),
        });
        generation
    }

    pub fn drain(&self) -> Vec<PreviewResult> {
        let mut out = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            out.push(result);
        }
        out
    }
}

fn preview_thread(rx: mpsc::Receiver<PreviewRequest>, tx: mpsc::Sender<PreviewResult>) {
    lower_thread_priority();
    while let Ok(mut req) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            req = newer;
        }
        let result = load_preview(req);
        if tx.send(result).is_err() {
            break;
        }
    }
}

fn load_preview(req: PreviewRequest) -> PreviewResult {
    match arcade_catalog::load_png_rgb8_timed(&req.image_path) {
        Ok(loaded) => {
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace decoded generation={} total_us={} read_us={} decode_us={} encoded_bytes={} decoded_bytes={} path={}",
                    req.generation,
                    loaded.timing.total_us,
                    loaded.timing.read_us,
                    loaded.timing.decode_us,
                    loaded.timing.encoded_bytes,
                    loaded.timing.decoded_bytes,
                    req.image_path
                );
            }
            PreviewResult {
                generation: req.generation,
                title: req.title,
                image_path: req.image_path,
                image: Some(loaded.image),
                request_age_us: req.requested_at.elapsed().as_micros() as u64,
                read_us: loaded.timing.read_us,
                decode_us: loaded.timing.decode_us,
                total_us: loaded.timing.total_us,
                encoded_bytes: loaded.timing.encoded_bytes,
                decoded_bytes: loaded.timing.decoded_bytes,
            }
        }
        Err(e) => {
            if preview_trace_enabled() {
                eprintln!(
                    "preview_trace decode_failed generation={} age_us={} path={} error={}",
                    req.generation,
                    req.requested_at.elapsed().as_micros(),
                    req.image_path,
                    e
                );
            }
            PreviewResult {
                generation: req.generation,
                title: req.title,
                image_path: req.image_path,
                image: None,
                request_age_us: req.requested_at.elapsed().as_micros() as u64,
                read_us: 0,
                decode_us: 0,
                total_us: 0,
                encoded_bytes: 0,
                decoded_bytes: 0,
            }
        }
    }
}

fn preview_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MISTER_PREVIEW_TRACE").as_deref(),
            Ok("1") | Ok("on") | Ok("true") | Ok("yes")
        )
    })
}

fn lower_thread_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}
