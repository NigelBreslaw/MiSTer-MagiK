// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production owner for the custom RGB565 card launcher: the root cards and
//! every nested hierarchy level, including the level-change card trick.

use super::DirtyRect;
use crate::bitmap_font_resource::{
    jersey_25_console_bitmap_font, launcher_bitmap_font, nocive_15_console_bitmap_font,
    spleen_6x12_native_console_bitmap_font, xerxes_10_console_bitmap_font,
};
use crate::launcher_home::{CARD_COUNT, CardLevelSnapshot};
use crate::ui_runner::launcher_card_pipeline::{
    CardFrameRequest, CardPipelineCounters, LauncherCardRenderAhead, RenderedCardFrame,
};
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use mister_magik_framebuffer_scenes::bitmap_text::BitmapFont;
use mister_magik_framebuffer_scenes::launcher::{
    LEVEL_TRICK_EDGE_MILLIS, LEVEL_TRICK_MILLIS, LauncherFaceCache, LauncherFrameRequest,
    LauncherScene, LauncherTypography, LevelChange, PreparedLauncher,
};
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowseFrame, BrowsePhase, SPRING_POSITION_UNITS,
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
    render_ahead: Option<LauncherCardRenderAhead>,
    render_ahead_enabled: bool,
    presented_frame: Option<RenderedCardFrame>,
    navigation_generation: u64,
    request_sequence: u64,
    target_vblank: u64,
    frame_timestamp_us: u64,
    last_visual_index: f32,
    frame: BrowseFrame,
    active: bool,
    content_dirty: bool,
    content_generation: u64,
    compositor_stale: bool,
    compositor_content_generation: Option<u64>,
    retired_pipeline_counters: CardPipelineCounters,
    #[cfg(feature = "tooling")]
    reported_pipeline_counters: CardPipelineCounters,
    measure_preparation: bool,
    preparation_measurement: Option<(bool, bool, u64)>,
}

