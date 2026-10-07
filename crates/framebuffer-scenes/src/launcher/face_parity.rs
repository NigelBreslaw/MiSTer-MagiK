// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! A harness that compares the two card face bakes: the fixed canvas's
//! (`artwork::face_cached` and `artwork::faces_rgb888`, 180x252) and the
//! responsive layout's (`Layout::faces`, here at the same 180x252). They are
//! separate renderers; before one replaces the other for HDMI landscape the
//! difference has to be known, by region, on real `Face` pixels.
//!
//! Each differing pixel is counted once, in the first region it falls in: the
//! border ring (the outer `RING` pixels, where the frame and rounded corners
//! live), the label band (the rows below `LABEL_ROW_PERCENT` of the height,
//! where the title and game count are drawn), or the interior.
//!
//! The table of current differences is pinned so any change to either bake,
//! intended or not, is visible as an edit to it.

use super::*;

const WIDTH: usize = 180;
const HEIGHT: usize = 252;
const RING: usize = 8;
const LABEL_ROW_PERCENT: usize = 70;

#[derive(Debug, Default, PartialEq, Eq)]
struct FaceDiff {
    ring: usize,
    labels: usize,
    interior: usize,
    max_channel_delta: i32,
}

impl FaceDiff {
    fn total(&self) -> usize {
        self.ring + self.labels + self.interior
    }
}

fn diff(a: &[Rgb565Pixel], b: &[Rgb565Pixel]) -> FaceDiff {
    assert_eq!(a.len(), WIDTH * HEIGHT);
    assert_eq!(b.len(), WIDTH * HEIGHT);
    let channels = |v: u16| {
        [
            i32::from((v >> 11) & 31),
            i32::from((v >> 5) & 63),
            i32::from(v & 31),
        ]
    };
    let mut out = FaceDiff::default();
    for (index, (x, y)) in a.iter().zip(b).enumerate() {
        if x == y {
            continue;
        }
        let (column, row) = (index % WIDTH, index / WIDTH);
        let ring = !(RING..WIDTH - RING).contains(&column) || !(RING..HEIGHT - RING).contains(&row);
        if ring {
            out.ring += 1;
        } else if row >= HEIGHT * LABEL_ROW_PERCENT / 100 {
            out.labels += 1;
        } else {
            out.interior += 1;
        }
        for (p, q) in channels(x.0).into_iter().zip(channels(y.0)) {
            out.max_channel_delta = out.max_channel_delta.max((p - q).abs());
        }
    }
    out
}

fn prepared_card<'a>(
    id: LauncherCardId,
    name: &'a str,
    games: Option<u32>,
    rgb888: Option<&'a [u8]>,
) -> PreparedCard<'a> {
    PreparedCard {
        id,
        name,
        games,
        colour: LauncherCardStyle::root(id).colour,
        name_mask: text_mask(name),
        games_mask: games.map_or_else(Vec::new, |games| text_mask(&format_games(games))),
        artwork: None,
        rgb888,
    }
}

/// The fixed canvas's faces: compact, detail and the MagiK back when it has one.
fn fixed(card: &PreparedCard<'_>) -> [Option<Vec<Rgb565Pixel>>; 3] {
    if card.rgb888.is_some() {
        let [compact, detail] = artwork::faces_rgb888(card, None);
        [Some(compact.pixels), Some(detail.pixels), None]
    } else {
        let mut bodies = artwork::BodyCache::default();
        let compact = artwork::face_cached(card, WIDTH, false, None, &mut bodies);
        let detail = artwork::face_cached(card, WIDTH, true, None, &mut bodies);
        let back = bodies.back_face(card).map(|face| face.pixels);
        [Some(compact.pixels), Some(detail.pixels), back]
    }
}

/// The responsive layout's faces at the same size.
fn responsive(card: &PreparedCard<'_>) -> [Option<Vec<Rgb565Pixel>>; 3] {
    let layout = responsive::Layout::for_card_size(WIDTH, HEIGHT);
    let fonts = layout.fonts(None);
    let mut bodies = artwork::BodyCache::default();
    let faces = layout.faces(card, &fonts, &mut bodies, false);
    [
        Some(faces.compact.pixels),
        Some(faces.detail.pixels),
        faces.back.map(|face| face.pixels),
    ]
}

fn cases() -> Vec<(String, FaceDiff)> {
    let source: Vec<u8> = (0..360 * 504 * 3)
        .map(|i| ((i * 7 + i / 360) % 251) as u8)
        .collect();
    let mut out = Vec::new();
    for (artwork_name, artwork) in [("generic", None), ("artwork", Some(source.as_slice()))] {
        for (id, name, games) in [
            (LauncherCardId::Arcade, "ARCADE", Some(1752)),
            (LauncherCardId::Consoles, "CONSOLES", Some(240)),
            (LauncherCardId::Computers, "COMPUTERS", None),
            (LauncherCardId::Handhelds, "HANDHELDS", Some(18)),
            (LauncherCardId::Favourites, "FAVOURITES", Some(126)),
            (LauncherCardId::Settings, "SETTINGS", None),
        ] {
            let card = prepared_card(id, name, games, artwork);
            let (a, b) = (fixed(&card), responsive(&card));
            for (face, (a, b)) in ["compact", "detail", "back"]
                .into_iter()
                .zip(a.iter().zip(&b))
            {
                match (a, b) {
                    (Some(a), Some(b)) => {
                        out.push((format!("{artwork_name} {id:?} {face}"), diff(a, b)));
                    }
                    (None, None) => {}
                    (a, b) => panic!(
                        "{artwork_name} {id:?} {face}: one path has a face the other lacks ({} vs {})",
                        a.is_some(),
                        b.is_some()
                    ),
                }
            }
        }
    }
    out
}

