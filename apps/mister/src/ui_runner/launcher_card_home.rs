// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production owner for the custom RGB565 card launcher: the root cards and
//! every nested hierarchy level, including the level-change card trick.

use super::{DirtyRect, DirtyRectList};
use crate::bitmap_font_resource::{
    jersey_25_console_bitmap_font, launcher_bitmap_font, nocive_15_console_bitmap_font,
    spleen_6x12_native_console_bitmap_font, xerxes_10_console_bitmap_font,
};
use crate::launcher_artwork::CardFaceCache;
#[cfg(test)]
use crate::launcher_home::CARD_COUNT;
use crate::launcher_home::CardLevelSnapshot;
use mister_magik_framebuffer_scenes::bitmap_text::BitmapFont;
use mister_magik_framebuffer_scenes::launcher::{
    CardSlot, LEVEL_TRICK_EDGE_MILLIS, LEVEL_TRICK_MILLIS, LauncherFrameRequest, LauncherScene,
    LauncherTypography, LevelChange, PreparedLauncher,
};
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowseFrame, BrowsePhase, CardLevelTransition, SPRING_POSITION_UNITS,
};
use mister_magik_framebuffer_scenes::launcher_parallel::{
    ParallelFrameTiming, ParallelLauncherRenderer,
};
use mister_magik_framebuffer_scenes::{
    Rgb565OutputLayout, Rgb565Pixel, Rgb565Rect, Rgb565SurfaceMut,
};
use std::sync::Arc;
#[path = "launcher_card_preparation.rs"]
mod preparation;
use preparation::{HomePreparation, PreparedContent};

/// Rotate a logical rectangle of `source` into the physical `destination` with
/// the shared tiled (NEON where available) rotation kernel. `false` if the
/// rectangle or buffers do not fit `output`; it never panics, because a layout
/// that is stale for this frame must make the frame fall back, not abort the app.
#[must_use]
fn rotate_rect(
    output: Rgb565OutputLayout,
    source: &[Rgb565Pixel],
    destination: &mut [Rgb565Pixel],
    rect: Rgb565Rect,
) -> bool {
    let Ok(mut surface) = Rgb565SurfaceMut::new(destination, output) else {
        return false;
    };
    surface.copy_rect_strided(
        rect.x0,
        rect.y0,
        rect.width(),
        rect.height(),
        source,
        output.logical_width(),
        rect.x0,
        rect.y0,
    )
}

/// The logical rectangles of chrome a level change fades, as the spans the
/// native path copies coalesced into bands of 32 rows.
fn level_chrome_rects(prepared: &PreparedLauncher, width: usize) -> Vec<Rgb565Rect> {
    let mut bands: Vec<(usize, Rgb565Rect)> = Vec::new();
    for (start, end) in prepared.level_chrome_copy_spans() {
        let y = start / width;
        let (x0, x1) = (start % width, (end - 1) % width + 1);
        match bands.iter_mut().find(|(band, _)| *band == y / 32) {
            Some((_, rect)) => {
                rect.x0 = rect.x0.min(x0);
                rect.x1 = rect.x1.max(x1);
                rect.y1 = rect.y1.max(y + 1);
            }
            None => bands.push((
                y / 32,
                Rgb565Rect {
                    x0,
                    y0: y,
                    x1,
                    y1: y + 1,
                },
            )),
        }
    }
    bands.into_iter().map(|(_, rect)| rect).collect()
}

/// A rotated output's physical copies of the card frame: the whole frame
/// (chrome and the primary band) and the helper band, each in scanout order.
/// The chrome is rotated only when it changes; each frame rotates the bands.
#[derive(Default)]
struct PhysicalBands {
    frame: Vec<Rgb565Pixel>,
    helper: Vec<Rgb565Pixel>,
    /// What the rotated chrome shows: its content and the rotation it used.
    chrome: Option<(u64, Rgb565OutputLayout)>,
}

/// The rotated frame and helper band with the physical rectangle each band owns.
pub(super) struct DirectBands<'a> {
    pub frame: &'a [Rgb565Pixel],
    pub helper: &'a [Rgb565Pixel],
    pub damage: [DirtyRect; 2],
    /// The rotated chrome a level change fades this frame; `None` outside one.
    pub chrome_damage: Option<DirtyRectList>,
}

struct VisiblePrepared(Option<Box<PreparedLauncher>>);
impl std::ops::Deref for VisiblePrepared {
    type Target = PreparedLauncher;
    fn deref(&self) -> &Self::Target {
        self.0.as_deref().expect("visible card content")
    }
}
impl std::ops::DerefMut for VisiblePrepared {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_deref_mut().expect("visible card content")
    }
}
struct PendingLevel {
    id: u64,
    scene: LauncherScene,
    level: CardLevelSnapshot,
}

pub(super) fn scene_for_display(
    ui: &crate::ui_display::UiDisplay,
    layout: crate::ui_display::UiLayoutGeometry,
) -> LauncherScene {
    if ui.output_route().is_crt() {
        let content = layout.content_rect();
        LauncherScene::crt(layout.logical_w(), layout.logical_h()).with_safe_content(
            mister_magik_framebuffer_scenes::Rgb565Rect {
                x0: content.x,
                y0: content.y,
                x1: content.x + content.width,
                y1: content.y + content.height,
            },
        )
    } else {
        LauncherScene::new(layout.logical_w(), layout.logical_h())
    }
}

struct LauncherFonts {
    heading: BitmapFont,
    number: BitmapFont,
    metadata: BitmapFont,
    fallback: BitmapFont,
}

impl LauncherFonts {
    fn load() -> Result<Self, String> {
        Ok(Self {
            heading: launcher_bitmap_font(nocive_15_console_bitmap_font()?),
            number: launcher_bitmap_font(jersey_25_console_bitmap_font()?),
            metadata: launcher_bitmap_font(xerxes_10_console_bitmap_font()?),
            fallback: launcher_bitmap_font(spleen_6x12_native_console_bitmap_font()?),
        })
    }

    fn typography(&self) -> LauncherTypography<'_> {
        LauncherTypography {
            heading: &self.heading,
            number: &self.number,
            metadata: &self.metadata,
            fallback: &self.fallback,
        }
    }
}

/// A level prepared aside: already built, or being built on a worker.
enum Prepared {
    Built(PreparedContent),
    Building(u64),
}

/// A prepared level and the content it was prepared for. Levels are set aside
/// two ways: the ones the selected card opens and the parent are prepared
/// ahead while idle, and a level we leave is kept so coming back is instant.
struct Aside {
    level: CardLevelSnapshot,
    prepared: Prepared,
}

/// The hierarchy is a few levels deep: the current level's neighbours plus the
/// levels already left behind.
const ASIDE_LEVELS: usize = 5;

/// A level change in progress. The level being left renders the gather while
/// destination and worker are ready before the gather starts. The edge-on
/// swap only adopts prepared content; animation never waits for preparation.
struct LevelTrick {
    change: LevelChange,
    source_level: CardLevelSnapshot,
    source_selected: usize,
    destination_selected: usize,
    source_slot: CardSlot,
    destination_slot: CardSlot,
    ready: bool,
    started_ms: u64,
    destination: Option<Prepared>,
    /// The prepared destination has replaced the source. Animation time is
    /// always elapsed time from readiness; there is no edge-delay clock.
    dealing: bool,
}

pub(super) struct LauncherCardHomeSession {
    scene: LauncherScene,
    level: CardLevelSnapshot,
    clock: String,
    fonts: Arc<LauncherFonts>,
    prepared: VisiblePrepared,
    preparation: HomePreparation,
    pending: Option<PendingLevel>,
    artwork_retry: Option<u64>,
    artwork_retry_at: u64,
    artwork_retry_delay: u64,
    trick: Option<LevelTrick>,
    aside: Vec<Aside>,
    now_ms: u64,
    renderer: Option<Box<ParallelLauncherRenderer>>,
    physical: PhysicalBands,
    last_rendered: Option<(BrowseFrame, u64)>,
    last_timing: Option<ParallelFrameTiming>,
    last_request: LauncherFrameRequest,
    last_visual_index: f32,
    /// Last logical selection associated with the visible prepared level.
    /// Used to settle a defensive fallback, never to invent a gather origin.
    visible_selection: usize,
    admission_waiting: bool,
    frame: BrowseFrame,
    active: bool,
    content_dirty: bool,
    chrome_refresh_pending: bool,
    content_generation: u64,
    compositor_stale: bool,
    compositor_content_generation: Option<u64>,
    measure_preparation: bool,
    preparation_measurement: Option<u64>,
}

impl LauncherCardHomeSession {
    pub(super) fn selected_card_rect(
        &self,
    ) -> mister_magik_framebuffer_scenes::navigation::NavigationTransitionRect {
        self.prepared.slot_zero().rect()
    }

    /// True while card-home, not the generic composed cache, owns the visible Home frame.
    pub(super) fn owns_visible_frame(&self) -> bool {
        self.active
    }

    pub(super) fn new(
        scene: LauncherScene,
        level: CardLevelSnapshot,
        selected: usize,
        clock: &str,
    ) -> Result<Self, String> {
        let cabinet_warm = crate::launcher_presentation::warm_arcade_cabinet();
        let cog_warm = crate::launcher_presentation::warm_settings_cog();
        let fonts = Arc::new(LauncherFonts::load()?);
        let selected = selected.min(level.cards.len().saturating_sub(1));
        let mut cache = CardFaceCache::default();
        let prepared = prepare_cached(scene, &level, selected, clock, &fonts, &mut cache);
        let preparation = HomePreparation::new(Arc::clone(&fonts), level.menu_id.clone(), cache)?;
        cabinet_warm
            .join()
            .map_err(|_| "cabinet preparation failed")?;
        cog_warm.join().map_err(|_| "cog preparation failed")?;
        let renderer = Some(home_renderer(&prepared));
        let frame = settled_frame(selected);
        Ok(Self {
            scene,
            level,
            clock: clock.to_owned(),
            fonts,
            prepared: VisiblePrepared(Some(Box::new(prepared))),
            preparation,
            pending: None,
            artwork_retry: None,
            artwork_retry_at: 1_000,
            artwork_retry_delay: 1_000,
            trick: None,
            aside: Vec::new(),
            now_ms: 0,
            renderer,
            physical: PhysicalBands::default(),
            last_rendered: None,
            last_timing: None,
            last_request: LauncherFrameRequest {
                frame,
                timestamp_us: 0,
                generation: 0,
            },
            last_visual_index: selected as f32,
            visible_selection: selected,
            admission_waiting: false,
            frame,
            active: false,
            content_dirty: true,
            chrome_refresh_pending: false,
            content_generation: 1,
            compositor_stale: false,
            compositor_content_generation: None,
            measure_preparation: std::env::var_os("MISTER_MAGIK2_STATE_ROOT").is_some(),
            preparation_measurement: None,
        })
    }

    pub(super) fn set_inactive(&mut self) {
        self.cancel_artwork_retry();
        // Preserve a pending destination while away; returning can adopt it
        // without waiting on or destroying a preparation worker here.
        self.invalidate_compositor();
        self.active = false;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn update_from_navigation(
        &mut self,
        scene: LauncherScene,
        level: &CardLevelSnapshot,
        selected: usize,
        visual_index: f32,
        clock: &str,
        now_ms: u64,
        motion: bool,
        nested_frame: Option<BrowseFrame>,
        transition: Option<&CardLevelTransition>,
    ) -> bool {
        let count = level.cards.len().max(1);
        let selected = selected.min(count - 1);
        self.now_ms = now_ms;
        self.admission_waiting = false;
        self.preparation
            .allow_background(!self.is_animating() && self.trick.is_none());
        if self.trick.is_some() {
            if self.clock != clock {
                self.chrome_refresh_pending = true;
            }
            self.clock.clear();
            self.clock.push_str(clock);
            if self.active && self.level.menu_id == level.menu_id && self.scene == scene && motion {
                self.advance_trick();
                return true;
            }
            self.finish_trick();
        }
        let level_changed = self.level.menu_id != level.menu_id;
        if level_changed
            && self.active
            && self.scene == scene
            && motion
            && let Some(source_selected) = transition.and_then(|origin| {
                origin.source_index(
                    &self.level.menu_id,
                    &level.menu_id,
                    self.level
                        .cards
                        .iter()
                        .map(|card| card.navigation_id.as_str()),
                )
            })
        {
            let accepted = self.begin_trick(level.clone(), selected, source_selected);
            self.admission_waiting = !accepted;
            return accepted;
        }
        // An absent/mismatched origin cannot invent an animated source. Finish
        // the last accepted source selection and hold it while adopting the target.
        if level_changed {
            self.settle_retained_source(self.visible_selection);
        }
        let faces_changed = self.scene != scene || self.level.cards != level.cards || level_changed;
        if faces_changed {
            self.admission_waiting = true;
            self.cancel_artwork_retry();
            let matches = self.pending.as_ref().is_some_and(|pending| {
                pending.scene == scene
                    && pending.level.menu_id == level.menu_id
                    && pending.level.cards == level.cards
            });
            if !matches {
                if let Some(pending) = self.pending.take() {
                    self.preparation.cancel(pending.id);
                }
                if let Some(id) = self
                    .preparation
                    .request(scene, level, selected, clock, true)
                {
                    self.pending = Some(PendingLevel {
                        id,
                        scene,
                        level: level.clone(),
                    });
                }
            }
            // Retain coherent pixels and their selection until the newest
            // requested faces and their producer are ready. Input continues.
            if !self.preparation.can_retire(ASIDE_LEVELS + 1) {
                return false;
            }
            let Some(content) = self
                .pending
                .as_ref()
                .and_then(|pending| self.preparation.take(pending.id))
            else {
                return false;
            };
            self.pending = None;
            if self.scene != scene {
                self.clear_aside();
            }
            let old = self.prepared.0.replace(content).unwrap();
            self.retire(Prepared::Built(old));
            self.scene = scene;
            self.level = level.clone();
            self.clock = clock.into();
            self.refresh_chrome(selected);
            self.frame = settled_frame(selected);
            self.last_visual_index = selected as f32;
            self.content_generation = self.content_generation.wrapping_add(1).max(1);

            self.content_dirty = true;
        } else if let Some(pending) = self.pending.take() {
            self.preparation.cancel(pending.id);
        }
        let mut previous_frame = self.frame;
        if !self.active || level_changed {
            self.last_visual_index = selected as f32;
            previous_frame = settled_frame(selected);
            self.active = true;
            self.content_dirty = true;
        }

        self.frame = nested_frame.unwrap_or_else(|| {
            browse_frame_from_position(
                selected,
                visual_index,
                self.last_visual_index,
                previous_frame,
                count,
                level.cycles(),
            )
        });
        self.last_visual_index = visual_index;
        self.visible_selection = selected;
        if navigation_identity_changed(previous_frame, self.frame) {
            self.content_dirty = true;
        }

        if self.level != *level || self.clock != clock {
            self.level = level.clone();
            self.clock.clear();
            self.clock.push_str(clock);
            self.chrome_refresh_pending = true;
        }
        if self.chrome_refresh_pending && !self.is_animating() {
            self.refresh_chrome(self.frame.selected);
            self.chrome_refresh_pending = false;
            self.content_generation = self.content_generation.wrapping_add(1).max(1);
            self.content_dirty = true;
        }
        self.preparation
            .allow_background(!self.is_animating() && self.trick.is_none());
        self.poll_artwork_retry();
        self.admission_waiting = false;
        true
    }

    /// Rendering-only fixtures construct an explicit source identity. Runtime
    /// callers must supply the navigation commit through update_from_navigation.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        scene: LauncherScene,
        level: &CardLevelSnapshot,
        selected: usize,
        visual_index: f32,
        clock: &str,
        now_ms: u64,
        motion: bool,
        nested_frame: Option<BrowseFrame>,
        source_id: Option<&str>,
    ) {
        let transition = source_id.map(|source_card| CardLevelTransition {
            source_level: self.level.menu_id.clone(),
            source_card: source_card.to_owned(),
            destination_level: level.menu_id.clone(),
        });
        self.update_from_navigation(
            scene,
            level,
            selected,
            visual_index,
            clock,
            now_ms,
            motion,
            nested_frame,
            transition.as_ref(),
        );
    }

