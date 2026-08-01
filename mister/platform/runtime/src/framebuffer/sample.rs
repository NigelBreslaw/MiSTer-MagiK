// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::boot_analytics;
use slint::platform::software_renderer::Rgb565Pixel;
use std::fs::OpenOptions;
use std::io::Write;

const SAMPLE_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const SAMPLE_HASH_PRIME: u64 = 0x1000_0000_01b3;
const SAMPLE_STEP: usize = 16;

pub struct Rgb565SampleView<'a> {
    pixels: &'a [Rgb565Pixel],
    width: usize,
    height: usize,
    stride_pixels: usize,
}

impl<'a> Rgb565SampleView<'a> {
    pub fn new(
        pixels: &'a [Rgb565Pixel],
        width: usize,
        height: usize,
        stride_pixels: usize,
    ) -> Self {
        Self {
            pixels,
            width,
            height,
            stride_pixels,
        }
    }

    pub fn right_edge_signature(&self, cols: usize) -> (u64, u32) {
        self.vertical_edge_signature(self.width.saturating_sub(cols), self.width, cols)
    }

    pub fn left_edge_signature(&self, cols: usize) -> (u64, u32) {
        self.vertical_edge_signature(0, cols.min(self.width), cols)
    }

    pub fn top_edge_signature(&self, rows: usize) -> (u64, u32) {
        self.horizontal_edge_signature(0, rows.min(self.height), rows)
    }

    pub fn bottom_edge_signature(&self, rows: usize) -> (u64, u32) {
        self.horizontal_edge_signature(self.height.saturating_sub(rows), self.height, rows)
    }

    pub fn sampled_signature(&self) -> (u64, u32) {
        let mut hash = SAMPLE_HASH_OFFSET;
        let mut nonzero = 0u32;
        for y in (0..self.height).step_by(SAMPLE_STEP) {
            for x in (0..self.width).step_by(SAMPLE_STEP) {
                let p = self.pixel_rgb_at(x, y);
                if p != 0 {
                    nonzero += 1;
                }
                hash = hash_sample(hash, p);
            }
        }
        (hash, nonzero)
    }

    pub fn rect_sampled_signature(
        &self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        step: usize,
    ) -> (u64, u32) {
        let x1 = x.saturating_add(w).min(self.width);
        let y1 = y.saturating_add(h).min(self.height);
        let step = step.max(1);
        let mut hash = SAMPLE_HASH_OFFSET;
        let mut nonzero = 0u32;

        for yy in (y..y1).step_by(step) {
            for xx in (x..x1).step_by(step) {
                let p = self.pixel_rgb_at(xx, yy);
                if p != 0 {
                    nonzero += 1;
                }
                hash = hash_sample(hash, p);
            }
        }

        (hash, nonzero)
    }

