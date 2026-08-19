// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Launcher-owned CRT backdrop preparation, caching, and frame composition.

use crate::arcade_list_renderer::CrtArcadeLayout;
use crate::crt_backdrop::{
    BackdropSource, CrtBackdropState, CrtBackdropWorkTrace, PreparedCrtBackdrop,
    prepare_dimmed_rgb565_target_for_output_with_maps,
};
use crate::ui_display::{CrtUiMetrics, UiDisplay, UiLayoutGeometry};
use mister_magik_framebuffer_scenes::{OutputRotation, Rgb565OutputLayout};
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
    reference_height: usize,
    logical_width: usize,
    logical_height: usize,
    rotation: u8,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct BackdropLayoutIdentity {
    logical_width: usize,
    logical_height: usize,
    physical_width: usize,
    physical_height: usize,
    rotation: u8,
}

impl BackdropLayoutIdentity {
    fn for_layout(layout: UiLayoutGeometry) -> Self {
        let output = layout.output_layout();
        Self {
            logical_width: output.logical_width(),
            logical_height: output.logical_height(),
            physical_width: output.physical_width(),
            physical_height: output.physical_height(),
            rotation: output_rotation_id(output.rotation()),
        }
    }
}

struct PrepareRequest {
    identity: PreparedIdentity,
    words: Arc<[u16]>,
    source_width: usize,
    source_height: usize,
    stride_pixels: usize,
    output: Rgb565OutputLayout,
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
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::CrtBackdropPrepare,
                );
                let mut x_map = Vec::new();
                let mut y_map = Vec::new();
                while let Ok(request) = requests.recv() {
                    let started = Instant::now();
                    let Some((pixels, row_repeats)) =
                        prepare_dimmed_rgb565_target_for_output_with_maps(
                            rgb565_words_as_pixels(&request.words),
                            request.source_width,
                            request.source_height,
                            request.stride_pixels,
                            request.output,
                            request.identity.reference_height,
                            &mut x_map,
                            &mut y_map,
                        )
                    else {
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
    active_layout: Option<BackdropLayoutIdentity>,
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
            active_layout: None,
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

    fn reference_height(&self) -> usize {
        self.state.reference_height()
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

    pub(super) fn backdrop_revision(&self) -> u64 {
        self.prepared_revision
    }

    pub(super) fn poll(&mut self) -> bool {
        let mut accepted = false;
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
            accepted = true;
            while self.cache_bytes > PREPARED_CACHE_BYTES {
                let Some(evicted) = self.cache.pop_front() else {
                    break;
                };
                self.cache_bytes = self.cache_bytes.saturating_sub(evicted.bytes);
            }
        }
        accepted
    }

    fn prepared_identity(
        &self,
        source: &BackdropSource,
        layout: UiLayoutGeometry,
    ) -> PreparedIdentity {
        let output = layout.output_layout();
        PreparedIdentity {
            key: source.key.clone(),
            epoch: source.epoch,
            width: self.width(),
            physical_height: self.physical_height(),
            reference_height: self.reference_height(),
            logical_width: output.logical_width(),
            logical_height: output.logical_height(),
            rotation: output_rotation_id(output.rotation()),
        }
    }

    fn request_prepare(&mut self, source: &BackdropSource, layout: UiLayoutGeometry) {
        let identity = self.prepared_identity(source, layout);
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
            output: layout.output_layout(),
        };
        match self.worker.tx.try_send(request) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.pending.remove(&identity);
            }
        }
    }

    fn prepared_target(
        &mut self,
        source: &BackdropSource,
        layout: UiLayoutGeometry,
    ) -> Option<PreparedCrtBackdrop> {
        let identity = self.prepared_identity(source, layout);
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
        force_full_repaint: bool,
        instant_transition: bool,
        selected: usize,
        transition_id: Option<u64>,
        source: Option<BackdropSource>,
        now: Duration,
        destination: &mut [Rgb565Pixel],
        layout: UiLayoutGeometry,
        arcade_layout: CrtArcadeLayout,
        metrics: CrtUiMetrics,
    ) -> CrtBackdropFrame {
        if let Some(source) = source.as_ref() {
            self.active_epoch = source.epoch;
        }
        self.poll();
        let layout_identity = BackdropLayoutIdentity::for_layout(layout);
        let layout_changed = self.active_layout != Some(layout_identity);
        if layout_changed {
            self.active_layout = Some(layout_identity);
            self.state.clear_plain();
        }
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
        let mut backdrop_state_changed = false;
        if selected_changed
            || transition_changed
            || prepared_changed
            || layout_changed
            || !self.was_eligible
        {
            if let Some(source) = source.as_ref() {
                self.request_prepare(source, layout);
                if let Some(prepared) = self.prepared_target(source, layout) {
                    self.state
                        .retarget_prepared(Some(prepared), now, instant_transition);
                    backdrop_state_changed = true;
                }
            } else {
                self.state.clear_plain();
                backdrop_state_changed = true;
            }
            self.selected = Some(selected);
            self.transition_id = transition_id;
            self.prepared_revision = self.revision;
        }

        let compose_full = backdrop_state_changed
            || layout_changed
            || !self.was_eligible
            || force_full_repaint
            || self.state.is_transitioning();
        let mut frame = CrtBackdropFrame::default();
        if compose_full {
            frame.trace = self.state.compose_product_into_layout(
                now,
                destination,
                layout,
                arcade_layout,
                metrics,
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

const fn output_rotation_id(rotation: OutputRotation) -> u8 {
    match rotation {
        OutputRotation::None => 0,
        OutputRotation::Clockwise90 => 1,
        OutputRotation::CounterClockwise90 => 2,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crt_backdrop::{
        CRT_BACKDROP_FADE_DURATION, CRT_PRODUCT_FOOTER_TEXT, CRT_PRODUCT_HEADER_TEXT,
        product_chrome_rects,
    };
    use crate::ui_display::{ScreenOrientation, UiDisplayPlan, UiLayoutGeometry};
    use crate::visual_composition::{PreviewFrame, PreviewPixels};

    #[test]
    fn prepared_identity_separates_all_output_orientations() {
        let display = UiDisplay::for_plan(
            UiDisplayPlan::from_mister_ini_text(
                "[MiSTer]\ndirect_video=1\nmenu_pal=0\nforced_scandoubler=0\n",
            )
            .expect("CRT240 display plan"),
        );
        let controller = CrtBackdropController::for_display(&display).expect("CRT backdrop");
        let source = BackdropSource {
            key: "four-corners".to_string(),
            epoch: 7,
            words: Arc::from(vec![0_u16; 4]),
            source_width: 2,
            source_height: 2,
            stride_pixels: 2,
        };
        let identities = ScreenOrientation::ALL.map(|orientation| {
            controller.prepared_identity(
                &source,
                UiLayoutGeometry::for_display(&display, orientation),
            )
        });
        assert_ne!(identities[0], identities[1]);
        assert_ne!(identities[0], identities[2]);
        assert_ne!(identities[1], identities[2]);
    }

    fn prepare_request(identity: PreparedIdentity, output: Rgb565OutputLayout) -> PrepareRequest {
        PrepareRequest {
            identity,
            words: Arc::from(vec![0_u16; 4]),
            source_width: 2,
            source_height: 2,
            stride_pixels: 2,
            output,
        }
    }

    #[test]
    fn prepare_queue_remains_bounded() {
        let (tx, _rx) = sync_channel(PREPARE_QUEUE_CAP);
        let output = Rgb565OutputLayout::new(2, 2, 2, OutputRotation::None).unwrap();
        let identity = PreparedIdentity {
            key: "bounded".to_string(),
            epoch: 1,
            width: 2,
            physical_height: 2,
            reference_height: 2,
            logical_width: 2,
            logical_height: 2,
            rotation: 0,
        };
        for _ in 0..PREPARE_QUEUE_CAP {
            assert!(
                tx.try_send(prepare_request(identity.clone(), output))
                    .is_ok()
            );
        }
        assert!(matches!(
            tx.try_send(prepare_request(identity, output)),
            Err(TrySendError::Full(_))
        ));
    }

    #[test]
    fn stale_prepare_epoch_is_still_rejected() {
        let display = UiDisplay::for_plan(
            UiDisplayPlan::from_mister_ini_text(
                "[MiSTer]\ndirect_video=1\nmenu_pal=0\nforced_scandoubler=0\n",
            )
            .expect("CRT240 display plan"),
        );
        let mut controller = CrtBackdropController::for_display(&display).expect("CRT backdrop");
        let (request_tx, _request_rx) = sync_channel(PREPARE_QUEUE_CAP);
        let (result_tx, result_rx) = sync_channel(PREPARE_QUEUE_CAP);
        controller.worker = PrepareWorker {
            tx: request_tx,
            rx: result_rx,
        };
        controller.active_epoch = 2;
        let layout = UiLayoutGeometry::for_display(&display, ScreenOrientation::Normal);
        let source = BackdropSource {
            key: "stale".to_string(),
            epoch: 1,
            words: Arc::from(vec![0_u16; 4]),
            source_width: 2,
            source_height: 2,
            stride_pixels: 2,
        };
        let identity = controller.prepared_identity(&source, layout);
        controller.pending.insert(identity.clone());
        assert!(
            result_tx
                .send(PrepareResult {
                    identity: identity.clone(),
                    prepare_us: 1,
                    pixels: Arc::from(vec![
                        Rgb565Pixel(0);
                        controller.width() * controller.physical_height()
                    ]),
                    row_repeats: Arc::from(vec![false; controller.physical_height()]),
                })
                .is_ok()
        );

        assert!(!controller.poll());
        assert!(controller.cache.is_empty());
        assert!(!controller.pending.contains(&identity));
    }

    #[test]
    fn pending_replacement_keeps_the_previous_backdrop_without_repainting() {
        let display = UiDisplay::for_plan(
            UiDisplayPlan::from_mister_ini_text(
                "[MiSTer]\ndirect_video=1\nmenu_pal=0\nforced_scandoubler=0\n",
            )
            .expect("CRT240 display plan"),
        );
        let layout = UiLayoutGeometry::for_display(&display, ScreenOrientation::Normal);
        let metrics = CrtUiMetrics::for_display(&display);
        let arcade_layout = CrtArcadeLayout::for_layout(layout, metrics, false);
        let mut controller = CrtBackdropController::for_display(&display).expect("CRT backdrop");
        let (request_tx, request_rx) = sync_channel(PREPARE_QUEUE_CAP);
        let (_result_tx, result_rx) = sync_channel(PREPARE_QUEUE_CAP);
        controller.worker = PrepareWorker {
            tx: request_tx,
            rx: result_rx,
        };
        let previous_source = [Rgb565Pixel(0xffff); 4];
        controller.state.retarget(
            Some(PreviewFrame {
                pixels: PreviewPixels::Rgb565 {
                    pixels: &previous_source,
                    stride_pixels: 2,
                },
                source_width: 2,
                source_height: 2,
                display_width: 2,
                display_height: 2,
            }),
            Duration::ZERO,
        );
        let settled_at = CRT_BACKDROP_FADE_DURATION + Duration::from_millis(1);
        let _ = controller.state.compose(settled_at);
        controller.selected = Some(0);
        controller.was_eligible = true;
        controller.active_layout = Some(BackdropLayoutIdentity::for_layout(layout));
        controller.prepared_revision = controller.revision;
        let previous_pixels = controller.state.pixels().to_vec();
        let sentinel = Rgb565Pixel(0xf81f);
        let mut destination = vec![sentinel; controller.width() * controller.height()];
        let replacement = BackdropSource {
            key: "replacement".to_string(),
            epoch: 2,
            words: Arc::from(vec![0_u16; 4]),
            source_width: 2,
            source_height: 2,
            stride_pixels: 2,
        };

        let frame = controller.compose(
            true,
            false,
            true,
            1,
            Some(9),
            Some(replacement.clone()),
            settled_at,
            &mut destination,
            layout,
            arcade_layout,
            metrics,
        );

        assert!(!frame.full_damage);
        assert!(destination.iter().all(|pixel| *pixel == sentinel));
        assert_eq!(controller.state.pixels(), previous_pixels);
        assert_eq!(controller.selected, Some(1));
        assert_eq!(controller.transition_id, Some(9));
        assert!(
            controller
                .pending
                .contains(&controller.prepared_identity(&replacement, layout))
        );
        assert!(request_rx.try_recv().is_ok());
    }

    #[test]
    fn forced_compose_repaints_settled_backdrop_but_preserves_chrome_text() {
        let display = UiDisplay::for_plan(
            UiDisplayPlan::from_mister_ini_text(
                "[MiSTer]\ndirect_video=1\nmenu_pal=0\nforced_scandoubler=0\n",
            )
            .expect("CRT240 display plan"),
        );
        let layout = UiLayoutGeometry::for_display(&display, ScreenOrientation::Normal);
        let metrics = CrtUiMetrics::for_display(&display);
        let content = layout.content_rect();
        let arcade_layout = CrtArcadeLayout::for_layout(layout, metrics, false);
        let mut controller = CrtBackdropController::for_display(&display).expect("CRT backdrop");
        let source = [Rgb565Pixel(0xffff); 4];
        controller.state.retarget(
            Some(PreviewFrame {
                pixels: PreviewPixels::Rgb565 {
                    pixels: &source,
                    stride_pixels: 2,
                },
                source_width: 2,
                source_height: 2,
                display_width: 2,
                display_height: 2,
            }),
            Duration::ZERO,
        );
        let settled_at = CRT_BACKDROP_FADE_DURATION + Duration::from_millis(1);
        let _ = controller.state.compose(settled_at);
        controller.selected = Some(0);
        controller.was_eligible = true;

        let sentinel = Rgb565Pixel(0xf81f);
        let mut destination = vec![sentinel; controller.width() * controller.height()];
        let [header, footer] = product_chrome_rects(content, metrics);
        let header_text_index = header.1 * controller.width() + header.0;
        let header_background_index = header_text_index + 1;
        let footer_text_index = footer.1 * controller.width() + footer.0;
        destination[header_text_index] = CRT_PRODUCT_HEADER_TEXT;
        destination[footer_text_index] = CRT_PRODUCT_FOOTER_TEXT;
        let restored = controller.compose(
            true,
            true,
            false,
            0,
            None,
            None,
            settled_at,
            &mut destination,
            layout,
            arcade_layout,
            metrics,
        );

        let background_index = content.y * controller.width() + content.x;
        assert!(restored.full_damage);
        assert_ne!(destination[background_index], sentinel);
        assert_ne!(destination[header_background_index], sentinel);
        assert_eq!(destination[header_text_index], CRT_PRODUCT_HEADER_TEXT);
        assert_eq!(destination[footer_text_index], CRT_PRODUCT_FOOTER_TEXT);
    }
}
