// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Independent coordinator regression checks for the milestone 2 contract.

use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection::{Left, Right},
    BrowsePhase, LauncherBrowser,
};

fn ready() -> LauncherBrowser {
    let mut browser = LauncherBrowser::new(5, 0);
    browser.neutral();
    browser
}

#[test]
fn duplicate_pressed_events_do_not_queue_a_second_tap() {
    let mut browser = ready();
    browser.press(Right, 0);
    browser.press(Right, 40);
    browser.press(Right, 80);
    browser.release(Right);
    assert_eq!(browser.frame(180).selected, 1);
    assert_eq!(browser.frame(1000).phase, BrowsePhase::Settled);
    assert_eq!(browser.selected(), 1);
}

#[test]
fn held_input_waits_until_threshold_and_release_keeps_slide_duration() {
    let mut browser = ready();
    browser.press(Right, 0);
    assert_eq!(browser.frame(180).phase, BrowsePhase::Settled);
    assert_eq!(browser.frame(299).phase, BrowsePhase::Settled);
    assert_eq!(browser.frame(300).target, 2);
    let moving = browser.frame(350);
    browser.release(Right);
    let released = browser.frame(350);
    assert_eq!(released.duration_millis, moving.duration_millis);
    assert_eq!(released.progress_millis, moving.progress_millis);
    assert_eq!(browser.frame(450).selected, 2);
    assert_eq!(browser.frame(1000).phase, BrowsePhase::Settled);
    assert_eq!(browser.selected(), 2);
}

#[test]
fn both_directions_held_never_restart_after_current_step() {
    let mut browser = ready();
    browser.press(Right, 0);
    browser.press(Left, 20);
    assert_eq!(browser.frame(180).selected, 1);
    for now in [300, 500, 1000, 5000] {
        assert_eq!(browser.frame(now).phase, BrowsePhase::Settled);
        assert_eq!(browser.selected(), 1);
    }
}

#[test]
fn delayed_tick_cannot_skip_unseen_categories() {
    let mut browser = ready();
    browser.press(Right, 0);
    let resumed = browser.frame(30_000);
    assert_eq!(resumed.selected, 1);
    assert_eq!(resumed.target, 2);
    assert_eq!(resumed.progress_millis, 0);
    assert_eq!(browser.frame(30_000).selected, 1);
}

#[test]
fn rapid_new_taps_have_one_latest_pending_intent() {
    let mut browser = ready();
    browser.press(Right, 0);
    browser.release(Right);
    browser.press(Right, 20);
    browser.release(Right);
    browser.press(Left, 40);
    browser.release(Left);
    let queued = browser.frame(180);
    assert_eq!(queued.selected, 1);
    assert_eq!(queued.target, 0);
    assert_eq!(browser.frame(360).selected, 0);
    assert_eq!(browser.frame(1000).phase, BrowsePhase::Settled);
}

#[test]
fn reset_removes_target_and_pending_intent_until_neutral() {
    let mut browser = ready();
    browser.press(Right, 0);
    browser.reset();
    browser.press(Left, 20);
    let reset = browser.frame(1000);
    assert_eq!(reset.phase, BrowsePhase::Settled);
    assert_eq!(reset.target, reset.selected);
    browser.neutral();
    browser.press(Left, 1001);
    browser.release(Left);
    assert_eq!(browser.frame(1181).selected, 4);
}

#[test]
fn cyclic_taps_wrap_both_directions_without_changing_order() {
    let mut browser = ready();
    for (step, expected) in [1, 2, 3, 4, 0].into_iter().enumerate() {
        let now = step as u64 * 200;
        browser.press(Right, now);
        browser.release(Right);
        assert_eq!(browser.frame(now + 180).selected, expected);
    }
    for (step, expected) in [4, 3, 2, 1, 0].into_iter().enumerate() {
        let now = 1000 + step as u64 * 200;
        browser.press(Left, now);
        browser.release(Left);
        assert_eq!(browser.frame(now + 180).selected, expected);
    }
}