    fn cancel_artwork_retry(&mut self) {
        if let Some(id) = self.artwork_retry.take() {
            self.preparation.cancel(id);
        }
    }

    /// Retry missing source files on the existing worker. Keep animating the
    /// current faces while it runs; only settled frames request/adopt repairs.
    fn poll_artwork_retry(&mut self) {
        if !self.active || self.trick.is_some() || self.pending.is_some() || self.is_animating() {
            return;
        }
        if let Some(id) = self.artwork_retry {
            if !self.preparation.can_retire(1) {
                return;
            }
            if let Some(content) = self.preparation.take(id) {
                self.artwork_retry = None;
                let retry = content.needs_artwork_retry();
                if self.prepared.shares_faces_with(&content) {
                    self.retire(Prepared::Built(content));
                } else {
                    let old = self.prepared.0.replace(content).unwrap();
                    self.retire(Prepared::Built(old));
                    self.refresh_chrome(self.frame.selected);
                    self.content_generation = self.content_generation.wrapping_add(1).max(1);
                    self.content_dirty = true;
                }
                self.artwork_retry_at = self.now_ms.saturating_add(self.artwork_retry_delay);
                self.artwork_retry_delay = if retry {
                    (self.artwork_retry_delay * 2).min(30_000)
                } else {
                    1_000
                };
            }
        }
        if self.artwork_retry.is_none()
            && self.prepared.needs_artwork_retry()
            && self.now_ms >= self.artwork_retry_at
        {
            self.artwork_retry = self.preparation.retry_artwork(
                self.scene,
                &self.level,
                self.frame.selected,
                &self.clock,
            );
            self.artwork_retry_at = self.now_ms.saturating_add(self.artwork_retry_delay);
        }
    }

    fn spawn_prepare(&self, level: &CardLevelSnapshot, selected: usize) -> Option<Prepared> {
        self.preparation
            .request(self.scene, level, selected, &self.clock, false)
            .map(Prepared::Building)
    }

    fn refresh_chrome(&mut self, selected: usize) {
        let typography = self.fonts.typography();
        let prepared = &mut self.prepared;
        self.level.with_data(selected, &self.clock, |data| {
            if !prepared.chrome_matches(data) {
                let started = self.measure_preparation.then(std::time::Instant::now);
                prepared.refresh_chrome(data, Some(typography));
                if let Some(started) = started {
                    self.preparation_measurement = Some(
                        self.preparation_measurement
                            .unwrap_or(0)
                            .saturating_add(started.elapsed().as_micros() as u64),
                    );
                }
            }
        });
    }

    fn refresh_target_chrome(&mut self, target: &mut PreparedLauncher, selected: usize) {
        self.level.with_data(selected, &self.clock, |data| {
            if !target.chrome_matches(data) {
                let started = self.measure_preparation.then(std::time::Instant::now);
                target.refresh_chrome(data, Some(self.fonts.typography()));
                if let Some(started) = started {
                    self.preparation_measurement = Some(
                        self.preparation_measurement
                            .unwrap_or(0)
                            .saturating_add(started.elapsed().as_micros() as u64),
                    );
                }
            }
        });
    }

    fn retire(&mut self, prepared: Prepared) {
        match prepared {
            Prepared::Built(content) => {
                self.preparation.retire(content);
            }
            Prepared::Building(id) => self.preparation.cancel(id),
        }
    }

    fn clear_aside(&mut self) {
        for aside in std::mem::take(&mut self.aside) {
            self.retire(aside.prepared);
        }
    }

    /// Whether a level with these cards is already set aside. Its header and
    /// summary are refreshed on install, so changed counts do not force a rebuild.
    fn is_aside(aside: &Aside, level: &CardLevelSnapshot) -> bool {
        aside.level.menu_id == level.menu_id && aside.level.cards == level.cards
    }

    fn set_aside(&mut self, level: CardLevelSnapshot, prepared: Prepared) {
        if let Some(index) = self
            .aside
            .iter()
            .position(|aside| aside.level.menu_id == level.menu_id)
        {
            let old = self.aside.remove(index);
            self.retire(old.prepared);
        }
        if self.aside.len() >= ASIDE_LEVELS {
            let old = self.aside.remove(0);
            self.retire(old.prepared);
        }
        self.aside.push(Aside { level, prepared });
    }

    fn take_aside(&mut self, level: &CardLevelSnapshot) -> Option<Prepared> {
        let index = self
            .aside
            .iter()
            .position(|aside| Self::is_aside(aside, level))?;
        Some(self.aside.remove(index).prepared)
    }

    /// Prepare neighbouring levels on workers while the launcher is idle.
    /// Cheap to call every frame: levels already set aside are skipped, and
    /// nothing is spawned during a level change.
    pub(super) fn prefetch(&mut self, levels: Vec<CardLevelSnapshot>) {
        if !self.active
            || self.pending.is_some()
            || self.trick.is_some()
            || self.is_animating()
            || !self.preparation.can_retire(2)
        {
            return;
        }
        for level in levels {
            if !self.preparation.can_retire(2) {
                break;
            }
            if level.menu_id == self.level.menu_id
                || self.aside.iter().any(|aside| Self::is_aside(aside, &level))
            {
                continue;
            }
            if let Some(building) = self.spawn_prepare(&level, 0) {
                self.set_aside(level, building);
            }
        }
    }

    /// Start the level-change trick toward `level`. The destination is the one
    /// set aside for it if there is one, else it is built on a worker.
    fn settle_retained_source(&mut self, selected: usize) {
        let frame = settled_frame(selected);
        self.visible_selection = selected;
        if self.frame != frame {
            self.frame = frame;
            self.last_visual_index = selected as f32;
            self.content_generation = self.content_generation.wrapping_add(1).max(1);
            self.content_dirty = true;
        }
    }

    fn begin_trick(
        &mut self,
        level: CardLevelSnapshot,
        selected: usize,
        source_selected: usize,
    ) -> bool {
        self.settle_retained_source(source_selected);
        self.cancel_artwork_retry();
        if !self.preparation.can_retire(2) {
            return false;
        }
        let change = if level.depth > self.level.depth {
            LevelChange::Descend
        } else {
            LevelChange::Ascend
        };
        if let Some(pending) = self.pending.take() {
            self.preparation.cancel(pending.id);
        }
        let Some(destination) = self.take_aside(&level).or_else(|| {
            self.preparation
                .request(self.scene, &level, selected, &self.clock, true)
                .map(Prepared::Building)
        }) else {
            return false;
        };
        // Reserve source parking before motion; no retirement may block an edge swap.
        if let Some(index) = self
            .aside
            .iter()
            .position(|aside| aside.level.menu_id == self.level.menu_id)
        {
            let old = self.aside.remove(index);
            self.retire(old.prepared);
        }
        if self.aside.len() >= ASIDE_LEVELS {
            let old = self.aside.remove(0);
            self.retire(old.prepared);
        }
        self.preparation.allow_background(false);
        if let Prepared::Building(id) = &destination {
            self.preparation.prioritize(*id);
        }
        self.trick = Some(LevelTrick {
            change,
            source_slot: self.prepared.slot_zero(),
            destination_slot: self.scene.slot_zero(!level.is_root()),
            ready: false,
            source_level: std::mem::replace(&mut self.level, level),
            source_selected,
            destination_selected: selected,
            started_ms: self.now_ms,
            destination: Some(destination),
            dealing: false,
        });
        self.content_generation = self.content_generation.wrapping_add(1).max(1);
        self.content_dirty = true;
        self.advance_trick();
        true
    }

    /// Taking a completed destination never joins or rebuilds on the UI.
    fn take_built_destination(&mut self) -> Option<PreparedContent> {
        let trick = self.trick.as_mut()?;
        match trick.destination.take()? {
            Prepared::Built(content) => Some(content),
            Prepared::Building(id) => match self.preparation.take(id) {
                Some(content) => Some(content),
                None => {
                    trick.destination = Some(Prepared::Building(id));
                    None
                }
            },
        }
    }

    fn install_destination(&mut self, content: PreparedContent) {
        #[cfg(feature = "tooling")]
        let _install =
            mister_magik_framebuffer_scenes::launcher_profile::span("prepare.install_destination");
        let source = self.trick.as_ref().map(|trick| trick.source_level.clone());
        let old = self.prepared.0.replace(content).unwrap();
        if let Some(source) = source {
            self.set_aside(source, Prepared::Built(old));
        } else {
            self.retire(Prepared::Built(old));
        }
        // Chrome was refreshed before motion. Only changes received during the
        // animation need an idle refresh after the destination is installed.
        let selected = self
            .trick
            .as_ref()
            .map_or(self.frame.selected, |trick| trick.destination_selected);
        self.visible_selection = selected;
        self.chrome_refresh_pending = self.level.with_data(selected, &self.clock, |data| {
            !self.prepared.chrome_matches(data)
        });
    }

    /// Finish an interruption when ready; otherwise restore the source and
    /// keep the preparation warm. No wait is needed to acknowledge leaving.
    fn finish_trick(&mut self) {
        let Some(trick) = self.trick.as_ref() else {
            return;
        };
        if !trick.dealing {
            if let Some(mut content) = self.take_built_destination() {
                let selected = self.trick.as_ref().unwrap().destination_selected;
                self.refresh_target_chrome(&mut content, selected);
                self.install_destination(content);
                self.frame = settled_frame(selected);
                self.last_visual_index = selected as f32;
            } else {
                let mut trick = self.trick.take().unwrap();
                let destination_level = std::mem::replace(&mut self.level, trick.source_level);
                if let Some(destination) = trick.destination.take() {
                    self.set_aside(destination_level, destination);
                }
                self.frame = settled_frame(trick.source_selected);
                self.last_visual_index = trick.source_selected as f32;
            }
        }
        self.trick = None;
        self.prepared.restore_chrome();
        self.content_generation = self.content_generation.wrapping_add(1).max(1);

        self.content_dirty = true;
    }

    /// Establish readiness and advance the edge swap before rendering.
    fn advance_trick(&mut self) {
        let Some(trick) = self.trick.as_ref() else {
            return;
        };
        if !trick.ready {
            let Some(mut prepared) = self.take_built_destination() else {
                return;
            };
            let selected = self.trick.as_ref().unwrap().destination_selected;
            self.refresh_target_chrome(&mut prepared, selected);
            let trick = self.trick.as_mut().unwrap();
            trick.destination = Some(Prepared::Built(prepared));
            trick.ready = true;
            trick.started_ms = self.now_ms;
            self.frame = settled_frame(trick.destination_selected);
            self.last_visual_index = trick.destination_selected as f32;
        }
        let trick = self.trick.as_ref().unwrap();
        let elapsed = self.now_ms.saturating_sub(trick.started_ms);
        let preparing = !trick.dealing;
        let edge = u64::from(LEVEL_TRICK_EDGE_MILLIS);
        if preparing && elapsed >= edge {
            // Readiness was established before t=0; no worker/queue operation
            // is allowed to freeze this pose or move the animation clock.
            let destination = self.trick.as_mut().unwrap().destination.take().unwrap();
            let Prepared::Built(prepared) = destination else {
                unreachable!("transition started without prepared content")
            };
            self.install_destination(prepared);
            self.trick.as_mut().unwrap().dealing = true;
        }
    }

    /// Render the prepared state; readiness and swaps precede frame evidence.
    fn render_trick(&mut self) -> bool {
        let Some(trick) = self.trick.as_ref().filter(|trick| trick.ready) else {
            return false;
        };
        let elapsed = self.now_ms.saturating_sub(trick.started_ms);
        let edge = u64::from(LEVEL_TRICK_EDGE_MILLIS);
        if !trick.dealing {
            let t = elapsed.min(edge) as u32;
            self.render_trick_frame(
                trick.source_selected,
                trick.change,
                t,
                trick.destination_slot,
                true,
            );
            if let Some(Prepared::Built(target)) = self
                .trick
                .as_ref()
                .and_then(|trick| trick.destination.as_ref())
            {
                self.prepared.render_transition_title_from(target, t);
            }
            return true;
        };
        let t = elapsed.min(u64::from(LEVEL_TRICK_MILLIS)) as u32;
        self.render_trick_frame(
            trick.destination_selected,
            trick.change,
            t,
            trick.source_slot,
            false,
        );
        if t >= LEVEL_TRICK_MILLIS {
            self.trick = None;
            self.content_generation = self.content_generation.wrapping_add(1).max(1);
        }
        true
    }

    fn render_trick_frame(
        &mut self,
        selected: usize,
        change: LevelChange,
        t: u32,
        slot: CardSlot,
        gather: bool,
    ) {
        let request = self.next_request(settled_frame(selected));
        self.last_timing = None;
        if self.prepared.supports_parallel()
            && let Some(renderer) = self.renderer.as_mut()
        {
            let timing = if gather {
                self.prepared
                    .render_level_gather_to_parallel(request, change, t, slot, renderer)
            } else {
                self.prepared
                    .render_level_deal_from_parallel(request, change, t, slot, renderer)
            };
            match timing {
                Ok(timing) => {
                    self.last_timing = Some(timing);
                    return;
                }
                Err(error) => self.stop_parallel_rendering(&error),
            }
        }
        if gather {
            self.prepared
                .render_level_gather_to(selected, change, t, slot);
        } else {
            self.prepared
                .render_level_deal_from(selected, change, t, slot);
        }
    }

