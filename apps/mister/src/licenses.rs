// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_framebuffer_scenes::settings_cog::CrtSettingsGeometry;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

// Directly shipped third-party assets remain visible in the in-app legal surface.
pub const LICENSE_TITLES: [&str; 12] = [
    "MiSTer MagiK",
    "FFmpeg",
    "Slint",
    "Press Start 2P",
    "Commercial Fonts",
    "Jersey",
    "Spleen",
    "Terminus",
    "Rust standard library",
    "zlib",
    "libpng",
    "Card artwork",
];

pub const LICENSE_KINDS: [&str; 12] = [
    "GPL-3.0", "LGPL-2.1", "GPL-3.0", "OFL-1.1", "LICENSED", "OFL-1.1", "BSD-2", "OFL-1.1", "MIT",
    "ZLIB", "LIBPNG", "CC / GML",
];

const GPL3: &str = include_str!("../../../LICENSE");
const FFMPEG: &str = include_str!("../licenses/FFMPEG.txt");
const PRESS_START_2P: &str = include_str!("../licenses/PRESS-START-2P.txt");
const COMMERCIAL_FONTS: &str = include_str!("../licenses/COMMERCIAL-FONTS.txt");
const JERSEY: &str = include_str!("../licenses/JERSEY.txt");
const TERMINUS_FONT: &str = include_str!("../licenses/TERMINUS-FONT.txt");
const SPLEEN: &str = include_str!("../licenses/SPLEEN.txt");
const RUST_LIBRARIES: &str = include_str!("../licenses/RUST-LIBRARIES.txt");
const HDMI_LINE_COLUMNS: usize = 105;
const HDMI_VISIBLE_ROWS: usize = 21;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LicenseViewport {
    columns: usize,
    visible_rows: usize,
}

impl LicenseViewport {
    pub const HDMI: Self = Self {
        columns: HDMI_LINE_COLUMNS,
        visible_rows: HDMI_VISIBLE_ROWS,
    };

    pub fn for_crt(width: usize, height: usize, safe_x: usize, safe_y: usize) -> Option<Self> {
        let geometry = CrtSettingsGeometry::for_viewport(width, height, safe_x, safe_y)?;
        let text_width = width
            .saturating_sub(2 * geometry.margin_x())
            .saturating_sub(10 * geometry.scale_x());
        let glyph_width = if geometry.scale_y() == 2 { 12 } else { 6 };
        let content_top = geometry.header_bottom() + 26 * geometry.scale_y();
        let viewport_height = geometry.footer_rule().saturating_sub(content_top);
        Some(Self {
            columns: (text_width / glyph_width).max(1),
            visible_rows: (viewport_height / (12 * geometry.scale_y())).max(1),
        })
    }

    pub const fn columns(self) -> usize {
        self.columns
    }

    pub const fn visible_rows(self) -> usize {
        self.visible_rows
    }
}

impl Default for LicenseViewport {
    fn default() -> Self {
        Self::HDMI
    }
}

type SharedLicenseLines = Arc<[String]>;
type LicenseLineCache = Mutex<HashMap<(usize, usize), SharedLicenseLines>>;

pub fn text(index: usize) -> &'static str {
    match index {
        0 | 2 => GPL3,
        1 => FFMPEG,
        3 => PRESS_START_2P,
        4 => COMMERCIAL_FONTS,
        5 => JERSEY,
        6 => SPLEEN,
        7 => TERMINUS_FONT,
        8..=10 => RUST_LIBRARIES,
        11 => include_str!("../licenses/CARD-ARTWORK.txt"),
        _ => GPL3,
    }
}

pub fn wrapped_lines(index: usize, viewport: LicenseViewport) -> SharedLicenseLines {
    static LINES: OnceLock<LicenseLineCache> = OnceLock::new();
    let index = index.min(LICENSE_TITLES.len() - 1);
    let key = (index, viewport.columns());
    let mut lines = LINES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("license line cache poisoned");
    lines
        .entry(key)
        .or_insert_with(|| Arc::from(wrap_text(index, viewport.columns())))
        .clone()
}

