// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Loss-minimizing mutation of MiSTer.ini files.

use std::fmt;

pub const MAX_INI_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    TooLarge { bytes: usize },
    InvalidUtf8,
    InteriorNul,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes } => {
                write!(formatter, "MiSTer.ini is too large ({bytes} bytes)")
            }
            Self::InvalidUtf8 => formatter.write_str("MiSTer.ini is not valid UTF-8"),
            Self::InteriorNul => formatter.write_str("MiSTer.ini contains a NUL byte"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    lines: Vec<String>,
    newline: &'static str,
    final_newline: bool,
    bom: bool,
}

impl Document {
    pub fn parse(input: &[u8]) -> Result<Self, Error> {
        if input.len() > MAX_INI_BYTES {
            return Err(Error::TooLarge { bytes: input.len() });
        }
        if input.contains(&0) {
            return Err(Error::InteriorNul);
        }
        let text = std::str::from_utf8(input).map_err(|_| Error::InvalidUtf8)?;
        let (bom, text) = match text.strip_prefix('\u{feff}') {
            Some(text) => (true, text),
            None => (false, text),
        };
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let final_newline = text.ends_with('\n');
        let lines = text
            .lines()
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect();
        Ok(Self {
            lines,
            newline,
            final_newline,
            bom,
        })
    }

    pub fn effective_value(&self, section: &str, key: &str) -> Option<String> {
        let mut current = String::new();
        let mut value = None;
        for line in &self.lines {
            if let Some(name) = section_name(line) {
                current = name;
            } else if current.eq_ignore_ascii_case(section) && active_key_eq(line, key) {
                value = assignment_value(line);
            }
        }
        value
    }

    pub fn active_count(&self, section: &str, key: &str) -> usize {
        let mut current = String::new();
        let mut count = 0;
        for line in &self.lines {
            if let Some(name) = section_name(line) {
                current = name;
            } else if current.eq_ignore_ascii_case(section) && active_key_eq(line, key) {
                count += 1;
            }
        }
        count
    }

    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let mut current = String::new();
        let mut first = None;
        let mut saw_section = false;
        let mut insert_at = None;

        for (index, line) in self.lines.iter().enumerate() {
            if let Some(name) = section_name(line) {
                if current.eq_ignore_ascii_case(section) {
                    insert_at = Some(index);
                }
                current = name;
                saw_section |= current.eq_ignore_ascii_case(section);
            } else if current.eq_ignore_ascii_case(section) && active_key_eq(line, key) {
                first.get_or_insert(index);
            }
        }
        if current.eq_ignore_ascii_case(section) {
            insert_at = Some(self.lines.len());
        }

        if let Some(first_index) = first {
            self.lines[first_index] = replace_assignment_value(&self.lines[first_index], value);
            let mut current = String::new();
            for index in 0..self.lines.len() {
                if let Some(name) = section_name(&self.lines[index]) {
                    current = name;
                } else if index != first_index
                    && current.eq_ignore_ascii_case(section)
                    && active_key_eq(&self.lines[index], key)
                {
                    self.lines[index] = format!(";{}", self.lines[index]);
                }
            }
            return;
        }