    /// The two-thread renderer failed (its helper stopped, or a frame did not
    /// match): keep going on one thread rather than abort. Cards render the
    /// same pixels, only slower, and the direct path turns itself off.
    fn stop_parallel_rendering(&mut self, error: &str) {
        crate::ui_errln!("card renderer failed ({error}); rendering cards on one thread");
        self.renderer = None;
        self.last_rendered = None;
        self.content_dirty = true;
    }

    /// Queue a helper band for exactly the next FrameClock step. Do not cross
    /// the preparation/swap or landing boundaries, where state may change.
    pub(super) fn prepare_helper_ahead(&mut self, next_ms: u64) {
        if !self.can_render_direct() || next_ms <= self.now_ms {
            return;
        }
        let Some(trick) = self.trick.as_ref() else {
            return;
        };
        if !trick.ready {
            return;
        }
        let elapsed = next_ms
            .saturating_sub(trick.started_ms)
            .min(u64::from(LEVEL_TRICK_MILLIS));
        let gather = !trick.dealing && elapsed < u64::from(LEVEL_TRICK_EDGE_MILLIS);
        let (selected, slot) = if gather {
            (trick.source_selected, trick.destination_slot)
        } else {
            (trick.destination_selected, trick.source_slot)
        };
        let request = LauncherFrameRequest {
            frame: settled_frame(selected),
            timestamp_us: next_ms.saturating_mul(1_000),
            generation: self.last_request.generation.wrapping_add(1).max(1),
        };
        let content = if !gather && !trick.dealing {
            match trick.destination.as_ref() {
                Some(Prepared::Built(content)) => content.as_ref(),
                _ => unreachable!("ready transition lost its destination"),
            }
        } else {
            &*self.prepared
        };
        let preparer =
            content.level_frame_preparer(selected, trick.change, elapsed as u32, slot, gather);
        if let Some(renderer) = self.renderer.as_mut()
            && let Err(error) = renderer.prepare_helper_ahead(&preparer, request)
        {
            crate::ui_errln!("card helper render-ahead failed: {error}");
        }
    }

    /// Prepare only the exact next predicted pose. Input remains authoritative:
    /// the parallel renderer discards this job if the next request differs.
    pub(super) fn prepare_browse_helper_ahead(
        &mut self,
        next_ms: u64,
        selected: usize,
        visual_index: f32,
        nested_frame: Option<BrowseFrame>,
    ) {
        if !self.can_render_direct() || self.trick.is_some() || next_ms <= self.now_ms {
            return;
        }
        let frame = nested_frame.unwrap_or_else(|| {
            browse_frame_from_position(
                selected,
                visual_index,
                self.last_visual_index,
                self.frame,
                self.level.cards.len(),
                self.level.cycles(),
            )
        });
        // A quantized duplicate needs no new pixels or speculative worker work.
        if self.last_rendered == Some((frame, self.content_generation)) {
            return;
        }
        let request = LauncherFrameRequest {
            frame,
            timestamp_us: next_ms.saturating_mul(1_000),
            generation: self.last_request.generation.wrapping_add(1).max(1),
        };
        let preparer = self.prepared.frame_preparer();
        if let Some(renderer) = self.renderer.as_mut()
            && let Err(error) = renderer.prepare_helper_ahead(&preparer, request)
        {
            crate::ui_errln!("browse helper render-ahead failed: {error}");
        }
    }

    /// A level change is playing. The carousel shows neither level's real
    /// selection, so the launcher must not act on input until it lands.
    pub(super) fn is_level_trick_active(&self) -> bool {
        self.active && self.trick.is_some()
    }

    pub(super) fn is_animating(&self) -> bool {
        self.active
            && (self.trick.as_ref().is_some_and(|trick| trick.ready)
                || self.frame.phase != BrowsePhase::Settled)
    }

    pub(super) fn waiting_for_destination(&self) -> bool {
        self.active
            && (self.admission_waiting || self.trick.as_ref().is_some_and(|trick| !trick.ready))
    }

    pub(super) fn needs_render(&self) -> bool {
        self.active && (self.content_dirty || self.is_animating())
    }

    pub(super) fn render(&mut self) -> &[Rgb565Pixel] {
        self.render_output(false)
    }

    pub(super) fn render_direct_bands(&mut self) {
        let _ = self.render_output(true);
    }

    fn render_output(&mut self, retain_bands: bool) -> &[Rgb565Pixel] {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.retain_bands(retain_bands);
        }
        if self.render_trick() {
            self.last_rendered = None;
            self.content_dirty = true;
            self.compositor_stale = false;
            return self.prepared.pixels();
        }
        if self.last_rendered != Some((self.frame, self.content_generation)) {
            let request = self.next_request(self.frame);
            self.last_timing = None;
            if self.prepared.supports_parallel()
                && let Some(renderer) = self.renderer.as_mut()
            {
                match self.prepared.render_parallel_frame(renderer, request) {
                    Ok(timing) => self.last_timing = Some(timing),
                    Err(error) => {
                        self.stop_parallel_rendering(&error);
                        self.prepared.render_frame(self.frame);
                    }
                }
            } else {
                self.prepared.render_frame(self.frame);
            }
            self.last_rendered = Some((self.frame, self.content_generation));
        } else {
            if !retain_bands
                && self.prepared.supports_parallel()
                && let Some(renderer) = self.renderer.as_mut()
            {
                self.prepared.merge_retained_helper(renderer);
            }
            self.last_timing = None;
        }
        self.content_dirty = false;
        self.compositor_stale = false;
        self.prepared.pixels()
    }

    pub(super) fn carousel_clip(&self) -> (usize, usize) {
        self.prepared.carousel_clip()
    }

    pub(super) fn scene_ready(&self, scene: LauncherScene) -> bool {
        self.scene == scene
    }

    pub(super) fn content_ready(&self, scene: LauncherScene, level: &CardLevelSnapshot) -> bool {
        self.scene_ready(scene)
            && self.level.menu_id == level.menu_id
            && self.level.cards == level.cards
            && self.pending.is_none()
    }

    #[cfg(any(feature = "tooling", test))]
    pub(super) fn evidence_pose(&self) -> (&'static str, u64) {
        if let Some(trick) = self.trick.as_ref() {
            if !trick.ready {
                return ("level-preparing", 0);
            }
            let elapsed = self.now_ms.saturating_sub(trick.started_ms);
            if trick.dealing {
                ("level-deal", elapsed.min(u64::from(LEVEL_TRICK_MILLIS)))
            } else {
                (
                    "level-gather",
                    elapsed.min(u64::from(LEVEL_TRICK_EDGE_MILLIS)),
                )
            }
        } else {
            (
                if self.frame.phase == BrowsePhase::Settled {
                    "settled"
                } else {
                    "browse"
                },
                u64::from(self.frame.progress_millis),
            )
        }
    }

    #[cfg(feature = "tooling")]
    pub(super) fn take_preparation_profile(&self) -> serde_json::Value {
        self.preparation.take_preparation_profile()
    }

    pub(super) const fn content_generation(&self) -> u64 {
        self.content_generation
    }

    pub(super) const fn compositor_stale(&self) -> bool {
        self.compositor_stale
    }

    pub(super) fn invalidate_compositor(&mut self) {
        self.compositor_content_generation = None;
    }

    pub(super) fn chrome_copy_damage(&self, level_trick: bool) -> DirtyRectList {
        let mut damage = DirtyRectList::new();
        if !level_trick {
            return damage;
        }
        // Only the native canvas has level-trick chrome copy spans; a rotated
        // output renders its level change on the Slint path.
        if self.scene != LauncherScene::new(960, 540) {
            return damage;
        }
        // Coalesce sparse rows in bounded 32-row bands. Separate sidebar and
        // panel runs; native chrome geometry needs at most 19 rectangles.
        let mut pending: Option<((usize, bool), DirtyRect)> = None;
        for (start, end) in self.prepared.level_chrome_copy_spans() {
            let y = start / 960;
            let key = (y / 32, start % 960 >= 268);
            let row = DirtyRect {
                x0: start % 960,
                y0: y,
                x1: (end - 1) % 960 + 1,
                y1: y + 1,
            };
            if let Some((previous, rect)) = pending.as_mut() {
                if *previous == key {
                    *rect = rect.union(row);
                    continue;
                }
                damage.push(*rect);
            }
            pending = Some((key, row));
        }
        if let Some((_, rect)) = pending {
            damage.push(rect);
        }
        damage
    }

    pub(super) fn compositor_copy_damage(&self, motion_only: bool) -> Option<DirtyRect> {
        // The trick fades the header and summary: always copy the whole frame.
        (motion_only
            && self.trick.is_none()
            && self.scene == LauncherScene::new(960, 540)
            && self.compositor_content_generation == Some(self.content_generation))
        .then_some(DirtyRect {
            x0: self.prepared.carousel_clip().0,
            y0: 120,
            x1: 934,
            y1: 495,
        })
    }

    pub(super) fn note_compositor_copied(&mut self, motion_only: bool) {
        self.compositor_content_generation = motion_only.then_some(self.content_generation);
    }

    pub(super) fn note_direct_presented(&mut self) {
        self.invalidate_compositor();
        self.compositor_stale = true;
    }
    /// Whether the card frame can go straight into the hidden scanout slot,
    /// without a Slint raster, with its next frame rendered ahead: native HDMI
    /// landscape and HDMI portrait, level changes included. One predicate gates
    /// the direct path, the helper's render-ahead and the bands, so no output
    /// runs a lesser copy of the same system.
    pub(super) fn can_render_direct(&self) -> bool {
        self.active
            && (self.scene == LauncherScene::new(960, 540) || self.scene.is_hdmi_portrait())
            && self.renderer.is_some()
            && self
                .pending
                .as_ref()
                .is_none_or(|pending| pending.scene == self.scene)
    }

    /// Have the helper rotate its band for this output, so the presenting
    /// thread only rotates its own.
    pub(super) fn set_output_layout(&mut self, output: Option<Rgb565OutputLayout>) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_rotation(output);
        }
    }

    /// The bands of the frame just rendered with `render_direct_bands`,
    /// rotated into scanout order. `None` if the helper band is not the
    /// current frame (the caller then falls back to the Slint path).
    pub(super) fn direct_physical_bands(
        &mut self,
        output: Rgb565OutputLayout,
    ) -> Option<DirectBands<'_>> {
        // The output must be this scene's: a layout left over from another
        // orientation can have the same pixel count and swapped dimensions.
        if (output.logical_width(), output.logical_height()) != self.scene.size() {
            return None;
        }
        let geometry = self.prepared.frame_preparer().geometry();
        let split = self.renderer.as_ref()?.rendered_split();
        let helper = self.renderer.as_ref()?.helper_pixels(self.last_request)?;
        if output.logical_width() * output.logical_height() != self.prepared.pixels().len() {
            return None;
        }
        let bands = [
            Rgb565Rect {
                x0: geometry.clip.0,
                y0: geometry.rows.0,
                x1: split,
                y1: geometry.rows.1,
            },
            Rgb565Rect {
                x0: split,
                y0: geometry.rows.0,
                x1: geometry.clip.1,
                y1: geometry.rows.1,
            },
        ];
        let physical = &mut self.physical;
        physical.frame.resize(output.len(), Rgb565Pixel(0));
        physical.helper.resize(output.len(), Rgb565Pixel(0));
        let source = self.prepared.pixels();
        if physical.chrome != Some((self.content_generation, output)) {
            let whole = Rgb565Rect {
                x0: 0,
                y0: 0,
                x1: output.logical_width(),
                y1: output.logical_height(),
            };
            if !rotate_rect(output, source, &mut physical.frame, whole) {
                return None;
            }
            physical.chrome = Some((self.content_generation, output));
        }
        // A level change fades parts of the chrome without a new generation:
        // rotate and copy only those regions each frame.
        let mut chrome_damage = None;
        if self.trick.is_some() {
            let mut list = DirtyRectList::new();
            for rect in level_chrome_rects(&self.prepared, self.scene.width) {
                if !rotate_rect(output, source, &mut physical.frame, rect) {
                    return None;
                }
                let rect = output.logical_rect_to_physical(rect);
                list.push(DirtyRect {
                    x0: rect.x0,
                    y0: rect.y0,
                    x1: rect.x1,
                    y1: rect.y1,
                });
            }
            chrome_damage = Some(list);
        }
        if !rotate_rect(output, source, &mut physical.frame, bands[0]) {
            return None;
        }
        // The helper normally rotated its band when it drew it.
        let helper = match self
            .renderer
            .as_ref()
            .and_then(|renderer| renderer.helper_rotated_pixels(self.last_request, output))
        {
            Some(rotated) => rotated,
            None => {
                if !rotate_rect(output, helper, &mut physical.helper, bands[1]) {
                    return None;
                }
                &physical.helper
            }
        };
        let damage = bands.map(|band| {
            let rect = output.logical_rect_to_physical(band);
            DirtyRect {
                x0: rect.x0,
                y0: rect.y0,
                x1: rect.x1,
                y1: rect.y1,
            }
        });
        Some(DirectBands {
            frame: &physical.frame,
            helper,
            damage,
            chrome_damage,
        })
    }

    fn next_request(&mut self, frame: BrowseFrame) -> LauncherFrameRequest {
        self.last_request = LauncherFrameRequest {
            frame,
            timestamp_us: self.now_ms.saturating_mul(1_000),
            generation: self.last_request.generation.wrapping_add(1).max(1),
        };
        self.last_request
    }

    pub(super) fn current_request(&self) -> LauncherFrameRequest {
        self.last_request
    }
    #[cfg(feature = "tooling")]
    pub(super) fn rendered_request(&self) -> Option<LauncherFrameRequest> {
        self.renderer.as_ref()?.rendered_request()
    }

    pub(super) fn last_timing(&self) -> Option<ParallelFrameTiming> {
        self.last_timing
    }
    pub(super) fn current_primary_pixels(&self) -> &[Rgb565Pixel] {
        self.prepared.pixels()
    }

    /// The helper's band for the current request; `None` once the two-thread
    /// renderer has been stopped.
    pub(super) fn current_helper_pixels(&self) -> Option<&[Rgb565Pixel]> {
        self.renderer
            .as_ref()
            .and_then(|renderer| renderer.helper_pixels(self.last_request))
    }

    pub(super) fn rendered_split(&self) -> Option<usize> {
        self.renderer
            .as_ref()
            .map(|renderer| renderer.rendered_split())
    }

    #[cfg(feature = "tooling")]
    pub(super) fn take_preparation_measurement(&mut self) -> Option<u64> {
        self.preparation_measurement.take()
    }
}

fn home_renderer(prepared: &PreparedLauncher) -> Box<ParallelLauncherRenderer> {
    fn setup() {
        use mister_magik_catalog::runtime_thread::{
            RuntimeThreadRole, apply_runtime_thread_policy,
        };
        apply_runtime_thread_policy(RuntimeThreadRole::LauncherCardHelper);
    }
    let clocks = std::env::var_os("MISTER_MAGIK2_STATE_ROOT")
        .is_some()
        .then_some(
            mister_magik_framebuffer_scenes::launcher_parallel::ThreadClocks {
                cpu_us: crate::ui_runner::launcher_frame_accounting::cpu_thread_us,
                run_delay_us: crate::ui_runner::launcher_frame_accounting::thread_run_delay_us,
            },
        );
    Box::new(
        ParallelLauncherRenderer::new(prepared.frame_preparer(), Some(setup), clocks)
            .expect("start current card renderer"),
    )
}