impl LauncherCardHomeSession {
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
        let render_ahead = native_render_ahead(scene, &prepared);
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
            render_ahead,
            render_ahead_enabled: true,
            presented_frame: None,
            navigation_generation: 1,
            request_sequence: 0,
            target_vblank: 0,
            frame_timestamp_us: 0,
            last_visual_index: selected as f32,
            frame,
            active: false,
            content_dirty: true,
            content_generation: 1,
            compositor_stale: false,
            compositor_content_generation: None,
            retired_pipeline_counters: CardPipelineCounters::default(),
            #[cfg(feature = "tooling")]
            reported_pipeline_counters: CardPipelineCounters::default(),
            measure_preparation: std::env::var_os("MISTER_MAGIK2_PROFILE_DIR").is_some(),
            preparation_measurement: None,
        })
    }

    /// Acknowledge the route chosen by the compositor. In-flight work may
    /// finish, but fallback owns rendering until the direct route is restored.
    pub(super) fn set_render_ahead_enabled(&mut self, enabled: bool) {
        if self.render_ahead_enabled == enabled {
            return;
        }
        self.render_ahead_enabled = enabled;
        self.release_presented_frame();
        self.bump_navigation_generation();
        if let Some(pipeline) = self.render_ahead.as_ref() {
            pipeline.invalidate_content_generation(self.content_generation);
        }
        if enabled && self.active {
            self.content_dirty = true;
            self.submit_render_ahead();
        }
    }

    pub(super) fn set_inactive(&mut self) {
        // Preserve a pending destination while away; returning can adopt it
        // without waiting on or destroying a preparation worker here.
        self.invalidate_compositor();
        self.active = false;
        self.release_presented_frame();
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
            self.release_presented_frame();
            if self.scene != scene {
                self.clear_aside();
            }
            let old = PreparedContent {
                prepared: self.prepared.0.replace(content.prepared).unwrap(),
                pipeline: std::mem::replace(&mut self.render_ahead, content.pipeline),
                retirement_baseline: CardPipelineCounters::default(),
            };
            self.retire(Prepared::Built(old));
            self.scene = scene;
            self.level = level.clone();
            self.clock = clock.into();
            self.refresh_chrome(selected);
            self.frame = settled_frame(selected);
            self.last_visual_index = selected as f32;
            self.content_generation = self.content_generation.wrapping_add(1).max(1);
            self.bump_navigation_generation();
            if let Some(pipeline) = &self.render_ahead {
                pipeline.invalidate_content_generation(self.content_generation);
            }
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
            self.bump_navigation_generation();
        }

        self.frame = browse_frame_from_position(
            selected,
            visual_index,
            self.last_visual_index,
            previous_frame,
            count,
            level.cycles(),
        );
        self.last_visual_index = visual_index;
        self.frame_timestamp_us = now_ms.saturating_mul(1_000);
        if navigation_identity_changed(previous_frame, self.frame) {
            self.bump_navigation_generation();
            self.content_dirty = true;
        }

        if self.level != *level || self.clock != clock {
            let preparation_started = self.measure_preparation.then(std::time::Instant::now);
            self.release_presented_frame();
            self.level = level.clone();
            self.clock.clear();
            self.clock.push_str(clock);
            self.content_generation = self.content_generation.wrapping_add(1).max(1);
            self.refresh_chrome(self.frame.selected);
            if let Some(pipeline) = self.render_ahead.as_ref() {
                pipeline.invalidate_content_generation(self.content_generation);
            }
            self.content_dirty = true;
            self.preparation_measurement = preparation_started.map(|start| {
                (
                    false,
                    false,
                    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                )
            });
        }
        if self.active && (self.content_dirty || self.is_animating()) {
            self.submit_render_ahead();
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
            Prepared::Built(mut content) => {
                if let Some(pipeline) = &content.pipeline {
                    content.retirement_baseline = pipeline.counters();
                    self.retired_pipeline_counters
                        .add_assign(content.retirement_baseline);
                }
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
        self.release_presented_frame();
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
        self.bump_navigation_generation();
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
        self.release_presented_frame();
        let (source, selected) = self.trick.as_ref().map_or_else(
            || (None, self.frame.selected),
            |trick| (Some(trick.source_level.clone()), trick.destination_selected),
        );
        let old = PreparedContent {
            prepared: self.prepared.0.replace(content.prepared).unwrap(),
            pipeline: std::mem::replace(&mut self.render_ahead, content.pipeline),
            retirement_baseline: CardPipelineCounters::default(),
        };
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
        self.bump_navigation_generation();
        if let Some(pipeline) = &self.render_ahead {
            pipeline.invalidate_content_generation(self.content_generation);
        }
        self.content_dirty = true;
        true
    }

    /// Render the current trick frame. Returns false once the trick is over.
    fn render_trick(&mut self) -> bool {
        let Some(trick) = self.trick.as_ref() else {
            return false;
        };
        let elapsed = self.now_ms.saturating_sub(trick.started_ms);
        let edge = u64::from(LEVEL_TRICK_EDGE_MILLIS);
        if trick.deal_delay_ms.is_none()
            && elapsed >= edge
            && let Some(prepared) = self.take_built_destination()
        {
            self.install_destination(prepared);
            if let Some(trick) = self.trick.as_mut() {
                trick.deal_delay_ms = Some(elapsed - edge);
            }
        }
        let Some(trick) = self.trick.as_ref() else {
            return false;
        };
        let Some(delay) = trick.deal_delay_ms else {
            let t = elapsed.min(edge) as u32;
            self.prepared
                .render_level_gather(trick.source_selected, trick.change, t);
            return true;
        };
        let t = elapsed
            .saturating_sub(delay)
            .min(u64::from(LEVEL_TRICK_MILLIS)) as u32;
        self.prepared
            .render_level_deal(trick.destination_selected, trick.change, t);
        if t >= LEVEL_TRICK_MILLIS {
            self.trick = None;
            self.content_generation = self.content_generation.wrapping_add(1).max(1);
        }
        true
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
        self.active
            && (self.content_dirty
                || self.trick.is_some()
                || self.is_animating()
                || self.render_ahead_enabled
                    && self
                        .render_ahead
                        .as_ref()
                        .is_some_and(LauncherCardRenderAhead::has_ready))
    }

    pub(super) fn render(&mut self) -> &[Rgb565Pixel] {
        self.release_presented_frame();
        if self.render_trick() {
            self.content_dirty = true;
            self.compositor_stale = false;
            return self.prepared.pixels();
        }
        self.prepared.render_frame(self.frame);
        self.content_dirty = false;
        self.compositor_stale = false;
        self.prepared.pixels()
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

    pub(super) const fn content_generation(&self) -> u64 {
        self.content_generation
    }

    pub(super) fn set_target_vblank(&mut self, target_vblank: u64) {
        self.target_vblank = target_vblank;
    }

    pub(super) fn chrome_pixels(&self) -> &[Rgb565Pixel] {
        self.prepared.pixels()
    }

    pub(super) const fn compositor_stale(&self) -> bool {
        self.compositor_stale
    }

    pub(super) fn invalidate_compositor(&mut self) {
        self.compositor_content_generation = None;
    }

    pub(super) fn compositor_copy_damage(&self, motion_only: bool) -> Option<DirtyRect> {
        // The trick fades the header and summary: always copy the whole frame.
        (motion_only
            && self.trick.is_none()
            && self.scene == LauncherScene::new(960, 540)
            && self.compositor_content_generation == Some(self.content_generation))
        .then_some(DirtyRect {
            x0: 296,
            y0: 120,
            x1: 934,
            y1: 495,
        })
    }

    pub(super) fn note_compositor_copied(&mut self, motion_only: bool) {
        self.compositor_content_generation = motion_only.then_some(self.content_generation);
    }

    pub(super) fn note_direct_presented(&mut self, frame: RenderedCardFrame) {
        self.invalidate_compositor();
        let previous = self.presented_frame.replace(frame);
        if let (Some(render_ahead), Some(previous)) = (self.render_ahead.as_ref(), previous) {
            render_ahead.recycle(previous);
        }
        self.content_dirty = false;
        self.compositor_stale = true;
    }

    pub(super) fn presented_render_ahead(&self) -> Option<&RenderedCardFrame> {
        self.presented_frame.as_ref()
    }

    pub(super) fn try_take_render_ahead(
        &self,
        now_us: u64,
        maximum_age_us: u64,
    ) -> Option<RenderedCardFrame> {
        if self.trick.is_some()
            || !self.render_ahead_enabled
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.scene != self.scene)
        {
            return None;
        }
        self.render_ahead.as_ref()?.try_take(
            self.content_generation,
            self.navigation_generation,
            now_us,
            maximum_age_us,
        )
    }

    pub(super) fn recycle_render_ahead(&self, frame: RenderedCardFrame) {
        if let Some(render_ahead) = self.render_ahead.as_ref() {
            render_ahead.recycle(frame);
        }
    }

    pub(super) fn return_render_ahead(&self, frame: RenderedCardFrame) {
        if let Some(render_ahead) = self.render_ahead.as_ref() {
            render_ahead.return_ready(frame);
        }
    }

    #[cfg(feature = "tooling")]
    pub(super) fn pipeline_counter_delta(&mut self) -> CardPipelineCounters {
        self.retired_pipeline_counters
            .add_assign(self.preparation.take_retired_counters());
        let mut current = self.retired_pipeline_counters;
        for aside in &self.aside {
            if let Prepared::Built(content) = &aside.prepared
                && let Some(pipeline) = &content.pipeline
            {
                current.add_assign(pipeline.counters());
            }
        }
        if let Some(render_ahead) = self.render_ahead.as_ref() {
            current.add_assign(render_ahead.counters());
        }
        let delta = current.delta(self.reported_pipeline_counters);
        self.reported_pipeline_counters = current;
        delta
    }

    #[cfg(feature = "tooling")]
    pub(super) fn take_preparation_measurement(&mut self) -> Option<(bool, bool, u64)> {
        self.preparation_measurement.take()
    }

    fn submit_render_ahead(&mut self) {
        if !self.render_ahead_enabled {
            return;
        }
        let Some(render_ahead) = self.render_ahead.as_ref() else {
            return;
        };
        self.request_sequence = self.request_sequence.wrapping_add(1).max(1);
        render_ahead.submit(CardFrameRequest {
            render: LauncherFrameRequest {
                frame: self.frame,
                timestamp_us: self.frame_timestamp_us,
                generation: self.request_sequence,
            },
            content_generation: self.content_generation,
            navigation_generation: self.navigation_generation,
            target_vblank: self.target_vblank,
        });
    }

    fn bump_navigation_generation(&mut self) {
        self.navigation_generation = self.navigation_generation.wrapping_add(1).max(1);
    }

    fn release_presented_frame(&mut self) {
        let Some(frame) = self.presented_frame.take() else {
            return;
        };
        if let Some(render_ahead) = self.render_ahead.as_ref() {
            render_ahead.recycle(frame);
        }
    }
}