fn wrap_text(index: usize, columns: usize) -> Vec<String> {
    let mut result = Vec::new();
    for source_line in text(index).lines() {
        if source_line.trim().is_empty() {
            result.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in source_line.split_whitespace() {
            let word_len = word.chars().count();
            if !line.is_empty() && line.chars().count() + 1 + word_len > columns {
                result.push(std::mem::take(&mut line));
            }
            if word_len > columns {
                if !line.is_empty() {
                    result.push(std::mem::take(&mut line));
                }
                let chars = word.chars().collect::<Vec<_>>();
                for chunk in chars.chunks(columns) {
                    result.push(chunk.iter().collect());
                }
            } else {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
            }
        }
        if !line.is_empty() {
            result.push(line);
        }
    }
    result
}

pub fn max_scroll_line(index: usize, viewport: LicenseViewport) -> usize {
    wrapped_lines(index, viewport)
        .len()
        .saturating_sub(viewport.visible_rows())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_major_license_has_full_text_and_can_scroll() {
        for index in [0, 1, 2, 3, 5, 7, 8, 9, 10] {
            assert!(
                text(index).len() > 1_000,
                "{} text is incomplete",
                LICENSE_TITLES[index]
            );
            assert!(
                max_scroll_line(index, LicenseViewport::HDMI) > 0,
                "{} text does not scroll",
                LICENSE_TITLES[index]
            );
            assert!(!wrapped_lines(index, LicenseViewport::HDMI).is_empty());
        }
        assert!(COMMERCIAL_FONTS.contains("Yesterday 10"));
        assert!(COMMERCIAL_FONTS.contains("Xerxes 10"));
        assert!(COMMERCIAL_FONTS.contains("Nocive 15"));
        assert!(COMMERCIAL_FONTS.contains("Bacteria 12"));
    }

    #[test]
    fn app_surface_is_limited_to_directly_relevant_license_texts() {
        assert_eq!(
            LICENSE_TITLES,
            [
                "MiSTer MagiK",
                "FFmpeg",
                "Slint",
                "Press Start 2P",
                "Commercial Fonts",
                "Jersey",
                "Spleen",
                "Terminus",
                "Rust standard library",
                "zlib",
                "libpng",
                "Card artwork"
            ]
        );
        assert_eq!(text(2), GPL3);
        assert!(FFMPEG.contains("FFmpeg 8.1.2"));
        assert!(PRESS_START_2P.contains("SIL Open Font License"));
        assert!(COMMERCIAL_FONTS.contains("commercial licences"));
        assert!(JERSEY.contains("SIL OPEN FONT LICENSE"));
        assert!(TERMINUS_FONT.contains("Reserved Font Name \"Terminus Font\""));
        assert!(SPLEEN.contains("Redistribution and use in source and binary forms"));
        assert!(RUST_LIBRARIES.contains("zlib License"));
    }

    #[test]
    fn crt_lines_fit_each_native_spleen_viewport_and_scroll_to_the_end() {
        for (width, height) in [
            (640, 240),
            (640, 288),
            (640, 480),
            (640, 512),
            (640, 576),
            (240, 640),
            (288, 640),
            (480, 640),
            (512, 640),
            (576, 640),
        ] {
            let viewport = LicenseViewport::for_crt(width, height, 0, 0).unwrap();
            for (index, title) in LICENSE_TITLES.iter().enumerate() {
                let lines = wrapped_lines(index, viewport);
                assert!(
                    lines
                        .iter()
                        .all(|line| line.chars().count() <= viewport.columns()),
                    "{} at {width}x{height}",
                    title
                );
                assert_eq!(
                    max_scroll_line(index, viewport),
                    lines.len().saturating_sub(viewport.visible_rows())
                );
            }
        }
        let portrait = LicenseViewport::for_crt(240, 640, 0, 0).unwrap();
        assert_eq!((portrait.columns(), portrait.visible_rows()), (16, 18));
    }
}