        if saw_section {
            self.lines.insert(
                insert_at.unwrap_or(self.lines.len()),
                format!("{key}={value}"),
            );
        } else {
            if self
                .lines
                .last()
                .is_some_and(|line| !line.trim().is_empty())
            {
                self.lines.push(String::new());
            }
            self.lines.push(format!("[{section}]"));
            self.lines.push(format!("{key}={value}"));
        }
    }

    pub fn remove(&mut self, section: &str, key: &str, reason: &str) {
        let mut current = String::new();
        for line in &mut self.lines {
            if let Some(name) = section_name(line) {
                current = name;
            } else if current.eq_ignore_ascii_case(section) && active_key_eq(line, key) {
                *line = format!(";{line} ; {reason}");
            }
        }
    }

    pub fn comment_if_value(&mut self, section: &str, key: &str, values: &[&str], reason: &str) {
        let mut current = String::new();
        for line in &mut self.lines {
            if let Some(name) = section_name(line) {
                current = name;
            } else if current.eq_ignore_ascii_case(section)
                && active_key_eq(line, key)
                && assignment_value(line).is_some_and(|value| {
                    values
                        .iter()
                        .any(|expected| value.eq_ignore_ascii_case(expected))
                })
            {
                *line = format!(";{line} ; {reason}");
            }
        }
    }

    pub fn ensure_section_after(&mut self, earlier: &str, later: &str) {
        let Some(earlier_range) = section_range(&self.lines, earlier) else {
            return;
        };
        let Some(later_range) = section_range(&self.lines, later) else {
            return;
        };
        if earlier_range.start < later_range.start {
            return;
        }
        let later_len = later_range.end - later_range.start;
        let moved: Vec<_> = self.lines.drain(later_range).collect();
        let insertion = earlier_range.end.saturating_sub(later_len);
        self.lines.splice(insertion..insertion, moved);
    }

    pub fn render(&self) -> Vec<u8> {
        let mut output = if self.bom {
            String::from("\u{feff}")
        } else {
            String::new()
        };
        output.push_str(&self.lines.join(self.newline));
        if self.final_newline {
            output.push_str(self.newline);
        }
        output.into_bytes()
    }
}

pub fn apply_install(document: &mut Document) {
    document.set("MiSTer", "main", "MiSTer_MagiK");
}

pub fn apply_restore(document: &mut Document, backup: Option<&Document>) {
    if let Some(value) = backup.and_then(|source| source.effective_value("MiSTer", "main")) {
        document.set("MiSTer", "main", &value);
    } else if backup.is_some() {
        document.remove("MiSTer", "main", "MiSTer MagiK restored absent value");
    } else {
        document.set("MiSTer", "main", "MiSTer");
    }
}

fn section_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with(';') || trimmed.starts_with('#') || !trimmed.starts_with('[') {
        return None;
    }
    let end = trimmed.find(']')?;
    Some(trimmed[1..end].trim().to_string())
}

fn active_key_eq(line: &str, expected: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
        return false;
    }
    trimmed
        .split_once('=')
        .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case(expected))
}

fn assignment_value(line: &str) -> Option<String> {
    Some(
        line.split_once('=')?
            .1
            .split([';', '#'])
            .next()?
            .trim()
            .to_string(),
    )
}

fn replace_assignment_value(line: &str, value: &str) -> String {
    let Some(eq) = line.find('=') else {
        return line.to_string();
    };
    let after_eq = &line[eq + 1..];
    let value_start = after_eq
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(after_eq.len());
    let value_and_comment = &after_eq[value_start..];
    let comment = value_and_comment.find([';', '#']).map_or("", |position| {
        let before = &value_and_comment[..position];
        let whitespace = before.trim_end().len();
        &value_and_comment[whitespace..]
    });
    format!("{}{}{}", &line[..eq + 1 + value_start], value, comment)
}

