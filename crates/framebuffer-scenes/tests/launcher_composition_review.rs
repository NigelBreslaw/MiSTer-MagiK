// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Coordinator checks against the accepted static composition, not new snapshots.
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use mister_magik_framebuffer_scenes::launcher::{LauncherCard, LauncherData, LauncherScene};
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
fn prepared_animation_performs_no_heap_allocations() {
    let mut prepared = LauncherScene::new(960, 540).prepare(data(0));
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
fn perspective_endpoints_clipping_and_reverse_mapping() {
    let scene = LauncherScene::new(960, 540);
    let baseline = scene.render(data(0));
    let mut prepared = scene.prepare(data(0));
    let mut output = vec![Rgb565Pixel(0); 960 * 540];
    let mut other = output.clone();
    for direction in [BrowseDirection::Left, BrowseDirection::Right] {
        for progress in [0, 1, 150, 229, 230, 231, 310, 459, 460] {
            let mut frame = moving(0, direction, progress);
            frame.phase = BrowsePhase::Flipping;
            frame.duration_millis = 460;
            prepared.set_reverse_flip(true);
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
            if progress == 150 {
                prepared.set_reverse_flip(false);
                prepared.render_into(frame, &mut other);
                assert_ne!(
                    output, other,
                    "mapping must change perspective, not navigation"
                );
            }
        }
    }
}

const CARDS: [LauncherCard<'static>; 5] = [
    LauncherCard {
        name: "ARCADE",
        games: 1752,
        colour: 0x88a6,
    },
    LauncherCard {
        name: "SNK NEOGEO",
        games: 324,
        colour: 0x195f,
    },
    LauncherCard {
        name: "CONSOLES",
        games: 842,
        colour: 0xc5b5,
    },
    LauncherCard {
        name: "HANDHELDS",
        games: 126,
        colour: 0x2c92,
    },
    LauncherCard {
        name: "COMPUTERS",
        games: 86,
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
        phase: BrowsePhase::Sliding,
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
