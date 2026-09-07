// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared archive records and source normalization. No catalog database or builder.
pub use crate::catalog_config::{
    default_hbmame_sqlite_path, default_mame_sqlite_path, default_sqlite_path,
};
use crate::launch_profiles::PayloadRule;
pub(crate) use crate::sqlite_support::{
    open_sqlite_read_only, sqlite_column_exists, sqlite_table_exists,
};
use std::path::Path;

pub(crate) const AMIGAVISION_GAME_LAUNCH_PREFIX: &str = "magik-amigavision:";
pub(crate) const AMIGAVISION_LAUNCHER_REF: &str = "magik-amigavision-launcher";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LibraryContainerEntry {
    pub file_path: String,
    pub entry_path: String,
    pub normalized_title: String,
    pub profile_id: String,
    pub rule: PayloadRule,
    pub compressed_size: Option<u64>,
    pub uncompressed_size: Option<u64>,
    pub crc32: Option<u32>,
    pub launchable: bool,
    pub launch_ref: String,
}

pub(crate) fn canonical_variant_title(title: &str) -> String {
    let base_title = variant_title_without_annotations(title);
    if base_title.is_empty() {
        String::new()
    } else {
        normalize_id(&base_title)
    }
}

fn variant_title_without_annotations(title: &str) -> String {
    let mut out = String::new();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    for ch in title.chars() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ if paren_depth == 0 && bracket_depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.trim_matches(|ch: char| ch.is_whitespace() || ch == '-' || ch == ',')
        .to_string()
}

pub(crate) fn normalize_id(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

pub(crate) fn report_library_scan_timing(stage: &str, us: u64, detail: impl std::fmt::Display) {
    crate::catalog_logln!("library_scan_timing\t{stage}\t{us}\t{detail}");
}

#[cfg(any(test, feature = "builder"))]
pub(crate) fn normalize_title(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_ascii_lowercase()
}

pub(crate) fn title_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .trim()
        .to_string()
}

pub(crate) fn find_eocd(buf: &[u8]) -> Option<usize> {
    if buf.len() < 22 {
        return None;
    }
    (0..=buf.len() - 22).rev().find(|&idx| {
        buf[idx..idx + 4] == [0x50, 0x4b, 0x05, 0x06]
            && idx + 22 + le_u16(&buf[idx + 20..idx + 22]) as usize == buf.len()
    })
}

pub(crate) fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

pub(crate) fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(any(test, feature = "builder"))]
pub(crate) fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

#[cfg(feature = "builder")]
pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(crate) fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_canonicalization_preserves_existing_family_rules() {
        for (title, expected) in [
            ("Example (World) [Rev A]", "example"),
            ("Example (Hack (Unofficial))", "example"),
            (" - Example, ", "example"),
            ("[Only annotation]", ""),
            ("", ""),
            ("Élite", "lite"),
        ] {
            assert_eq!(canonical_variant_title(title), expected, "{title}");
        }
        assert_eq!(normalize_id(""), "unknown");
    }
}
