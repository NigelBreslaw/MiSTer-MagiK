// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[test]
fn navigation_label_signatures_are_ascii_case_insensitive() {
    assert_eq!(
        navigation_label_signature("Atari"),
        navigation_label_signature("ATARI")
    );
}

#[test]
fn crt_overlay_sweeps_holds_clears_and_preserves_endpoints() {
    let width = 12;
    let height = 10;
    let original = vec![Rgb565Pixel(0xffff); width * height];
    let full_phosphor_pixels = ((height + 1) / CRT_SCANLINE_PERIOD_ROWS * width) as u64;
    for (progress, expected_full) in [
        (0, false),
        (1, false),
        (CRT_SWEEP_END_Q16, true),
        (PROGRESS_MAX / 2, true),
        (CRT_CLEAR_START_Q16, true),
        (PROGRESS_MAX - 1, false),
        (PROGRESS_MAX, false),
    ] {
        let mut pixels = original.clone();
        let mut stats = NavigationTransitionRenderStats::default();
        apply_crt_scanline_overlay(
            &mut pixels,
            width,
            height,
            NavigationTransitionFrame {
                phase: NavigationTransitionPhase::Reveal,
                progress_q16: progress,
                ..NavigationTransitionFrame::default()
            },
            &mut stats,
        );
        if progress == 0 || progress == PROGRESS_MAX {
            assert_eq!(pixels, original);
            assert_eq!(stats.phosphor_pixels, 0);
            assert_eq!(stats.scanline_pixels, 0);
        } else if expected_full {
            assert_eq!(stats.phosphor_pixels, full_phosphor_pixels);
            assert_eq!(stats.scanline_pixels, 0);
            let darkened = darken_rgb565_7_8(Rgb565Pixel(0xffff));
            for y in 0..height {
                let expected = if y >= 1 && (y - 1) % CRT_SCANLINE_PERIOD_ROWS == 0 {
                    darkened
                } else {
                    Rgb565Pixel(0xffff)
                };
                assert!(
                    pixels[y * width..(y + 1) * width]
                        .iter()
                        .all(|pixel| *pixel == expected)
                );
            }
        } else {
            assert!(
                stats.phosphor_pixels < full_phosphor_pixels,
                "progress={progress} phosphor_pixels={}",
                stats.phosphor_pixels
            );
            assert!(stats.scanline_pixels <= width as u64 * 5);
        }
    }

    let mut reversing = original.clone();
    let mut stats = NavigationTransitionRenderStats::default();
    apply_crt_scanline_overlay(
        &mut reversing,
        width,
        height,
        NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Reversing,
            progress_q16: 20_000,
            reverse_origin_q16: PROGRESS_MAX / 2,
            reverse_leg_progress_q16: PROGRESS_MAX / 2,
            ..NavigationTransitionFrame::default()
        },
        &mut stats,
    );
    assert_eq!(stats.phosphor_pixels, full_phosphor_pixels);
    assert_eq!(stats.scanline_pixels, 0);
}