    pub fn record_visual_sample(&self, label: &str) {
        if !boot_analytics::enabled() {
            return;
        }

        let sample = self.visual_sample();
        boot_analytics::event(
            "fb_visual_sample",
            format!(
                "label={} class={} samples={} nonzero={} blackish={} color_min={:06x} color_max={:06x} transitions={} hash={:016x}",
                label,
                sample.classification,
                sample.samples,
                sample.nonzero,
                sample.blackish,
                sample.color_min,
                sample.color_max,
                sample.transitions,
                sample.hash
            ),
        );

        let path = "/tmp/mister-magik-visual-samples.tsv";
        let needs_header = std::fs::metadata(path)
            .map(|m| m.len() == 0)
            .unwrap_or(true);
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            if needs_header {
                let _ = writeln!(
                    f,
                    "boot_ms\tlabel\tclass\tsamples\tnonzero\tblackish\tcolor_min\tcolor_max\ttransitions\thash"
                );
            }
            let _ = writeln!(
                f,
                "{}\t{}\t{}\t{}\t{}\t{}\t{:06x}\t{:06x}\t{}\t{:016x}",
                boot_ms(),
                sanitize_tsv(label),
                sample.classification,
                sample.samples,
                sample.nonzero,
                sample.blackish,
                sample.color_min,
                sample.color_max,
                sample.transitions,
                sample.hash
            );
        }
    }

    fn pixel_rgb_at(&self, x: usize, y: usize) -> u32 {
        rgb565_to_rgb888(self.pixels[y * self.stride_pixels + x])
    }

    fn vertical_edge_signature(&self, x0: usize, x1: usize, min_cols: usize) -> (u64, u32) {
        let mut hash = SAMPLE_HASH_OFFSET;
        let mut nonzero = 0u32;
        let x_end = x1.max(x0 + min_cols.min(self.width - x0)).min(self.width);
        for y in 0..self.height {
            for x in x0..x_end {
                let p = self.pixel_rgb_at(x, y);
                if p != 0 {
                    nonzero += 1;
                }
                hash = hash_sample(hash, p);
            }
        }
        (hash, nonzero)
    }

    fn horizontal_edge_signature(&self, y0: usize, y1: usize, min_rows: usize) -> (u64, u32) {
        let mut hash = SAMPLE_HASH_OFFSET;
        let mut nonzero = 0u32;
        let y_end = y1.max(y0 + min_rows.min(self.height - y0)).min(self.height);
        for y in y0..y_end {
            for x in 0..self.width {
                let p = self.pixel_rgb_at(x, y);
                if p != 0 {
                    nonzero += 1;
                }
                hash = hash_sample(hash, p);
            }
        }
        (hash, nonzero)
    }

    fn visual_sample(&self) -> VisualSample {
        let mut hash = SAMPLE_HASH_OFFSET;
        let mut samples = 0u32;
        let mut nonzero = 0u32;
        let mut blackish = 0u32;
        let mut transitions = 0u32;
        let mut color_min = 0x00ff_ffffu32;
        let mut color_max = 0u32;
        let mut prev: Option<u32> = None;
        for y in (0..self.height).step_by(SAMPLE_STEP) {
            for x in (0..self.width).step_by(SAMPLE_STEP) {
                let p = self.pixel_rgb_at(x, y);
                samples += 1;
                if p != 0 {
                    nonzero += 1;
                }
                let r = (p >> 16) & 0xff;
                let g = (p >> 8) & 0xff;
                let b = p & 0xff;
                if r < 8 && g < 8 && b < 8 {
                    blackish += 1;
                }
                color_min = color_min.min(p);
                color_max = color_max.max(p);
                if let Some(prev) = prev {
                    if color_distance(prev, p) > 96 {
                        transitions += 1;
                    }
                }
                prev = Some(p);
                hash = hash_sample(hash, p);
            }
        }
        let nonzero_pct = pct(nonzero, samples);
        let blackish_pct = pct(blackish, samples);
        let transition_pct = pct(transitions, samples.saturating_sub(1).max(1));
        let classification = if blackish_pct >= 95 {
            "mostly_black"
        } else if nonzero_pct >= 20 && transition_pct >= 35 {
            "static_like"
        } else if nonzero_pct >= 5 {
            "slint_like"
        } else {
            "unknown"
        };
        VisualSample {
            hash,
            samples,
            nonzero,
            blackish,
            color_min,
            color_max,
            transitions,
            classification,
        }
    }
}

struct VisualSample {
    hash: u64,
    samples: u32,
    nonzero: u32,
    blackish: u32,
    color_min: u32,
    color_max: u32,
    transitions: u32,
    classification: &'static str,
}

fn hash_sample(hash: u64, pixel_rgb888: u32) -> u64 {
    (hash ^ pixel_rgb888 as u64).wrapping_mul(SAMPLE_HASH_PRIME)
}

fn pct(n: u32, d: u32) -> u32 {
    if d == 0 { 0 } else { n.saturating_mul(100) / d }
}

fn color_distance(a: u32, b: u32) -> u32 {
    let ar = (a >> 16) & 0xff;
    let ag = (a >> 8) & 0xff;
    let ab = a & 0xff;
    let br = (b >> 16) & 0xff;
    let bg = (b >> 8) & 0xff;
    let bb = b & 0xff;
    ar.abs_diff(br) + ag.abs_diff(bg) + ab.abs_diff(bb)
}

pub(crate) fn rgb565_to_rgb888(pixel: Rgb565Pixel) -> u32 {
    let v = pixel.0;
    let r5 = (v >> 11) & 0x1f;
    let g6 = (v >> 5) & 0x3f;
    let b5 = v & 0x1f;
    let r = ((r5 << 3) | (r5 >> 2)) as u32;
    let g = ((g6 << 2) | (g6 >> 4)) as u32;
    let b = ((b5 << 3) | (b5 >> 2)) as u32;
    (r << 16) | (g << 8) | b
}