fn navigation_identity_changed(previous: BrowseFrame, current: BrowseFrame) -> bool {
    previous.selected != current.selected
        || previous.target != current.target
        || previous.phase != current.phase
        || previous.direction != current.direction
}

fn settled_frame(selected: usize) -> BrowseFrame {
    BrowseFrame {
        selected,
        target: selected,
        phase: BrowsePhase::Settled,
        direction: None,
        progress_millis: 0,
        duration_millis: SPRING_POSITION_UNITS,
    }
}

fn browse_frame_from_position(
    selected: usize,
    visual_index: f32,
    previous: f32,
    previous_frame: BrowseFrame,
    count: usize,
    cyclic: bool,
) -> BrowseFrame {
    let position = if visual_index.is_finite() {
        visual_index
    } else {
        selected as f32
    };
    let count = count.max(1) as i64;
    let card_at = |index: i64| {
        if cyclic {
            index.rem_euclid(count) as usize
        } else {
            index.clamp(0, count - 1) as usize
        }
    };
    if position == position.round() {
        return settled_frame(card_at(position as i64));
    }

    let base = position.floor() as i64;
    let movement = position - previous;
    // Preserve the same flip through a reversal within a pair of card slots.
    // Changing its direction halfway through would swap the outgoing card and
    // restart both rotations even though the cards have not changed position.
    let right = if previous_frame.phase == BrowsePhase::Flipping && previous.floor() as i64 == base
    {
        previous_frame.direction == Some(BrowseDirection::Right)
    } else if movement.abs() > f32::EPSILON {
        movement > 0.0
    } else {
        selected == card_at(base + 1)
    };
    let (from, to, progress) = if right {
        (base, base + 1, position - base as f32)
    } else {
        (base + 1, base, (base + 1) as f32 - position)
    };
    BrowseFrame {
        selected: card_at(from),
        target: card_at(to),
        phase: BrowsePhase::Flipping,
        direction: Some(if right {
            BrowseDirection::Right
        } else {
            BrowseDirection::Left
        }),
        // Any nonzero movement starts the rotation on this frame. Keep the
        // final moving frame below one; the exact slot endpoint is settled.
        progress_millis: (progress * SPRING_POSITION_UNITS as f32)
            .round()
            .clamp(1.0, (SPRING_POSITION_UNITS - 1) as f32) as u32,
        duration_millis: SPRING_POSITION_UNITS,
    }
}

#[cfg(test)]
fn prepare(
    scene: LauncherScene,
    level: &CardLevelSnapshot,
    selected: usize,
    clock: &str,
    fonts: &LauncherFonts,
) -> PreparedLauncher {
    let keys: Vec<_> = level
        .cards
        .iter()
        .map(|card| card.artwork_key().to_owned())
        .collect();
    let artwork =
        crate::launcher_artwork::load_cards(&crate::launcher_artwork::asset_root(), &keys);
    let sources: Vec<_> = artwork.iter().map(|pixels| pixels.as_ref()).collect();
    level.with_data(selected, clock, |data| {
        scene
            .prepare_initial_with_rgb888_artwork_and_typography(data, &sources, fonts.typography())
            .finish()
    })
}

#[allow(clippy::too_many_arguments)]
fn prepare_cached(
    scene: LauncherScene,
    level: &CardLevelSnapshot,
    selected: usize,
    clock: &str,
    fonts: &LauncherFonts,
    cache: &mut CardFaceCache,
) -> PreparedLauncher {
    cache.prepare(scene, level, selected, clock, Some(fonts.typography()))
}

