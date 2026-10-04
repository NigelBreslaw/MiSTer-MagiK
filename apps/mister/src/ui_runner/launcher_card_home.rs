// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production owner for the custom RGB565 card launcher: the root cards and
//! every nested hierarchy level, including the level-change card trick.

use super::{DirtyRect, DirtyRectList};
use crate::bitmap_font_resource::{
    jersey_25_console_bitmap_font, launcher_bitmap_font, nocive_15_console_bitmap_font,
    spleen_6x12_native_console_bitmap_font, xerxes_10_console_bitmap_font,
};
use crate::launcher_home::{CARD_COUNT, CardLevelSnapshot};
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use mister_magik_framebuffer_scenes::bitmap_text::BitmapFont;
use mister_magik_framebuffer_scenes::launcher::{
    CardSlot, LEVEL_TRICK_EDGE_MILLIS, LEVEL_TRICK_MILLIS, LauncherFaceCache, LauncherFrameRequest,
    LauncherScene, LauncherTypography, LevelChange, PreparedLauncher,
};
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowseFrame, BrowsePhase, SPRING_POSITION_UNITS,
};
use mister_magik_framebuffer_scenes::launcher_parallel::{
    ParallelFrameTiming, ParallelLauncherRenderer,
};
use std::sync::Arc;
#[path = "launcher_card_preparation.rs"]
mod preparation;
use preparation::{HomePreparation, PreparedContent};

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

const CARD_RGB888: [&[u8]; CARD_COUNT] = [
    include_bytes!("../../assets/ui/launcher-cards/01_arcade.rgb888"),
    include_bytes!("../../assets/ui/launcher-cards/02_consoles.rgb888"),
    include_bytes!("../../assets/ui/launcher-cards/03_computers.rgb888"),
    include_bytes!("../../assets/ui/launcher-cards/04_handhelds.rgb888"),
    include_bytes!("../../assets/ui/launcher-cards/05_favourites.rgb888"),
    include_bytes!("../../assets/ui/launcher-cards/06_settings.rgb888"),
];

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
/// the destination is made ready; the swap happens at the all-edge-on moment,
/// holding there if it is not ready yet.
struct LevelTrick {
    change: LevelChange,
    source_level: CardLevelSnapshot,
    source_selected: usize,
    destination_selected: usize,
    source_slot: CardSlot,
    destination_slot: CardSlot,
    edge_waiting: bool,
    started_ms: u64,
    destination: Option<Prepared>,
    /// Set when the destination became the prepared level. A late swap
    /// delays the deal by the time spent holding edge-on.
    deal_delay_ms: Option<u64>,
}

