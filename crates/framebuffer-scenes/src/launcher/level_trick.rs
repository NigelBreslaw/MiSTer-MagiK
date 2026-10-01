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

impl PreparedLauncher {
    pub fn render_level_gather_to(
        &mut self,
        selected: usize,
        change: LevelChange,
        elapsed_millis: u32,
        destination: CardSlot,
    ) {
        let t = elapsed_millis.min(EDGE_MILLIS);
        if t == 0 {
            self.restore_chrome();
            self.render_frame(settled(selected));
            return;
        }
        self.fade_level_chrome(GEOMETRY_ONE - ease_in_out_cubic(window(t, 0, CHROME_OUT_MILLIS)));
        let hero = hero_pose(self.slot_zero(), destination, change, t);
        let progress = window(t, 0, EDGE_MILLIS);
        let gather = ease_in_out_cubic(progress);
        let mut behind = scaled_pose(hero, BEHIND_SCALE);
        behind.angle = 0;
        behind.brightness = 64;
        let faces = Arc::clone(&self.faces);
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
            let Some(index) = neighbour(&faces, self.cyclic, selected, relative) else {
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
            items[count] = Some(CarouselItem {
                face: &faces[index].compact,
                blend: None,
                pose,
            });
            count += 1;
        }
        if let Some(card) = faces.get(selected) {
            items[count] = Some(CarouselItem {
                face: &card.detail,
                blend: None,
                pose: hero,
            });
        }
        self.draw_trick_plan(&mut CarouselPlan { items, row: false });
    }

    pub fn render_level_deal_from(
        &mut self,
        selected: usize,
        change: LevelChange,
        elapsed_millis: u32,
        source: CardSlot,
    ) {
        let t = elapsed_millis.clamp(EDGE_MILLIS, LEVEL_TRICK_MILLIS);
        if t == LEVEL_TRICK_MILLIS {
            self.restore_chrome();
            self.render_frame(settled(selected));
            return;
        }
        self.fade_level_chrome(ease_out_quart(window(
            t,
            CHROME_IN_AT_MILLIS,
            CHROME_IN_MILLIS,
        )));
        let hero = hero_pose(source, self.slot_zero(), change, t);
        let mut behind = scaled_pose(hero, BEHIND_SCALE);
        behind.angle = 0;
        behind.brightness = 64;
        let faces = Arc::clone(&self.faces);
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
            let Some(index) = neighbour(&faces, self.cyclic, selected, relative) else {
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
            items[count] = Some(CarouselItem {
                face: &faces[index].compact,
                blend: None,
                pose,
            });
            count += 1;
        }
        if let Some(card) = faces.get(selected) {
            items[count] = Some(CarouselItem {
                face: &card.detail,
                blend: None,
                pose: hero,
            });
        }
        self.draw_trick_plan(&mut CarouselPlan { items, row: false });
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
        self.fit_output();
    }

    pub fn restore_chrome(&mut self) {
        self.logical.copy_from_slice(&self.chrome);
        self.fit_output();
    }

    fn draw_trick_plan(&mut self, plan: &mut CarouselPlan<'_>) {
        // Both halves already use native poses. Mapping again would move the hero at the swap.
        if let Some(layout) = self.responsive {
            layout.clear_carousel(&mut self.logical);
            layout.draw_plan(&mut self.logical, plan, &mut self.flip_columns);
            return;
        }
        for y in 120..495 {
            self.logical[y * LOGICAL_WIDTH + 268..y * LOGICAL_WIDTH + 934]
                .fill(Rgb565Pixel(BACKGROUND));
        }
        for left in (268..934).step_by(crate::launcher_flip::STRIP_WIDTH) {
            draw_carousel_plan(
                &mut self.logical,
                LOGICAL_WIDTH,
                (0, 0),
                plan,
                &mut self.flip_columns,
                (left, (left + crate::launcher_flip::STRIP_WIDTH).min(934)),
            );
        }
        self.fit_output();
    }
    fn fade_level_chrome(&mut self, alpha: i64) {
        let alpha = (alpha * 256 / GEOMETRY_ONE) as u32;
        let width = if self.responsive.is_some() {
            self.scene.width
        } else {
            LOGICAL_WIDTH
        };
        let (logical, chrome) = (&mut self.logical, &self.chrome);
        let mut fade = |x0: usize, y0: usize, x1: usize, y1: usize| {
            for y in y0..y1 {
                let row = y * width;
                for (out, pixel) in logical[row + x0..row + x1]
                    .iter_mut()
                    .zip(&chrome[row + x0..row + x1])
                {
                    *out = Rgb565Pixel(scale_rgb565(pixel.0, alpha));
                }
            }
        };
        if let Some(layout) = self.responsive {
            let (_, title_end) = layout.level_chrome_rows()[0];
            let (_, _, _, rule_y) = layout.title_rect();
            fade(0, rule_y + 1, width, title_end);
            let (y0, y1) = layout.level_chrome_rows()[1];
            fade(0, y0, width, y1);
        } else {
            fade(0, 77, 265, 500);
            fade(296, 77, LOGICAL_WIDTH, 120);
        }
        let title_alpha = 76 + 180 * alpha / 256;
        let (x0, y0, x1, y1) = self.responsive.map_or((26, 0, 826, 76), |l| l.title_rect());
        for y in y0..y1 {
            for x in x0..x1 {
                let at = y * width + x;
                self.logical[at] = Rgb565Pixel(scale_rgb565(self.chrome[at].0, title_alpha));
            }
        }
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