impl Drop for LauncherCardHomeSession {
    fn drop(&mut self) {
        let mut contents = Vec::with_capacity(ASIDE_LEVELS + 2);
        if let Some(prepared) = self.prepared.0.take() {
            contents.push(prepared);
        }
        for aside in std::mem::take(&mut self.aside) {
            if let Prepared::Built(content) = aside.prepared {
                contents.push(content);
            }
        }
        if let Some(trick) = self.trick.take()
            && let Some(Prepared::Built(content)) = trick.destination
        {
            contents.push(content);
        }
        self.preparation.shutdown(contents, self.renderer.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_destination_starts_during_unrelated_work_and_refreshes_only_stale_chrome() {
        use std::sync::mpsc;
        let scene = LauncherScene::new(960, 540);
        let root = snapshot();
        let target = consoles();
        let mut session = LauncherCardHomeSession::new(scene, root.clone(), 3, "12:00").unwrap();
        session.update(scene, &root, 3, 3.0, "12:00", 0, false, None, None);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        session.preparation = HomePreparation::start(
            Arc::clone(&session.fonts),
            root.menu_id.clone(),
            CardFaceCache::default(),
            move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            },
        )
        .unwrap();
        let mut unrelated = target.clone();
        unrelated.menu_id = "unrelated".into();
        session.spawn_prepare(&unrelated, 0).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        session.set_aside(
            target.clone(),
            Prepared::Built(Box::new(prepare(
                scene,
                &target,
                1,
                "11:59",
                &session.fonts,
            ))),
        );
        session.measure_preparation = true;
        session.begin_trick(target.clone(), 1, 3);
        assert!(
            session.trick.as_ref().unwrap().ready,
            "warm destination cannot wait for unrelated work"
        );
        assert!(!session.preparation.quiescent());
        let Prepared::Built(content) = session
            .trick
            .as_ref()
            .unwrap()
            .destination
            .as_ref()
            .unwrap()
        else {
            panic!("not ready")
        };
        assert!(target.with_data(1, "12:00", |data| content.chrome_matches(data)));
        assert!(
            session.preparation_measurement.take().unwrap() > 0,
            "actual stale repaint must be timed"
        );
        session.now_ms = 1000;
        // The interruption must complete despite retirement backpressure.
        for _ in 0..ASIDE_LEVELS + 2 {
            session
                .preparation
                .retire(Box::new(prepare(scene, &root, 0, "12:00", &session.fonts)));
        }
        assert!(!session.preparation.can_retire(2));
        session.finish_trick();
        assert_eq!(session.frame.selected, 1);
        assert_eq!(session.last_visual_index, 1.0);
        assert!(
            !session.chrome_refresh_pending,
            "current destination needs no settle repaint"
        );
        session.render();
        release_tx.send(()).unwrap();
    }

    #[test]
    fn waiting_source_is_retained_and_completed_interruption_uses_destination_selection() {
        use std::sync::mpsc;
        let scene = LauncherScene::new(960, 540);
        let root = snapshot();
        let target = consoles();
        let mut session = LauncherCardHomeSession::new(scene, root.clone(), 3, "12:00").unwrap();
        session.update(scene, &root, 3, 3.0, "12:00", 0, false, None, None);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        session.preparation = HomePreparation::start(
            Arc::clone(&session.fonts),
            root.menu_id.clone(),
            CardFaceCache::default(),
            move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            },
        )
        .unwrap();
        session.begin_trick(target.clone(), 1, 3);
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        session.render();
        let generation = session.current_request().generation;
        assert!(session.waiting_for_destination());
        assert!(!session.needs_render());
        for now in [16, 32, 1000] {
            session.update(scene, &target, 1, 1.0, "12:00", now, true, None, None);
            assert!(!session.needs_render());
            assert_eq!(session.current_request().generation, generation);
        }
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let content = loop {
            if let Some(content) = session.take_built_destination() {
                break content;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        // Destination completed, but advance_trick has not changed selection.
        assert!(!session.trick.as_ref().unwrap().ready);
        session.trick.as_mut().unwrap().destination = Some(Prepared::Built(content));
        session.finish_trick();
        assert_eq!(session.frame.selected, 1);
        assert_eq!(session.last_visual_index, 1.0);
    }
    use crate::launcher_home::{LauncherHomeCounts, LauncherHomeSnapshot, LevelCard, LevelSummary};
    use std::time::{Duration, Instant};

    fn snapshot() -> CardLevelSnapshot {
        CardLevelSnapshot::root(&LauncherHomeSnapshot::from_counts(LauncherHomeCounts {
            arcade: 1,
            consoles: 2,
            computers: 3,
            handhelds: 4,
            favourites: 5,
            collections: 4,
        }))
    }

    fn consoles() -> CardLevelSnapshot {
        CardLevelSnapshot {
            menu_id: "menu:consoles".into(),
            depth: 1,
            cards: ["ATARI", "SEGA", "NINTENDO"]
                .into_iter()
                .map(|name| LevelCard {
                    navigation_id: name.to_owned(),
                    artwork_override: Some(name.to_ascii_lowercase()),
                    id: mister_magik_framebuffer_scenes::launcher::LauncherCardId::Consoles,
                    name: name.into(),
                    games: Some(10),
                    colour: 0x2a7f,
                })
                .collect(),
            summary: LevelSummary::Nested {
                path: vec!["CONSOLES".into()],
                games: 30,
                children_label: "MAKERS",
                systems: Some(9),
                accent: 0x2a7f,
            },
        }
    }

    fn wait_content(
        session: &mut LauncherCardHomeSession,
        scene: LauncherScene,
        level: &CardLevelSnapshot,
        selected: usize,
        clock: &str,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            session.update(
                scene,
                level,
                selected,
                selected as f32,
                clock,
                32,
                false,
                None,
                None,
            );
            if session.content_ready(scene, level) && session.trick.is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "card content did not become ready"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn artwork_retry_waits_for_settled_motion_without_resetting_selection() {
        use mister_magik_framebuffer_scenes::launcher::{LauncherArtwork, LauncherFaceCache};
        let root = snapshot();
        let scene = LauncherScene::new(960, 540);
        let mut session = LauncherCardHomeSession::new(scene, root.clone(), 0, "12:00").unwrap();
        let failed = root.with_data(0, "12:00", |data| {
            scene
                .prepare_initial_with_rgb888_loader_and_cache(
                    data,
                    &mut |_| LauncherArtwork {
                        prepared: None,
                        pixels: std::borrow::Cow::Borrowed(&[]),
                        retry: true,
                        contains_name: false,
                        flat_colours: Vec::new(),
                    },
                    Some(session.fonts.typography()),
                    &mut LauncherFaceCache::default(),
                    1,
                )
                .finish()
        });
        session.prepared.0 = Some(Box::new(failed));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let ui_thread = std::thread::current().id();
        session.preparation = HomePreparation::start(
            Arc::clone(&session.fonts),
            root.menu_id.clone(),
            CardFaceCache::default(),
            move |_| {
                assert_ne!(std::thread::current().id(), ui_thread);
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            },
        )
        .unwrap();
        session.update(scene, &root, 0, 0.0, "12:00", 999, false, None, None);
        assert!(session.artwork_retry.is_none());
        session.update(scene, &root, 1, 0.25, "12:00", 1000, true, None, None);
        assert!(
            session.artwork_retry.is_none(),
            "motion must defer retry requests"
        );
        session.update(scene, &root, 0, 0.0, "12:00", 1001, false, None, None);
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        session.update(scene, &root, 1, 0.25, "12:00", 1016, true, None, None);
        assert!(session.prepared.needs_artwork_retry());
        assert_eq!(session.last_visual_index, 0.25);
        let moving = session.frame;
        assert_ne!(moving, settled_frame(0));
        assert_eq!(session.render().len(), scene.width * scene.height);
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        // Even a ready result must not be adopted during motion.
        session.update(scene, &root, 1, 0.25, "12:00", 1032, true, None, None);
        assert!(session.prepared.needs_artwork_retry());
        while session.prepared.needs_artwork_retry() {
            session.update(scene, &root, 1, 1.0, "12:00", 1048, false, None, None);
            assert!(Instant::now() < deadline, "artwork did not recover");
            std::thread::yield_now();
        }
        assert_eq!(session.frame, settled_frame(1));
        assert_eq!(session.last_visual_index, 1.0);
        assert!(session.artwork_retry.is_none());
        let mut expected = prepare(scene, &root, 1, "12:00", &session.fonts);
        expected.render_frame(settled_frame(1));
        assert_eq!(session.render(), expected.pixels());
    }

    #[test]
    fn unchanged_retry_does_not_invalidate_content_and_count_changes_keep_backoff() {
        let mut level = snapshot();
        let scene = LauncherScene::new(960, 540);
        let mut session = LauncherCardHomeSession::new(scene, level.clone(), 0, "12:00").unwrap();
        session.update(scene, &level, 0, 0.0, "12:00", 0, false, None, None);
        session.render();
        let generation = session.content_generation;
        session.artwork_retry = session.preparation.retry_artwork(scene, &level, 0, "12:00");
        assert!(session.artwork_retry.is_some());
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.artwork_retry.is_some() {
            session.update(scene, &level, 0, 0.0, "12:00", 1000, false, None, None);
            assert!(Instant::now() < deadline, "retry did not complete");
            std::thread::yield_now();
        }
        assert_eq!(session.content_generation, generation);
        assert!(!session.content_dirty);
        session.artwork_retry_delay = 8_000;
        session.artwork_retry_at = 9_000;
        level.cards[0].games = Some(12345);
        session.update(scene, &level, 0, 0.0, "12:00", 1001, false, None, None);
        assert_eq!(session.artwork_retry_delay, 8_000);
        assert_eq!(session.artwork_retry_at, 9_000);
    }

    #[test]
    fn panicking_preparation_recovers_pending_and_trick_content_off_ui() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for (motion, prefetch) in [(false, false), (true, false), (true, true)] {
            let scene = LauncherScene::new(960, 540);
            let root = snapshot();
            let mut destination = consoles();
            let mut session =
                LauncherCardHomeSession::new(scene, root.clone(), 0, "07:28").unwrap();
            session.update(scene, &root, 0, 0.0, "07:28", 0, motion, None, None);
            let attempts = Arc::new(AtomicUsize::new(0));
            let worker_attempts = Arc::clone(&attempts);
            let ui_thread = std::thread::current().id();
            session.preparation = HomePreparation::start(
                Arc::clone(&session.fonts),
                root.menu_id.clone(),
                CardFaceCache::default(),
                move |_| {
                    assert_ne!(std::thread::current().id(), ui_thread);
                    if worker_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                        panic!("injected card preparation failure");
                    }
                },
            )
            .unwrap();
            if prefetch {
                session.prefetch(vec![destination.clone()]);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut now = 16;
            loop {
                session.update(
                    scene,
                    &destination,
                    0,
                    0.0,
                    "07:28",
                    now,
                    motion,
                    None,
                    Some("arcade"),
                );
                session.render();
                if session.content_ready(scene, &destination) && session.trick.is_none() {
                    break;
                }
                assert!(Instant::now() < deadline, "panic stranded Home content");
                now += 16;
                std::thread::yield_now();
            }
            let expected = prepare(scene, &destination, 0, "07:28", &session.fonts);
            assert_eq!(session.render(), expected.pixels());
            destination.cards[0].games = Some(123);
            wait_content(&mut session, scene, &destination, 0, "07:28");
            let expected = prepare(scene, &destination, 0, "07:28", &session.fonts);
            assert_eq!(session.render(), expected.pixels());
            assert!(attempts.load(Ordering::SeqCst) >= 3);
            assert!(session.preparation.ownership_is_bounded());
        }
    }

    #[test]
    fn repeated_preparation_panic_is_reported_instead_of_hanging_or_retrying_forever() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let scene = LauncherScene::new(960, 540);
        let root = snapshot();
        let destination = consoles();
        let mut session = LauncherCardHomeSession::new(scene, root.clone(), 0, "07:28").unwrap();
        session.update(scene, &root, 0, 0.0, "07:28", 0, false, None, None);
        let attempts = Arc::new(AtomicUsize::new(0));
        let worker_attempts = Arc::clone(&attempts);
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        session.preparation = HomePreparation::start(
            Arc::clone(&session.fonts),
            root.menu_id.clone(),
            CardFaceCache::default(),
            move |_| {
                if worker_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                panic!("persistent preparation failure");
            },
        )
        .unwrap();
        session.update(scene, &destination, 0, 0.0, "07:28", 16, false, None, None);
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session.preparation.has_failed() {
            assert!(Instant::now() < deadline, "worker failure was not recorded");
            std::thread::yield_now();
        }
        let failure = catch_unwind(AssertUnwindSafe(|| {
            session.update(scene, &destination, 0, 0.0, "07:28", 32, false, None, None);
        }))
        .unwrap_err();
        assert_eq!(
            failure.downcast_ref::<&str>(),
            Some(&"persistent preparation failure")
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(
            session
                .preparation
                .request(scene, &destination, 0, "07:28", true)
                .is_none()
        );
    }

    #[test]
    fn changed_card_ui_preparation_probe() {
        let scene = LauncherScene::new(960, 540);
        let mut measurements = Vec::new();
        for mut level in [snapshot(), consoles()] {
            let mut session =
                LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
            session.update(scene, &level, 0, 0.0, "07:28", 0, false, None, None);
            session.render();
            level.cards[0].games = Some(999);
            crate::allocation_metrics::begin();
            session.update(scene, &level, 0, 0.0, "07:28", 16, false, None, None);
            measurements.push(crate::allocation_metrics::finish().bytes);
        }
        println!("changed_card_ui_preparation_allocated_bytes={measurements:?}");
        assert!(measurements.iter().all(|bytes| *bytes < 960 * 540 * 2));
    }

    #[test]
    fn blocked_preparation_accepts_input_and_installs_only_newest_content() {
        use std::sync::mpsc::channel;
        for level in [snapshot(), consoles()] {
            let (entered_tx, entered_rx) = channel();
            let (release_tx, release_rx) = channel();
            let (continue_tx, continue_rx) = channel();
            let (input_tx, input_rx) = channel();
            let ui = std::thread::spawn(move || {
                let scene = LauncherScene::new(960, 540);
                let mut session =
                    LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
                session.update(scene, &level, 0, 0.0, "07:28", 0, false, None, None);
                let old_pixels = session.render().to_vec();
                session.preparation = HomePreparation::start(
                    Arc::clone(&session.fonts),
                    level.menu_id.clone(),
                    CardFaceCache::default(),
                    move |id| {
                        if id == 1 {
                            entered_tx.send(()).unwrap();
                            let _ = release_rx.recv_timeout(Duration::from_secs(5));
                        }
                    },
                )
                .unwrap();
                let mut changed = level.clone();
                changed.cards[0].games = Some(100);
                session.update(scene, &changed, 0, 0.0, "07:28", 16, false, None, None);
                continue_rx.recv().unwrap();
                changed.cards[0].games = Some(999);
                session.update(scene, &changed, 1, 1.0, "07:29", 32, false, None, None);
                assert_eq!(
                    session.level, level,
                    "old faces must retain their own content identity"
                );
                assert_eq!(session.render(), old_pixels);
                let mut nav = crate::launcher::LauncherNav::new();
                let catalog = crate::arcade_catalog::ArcadeCatalog::new(
                    std::path::PathBuf::new(),
                    vec![],
                    vec![],
                );
                nav.handle_input(
                    &crate::input::PadState {
                        dpad_right: true,
                        ..Default::default()
                    },
                    Instant::now(),
                    &catalog,
                );
                input_tx.send(nav.selected).unwrap();
                (session, changed)
            });
            let entered = entered_rx.recv_timeout(Duration::from_secs(5));
            let _ = continue_tx.send(());
            let input = input_rx.recv_timeout(Duration::from_secs(3));
            let _ = release_tx.send(()); // Release before assertions on every outcome.
            let (mut session, changed) = ui.join().unwrap();
            assert!(entered.is_ok());
            assert_eq!(input.unwrap(), 1);
            wait_content(
                &mut session,
                LauncherScene::new(960, 540),
                &changed,
                1,
                "07:29",
            );
            assert_eq!(session.level.cards[0].games, Some(999));
            let mut reference = prepare(session.scene, &changed, 1, "07:29", &session.fonts);
            reference.render_frame(session.frame);
            assert_eq!(session.render(), reference.pixels());
        }
    }

    #[test]
    fn blocked_prefetch_and_retirement_remain_bounded_and_drop_without_waiting() {
        use std::sync::mpsc::channel;
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        let (continue_tx, continue_rx) = channel();
        let (dropped_tx, dropped_rx) = channel();
        let ui = std::thread::spawn(move || {
            let scene = LauncherScene::new(960, 540);
            let level = snapshot();
            let mut session =
                LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
            session.update(scene, &level, 0, 0.0, "07:28", 0, false, None, None);
            session.preparation = HomePreparation::start(
                Arc::clone(&session.fonts),
                level.menu_id.clone(),
                CardFaceCache::default(),
                move |id| {
                    if id == 1 {
                        entered_tx.send(()).unwrap();
                        let _ = release_rx.recv_timeout(Duration::from_secs(5));
                    }
                },
            )
            .unwrap();
            session.prefetch(vec![consoles()]);
            continue_rx.recv().unwrap();
            for index in 0..40 {
                let mut neighbour = consoles();
                neighbour.menu_id = format!("menu:fixture-{index}");
                session.prefetch(vec![neighbour]);
                assert!(session.aside.len() <= ASIDE_LEVELS);
                assert!(session.preparation.ownership_is_bounded());
            }
            session.set_inactive();
            drop(session);
            dropped_tx.send(()).unwrap();
        });
        let entered = entered_rx.recv_timeout(Duration::from_secs(5));
        let _ = continue_tx.send(());
        let dropped = dropped_rx.recv_timeout(Duration::from_secs(3));
        let _ = release_tx.send(());
        ui.join().unwrap();
        assert!(entered.is_ok());
        assert!(
            dropped.is_ok(),
            "launcher waited on the blocked preparation worker"
        );
    }

    #[test]
    fn pending_orientation_does_not_publish_old_geometry() {
        use std::sync::mpsc::channel;
        let scene = LauncherScene::new(960, 540);
        let portrait = LauncherScene::new(540, 960);
        let level = snapshot();
        let mut session = LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        session.preparation = HomePreparation::start(
            Arc::clone(&session.fonts),
            level.menu_id.clone(),
            CardFaceCache::default(),
            move |_| {
                entered_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(Duration::from_secs(5));
            },
        )
        .unwrap();
        session.update(portrait, &level, 0, 0.0, "07:28", 16, false, None, None);
        let entered = entered_rx.recv_timeout(Duration::from_secs(5));
        let old_scene_offered = session.scene_ready(portrait);
        let old_direct_offered = session.can_render_direct();
        let _ = release_tx.send(());
        assert!(entered.is_ok());
        assert!(!old_scene_offered);
        assert!(!old_direct_offered);
        wait_content(&mut session, portrait, &level, 0, "07:28");
        assert!(session.scene_ready(portrait));
        let reference = prepare(portrait, &level, 0, "07:28", &session.fonts);
        assert_eq!(session.render(), reference.pixels());
    }

    #[test]
    fn current_renderer_matches_serial_during_hold_reverse_and_wrap() {
        for level in [snapshot(), consoles()] {
            let scene = LauncherScene::new(960, 540);
            let mut session =
                LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
            let mut serial = prepare(scene, &level, 0, "07:28", &session.fonts);
            for (tick, position) in [0.0, 0.25, 0.75, 1.1, 1.8, 1.3, 0.9, -0.25, 0.0]
                .into_iter()
                .enumerate()
            {
                session.update(
                    scene,
                    &level,
                    0,
                    position,
                    "07:28",
                    tick as u64 * 16,
                    false,
                    None,
                    None,
                );
                serial.render_frame(session.frame);
                assert_eq!(session.render(), serial.pixels());
            }
        }
    }

    #[test]
    fn browse_ahead_preserves_pixels_and_rejects_changed_direction_predictions() {
        for level in [snapshot(), consoles()] {
            let scene = LauncherScene::new(960, 540);
            let mut session =
                LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
            let mut serial = prepare(scene, &level, 0, "07:28", &session.fonts);
            session.update(scene, &level, 0, 0.0, "07:28", 0, false, None, None);
            session.render_direct_bands();
            for (tick, position) in [0.25, 0.75, 1.1, 1.8, 1.3, 0.9, -0.25, 0.0]
                .into_iter()
                .enumerate()
            {
                let next_ms = (tick as u64 + 1) * 16;
                let predicted = if tick == 4 { 2.1 } else { position };
                let primary = session.current_primary_pixels().to_vec();
                let helper = session.current_helper_pixels().unwrap().to_vec();
                let request = session.current_request();
                session.prepare_browse_helper_ahead(next_ms, 0, predicted, None);
                assert_eq!(session.current_request(), request);
                assert!(session.current_primary_pixels() == primary);
                assert!(session.current_helper_pixels() == Some(helper.as_slice()));
                session.update(
                    scene, &level, 0, position, "07:28", next_ms, false, None, None,
                );
                serial.render_frame(session.frame);
                assert_eq!(session.render(), serial.pixels());
                let timing = session.last_timing().expect("new pose rendered");
                assert_eq!(timing.helper_ahead, tick != 4);
                assert_eq!(timing.discarded_generation.is_some(), tick == 4);
            }
        }
    }

    #[test]
    fn browse_ahead_uses_the_accepted_nested_frame_and_skips_unchanged_poses() {
        let scene = LauncherScene::new(960, 540);
        let level = consoles();
        let mut session = LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
        session.update(scene, &level, 0, 0.0, "07:28", 0, false, None, None);
        session.render();
        session.prepare_browse_helper_ahead(16, 0, 0.0, None);
        session.update(scene, &level, 0, 0.0, "07:28", 16, false, None, None);
        session.render();
        assert!(session.last_timing().is_none());
        let frame = BrowseFrame {
            selected: 0,
            target: 1,
            phase: BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis: 32,
            duration_millis: 460,
        };
        session.prepare_browse_helper_ahead(32, 0, 0.99, Some(frame));
        session.update(
            scene,
            &level,
            0,
            0.99,
            "07:28",
            32,
            false,
            Some(frame),
            None,
        );
        let mut serial = prepare(scene, &level, 0, "07:28", &session.fonts);
        serial.render_frame(frame);
        assert_eq!(session.render(), serial.pixels());
        assert!(session.last_timing().unwrap().helper_ahead);
    }

    #[test]
    fn current_renderer_reentry_delivers_the_current_pose() {
        let scene = LauncherScene::new(960, 540);
        let level = snapshot();
        let mut session = LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
        session.update(scene, &level, 0, 0.25, "07:28", 16, false, None, None);
        session.render();
        session.set_inactive();
        session.update(scene, &level, 1, 1.75, "07:28", 32, false, None, None);
        let mut serial = prepare(scene, &level, 1, "07:28", &session.fonts);
        serial.render_frame(session.frame);
        assert_eq!(session.render(), serial.pixels());
        assert!(session.can_render_direct());
        assert_eq!(session.current_request().timestamp_us, 32_000);
    }

    #[test]
    fn portrait_direct_bands_rotate_to_the_serial_frame() {
        use mister_magik_framebuffer_scenes::OutputRotation;
        let scene = LauncherScene::new(540, 960);
        let level = snapshot();
        let mut session = LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
        let frame = BrowseFrame {
            selected: 0,
            target: 1,
            phase: BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis: 230,
            duration_millis: 460,
        };
        session.update(scene, &level, 0, 0.5, "07:28", 230, true, Some(frame), None);
        assert!(session.can_render_direct());
        // A level change on a rotated output has no native chrome spans; its
        // whole-frame damage comes from the rotated bands.
        assert!(session.chrome_copy_damage(true).iter().next().is_none());
        session.render_direct_bands();
        // A new request (a level change starting after the render) leaves no
        // matching helper band: the caller then falls back to the Slint path.
        let rendered = session.last_request;
        session.next_request(frame);
        assert!(
            session
                .direct_physical_bands(
                    Rgb565OutputLayout::new(540, 960, 960, OutputRotation::Clockwise90).unwrap()
                )
                .is_none()
        );
        session.last_request = rendered;
        for rotation in [
            OutputRotation::Clockwise90,
            OutputRotation::CounterClockwise90,
        ] {
            let output = Rgb565OutputLayout::new(540, 960, 960, rotation).unwrap();
            let mut serial = prepare(scene, &level, 0, "07:28", &session.fonts);
            serial.render_frame(frame);
            let mut expected = vec![Rgb565Pixel(0); output.len()];
            assert!(rotate_rect(
                output,
                serial.pixels(),
                &mut expected,
                Rgb565Rect {
                    x0: 0,
                    y0: 0,
                    x1: 540,
                    y1: 960,
                },
            ));
            let bands = session.direct_physical_bands(output).expect("bands");
            let mut composed = bands.frame.to_vec();
            let rect = bands.damage[1];
            for y in rect.y0..rect.y1 {
                let range = y * 960 + rect.x0..y * 960 + rect.x1;
                composed[range.clone()].copy_from_slice(&bands.helper[range]);
            }
            assert!(composed == expected, "{rotation:?}");
        }
    }

    #[test]
    fn the_helper_rotates_its_own_band_and_the_frame_is_unchanged() {
        use mister_magik_framebuffer_scenes::OutputRotation;
        let scene = LauncherScene::new(540, 960);
        let level = snapshot();
        let frame = BrowseFrame {
            selected: 0,
            target: 1,
            phase: BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis: 230,
            duration_millis: 460,
        };
        for rotation in [
            OutputRotation::Clockwise90,
            OutputRotation::CounterClockwise90,
        ] {
            let output = Rgb565OutputLayout::new(540, 960, 960, rotation).unwrap();
            let mut session =
                LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
            session.set_output_layout(Some(output));
            session.update(scene, &level, 0, 0.5, "07:28", 230, true, Some(frame), None);
            session.render_direct_bands();
            // The helper rotated its band while it drew it.
            assert!(
                session
                    .renderer
                    .as_ref()
                    .unwrap()
                    .helper_rotated_pixels(session.last_request, output)
                    .is_some(),
                "{rotation:?}"
            );
            let mut serial = prepare(scene, &level, 0, "07:28", &session.fonts);
            serial.render_frame(frame);
            let mut expected = vec![Rgb565Pixel(0); output.len()];
            assert!(rotate_rect(
                output,
                serial.pixels(),
                &mut expected,
                Rgb565Rect {
                    x0: 0,
                    y0: 0,
                    x1: 540,
                    y1: 960,
                },
            ));
            let bands = session.direct_physical_bands(output).expect("bands");
            let mut composed = bands.frame.to_vec();
            let rect = bands.damage[1];
            for y in rect.y0..rect.y1 {
                let range = y * 960 + rect.x0..y * 960 + rect.x1;
                composed[range.clone()].copy_from_slice(&bands.helper[range]);
            }
            assert!(composed == expected, "{rotation:?}");
        }
    }

    /// The session across scene and orientation changes, in a seeded random
    /// order: every output in turn, frames at rest and in motion, and direct
    /// bands asked for with the right layout and with a layout left over from
    /// another orientation. Nothing may panic, a frame always equals the
    /// serial render for its scene, and bands exist only for the right layout.
    #[test]
    fn scene_and_orientation_walks_never_break_the_session() {
        use mister_magik_framebuffer_scenes::OutputRotation;
        let scenes = [
            LauncherScene::new(960, 540),
            LauncherScene::new(540, 960),
            LauncherScene::crt(640, 240),
            LauncherScene::crt(480, 640),
            LauncherScene::crt(640, 480),
        ];
        let moving = BrowseFrame {
            selected: 0,
            target: 1,
            phase: BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis: 230,
            duration_millis: 460,
        };
        let rotations = [
            OutputRotation::None,
            OutputRotation::Clockwise90,
            OutputRotation::CounterClockwise90,
        ];
        let mut state = 0x5CE7_E0F5_CA1E_D00Du64;
        let mut below = |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        let level = snapshot();
        let mut session =
            LauncherCardHomeSession::new(scenes[0], level.clone(), 0, "07:28").unwrap();
        let mut now = 100;
        let (mut direct_checked, mut stale_refused) = (0, 0);
        for step in 0..40 {
            let scene = scenes[below(scenes.len())];
            wait_content(&mut session, scene, &level, 0, "07:28");
            now += 16;
            let frame = if below(2) == 0 {
                moving
            } else {
                settled_frame(0)
            };
            session.update(scene, &level, 0, 0.5, "07:28", now, true, Some(frame), None);
            let mut serial = prepare(scene, &level, 0, "07:28", &session.fonts);
            serial.render_frame(frame);
            assert!(session.render() == serial.pixels(), "step {step} {scene:?}");
            if !session.can_render_direct() {
                continue;
            }
            // The portrait layout and the landscape one: right for one scene
            // each, a stale leftover for the others.
            let rotation = rotations[1 + below(2)];
            let own = Rgb565OutputLayout::new(540, 960, 960, rotation).unwrap();
            let landscape = Rgb565OutputLayout::new(960, 540, 960, OutputRotation::None).unwrap();
            let layouts = [own, landscape];
            let layout = layouts[below(2)];
            session.set_output_layout(Some(layout));
            session.render_direct_bands();
            let bands = session.direct_physical_bands(layout);
            if (layout.logical_width(), layout.logical_height()) == scene.size()
                && layout.rotation() != OutputRotation::None
            {
                direct_checked += 1;
                assert!(bands.is_some(), "step {step}: bands for {scene:?}");
            } else {
                stale_refused += usize::from(bands.is_none());
                if scene.size() != (layout.logical_width(), layout.logical_height()) {
                    assert!(bands.is_none(), "step {step}: stale layout for {scene:?}");
                }
            }
        }
        assert!(
            direct_checked > 0 && stale_refused > 0,
            "{direct_checked} {stale_refused}"
        );
    }

    /// A failed two-thread renderer (its helper stopped) must not abort the
    /// app: cards keep rendering the same pixels on one thread, the direct
    /// path turns itself off and the helper accessors report nothing.
    #[test]
    fn a_stopped_helper_degrades_to_one_thread_instead_of_aborting() {
        for scene in [LauncherScene::new(960, 540), LauncherScene::new(540, 960)] {
            let level = snapshot();
            let mut session =
                LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
            let frame = BrowseFrame {
                selected: 0,
                target: 1,
                phase: BrowsePhase::Flipping,
                direction: Some(BrowseDirection::Right),
                progress_millis: 230,
                duration_millis: 460,
            };
            session.update(scene, &level, 0, 0.5, "07:28", 230, true, Some(frame), None);
            session.render_direct_bands();
            assert!(session.can_render_direct() && session.current_helper_pixels().is_some());
            session.renderer.as_mut().unwrap().stop();
            session.update(scene, &level, 0, 0.6, "07:28", 246, true, Some(frame), None);
            let mut serial = prepare(scene, &level, 0, "07:28", &session.fonts);
            serial.render_frame(frame);
            session.last_rendered = None;
            session.render_direct_bands();
            assert!(session.renderer.is_none(), "{scene:?}");
            assert!(!session.can_render_direct());
            assert!(session.current_helper_pixels().is_none());
            assert!(session.rendered_split().is_none());
            assert!(session.current_primary_pixels() == serial.pixels());
        }
    }

    #[test]
    fn nested_elapsed_clock_is_independent_of_the_root_position_channel() {
        let scene = LauncherScene::new(960, 540);
        let level = consoles();
        let mut session = LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
        let frame = BrowseFrame {
            selected: 0,
            target: 1,
            phase: BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis: 230,
            duration_millis: 460,
        };
        session.update(
            scene,
            &level,
            0,
            0.99,
            "07:28",
            230,
            true,
            Some(frame),
            None,
        );
        assert_eq!(session.frame, frame);
        session.render();
        assert_eq!(session.current_request().timestamp_us, 230_000);
        assert!(session.is_animating());
    }

    #[test]
    fn route_changes_rebuild_faces_and_drop_native_tile_damage() {
        let mut session =
            LauncherCardHomeSession::new(LauncherScene::new(960, 540), snapshot(), 0, "07:28")
                .unwrap();
        let mut generation = session.content_generation();
        for scene in [
            LauncherScene::new(540, 960),
            LauncherScene::crt(640, 240),
            LauncherScene::crt(240, 640),
            LauncherScene::new(960, 540),
        ] {
            session.update(scene, &snapshot(), 0, 0.0, "07:28", 16, true, None, None);
            wait_content(&mut session, scene, &snapshot(), 0, "07:28");
            assert!(session.content_generation() > generation);
            generation = session.content_generation();
            assert_eq!(session.render().len(), scene.width * scene.height);
            session.note_compositor_copied(true);
            assert_eq!(
                session.compositor_copy_damage(true).is_some(),
                scene == LauncherScene::new(960, 540)
            );
            assert_eq!(
                session.can_render_direct(),
                scene == LauncherScene::new(960, 540) || scene.is_hdmi_portrait()
            );
            session.update(scene, &snapshot(), 0, 0.0, "07:29", 32, true, None, None);
            let expected = prepare(scene, &snapshot(), 0, "07:29", &session.fonts);
            assert_eq!(session.render(), expected.pixels());
        }
    }

    #[test]
    fn card_frames_follow_unbounded_position_through_both_wraps() {
        let settled = settled_frame(0);
        assert_eq!(
            browse_frame_from_position(0, 0.0, 0.0, settled, CARD_COUNT, true).phase,
            BrowsePhase::Settled
        );
        let right = browse_frame_from_position(0, 5.25, 5.1, settled_frame(5), CARD_COUNT, true);
        assert_eq!((right.selected, right.target), (5, 0));
        assert_eq!(right.direction, Some(BrowseDirection::Right));
        assert_eq!(right.progress_millis, SPRING_POSITION_UNITS / 4);
        let next = browse_frame_from_position(1, 6.25, 6.1, settled, CARD_COUNT, true);
        assert_eq!((next.selected, next.target), (0, 1));
        let left = browse_frame_from_position(5, -0.25, -0.1, settled, CARD_COUNT, true);
        assert_eq!((left.selected, left.target), (0, 5));
        assert_eq!(left.direction, Some(BrowseDirection::Left));
        assert_eq!(left.progress_millis, SPRING_POSITION_UNITS / 4);
        assert_eq!(
            browse_frame_from_position(5, -1.0, -0.9, left, CARD_COUNT, true).phase,
            BrowsePhase::Settled
        );
    }

    #[test]
    fn reversal_unwinds_the_same_two_card_rotations() {
        let forward = browse_frame_from_position(2, 1.30, 1.20, settled_frame(1), CARD_COUNT, true);
        let reverse = browse_frame_from_position(1, 1.25, 1.30, forward, CARD_COUNT, true);
        assert_eq!((forward.selected, forward.target), (1, 2));
        assert_eq!((reverse.selected, reverse.target), (1, 2));
        assert_eq!(reverse.direction, Some(BrowseDirection::Right));
        assert!(reverse.progress_millis < forward.progress_millis);

        let moving = browse_frame_from_position(1, 1.000_001, 1.01, reverse, CARD_COUNT, true);
        assert_eq!(moving.progress_millis, 1);
        assert_eq!(
            browse_frame_from_position(1, 1.0, 1.000_001, moving, CARD_COUNT, true).phase,
            BrowsePhase::Settled
        );
    }

    #[test]
    fn root_session_renders_exact_geometry_and_animates_toward_navigation() {
        let mut session =
            LauncherCardHomeSession::new(LauncherScene::new(960, 540), snapshot(), 0, "21:37")
                .unwrap();
        session.update(
            LauncherScene::new(960, 540),
            &snapshot(),
            0,
            0.0,
            "21:37",
            0,
            true,
            None,
            None,
        );
        session.update(
            LauncherScene::new(960, 540),
            &snapshot(),
            1,
            0.2,
            "21:37",
            10,
            true,
            None,
            None,
        );
        assert!(session.is_animating());
        assert_eq!(session.frame.selected, 0);
        assert_eq!(session.frame.target, 1);
        assert_eq!(session.render().len(), 960 * 540);
    }

    #[test]
    fn direct_publication_requires_one_compositor_reconciliation() {
        let scene = LauncherScene::new(960, 540);
        let level = snapshot();
        let mut session = LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
        let mut reference = LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
        reference.update(scene, &level, 0, 0.25, "07:28", 16, false, None, None);
        let captured = reference.render().to_vec();
        session.update(scene, &level, 0, 0.25, "07:28", 16, false, None, None);
        session.render_direct_bands();
        assert_eq!(session.last_timing().unwrap().merge_us, 0);
        let mut published = session.current_primary_pixels().to_vec();
        for y in 120..495 {
            let range = y * 960 + session.rendered_split().unwrap()..y * 960 + 934;
            published[range.clone()]
                .copy_from_slice(&session.current_helper_pixels().unwrap()[range]);
        }
        assert_eq!(published, captured);
        let generation = session.current_request().generation;
        session.note_direct_presented();
        assert!(session.compositor_stale());
        assert_eq!(session.render(), captured);
        assert_eq!(session.current_request().generation, generation);
        assert!(!session.compositor_stale());
    }

    #[test]
    fn compositor_cache_requires_seed_after_content_overlay_or_home_reentry() {
        let mut session =
            LauncherCardHomeSession::new(LauncherScene::new(960, 540), snapshot(), 0, "12:34")
                .unwrap();
        session.update(
            LauncherScene::new(960, 540),
            &snapshot(),
            0,
            0.0,
            "12:34",
            0,
            true,
            None,
            None,
        );
        assert_eq!(session.compositor_copy_damage(true), None);
        session.render();
        session.note_compositor_copied(true);
        let rect = session.compositor_copy_damage(true).unwrap();
        assert_eq!((rect.x1 - rect.x0) * (rect.y1 - rect.y0), 239250);
        session.update(
            LauncherScene::new(960, 540),
            &snapshot(),
            1,
            1.0,
            "12:34",
            16,
            true,
            None,
            None,
        );
        assert_eq!(session.compositor_copy_damage(true), Some(rect));
        session.update(
            LauncherScene::new(960, 540),
            &snapshot(),
            1,
            1.0,
            "12:35",
            32,
            true,
            None,
            None,
        );
        assert_eq!(session.compositor_copy_damage(true), None);
        session.note_compositor_copied(true);
        assert_eq!(session.compositor_copy_damage(false), None);
        session.note_compositor_copied(false); // overlay/full-raster poisons retained content
        assert_eq!(session.compositor_copy_damage(true), None);
        session.note_compositor_copied(true);
        session.set_inactive();
        assert_eq!(session.compositor_copy_damage(true), None);
    }

    #[test]
    fn clock_and_sidebar_refresh_preserve_worker_and_match_fresh_preparation() {
        let mut data = snapshot();
        let mut session =
            LauncherCardHomeSession::new(LauncherScene::new(960, 540), data.clone(), 0, "21:37")
                .unwrap();
        session.update(
            LauncherScene::new(960, 540),
            &data.clone(),
            0,
            0.0,
            "21:37",
            0,
            true,
            None,
            None,
        );
        let worker = session.renderer.as_ref().unwrap().helper_thread_id();
        for clock in ["21:38", "22:00"] {
            if let LevelSummary::Root {
                collections,
                library_games,
                ..
            } = &mut data.summary
            {
                *collections += 1;
                *library_games += 123;
            }
            session.update(
                LauncherScene::new(960, 540),
                &data.clone(),
                0,
                0.0,
                clock,
                16,
                true,
                None,
                None,
            );
            assert_eq!(
                session.renderer.as_ref().unwrap().helper_thread_id(),
                worker
            );
            assert!(
                session
                    .last_rendered
                    .is_none_or(|(_, generation)| generation != session.content_generation())
            );
            let mut reference = prepare(
                LauncherScene::new(960, 540),
                &data,
                0,
                clock,
                &session.fonts,
            );
            reference.render_frame(session.frame);
            assert_eq!(session.render(), reference.pixels());
        }
        data.cards[0].games = Some(999);
        session.update(
            LauncherScene::new(960, 540),
            &data,
            0,
            0.0,
            "22:00",
            32,
            true,
            None,
            None,
        );
        wait_content(
            &mut session,
            LauncherScene::new(960, 540),
            &data,
            0,
            "22:00",
        );
        assert_eq!(
            session.renderer.as_ref().unwrap().helper_thread_id(),
            worker
        );
    }

    #[cfg(feature = "tooling")]
    #[test]
    fn card_delivery_uses_completed_pixels_and_rejects_an_unrendered_motion_request() {
        let mut session =
            LauncherCardHomeSession::new(LauncherScene::new(960, 540), snapshot(), 0, "21:37")
                .unwrap();
        session.render_direct_bands();
        let completed = session.rendered_request().unwrap();
        let mut metrics = mister_magik_tooling_support::measurement::PresentationMetrics::default();
        assert!(
            metrics.note_card_delivery(session.current_request().generation, completed.generation)
        );
        // A quantized duplicate retains the same pixel identity and remains valid.
        session.render_direct_bands();
        assert_eq!(session.rendered_request(), Some(completed));
        assert!(
            metrics.note_card_delivery(session.current_request().generation, completed.generation)
        );
        // Request a new moving pose without producing it: the old bands must not
        // acquire the requested generation simply because submission advanced.
        let mut pose = session.frame;
        pose.selected = 1;
        let requested = session.next_request(pose);
        assert_eq!(session.rendered_request(), Some(completed));
        assert!(!metrics.note_card_delivery(
            requested.generation,
            session.rendered_request().unwrap().generation
        ));
        assert_eq!(metrics.counters.card_dropped_frames, 1);
        session.frame = pose;
        session.render_direct_bands();
        assert!(metrics.note_card_delivery(
            session.current_request().generation,
            session.rendered_request().unwrap().generation
        ));
    }

    #[test]
    fn settled_clean_home_does_not_sustain_render_work() {
        let snapshot = snapshot();
        let mut session = LauncherCardHomeSession::new(
            LauncherScene::new(960, 540),
            snapshot.clone(),
            0,
            "21:37",
        )
        .unwrap();
        session.update(
            LauncherScene::new(960, 540),
            &snapshot.clone(),
            0,
            0.0,
            "21:37",
            0,
            true,
            None,
            None,
        );
        session.render();
        let submitted_sequence = session.current_request().generation;

        session.update(
            LauncherScene::new(960, 540),
            &snapshot,
            0,
            0.0,
            "21:37",
            16,
            true,
            None,
            None,
        );

        assert_eq!(session.current_request().generation, submitted_sequence);
    }

    fn wait_trick_ready(
        session: &mut LauncherCardHomeSession,
        scene: LauncherScene,
        level: &CardLevelSnapshot,
        selected: usize,
        clock: &str,
        now: u64,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.trick.as_ref().is_some_and(|trick| !trick.ready) {
            session.update(
                scene,
                level,
                selected,
                selected as f32,
                clock,
                now,
                true,
                None,
                None,
            );
            assert!(Instant::now() < deadline, "transition did not become ready");
            std::thread::yield_now();
        }
    }

    #[test]
    fn blocked_preparation_never_starts_a_transition_that_can_hold_at_the_edge() {
        let scene = LauncherScene::new(960, 540);
        let root = snapshot();
        let target = consoles();
        let mut session = LauncherCardHomeSession::new(scene, root.clone(), 1, "12:00").unwrap();
        session.update(scene, &root, 1, 1.0, "12:00", 0, true, None, None);
        let old = session.render().to_vec();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        session.preparation = HomePreparation::start(
            Arc::clone(&session.fonts),
            root.menu_id.clone(),
            CardFaceCache::default(),
            move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            },
        )
        .unwrap();
        session.update(
            scene,
            &target,
            0,
            0.0,
            "12:00",
            100,
            true,
            None,
            Some("menu:consoles"),
        );
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        for now in [100, 300, 600, 1_000] {
            session.update(scene, &target, 0, 0.0, "12:00", now, true, None, None);
            assert!(!session.is_animating());
            assert_eq!(session.evidence_pose(), ("level-preparing", 0));
            assert_eq!(session.render(), old);
        }
        release_tx.send(()).unwrap();
        wait_trick_ready(&mut session, scene, &target, 0, "12:00", 1_000);
        assert_eq!(session.trick.as_ref().unwrap().started_ms, 1_000);
        session.render();
        // Each produced-frame step advances the pose. It cannot repeat the
        // edge while waiting for work, even after a deliberately slow load.
        for t in (16..=928).step_by(16) {
            session.prepare_helper_ahead(1_000 + t);
            session.update(scene, &target, 0, 0.0, "12:00", 1_000 + t, true, None, None);
            let (phase, progress) = session.evidence_pose();
            assert_eq!(progress, t.min(u64::from(LEVEL_TRICK_MILLIS)));
            assert_eq!(
                phase,
                if t < u64::from(LEVEL_TRICK_EDGE_MILLIS) {
                    "level-gather"
                } else {
                    "level-deal"
                }
            );
            session.render();
        }
        assert!(session.trick.is_none());
        assert!(session.preparation.quiescent());
    }

    fn handoff_catalog() -> crate::arcade_catalog::ArcadeCatalog {
        use crate::test_support::{arcade_catalog, arcade_game, arcade_system};
        arcade_catalog(
            vec![
                arcade_game("Mario").system_id("nes").build(),
                arcade_game("Zelda").system_id("snes").build(),
            ],
            vec![arcade_system("nes", 1), arcade_system("snes", 1)],
        )
    }

    #[test]
    fn undrawn_back_batches_keep_the_presented_source_until_acceptance() {
        use crate::launcher::{LauncherAction, LauncherEvent, LauncherNav};
        let catalog = handoff_catalog();
        let scene = LauncherScene::new(960, 540);
        for last in [LauncherAction::NavigateBack, LauncherAction::NavigateHome] {
            let mut nav = LauncherNav::new();
            nav.sync_launcher_taxonomy(&catalog);
            nav.selected = 1;
            nav.open_menu("menu:consoles");
            nav.acknowledge_home_level_transition();
            nav.open_menu("menu:consoles:nintendo");
            nav.acknowledge_home_level_transition();
            nav.selected = 1;
            let source = CardLevelSnapshot::from_runtime(&nav, &catalog);
            let mut session =
                LauncherCardHomeSession::new(scene, source.clone(), 1, "12:00").unwrap();
            session.update_from_navigation(scene, &source, 1, 1.0, "12:00", 0, true, None, None);
            session.render();
            nav.commit_navigation_intent(
                &LauncherEvent {
                    action: LauncherAction::NavigateBack,
                    path: None,
                    settings: None,
                },
                &catalog,
            );
            nav.commit_navigation_intent(
                &LauncherEvent {
                    action: last,
                    path: None,
                    settings: None,
                },
                &catalog,
            );
            let destination = CardLevelSnapshot::from_runtime(&nav, &catalog);
            assert!(session.update_from_navigation(
                scene,
                &destination,
                nav.selected,
                nav.home_card_visual_index(),
                "12:00",
                16,
                true,
                None,
                nav.home_level_transition()
            ));
            nav.acknowledge_home_level_transition();
            assert!(nav.home_level_transition().is_none());
            assert_eq!(
                session.trick.as_ref().unwrap().source_level.menu_id,
                source.menu_id
            );
            assert_eq!(session.trick.as_ref().unwrap().source_selected, 1);
            wait_trick_ready(&mut session, scene, &destination, nav.selected, "12:00", 16);
            let mut reference = prepare(scene, &source, 1, "12:00", &session.fonts);
            reference.render_level_gather_to(
                1,
                LevelChange::Ascend,
                0,
                session.trick.as_ref().unwrap().destination_slot,
            );
            assert_eq!(session.render(), reference.pixels());
        }
    }

    #[test]
    fn admission_failures_settle_source_and_do_not_acknowledge_navigation() {
        use crate::launcher::LauncherNav;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        let scene = LauncherScene::new(960, 540);
        let catalog = handoff_catalog();
        for (retirement, valid_origin) in [(true, true), (false, true), (false, false)] {
            let mut nav = LauncherNav::new();
            nav.sync_launcher_taxonomy(&catalog);
            let source = CardLevelSnapshot::from_runtime(&nav, &catalog);
            let mut session =
                LauncherCardHomeSession::new(scene, source.clone(), 0, "12:00").unwrap();
            session.update_from_navigation(scene, &source, 0, 0.0, "12:00", 0, true, None, None);
            session.update_from_navigation(scene, &source, 1, 0.99, "12:00", 16, true, None, None);
            assert_eq!(session.frame.phase, BrowsePhase::Flipping);
            nav.selected = 1;
            nav.open_menu("menu:consoles");
            let target = CardLevelSnapshot::from_runtime(&nav, &catalog);
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let once = AtomicBool::new(false);
            session.preparation = HomePreparation::start(
                Arc::clone(&session.fonts),
                source.menu_id.clone(),
                CardFaceCache::default(),
                move |_| {
                    if !once.swap(true, Ordering::Relaxed) {
                        entered_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    }
                },
            )
            .unwrap();
            let mut requests = vec![
                session
                    .preparation
                    .request(scene, &source, 0, "12:00", true)
                    .unwrap(),
            ];
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if retirement {
                while session.preparation.can_retire(2) {
                    session.preparation.retire(Box::new(prepare(
                        scene,
                        &source,
                        0,
                        "12:00",
                        &session.fonts,
                    )));
                }
            } else {
                while let Some(id) = session
                    .preparation
                    .request(scene, &source, 0, "12:00", false)
                {
                    requests.push(id);
                }
            }
            let receipt = if valid_origin {
                nav.home_level_transition()
            } else {
                None
            };
            let accepted = session
                .update_from_navigation(scene, &target, 0, 0.0, "12:00", 32, true, None, receipt);
            assert!(!accepted);
            assert!(
                session.waiting_for_destination(),
                "admission failure must retain the 16 ms retry wakeup"
            );
            assert!(
                nav.home_level_transition().is_some(),
                "admission failure must retain the receipt"
            );
            assert!(session.trick.is_none());
            assert_eq!(session.frame, settled_frame(1));
            let mut reference = prepare(scene, &source, 1, "12:00", &session.fonts);
            reference.render_frame(settled_frame(1));
            assert_eq!(
                session.render(),
                reference.pixels(),
                "never hold the half-flipped source"
            );
            for id in requests {
                session.preparation.cancel(id);
            }
            release_tx.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let receipt = if valid_origin {
                    nav.home_level_transition()
                } else {
                    None
                };
                if session.update_from_navigation(
                    scene, &target, 0, 0.0, "12:00", 32, true, None, receipt,
                ) {
                    nav.acknowledge_home_level_transition();
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "handoff never accepted after admission recovered"
                );
                std::thread::yield_now();
            }
            assert!(nav.home_level_transition().is_none());
            assert_eq!(session.trick.is_some(), valid_origin);
        }
    }