pub(super) struct LauncherCardHomeSession {
    scene: LauncherScene,
    level: CardLevelSnapshot,
    clock: String,
    fonts: Arc<LauncherFonts>,
    prepared: VisiblePrepared,
    preparation: HomePreparation,
    pending: Option<PendingLevel>,
    trick: Option<LevelTrick>,
    aside: Vec<Aside>,
    now_ms: u64,
    renderer: Option<Box<ParallelLauncherRenderer>>,
    last_rendered: Option<(BrowseFrame, u64)>,
    last_timing: Option<ParallelFrameTiming>,
    last_request: LauncherFrameRequest,
    last_visual_index: f32,
    frame: BrowseFrame,
    active: bool,
    content_dirty: bool,
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
        let mut cache = LauncherFaceCache::default();
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
            trick: None,
            aside: Vec::new(),
            now_ms: 0,
            renderer,
            last_rendered: None,
            last_timing: None,
            last_request: LauncherFrameRequest {
                frame,
                timestamp_us: 0,
                generation: 0,
            },
            last_visual_index: selected as f32,
            frame,
            active: false,
            content_dirty: true,
            content_generation: 1,
            compositor_stale: false,
            compositor_content_generation: None,
            measure_preparation: std::env::var_os("MISTER_MAGIK2_STATE_ROOT").is_some(),
            preparation_measurement: None,
        })
    }

    pub(super) fn set_inactive(&mut self) {
        // Preserve a pending destination while away; returning can adopt it
        // without waiting on or destroying a preparation worker here.
        self.invalidate_compositor();
        self.active = false;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn update(
        &mut self,
        scene: LauncherScene,
        level: &CardLevelSnapshot,
        selected: usize,
        visual_index: f32,
        clock: &str,
        now_ms: u64,
        motion: bool,
        nested_frame: Option<BrowseFrame>,
    ) {
        let count = level.cards.len().max(1);
        let selected = selected.min(count - 1);
        self.now_ms = now_ms;
        if self.trick.is_some() {
            if self.active && self.level.menu_id == level.menu_id && self.scene == scene && motion {
                return;
            }
            if !self.finish_trick() {
                return;
            }
        }
        let level_changed = self.level.menu_id != level.menu_id;
        if level_changed && self.active && self.scene == scene && motion {
            self.begin_trick(level.clone(), selected);
            return;
        }
        let faces_changed = self.scene != scene || self.level.cards != level.cards || level_changed;
        if faces_changed {
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
                return;
            }
            let Some(content) = self
                .pending
                .as_ref()
                .and_then(|pending| self.preparation.take(pending.id))
            else {
                return;
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
        if navigation_identity_changed(previous_frame, self.frame) {
            self.content_dirty = true;
        }

        if self.level != *level || self.clock != clock {
            let preparation_started = self.measure_preparation.then(std::time::Instant::now);
            self.level = level.clone();
            self.clock.clear();
            self.clock.push_str(clock);
            self.content_generation = self.content_generation.wrapping_add(1).max(1);
            self.refresh_chrome(self.frame.selected);

            self.content_dirty = true;
            self.preparation_measurement = preparation_started
                .map(|start| start.elapsed().as_micros().try_into().unwrap_or(u64::MAX));
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
            prepared.refresh_chrome(data, Some(typography))
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
    fn begin_trick(&mut self, level: CardLevelSnapshot, selected: usize) {
        if !self.preparation.can_retire(2) {
            return;
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
            return;
        };
        self.trick = Some(LevelTrick {
            change,
            source_slot: self.prepared.slot_zero(),
            destination_slot: self.scene.slot_zero(!level.is_root()),
            edge_waiting: false,
            source_level: std::mem::replace(&mut self.level, level),
            source_selected: self.frame.selected,
            destination_selected: selected,
            started_ms: self.now_ms,
            destination: Some(destination),
            deal_delay_ms: None,
        });
        self.frame = settled_frame(selected);
        self.last_visual_index = selected as f32;
        self.content_generation = self.content_generation.wrapping_add(1).max(1);
        self.content_dirty = true;
    }

    /// Taking a completed destination never joins or rebuilds on the UI.
    fn take_built_destination(&mut self) -> Option<PreparedContent> {
        if !self.preparation.can_retire(2) {
            return None;
        }
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
        let (source, selected) = self.trick.as_ref().map_or_else(
            || (None, self.frame.selected),
            |trick| (Some(trick.source_level.clone()), trick.destination_selected),
        );
        let old = self.prepared.0.replace(content).unwrap();
        if let Some(source) = source {
            self.set_aside(source, Prepared::Built(old));
        } else {
            self.retire(Prepared::Built(old));
        }
        self.refresh_chrome(selected);
    }

    /// Finish an interruption when ready; otherwise restore the source and
    /// keep the preparation warm. No wait is needed to acknowledge leaving.
    fn finish_trick(&mut self) -> bool {
        let Some(trick) = self.trick.as_ref() else {
            return true;
        };
        if trick.deal_delay_ms.is_none() {
            if !self.preparation.can_retire(2) {
                return false;
            }
            if let Some(content) = self.take_built_destination() {
                self.install_destination(content);
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
        true
    }

    /// Render the current trick frame. Returns false once the trick is over.
    fn render_trick(&mut self) -> bool {
        let Some(trick) = self.trick.as_ref() else {
            return false;
        };
        let elapsed = self.now_ms.saturating_sub(trick.started_ms);
        let preparing = trick.deal_delay_ms.is_none();
        let edge = u64::from(LEVEL_TRICK_EDGE_MILLIS);
        if preparing
            && elapsed >= u64::from(LEVEL_TRICK_MILLIS * 45 / 100)
            && elapsed < edge
            && let Some(prepared) = self.take_built_destination()
        {
            self.trick.as_mut().unwrap().destination = Some(Prepared::Built(prepared));
        }
        if preparing && elapsed >= edge {
            if let Some(prepared) = self.take_built_destination() {
                self.install_destination(prepared);
                if let Some(trick) = self.trick.as_mut() {
                    trick.deal_delay_ms = Some(if trick.edge_waiting {
                        elapsed - edge
                    } else {
                        0
                    });
                }
            } else if let Some(trick) = self.trick.as_mut() {
                trick.edge_waiting = true;
            }
        }
        let Some(trick) = self.trick.as_ref() else {
            return false;
        };
        let Some(delay) = trick.deal_delay_ms else {
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
        let t = elapsed
            .saturating_sub(delay)
            .min(u64::from(LEVEL_TRICK_MILLIS)) as u32;
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
        self.last_timing = if self.scene == LauncherScene::new(960, 540)
            && let Some(renderer) = self.renderer.as_mut()
        {
            Some(
                if gather {
                    self.prepared
                        .render_level_gather_to_parallel(request, change, t, slot, renderer)
                } else {
                    self.prepared
                        .render_level_deal_from_parallel(request, change, t, slot, renderer)
                }
                .expect("current level rendering failed"),
            )
        } else {
            if gather {
                self.prepared
                    .render_level_gather_to(selected, change, t, slot);
            } else {
                self.prepared
                    .render_level_deal_from(selected, change, t, slot);
            }
            None
        };
    }

    /// Queue a helper band for exactly the next FrameClock step. Do not cross
    /// the preparation/swap or landing boundaries, where state may change.
    pub(super) fn prepare_helper_ahead(&mut self, next_ms: u64) {
        if !self.can_render_native() || next_ms <= self.now_ms {
            return;
        }
        let Some(trick) = self.trick.as_ref() else {
            return;
        };
        let elapsed = next_ms.saturating_sub(trick.started_ms);
        let (selected, t, slot, gather) = if let Some(delay) = trick.deal_delay_ms {
            let t = elapsed.saturating_sub(delay);
            if t >= u64::from(LEVEL_TRICK_MILLIS) {
                return;
            }
            (
                trick.destination_selected,
                t as u32,
                trick.source_slot,
                false,
            )
        } else {
            if elapsed >= u64::from(LEVEL_TRICK_EDGE_MILLIS) {
                return;
            }
            (
                trick.source_selected,
                elapsed as u32,
                trick.destination_slot,
                true,
            )
        };
        let request = LauncherFrameRequest {
            frame: settled_frame(selected),
            timestamp_us: next_ms.saturating_mul(1_000),
            generation: self.last_request.generation.wrapping_add(1).max(1),
        };
        let preparer = self
            .prepared
            .level_frame_preparer(selected, trick.change, t, slot, gather);
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
        if !self.can_render_native() || self.trick.is_some() || next_ms <= self.now_ms {
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
        self.active && (self.trick.is_some() || self.frame.phase != BrowsePhase::Settled)
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
            if self.scene == LauncherScene::new(960, 540)
                && let Some(renderer) = self.renderer.as_mut()
            {
                self.last_timing = Some(
                    self.prepared
                        .render_parallel_frame(renderer, request)
                        .expect("current card rendering failed"),
                );
            } else {
                self.prepared.render_frame(self.frame);
                self.last_timing = None;
            }
            self.last_rendered = Some((self.frame, self.content_generation));
        } else {
            if !retain_bands
                && self.scene == LauncherScene::new(960, 540)
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

    #[cfg(feature = "tooling")]
    pub(super) fn evidence_pose(&self) -> (&'static str, u64) {
        if let Some(trick) = self.trick.as_ref() {
            let elapsed = self.now_ms.saturating_sub(trick.started_ms);
            if let Some(delay) = trick.deal_delay_ms {
                (
                    "level-deal",
                    elapsed
                        .saturating_sub(delay)
                        .min(u64::from(LEVEL_TRICK_MILLIS)),
                )
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
        assert_eq!(self.scene, LauncherScene::new(960, 540));
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
    pub(super) fn can_render_native(&self) -> bool {
        self.active
            && self.scene == LauncherScene::new(960, 540)
            && self.renderer.is_some()
            && self
                .pending
                .as_ref()
                .is_none_or(|pending| pending.scene == self.scene)
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
    pub(super) fn last_timing(&self) -> Option<ParallelFrameTiming> {
        self.last_timing
    }
    pub(super) fn current_primary_pixels(&self) -> &[Rgb565Pixel] {
        self.prepared.pixels()
    }

    pub(super) fn current_helper_pixels(&self) -> &[Rgb565Pixel] {
        self.renderer
            .as_ref()
            .and_then(|renderer| renderer.helper_pixels(self.last_request))
            .expect("matching current helper band")
    }

    pub(super) fn rendered_split(&self) -> usize {
        self.renderer
            .as_ref()
            .expect("native renderer")
            .rendered_split()
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
    // Only the root cards have approved artwork; nested levels are generic.
    let root = level.is_root();
    let rgb888: &[&[u8]] = if root { &CARD_RGB888 } else { &[] };
    level.with_data(selected, clock, |data| {
        scene
            .prepare_initial_with_rgb888_artwork_and_typography(data, rgb888, fonts.typography())
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
    cache: &mut LauncherFaceCache,
) -> PreparedLauncher {
    let root = level.is_root();
    let rgb888: &[&[u8]] = if root { &CARD_RGB888 } else { &[] };
    // Immutable assets/fonts belong to this session. Root and nested assets
    // are separate contexts even when card IDs and geometry coincide.
    let assets = if root { 1 } else { 2 };
    level.with_data(selected, clock, |data| {
        scene
            .prepare_initial_with_rgb888_artwork_typography_and_cache(
                data,
                rgb888,
                fonts.typography(),
                cache,
                assets,
            )
            .finish()
    })
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
    fn panicking_preparation_recovers_pending_and_trick_content_off_ui() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for (motion, prefetch) in [(false, false), (true, false), (true, true)] {
            let scene = LauncherScene::new(960, 540);
            let root = snapshot();
            let mut destination = consoles();
            let mut session =
                LauncherCardHomeSession::new(scene, root.clone(), 0, "07:28").unwrap();
            session.update(scene, &root, 0, 0.0, "07:28", 0, motion, None);
            let attempts = Arc::new(AtomicUsize::new(0));
            let worker_attempts = Arc::clone(&attempts);
            let ui_thread = std::thread::current().id();
            session.preparation = HomePreparation::start(
                Arc::clone(&session.fonts),
                root.menu_id.clone(),
                LauncherFaceCache::default(),
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
                session.update(scene, &destination, 0, 0.0, "07:28", now, motion, None);
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
        session.update(scene, &root, 0, 0.0, "07:28", 0, false, None);
        let attempts = Arc::new(AtomicUsize::new(0));
        let worker_attempts = Arc::clone(&attempts);
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        session.preparation = HomePreparation::start(
            Arc::clone(&session.fonts),
            root.menu_id.clone(),
            LauncherFaceCache::default(),
            move |_| {
                if worker_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                panic!("persistent preparation failure");
            },
        )
        .unwrap();
        session.update(scene, &destination, 0, 0.0, "07:28", 16, false, None);
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session.preparation.has_failed() {
            assert!(Instant::now() < deadline, "worker failure was not recorded");
            std::thread::yield_now();
        }
        let failure = catch_unwind(AssertUnwindSafe(|| {
            session.update(scene, &destination, 0, 0.0, "07:28", 32, false, None);
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
            session.update(scene, &level, 0, 0.0, "07:28", 0, false, None);
            session.render();
            level.cards[0].games = Some(999);
            crate::allocation_metrics::begin();
            session.update(scene, &level, 0, 0.0, "07:28", 16, false, None);
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
                session.update(scene, &level, 0, 0.0, "07:28", 0, false, None);
                let old_pixels = session.render().to_vec();
                session.preparation = HomePreparation::start(
                    Arc::clone(&session.fonts),
                    level.menu_id.clone(),
                    LauncherFaceCache::default(),
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
                session.update(scene, &changed, 0, 0.0, "07:28", 16, false, None);
                continue_rx.recv().unwrap();
                changed.cards[0].games = Some(999);
                session.update(scene, &changed, 1, 1.0, "07:29", 32, false, None);
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
            session.update(scene, &level, 0, 0.0, "07:28", 0, false, None);
            session.preparation = HomePreparation::start(
                Arc::clone(&session.fonts),
                level.menu_id.clone(),
                LauncherFaceCache::default(),
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
            LauncherFaceCache::default(),
            move |_| {
                entered_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(Duration::from_secs(5));
            },
        )
        .unwrap();
        session.update(portrait, &level, 0, 0.0, "07:28", 16, false, None);
        let entered = entered_rx.recv_timeout(Duration::from_secs(5));
        let old_scene_offered = session.scene_ready(portrait);
        let old_direct_offered = session.can_render_native();
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
            session.update(scene, &level, 0, 0.0, "07:28", 0, false, None);
            session.render_direct_bands();
            for (tick, position) in [0.25, 0.75, 1.1, 1.8, 1.3, 0.9, -0.25, 0.0]
                .into_iter()
                .enumerate()
            {
                let next_ms = (tick as u64 + 1) * 16;
                let predicted = if tick == 4 { 2.1 } else { position };
                let primary = session.current_primary_pixels().to_vec();
                let helper = session.current_helper_pixels().to_vec();
                let request = session.current_request();
                session.prepare_browse_helper_ahead(next_ms, 0, predicted, None);
                assert_eq!(session.current_request(), request);
                assert!(session.current_primary_pixels() == primary);
                assert!(session.current_helper_pixels() == helper);
                session.update(scene, &level, 0, position, "07:28", next_ms, false, None);
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
        session.update(scene, &level, 0, 0.0, "07:28", 0, false, None);
        session.render();
        session.prepare_browse_helper_ahead(16, 0, 0.0, None);
        session.update(scene, &level, 0, 0.0, "07:28", 16, false, None);
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
        session.update(scene, &level, 0, 0.99, "07:28", 32, false, Some(frame));
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
        session.update(scene, &level, 0, 0.25, "07:28", 16, false, None);
        session.render();
        session.set_inactive();
        session.update(scene, &level, 1, 1.75, "07:28", 32, false, None);
        let mut serial = prepare(scene, &level, 1, "07:28", &session.fonts);
        serial.render_frame(session.frame);
        assert_eq!(session.render(), serial.pixels());
        assert!(session.can_render_native());
        assert_eq!(session.current_request().timestamp_us, 32_000);
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
        session.update(scene, &level, 0, 0.99, "07:28", 230, true, Some(frame));
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
            session.update(scene, &snapshot(), 0, 0.0, "07:28", 16, true, None);
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
                session.can_render_native(),
                scene == LauncherScene::new(960, 540)
            );
            session.update(scene, &snapshot(), 0, 0.0, "07:29", 32, true, None);
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
        reference.update(scene, &level, 0, 0.25, "07:28", 16, false, None);
        let captured = reference.render().to_vec();
        session.update(scene, &level, 0, 0.25, "07:28", 16, false, None);
        session.render_direct_bands();
        assert_eq!(session.last_timing().unwrap().merge_us, 0);
        let mut published = session.current_primary_pixels().to_vec();
        for y in 120..495 {
            let range = y * 960 + session.rendered_split()..y * 960 + 934;
            published[range.clone()].copy_from_slice(&session.current_helper_pixels()[range]);
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
        );

        assert_eq!(session.current_request().generation, submitted_sequence);
    }

    #[test]
    fn level_change_plays_the_trick_then_settles_on_the_destination() {
        let scene = LauncherScene::new(960, 540);
        let mut session = LauncherCardHomeSession::new(scene, snapshot(), 1, "21:37").unwrap();
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true, None);
        session.render();
        assert!(!session.is_level_trick_active());
        session.update(scene, &consoles(), 0, 0.0, "21:37", 16, true, None);
        assert!(session.is_animating());
        assert!(session.is_level_trick_active(), "input is held during it");
        assert!(session.can_render_native());
        assert_eq!(session.compositor_copy_damage(true), None);
        let source_generation = session.current_request().generation;
        // Gather, then hold edge-on until the worker has prepared the level.
        session.update(scene, &consoles(), 0, 0.0, "21:37", 200, true, None);
        session.render();
        assert_eq!(session.current_request().frame.selected, 1);
        assert_eq!(session.current_request().timestamp_us, 200_000);
        assert!(session.current_request().generation > source_generation);
        let current = session.current_request();
        let helper = session.current_helper_pixels().to_vec();
        session.prepare_helper_ahead(216);
        assert_eq!(session.now_ms, 200);
        assert_eq!(session.current_request(), current);
        assert_eq!(session.current_helper_pixels(), helper);
        assert!(session.is_level_trick_active());
        session.update(scene, &consoles(), 0, 0.0, "21:37", 216, true, None);
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
        while session
            .trick
            .as_ref()
            .is_some_and(|trick| trick.deal_delay_ms.is_none())
        {
            assert!(Instant::now() < deadline, "level preparation timed out");
            now += 16;
            session.update(scene, &consoles(), 0, 0.0, "21:37", now, true, None);
            session.render();
            std::thread::yield_now();
        }
        now += u64::from(LEVEL_TRICK_MILLIS);
        session.update(scene, &consoles(), 0, 0.0, "21:37", now, true, None);
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
        assert!(session.can_render_native());
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
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true, None);
        session.update(scene, &consoles(), 0, 0.0, "21:37", 16, false, None);
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
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true, None);
        session.update(scene, &consoles(), 0, 0.0, "21:37", 16, true, None);
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
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true, None);
        let helper = session.renderer.as_ref().unwrap().helper_thread_id();
        session.prefetch(vec![consoles()]);
        assert_eq!(session.aside.len(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(&session.aside[0].prepared, Prepared::Building(id) if session.preparation.is_ready(*id))
        {
            assert!(Instant::now() < deadline, "prefetch timed out");
            std::thread::yield_now();
        }
        session.update(scene, &consoles(), 0, 0.0, "21:37", 100, true, None);
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
        );
        session.render();
        assert_eq!(
            session.renderer.as_ref().unwrap().helper_thread_id(),
            helper
        );
        let trick = session.trick.as_ref().unwrap();
        assert_eq!(
            trick.deal_delay_ms,
            Some(0),
            "no time spent holding edge-on"
        );
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
        session.update(scene, &root, 1, 1.0, "21:37", 0, true, None);
        session.update(scene, &consoles, 0, 0.0, "21:37", 16, true, None);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut now = 16;
        while session.trick.is_some() {
            assert!(Instant::now() < deadline, "level change timed out");
            now += 16;
            session.update(scene, &consoles, 0, 0.0, "21:37", now, true, None);
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
        session.update(scene, &root, 1, 1.0, "21:38", now + 100, true, None);
        session.update(
            scene,
            &root,
            1,
            1.0,
            "21:38",
            now + 100 + u64::from(LEVEL_TRICK_EDGE_MILLIS),
            true,
            None,
        );
        session.render();
        assert_eq!(
            session.trick.as_ref().unwrap().deal_delay_ms,
            Some(0),
            "no time spent holding edge-on"
        );
    }

    #[test]
    fn returning_to_a_left_level_reuses_it_without_holding() {
        let scene = LauncherScene::new(960, 540);
        let mut session = LauncherCardHomeSession::new(scene, snapshot(), 1, "21:37").unwrap();
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true, None);
        session.update(scene, &consoles(), 0, 0.0, "21:37", 16, true, None);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut now = 16;
        while session.trick.is_some() {
            assert!(Instant::now() < deadline, "level change timed out");
            now += 16;
            session.update(scene, &consoles(), 0, 0.0, "21:37", now, true, None);
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
        session.update(scene, &snapshot(), 1, 1.0, "21:38", now + 100, true, None);
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
        );
        session.render();
        assert_eq!(session.trick.as_ref().unwrap().deal_delay_ms, Some(0));
    }
}