fn section_range(lines: &[String], section: &str) -> Option<std::ops::Range<usize>> {
    let start = lines.iter().position(|line| {
        section_name(line).is_some_and(|name| name.eq_ignore_ascii_case(section))
    })?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| section_name(line).is_some())
        .map(|offset| start + 1 + offset)
        .unwrap_or(lines.len());
    Some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_value_uses_last_active_assignment_across_repeated_sections() {
        let document =
            Document::parse(b"[MiSTer]\nmain=First\n[Menu]\nx=1\n[mister]\nMAIN=Last ; note\n")
                .unwrap();
        assert_eq!(
            document.effective_value("MiSTer", "main").as_deref(),
            Some("Last")
        );
    }

    #[test]
    fn set_preserves_first_context_and_comments_later_duplicates() {
        let mut document = Document::parse(
            b"[MiSTer]\r\n MAIN = Old ; first\r\nfoo=keep\r\n[mister]\r\nmain=Later # context\r\n",
        )
        .unwrap();
        document.set("MiSTer", "main", "MiSTer_MagiK");
        assert_eq!(document.active_count("MiSTer", "main"), 1);
        assert_eq!(
            document.effective_value("MiSTer", "main").as_deref(),
            Some("MiSTer_MagiK")
        );
        assert_eq!(document.render(), b"[MiSTer]\r\n MAIN = MiSTer_MagiK ; first\r\nfoo=keep\r\n[mister]\r\n;main=Later # context\r\n");
    }

    #[test]
    fn install_is_idempotent_and_only_deduplicates_main() {
        let input = b"[MiSTer]\nmain=MiSTer\nmain=Other\n[Menu]\ndirect_video=9\ndirect_video=8\nmenu_pal=9\nforced_scandoubler=9\ncustom=keep\n";
        let mut once = Document::parse(input).unwrap();
        apply_install(&mut once);
        let rendered = once.render();
        let mut twice = Document::parse(&rendered).unwrap();
        apply_install(&mut twice);
        assert_eq!(twice.render(), rendered);
        assert_eq!(twice.active_count("MiSTer", "main"), 1);
        assert_eq!(twice.active_count("Menu", "direct_video"), 2);
        assert!(String::from_utf8(rendered).unwrap().contains("custom=keep"));
    }

    #[test]
    fn restore_uses_backup_values_without_losing_later_user_lines() {
        let mut live = Document::parse(b"[MiSTer]\nmain=MiSTer_MagiK\n[Menu]\ndirect_video=2\nmenu_pal=0\nforced_scandoubler=0\nuser=keep\n").unwrap();
        let backup = Document::parse(b"[MiSTer]\nmain=Other\n[Menu]\ndirect_video=1\n").unwrap();
        apply_restore(&mut live, Some(&backup));
        let output = String::from_utf8(live.render()).unwrap();
        assert!(output.contains("main=Other"));
        assert!(output.contains("direct_video=2"));
        assert!(output.contains("menu_pal=0"));
        assert!(output.contains("user=keep"));
    }

    #[test]
    fn restore_without_backup_selects_stock_without_removing_user_settings() {
        let mut live =
            Document::parse(b"[MiSTer]\nmain=MiSTer_MagiK\n[Menu]\ndirect_video=2\nuser=keep\n")
                .unwrap();
        apply_restore(&mut live, None);
        assert_eq!(
            live.effective_value("MiSTer", "main").as_deref(),
            Some("MiSTer")
        );
        assert_eq!(
            live.effective_value("Menu", "direct_video").as_deref(),
            Some("2")
        );
        assert!(
            String::from_utf8(live.render())
                .unwrap()
                .contains("user=keep")
        );
    }

    #[test]
    fn hostile_encodings_are_rejected() {
        assert_eq!(Document::parse(b"a=\0b"), Err(Error::InteriorNul));
        assert_eq!(Document::parse(&[0xff]), Err(Error::InvalidUtf8));
        assert!(matches!(
            Document::parse(&vec![b'x'; MAX_INI_BYTES + 1]),
            Err(Error::TooLarge { .. })
        ));
    }

    #[test]
    fn every_conflicting_duplicate_is_made_inactive_without_losing_text() {
        let mut input = String::from("[Menu] ; first\n");
        for index in 0..64 {
            input.push_str(&format!(" DiReCt_ViDeO = {index} ; user note {index}\n"));
            if index % 8 == 7 {
                input.push_str("[Other]\nuntouched=yes\n[menu]\n");
            }
        }
        let mut document = Document::parse(input.as_bytes()).unwrap();
        document.set("Menu", "direct_video", "2");
        let output = String::from_utf8(document.render()).unwrap();
        assert_eq!(document.active_count("Menu", "direct_video"), 1);
        assert_eq!(
            document.effective_value("Menu", "direct_video").as_deref(),
            Some("2")
        );
        for index in 0..64 {
            assert!(output.contains(&format!("user note {index}")));
        }
        assert_eq!(output.matches("untouched=yes").count(), 8);
    }

    #[test]
    fn malformed_headers_and_commented_assignments_are_never_claimed() {
        let input = b"[MiSTer\nmain=decoy\n; [MiSTer]\nmain=also-decoy\n[MiSTer] ; real\n#main=commented\nmain=MiSTer\n";
        let mut document = Document::parse(input).unwrap();
        document.set("MiSTer", "main", "MiSTer_MagiK");
        let output = String::from_utf8(document.render()).unwrap();
        assert!(output.contains("main=decoy"));
        assert!(output.contains("main=also-decoy"));
        assert!(output.contains("#main=commented"));
        assert!(output.ends_with("main=MiSTer_MagiK\n"));
    }

    #[test]
    fn no_final_newline_is_preserved_across_idempotent_edits() {
        let mut document = Document::parse(b"[Menu]\ndirect_video=0").unwrap();
        document.set("Menu", "direct_video", "2");
        let once = document.render();
        assert!(!once.ends_with(b"\n"));
        let mut document = Document::parse(&once).unwrap();
        document.set("Menu", "direct_video", "2");
        assert_eq!(document.render(), once);
    }

    #[test]
    fn bom_is_understood_and_preserved() {
        let mut document = Document::parse(b"\xef\xbb\xbf[MiSTer]\r\nmain=MiSTer\r\n").unwrap();
        document.set("MiSTer", "main", "MiSTer_MagiK");
        assert_eq!(
            document.render(),
            b"\xef\xbb\xbf[MiSTer]\r\nmain=MiSTer_MagiK\r\n"
        );
    }

    #[test]
    fn mixed_endings_use_crlf_without_leaving_stray_carriage_returns() {
        let mut document =
            Document::parse(b"[MiSTer]\r\nmain=MiSTer\n[Menu]\r\ndirect_video=0").unwrap();
        document.set("Menu", "direct_video", "2");
        let output = document.render();
        assert_eq!(
            output,
            b"[MiSTer]\r\nmain=MiSTer\r\n[Menu]\r\ndirect_video=2"
        );
        assert!(!output.windows(2).any(|bytes| bytes == b"\r\r"));
    }

    #[test]
    fn long_lines_are_preserved_below_the_document_limit() {
        let note = "x".repeat(256 * 1024);
        let input = format!("[MiSTer]\nmain=MiSTer\nuser={note}\n");
        let mut document = Document::parse(input.as_bytes()).unwrap();
        document.set("MiSTer", "main", "MiSTer_MagiK");
        assert!(
            String::from_utf8(document.render())
                .unwrap()
                .contains(&note)
        );
    }

    #[test]
    fn generated_installs_preserve_unrelated_lines_and_converge() {
        for seed in 0_usize..128 {
            let mut input = String::from("[MiSTer]\n");
            for duplicate in 0..=(seed % 11) {
                input.push_str(&format!("main=value-{seed}-{duplicate} ; context\n"));
            }
            input.push_str("[Menu]\n");
            for duplicate in 0..=(seed % 17) {
                input.push_str(&format!("direct_video={duplicate} ; video-context\n"));
            }
            input.push_str(&format!("user_seed_{seed}=keep-{seed}\n"));
            let menu_offset = input.find("[Menu]").unwrap();
            let expected_menu_tail = input.as_bytes()[menu_offset..].to_vec();

            let mut once = Document::parse(input.as_bytes()).unwrap();
            apply_install(&mut once);
            let rendered = once.render();
            let mut twice = Document::parse(&rendered).unwrap();
            apply_install(&mut twice);

            assert_eq!(twice.render(), rendered);
            assert_eq!(twice.active_count("MiSTer", "main"), 1);
            assert_eq!(twice.active_count("Menu", "direct_video"), seed % 17 + 1);
            let rendered_menu_offset = rendered
                .windows(b"[Menu]".len())
                .position(|window| window == b"[Menu]")
                .unwrap();
            assert_eq!(&rendered[rendered_menu_offset..], expected_menu_tail);
            assert_eq!(
                twice.effective_value("MiSTer", "main").as_deref(),
                Some("MiSTer_MagiK")
            );
            assert!(
                String::from_utf8(rendered)
                    .unwrap()
                    .contains(&format!("user_seed_{seed}=keep-{seed}"))
            );
        }
    }

    #[test]
    fn generated_restore_uses_backup_without_replacing_live_context() {
        for seed in 0..64 {
            let backup_text = format!(
                "[MiSTer]\nmain=stock-{seed}\n[Menu]\ndirect_video={}\n",
                seed % 3
            );
            let live_text = format!(
                "[MiSTer]\nmain=MiSTer_MagiK\n[Menu]\ndirect_video=2\npost_install_{seed}=keep\n"
            );
            let backup = Document::parse(backup_text.as_bytes()).unwrap();
            let mut live = Document::parse(live_text.as_bytes()).unwrap();
            apply_restore(&mut live, Some(&backup));
            let expected = format!("stock-{seed}");
            assert_eq!(
                live.effective_value("MiSTer", "main").as_deref(),
                Some(expected.as_str())
            );
            assert!(
                String::from_utf8(live.render())
                    .unwrap()
                    .contains(&format!("post_install_{seed}=keep"))
            );
        }
    }

    #[test]
    fn mutators_add_missing_keys_and_preserve_requested_section_order() {
        let mut document =
            Document::parse(b"[arcade_vertical]\nvideo_mode=8\n[arcade]\ncore=keep\n").unwrap();

        document.set("arcade", "direct_video", "1");
        document.set("Menu", "video_mode", "6");
        document.ensure_section_after("arcade", "arcade_vertical");

        let output = String::from_utf8(document.render()).unwrap();
        assert!(output.starts_with("[arcade]\ncore=keep\ndirect_video=1\n"));
        assert!(output.find("[arcade]").unwrap() < output.find("[arcade_vertical]").unwrap());
        assert!(output.ends_with("[Menu]\nvideo_mode=6\n"));

        let stable = document.render();
        document.ensure_section_after("arcade", "arcade_vertical");
        document.ensure_section_after("missing", "arcade_vertical");
        document.ensure_section_after("arcade", "missing");
        assert_eq!(document.render(), stable);
    }

    #[test]
    fn selective_commenting_and_removal_leave_unmatched_settings_active() {
        let mut document = Document::parse(
            b"[Menu]\nvideo_mode=8 ; owned\ndirect_video=2\nuser=keep\n[Other]\nvideo_mode=8\n",
        )
        .unwrap();

        document.comment_if_value("Menu", "video_mode", &["7", "8"], "restored");
        document.comment_if_value("Menu", "user", &["different"], "not-used");
        document.remove("Menu", "direct_video", "removed");

        let output = String::from_utf8(document.render()).unwrap();
        assert!(output.contains(";video_mode=8 ; owned ; restored"));
        assert!(output.contains(";direct_video=2 ; removed"));
        assert!(output.contains("user=keep"));
        assert!(output.contains("[Other]\nvideo_mode=8"));
        assert_eq!(document.active_count("Menu", "video_mode"), 0);
        assert_eq!(document.active_count("Menu", "direct_video"), 0);
        assert_eq!(document.active_count("Menu", "user"), 1);
    }

    #[test]
    fn restore_with_backup_missing_main_restores_the_absence() {
        let mut live = Document::parse(b"[MiSTer]\nmain=MiSTer_MagiK\n").unwrap();
        let backup = Document::parse(b"[Menu]\nvideo_mode=6\n").unwrap();

        apply_restore(&mut live, Some(&backup));

        assert_eq!(live.effective_value("MiSTer", "main"), None);
        assert!(
            String::from_utf8(live.render())
                .unwrap()
                .contains(";main=MiSTer_MagiK ; MiSTer MagiK restored absent value")
        );
    }

    #[test]
    fn parse_errors_have_actionable_messages() {
        assert_eq!(
            Error::TooLarge { bytes: 42 }.to_string(),
            "MiSTer.ini is too large (42 bytes)"
        );
        assert_eq!(
            Error::InvalidUtf8.to_string(),
            "MiSTer.ini is not valid UTF-8"
        );
        assert_eq!(
            Error::InteriorNul.to_string(),
            "MiSTer.ini contains a NUL byte"
        );
    }
}
