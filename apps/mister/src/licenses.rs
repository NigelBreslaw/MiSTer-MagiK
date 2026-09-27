// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::OnceLock;

// Directly shipped third-party assets remain visible in the in-app legal surface.
pub const LICENSE_TITLES: [&str; 11] = [
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
];

pub const LICENSE_KINDS: [&str; 11] = [
    "GPL-3.0", "LGPL-2.1", "GPL-3.0", "OFL-1.1", "LICENSED", "OFL-1.1", "BSD-2", "OFL-1.1", "MIT",
    "ZLIB", "LIBPNG",
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
const CRT_LINE_COLUMNS: usize = 43;
const CRT_VISIBLE_ROWS: usize = 12;

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
        _ => GPL3,
    }
}

pub fn wrapped_lines(index: usize) -> &'static [String] {
    wrapped_lines_for(index, false)
}

pub fn wrapped_lines_for(index: usize, crt: bool) -> &'static [String] {
    static HDMI_LINES: [OnceLock<Vec<String>>; 11] = [const { OnceLock::new() }; 11];
    static CRT_LINES: [OnceLock<Vec<String>>; 11] = [const { OnceLock::new() }; 11];
    let index = index.min(LICENSE_TITLES.len() - 1);
    let (lines, columns) = if crt {
        (&CRT_LINES, CRT_LINE_COLUMNS)
    } else {
        (&HDMI_LINES, HDMI_LINE_COLUMNS)
    };
    lines[index].get_or_init(|| wrap_text(index, columns))
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

pub fn max_scroll_line(index: usize) -> usize {
    max_scroll_line_for(index, false)
}

pub fn max_scroll_line_for(index: usize, crt: bool) -> usize {
    let visible_rows = if crt {
        CRT_VISIBLE_ROWS
    } else {
        HDMI_VISIBLE_ROWS
    };
    wrapped_lines_for(index, crt)
        .len()
        .saturating_sub(visible_rows)
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
                max_scroll_line(index) > 0,
                "{} text does not scroll",
                LICENSE_TITLES[index]
            );
            assert!(!wrapped_lines(index).is_empty());
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
                "libpng"
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
    fn crt_lines_fit_the_native_spleen_viewport_and_scroll_to_the_end() {
        for index in 0..LICENSE_TITLES.len() {
            assert!(
                wrapped_lines_for(index, true)
                    .iter()
                    .all(|line| line.chars().count() <= CRT_LINE_COLUMNS),
                "{}",
                LICENSE_TITLES[index]
            );
            assert_eq!(
                max_scroll_line_for(index, true),
                wrapped_lines_for(index, true)
                    .len()
                    .saturating_sub(CRT_VISIBLE_ROWS)
            );
        }
    }
}
