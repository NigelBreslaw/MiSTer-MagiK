// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production owner for the custom RGB565 root launcher.

use super::DirtyRect;
use crate::bitmap_font_resource::{
    jersey_25_console_bitmap_font, launcher_bitmap_font, nocive_15_console_bitmap_font,
    spleen_6x12_native_console_bitmap_font, xerxes_10_console_bitmap_font,
};
use crate::launcher_home::{CARD_COUNT, LauncherHomeSnapshot};
use crate::ui_runner::launcher_card_pipeline::{
    CardFrameRequest, CardPipelineCounters, LauncherCardRenderAhead, RenderedCardFrame,
};
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use mister_magik_framebuffer_scenes::bitmap_text::BitmapFont;
use mister_magik_framebuffer_scenes::launcher::{
    LauncherData, LauncherFrameRequest, LauncherScene, LauncherTypography, PreparedLauncher,
};
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowseFrame, BrowsePhase, SPRING_POSITION_UNITS,
};

const CARD_WIDTH: usize = 180;
const CARD_HEIGHT: usize = 252;

const CARD_ASSETS: [&[u8]; CARD_COUNT] = [
    include_bytes!("../../assets/ui/launcher-cards/01_arcade.rgb565"),
    include_bytes!("../../assets/ui/launcher-cards/02_consoles.rgb565"),
    include_bytes!("../../assets/ui/launcher-cards/03_computers.rgb565"),
    include_bytes!("../../assets/ui/launcher-cards/04_handhelds.rgb565"),
    include_bytes!("../../assets/ui/launcher-cards/05_favourites.rgb565"),
    include_bytes!("../../assets/ui/launcher-cards/06_settings.rgb565"),
];

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

