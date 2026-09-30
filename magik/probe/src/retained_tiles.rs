// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Eight immutable tile publications through the qualified native presenter.
use mister_magik_framebuffer_scenes::retained_tiles::{RetainedTileSlots, TileImageIdentity};
use mister_magik_mister_runtime::framebuffer::hidden_latch::HiddenLatchPresenter;
use mister_magik_mister_runtime::framebuffer::rgb565::Rgb565;
use std::time::Instant;

const W: usize = 960;
const H: usize = 540;
const RECTS: [(usize, usize, usize, usize); 2] = [(296, 120, 629, 495), (629, 120, 934, 495)];
const FIXTURE: &str = "5c8a464daea0e0aa0a8253855f04d63122bf528652a23864e60ae0fc567f9551";
const TILE_PIXELS: usize = 239_250;

pub(super) fn run() -> Result<serde_json::Value, String> {
    let mut expected = vec![Rgb565(7); W * H];
    for (index, (x0, y0, x1, y1)) in RECTS.into_iter().enumerate() {
        for y in y0..y1 {
            expected[y * W + x0..y * W + x1].fill(Rgb565(if index == 0 { 11 } else { 13 }));
        }
    }
    let mut presenter =
        HiddenLatchPresenter::open(W as u16, H as u16).map_err(|e| e.to_string())?;
    let work_count = TILE_PIXELS * 8;
    let mut samples = Vec::with_capacity(2);
    for repetition in 0..2 {
        // Both slots receive identical chrome outside timing. Tile identity
        // starts unknown, so each slot must receive the complete tile image.
        let mut baseline = None;
        for _ in 0..2 {
            presenter.pixels_mut().fill(Rgb565(7));
            baseline = Some(presenter.present().map_err(|e| e.to_string())?);
        }
        let baseline = baseline.unwrap();
        let mut retained = RetainedTileSlots::default();
        let identity = TileImageIdentity::new(7, 19);
        let mut bytes = 0;
        let mut copy_ns = 0;
        let start = Instant::now();
        let mut last = baseline;
        let mut slots = Vec::with_capacity(8);
        for _ in 0..8 {
            let slot = presenter.writable_slot_index();
            slots.push(slot);
            let copied = retained.write_if_changed(slot, identity, || {
                let copy_start = Instant::now();
                let destination = presenter.pixels_mut();
                for (x0, y0, x1, y1) in RECTS {
                    for y in y0..y1 {
                        let range = y * W + x0..y * W + x1;
                        destination[range.clone()].copy_from_slice(&expected[range]);
                    }
                }
                copy_ns += copy_start.elapsed().as_nanos() as u64;
                Ok::<_, String>(TILE_PIXELS * 2)
            })?;
            bytes += copied.unwrap_or(0);
            // Read the complete physical slot before posting, including chrome.
            // This identical oracle overhead is inside each sample's wall time.
            if presenter.pixels_mut() != expected.as_slice() {
                return Err("retained slot pixel mismatch".into());
            }
            last = presenter.present().map_err(|e| e.to_string())?;
        }
        let duration_ns = start.elapsed().as_nanos() as u64;
        let posts = last.post_count.wrapping_sub(baseline.post_count);
        let flips = last.flip_count.wrapping_sub(baseline.flip_count);
        if posts != 8 || flips != 8 || slots.windows(2).any(|slots| slots[0] == slots[1]) {
            return Err(
                "retained publications lost physical posts, flips or slot alternation".into(),
            );
        }
        samples.push(serde_json::json!({"repetition":repetition,"fixture_identity":FIXTURE,"work_count":work_count,
            "duration_ns":duration_ns,"ns_per_pixel":duration_ns as f64/work_count as f64,
            "copied_tile_bytes":bytes,"copy_duration_ns":copy_ns,"physical_posts":posts,"confirmed_flips":flips,
            "repeated_vblanks_delta":last.drop_count.wrapping_sub(baseline.drop_count),"slots":slots}));
    }
    let sha = std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256")
        .map_err(|_| "native benchmark artifact identity missing")?;
    Ok(
        serde_json::json!({"schema_version":1,"workload":"retained-home-tiles","mode":"timing","artifact_sha256":sha,
        "correctness":"passed","fixture":{"identity":FIXTURE,"geometry":[W,H],"rectangles":RECTS,
        "publications_per_sample":8,"chrome":"preseeded outside timing","image_identity":[7,19],
        "timed_work":"tile policy, writes, exact physical-slot oracle, v5 posts and confirmed flips"},
        "work_count":work_count,"samples":samples}),
    )
}
