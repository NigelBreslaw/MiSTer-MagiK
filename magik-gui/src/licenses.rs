use std::sync::OnceLock;

pub const LICENSE_TITLES: [&str; 3] = ["MiSTer MagiK", "FFmpeg", "Press Start 2P"];

#[cfg(target_arch = "arm")]
const GPL3: &str = include_str!("../LICENSE");
#[cfg(not(target_arch = "arm"))]
const GPL3: &str = include_str!("../../LICENSE");
const FFMPEG: &str = include_str!("../licenses/FFMPEG.txt");
const PRESS_START_2P: &str = include_str!("../licenses/PRESS-START-2P.txt");
const LICENSE_LINE_COLUMNS: usize = 105;
const LICENSE_VISIBLE_ROWS: usize = 40;

pub fn text(index: usize) -> &'static str {
    match index {
        0 => GPL3,
        1 => FFMPEG,
        _ => PRESS_START_2P,
    }
}

pub fn wrapped_lines(index: usize) -> &'static [String] {
    static LINES: [OnceLock<Vec<String>>; 3] = [const { OnceLock::new() }; 3];
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
            assert!(!wrapped_lines(index).is_empty());
        }
    }

    #[test]
    fn app_surface_is_limited_to_directly_relevant_license_texts() {
        assert_eq!(LICENSE_TITLES, ["MiSTer MagiK", "FFmpeg", "Press Start 2P"]);
        assert!(FFMPEG.contains("FFmpeg 8.1.2"));
        assert!(PRESS_START_2P.contains("SIL Open Font License"));
    }
}
