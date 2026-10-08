// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Animation time is vsync-locked: one display period per produced frame (see
//! `mister_magik_core::frame_clock`). Nothing that moves may read a real clock.
//!
//! Each file below owns animation state. Its production code may contain only
//! the listed number of wall-clock reads, and every one of those is profiling,
//! a timeout, or file I/O pacing - never a source of motion. A new read fails
//! this test. If it is genuinely telemetry, raise the allowance and say why in
//! the commit; if it feeds an animation, take the time from `FrameClock`.

use std::path::Path;

const WALL_CLOCK_READS: &[&str] = &[
    "Instant::now()",
    "SystemTime::now()",
    ".elapsed()",
    "monotonic_us()",
    "thread::sleep",
];

/// (path relative to the crate, allowed production wall-clock reads)
const ANIMATION_SOURCES: &[(&str, usize)] = &[
    ("src/launcher.rs", 12),
    ("src/launcher_runtime/navigation_transition.rs", 6),
    ("src/launcher_runtime/orientation_transition.rs", 5),
    ("src/launcher_runtime/full_screen_transition.rs", 0),
    ("src/launcher_runtime/startup_intro.rs", 0),
    ("src/launcher_runtime/input_router.rs", 0),
    ("src/preview_transition.rs", 0),
    ("src/screenshot_transitions.rs", 0),
    ("src/crt_backdrop.rs", 8),
    ("src/ui_runner/crt_backdrop_controller.rs", 2),
    // Both reads time preparation only when `measure_preparation` is set.
    ("src/ui_runner/launcher_card_home.rs", 2),
    ("src/ui_runner/launcher_startup_intro.rs", 1),
    ("src/return_catalog_capsule.rs", 0),
    ("src/setup_nav.rs", 0),
    ("../../crates/magik-core/src/input_repeat.rs", 0),
    ("../../crates/magik-core/src/frame_clock.rs", 0),
    ("../../crates/framebuffer-scenes/src/spring_animation.rs", 0),
    ("../../crates/framebuffer-scenes/src/navigation.rs", 20),
    ("../../crates/screenshot-parade/src/schedule.rs", 12),
    ("../../crates/particles/src/engine.rs", 0),
    ("../../crates/particles/src/intro.rs", 26),
];

fn production_wall_clock_reads(source: &str) -> usize {
    let lines: Vec<&str> = source.lines().collect();
    let production_end = lines
        .iter()
        .enumerate()
        .find(|(index, line)| {
            line.trim() == "#[cfg(test)]"
                && lines
                    .get(index + 1)
                    .is_some_and(|next| next.trim_start().starts_with("mod "))
        })
        .map_or(lines.len(), |(index, _)| index);
    lines[..production_end]
        .iter()
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| WALL_CLOCK_READS.iter().any(|read| line.contains(read)))
        .count()
}

#[test]
fn animation_modules_do_not_read_a_wall_clock() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut failures = Vec::new();
    for (path, allowed) in ANIMATION_SOURCES {
        let source = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|error| panic!("read {path}: {error}"));
        let found = production_wall_clock_reads(&source);
        if found != *allowed {
            failures.push(format!(
                "{path}: {found} wall-clock reads, allowed {allowed}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "animation time must come from FrameClock, not a wall clock:\n{}",
        failures.join("\n")
    );
}
