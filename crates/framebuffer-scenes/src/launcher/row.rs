// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Nested card choreography; faces and projection remain owned by the renderer.
use super::*;
use crate::launcher_flip::Pose;

pub(super) const TILT: i64 = GEOMETRY_ONE * 14 / 180;
const BRIGHTNESS: [u32; 5] = [256, 184, 143, 108, 82];

pub(super) fn slot(k: usize) -> Pose {
    let mut scale = GEOMETRY_ONE;
    let mut left = 292 * GEOMETRY_ONE;
    for _ in 0..k {
        left += 153 * scale;
        scale = scale * 9 / 10;
    }
    Pose {
        x: left,
        top: 284 * GEOMETRY_ONE - 126 * scale,
        width: 180 * scale,
        height: 252 * scale,
        angle: if k == 0 { 0 } else { TILT },
        brightness: BRIGHTNESS[k],
        clip: (268, 934),
        body_clip: (268, 934),
        vertical_clip: (120, 438, 495),
    }
}

pub(super) fn crt_brightness(brightness: u32) -> u32 {
    const CRT: [u32; 5] = [256, 154, 115, 90, 72];
    for i in 0..4 {
        if brightness >= BRIGHTNESS[i + 1] {
            let fraction = brightness.saturating_sub(BRIGHTNESS[i + 1]);
            return CRT[i + 1]
                + (CRT[i] - CRT[i + 1]) * fraction / (BRIGHTNESS[i] - BRIGHTNESS[i + 1]);
        }
    }
    CRT[4]
}

fn between(a: Pose, b: Pose, k: i64) -> Pose {
    let lerp = |x, y| x + (y - x) * k / GEOMETRY_ONE;
    Pose {
        x: lerp(a.x, b.x),
        top: lerp(a.top, b.top),
        width: lerp(a.width, b.width),
        height: lerp(a.height, b.height),
        angle: lerp(a.angle, b.angle),
        brightness: lerp(i64::from(a.brightness), i64::from(b.brightness)) as u32,
        ..a
    }
}

pub(super) fn build(faces: &[CardFaces], frame: BrowseFrame) -> CarouselPlan<'_> {
    let n = faces.len();
    let visible = n.min(5);
    let selected = frame.selected % n;
    let mut items = [None; CAROUSEL_CAPACITY];
    let mut count = 0;
    let mut push = |index: usize, pose: Pose, back: bool| {
        items[count] = Some(CarouselItem {
            face: if back {
                faces[index].back.as_ref().unwrap_or(&faces[index].compact)
            } else if pose.brightness == 256 {
                &faces[index].detail
            } else {
                &faces[index].compact
            },
            blend: None,
            pose,
        });
        count += 1;
    };
    if n == 1 || frame.phase == crate::launcher_navigation::BrowsePhase::Settled {
        for k in (0..visible).rev() {
            push((selected + k) % n, slot(k), false);
        }
        return CarouselPlan { items, row: true };
    }
    let right = frame.direction == Some(BrowseDirection::Right);
    let raw = i64::from(frame.progress_millis.min(frame.duration_millis)) * GEOMETRY_ONE
        / i64::from(frame.duration_millis.max(1));
    let k = if frame.duration_millis == crate::launcher_navigation::SPRING_POSITION_UNITS {
        raw
    } else {
        crate::card_page::ease_in_out(raw)
    };
    let front = if right { selected } else { frame.target % n };
    let end = if right {
        (selected + visible) % n
    } else {
        (selected + visible - 1) % n
    };
    let home = slot(visible - 1);
    let tuck = Pose {
        x: home.x - 10 * GEOMETRY_ONE,
        ..home
    };
    let e = if right { k } else { GEOMETRY_ONE - k };
    let mut pose = between(tuck, home, e);
    let turn = if right { raw } else { GEOMETRY_ONE - raw };
    pose.angle = GEOMETRY_ONE + (TILT - GEOMETRY_ONE) * turn / GEOMETRY_ONE;
    // End card is always furthest back. A short row may use the same face twice.
    push(end, pose, pose.angle > GEOMETRY_ONE / 2);
    for rel in (1..visible).rev() {
        let index = if right {
            (selected + rel) % n
        } else {
            (selected + rel - 1) % n
        };
        let (a, b) = if right {
            (slot(rel), slot(rel - 1))
        } else {
            (slot(rel - 1), slot(rel))
        };
        push(index, between(a, b, k), false);
    }
    let out = Pose {
        x: 68 * GEOMETRY_ONE,
        ..slot(0)
    };
    let pose = if right {
        between(slot(0), out, k)
    } else {
        between(out, slot(0), k)
    };
    push(front, pose, false);
    CarouselPlan { items, row: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_slots_follow_the_reference_edges_and_overlap() {
        let edges = [292.0, 445.0, 582.7, 706.63, 818.167];
        for (k, edge) in edges.into_iter().enumerate() {
            let pose = slot(k);
            assert!((pose.x as f64 / GEOMETRY_ONE as f64 - edge).abs() < 0.01);
            assert_eq!(pose.width * 7, pose.height * 5);
        }
    }

    #[test]
    fn short_rows_draw_the_same_card_in_front_and_at_the_end() {
        let cards = [LauncherCard {
            id: LauncherCardId::Consoles,
            name: "ONE",
            games: Some(1),
            colour: 0x2a7f,
        }; 2];
        let level = LauncherLevel::Nested(NestedLevel {
            path: &["CONSOLES"],
            games: 2,
            children: 2,
            children_label: "SYSTEMS",
            detail: None,
            accent: 0x2a7f,
        });
        let prepared = LauncherScene::new(960, 540).prepare(LauncherData {
            cards: &cards,
            selected: 0,
            library_games: 2,
            collections: 2,
            favourites: 0,
            clock: "12:00",
            level,
        });
        let frame = BrowseFrame {
            selected: 0,
            target: 1,
            phase: crate::launcher_navigation::BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis: 345,
            duration_millis: 460,
        };
        let plan = build(&prepared.faces, frame);
        let items: Vec<_> = plan.items.iter().flatten().collect();
        assert_eq!(items.len(), 3);
        assert_eq!(
            items[0].pose.angle,
            GEOMETRY_ONE + (TILT - GEOMETRY_ONE) * 3 / 4
        );
        assert!(std::ptr::eq(items[0].face, &prepared.faces[0].compact));
        assert!(std::ptr::eq(items[2].face, &prepared.faces[0].detail));
        assert_eq!(items[2].pose.angle, 0);
    }
}
