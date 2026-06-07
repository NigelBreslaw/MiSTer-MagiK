//! Background arcade preview image loader.

use crate::arcade_catalog::{self, DecodedImage};
use std::sync::mpsc;

#[derive(Clone, Debug)]
pub struct PreviewRequest {
    pub generation: u64,
    pub title: String,
    pub image_path: String,
}

#[derive(Clone, Debug)]
pub struct PreviewResult {
    pub generation: u64,
    pub title: String,
    pub image_path: String,
    pub image: Option<DecodedImage>,
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
        Ok(loaded) => PreviewResult {
            generation: req.generation,
            title: req.title,
            image_path: req.image_path,
            image: Some(loaded.image),
        },
        Err(_) => PreviewResult {
            generation: req.generation,
            title: req.title,
            image_path: req.image_path,
            image: None,
        },
    }
}

fn lower_thread_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}
