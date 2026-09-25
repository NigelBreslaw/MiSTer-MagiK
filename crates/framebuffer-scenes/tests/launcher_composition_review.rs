// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Coordinator checks against the accepted static composition, not new snapshots.
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use mister_magik_framebuffer_scenes::launcher::{
    LauncherCard, LauncherCardId, LauncherData, LauncherFrameRequest, LauncherScene,
};
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowseFrame, BrowsePhase,
};

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static WATCH_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
struct ReviewAllocator;
fn record_allocation() {
    if WATCH_ALLOCATIONS.try_with(Cell::get).unwrap_or(false) {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
    }
}
// SAFETY: ownership and layouts are delegated unchanged to System.
unsafe impl GlobalAlloc for ReviewAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        // SAFETY: forward the caller's valid layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: this pointer/layout came from the same System allocator.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation();
        // SAFETY: forward the caller's valid allocation and new size.
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: ReviewAllocator = ReviewAllocator;

#[test]
fn streamed_tiles_match_serial_at_changing_strip_boundaries() {
    let mut serial = LauncherScene::new(960, 540).prepare(data(0));
    let preparer = serial.frame_preparer();
    let mut tile = preparer.new_tile_buffer();
    assert!(tile.storage_bytes() < 2 * 1024 * 1024);
    for direction in [BrowseDirection::Left, BrowseDirection::Right] {
        for phase in [BrowsePhase::Flipping] {
            for (i, progress) in [0, 1, 100, 229, 230, 231, 310, 459, 460]
                .into_iter()
                .enumerate()
            {
                let mut frame = moving(i % 5, direction, progress);
                frame.phase = phase;
                frame.duration_millis = 460;
                serial.render_frame(frame);
                let split = [296, 457, 615, 774, 934][i % 5];
                let request = LauncherFrameRequest {
                    frame,
                    timestamp_us: 0,
                    generation: 1,
                };
                for clip in [(296, split), (split, 934)] {
                    ALLOCATIONS.with(|count| count.set(0));
                    WATCH_ALLOCATIONS.with(|watch| watch.set(true));
                    preparer.render_tile(request, &mut tile, clip);
                    WATCH_ALLOCATIONS.with(|watch| watch.set(false));
                    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
                    for y in 120..495 {
                        assert!(
                            serial.pixels()[y * 960 + clip.0..y * 960 + clip.1]
                                == tile.pixels()[y * 960 + clip.0..y * 960 + clip.1],
                            "{direction:?} {phase:?} {progress} row {y} clip {clip:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn first_display_frame_is_identical_to_fully_prepared_resting_frame() {
    for selected in 0..5 {
        let scene = LauncherScene::new(960, 540);
        let initial = scene.prepare_initial(data(selected));
        let first = initial.pixels().to_vec();
        let mut complete = initial.finish();
        complete.render_frame(BrowseFrame {
            selected,
            target: selected,
            phase: BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 0,
        });
        assert_eq!(first, complete.pixels());
        assert_eq!(first, scene.render(data(selected)));
    }
}

#[test]
fn collection_index_is_absent_from_static_and_animated_frames() {
    let scene = LauncherScene::new(960, 540);
    let mut prepared = scene.prepare(data(0));
    for frame in [
        BrowseFrame {
            selected: 0,
            target: 0,
            phase: BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 0,
        },
        moving(0, BrowseDirection::Right, 90),
    ] {
        prepared.render_frame(frame);
        for y in 95..111 {
            assert!(
                prepared.pixels()[y * 960 + 880..y * 960 + 934]
                    .iter()
                    .all(|pixel| *pixel == Rgb565Pixel(0)),
                "collection index region was not blank on row {y}: {frame:?}"
            );
        }
    }
}

#[test]
fn prepared_animation_performs_no_heap_allocations() {
    let mut prepared = LauncherScene::new(960, 540).prepare(data(0));
    assert!(
        prepared.cached_raster_bytes() < 64 * 1024 * 1024,
        "five-category fixture cache exceeds the documented budget"
    );
    let mut output = vec![Rgb565Pixel(0); 960 * 540];
    ALLOCATIONS.with(|count| count.set(0));
    WATCH_ALLOCATIONS.with(|watch| watch.set(true));
    for progress in (0..=180).step_by(15) {
        prepared.render_into(moving(0, BrowseDirection::Right, progress), &mut output);
    }
    for progress in (0..=460).step_by(10) {
        let mut frame = moving(0, BrowseDirection::Right, progress);
        frame.phase = BrowsePhase::Flipping;
        frame.duration_millis = 460;
        prepared.render_into(frame, &mut output);
    }
    WATCH_ALLOCATIONS.with(|watch| watch.set(false));
    assert_eq!(ALLOCATIONS.with(Cell::get), 0);
}

#[test]
fn incoming_and_outgoing_cards_cross_with_opposed_flips() {
    let scene = LauncherScene::new(960, 540);
    let baseline = scene.render(data(0));
    let mut prepared = scene.prepare(data(0));
    let mut output = vec![Rgb565Pixel(0); 960 * 540];
    for direction in [BrowseDirection::Left, BrowseDirection::Right] {
        let mut frame = moving(0, direction, 70);
        frame.phase = BrowsePhase::Flipping;
        frame.duration_millis = 460;
        prepared.render_into(frame, &mut output);
        // The outgoing card translates, shrinks and flips away during approach.
        assert_ne!(
            &output[163 * 960..405 * 960],
            &baseline[163 * 960..405 * 960]
        );
        let spring_progress = |t: u32| {
            i64::from(
                mister_magik_framebuffer_scenes::spring_animation::smooth_spring_q16(
                    (t * u32::from(u16::MAX) / 460) as u16,
                ),
            ) * 460
                / i64::from(u16::MAX)
        };
        let delta = if direction == BrowseDirection::Right {
            -144
        } else {
            144
        };
        frame.progress_millis = 380;
        prepared.render_into(frame, &mut output);
        // Actual slot overlap, not delayed movement, brings the incoming face
        // in front as it becomes face-on.
        let progress = spring_progress(380);
        let centre = 610 + delta * progress / 460;
        let scale = 90 - 18 * progress / 460;
        let x = (centre - scale) as usize;
        let overlap = output[300 * 960 + x..300 * 960 + x + 2 * scale as usize]
            .iter()
            .filter(|p| {
                if direction == BrowseDirection::Right {
                    p.0 & 31 > (p.0 >> 11) * 2
                } else {
                    p.0 >> 11 > (p.0 & 31) * 2
                }
            })
            .count();
        assert!(
            overlap >= 2,
            "incoming overlaps while outgoing moves: {direction:?}, {overlap}"
        );
    }
}

#[test]
fn perspective_endpoints_and_clipping_match_the_accepted_mapping() {
    let scene = LauncherScene::new(960, 540);
    let baseline = scene.render(data(0));
    let mut prepared = scene.prepare(data(0));
    let mut output = vec![Rgb565Pixel(0); 960 * 540];
    for direction in [BrowseDirection::Left, BrowseDirection::Right] {
        for progress in [0, 1, 150, 229, 230, 231, 310, 459, 460] {
            let mut frame = moving(0, direction, progress);
            frame.phase = BrowsePhase::Flipping;
            frame.duration_millis = 460;
            prepared.render_into(frame, &mut output);
            for row in 0..540 {
                assert_eq!(
                    &output[row * 960..row * 960 + 296],
                    &baseline[row * 960..row * 960 + 296]
                );
                assert_eq!(
                    &output[row * 960 + 934..(row + 1) * 960],
                    &baseline[row * 960 + 934..(row + 1) * 960]
                );
            }
            if progress == 0 || progress == 460 {
                let expected = scene.render(data(if progress == 0 { 0 } else { frame.target }));
                assert_eq!(
                    &output[135 * 960..465 * 960],
                    &expected[135 * 960..465 * 960]
                );
            }
        }
    }
}

const CARDS: [LauncherCard<'static>; 5] = [
    LauncherCard {
        id: LauncherCardId::Arcade,
        name: "ARCADE",
        games: Some(1752),
        colour: 0x88a6,
    },
    LauncherCard {
        id: LauncherCardId::Consoles,
        name: "SNK NEOGEO",
        games: Some(324),
        colour: 0x195f,
    },
    LauncherCard {
        id: LauncherCardId::Consoles,
        name: "CONSOLES",
        games: Some(842),
        colour: 0xc5b5,
    },
    LauncherCard {
        id: LauncherCardId::Handhelds,
        name: "HANDHELDS",
        games: Some(126),
        colour: 0x2c92,
    },
    LauncherCard {
        id: LauncherCardId::Computers,
        name: "COMPUTERS",
        games: Some(86),
        colour: 0xb9a6,
    },
];
fn data(selected: usize) -> LauncherData<'static> {
    LauncherData {
        cards: &CARDS,
        selected,
        library_games: 6842,
        collections: 18,
        favourites: 126,
        clock: "21:37",
    }
}
fn moving(selected: usize, direction: BrowseDirection, progress: u32) -> BrowseFrame {
    BrowseFrame {
        selected,
        target: (selected
            + if direction == BrowseDirection::Right {
                1
            } else {
                4
            })
            % 5,
        phase: BrowsePhase::Flipping,
        direction: Some(direction),
        progress_millis: progress,
        duration_millis: 180,
    }
}

#[test]
fn all_moving_frames_preserve_every_sidebar_pixel() {
    let scene = LauncherScene::new(960, 540);
    let baseline = scene.render(data(0));
    let mut prepared = scene.prepare(data(0));
    let mut output = vec![Rgb565Pixel(0); 960 * 540];
    for direction in [BrowseDirection::Left, BrowseDirection::Right] {
        for progress in [0, 30, 90, 150, 180] {
            prepared.render_into(moving(0, direction, progress), &mut output);
            for row in 0..540 {
                assert_eq!(
                    &output[row * 960..row * 960 + 296],
                    &baseline[row * 960..row * 960 + 296],
                    "sidebar row {row}, {direction:?}, {progress}"
                );
            }
        }
    }
}

#[test]
fn each_start_and_end_matches_the_accepted_card_geometry() {
    let scene = LauncherScene::new(960, 540);
    let mut prepared = scene.prepare(data(0));
    let mut output = vec![Rgb565Pixel(0); 960 * 540];
    for selected in 0..5 {
        for direction in [BrowseDirection::Left, BrowseDirection::Right] {
            for progress in [0, 180] {
                let frame = moving(selected, direction, progress);
                let expected = scene.render(data(if progress == 0 {
                    selected
                } else {
                    frame.target
                }));
                prepared.render_into(frame, &mut output);
                // Indicator is intentionally allowed to switch only on settlement.
                for row in 135..465 {
                    assert_eq!(
                        &output[row * 960 + 296..row * 960 + 934],
                        &expected[row * 960 + 296..row * 960 + 934],
                        "card row {row}, selection {selected}, {direction:?}, {progress}"
                    );
                }
            }
        }
    }
}