    #[test]
    fn queued_activation_keeps_the_correct_pixels_across_the_navigation_commit() {
        use crate::input_state::PadState;
        use crate::launcher::{LauncherAction, LauncherNav};
        use crate::test_support::{arcade_catalog, arcade_game, arcade_system};
        let catalog = arcade_catalog(
            vec![
                arcade_game("Mario").system_id("nes").build(),
                arcade_game("Agony").system_id("amiga").build(),
            ],
            vec![arcade_system("nes", 1), arcade_system("amiga", 1)],
        );
        for (initial, right, activated) in [(0, true, 1), (1, true, 2), (3, false, 2)] {
            let scene = LauncherScene::new(960, 540);
            let mut nav = LauncherNav::new();
            nav.sync_launcher_taxonomy(&catalog);
            nav.selected = initial;
            nav.restore_pending_home_view(nav.home_view_state());
            let start = Instant::now();
            nav.handle_input_with_navigation_intents(&PadState::default(), start, &catalog);
            let source = CardLevelSnapshot::from_runtime(&nav, &catalog);
            let mut session =
                LauncherCardHomeSession::new(scene, source.clone(), initial, "21:37").unwrap();
            session.update_from_navigation(
                scene,
                &source,
                initial,
                initial as f32,
                "21:37",
                0,
                true,
                None,
                None,
            );
            session.render();
            let direction = PadState {
                dpad_right: right,
                dpad_left: !right,
                ..Default::default()
            };
            nav.handle_input_with_navigation_intents(
                &direction,
                start + Duration::from_millis(16),
                &catalog,
            );
            nav.handle_input_with_navigation_intents(
                &PadState::default(),
                start + Duration::from_millis(32),
                &catalog,
            );
            nav.handle_input_with_navigation_intents(
                &PadState {
                    btn_a: true,
                    ..Default::default()
                },
                start + Duration::from_millis(48),
                &catalog,
            );
            let mut committed = false;
            for tick in 4..180 {
                let now = start + Duration::from_millis(tick * 16);
                let event =
                    nav.handle_input_with_navigation_intents(&PadState::default(), now, &catalog);
                let previous = session.frame;
                if let Some(event) = event {
                    assert_eq!(event.action, LauncherAction::OpenMenu);
                    assert_eq!(nav.selected, activated);
                    assert_ne!(
                        previous.selected, activated,
                        "must reproduce a stale outgoing frame"
                    );
                    assert!(nav.commit_navigation_intent(&event, &catalog));
                    committed = true;
                }
                let current = CardLevelSnapshot::from_runtime(&nav, &catalog);
                session.update_from_navigation(
                    scene,
                    &current,
                    nav.selected,
                    nav.home_card_visual_index(),
                    "21:37",
                    tick * 16,
                    true,
                    nav.home_card_browse_prediction(now),
                    nav.home_level_transition(),
                );
                if committed {
                    let trick = session
                        .trick
                        .as_ref()
                        .expect("navigation origin should start the trick");
                    assert_eq!(trick.source_selected, activated);
                    if !trick.ready {
                        let mut waiting =
                            prepare(scene, &source, activated, "21:37", &session.fonts);
                        waiting.render_frame(settled_frame(activated));
                        assert!(
                            session.render() == waiting.pixels(),
                            "preparation must retain the committed source card"
                        );
                        assert!(!session.is_animating());
                    }
                    wait_trick_ready(
                        &mut session,
                        scene,
                        &current,
                        nav.selected,
                        "21:37",
                        tick * 16,
                    );
                    let trick = session.trick.as_ref().unwrap();
                    assert_eq!(trick.source_selected, activated);
                    let mut expected = prepare(scene, &source, activated, "21:37", &session.fonts);
                    expected.render_level_gather_to(
                        activated,
                        LevelChange::Descend,
                        0,
                        trick.destination_slot,
                    );
                    assert!(
                        session.render() == expected.pixels(),
                        "wrong source card pixels at activation"
                    );
                    break;
                }
                session.render();
            }
            assert!(
                committed,
                "queued activation never committed: initial={initial} selected={} position={}",
                nav.selected,
                nav.home_card_visual_index()
            );
        }
    }

