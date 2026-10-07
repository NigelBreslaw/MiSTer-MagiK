// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hierarchy level change, the "card trick". One continuous motion: the chosen
//! card turns and travels between the two selected slots for the whole 920 ms.
//! The source gathers until 460 ms; the destination deals after it. Both use
//! the same native hero pose and the existing projection and reflections.
//!
//! The two halves need only their own level. The level being left renders the
//! gather; the level being entered renders the deal. A consumer can therefore
//! prepare the next level off the UI thread while the gather plays, and hold
//! every card edge-on at [`LEVEL_TRICK_EDGE_MILLIS`] until it is ready.
//!
//! Frames are pure functions of elapsed time, so every frame is reproducible
//! in tests. Rendering reuses the carousel's projection, occlusion and
//! reflection path and allocates nothing.
use super::*;
use crate::launcher_flip::Pose;
use crate::launcher_parallel::{ParallelFrameTiming, ParallelLauncherRenderer};

/// Complete duration of a level change.
pub const LEVEL_TRICK_MILLIS: u32 = 920;
/// Every card is edge-on at this moment; the level swaps here.
pub const LEVEL_TRICK_EDGE_MILLIS: u32 = 460;
const EDGE_MILLIS: u32 = LEVEL_TRICK_EDGE_MILLIS;
const DEAL_MILLIS: u32 = 360;
const DEAL_STAGGER_MILLIS: u32 = 20;
/// Header and group summary fade out, then the next level's fade in.
const CHROME_OUT_MILLIS: u32 = 260;
const CHROME_IN_AT_MILLIS: u32 = LEVEL_TRICK_MILLIS * 55 / 100;
const CHROME_IN_MILLIS: u32 = 360;
/// The cards gather behind the chosen card at 90% of its size.
const BEHIND_SCALE: i64 = GEOMETRY_ONE * 9 / 10;
const LIFT_SCALE: i64 = GEOMETRY_ONE * 4 / 100;
const EDGE_ON: i64 = GEOMETRY_ONE / 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LevelChange {
    Descend,
    Ascend,
}
impl LevelChange {
    const fn spin(self) -> i64 {
        match self {
            Self::Descend => 1,
            Self::Ascend => -1,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct TrickCard {
    index: usize,
    detail: bool,
    pose: Pose,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct TrickPlan {
    items: [Option<TrickCard>; CAROUSEL_CAPACITY],
}

impl TrickPlan {
    pub(super) fn with_faces(self, faces: &[Arc<CardFaces>]) -> CarouselPlan<'_> {
        CarouselPlan {
            items: self.items.map(|item| {
                item.map(|item| CarouselItem {
                    face: if item.detail {
                        &faces[item.index].detail
                    } else {
                        &faces[item.index].compact
                    },
                    blend: None,
                    pose: item.pose,
                })
            }),
            row: false,
        }
    }
}

pub(super) struct ChromeSpan {
    start: usize,
    end: usize,
    title: bool,
}

impl PreparedLauncher {
    pub fn render_level_gather_to(
        &mut self,
        selected: usize,
        change: LevelChange,
        elapsed_millis: u32,
        destination: CardSlot,
    ) {
        if let Some(plan) = self.level_gather_plan(selected, change, elapsed_millis, destination) {
            self.draw_trick_plan(plan);
        } else {
            self.render_frame(settled(selected));
        }
    }

    pub fn render_level_deal_from(
        &mut self,
        selected: usize,
        change: LevelChange,
        elapsed_millis: u32,
        source: CardSlot,
    ) {
        if let Some(plan) = self.level_deal_plan(selected, change, elapsed_millis, source) {
            self.draw_trick_plan(plan);
        } else {
            self.render_frame(settled(selected));
        }
    }

    pub fn render_level_gather_to_parallel(
        &mut self,
        request: LauncherFrameRequest,
        change: LevelChange,
        elapsed_millis: u32,
        destination: CardSlot,
        renderer: &mut ParallelLauncherRenderer,
    ) -> Result<ParallelFrameTiming, String> {
        let plan =
            self.level_gather_plan(request.frame.selected, change, elapsed_millis, destination);
        self.render_parallel_trick_plan(renderer, request, plan)
    }

    pub fn render_level_deal_from_parallel(
        &mut self,
        request: LauncherFrameRequest,
        change: LevelChange,
        elapsed_millis: u32,
        source: CardSlot,
        renderer: &mut ParallelLauncherRenderer,
    ) -> Result<ParallelFrameTiming, String> {
        let plan = self.level_deal_plan(request.frame.selected, change, elapsed_millis, source);
        self.render_parallel_trick_plan(renderer, request, plan)
    }

    fn render_parallel_trick_plan(
        &mut self,
        renderer: &mut ParallelLauncherRenderer,
        request: LauncherFrameRequest,
        plan: Option<TrickPlan>,
    ) -> Result<ParallelFrameTiming, String> {
        if !self.supports_parallel() {
            return Err("parallel cards require native or responsive geometry".into());
        }
        if let Some(plan) = plan {
            let mut preparer = self.frame_preparer();
            preparer.trick = Some(plan);
            renderer.render(&preparer, request, &mut self.logical)
        } else {
            self.render_parallel_frame(renderer, request)
        }
    }

    /// Prepare only the card pixels for a future virtual-clock step. Chrome,
    /// destination readiness and navigation remain owned by the current frame.
    pub fn level_frame_preparer(
        &self,
        selected: usize,
        change: LevelChange,
        elapsed_millis: u32,
        slot: CardSlot,
        gather: bool,
    ) -> LauncherFramePreparer {
        let mut preparer = self.frame_preparer();
        preparer.trick = if gather {
            self.level_gather_pose(selected, change, elapsed_millis, slot)
        } else {
            self.level_deal_pose(selected, change, elapsed_millis, slot)
        };
        preparer
    }

    fn level_gather_plan(
        &mut self,
        selected: usize,
        change: LevelChange,
        elapsed_millis: u32,
        destination: CardSlot,
    ) -> Option<TrickPlan> {
        let t = elapsed_millis.min(EDGE_MILLIS);
        if t == 0 {
            self.restore_chrome();
        } else {
            self.fade_level_chrome(
                GEOMETRY_ONE - ease_in_out_cubic(window(t, 0, CHROME_OUT_MILLIS)),
            );
        }
        self.level_gather_pose(selected, change, t, destination)
    }

    fn level_gather_pose(
        &self,
        selected: usize,
        change: LevelChange,
        elapsed_millis: u32,
        destination: CardSlot,
    ) -> Option<TrickPlan> {
        let t = elapsed_millis.min(EDGE_MILLIS);
        if t == 0 {
            return None;
        }
        let hero = hero_pose(self.slot_zero(), destination, change, t);
        let progress = window(t, 0, EDGE_MILLIS);
        let gather = ease_in_out_cubic(progress);
        let mut behind = scaled_pose(hero, BEHIND_SCALE);
        behind.angle = 0;
        behind.brightness = 64;
        let faces = &self.faces;
        let nested = faces.first().is_some_and(|f| f.slides);
        let relatives: &[isize] = if nested {
            &[4, 3, 2, 1]
        } else {
            &[2, -2, 1, -1]
        };
        let mut items = [None; CAROUSEL_CAPACITY];
        let mut count = 0;
        for &relative in relatives {
            if nested && relative as usize >= faces.len().min(5) {
                continue;
            }
            let Some(index) = neighbour(faces, self.cyclic, selected, relative) else {
                continue;
            };
            if t == EDGE_MILLIS {
                continue;
            }
            let rest = self.resting_pose(relative);
            let mut pose = lerp_pose(rest, behind, gather);
            pose.angle = rest.angle * (GEOMETRY_ONE - gather) / GEOMETRY_ONE
                + relative.signum() as i64 * EDGE_ON * progress * progress
                    / GEOMETRY_ONE
                    / GEOMETRY_ONE;
            pose.brightness = pose.brightness * ((EDGE_MILLIS - t).min(20) * 256 / 20) / 256;
            items[count] = Some(TrickCard {
                index,
                detail: false,
                pose,
            });
            count += 1;
        }
        if faces.get(selected).is_some() {
            items[count] = Some(TrickCard {
                index: selected,
                detail: true,
                pose: hero,
            });
        }
        Some(TrickPlan { items })
    }

    fn level_deal_plan(
        &mut self,
        selected: usize,
        change: LevelChange,
        elapsed_millis: u32,
        source: CardSlot,
    ) -> Option<TrickPlan> {
        let t = elapsed_millis.clamp(EDGE_MILLIS, LEVEL_TRICK_MILLIS);
        if t == LEVEL_TRICK_MILLIS {
            self.restore_chrome();
        } else {
            self.fade_level_chrome(ease_out_quart(window(
                t,
                CHROME_IN_AT_MILLIS,
                CHROME_IN_MILLIS,
            )));
        }
        self.level_deal_pose(selected, change, t, source)
    }

    fn level_deal_pose(
        &self,
        selected: usize,
        change: LevelChange,
        elapsed_millis: u32,
        source: CardSlot,
    ) -> Option<TrickPlan> {
        let t = elapsed_millis.clamp(EDGE_MILLIS, LEVEL_TRICK_MILLIS);
        if t == LEVEL_TRICK_MILLIS {
            return None;
        }
        let hero = hero_pose(source, self.slot_zero(), change, t);
        let mut behind = scaled_pose(hero, BEHIND_SCALE);
        behind.angle = 0;
        behind.brightness = 64;
        let faces = &self.faces;
        let nested = faces.first().is_some_and(|f| f.slides);
        let relatives: &[isize] = if nested {
            &[4, 3, 2, 1]
        } else {
            &[2, -2, 1, -1]
        };
        let mut items = [None; CAROUSEL_CAPACITY];
        let mut count = 0;
        for &relative in relatives {
            if nested && relative as usize >= faces.len().min(5) {
                continue;
            }
            let Some(index) = neighbour(faces, self.cyclic, selected, relative) else {
                continue;
            };
            let order = if nested {
                relative as usize - 1
            } else {
                (relative.unsigned_abs() - 1) * 2 + usize::from(relative < 0)
            };
            let dealt = ease_out_quart(window(
                t,
                EDGE_MILLIS + order.min(5) as u32 * DEAL_STAGGER_MILLIS,
                DEAL_MILLIS,
            ));
            if dealt == 0 {
                continue;
            }
            let rest = self.resting_pose(relative);
            let mut pose = lerp_pose(behind, rest, dealt);
            pose.angle = rest.angle * dealt / GEOMETRY_ONE
                - relative.signum() as i64 * EDGE_ON * (GEOMETRY_ONE - dealt) / GEOMETRY_ONE;
            items[count] = Some(TrickCard {
                index,
                detail: false,
                pose,
            });
            count += 1;
        }
        if faces.get(selected).is_some() {
            items[count] = Some(TrickCard {
                index: selected,
                detail: true,
                pose: hero,
            });
        }
        Some(TrickPlan { items })
    }

    /// The target breadcrumb swaps at 45% of the timeline while both panels are dim.
    pub fn render_transition_title_from(&mut self, target: &Self, elapsed_millis: u32) {
        if self.scene != target.scene || elapsed_millis < LEVEL_TRICK_MILLIS * 45 / 100 {
            return;
        }
        let k = ease_out_quart(window(
            elapsed_millis,
            CHROME_IN_AT_MILLIS,
            CHROME_IN_MILLIS,
        ));
        let alpha = 76 + (180 * k / GEOMETRY_ONE) as u32;
        let width = if self.responsive.is_some() {
            self.scene.width
        } else {
            LOGICAL_WIDTH
        };
        let (x0, y0, x1, y1) = self.responsive.map_or((26, 0, 826, 76), |l| l.title_rect());
        for y in y0..y1 {
            for x in x0..x1 {
                let at = y * width + x;
                self.logical[at] = Rgb565Pixel(scale_rgb565(target.chrome[at].0, alpha));
            }
        }
        self.level_foreign_title = true;
        self.fit_output();
    }

    pub fn restore_chrome(&mut self) {
        self.logical.copy_from_slice(&self.chrome);
        self.level_chrome_alpha = None;
        self.level_foreign_title = false;
        self.fit_output();
    }

    fn draw_trick_plan(&mut self, plan: TrickPlan) {
        let faces = &self.faces;
        let plan = plan.with_faces(faces);
        // Both halves already use native poses. Mapping again would move the hero at the swap.
        if let Some(layout) = self.responsive {
            layout.clear_carousel(&mut self.logical);
            layout.draw_plan(&mut self.logical, &plan, &mut self.flip_columns);
            return;
        }
        let row = CardRow::canvas(true);
        clear_card_rows(&mut self.logical, LOGICAL_WIDTH, row);
        draw_card_strips(
            &mut self.logical,
            LOGICAL_WIDTH,
            row,
            &plan,
            &mut self.flip_columns,
        );
        self.fit_output();
    }
    fn level_chrome_regions(&self) -> [((usize, usize, usize, usize), bool); 3] {
        let width = if self.responsive.is_some() {
            self.scene.width
        } else {
            LOGICAL_WIDTH
        };
        let (summary, panel) = if let Some(layout) = self.responsive {
            let (_, title_end) = layout.level_chrome_rows()[0];
            let (_, _, _, rule_y) = layout.title_rect();
            let (y0, y1) = layout.level_chrome_rows()[1];
            ((0, rule_y + 1, width, title_end), (0, y0, width, y1))
        } else {
            ((0, 77, 265, 500), (296, 77, LOGICAL_WIDTH, 120))
        };
        let title = self.responsive.map_or((26, 0, 826, 76), |l| l.title_rect());
        [(summary, false), (panel, false), (title, true)]
    }

    pub(super) fn rebuild_level_chrome_spans(&mut self) {
        self.level_chrome_spans.clear();
        self.level_chrome_alpha = None;
        self.level_foreign_title = false;
        let width = if self.responsive.is_some() {
            self.scene.width
        } else {
            LOGICAL_WIDTH
        };
        for ((x0, y0, x1, y1), title) in self.level_chrome_regions() {
            for y in y0..y1 {
                let start = y * width + x0;
                let row = &self.chrome[start..y * width + x1];
                if let Some(first) = row.iter().position(|p| p.0 != BACKGROUND) {
                    let last = row.iter().rposition(|p| p.0 != BACKGROUND).unwrap();
                    self.level_chrome_spans.push(ChromeSpan {
                        start: start + first,
                        end: start + last + 1,
                        title,
                    });
                }
            }
        }
    }

    /// Rows that can change during a level trick, excluding the carousel.
    /// Copy the full title region: a wider target breadcrumb and its later
    /// clearing can change pixels that are black in the source title.
    pub fn level_chrome_copy_spans(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let width = self.responsive.map_or(LOGICAL_WIDTH, |_| self.scene.width);
        let ((x0, y0, x1, y1), _) = self.level_chrome_regions()[2];
        (y0..y1)
            .map(move |y| (y * width + x0, y * width + x1))
            .chain(
                self.level_chrome_spans
                    .iter()
                    .filter(|s| !s.title)
                    .map(|s| (s.start, s.end)),
            )
    }

    fn fade_level_chrome(&mut self, alpha: i64) {
        let alpha = (alpha * 256 / GEOMETRY_ONE) as u32;
        let foreign_title = std::mem::take(&mut self.level_foreign_title);
        if self.level_chrome_alpha == Some(alpha) && !foreign_title {
            return;
        }
        // A target breadcrumb may occupy pixels that are black in our source.
        // Clear that overlay before restoring this level's sparse title rows.
        if foreign_title {
            let ((x0, y0, x1, y1), _) = self.level_chrome_regions()[2];
            let width = if self.responsive.is_some() {
                self.scene.width
            } else {
                LOGICAL_WIDTH
            };
            for y in y0..y1 {
                self.logical[y * width + x0..y * width + x1].fill(Rgb565Pixel(BACKGROUND));
            }
        }
        for span in &self.level_chrome_spans {
            if self.level_chrome_alpha == Some(alpha) && !span.title {
                continue;
            }
            let alpha = if span.title {
                76 + 180 * alpha / 256
            } else {
                alpha
            };
            let output = &mut self.logical[span.start..span.end];
            let source = &self.chrome[span.start..span.end];
            if alpha == 0 {
                output.fill(Rgb565Pixel(0));
            } else if alpha >= 256 {
                output.copy_from_slice(source);
            } else {
                for (out, pixel) in output.iter_mut().zip(source) {
                    *out = Rgb565Pixel(scale_rgb565(pixel.0, alpha));
                }
            }
        }
        self.level_chrome_alpha = Some(alpha);
    }
}

fn neighbour(
    faces: &[Arc<CardFaces>],
    cyclic: bool,
    selected: usize,
    relative: isize,
) -> Option<usize> {
    let count = faces.len() as isize;
    if count == 0 {
        return None;
    }
    let position = selected as isize + relative;
    if cyclic {
        // A cycling level always has enough cards to fill the carousel.
        Some(position.rem_euclid(count) as usize)
    } else {
        (0..count).contains(&position).then_some(position as usize)
    }
}

fn settled(selected: usize) -> BrowseFrame {
    BrowseFrame {
        selected,
        target: selected,
        phase: crate::launcher_navigation::BrowsePhase::Settled,
        direction: None,
        progress_millis: 0,
        duration_millis: 0,
    }
}

fn scaled_pose(mut pose: Pose, scale: i64) -> Pose {
    let width = pose.width * scale / GEOMETRY_ONE;
    let height = pose.height * scale / GEOMETRY_ONE;
    pose.x += (pose.width - width) / 2;
    pose.top += (pose.height - height) / 2;
    pose.width = width;
    pose.height = height;
    pose
}

fn hero_pose(from: CardSlot, to: CardSlot, change: LevelChange, t: u32) -> Pose {
    let k = ease_in_out_cubic(window(t, 0, LEVEL_TRICK_MILLIS));
    let scale =
        GEOMETRY_ONE + LIFT_SCALE * sin_half_turn(window(t, 0, LEVEL_TRICK_MILLIS)) / GEOMETRY_ONE;
    let mut pose = scaled_pose(lerp_pose(from.pose, to.pose, k), scale);
    pose.angle = change.spin() * k;
    pose.clip = (
        from.pose.clip.0.min(to.pose.clip.0),
        from.pose.clip.1.max(to.pose.clip.1),
    );
    pose.body_clip = pose.clip;
    pose
}

fn lerp_pose(a: Pose, b: Pose, k: i64) -> Pose {
    let lerp = |from: i64, to: i64| from + (to - from) * k / GEOMETRY_ONE;
    let width = lerp(a.width, b.width);
    let height = lerp(a.height, b.height);
    let centre_x = lerp(a.x + a.width / 2, b.x + b.width / 2);
    let centre_y = lerp(a.top + a.height / 2, b.top + b.height / 2);
    Pose {
        x: centre_x - width / 2,
        top: centre_y - height / 2,
        width,
        height,
        angle: lerp(a.angle, b.angle),
        brightness: lerp(i64::from(a.brightness), i64::from(b.brightness)) as u32,
        ..a
    }
}

/// Q16 progress through `[at, at + duration)`.
fn window(t: u32, at: u32, duration: u32) -> i64 {
    (i64::from(t.saturating_sub(at)) * GEOMETRY_ONE / i64::from(duration.max(1))).min(GEOMETRY_ONE)
}

fn ease_in_out_cubic(p: i64) -> i64 {
    if p < GEOMETRY_ONE / 2 {
        4 * p * p / GEOMETRY_ONE * p / GEOMETRY_ONE
    } else {
        let q = 2 * (GEOMETRY_ONE - p);
        GEOMETRY_ONE - q * q / GEOMETRY_ONE * q / GEOMETRY_ONE / 2
    }
}

fn ease_out_quart(p: i64) -> i64 {
    let q = GEOMETRY_ONE - p;
    let q2 = q * q / GEOMETRY_ONE;
    GEOMETRY_ONE - q2 * q2 / GEOMETRY_ONE
}

/// sin(pi * p) for Q16 progress p.
fn sin_half_turn(p: i64) -> i64 {
    crate::launcher_flip::sin_cos(p).0
}

fn scale_rgb565(pixel: u16, alpha: u32) -> u16 {
    if alpha >= 256 {
        return pixel;
    }
    let r = (u32::from(pixel >> 11) * alpha) >> 8;
    let g = (u32::from((pixel >> 5) & 63) * alpha) >> 8;
    let b = (u32::from(pixel & 31) * alpha) >> 8;
    ((r << 11) | (g << 5) | b) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher_navigation::BrowsePhase;

    fn cards(count: usize) -> Vec<LauncherCard<'static>> {
        const NAMES: [&str; 6] = ["ATARI", "SEGA", "SONY", "NINTENDO", "NEC", "SNK"];
        (0..count)
            .map(|index| LauncherCard {
                id: LauncherCardId::Consoles,
                name: NAMES[index],
                games: Some(100 + index as u32),
                colour: 0x2a7f,
            })
            .collect()
    }

    fn level<'a>(
        cards: &'a [LauncherCard<'a>],
        selected: usize,
        path: &'a [&'a str],
    ) -> LauncherData<'a> {
        LauncherData {
            cards,
            selected,
            library_games: 0,
            collections: 0,
            favourites: 0,
            clock: "12:00",
            level: LauncherLevel::Nested(NestedLevel {
                path,
                games: 600,
                children: cards.len() as u32,
                children_label: "SYSTEMS",
                detail: None,
                accent: 0x2a7f,
            }),
        }
    }

    fn settled(selected: usize) -> BrowseFrame {
        BrowseFrame {
            selected,
            target: selected,
            phase: BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 0,
        }
    }

    #[test]
    fn travelling_hero_is_halfway_and_edge_on_at_460_ms_on_native_routes() {
        for scene in [LauncherScene::new(960, 540), LauncherScene::crt(640, 240)] {
            let from = scene.slot_zero(false);
            let to = scene.slot_zero(true);
            let centre = |p: Pose| p.x + p.width / 2;
            let mut previous = centre(from.pose);
            for t in 0..=LEVEL_TRICK_MILLIS {
                let pose = hero_pose(from, to, LevelChange::Descend, t);
                assert!(centre(pose) <= previous);
                assert!(previous - centre(pose) < 2 * GEOMETRY_ONE);
                previous = centre(pose);
            }
            let middle = hero_pose(from, to, LevelChange::Descend, 460);
            assert_eq!(centre(middle), (centre(from.pose) + centre(to.pose)) / 2);
            assert_eq!(middle.angle, EDGE_ON);
            assert_eq!(
                centre(hero_pose(from, to, LevelChange::Descend, 920)),
                centre(to.pose)
            );
            assert_eq!(
                hero_pose(to, from, LevelChange::Ascend, 460).angle,
                -EDGE_ON
            );
        }
    }

    /// Every output draws a level change in two bands, equal to the serial
    /// render: gathering the source level and dealing the destination.
    #[test]
    fn level_change_bands_match_the_serial_render_on_every_output() {
        use crate::launcher_parallel::ParallelLauncherRenderer;
        let from_cards = cards(6);
        let to_cards = cards(4);
        for scene in [
            LauncherScene::new(960, 540),
            LauncherScene::crt(640, 240),
            LauncherScene::crt(640, 288),
            LauncherScene::crt(640, 480),
            LauncherScene::crt(480, 640),
            LauncherScene::new(540, 960),
        ] {
            let from_data = level(&from_cards, 3, &["CONSOLES"]);
            let to_data = level(&to_cards, 0, &["CONSOLES", "NINTENDO"]);
            let mut serial_from = scene.prepare(from_data);
            let mut parallel_from = scene.prepare(from_data);
            let mut serial_to = scene.prepare(to_data);
            let mut parallel_to = scene.prepare(to_data);
            let mut renderer =
                ParallelLauncherRenderer::new(parallel_from.frame_preparer(), None, None)
                    .expect("renderer");
            let mut generation = 0;
            let mut request = |selected: usize| {
                generation += 1;
                LauncherFrameRequest {
                    frame: settled(selected),
                    timestamp_us: 0,
                    generation,
                }
            };
            for t in [1, EDGE_MILLIS / 2, EDGE_MILLIS - 1] {
                serial_from.render_level_gather_to(
                    3,
                    LevelChange::Descend,
                    t,
                    serial_from.slot_zero(),
                );
                parallel_from
                    .render_level_gather_to_parallel(
                        request(3),
                        LevelChange::Descend,
                        t,
                        parallel_from.slot_zero(),
                        &mut renderer,
                    )
                    .unwrap();
                assert!(
                    parallel_from.pixels() == serial_from.pixels(),
                    "{scene:?} gather {t}"
                );
            }
            for t in [
                EDGE_MILLIS + 100,
                LEVEL_TRICK_MILLIS / 2,
                LEVEL_TRICK_MILLIS - 100,
            ] {
                serial_to.render_level_deal_from(0, LevelChange::Descend, t, serial_to.slot_zero());
                parallel_to
                    .render_level_deal_from_parallel(
                        request(0),
                        LevelChange::Descend,
                        t,
                        parallel_to.slot_zero(),
                        &mut renderer,
                    )
                    .unwrap();
                assert!(
                    parallel_to.pixels() == serial_to.pixels(),
                    "{scene:?} deal {t}"
                );
            }
        }
    }

    #[test]
    fn trick_starts_on_the_source_and_ends_on_the_destination() {
        for scene in [LauncherScene::new(960, 540), LauncherScene::crt(640, 240)] {
            let from_cards = cards(6);
            let to_cards = cards(4);
            let mut from = scene.prepare(level(&from_cards, 3, &["CONSOLES"]));
            let mut expected_from = scene.prepare(level(&from_cards, 3, &["CONSOLES"]));
            expected_from.render_frame(settled(3));
            from.render_level_gather_to(3, LevelChange::Descend, 0, from.slot_zero());
            assert!(
                from.pixels() == expected_from.pixels(),
                "{scene:?} first frame"
            );

            let mut to = scene.prepare(level(&to_cards, 0, &["CONSOLES", "NINTENDO"]));
            let mut expected_to = scene.prepare(level(&to_cards, 0, &["CONSOLES", "NINTENDO"]));
            expected_to.render_frame(settled(0));
            to.render_level_deal_from(0, LevelChange::Descend, LEVEL_TRICK_MILLIS, to.slot_zero());
            assert!(to.pixels() == expected_to.pixels(), "{scene:?} final frame");
        }
    }

    #[test]
    fn both_halves_meet_at_an_all_edge_on_hold_frame() {
        let from_cards = cards(6);
        let to_cards = cards(4);
        let scene = LauncherScene::new(960, 540);
        let mut from = scene.prepare(level(&from_cards, 3, &["CONSOLES"]));
        let mut to = scene.prepare(level(&to_cards, 0, &["CONSOLES", "NINTENDO"]));
        let widest_lit_row = |pixels: &[Rgb565Pixel]| {
            pixels[120 * 960..495 * 960]
                .chunks(960)
                .map(|row| row[296..934].iter().filter(|p| p.0 != 0).count())
                .max()
                .unwrap()
        };
        from.render_level_gather_to(3, LevelChange::Descend, EDGE_MILLIS, to.slot_zero());
        from.render_transition_title_from(&to, EDGE_MILLIS);
        assert!(widest_lit_row(from.pixels()) < 16);
        // The destination can hold here for as long as preparation takes.
        to.render_level_deal_from(0, LevelChange::Descend, 0, to.slot_zero());
        assert!(widest_lit_row(to.pixels()) < 16);
        // The target breadcrumb is dim; the clock and rule stay visible.
        for y in 0..76 {
            assert!(
                from.pixels()[y * 960 + 26..y * 960 + 826]
                    == to.pixels()[y * 960 + 26..y * 960 + 826]
            );
            assert!(
                from.pixels()[y * 960 + 874..y * 960 + 934]
                    == from.chrome[y * 960 + 874..y * 960 + 934]
            );
        }
        assert!(from.pixels()[76 * 960..77 * 960] == from.chrome[76 * 960..77 * 960]);
    }

    #[test]
    fn sparse_copy_spans_cover_tricks_and_interrupted_foreign_titles() {
        let cards = cards(6);
        let scene = LauncherScene::new(960, 540);
        for root in [false, true] {
            let mut data = level(&cards, 3, &["CONSOLES"]);
            if root {
                data.level = LauncherLevel::Root;
            }
            let mut from = scene.prepare(data);
            let target = scene.prepare(level(
                &cards,
                0,
                &["CONSOLES", "NINTENDO ENTERTAINMENT SYSTEM"],
            ));
            for change in [LevelChange::Descend, LevelChange::Ascend] {
                let mut slots = [from.pixels().to_vec(), from.pixels().to_vec()];
                for (frame, t) in [
                    0, 1, 150, 260, 414, 459, 75, 460, 461, 600, 750, 866, 899, 919, 920,
                ]
                .into_iter()
                .enumerate()
                {
                    if t <= EDGE_MILLIS {
                        from.render_level_gather_to(3, change, t, target.slot_zero());
                        from.render_transition_title_from(&target, t);
                    } else {
                        from.render_level_deal_from(3, change, t, target.slot_zero());
                    }
                    let slot = &mut slots[frame % 2];
                    for (start, end) in from.level_chrome_copy_spans() {
                        slot[start..end].copy_from_slice(&from.pixels()[start..end]);
                    }
                    for y in 120..495 {
                        let range = y * 960 + 268..y * 960 + 934;
                        slot[range.clone()].copy_from_slice(&from.pixels()[range]);
                    }
                    assert!(slot == from.pixels(), "root={root} {change:?} phase={t}");
                }
            }
        }
    }

    #[test]
    fn parallel_tricks_match_serial_pixels_and_reuse_the_current_worker() {
        let cards = cards(6);
        let scene = LauncherScene::new(960, 540);
        let mut initial_data = level(&cards, 3, &["CONSOLES"]);
        initial_data.level = LauncherLevel::Root;
        let initial = scene.prepare(initial_data);
        let mut renderer =
            ParallelLauncherRenderer::new(initial.frame_preparer(), None, None).unwrap();
        renderer.retain_bands(true);
        let mut generation = 0;
        for root in [true, false] {
            let mut data = level(&cards, 3, &["CONSOLES"]);
            if root {
                data.level = LauncherLevel::Root;
            }
            let mut serial = scene.prepare(data);
            let mut parallel = scene.prepare(data);
            let mut target_data = level(&cards, 0, &["CONSOLES", "NINTENDO"]);
            if !root {
                target_data.level = LauncherLevel::Root;
            }
            let target = scene.prepare(target_data);
            for change in [LevelChange::Descend, LevelChange::Ascend] {
                serial.restore_chrome();
                parallel.restore_chrome();
                for t in [0, 1, 150, 260, 414, 459, 460] {
                    generation += 1;
                    let request = LauncherFrameRequest {
                        frame: settled(3),
                        timestamp_us: t as u64 * 1000,
                        generation,
                    };
                    let before = parallel.pixels().to_vec();
                    let prepared = renderer
                        .prepare_helper_ahead(
                            &parallel.level_frame_preparer(3, change, t, target.slot_zero(), true),
                            request,
                        )
                        .unwrap();
                    assert!(parallel.pixels() == before);
                    serial.render_level_gather_to(3, change, t, target.slot_zero());
                    let timing = parallel
                        .render_level_gather_to_parallel(
                            request,
                            change,
                            t,
                            target.slot_zero(),
                            &mut renderer,
                        )
                        .unwrap();
                    assert_eq!(timing.helper_ahead, prepared);
                    serial.render_transition_title_from(&target, t);
                    parallel.render_transition_title_from(&target, t);
                    parallel.merge_retained_helper(&mut renderer);
                    assert!(
                        serial.pixels() == parallel.pixels(),
                        "root={root} {change:?} gather {t}"
                    );
                }
                for t in [460, 461, 600, 750, 866, 899, 919, 920] {
                    generation += 1;
                    let request = LauncherFrameRequest {
                        frame: settled(3),
                        timestamp_us: t as u64 * 1000,
                        generation,
                    };
                    let prepared = renderer
                        .prepare_helper_ahead(
                            &parallel.level_frame_preparer(3, change, t, target.slot_zero(), false),
                            request,
                        )
                        .unwrap();
                    serial.render_level_deal_from(3, change, t, target.slot_zero());
                    let timing = parallel
                        .render_level_deal_from_parallel(
                            request,
                            change,
                            t,
                            target.slot_zero(),
                            &mut renderer,
                        )
                        .unwrap();
                    assert_eq!(timing.helper_ahead, prepared);
                    parallel.merge_retained_helper(&mut renderer);
                    assert!(
                        serial.pixels() == parallel.pixels(),
                        "root={root} {change:?} deal {t}"
                    );
                }
            }
        }
    }

    #[test]
    fn sparse_chrome_matches_dense_fade_through_breadcrumbs_and_interruptions() {
        let source_cards = cards(6);
        let target_cards = cards(4);
        for scene in [
            LauncherScene::new(960, 540),
            LauncherScene::crt(640, 240),
            LauncherScene::new(540, 960),
        ] {
            for root in [false, true] {
                let mut data = level(&source_cards, 3, &["CONSOLES"]);
                if root {
                    data.level = LauncherLevel::Root;
                }
                let mut sparse = scene.prepare(data);
                let mut dense = scene.prepare(data);
                let target = scene.prepare(level(&target_cards, 0, &["CONSOLES", "NINTENDO"]));
                for change in [LevelChange::Descend, LevelChange::Ascend] {
                    sparse.restore_chrome();
                    dense.restore_chrome();
                    for t in [1, 130, 260, 280, 414, 459, 460, 260, 200, 459] {
                        sparse.render_level_gather_to(3, change, t, target.slot_zero());
                        dense.render_level_gather_to(3, change, t, target.slot_zero());
                        let alpha = ((GEOMETRY_ONE
                            - ease_in_out_cubic(window(t, 0, CHROME_OUT_MILLIS)))
                            * 256
                            / GEOMETRY_ONE) as u32;
                        let width = if dense.responsive.is_some() {
                            dense.scene.width
                        } else {
                            LOGICAL_WIDTH
                        };
                        // Previous implementation: repaint every pixel, including black.
                        for ((x0, y0, x1, y1), title) in dense.level_chrome_regions() {
                            let alpha = if title { 76 + 180 * alpha / 256 } else { alpha };
                            for y in y0..y1 {
                                for x in x0..x1 {
                                    let at = y * width + x;
                                    dense.logical[at] =
                                        Rgb565Pixel(scale_rgb565(dense.chrome[at].0, alpha));
                                }
                            }
                        }
                        dense.fit_output();
                        sparse.render_transition_title_from(&target, t);
                        dense.render_transition_title_from(&target, t);
                        assert!(
                            sparse.pixels() == dense.pixels(),
                            "scene={scene:?} root={root} change={change:?} t={t}"
                        );
                    }
                    for t in [460, 490, 600, 760, 866, 880, 900, 919, 790, 920] {
                        sparse.render_level_deal_from(3, change, t, target.slot_zero());
                        dense.render_level_deal_from(3, change, t, target.slot_zero());
                        let alpha =
                            (ease_out_quart(window(t, CHROME_IN_AT_MILLIS, CHROME_IN_MILLIS)) * 256
                                / GEOMETRY_ONE) as u32;
                        let width = if dense.responsive.is_some() {
                            dense.scene.width
                        } else {
                            LOGICAL_WIDTH
                        };
                        for ((x0, y0, x1, y1), title) in dense.level_chrome_regions() {
                            let alpha = if title { 76 + 180 * alpha / 256 } else { alpha };
                            for y in y0..y1 {
                                for x in x0..x1 {
                                    let at = y * width + x;
                                    dense.logical[at] =
                                        Rgb565Pixel(scale_rgb565(dense.chrome[at].0, alpha));
                                }
                            }
                        }
                        dense.fit_output();
                        assert!(
                            sparse.pixels() == dense.pixels(),
                            "scene={scene:?} root={root} change={change:?} deal t={t}"
                        );
                    }
                    sparse.refresh_chrome(data, None);
                    dense.refresh_chrome(data, None);
                }
            }
        }
    }

    #[test]
    fn restore_chrome_recovers_an_interrupted_change() {
        let from_cards = cards(6);
        let scene = LauncherScene::new(960, 540);
        let mut from = scene.prepare(level(&from_cards, 3, &["CONSOLES"]));
        from.render_level_gather_to(3, LevelChange::Ascend, 200, from.slot_zero());
        from.restore_chrome();
        from.render_frame(settled(3));
        let mut expected = scene.prepare(level(&from_cards, 3, &["CONSOLES"]));
        expected.render_frame(settled(3));
        assert!(from.pixels() == expected.pixels());
    }
}
