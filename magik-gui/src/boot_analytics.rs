//! Opt-in boot analytics for the Main->Slint handoff.
//!
//! Enabled only by `MISTER_BOOT_ANALYTICS=1`, which the Main fork injects when
//! `/media/fat/mister-magik/boot-analytics.enabled` exists.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

const OUT_PATH: &str = "/tmp/mister-magik-boot-analytics.tsv";

static SEQ: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool {
    matches!(
        std::env::var("MISTER_BOOT_ANALYTICS")
            .ok()
            .map(|s| s.to_ascii_lowercase()),
        Some(s) if s == "1" || s == "true" || s == "yes"
    )
}

pub fn event(name: &str, detail: impl std::fmt::Display) {
    let detail = detail.to_string();
    crate::runtime_status::event(name, &detail);

    if !enabled() {
        return;
    }

    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let boot_ms = boot_ms();
    let pid = unsafe { libc::getpid() };
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
            eprintln!("boot_analytics: open {OUT_PATH}: {e}");
        }
    }
}

pub struct LauncherFrameWriter {
    file: File,
    limit: u64,
}

impl LauncherFrameWriter {
    pub fn from_env() -> Option<Self> {
        if !enabled() {
            return None;
        }
        let path = std::env::var("MISTER_BOOT_FRAME_PROFILE_FILE")
            .unwrap_or_else(|_| "/tmp/mister-magik-launcher-frame-profile.tsv".to_string());
        let limit = std::env::var("MISTER_BOOT_FRAME_PROFILE_FRAMES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);
        match File::create(&path) {
            Ok(mut file) => {
                let _ = writeln!(
                    file,
                    "frame\tboot_ms\tanim_us\trender_us\tvsync_us\tcopy_us\trows\treasserted\tedge1_hash\tedge1_nonzero\tedge8_hash\tedge8_nonzero\tleft8_hash\tleft8_nonzero\ttop8_hash\ttop8_nonzero\tbottom8_hash\tbottom8_nonzero\tfull_sample_hash\tfull_sample_nonzero"
                );
                event(
                    "launcher_frame_profile_start",
                    format!("path={path} limit={limit}"),
                );
                Some(Self { file, limit })
            }
            Err(e) => {
                event(
                    "launcher_frame_profile_failed",
                    format!("path={path} error={e}"),
                );
                None
            }
        }
    }

    pub fn should_record(&self, frame: u64) -> bool {
        frame < self.limit
    }

    pub fn record(
        &mut self,
        frame: u64,
        anim_us: u64,
        render_us: u64,
        vsync_us: u64,
        copy_us: u64,
        rows: u32,
        reasserted: bool,
        edge1_hash: u64,
        edge1_nonzero: u32,
        edge8_hash: u64,
        edge8_nonzero: u32,
        left8_hash: u64,
        left8_nonzero: u32,
        top8_hash: u64,
        top8_nonzero: u32,
        bottom8_hash: u64,
        bottom8_nonzero: u32,
        full_sample_hash: u64,
        full_sample_nonzero: u32,
    ) {
        if !self.should_record(frame) {
            return;
        }
        let _ = writeln!(
            self.file,
            "{frame}\t{}\t{anim_us}\t{render_us}\t{vsync_us}\t{copy_us}\t{rows}\t{}\t{edge1_hash:016x}\t{edge1_nonzero}\t{edge8_hash:016x}\t{edge8_nonzero}\t{left8_hash:016x}\t{left8_nonzero}\t{top8_hash:016x}\t{top8_nonzero}\t{bottom8_hash:016x}\t{bottom8_nonzero}\t{full_sample_hash:016x}\t{full_sample_nonzero}",
            boot_ms(),
            u8::from(reasserted),
        );
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
