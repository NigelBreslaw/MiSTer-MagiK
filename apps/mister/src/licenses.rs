// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::OnceLock;

// Directly shipped third-party assets remain visible in the in-app legal surface.
pub const LICENSE_TITLES: [&str; 7] = [
    "MiSTer MagiK",
    "FFmpeg",
    "Press Start 2P",
    "Yesterday 10 + Xerxes 10",
    "Jersey 25",
    "Arcade Cabinet",
    "Slint",
];

const GPL3: &str = include_str!("../../../LICENSE");
const FFMPEG: &str = include_str!("../licenses/FFMPEG.txt");
const PRESS_START_2P: &str = include_str!("../licenses/PRESS-START-2P.txt");
const COMMERCIAL_FONTS: &str = include_str!("../licenses/COMMERCIAL-FONTS.txt");
const JERSEY_25: &str = include_str!("../licenses/JERSEY-25.txt");
const ARCADE_CABINET: &str =
    include_str!("../../../crates/particles/assets/cabinet/arcade-cabinet.LICENSE.txt");
const LICENSE_LINE_COLUMNS: usize = 105;
const LICENSE_VISIBLE_ROWS: usize = 40;

pub fn text(index: usize) -> &'static str {
    match index {
        0 | 6 => GPL3,
        1 => FFMPEG,
        2 => PRESS_START_2P,
        3 => COMMERCIAL_FONTS,
        4 => JERSEY_25,
        _ => ARCADE_CABINET,
    }
}

pub fn wrapped_lines(index: usize) -> &'static [String] {
    static LINES: [OnceLock<Vec<String>>; 7] = [const { OnceLock::new() }; 7];
    let index = index.min(LICENSE_TITLES.len() - 1);
    LINES[index].get_or_init(|| wrap_text(index))
}

fn wrap_text(index: usize) -> Vec<String> {
    let mut result = Vec::new();
    for source_line in text(index).lines() {
        if source_line.trim().is_empty() {
            result.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in source_line.split_whitespace() {
            let word_len = word.chars().count();
            if !line.is_empty() && line.chars().count() + 1 + word_len > LICENSE_LINE_COLUMNS {
                result.push(std::mem::take(&mut line));
            }
            if word_len > LICENSE_LINE_COLUMNS {
                if !line.is_empty() {
                    result.push(std::mem::take(&mut line));
                }
                let chars = word.chars().collect::<Vec<_>>();
                for chunk in chars.chunks(LICENSE_LINE_COLUMNS) {
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
    wrapped_lines(index)
        .len()
        .saturating_sub(LICENSE_VISIBLE_ROWS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_major_license_has_full_text_and_can_scroll() {
        for index in [0, 1, 2, 4, 6] {
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
    }

    #[test]
    fn app_surface_is_limited_to_directly_relevant_license_texts() {
        assert_eq!(
            LICENSE_TITLES,
            [
                "MiSTer MagiK",
                "FFmpeg",
                "Press Start 2P",
                "Yesterday 10 + Xerxes 10",
                "Jersey 25",
                "Arcade Cabinet",
                "Slint"
            ]
        );
        assert_eq!(text(6), GPL3);
        assert!(FFMPEG.contains("FFmpeg 8.1.2"));
        assert!(PRESS_START_2P.contains("SIL Open Font License"));
        assert!(COMMERCIAL_FONTS.contains("commercial licences"));
        assert!(JERSEY_25.contains("SIL OPEN FONT LICENSE"));
        assert!(ARCADE_CABINET.contains("Lluc Guardiolaa"));
        assert!(ARCADE_CABINET.contains("CC-BY-NC-4.0"));
    }
}