/// The current differences, (case, ring, labels, interior, max channel delta).
/// The responsive path is the one being compared against; a change to either
/// bake that moves a number is intended only if this table is edited with it.
#[cfg(not(any(feature = "card-axis-filter", feature = "card-fast-quantisation")))]
const PINNED: [(&str, usize, usize, usize, i32); 27] = [
    ("generic Arcade compact", 4705, 11105, 25630, 52),
    ("generic Arcade detail", 4705, 2805, 3654, 51),
    ("generic Consoles compact", 4705, 1863, 2648, 54),
    ("generic Consoles detail", 4705, 2390, 2648, 55),
    ("generic Consoles back", 4705, 698, 1676, 31),
    ("generic Computers compact", 4345, 1722, 3151, 50),
    ("generic Computers detail", 4345, 1722, 3151, 50),
    ("generic Computers back", 4345, 879, 2146, 46),
    ("generic Handhelds compact", 4345, 1761, 3368, 50),
    ("generic Handhelds detail", 4345, 2197, 3368, 52),
    ("generic Handhelds back", 4345, 879, 2102, 47),
    ("generic Favourites compact", 4545, 11086, 26710, 53),
    ("generic Favourites detail", 4545, 3176, 5336, 50),
    ("generic Settings compact", 4446, 11089, 26651, 50),
    ("generic Settings detail", 4446, 1837, 2075, 32),
    ("artwork Arcade compact", 6656, 7067, 16580, 50),
    ("artwork Arcade detail", 6656, 7264, 16580, 50),
    ("artwork Consoles compact", 6656, 7128, 16580, 50),
    ("artwork Consoles detail", 6656, 7345, 16580, 50),
    ("artwork Computers compact", 6656, 6931, 16580, 50),
    ("artwork Computers detail", 6656, 6931, 16580, 50),
    ("artwork Handhelds compact", 6655, 6950, 16580, 50),
    ("artwork Handhelds detail", 6655, 7109, 16580, 50),
    ("artwork Favourites compact", 6656, 6964, 16580, 50),
    ("artwork Favourites detail", 6656, 7167, 16580, 50),
    ("artwork Settings compact", 6656, 7101, 16580, 50),
    ("artwork Settings detail", 6656, 7101, 16580, 50),
];

#[test]
#[cfg(not(any(feature = "card-axis-filter", feature = "card-fast-quantisation")))]
fn the_two_face_bakes_differ_exactly_as_pinned() {
    let actual = cases();
    let changed: Vec<_> = actual
        .iter()
        .zip(PINNED)
        .filter(
            |((name, diff), (pinned_name, ring, labels, interior, max))| {
                name != pinned_name
                    || diff
                        != &FaceDiff {
                            ring: *ring,
                            labels: *labels,
                            interior: *interior,
                            max_channel_delta: *max,
                        }
            },
        )
        .map(|((name, d), _)| {
            format!(
                "(\"{name}\", {}, {}, {}, {}),",
                d.ring, d.labels, d.interior, d.max_channel_delta
            )
        })
        .collect();
    assert_eq!(actual.len(), PINNED.len());
    assert!(
        changed.is_empty(),
        "a face bake changed; if that is intended, update the table:\n{}",
        changed.join("\n")
    );
}

/// Print the table in a readable form: `cargo test --lib print_the_face_differences -- --nocapture`.
#[test]
fn print_the_face_differences() {
    for (name, diff) in cases() {
        println!(
            "FACEDIFF {name:?} total={} ring={} labels={} interior={} max={}",
            diff.total(),
            diff.ring,
            diff.labels,
            diff.interior,
            diff.max_channel_delta
        );
    }
}

/// Which fixed face the responsive compact face resembles, for a generic card.
///
/// `native_surface` builds the body of a generic card from the *detail*
/// surface whatever face it is for, so a responsive compact face of a card
/// whose compact and detail bodies differ (the cards without an icon: Arcade,
/// Favourites, Settings, whose selected face is flooded with their colour) is drawn in the
/// detail colours. Cards whose two bodies are nearly the same (the collection
/// cards) show no such difference. This pins the relationship, not a count: if
/// the responsive path starts to honour the compact body, the first assertion
/// fails and says so.
#[test]
fn the_responsive_compact_face_of_a_flooded_card_is_drawn_in_the_detail_colours() {
    for (id, name, games) in [
        (LauncherCardId::Arcade, "ARCADE", Some(1752)),
        (LauncherCardId::Favourites, "FAVOURITES", Some(126)),
        (LauncherCardId::Settings, "SETTINGS", None),
    ] {
        let card = prepared_card(id, name, games, None);
        let (fixed, responsive) = (fixed(&card), responsive(&card));
        let (compact, detail) = (fixed[0].as_ref().unwrap(), fixed[1].as_ref().unwrap());
        let responsive_compact = responsive[0].as_ref().unwrap();
        assert!(
            diff(compact, detail).total() > 30_000,
            "{id:?}: the fixed compact and detail faces are meant to differ"
        );
        assert!(
            diff(detail, responsive_compact).total() * 3
                < diff(compact, responsive_compact).total(),
            "{id:?}: the responsive compact face should resemble the fixed detail face"
        );
    }
    for (id, name, games) in [
        (LauncherCardId::Consoles, "CONSOLES", Some(240)),
        (LauncherCardId::Handhelds, "HANDHELDS", Some(18)),
    ] {
        let card = prepared_card(id, name, games, None);
        let fixed = fixed(&card);
        assert!(
            diff(fixed[0].as_ref().unwrap(), fixed[1].as_ref().unwrap()).total() < 2_000,
            "{id:?}: a collection card's compact and detail bodies are nearly the same"
        );
    }
}