    #[test]
    fn missing_or_stale_origin_cannot_animate_an_unrelated_source_card() {
        let scene = LauncherScene::new(960, 540);
        for origin in [
            None,
            Some(CardLevelTransition {
                source_level: "unrelated-level".into(),
                source_card: "menu:consoles".into(),
                destination_level: consoles().menu_id,
            }),
            Some(CardLevelTransition {
                source_level: snapshot().menu_id,
                source_card: "removed-card".into(),
                destination_level: consoles().menu_id,
            }),
        ] {
            let mut session = LauncherCardHomeSession::new(scene, snapshot(), 1, "21:37").unwrap();
            session.update_from_navigation(
                scene,
                &snapshot(),
                1,
                1.0,
                "21:37",
                0,
                true,
                None,
                None,
            );
            let pixels = session.render().to_vec();
            session.update_from_navigation(
                scene,
                &consoles(),
                0,
                0.0,
                "21:37",
                16,
                true,
                None,
                origin.as_ref(),
            );
            assert!(session.trick.is_none());
            if session.level.is_root() {
                assert!(
                    session.render() == pixels,
                    "waiting must preserve coherent source pixels"
                );
            }
            wait_content(&mut session, scene, &consoles(), 0, "21:37");
            assert!(session.trick.is_none());
            assert_eq!(session.frame.selected, 0);
        }
    }

