// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Launcher-owned CRT backdrop preparation, caching, and frame composition.

use crate::crt_backdrop::{
    BackdropSource, CrtBackdropState, CrtBackdropWorkTrace, PreparedCrtBackdrop,
    prepare_dimmed_rgb565_target_with_maps, product_chrome_rects,
};
use crate::ui_display::{CrtContentRect, CrtUiMetrics, UiDisplay};
use slint::platform::software_renderer::Rgb565Pixel;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

const PREPARE_QUEUE_CAP: usize = 2;
const PREPARED_CACHE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct PreparedIdentity {
    key: String,
    epoch: u64,
    width: usize,
    physical_height: usize,
    logical_height: usize,
}

struct PrepareRequest {
    identity: PreparedIdentity,
    words: Arc<[u16]>,
    source_width: usize,
    source_height: usize,
    stride_pixels: usize,
}

struct PrepareResult {
    identity: PreparedIdentity,
    prepare_us: u64,
    pixels: Arc<[Rgb565Pixel]>,
    row_repeats: Arc<[bool]>,
}

struct PrepareWorker {
    tx: SyncSender<PrepareRequest>,
    rx: Receiver<PrepareResult>,
}

impl PrepareWorker {
    fn new() -> Self {
        let (tx, requests) = sync_channel::<PrepareRequest>(PREPARE_QUEUE_CAP);
        let (results, rx) = sync_channel::<PrepareResult>(PREPARE_QUEUE_CAP);
        std::thread::Builder::new()
            .name("crt-backdrop-preparer".to_string())
            .spawn(move || {
                lower_prepare_worker_priority();
                let mut x_map = Vec::new();
                let mut y_map = Vec::new();
                while let Ok(request) = requests.recv() {
                    let started = Instant::now();
                    let Some((pixels, row_repeats)) = prepare_dimmed_rgb565_target_with_maps(
                        rgb565_words_as_pixels(&request.words),
                        request.source_width,
                        request.source_height,
                        request.stride_pixels,
                        request.identity.width,
                        request.identity.physical_height,
                        request.identity.logical_height,
                        &mut x_map,
                        &mut y_map,
                    ) else {
                        continue;
                    };
                    let result = PrepareResult {
                        identity: request.identity,
                        prepare_us: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
                        pixels: Arc::from(pixels),
                        row_repeats: Arc::from(row_repeats),
                    };
                    if results.send(result).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn crt-backdrop-preparer");
        Self { tx, rx }
    }
}

#[cfg(target_os = "linux")]
fn lower_prepare_worker_priority() {
    // The worker must not preempt the foreground 60 Hz list/latch loop.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        let _ = libc::setpriority(libc::PRIO_PROCESS, tid, 10);
    }
}

#[cfg(not(target_os = "linux"))]
fn lower_prepare_worker_priority() {}

#[derive(Clone)]
struct PreparedEntry {
    identity: PreparedIdentity,
    pixels: Arc<[Rgb565Pixel]>,
    row_repeats: Arc<[bool]>,
    bytes: usize,
    prepare_us: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CrtBackdropFrame {
    pub(super) trace: CrtBackdropWorkTrace,
    pub(super) full_damage: bool,
}

pub(super) struct CrtBackdropController {
    state: CrtBackdropState,
    worker: PrepareWorker,
    cache: VecDeque<PreparedEntry>,
    cache_bytes: usize,
    pending: HashSet<PreparedIdentity>,
    active_epoch: u64,
    revision: u64,
    pending_prepare_us: u64,
    selected: Option<usize>,
    transition_id: Option<u64>,
    prepared_revision: u64,
    was_eligible: bool,
}

impl CrtBackdropController {
    pub(super) fn for_display(display: &UiDisplay) -> Option<Self> {
        Some(Self {
            state: CrtBackdropState::for_display(display)?,
            worker: PrepareWorker::new(),
            cache: VecDeque::new(),
            cache_bytes: 0,
            pending: HashSet::new(),
            active_epoch: 0,
            revision: 0,
            pending_prepare_us: 0,
            selected: None,
            transition_id: None,
            prepared_revision: 0,
            was_eligible: false,
        })
    }

    pub(super) fn width(&self) -> usize {
        self.state.width()
    }

    pub(super) fn height(&self) -> usize {
        self.state.height()
    }

    pub(super) fn physical_height(&self) -> usize {
        self.state.physical_height()
    }

    pub(super) fn pixels(&self) -> &[Rgb565Pixel] {
        self.state.pixels()
    }

    pub(super) fn is_transitioning(&self) -> bool {
        self.state.is_transitioning()
    }

    pub(super) fn was_eligible(&self) -> bool {
        self.was_eligible
    }

    pub(super) fn selection_matches(&self, selected: usize) -> bool {
        self.selected == Some(selected)
    }

    pub(super) fn transition_id(&self) -> Option<u64> {
        self.transition_id
    }

    pub(super) fn poll(&mut self) {
        while let Ok(result) = self.worker.rx.try_recv() {
            self.pending.remove(&result.identity);
            if self.active_epoch != 0 && result.identity.epoch != self.active_epoch {
                continue;
            }
            self.pending_prepare_us = result.prepare_us;
            let bytes = result
                .pixels
                .len()
                .saturating_mul(std::mem::size_of::<Rgb565Pixel>())
                .saturating_add(result.row_repeats.len());
            if bytes > PREPARED_CACHE_BYTES {
                continue;
            }
            if let Some(index) = self
                .cache
                .iter()
                .position(|entry| entry.identity == result.identity)
            {
                self.cache_bytes = self.cache_bytes.saturating_sub(self.cache[index].bytes);
                self.cache.remove(index);
            }
            self.cache.push_back(PreparedEntry {
                identity: result.identity,
                pixels: result.pixels,
                row_repeats: result.row_repeats,
                bytes,
                prepare_us: result.prepare_us,
            });
            self.cache_bytes = self.cache_bytes.saturating_add(bytes);
            self.revision = self.revision.wrapping_add(1).max(1);
            while self.cache_bytes > PREPARED_CACHE_BYTES {
                let Some(evicted) = self.cache.pop_front() else {
                    break;
                };
                self.cache_bytes = self.cache_bytes.saturating_sub(evicted.bytes);
            }
        }
    }

    fn request_prepare(&mut self, source: &BackdropSource) {
        let identity = PreparedIdentity {
            key: source.key.clone(),
            epoch: source.epoch,
            width: self.width(),
            physical_height: self.physical_height(),
            logical_height: self.height(),
        };
        if self.cache.iter().any(|entry| entry.identity == identity)
            || !self.pending.insert(identity.clone())
        {
            return;
        }
        let request = PrepareRequest {
            identity: identity.clone(),
            words: Arc::clone(&source.words),
            source_width: source.source_width,
            source_height: source.source_height,
            stride_pixels: source.stride_pixels,
        };
        match self.worker.tx.try_send(request) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.pending.remove(&identity);
            }
        }
    }

    fn prepared_target(&mut self, source: &BackdropSource) -> Option<PreparedCrtBackdrop> {
        let identity = PreparedIdentity {
            key: source.key.clone(),
            epoch: source.epoch,
            width: self.width(),
            physical_height: self.physical_height(),
            logical_height: self.height(),
        };
        let index = self
            .cache
            .iter()
            .position(|entry| entry.identity == identity)?;
        let entry = self.cache.remove(index)?;
        self.pending_prepare_us = entry.prepare_us;
        let prepared = PreparedCrtBackdrop {
            pixels: Arc::clone(&entry.pixels),
            row_repeats: Arc::clone(&entry.row_repeats),
            is_plain: false,
        };
        self.cache.push_back(entry);
        Some(prepared)
    }

    pub(super) fn compose(
        &mut self,
        eligible: bool,
        selected: usize,
        transition_id: Option<u64>,
        source: Option<BackdropSource>,
        now: Duration,
        destination: &mut [Rgb565Pixel],
        content: CrtContentRect,
        metrics: CrtUiMetrics,
    ) -> CrtBackdropFrame {
        if let Some(source) = source.as_ref() {
            self.active_epoch = source.epoch;
        }
        self.poll();
        if !eligible {
            self.selected = None;
            self.transition_id = None;
            self.prepared_revision = self.revision;
            self.was_eligible = false;
            return CrtBackdropFrame::default();
        }

        let selected_changed = self.selected != Some(selected);
        let transition_changed = self.transition_id != transition_id;
        let prepared_changed = self.prepared_revision != self.revision;
        if selected_changed || transition_changed || prepared_changed || !self.was_eligible {
            if let Some(source) = source.as_ref() {
                self.request_prepare(source);
                self.state
                    .retarget_prepared(self.prepared_target(source), now);
            } else {
                self.state.clear_plain();
            }
            self.selected = Some(selected);
            self.transition_id = transition_id;
            self.prepared_revision = self.revision;
        }

        let compose_full = selected_changed
            || transition_changed
            || prepared_changed
            || !self.was_eligible
            || self.state.is_transitioning();
        let mut frame = CrtBackdropFrame::default();
        if compose_full {
            frame.trace = self.state.compose_into_coarse_excluding(
                now,
                destination,
                &product_chrome_rects(content, metrics),
            );
            if prepared_changed {
                frame.trace.prepare_us = self.pending_prepare_us;
                frame.trace.prepare_pixels = self
                    .width()
                    .saturating_mul(self.physical_height())
                    .min(u32::MAX as usize) as u32;
            }
            frame.full_damage = destination.len() >= self.width().saturating_mul(self.height());
        }
        self.was_eligible = true;
        frame
    }
}

const _: () = {
    assert!(std::mem::size_of::<Rgb565Pixel>() == std::mem::size_of::<u16>());
    assert!(std::mem::align_of::<Rgb565Pixel>() == std::mem::align_of::<u16>());
};

fn rgb565_words_as_pixels(words: &[u16]) -> &[Rgb565Pixel] {
    // SAFETY: Rgb565Pixel is repr(transparent) over u16; the layout is
    // asserted above and the slice remains borrowed for the worker call.
    unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<Rgb565Pixel>(), words.len()) }
}
