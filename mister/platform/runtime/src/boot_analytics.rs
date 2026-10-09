// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opt-in boot analytics for the Main->Slint handoff.
//!
//! Enabled only by `MISTER_BOOT_ANALYTICS=1`, which the Main fork injects when
//! `/media/fat/mister-magik/boot-analytics.enabled` exists.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

const OUT_PATH: &str = "/tmp/mister-magik-boot-analytics.tsv";

static SEQ: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool {
    enabled_from_value(std::env::var("MISTER_BOOT_ANALYTICS").ok().as_deref())
}

fn enabled_from_value(value: Option<&str>) -> bool {
    matches!(
        value.map(str::to_ascii_lowercase),
        Some(s) if s == "1" || s == "true" || s == "yes"
    )
}

pub fn event(name: &str, detail: impl std::fmt::Display) {
    let detail = detail.to_string();
    #[cfg(feature = "app-runtime")]
    crate::runtime_status::event(name, &detail);

    if !enabled() {
        return;
    }

    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let boot_ms = boot_ms();
    let pid = std::process::id();
    let detail = sanitize(&detail);
    let needs_header = std::fs::metadata(OUT_PATH)
        .map(|m| m.len() == 0)
        .unwrap_or(true);

    match OpenOptions::new().create(true).append(true).open(OUT_PATH) {
        Ok(mut f) => {
            if needs_header {
                let _ = writeln!(f, "seq\tsource\tboot_ms\tevent\tpid\tdetails");
            }
            let _ = writeln!(f, "{seq}\tslint\t{boot_ms}\t{name}\t{pid}\t{detail}");
        }
        Err(e) => {
            crate::ui_errln!("boot_analytics: open {OUT_PATH}: {e}");
        }
    }
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

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\t' | '\n' | '\r' => ' ',
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_accepts_only_explicit_truthy_values() {
        assert!(enabled_from_value(Some("YeS")));
        assert!(!enabled_from_value(Some("0")));
        assert!(!enabled_from_value(Some("false")));
        assert!(!enabled_from_value(None));
    }

    #[test]
    fn sanitize_keeps_tsv_rows_single_line() {
        assert_eq!(sanitize("a\tb\nc\rd"), "a b c d");
    }
}
