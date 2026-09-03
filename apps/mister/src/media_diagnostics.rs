// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Local-only, bounded screenshot support journal. Producers never perform IO
//! or wait for the writer. A full queue drops events and counts the loss.

use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_BYTES: usize = 64 * 1024;
const MAX_EVENTS: usize = 24;
static DROPPED: AtomicU64 = AtomicU64::new(0);
static WRITER: OnceLock<Option<SyncSender<Event>>> = OnceLock::new();

struct Event {
    name: String,
    detail: String,
    failure: bool,
    at: u64,
}

/// Strip query strings, URL credentials and control characters before either
/// memory retention or disk writes. Callers supply only media-specific data.
pub(crate) fn sanitize(detail: &str) -> String {
    let mut text = detail
        .split_whitespace()
        .take(80)
        .map(|word| {
            let word = word.split(['?', '#']).next().unwrap_or("");
            if let Some((prefix, rest)) = word.split_once("://") {
                let rest = rest.rsplit_once('@').map_or(rest, |(_, tail)| tail);
                format!("{prefix}://{rest}")
            } else if word.to_ascii_lowercase().contains("token=")
                || word.to_ascii_lowercase().contains("password=")
                || word.to_ascii_lowercase().contains("authorization")
            {
                "[redacted]".to_string()
            } else {
                word.chars().filter(|ch| !ch.is_control()).collect()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if text.len() > 768 {
        let mut end = 768;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

pub(crate) fn record(name: &str, detail: impl AsRef<str>, failure: bool) {
    let writer = WRITER.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel(64);
        std::thread::Builder::new()
            .name("media-diagnostics".into())
            .spawn(move || run(receiver))
            .ok()
            .map(|_| sender)
    });
    let event = Event {
        name: name
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
            .take(64)
            .collect(),
        detail: sanitize(detail.as_ref()),
        failure,
        at: unix_ms(),
    };
    if writer
        .as_ref()
        .is_none_or(|writer| writer.try_send(event).is_err())
    {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Journal {
    events: VecDeque<Value>,
    counters: BTreeMap<String, u64>,
    context: BTreeMap<String, String>,
    last_failure: Option<(String, String)>,
    pending_failure: bool,
    suppressed: u64,
    changed: bool,
}

impl Journal {
    fn push(&mut self, event: Event) {
        let key = if self.counters.contains_key(&event.name) || self.counters.len() < 48 {
            event.name.clone()
        } else {
            "other".into()
        };
        *self.counters.entry(key).or_default() += 1;
        if matches!(
            event.name.as_str(),
            "manifest_identity"
                | "preview_configuration"
                | "pack_identity"
                | "preview_presentation_receipt"
                | "screenshot_media_cache_metadata"
        ) {
            self.context
                .insert(event.name.clone(), event.detail.clone());
        }
        if event.failure {
            let key = (event.name.clone(), event.detail.clone());
            if self.last_failure.as_ref() != Some(&key) {
                self.pending_failure = true;
                self.last_failure = Some(key);
            } else {
                self.suppressed += 1;
            }
        }
        self.events.push_back(json!({"name": event.name, "detail": event.detail, "failure": event.failure, "unix_ms": event.at}));
        if self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }
        self.changed = true;
    }

    fn snapshot(&self, boot: &str, session: u64, saved: u64, io_errors: u64) -> Value {
        json!({
            "schema": "mister-magik-media-diagnostics-v1",
            "updated_unix_ms": unix_ms(), "boot_id": boot, "session": session,
            "launcher_pid": std::process::id(),
            "build": crate::build_identity::BuildIdentity::current(),
            "events": self.events, "counters": self.counters, "context": self.context,
            "last_failure": self.last_failure,
            "queue_dropped": DROPPED.load(Ordering::Relaxed),
            "suppressed_failures": self.suppressed, "persistent_attempts_this_boot": saved,
            "storage_errors": io_errors,
            "visibility_assurance": "decode/apply and presentation receipts are not physical display verification",
        })
    }
}

fn run(receiver: mpsc::Receiver<Event>) {
    let tmp = Path::new("/tmp/mister-magik");
    let dir = mister_magik_catalog::device_layout::current_app_path("diagnostics/media");
    let boot =
        fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_else(|_| "host".into());
    let boot = sanitize(&boot);
    let session = unix_ms();
    let ledger_path = tmp.join("media-diagnostics-budget.json");
    let ledger: Value = read_bounded(&ledger_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    let same_boot = ledger["boot_id"].as_str() == Some(boot.as_str());
    let mut saved = if same_boot {
        ledger["attempts"].as_u64().unwrap_or(16)
    } else if ledger.is_null() && ledger_path.exists() {
        16
    } else {
        0
    };
    let mut last_saved_ms = if same_boot {
        ledger["at"].as_u64().unwrap_or(session)
    } else {
        0
    };
    let mut journal = Journal::default();
    let platform_path =
        mister_magik_catalog::device_layout::current_app_path("platform-v3.manifest");
    if let Ok(bytes) = read_bounded(&platform_path) {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        journal.context.insert(
            "platform_manifest".into(),
            format!("path={} sha256={digest}", platform_path.display()),
        );
    }
    let mut last_live = Instant::now();
    let mut io_errors = 0;
    loop {
        let remaining = Duration::from_secs(2).saturating_sub(last_live.elapsed());
        match receiver.recv_timeout(remaining) {
            Ok(event) => journal.push(event),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if last_live.elapsed() < Duration::from_secs(2) {
            continue;
        }
        last_live = Instant::now();
        if !journal.changed {
            continue;
        }
        journal.changed = false;
        if journal.pending_failure {
            journal.pending_failure = false;
            let now = unix_ms();
            if persistent_budget_available(saved, last_saved_ms, now) {
                // Reserve the budget in tmpfs first, even when the SD write fails.
                // A restarted launcher must not restart the persistent-write budget.
                saved += 1;
                last_saved_ms = now;
                let ledger = json!({"boot_id": boot, "attempts": saved, "at": now});
                if atomic_json(&ledger_path, &ledger).is_ok() {
                    let report = journal.snapshot(&boot, session, saved, io_errors);
                    if persist(&dir, &report, now, saved).is_err() {
                        io_errors += 1;
                    }
                } else {
                    io_errors += 1;
                }
            } else {
                journal.suppressed += 1;
            }
        }
        if atomic_json(
            &tmp.join("media-diagnostics.json"),
            &journal.snapshot(&boot, session, saved, io_errors),
        )
        .is_err()
        {
            io_errors += 1;
        }
    }
}

fn persistent_budget_available(saved: u64, last_saved_ms: u64, now: u64) -> bool {
    saved < 16 && now.saturating_sub(last_saved_ms) >= 60_000
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::other("media diagnostic input exceeds bound"));
    }
    Ok(bytes)
}

fn atomic_json(path: &Path, value: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::other("media report exceeds bound"));
    }
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| io::Error::other("no report directory"))?,
    )?;
    let temporary = path.with_extension("json.tmp");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(&bytes)?;
    drop(file);
    fs::rename(temporary, path)
}