pub(super) struct LauncherCardHomeSession {
    scene: LauncherScene,
    snapshot: LauncherHomeSnapshot,
    clock: String,
    artwork: [Vec<Rgb565Pixel>; CARD_COUNT],
    fonts: LauncherFonts,
    prepared: PreparedLauncher,
    render_ahead: Option<LauncherCardRenderAhead>,
    presented_frame: Option<RenderedCardFrame>,
    navigation_generation: u64,
    request_sequence: u64,
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
        snapshot: LauncherHomeSnapshot,
        selected: usize,
        clock: &str,
    ) -> Result<Self, String> {
        let artwork = CARD_ASSETS.map(decode_card_asset);
        let fonts = LauncherFonts::load()?;
        let prepared = prepare(scene, &snapshot, selected, clock, &artwork, &fonts);
        let render_ahead = native_render_ahead(scene, &prepared);
        let frame = settled_frame(selected);
        Ok(Self {
            scene,
            snapshot,
            clock: clock.to_owned(),
            artwork,
            fonts,
            prepared,
            render_ahead,
            presented_frame: None,
            navigation_generation: 1,
            request_sequence: 0,
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

    pub(super) fn set_inactive(&mut self) {
        self.invalidate_compositor();
        self.active = false;
        self.release_presented_frame();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn update(
        &mut self,
        scene: LauncherScene,
        snapshot: LauncherHomeSnapshot,
        selected: usize,
        visual_index: f32,
        clock: &str,
        now_ms: u64,
    ) {
        let selected = selected.min(CARD_COUNT - 1);
        let mut previous_frame = self.frame;
        if !self.active {
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
        );
        self.last_visual_index = visual_index;
        self.frame_timestamp_us = now_ms.saturating_mul(1_000);
        if navigation_identity_changed(previous_frame, self.frame) {
            self.bump_navigation_generation();
            self.content_dirty = true;
        }

        let faces_changed = self.scene != scene || self.snapshot.cards != snapshot.cards;
        if faces_changed || self.snapshot != snapshot || self.clock != clock {
            let preparation_started = self.measure_preparation.then(std::time::Instant::now);
            self.release_presented_frame();
            self.scene = scene;
            self.snapshot = snapshot;
            self.clock.clear();
            self.clock.push_str(clock);
            self.content_generation = self.content_generation.wrapping_add(1).max(1);
            if faces_changed {
                if let Some(mut old) = self.render_ahead.take() {
                    old.stop();
                    self.retired_pipeline_counters.add_assign(old.counters());
                }
                self.prepared = prepare(
                    scene,
                    &self.snapshot,
                    self.frame.selected,
                    &self.clock,
                    &self.artwork,
                    &self.fonts,
                );
                self.render_ahead = native_render_ahead(scene, &self.prepared);
            } else {
                self.prepared.refresh_chrome(
                    LauncherData {
                        cards: &self.snapshot.cards,
                        selected: self.frame.selected,
                        library_games: self.snapshot.library_games,
                        collections: self.snapshot.collections,
                        favourites: self.snapshot.favourites,
                        clock: &self.clock,
                    },
                    Some(self.fonts.typography()),
                );
            }
            if let Some(pipeline) = self.render_ahead.as_ref() {
                pipeline.invalidate_content_generation(self.content_generation);
            }
            self.content_dirty = true;
            self.preparation_measurement = preparation_started.map(|start| {
                (
                    faces_changed,
                    faces_changed && self.render_ahead.is_some(),
                    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                )
            });
        }
        if self.active && (self.content_dirty || self.is_animating()) {
            self.submit_render_ahead();
        }
    }

    pub(super) fn is_animating(&self) -> bool {
        self.active && self.frame.phase != BrowsePhase::Settled
    }

    pub(super) fn needs_render(&self) -> bool {
        self.active
            && (self.content_dirty
                || self.is_animating()
                || self
                    .render_ahead
                    .as_ref()
                    .is_some_and(LauncherCardRenderAhead::has_ready))
    }

    pub(super) fn render(&mut self) -> &[Rgb565Pixel] {
        self.release_presented_frame();
        self.prepared.render_frame(self.frame);
        self.content_dirty = false;
        self.compositor_stale = false;
        self.prepared.pixels()
    }

    pub(super) const fn content_generation(&self) -> u64 {
        self.content_generation
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
        (motion_only
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
        let mut current = self.retired_pipeline_counters;
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
) -> BrowseFrame {
    let position = if visual_index.is_finite() {
        visual_index
    } else {
        selected as f32
    };
    let card_at = |index: i64| index.rem_euclid(CARD_COUNT as i64) as usize;
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

fn prepare(
    scene: LauncherScene,
    snapshot: &LauncherHomeSnapshot,
    selected: usize,
    clock: &str,
    artwork: &[Vec<Rgb565Pixel>; CARD_COUNT],
    fonts: &LauncherFonts,
) -> PreparedLauncher {
    let artwork: [&[Rgb565Pixel]; CARD_COUNT] =
        std::array::from_fn(|index| artwork[index].as_slice());
    let data = LauncherData {
        cards: &snapshot.cards,
        selected,
        library_games: snapshot.library_games,
        collections: snapshot.collections,
        favourites: snapshot.favourites,
        clock,
    };
    if scene.uses_responsive_layout() {
        scene
            .prepare_initial_with_rgb888_artwork_and_typography(
                data,
                &CARD_RGB888,
                fonts.typography(),
            )
            .finish()
    } else {
        scene
            .prepare_initial_with_artwork_and_typography(data, &artwork, fonts.typography())
            .finish()
    }
}

fn decode_card_asset(bytes: &[u8]) -> Vec<Rgb565Pixel> {
    assert_eq!(bytes.len(), CARD_WIDTH * CARD_HEIGHT * 2);
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| Rgb565Pixel(u16::from_le_bytes(*pair)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher_home::LauncherHomeCounts;
    use std::time::{Duration, Instant};

    fn snapshot() -> LauncherHomeSnapshot {
        LauncherHomeSnapshot::from_counts(LauncherHomeCounts {
            arcade: 1,
            consoles: 2,
            computers: 3,
            handhelds: 4,
            favourites: 5,
            collections: 4,
        })
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
            session.update(scene, snapshot(), 0, 0.0, "07:28", 16);
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
            session.update(scene, snapshot(), 0, 0.0, "07:29", 32);
            let expected = prepare(
                scene,
                &snapshot(),
                0,
                "07:29",
                &session.artwork,
                &session.fonts,
            );
            assert_eq!(session.render(), expected.pixels());
        }
    }

    #[test]
    fn card_frames_follow_unbounded_position_through_both_wraps() {
        let settled = settled_frame(0);
        assert_eq!(
            browse_frame_from_position(0, 0.0, 0.0, settled).phase,
            BrowsePhase::Settled
        );
        let right = browse_frame_from_position(0, 5.25, 5.1, settled_frame(5));
        assert_eq!((right.selected, right.target), (5, 0));
        assert_eq!(right.direction, Some(BrowseDirection::Right));
        assert_eq!(right.progress_millis, SPRING_POSITION_UNITS / 4);
        let next = browse_frame_from_position(1, 6.25, 6.1, settled);
        assert_eq!((next.selected, next.target), (0, 1));
        let left = browse_frame_from_position(5, -0.25, -0.1, settled);
        assert_eq!((left.selected, left.target), (0, 5));
        assert_eq!(left.direction, Some(BrowseDirection::Left));
        assert_eq!(left.progress_millis, SPRING_POSITION_UNITS / 4);
        assert_eq!(
            browse_frame_from_position(5, -1.0, -0.9, left).phase,
            BrowsePhase::Settled
        );
    }

    #[test]
    fn reversal_unwinds_the_same_two_card_rotations() {
        let forward = browse_frame_from_position(2, 1.30, 1.20, settled_frame(1));
        let reverse = browse_frame_from_position(1, 1.25, 1.30, forward);
        assert_eq!((forward.selected, forward.target), (1, 2));
        assert_eq!((reverse.selected, reverse.target), (1, 2));
        assert_eq!(reverse.direction, Some(BrowseDirection::Right));
        assert!(reverse.progress_millis < forward.progress_millis);

        let moving = browse_frame_from_position(1, 1.000_001, 1.01, reverse);
        assert_eq!(moving.progress_millis, 1);
        assert_eq!(
            browse_frame_from_position(1, 1.0, 1.000_001, moving).phase,
            BrowsePhase::Settled
        );
    }

    #[test]
    fn root_session_renders_exact_geometry_and_animates_toward_navigation() {
        let mut session =
            LauncherCardHomeSession::new(LauncherScene::new(960, 540), snapshot(), 0, "21:37")
                .unwrap();
        session.update(LauncherScene::new(960, 540), snapshot(), 0, 0.0, "21:37", 0);
        session.update(
            LauncherScene::new(960, 540),
            snapshot(),
            1,
            0.2,
            "21:37",
            10,
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
        session.update(LauncherScene::new(960, 540), snapshot(), 0, 0.0, "21:37", 0);
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
        session.update(LauncherScene::new(960, 540), snapshot(), 0, 0.0, "12:34", 0);
        assert_eq!(session.compositor_copy_damage(true), None);
        session.render();
        session.note_compositor_copied(true);
        let rect = session.compositor_copy_damage(true).unwrap();
        assert_eq!((rect.x1 - rect.x0) * (rect.y1 - rect.y0), 239250);
        session.update(
            LauncherScene::new(960, 540),
            snapshot(),
            1,
            1.0,
            "12:34",
            16,
        );
        assert_eq!(session.compositor_copy_damage(true), Some(rect));
        session.update(
            LauncherScene::new(960, 540),
            snapshot(),
            1,
            1.0,
            "12:35",
            32,
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
            data.clone(),
            0,
            0.0,
            "21:37",
            0,
        );
        let worker = session.render_ahead.as_ref().unwrap().worker_identity();
        for clock in ["21:38", "22:00"] {
            data.collections += 1;
            data.library_games += 123;
            session.update(
                LauncherScene::new(960, 540),
                data.clone(),
                0,
                0.0,
                clock,
                16,
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
                &session.artwork,
                &session.fonts,
            );
            reference.render_frame(session.frame);
            assert_eq!(session.render(), reference.pixels());
        }
        data.cards[0].games = Some(999);
        session.update(LauncherScene::new(960, 540), data, 0, 0.0, "22:00", 32);
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
            snapshot.clone(),
            0,
            0.0,
            "21:37",
            0,
        );
        session.render();
        let submitted_sequence = session.request_sequence;

        session.update(LauncherScene::new(960, 540), snapshot, 0, 0.0, "21:37", 16);

        assert_eq!(session.request_sequence, submitted_sequence);
    }
}