    #[test]
    fn a_portrait_level_change_presents_through_the_rotated_bands() {
        use mister_magik_framebuffer_scenes::OutputRotation;
        let scene = LauncherScene::new(540, 960);
        let output = Rgb565OutputLayout::new(540, 960, 960, OutputRotation::Clockwise90).unwrap();
        let mut session = LauncherCardHomeSession::new(scene, snapshot(), 1, "21:37").unwrap();
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true, None, None);
        session.render();
        assert!(session.can_render_direct());
        session.update(
            scene,
            &consoles(),
            0,
            0.0,
            "21:37",
            16,
            true,
            None,
            Some("menu:consoles"),
        );
        wait_trick_ready(&mut session, scene, &consoles(), 0, "21:37", 16);
        // Several frames in a row: after the first, only the fading chrome is
        // rotated again, so each frame must still equal the serial one.
        for now in [200, 216, 232, 248] {
            session.update(scene, &consoles(), 0, 0.0, "21:37", now, true, None, None);
            assert!(session.is_level_trick_active());
            assert!(session.can_render_direct());
            session.render_direct_bands();
            let trick = session.trick.as_ref().unwrap();
            let elapsed = (session.now_ms - trick.started_ms) as u32;
            assert!(
                !trick.dealing && elapsed < LEVEL_TRICK_EDGE_MILLIS,
                "a gather frame"
            );
            let mut expected = prepare(
                scene,
                &snapshot(),
                trick.source_selected,
                "21:37",
                &session.fonts,
            );
            expected.render_level_gather_to(
                trick.source_selected,
                LevelChange::Descend,
                elapsed,
                trick.destination_slot,
            );
            let mut rotated = vec![Rgb565Pixel(0); output.len()];
            assert!(rotate_rect(
                output,
                expected.pixels(),
                &mut rotated,
                Rgb565Rect {
                    x0: 0,
                    y0: 0,
                    x1: 540,
                    y1: 960,
                },
            ));
            let bands = session.direct_physical_bands(output).expect("bands");
            // The chrome the level change fades is copied, and only that.
            let damage = bands.chrome_damage.as_ref().expect("fading chrome damage");
            assert!(!damage.is_empty());
            let mut composed = bands.frame.to_vec();
            let rect = bands.damage[1];
            for y in rect.y0..rect.y1 {
                let range = y * 960 + rect.x0..y * 960 + rect.x1;
                composed[range.clone()].copy_from_slice(&bands.helper[range]);
            }
            assert!(
                composed == rotated,
                "the rotated level-change frame at {now}"
            );
        }
    }

    #[test]
    fn level_change_plays_the_trick_then_settles_on_the_destination() {
        let scene = LauncherScene::new(960, 540);
        let mut session = LauncherCardHomeSession::new(scene, snapshot(), 1, "21:37").unwrap();
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true, None, None);
        session.render();
        assert!(!session.is_level_trick_active());
        session.update(
            scene,
            &consoles(),
            0,
            0.0,
            "21:37",
            16,
            true,
            None,
            Some("menu:consoles"),
        );
        wait_trick_ready(&mut session, scene, &consoles(), 0, "21:37", 16);
        assert!(session.is_animating());
        assert!(session.is_level_trick_active(), "input is held during it");
        assert!(session.can_render_direct());
        assert_eq!(session.compositor_copy_damage(true), None);
        let source_generation = session.current_request().generation;
        // The destination is ready before the gather begins.
        session.update(scene, &consoles(), 0, 0.0, "21:37", 200, true, None, None);
        session.render();
        assert_eq!(session.current_request().frame.selected, 1);
        assert_eq!(session.current_request().timestamp_us, 200_000);
        assert!(session.current_request().generation > source_generation);
        let current = session.current_request();
        let helper = session.current_helper_pixels().unwrap().to_vec();
        session.prepare_helper_ahead(216);
        assert_eq!(session.now_ms, 200);
        assert_eq!(session.current_request(), current);
        assert_eq!(session.current_helper_pixels(), Some(helper.as_slice()));
        assert!(session.is_level_trick_active());
        session.update(scene, &consoles(), 0, 0.0, "21:37", 216, true, None, None);
        let produced = session.render().to_vec();
        assert!(session.last_timing().unwrap().helper_ahead);
        let mut expected = prepare(scene, &snapshot(), 1, "21:37", &session.fonts);
        expected.render_level_gather_to(
            1,
            LevelChange::Descend,
            200,
            session.trick.as_ref().unwrap().destination_slot,
        );
        assert_eq!(produced, expected.pixels());
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut now = 400;
        while session.trick.as_ref().is_some_and(|trick| !trick.dealing) {
            assert!(Instant::now() < deadline, "level preparation timed out");
            now += 16;
            session.update(scene, &consoles(), 0, 0.0, "21:37", now, true, None, None);
            session.render();
            std::thread::yield_now();
        }
        now += u64::from(LEVEL_TRICK_MILLIS);
        session.update(scene, &consoles(), 0, 0.0, "21:37", now, true, None, None);
        #[cfg(feature = "tooling")]
        let final_evidence = {
            let nav = crate::launcher::LauncherNav::new();
            let mut frame =
                Some(mister_magik_tooling_support::frame_evidence::FrameEvidence::default());
            assert!(
                super::super::launcher_frame_accounting::capture_evidence_state(
                    &mut frame,
                    &nav,
                    Some(&session),
                    super::super::launcher_pacing::FrameProductionClass::EventDriven,
                    now * 1000,
                    0,
                    1,
                )
            );
            frame.unwrap()
        };
        session.render();
        #[cfg(feature = "tooling")]
        {
            assert_eq!(final_evidence.pose_phase, "level-deal");
            assert_eq!(final_evidence.pose_progress, u64::from(LEVEL_TRICK_MILLIS));
            assert!(final_evidence.motion);
            assert!(final_evidence.card_snapshot_locked);
        }
        assert!(session.trick.is_none());
        assert!(!session.is_level_trick_active());
        assert!(session.can_render_direct());
        assert_eq!(session.current_request().frame.selected, 0);
        assert_eq!(session.current_request().timestamp_us, now * 1_000);
        let mut expected = prepare(scene, &consoles(), 0, "21:37", &session.fonts);
        expected.render_frame(settled_frame(0));
        assert_eq!(session.render(), expected.pixels());
    }

    #[test]
    fn reduced_motion_changes_level_without_the_trick() {
        let scene = LauncherScene::crt(640, 240);
        let mut session = LauncherCardHomeSession::new(scene, snapshot(), 1, "21:37").unwrap();
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true, None, None);
        session.update(scene, &consoles(), 0, 0.0, "21:37", 16, false, None, None);
        wait_content(&mut session, scene, &consoles(), 0, "21:37");
        assert!(session.trick.is_none());
        let mut expected = prepare(scene, &consoles(), 0, "21:37", &session.fonts);
        expected.render_frame(settled_frame(0));
        assert_eq!(session.render(), expected.pixels());
    }

    #[test]
    fn leaving_home_mid_trick_settles_the_destination() {
        let scene = LauncherScene::new(960, 540);
        let mut session = LauncherCardHomeSession::new(scene, snapshot(), 1, "21:37").unwrap();
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true, None, None);
        session.update(
            scene,
            &consoles(),
            0,
            0.0,
            "21:37",
            16,
            true,
            None,
            Some("menu:consoles"),
        );
        session.set_inactive();
        assert!(!session.active);
        wait_content(&mut session, scene, &consoles(), 0, "21:37");
        let mut expected = prepare(scene, &consoles(), 0, "21:37", &session.fonts);
        expected.render_frame(settled_frame(0));
        assert_eq!(session.render(), expected.pixels());
    }

    #[test]
    fn nested_browse_frames_stop_at_the_ends() {
        let left = browse_frame_from_position(0, -0.25, -0.1, settled_frame(0), 3, false);
        assert_eq!((left.selected, left.target), (0, 0));
        let right = browse_frame_from_position(2, 1.5, 1.4, settled_frame(1), 3, false);
        assert_eq!((right.selected, right.target), (1, 2));
    }

    #[test]
    fn prefetched_level_swaps_at_the_edge_without_holding() {
        let scene = LauncherScene::new(960, 540);
        let mut session = LauncherCardHomeSession::new(scene, snapshot(), 1, "21:37").unwrap();
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true, None, None);
        let helper = session.renderer.as_ref().unwrap().helper_thread_id();
        session.prefetch(vec![consoles()]);
        assert_eq!(session.aside.len(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(&session.aside[0].prepared, Prepared::Building(id) if session.preparation.is_ready(*id))
        {
            assert!(Instant::now() < deadline, "prefetch timed out");
            std::thread::yield_now();
        }
        session.update(
            scene,
            &consoles(),
            0,
            0.0,
            "21:37",
            100,
            true,
            None,
            Some("menu:consoles"),
        );
        wait_trick_ready(&mut session, scene, &consoles(), 0, "21:37", 100);
        assert!(session.aside.is_empty(), "the prefetched level was used");
        session.update(
            scene,
            &consoles(),
            0,
            0.0,
            "21:37",
            100 + u64::from(LEVEL_TRICK_EDGE_MILLIS) + 17,
            true,
            None,
            None,
        );
        session.render();
        assert_eq!(
            session.renderer.as_ref().unwrap().helper_thread_id(),
            helper
        );
        let trick = session.trick.as_ref().unwrap();
        assert!(trick.dealing, "prepared destination adopted at the edge");
    }

    #[test]
    fn parent_prefetch_from_a_nested_level_keeps_the_prepared_root() {
        use crate::launcher::LauncherNav;
        use crate::test_support::{arcade_catalog, arcade_game, arcade_system};
        let catalog = arcade_catalog(
            vec![
                arcade_game("Super Mario Bros").system_id("nes").build(),
                arcade_game("Super Mario 64").system_id("n64").build(),
                arcade_game("Sonic").system_id("gamegear").build(),
            ],
            vec![
                arcade_system("nes", 1),
                arcade_system("n64", 1),
                arcade_system("gamegear", 1),
            ],
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        let root = CardLevelSnapshot::from_runtime(&nav, &catalog);
        assert!(nav.open_menu("menu:consoles"));
        let consoles = CardLevelSnapshot::from_runtime(&nav, &catalog);

        let scene = LauncherScene::new(960, 540);
        let mut session = LauncherCardHomeSession::new(scene, root.clone(), 1, "21:37").unwrap();
        session.update(scene, &root, 1, 1.0, "21:37", 0, true, None, None);
        session.update(
            scene,
            &consoles,
            0,
            0.0,
            "21:37",
            16,
            true,
            None,
            Some("menu:consoles"),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut now = 16;
        while session.trick.is_some() {
            assert!(Instant::now() < deadline, "level change timed out");
            now += 16;
            session.update(scene, &consoles, 0, 0.0, "21:37", now, true, None, None);
            session.render();
            std::thread::yield_now();
        }
        // The loop's idle prefetch from the nested level: the parent.
        let parent = nav.parent_menu_id().map(str::to_owned).unwrap();
        let parent = CardLevelSnapshot::for_menu(&nav, &catalog, &parent);
        assert_eq!(parent, root, "the parent snapshot matches the real root");
        session.prefetch(vec![parent]);
        assert!(
            session.aside.iter().any(|aside| {
                aside.level == root && matches!(aside.prepared, Prepared::Built(_))
            }),
            "the prepared root was kept, not replaced by a rebuild"
        );
        // Back to the root: the deal starts at the edge without holding.
        session.update(
            scene,
            &root,
            1,
            1.0,
            "21:38",
            now + 100,
            true,
            None,
            Some(consoles.cards[0].navigation_id.as_str()),
        );
        wait_trick_ready(&mut session, scene, &root, 1, "21:38", now + 100);
        session.update(
            scene,
            &root,
            1,
            1.0,
            "21:38",
            now + 100 + u64::from(LEVEL_TRICK_EDGE_MILLIS),
            true,
            None,
            None,
        );
        session.render();
        assert!(
            session.trick.as_ref().unwrap().dealing,
            "prepared destination adopted at the edge"
        );
    }

    #[test]
    fn returning_to_a_left_level_reuses_it_without_holding() {
        let scene = LauncherScene::new(960, 540);
        let mut session = LauncherCardHomeSession::new(scene, snapshot(), 1, "21:37").unwrap();
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true, None, None);
        session.update(
            scene,
            &consoles(),
            0,
            0.0,
            "21:37",
            16,
            true,
            None,
            Some("menu:consoles"),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut now = 16;
        while session.trick.is_some() {
            assert!(Instant::now() < deadline, "level change timed out");
            now += 16;
            session.update(scene, &consoles(), 0, 0.0, "21:37", now, true, None, None);
            session.render();
            std::thread::yield_now();
        }
        assert!(
            session.aside.iter().any(|aside| {
                aside.level.menu_id == crate::launcher_taxonomy::ROOT_MENU_ID
                    && matches!(aside.prepared, Prepared::Built(_))
            }),
            "the root was set aside when left"
        );
        // Back to the root: nothing to prepare, so the deal starts at the edge.
        session.update(
            scene,
            &snapshot(),
            1,
            1.0,
            "21:38",
            now + 100,
            true,
            None,
            Some("ATARI"),
        );
        wait_trick_ready(&mut session, scene, &snapshot(), 1, "21:38", now + 100);
        assert!(
            session
                .aside
                .iter()
                .all(|aside| aside.level.menu_id != crate::launcher_taxonomy::ROOT_MENU_ID),
            "the root set aside was used"
        );
        session.update(
            scene,
            &snapshot(),
            1,
            1.0,
            "21:38",
            now + 100 + u64::from(LEVEL_TRICK_EDGE_MILLIS),
            true,
            None,
            None,
        );
        session.render();
        assert!(session.trick.as_ref().unwrap().dealing);
    }
}
