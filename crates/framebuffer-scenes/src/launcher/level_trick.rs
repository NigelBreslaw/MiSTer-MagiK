// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hierarchy level change, the "card trick". One continuous motion: the chosen
//! card eases round to edge-on while the other cards turn and slide in behind
//! it. At the edge-on moment it snaps round into the next level's card and the
//! next level's cards flip out from behind it, edge first, all together.
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
pub const LEVEL_TRICK_EDGE_MILLIS: u32 = 400;
const EDGE_MILLIS: u32 = LEVEL_TRICK_EDGE_MILLIS;
/// The chosen card completes its turn quickly after edge-on.
const SNAP_MILLIS: u32 = 200;
const DEAL_MILLIS: u32 = 480;
const DEAL_STAGGER_MILLIS: u32 = 30;
/// Header and group summary fade out, then the next level's fade in.
const CHROME_OUT_MILLIS: u32 = 260;
const CHROME_IN_AT_MILLIS: u32 = EDGE_MILLIS + 60;
const CHROME_IN_MILLIS: u32 = 360;
/// The cards gather behind the chosen card at 90% of its size.
const BEHIND_SCALE: i64 = GEOMETRY_ONE * 9 / 10;
/// The chosen card lifts by 4% while it turns.
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
    /// First half of a level change, rendered by the level being left: the
    /// other cards turn and slide in behind `selected` while it eases round
    /// to edge-on and this level's header and summary fade out. Times at or
    /// after the edge render the all-edge-on hold frame.
    pub fn render_level_gather(
        &mut self,
        selected: usize,
        change: LevelChange,
        elapsed_millis: u32,
    ) {
        let t = elapsed_millis.min(EDGE_MILLIS);
        let out = ease_in_out_cubic(window(t, 0, CHROME_OUT_MILLIS));
        self.fade_level_chrome(GEOMETRY_ONE - out);
        let progress = window(t, 0, EDGE_MILLIS);
        let gather = ease_in_out_cubic(progress);
        let turn = progress * progress / GEOMETRY_ONE;
        let faces = Arc::clone(&self.faces);
        let mut items = [None; 6];
        let mut count = 0;
        for relative in [2, -2, 1, -1] {
            let Some(index) = neighbour(&faces, self.cyclic, selected, relative) else {
                continue;
            };
            let rest = settled_pose(relative);
            let mut pose = lerp_pose(rest, scaled_centre(BEHIND_SCALE), gather);
            pose.angle = rest.angle * (GEOMETRY_ONE - gather) / GEOMETRY_ONE
                + relative.signum() as i64 * EDGE_ON * turn / GEOMETRY_ONE;
            items[count] = Some(CarouselItem {
                face: &faces[index].compact,
                blend: None,
                pose,
            });
            count += 1;
        }
        if let Some(card) = faces.get(selected) {
            let mut pose = scaled_centre(lift(t));
            pose.angle = change.spin() * EDGE_ON * turn / GEOMETRY_ONE;
            items[count] = Some(CarouselItem {
                face: &card.detail,
                blend: None,
                pose,
            });
        }
        self.draw_trick_plan(&mut CarouselPlan { items });
    }

    /// Second half of a level change, rendered by the level being entered:
    /// `selected` snaps round from edge-on and its neighbours flip out from
    /// behind it while this level's header and summary fade in. Elapsed time
    /// is measured from the start of the gather; times before the edge render
    /// the all-edge-on hold frame. At [`LEVEL_TRICK_MILLIS`] the frame equals
    /// a settled frame of this level.
    pub fn render_level_deal(&mut self, selected: usize, change: LevelChange, elapsed_millis: u32) {
        let t = elapsed_millis.clamp(EDGE_MILLIS, LEVEL_TRICK_MILLIS);
        self.fade_level_chrome(ease_out_quart(window(
            t,
            CHROME_IN_AT_MILLIS,
            CHROME_IN_MILLIS,
        )));
        let faces = Arc::clone(&self.faces);
        let mut items = [None; 6];
        let mut count = 0;
        for relative in [2, -2, 1, -1] {
            let Some(index) = neighbour(&faces, self.cyclic, selected, relative) else {
                continue;
            };
            // Nearest cards leave first, right before left.
            let order = (relative.unsigned_abs() - 1) * 2 + usize::from(relative < 0);
            let at = EDGE_MILLIS + order as u32 * DEAL_STAGGER_MILLIS;
            let dealt = ease_out_quart(window(t, at, DEAL_MILLIS));
            let rest = settled_pose(relative);
            let mut pose = lerp_pose(scaled_centre(BEHIND_SCALE), rest, dealt);
            // A generic card emerges showing its MagiK back, 150 degrees round,
            // and turns face-up on its way out. Cards with artwork have no
            // back and simply turn in from edge-on.
            let back = faces[index].back.as_ref();
            if dealt == 0 && back.is_some() {
                // Still hidden behind the chosen card: a card showing its back
                // must not appear during the edge-on hold or before its turn.
                continue;
            }
            let start = if back.is_some() {
                EDGE_ON * 5 / 3
            } else {
                EDGE_ON
            };
            pose.angle = rest.angle * dealt / GEOMETRY_ONE
                - relative.signum() as i64 * start * (GEOMETRY_ONE - dealt) / GEOMETRY_ONE;
            let face = match back {
                Some(back) if pose.angle.abs() > EDGE_ON => back,
                _ => &faces[index].compact,
            };
            items[count] = Some(CarouselItem {
                face,
                blend: None,
                pose,
            });
            count += 1;
        }
        if let Some(card) = faces.get(selected) {
            let snapped = ease_out_quart(window(t, EDGE_MILLIS, SNAP_MILLIS));
            let mut pose = scaled_centre(lift(t));
            pose.angle = -change.spin() * EDGE_ON * (GEOMETRY_ONE - snapped) / GEOMETRY_ONE;
            items[count] = Some(CarouselItem {
                face: &card.detail,
                blend: None,
                pose,
            });
        }
        self.draw_trick_plan(&mut CarouselPlan { items });
    }

    /// Restore this level's complete chrome after an interrupted level change.
    pub fn restore_chrome(&mut self) {
        self.logical.copy_from_slice(&self.chrome);
        self.fit_output();
    }

    fn draw_trick_plan(&mut self, plan: &mut CarouselPlan<'_>) {
        if let Some(layout) = self.responsive {
            layout.clear_carousel(&mut self.logical);
            layout.map_plan(plan);
            layout.draw_plan(&mut self.logical, plan, &mut self.flip_columns);
            return;
        }
        for rect in Self::logical_damage() {
            for y in rect.y0..rect.y1 {
                self.logical[y * LOGICAL_WIDTH + rect.x0..y * LOGICAL_WIDTH + rect.x1]
                    .fill(Rgb565Pixel(BACKGROUND));
            }
        }
        for left in (296..934).step_by(crate::launcher_flip::STRIP_WIDTH) {
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

    /// Scale this level's header and group summary. Chrome is text on black,
    /// so scaling each pixel is an exact fade. Shared chrome is untouched.
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
            for (y0, y1) in layout.level_chrome_rows() {
                fade(0, y0, width, y1);
            }
        } else {
            // Header, sidebar, and the carousel's section label.
            fade(0, 0, LOGICAL_WIDTH, 77);
            fade(0, 77, 265, 500);
            fade(296, 77, LOGICAL_WIDTH, 120);
        }
    }
}

