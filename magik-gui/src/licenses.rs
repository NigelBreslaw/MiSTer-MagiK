pub const LICENSE_TITLES: [&str; 5] = [
    "MiSTer MagiK",
    "Made with Slint",
    "Rust Libraries",
    "FFmpeg",
    "Press Start 2P",
];

const GPL3: &str = include_str!("../../LICENSE");
const RUST_LIBRARIES: &str = include_str!("../licenses/RUST-LIBRARIES.txt");
const FFMPEG: &str = include_str!("../licenses/FFMPEG.txt");
const PRESS_START_2P: &str = include_str!("../licenses/PRESS-START-2P.txt");

pub fn text(index: usize) -> &'static str {
    match index {
        0 => GPL3,
        1 => concat!(
            "Made with Slint\n\n",
            "MiSTer MagiK uses Slint 1.17.0 under Slint's GPL-3.0-only option. ",
            "Slint is Copyright (c) SixtyFPS GmbH and Slint contributors.\n\n",
            include_str!("../../LICENSE")
        ),
        2 => RUST_LIBRARIES,
        3 => FFMPEG,
        _ => PRESS_START_2P,
    }
}

pub fn visible_text(index: usize, first_line: usize) -> String {
    text(index)
        .lines()
        .skip(first_line)
        .take(80)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn max_scroll_line(index: usize) -> usize {
    text(index).lines().count().saturating_sub(20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_major_license_has_full_text_and_can_scroll() {
        for index in 0..LICENSE_TITLES.len() {
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
            assert!(!visible_text(index, 0).is_empty());
        }
    }

    #[test]
    fn generated_runtime_inventory_covers_key_release_dependencies() {
        for expected in ["slint 1.17.0", "ffmpeg-next 8.1.0", "serde 1.0.228"] {
            assert!(RUST_LIBRARIES.contains(expected), "missing {expected}");
        }
        assert!(RUST_LIBRARIES.contains("Only normal runtime dependencies are included"));
        assert!(!RUST_LIBRARIES.contains("Full license text: SPDX identifier above"));
    }
}