fn persist(dir: &Path, report: &Value, now: u64, sequence: u64) -> io::Result<()> {
    atomic_json(
        &dir.join(format!("report-{now:020}-{sequence:02}.json")),
        report,
    )?;
    atomic_json(&dir.join("latest.json"), report)?;
    let mut reports = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("report-") && name.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    reports.sort();
    let remove = reports.len().saturating_sub(5);
    for path in reports.into_iter().take(remove) {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_is_bounded_and_deduplicates_failures() {
        let mut journal = Journal::default();
        for _ in 0..1000 {
            journal.push(Event {
                name: "decode_failed".into(),
                detail: sanitize(&"x".repeat(4000)),
                failure: true,
                at: 0,
            });
        }
        assert_eq!(journal.events.len(), MAX_EVENTS);
        assert_eq!(journal.suppressed, 999);
        assert!(
            serde_json::to_vec(&journal.snapshot("boot", 0, 0, 0))
                .unwrap()
                .len()
                < MAX_BYTES
        );
    }
    #[test]
    fn credentials_queries_and_control_characters_are_removed() {
        assert!(sanitize(&"🎮".repeat(1000)).len() <= 768);
        assert_eq!(
            sanitize("url=https://user:secret@example.org/a?token=secret password=secret\nnext"),
            "url=https://example.org/a [redacted] next"
        );
    }
    #[test]
    fn snapshots_rotate_and_storage_errors_are_returned() {
        let dir = std::env::temp_dir().join(format!(
            "magik-media-report-test-{}-{}",
            std::process::id(),
            unix_ms()
        ));
        for i in 0..8 {
            persist(&dir, &json!({"test": i}), i, i).unwrap();
        }
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 6);
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(dir.join("latest.json")).unwrap()).unwrap()["test"],
            7
        );
        assert!(persist(&dir.join("latest.json"), &json!({}), 9, 9).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn persistent_budget_and_queue_are_fail_closed() {
        assert!(persistent_budget_available(0, 0, 60_000));
        assert!(!persistent_budget_available(1, 60_000, 119_999));
        assert!(!persistent_budget_available(1, 60_000, 10));
        assert!(!persistent_budget_available(16, 0, 999_999));
        let (sender, _receiver) = mpsc::sync_channel(1);
        sender.try_send(1).unwrap();
        assert!(matches!(
            sender.try_send(2),
            Err(mpsc::TrySendError::Full(2))
        ));
    }
}