fn neighbour(faces: &[CardFaces], cyclic: bool, selected: usize, relative: isize) -> Option<usize> {
    let count = faces.len() as isize;
    if count == 0 {
        return None;
    }
    let position = selected as isize + relative;
    if cyclic {
        // A short cyclic ring must not show the same card twice.
        (count > relative.unsigned_abs() as isize * 2 || relative.abs() == 1 && count > 1)
            .then(|| position.rem_euclid(count) as usize)
    } else {
        (0..count).contains(&position).then_some(position as usize)
    }
}

/// The chosen card lifts slightly through the gather and snap.
fn lift(t: u32) -> i64 {
    GEOMETRY_ONE
        + LIFT_SCALE * sin_half_turn(window(t, 0, EDGE_MILLIS + SNAP_MILLIS)) / GEOMETRY_ONE
}

fn settled_pose(relative: isize) -> Pose {
    continuous_geometry(relative, relative, 0)
}

/// The centre card scaled about its own centre.
fn scaled_centre(scale: i64) -> Pose {
    let mut pose = settled_pose(0);
    let width = pose.width * scale / GEOMETRY_ONE;
    let height = pose.height * scale / GEOMETRY_ONE;
    pose.x += (pose.width - width) / 2;
    pose.top += (pose.height - height) / 2;
    pose.width = width;
    pose.height = height;
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
    fn trick_starts_on_the_source_and_ends_on_the_destination() {
        for scene in [LauncherScene::new(960, 540), LauncherScene::crt(640, 240)] {
            let from_cards = cards(6);
            let to_cards = cards(4);
            let mut from = scene.prepare(level(&from_cards, 3, &["CONSOLES"]));
            let mut expected_from = scene.prepare(level(&from_cards, 3, &["CONSOLES"]));
            expected_from.render_frame(settled(3));
            from.render_level_gather(3, LevelChange::Descend, 0);
            assert!(
                from.pixels() == expected_from.pixels(),
                "{scene:?} first frame"
            );

            let mut to = scene.prepare(level(&to_cards, 0, &["CONSOLES", "NINTENDO"]));
            let mut expected_to = scene.prepare(level(&to_cards, 0, &["CONSOLES", "NINTENDO"]));
            expected_to.render_frame(settled(0));
            to.render_level_deal(0, LevelChange::Descend, LEVEL_TRICK_MILLIS);
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
        from.render_level_gather(3, LevelChange::Descend, EDGE_MILLIS);
        assert!(widest_lit_row(from.pixels()) < 16);
        // The destination can hold here for as long as preparation takes.
        to.render_level_deal(0, LevelChange::Descend, 0);
        assert!(widest_lit_row(to.pixels()) < 16);
        // Headers and summaries are fully faded at the swap.
        assert!(from.pixels()[..77 * 960].iter().all(|p| p.0 == 0));
        assert!(to.pixels()[..77 * 960].iter().all(|p| p.0 == 0));
    }

    #[test]
    fn restore_chrome_recovers_an_interrupted_change() {
        let from_cards = cards(6);
        let scene = LauncherScene::new(960, 540);
        let mut from = scene.prepare(level(&from_cards, 3, &["CONSOLES"]));
        from.render_level_gather(3, LevelChange::Ascend, 200);
        from.restore_chrome();
        from.render_frame(settled(3));
        let mut expected = scene.prepare(level(&from_cards, 3, &["CONSOLES"]));
        expected.render_frame(settled(3));
        assert!(from.pixels() == expected.pixels());
    }
}
