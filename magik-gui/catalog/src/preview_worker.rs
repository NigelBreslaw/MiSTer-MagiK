//! Background arcade preview image loader.

use crate::arcade_catalog::{self, DecodedImage};
use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Instant;

pub const DEFAULT_PREVIEW_RADIUS: usize = 5;
pub const DEFAULT_PREVIEW_CACHE_CAP: usize = DEFAULT_PREVIEW_RADIUS * 2 + 1;

#[derive(Clone, Debug)]
pub struct PreviewRequest {
    pub generation: u64,
    pub title: String,
    pub image_path: String,
    pub requested_at: Instant,
    pub priority: PreviewPriority,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewPriority {
    Selected,
    Prefetch { distance: usize },
}

impl PreviewPriority {
    fn rank(self) -> usize {
        match self {
            Self::Selected => 0,
            Self::Prefetch { distance } => 1 + distance,
        }
    }
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
    pub priority: PreviewPriority,
}

pub struct PreviewWorker {
    tx: mpsc::Sender<PreviewCommand>,
    rx: mpsc::Receiver<PreviewResult>,
    next_generation: u64,
}

#[derive(Clone, Debug)]
enum PreviewCommand {
    Request(PreviewRequest),
}

impl PreviewWorker {
    pub fn new() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<PreviewCommand>();
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

    pub fn request_selected(&mut self, title: String, image_path: String) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        let _ = self.tx.send(PreviewCommand::Request(PreviewRequest {
            generation,
            title,
            image_path,
            requested_at: Instant::now(),
            priority: PreviewPriority::Selected,
        }));
        generation
    }

    pub fn request_prefetch(&mut self, title: String, image_path: String, distance: usize) {
        let generation = self.next_generation;
        self.next_generation += 1;
        let _ = self.tx.send(PreviewCommand::Request(PreviewRequest {
            generation,
            title,
            image_path,
            requested_at: Instant::now(),
            priority: PreviewPriority::Prefetch { distance },
        }));
    }

    pub fn drain(&self) -> Vec<PreviewResult> {
        let mut out = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            out.push(result);
        }
        out
    }
}

pub fn preview_window_indices(len: usize, selected: usize, radius: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let selected = selected.min(len - 1);
    let start = selected.saturating_sub(radius);
    let end = selected.saturating_add(radius).min(len - 1);
    let mut indices: Vec<usize> = (start..=end).collect();
    indices.sort_by_key(|idx| (idx.abs_diff(selected), *idx));
    indices
}

pub fn preview_window_paths<'a, T, F>(
    items: &'a [T],
    selected: usize,
    radius: usize,
    mut path: F,
) -> Vec<&'a str>
where
    F: FnMut(&'a T) -> Option<&'a str>,
{
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for idx in preview_window_indices(items.len(), selected, radius) {
        if let Some(p) = path(&items[idx]) {
            if seen.insert(p) {
                out.push(p);
            }
        }
    }
    out
}

fn preview_thread(rx: mpsc::Receiver<PreviewCommand>, tx: mpsc::Sender<PreviewResult>) {
    lower_thread_priority();
    let mut queue: Vec<PreviewRequest> = Vec::new();
    loop {
        match rx.recv() {
            Ok(command) => enqueue_command(&mut queue, command),
            Err(_) => break,
        }
        while let Ok(command) = rx.try_recv() {
            enqueue_command(&mut queue, command);
        }
        queue.sort_by_key(|req| (req.priority.rank(), req.requested_at));
        if let Some(req) = queue.first().cloned() {
            queue.remove(0);
            let result = load_preview(req);
            if tx.send(result).is_err() {
                break;
            }
        }
    }
}

fn enqueue_command(queue: &mut Vec<PreviewRequest>, command: PreviewCommand) {
    match command {
        PreviewCommand::Request(req) => {
            if let Some(existing) = queue
                .iter_mut()
                .find(|existing| existing.image_path == req.image_path)
            {
                if req.priority.rank() <= existing.priority.rank() {
                    *existing = req;
                }
            } else {
                queue.push(req);
            }
            queue.retain(|req| {
                matches!(req.priority, PreviewPriority::Selected)
                    || req.priority.rank() <= DEFAULT_PREVIEW_CACHE_CAP
            });
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
                priority: req.priority,
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
                priority: req.priority,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_window_orders_selected_then_nearby() {
        assert_eq!(
            preview_window_indices(20, 10, 5),
            vec![10, 9, 11, 8, 12, 7, 13, 6, 14, 5, 15]
        );
    }

    #[test]
    fn preview_window_clamps_at_edges() {
        assert_eq!(preview_window_indices(4, 0, 5), vec![0, 1, 2, 3]);
        assert_eq!(preview_window_indices(4, 3, 5), vec![3, 2, 1, 0]);
    }

    #[test]
    fn preview_window_paths_dedupes_missing_and_duplicate_paths() {
        let items = vec![
            Some("a"),
            Some("b"),
            None,
            Some("b"),
            Some("c"),
            Some("d"),
        ];
        let paths = preview_window_paths(&items, 2, 3, |p| *p);
        assert_eq!(paths, vec!["b", "a", "c", "d"]);
    }
}