fn sanitize_tsv(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\t' | '\n' | '\r' => ' ',
            _ => c,
        })
        .collect()
}

fn boot_ms() -> u64 {
    let Ok(s) = std::fs::read_to_string("/proc/uptime") else {
        return 0;
    };
    let Some(first) = s.split_whitespace().next() else {
        return 0;
    };
    let Ok(secs) = first.parse::<f64>() else {
        return 0;
    };
    (secs * 1000.0).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_to_rgb888_expands_channels() {
        assert_eq!(rgb565_to_rgb888(Rgb565Pixel(0xf800)), 0xff0000);
        assert_eq!(rgb565_to_rgb888(Rgb565Pixel(0x07e0)), 0x00ff00);
        assert_eq!(rgb565_to_rgb888(Rgb565Pixel(0x001f)), 0x0000ff);
    }

    #[test]
    fn sampled_signature_uses_stride() {
        let pixels = [
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0x0000),
        ];
        let view = Rgb565SampleView::new(&pixels, 2, 2, 3);

        assert_eq!(view.sampled_signature().1, 1);
        assert_eq!(view.right_edge_signature(1).1, 2);
    }

    #[test]
    fn edge_signatures_clip_requests_and_count_only_visible_pixels() {
        let pixels = [
            Rgb565Pixel(1),
            Rgb565Pixel(0),
            Rgb565Pixel(2),
            Rgb565Pixel(0),
            Rgb565Pixel(3),
            Rgb565Pixel(4),
        ];
        let view = Rgb565SampleView::new(&pixels, 3, 2, 3);
        assert_eq!(view.left_edge_signature(99).1, 4);
        assert_eq!(view.right_edge_signature(1).1, 2);
        assert_eq!(view.top_edge_signature(1).1, 2);
        assert_eq!(view.bottom_edge_signature(99).1, 4);
    }

    #[test]
    fn rect_sampling_clips_to_geometry_and_normalizes_zero_step() {
        let pixels = (0..16)
            .map(|value| Rgb565Pixel(if value % 2 == 0 { 0 } else { 1 }))
            .collect::<Vec<_>>();
        let view = Rgb565SampleView::new(&pixels, 4, 4, 4);
        assert_eq!(view.rect_sampled_signature(2, 2, 99, 99, 0).1, 2);
        assert_eq!(view.rect_sampled_signature(9, 9, 2, 2, 1).1, 0);
        assert_eq!(view.rect_sampled_signature(0, 0, 4, 4, 2).1, 0);
    }

    #[test]
    fn visual_classifier_distinguishes_black_content_and_transitions() {
        let black = vec![Rgb565Pixel(0); 64 * 64];
        let solid = vec![Rgb565Pixel(0xffff); 64 * 64];
        let alternating = (0..64 * 64)
            .map(|index| {
                let x = index % 64;
                let y = index / 64;
                Rgb565Pixel(if (x / 16 + y / 16) % 2 == 0 {
                    0xf800
                } else {
                    0x001f
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            Rgb565SampleView::new(&black, 64, 64, 64)
                .visual_sample()
                .classification,
            "mostly_black"
        );
        assert_eq!(
            Rgb565SampleView::new(&solid, 64, 64, 64)
                .visual_sample()
                .classification,
            "slint_like"
        );
        let sample = Rgb565SampleView::new(&alternating, 64, 64, 64).visual_sample();
        assert_eq!(sample.classification, "static_like");
        assert!(sample.transitions > 0);
        assert_eq!(sample.samples, 16);
    }

    #[test]
    fn sampling_helpers_are_bounded_and_tsv_safe() {
        assert_eq!(pct(1, 0), 0);
        assert_eq!(pct(1, 3), 33);
        assert_eq!(pct(u32::MAX, 1), u32::MAX);
        assert_eq!(color_distance(0xff0000, 0x0000ff), 510);
        assert_eq!(sanitize_tsv("a\tb\nc\rd"), "a b c d");
        assert_ne!(hash_sample(SAMPLE_HASH_OFFSET, 0), SAMPLE_HASH_OFFSET);
    }
}
