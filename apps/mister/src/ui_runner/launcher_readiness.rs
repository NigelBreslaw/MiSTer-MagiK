// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ConfirmedLatchPost {
    pub(super) sequence: u16,
    pub(super) route_epoch: u16,
    pub(super) slot: u8,
}

impl ConfirmedLatchPost {
    fn valid(self) -> bool {
        matches!(self.slot, 1 | 2)
    }

    fn advances_and_alternates(self, previous: Self) -> bool {
        advances(self.sequence, previous.sequence)
            && advances(self.route_epoch, previous.route_epoch)
            && self.slot != previous.slot
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadyPhase {
    Disabled,
    AwaitingFirst,
    AwaitingSecond(ConfirmedLatchPost),
    PendingSend,
    Sent,
}

pub(super) struct LauncherReadiness {
    phase: ReadyPhase,
    token: String,
    fifo: PathBuf,
    pid: u32,
}

impl LauncherReadiness {
    pub(super) fn from_env() -> Self {
        Self::from_config(
            std::env::var("MISTER_MAGIK_STARTUP_TOKEN").unwrap_or_default(),
            std::env::var_os("MISTER_MAGIK_READY_FIFO")
                .map(PathBuf::from)
                .unwrap_or_default(),
            std::process::id(),
        )
    }

    fn from_config(token: String, fifo: PathBuf, pid: u32) -> Self {
        let configured = valid_token(&token) && !fifo.as_os_str().is_empty() && pid != 0;
        Self {
            phase: if configured {
                ReadyPhase::AwaitingFirst
            } else {
                ReadyPhase::Disabled
            },
            token,
            fifo,
            pid,
        }
    }

    pub(super) fn needs_full_present(&self) -> bool {
        matches!(self.phase, ReadyPhase::AwaitingSecond(_))
    }

    pub(super) fn poll(&mut self) {
        if self.phase == ReadyPhase::PendingSend {
            self.try_send();
        }
    }

    pub(super) fn observe(&mut self, post: ConfirmedLatchPost, intended_for_display: bool) {
        if !intended_for_display || !post.valid() {
            return;
        }
        match self.phase {
            ReadyPhase::AwaitingFirst => self.phase = ReadyPhase::AwaitingSecond(post),
            ReadyPhase::AwaitingSecond(previous) => {
                if post.advances_and_alternates(previous) {
                    self.phase = ReadyPhase::PendingSend;
                    self.try_send();
                } else {
                    self.phase = ReadyPhase::AwaitingSecond(post);
                }
            }
            ReadyPhase::Disabled | ReadyPhase::PendingSend | ReadyPhase::Sent => {}
        }
    }

    fn try_send(&mut self) {
        let line = format!("ready-v1 token={} pid={}\n", self.token, self.pid);
        let sent = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&self.fifo)
            .and_then(|mut fifo| fifo.write(line.as_bytes()))
            .is_ok_and(|written| written == line.len());
        if sent {
            self.phase = ReadyPhase::Sent;
        }
    }
}

fn valid_token(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn advances(current: u16, previous: u16) -> bool {
    let delta = current.wrapping_sub(previous);
    delta != 0 && delta < (1 << 15)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::fs;
    use std::io::{self, Read};
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_FIFO: AtomicU64 = AtomicU64::new(0);
    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    struct TestFifo(PathBuf);

    impl TestFifo {
        fn new() -> Self {
            let serial = NEXT_FIFO.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "mister-magik-ready-{}-{nanos}-{serial}",
                std::process::id(),
            ));
            let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
            Self(path)
        }

        fn controller(&self) -> LauncherReadiness {
            LauncherReadiness::from_config(TOKEN.into(), self.0.clone(), 42)
        }

        fn reader(&self) -> fs::File {
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(&self.0)
                .unwrap()
        }
    }

    impl Drop for TestFifo {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn post(sequence: u16, route_epoch: u16, slot: u8) -> ConfirmedLatchPost {
        ConfirmedLatchPost {
            sequence,
            route_epoch,
            slot,
        }
    }

    fn read_message(reader: &mut fs::File) -> String {
        let mut message = String::new();
        reader.read_to_string(&mut message).unwrap();
        message
    }

    #[test]
    fn absent_reader_keeps_ready_message_pending_for_retry() {
        let fifo = TestFifo::new();
        let mut readiness = fifo.controller();
        readiness.observe(post(1, 1, 1), true);
        readiness.observe(post(2, 2, 2), true);
        assert_eq!(readiness.phase, ReadyPhase::PendingSend);

        let mut reader = fifo.reader();
        readiness.poll();
        assert_eq!(readiness.phase, ReadyPhase::Sent);
        assert_eq!(
            read_message(&mut reader),
            "ready-v1 token=0123456789abcdef0123456789abcdef pid=42\n"
        );
    }

    #[test]
    fn invalid_or_stale_token_configuration_is_disabled() {
        let fifo = TestFifo::new();
        let mut readiness = LauncherReadiness::from_config("stale".into(), fifo.0.clone(), 42);
        readiness.observe(post(1, 1, 1), true);
        readiness.observe(post(2, 2, 2), true);
        assert_eq!(readiness.phase, ReadyPhase::Disabled);
    }

    #[test]
    fn duplicate_posts_do_not_complete_readiness() {
        let fifo = TestFifo::new();
        let mut readiness = fifo.controller();
        readiness.observe(post(7, 9, 1), true);
        readiness.observe(post(7, 9, 1), true);
        assert_eq!(readiness.phase, ReadyPhase::AwaitingSecond(post(7, 9, 1)));
        assert!(readiness.needs_full_present());
    }

    #[test]
    fn nonalternating_post_restarts_the_consecutive_pair() {
        let fifo = TestFifo::new();
        let mut reader = fifo.reader();
        let mut readiness = fifo.controller();
        readiness.observe(post(1, 1, 1), true);
        readiness.observe(post(2, 2, 1), true);
        assert_eq!(readiness.phase, ReadyPhase::AwaitingSecond(post(2, 2, 1)));
        readiness.observe(post(3, 3, 2), true);
        assert_eq!(readiness.phase, ReadyPhase::Sent);
        assert!(!read_message(&mut reader).is_empty());
    }

    #[test]
    fn sequence_and_route_epoch_wrap_still_advance() {
        let fifo = TestFifo::new();
        let mut reader = fifo.reader();
        let mut readiness = fifo.controller();
        readiness.observe(post(u16::MAX, u16::MAX, 1), true);
        readiness.observe(post(1, 0, 2), true);
        assert_eq!(readiness.phase, ReadyPhase::Sent);
        assert!(!read_message(&mut reader).is_empty());
    }

    #[test]
    fn only_displayable_posts_count_and_ready_is_emitted_once() {
        let fifo = TestFifo::new();
        let mut reader = fifo.reader();
        let mut readiness = fifo.controller();
        readiness.observe(post(1, 1, 1), false);
        assert_eq!(readiness.phase, ReadyPhase::AwaitingFirst);
        readiness.observe(post(1, 1, 1), true);
        assert!(readiness.needs_full_present());
        readiness.observe(post(2, 2, 2), true);
        assert_eq!(readiness.phase, ReadyPhase::Sent);
        let first = read_message(&mut reader);
        readiness.poll();
        readiness.observe(post(3, 3, 1), true);
        let mut extra = [0u8; 1];
        let second = reader.read(&mut extra);
        assert_eq!(first.matches("ready-v1").count(), 1);
        match second {
            Ok(0) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            other => panic!("unexpected second FIFO read: {other:?}"),
        }
    }
}