fn native_render_ahead(
    scene: LauncherScene,
    prepared: &PreparedLauncher,
) -> Option<LauncherCardRenderAhead> {
    (scene == LauncherScene::new(960, 540)).then(|| {
        LauncherCardRenderAhead::start(
            prepared.frame_preparer(),
            std::env::var_os("MISTER_MAGIK2_PROFILE_DIR").is_some(),
        )
    })
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
        self.release_presented_frame();
        let mut contents = Vec::with_capacity(ASIDE_LEVELS + 2);
        if let Some(prepared) = self.prepared.0.take() {
            contents.push(PreparedContent {
                prepared,
                pipeline: self.render_ahead.take(),
                retirement_baseline: CardPipelineCounters::default(),
            });
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
        self.preparation.shutdown(contents);
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
            session.update(scene, level, selected, selected as f32, clock, 32, false);
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
            session.update(scene, &root, 0, 0.0, "07:28", 0, motion);
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
                session.update(scene, &destination, 0, 0.0, "07:28", now, motion);
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
        session.update(scene, &root, 0, 0.0, "07:28", 0, false);
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
        session.update(scene, &destination, 0, 0.0, "07:28", 16, false);
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session.preparation.has_failed() {
            assert!(Instant::now() < deadline, "worker failure was not recorded");
            std::thread::yield_now();
        }
        let failure = catch_unwind(AssertUnwindSafe(|| {
            session.update(scene, &destination, 0, 0.0, "07:28", 32, false);
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
            session.update(scene, &level, 0, 0.0, "07:28", 0, false);
            session.render();
            level.cards[0].games = Some(999);
            crate::allocation_metrics::begin();
            session.update(scene, &level, 0, 0.0, "07:28", 16, false);
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
                session.update(scene, &level, 0, 0.0, "07:28", 0, false);
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
                session.update(scene, &changed, 0, 0.0, "07:28", 16, false);
                continue_rx.recv().unwrap();
                changed.cards[0].games = Some(999);
                session.update(scene, &changed, 1, 1.0, "07:29", 32, false);
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
            session.update(scene, &level, 0, 0.0, "07:28", 0, false);
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
        session.update(portrait, &level, 0, 0.0, "07:28", 16, false);
        let entered = entered_rx.recv_timeout(Duration::from_secs(5));
        let old_scene_offered = session.scene_ready(portrait);
        let old_direct_offered = session.try_take_render_ahead(16_000, u64::MAX).is_some();
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
    fn fallback_route_stops_new_producer_work_and_preserves_pixels() {
        let mut submissions = Vec::new();
        for level in [snapshot(), consoles()] {
            let scene = LauncherScene::new(960, 540);
            let mut session =
                LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
            session.render_ahead = Some(LauncherCardRenderAhead::start(
                session.prepared.frame_preparer(),
                true,
            ));
            session.update(scene, &level, 0, 0.0, "07:28", 0, false);
            session.set_render_ahead_enabled(false);
            let before = session.render_ahead.as_ref().unwrap().counters().submitted;
            let mut serial = prepare(scene, &level, 0, "07:28", &session.fonts);
            for tick in 1..=120 {
                let position = if tick % 2 == 0 { 0.25 } else { 0.75 };
                session.update(scene, &level, 0, position, "07:28", tick * 16, false);
                serial.render_frame(session.frame);
                assert!(
                    session.render() == serial.pixels(),
                    "fallback pixels differ at {tick}"
                );
            }
            let submitted = session.render_ahead.as_ref().unwrap().counters().submitted - before;
            println!(
                "fallback_new_producer_submissions={} level={}",
                submitted, level.menu_id
            );
            submissions.push(submitted);
            session.set_render_ahead_enabled(true);
            let request_sequence = session.request_sequence;
            session.update(scene, &level, 0, 0.5, "07:28", 1936, false);
            assert!(
                session.request_sequence > request_sequence,
                "direct route did not resume"
            );
        }
        assert_eq!(submissions, vec![0, 0]);
    }

    #[test]
    fn direct_reentry_takes_only_the_current_route_generation() {
        let scene = LauncherScene::new(960, 540);
        let level = snapshot();
        let mut session = LauncherCardHomeSession::new(scene, level.clone(), 0, "07:28").unwrap();
        session.update(scene, &level, 0, 0.25, "07:28", 16, false);
        let old_generation = session.navigation_generation;
        session.set_render_ahead_enabled(false);
        session.update(scene, &level, 1, 1.75, "07:28", 32, false);
        assert!(session.try_take_render_ahead(32_000, u64::MAX).is_none());
        session.set_render_ahead_enabled(true);
        assert!(session.navigation_generation > old_generation);
        let deadline = Instant::now() + Duration::from_secs(5);
        let frame = loop {
            if let Some(frame) = session.try_take_render_ahead(32_000, u64::MAX) {
                break frame;
            }
            assert!(
                Instant::now() < deadline,
                "direct route did not produce a current frame"
            );
            std::thread::yield_now();
        };
        assert_eq!(
            frame.request().navigation_generation,
            session.navigation_generation
        );
        assert_eq!(frame.request().render.timestamp_us, 32_000);
        assert_eq!(frame.request().render.frame, session.frame);
        session.recycle_render_ahead(frame);
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
            session.update(scene, &snapshot(), 0, 0.0, "07:28", 16, true);
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
                session.render_ahead.is_some(),
                scene == LauncherScene::new(960, 540)
            );
            session.update(scene, &snapshot(), 0, 0.0, "07:29", 32, true);
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
        );
        session.update(
            LauncherScene::new(960, 540),
            &snapshot(),
            1,
            0.2,
            "21:37",
            10,
            true,
        );
        assert!(session.is_animating());
        assert_eq!(session.frame.selected, 0);
        assert_eq!(session.frame.target, 1);
        assert_eq!(session.render().len(), 960 * 540);
    }

    #[test]
    fn direct_publication_requires_one_compositor_reconciliation() {
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
        );
        session.render();
        session.note_compositor_copied(true);
        assert!(session.compositor_copy_damage(true).is_some());
        let deadline = Instant::now() + Duration::from_secs(2);
        let frame = loop {
            if let Some(frame) = session.try_take_render_ahead(0, u64::MAX) {
                break frame;
            }
            assert!(Instant::now() < deadline, "render-ahead worker timed out");
            std::thread::yield_now();
        };
        session.note_direct_presented(frame);
        assert!(session.compositor_stale());
        assert_eq!(session.compositor_copy_damage(true), None);

        session.render();
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
        );
        let worker = session.render_ahead.as_ref().unwrap().worker_identity();
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
            );
            assert_eq!(
                session.render_ahead.as_ref().unwrap().worker_identity(),
                worker
            );
            assert!(session.presented_frame.is_none());
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
        );
        wait_content(
            &mut session,
            LauncherScene::new(960, 540),
            &data,
            0,
            "22:00",
        );
        assert_ne!(
            session.render_ahead.as_ref().unwrap().worker_identity(),
            worker
        );
    }

    #[test]
    fn settled_clean_home_does_not_sustain_render_ahead_work() {
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
        );
        session.render();
        let submitted_sequence = session.request_sequence;

        session.update(
            LauncherScene::new(960, 540),
            &snapshot,
            0,
            0.0,
            "21:37",
            16,
            true,
        );

        assert_eq!(session.request_sequence, submitted_sequence);
    }

    #[test]
    fn level_change_plays_the_trick_then_settles_on_the_destination() {
        let scene = LauncherScene::new(960, 540);
        let mut session = LauncherCardHomeSession::new(scene, snapshot(), 1, "21:37").unwrap();
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true);
        session.render();
        assert!(!session.is_level_trick_active());
        session.update(scene, &consoles(), 0, 0.0, "21:37", 16, true);
        assert!(session.is_animating());
        assert!(session.is_level_trick_active(), "input is held during it");
        assert!(session.try_take_render_ahead(0, u64::MAX).is_none());
        assert_eq!(session.compositor_copy_damage(true), None);
        // Gather, then hold edge-on until the worker has prepared the level.
        session.update(scene, &consoles(), 0, 0.0, "21:37", 200, true);
        session.render();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut now = 400;
        while session
            .trick
            .as_ref()
            .is_some_and(|trick| trick.deal_delay_ms.is_none())
        {
            assert!(Instant::now() < deadline, "level preparation timed out");
            now += 16;
            session.update(scene, &consoles(), 0, 0.0, "21:37", now, true);
            session.render();
            std::thread::yield_now();
        }
        now += u64::from(LEVEL_TRICK_MILLIS);
        session.update(scene, &consoles(), 0, 0.0, "21:37", now, true);
        session.render();
        assert!(session.trick.is_none());
        assert!(!session.is_level_trick_active());
        let mut expected = prepare(scene, &consoles(), 0, "21:37", &session.fonts);
        expected.render_frame(settled_frame(0));
        assert_eq!(session.render(), expected.pixels());
    }

    #[test]
    fn reduced_motion_changes_level_without_the_trick() {
        let scene = LauncherScene::crt(640, 240);
        let mut session = LauncherCardHomeSession::new(scene, snapshot(), 1, "21:37").unwrap();
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true);
        session.update(scene, &consoles(), 0, 0.0, "21:37", 16, false);
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
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true);
        session.update(scene, &consoles(), 0, 0.0, "21:37", 16, true);
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
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true);
        session.prefetch(vec![consoles()]);
        assert_eq!(session.aside.len(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(&session.aside[0].prepared, Prepared::Building(id) if session.preparation.is_ready(*id))
        {
            assert!(Instant::now() < deadline, "prefetch timed out");
            std::thread::yield_now();
        }
        session.update(scene, &consoles(), 0, 0.0, "21:37", 100, true);
        assert!(session.aside.is_empty(), "the prefetched level was used");
        session.update(
            scene,
            &consoles(),
            0,
            0.0,
            "21:37",
            100 + u64::from(LEVEL_TRICK_EDGE_MILLIS),
            true,
        );
        session.render();
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
        session.update(scene, &root, 1, 1.0, "21:37", 0, true);
        session.update(scene, &consoles, 0, 0.0, "21:37", 16, true);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut now = 16;
        while session.trick.is_some() {
            assert!(Instant::now() < deadline, "level change timed out");
            now += 16;
            session.update(scene, &consoles, 0, 0.0, "21:37", now, true);
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
        session.update(scene, &root, 1, 1.0, "21:38", now + 100, true);
        session.update(
            scene,
            &root,
            1,
            1.0,
            "21:38",
            now + 100 + u64::from(LEVEL_TRICK_EDGE_MILLIS),
            true,
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
        session.update(scene, &snapshot(), 1, 1.0, "21:37", 0, true);
        session.update(scene, &consoles(), 0, 0.0, "21:37", 16, true);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut now = 16;
        while session.trick.is_some() {
            assert!(Instant::now() < deadline, "level change timed out");
            now += 16;
            session.update(scene, &consoles(), 0, 0.0, "21:37", now, true);
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
        session.update(scene, &snapshot(), 1, 1.0, "21:38", now + 100, true);
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
        );
        session.render();
        assert_eq!(session.trick.as_ref().unwrap().deal_delay_ms, Some(0));
    }
}
