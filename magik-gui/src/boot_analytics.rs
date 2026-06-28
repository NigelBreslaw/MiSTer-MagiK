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

    #[allow(clippy::too_many_arguments)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // Environment variables are process-global, so keep these tests serialized.
    struct EnvScope {
        originals: Vec<(&'static str, Option<String>)>,
        _guard: MutexGuard<'static, ()>,
    }

    impl EnvScope {
        fn new() -> Self {
            let guard = ENV_LOCK.lock().expect("lock env");
            Self {
                originals: Vec::new(),
                _guard: guard,
            }
        }

        fn set(&mut self, key: &'static str, value: &str) {
            if !self
                .originals
                .iter()
                .any(|(existing_key, _)| *existing_key == key)
            {
                self.originals.push((key, std::env::var(key).ok()));
            }
            std::env::set_var(key, value);
        }
    }

    impl Drop for EnvScope {
        fn drop(&mut self) {
            for (key, original) in self.originals.iter().rev() {
                match original {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn enabled_accepts_only_explicit_truthy_values() {
        let mut env = EnvScope::new();
        env.set("MISTER_BOOT_ANALYTICS", "YeS");
        assert!(enabled());

        std::env::set_var("MISTER_BOOT_ANALYTICS", "0");
        assert!(!enabled());

        std::env::set_var("MISTER_BOOT_ANALYTICS", "false");
        assert!(!enabled());
    }

    #[test]
    fn sanitize_keeps_tsv_rows_single_line() {
        assert_eq!(sanitize("a\tb\nc\rd"), "a b c d");
    }

    #[test]
    fn launcher_frame_writer_records_until_limit() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-frame-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp dir");
        let profile_path = root.join("frames.tsv");
        let mut env = EnvScope::new();
        env.set("MISTER_BOOT_ANALYTICS", "1");
        env.set(
            "MISTER_BOOT_FRAME_PROFILE_FILE",
            profile_path.to_str().expect("utf8 path"),
        );
        env.set("MISTER_BOOT_FRAME_PROFILE_FRAMES", "1");

        {
            let mut writer = LauncherFrameWriter::from_env().expect("profile writer");
            assert!(writer.should_record(0));
            assert!(!writer.should_record(1));
            writer.record(
                0, 1, 2, 3, 4, 5, true, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17,
            );
            writer.record(
                1, 21, 22, 23, 24, 25, false, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37,
            );
        }

        let rows = std::fs::read_to_string(&profile_path).expect("read frame profile");
        assert_eq!(rows.lines().count(), 2);
        assert!(rows.lines().next().unwrap().starts_with("frame\tboot_ms"));
        assert!(rows.contains("\t1\t2\t3\t4\t5\t1\t0000000000000006"));
        assert!(!rows.contains("\t21\t22\t23\t24\t25\t0\t000000000000001a"));
        let _ = std::fs::remove_dir_all(root);
    }
}
