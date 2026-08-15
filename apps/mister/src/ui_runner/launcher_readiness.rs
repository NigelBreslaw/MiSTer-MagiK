// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use slint::platform::software_renderer::Rgb565Pixel;

const LATCH_PROTOCOL: u16 = 5;
const LATCH_CAPABILITIES: u16 = 0x03ff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReadyContext {
    main_pid: u32,
    main_generation: u64,
    owner_epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SourceFrameEvidence {
    sha256: String,
    nonzero_pixels: u32,
}

impl SourceFrameEvidence {
    pub(super) fn from_rgb565_rows(
        pixels: &[Rgb565Pixel],
        width: usize,
        height: usize,
        stride_pixels: usize,
    ) -> Option<Self> {
        if width == 0
            || height == 0
            || stride_pixels < width
            || stride_pixels.checked_mul(height)? > pixels.len()
        {
            return None;
        }
        let mut digest = Sha256::new();
        let mut nonzero_pixels = 0u32;
        for row in pixels.chunks_exact(stride_pixels).take(height) {
            for pixel in &row[..width] {
                digest.update(pixel.0.to_le_bytes());
                nonzero_pixels = nonzero_pixels.saturating_add(u32::from(pixel.0 != 0));
            }
        }
        Some(Self {
            sha256: encode_hex(&digest.finalize()),
            nonzero_pixels,
        })
    }

    fn valid(&self) -> bool {
        self.nonzero_pixels != 0
            && self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PostedSourceFrameEvidence {
    sequence: u16,
    slot: u8,
    source: SourceFrameEvidence,
}

impl PostedSourceFrameEvidence {
    pub(super) fn new(sequence: u16, slot: u8, source: SourceFrameEvidence) -> Self {
        Self {
            sequence,
            slot,
            source,
        }
    }

    pub(super) fn matches(&self, post: ConfirmedLatchPost) -> bool {
        self.sequence == post.sequence && self.slot == post.slot
    }

    pub(super) fn into_source(self) -> SourceFrameEvidence {
        self.source
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ConfirmedLatchPost {
    pub(super) sequence: u16,
    pub(super) route_epoch: u16,
    pub(super) slot: u8,
    pub(super) receipt_crc: u16,
    pub(super) active_base: u32,
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) stride: u16,
}

impl ConfirmedLatchPost {
    fn valid(self) -> bool {
        matches!(self.slot, 1 | 2)
            && self.active_base != 0
            && self.width != 0
            && self.height != 0
            && usize::from(self.stride) >= usize::from(self.width) * 2
    }

    fn advances_and_alternates(self, previous: Self) -> bool {
        advances(self.sequence, previous.sequence)
            && advances(self.route_epoch, previous.route_epoch)
            && self.slot != previous.slot
            && self.active_base != previous.active_base
            && self.width == previous.width
            && self.height == previous.height
            && self.stride == previous.stride
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ReadyPhase {
    Disabled,
    AwaitingFirst,
    AwaitingSecond(ConfirmedLatchPost, SourceFrameEvidence),
    PendingSend(ConfirmedLatchPost, ConfirmedLatchPost, SourceFrameEvidence),
    Sent,
}

pub(super) struct LauncherReadiness {
    phase: ReadyPhase,
    token: String,
    fifo: PathBuf,
    pid: u32,
    context: ReadyContext,
}

impl LauncherReadiness {
    pub(super) fn from_process_config(
        config: mister_magik_fb::process_config::LauncherReadinessConfig,
    ) -> Self {
        let (token, fifo, main_pid, main_generation, owner_epoch) = config.into_parts();
        let context = ReadyContext {
            main_pid,
            main_generation,
            owner_epoch,
        };
        Self::from_config(token, fifo, std::process::id(), context)
    }

    fn from_config(token: String, fifo: PathBuf, pid: u32, context: ReadyContext) -> Self {
        let configured = valid_token(&token)
            && !fifo.as_os_str().is_empty()
            && pid != 0
            && context.main_pid != 0
            && context.main_generation != 0
            && context.owner_epoch != 0;
        Self {
            phase: if configured {
                ReadyPhase::AwaitingFirst
            } else {
                ReadyPhase::Disabled
            },
            token,
            fifo,
            pid,
            context,
        }
    }

    pub(super) fn needs_full_present(&self) -> bool {
        matches!(self.phase, ReadyPhase::AwaitingSecond(_, _))
    }

    pub(super) fn needs_source_evidence(&self) -> bool {
        matches!(
            self.phase,
            ReadyPhase::AwaitingFirst | ReadyPhase::AwaitingSecond(_, _)
        )
    }

    pub(super) fn poll(&mut self) {
        if matches!(self.phase, ReadyPhase::PendingSend(..)) {
            self.try_send();
        }
    }

    pub(super) fn observe_posted(
        &mut self,
        post: ConfirmedLatchPost,
        source: PostedSourceFrameEvidence,
        intended_for_display: bool,
    ) {
        if !source.matches(post) {
            return;
        }
        self.observe(post, source.into_source(), intended_for_display);
    }

    pub(super) fn observe(
        &mut self,
        post: ConfirmedLatchPost,
        source: SourceFrameEvidence,
        intended_for_display: bool,
    ) {
        if !intended_for_display || !post.valid() || !source.valid() {
            return;
        }
        match self.phase.clone() {
            ReadyPhase::AwaitingFirst => self.phase = ReadyPhase::AwaitingSecond(post, source),
            ReadyPhase::AwaitingSecond(previous, _) => {
                if post.advances_and_alternates(previous) {
                    self.phase = ReadyPhase::PendingSend(previous, post, source);
                    self.try_send();
                } else {
                    self.phase = ReadyPhase::AwaitingSecond(post, source);
                }
            }
            ReadyPhase::Disabled | ReadyPhase::PendingSend(..) | ReadyPhase::Sent => {}
        }
    }

    fn try_send(&mut self) {
        let ReadyPhase::PendingSend(first, second, source) = &self.phase else {
            return;
        };
        let line = format!(
            "ready-v2 token={} pid={} main_pid={} main_generation={} owner_epoch={} protocol={} capabilities={:04x} base={:08x} width={} height={} stride={} first_sequence={} first_route_epoch={} first_slot={} first_receipt_crc={:04x} second_sequence={} second_route_epoch={} second_slot={} second_receipt_crc={:04x} source_sha256={} source_nonzero={}\n",
            self.token,
            self.pid,
            self.context.main_pid,
            self.context.main_generation,
            self.context.owner_epoch,
            LATCH_PROTOCOL,
            LATCH_CAPABILITIES,
            second.active_base,
            second.width,
            second.height,
            second.stride,
            first.sequence,
            first.route_epoch,
            first.slot,
            first.receipt_crc,
            second.sequence,
            second.route_epoch,
            second.slot,
            second.receipt_crc,
            source.sha256,
            source.nonzero_pixels,
        );
        if line.len() > 1024 {
            self.phase = ReadyPhase::Disabled;
            return;
        }
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
            LauncherReadiness::from_config(
                TOKEN.into(),
                self.0.clone(),
                42,
                ReadyContext {
                    main_pid: 7,
                    main_generation: 11,
                    owner_epoch: 13,
                },
            )
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
            receipt_crc: sequence.max(1),
            active_base: if slot == 1 { 0x227e_9000 } else { 0x229e_9000 },
            width: 960,
            height: 540,
            stride: 1920,
        }
    }

    fn evidence() -> SourceFrameEvidence {
        SourceFrameEvidence::from_rgb565_rows(&[Rgb565Pixel(0x1234); 4], 2, 2, 2).unwrap()
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
        assert!(readiness.needs_source_evidence());
        readiness.observe(post(1, 1, 1), evidence(), true);
        assert!(readiness.needs_source_evidence());
        readiness.observe(post(2, 2, 2), evidence(), true);
        assert!(matches!(readiness.phase, ReadyPhase::PendingSend(..)));
        assert!(!readiness.needs_source_evidence());

        let mut reader = fifo.reader();
        readiness.poll();
        assert_eq!(readiness.phase, ReadyPhase::Sent);
        assert!(!readiness.needs_source_evidence());
        let message = read_message(&mut reader);
        assert!(message.starts_with("ready-v2 token=0123456789abcdef0123456789abcdef pid=42 main_pid=7 main_generation=11 owner_epoch=13 protocol=5 capabilities=03ff "));
        assert!(message.contains("source_nonzero=4\n"));
    }

    #[test]
    fn invalid_or_stale_token_configuration_is_disabled() {
        let fifo = TestFifo::new();
        let mut readiness = LauncherReadiness::from_config(
            "stale".into(),
            fifo.0.clone(),
            42,
            ReadyContext {
                main_pid: 7,
                main_generation: 11,
                owner_epoch: 13,
            },
        );
        readiness.observe(post(1, 1, 1), evidence(), true);
        readiness.observe(post(2, 2, 2), evidence(), true);
        assert_eq!(readiness.phase, ReadyPhase::Disabled);
    }

    #[test]
    fn missing_spawn_context_disables_readiness() {
        let fifo = TestFifo::new();
        let mut readiness = LauncherReadiness::from_config(
            TOKEN.into(),
            fifo.0.clone(),
            42,
            ReadyContext {
                main_pid: 7,
                main_generation: 0,
                owner_epoch: 13,
            },
        );
        readiness.observe(post(1, 1, 1), evidence(), true);
        assert_eq!(readiness.phase, ReadyPhase::Disabled);
    }

    #[test]
    fn blank_source_frame_cannot_complete_readiness() {
        let fifo = TestFifo::new();
        let mut readiness = fifo.controller();
        let blank = SourceFrameEvidence::from_rgb565_rows(&[Rgb565Pixel(0); 4], 2, 2, 2).unwrap();
        readiness.observe(post(1, 1, 1), blank.clone(), true);
        readiness.observe(post(2, 2, 2), blank, true);
        assert_eq!(readiness.phase, ReadyPhase::AwaitingFirst);
    }

    #[test]
    fn posted_source_evidence_is_bound_to_sequence_and_slot() {
        let expected = post(7, 9, 1);
        let source = PostedSourceFrameEvidence::new(7, 1, evidence());

        assert!(source.matches(expected));
        assert!(!source.matches(post(8, 10, 1)));
        assert!(!source.matches(post(7, 10, 2)));
    }

    #[test]
    fn mismatched_posted_source_cannot_advance_readiness() {
        let fifo = TestFifo::new();
        let mut readiness = fifo.controller();
        let post = post(7, 9, 1);

        readiness.observe_posted(post, PostedSourceFrameEvidence::new(8, 1, evidence()), true);
        readiness.observe_posted(post, PostedSourceFrameEvidence::new(7, 2, evidence()), true);

        assert_eq!(readiness.phase, ReadyPhase::AwaitingFirst);
    }

    #[test]
    fn posted_intro_sources_complete_readiness_while_cached_source_is_blank() {
        let fifo = TestFifo::new();
        let mut reader = fifo.reader();
        let mut readiness = fifo.controller();
        let blank_cached =
            SourceFrameEvidence::from_rgb565_rows(&[Rgb565Pixel(0); 4], 2, 2, 2).unwrap();
        assert!(!blank_cached.valid());
        let first = post(1, 1, 1);
        let second = post(2, 2, 2);

        readiness.observe_posted(
            first,
            PostedSourceFrameEvidence::new(1, 1, evidence()),
            true,
        );
        readiness.observe_posted(
            second,
            PostedSourceFrameEvidence::new(2, 2, evidence()),
            true,
        );

        assert_eq!(readiness.phase, ReadyPhase::Sent);
        assert_eq!(read_message(&mut reader).matches("ready-v2").count(), 1);
    }

    #[test]
    fn duplicate_posts_do_not_complete_readiness() {
        let fifo = TestFifo::new();
        let mut readiness = fifo.controller();
        readiness.observe(post(7, 9, 1), evidence(), true);
        readiness.observe(post(7, 9, 1), evidence(), true);
        assert!(
            matches!(readiness.phase, ReadyPhase::AwaitingSecond(current, _) if current == post(7, 9, 1))
        );
        assert!(readiness.needs_full_present());
    }

    #[test]
    fn nonalternating_post_restarts_the_consecutive_pair() {
        let fifo = TestFifo::new();
        let mut reader = fifo.reader();
        let mut readiness = fifo.controller();
        readiness.observe(post(1, 1, 1), evidence(), true);
        readiness.observe(post(2, 2, 1), evidence(), true);
        assert!(
            matches!(readiness.phase, ReadyPhase::AwaitingSecond(current, _) if current == post(2, 2, 1))
        );
        readiness.observe(post(3, 3, 2), evidence(), true);
        assert_eq!(readiness.phase, ReadyPhase::Sent);
        assert!(!read_message(&mut reader).is_empty());
    }

    #[test]
    fn sequence_and_route_epoch_wrap_still_advance() {
        let fifo = TestFifo::new();
        let mut reader = fifo.reader();
        let mut readiness = fifo.controller();
        readiness.observe(post(u16::MAX, u16::MAX, 1), evidence(), true);
        readiness.observe(post(1, 0, 2), evidence(), true);
        assert_eq!(readiness.phase, ReadyPhase::Sent);
        assert!(!read_message(&mut reader).is_empty());
    }

    #[test]
    fn only_displayable_posts_count_and_ready_is_emitted_once() {
        let fifo = TestFifo::new();
        let mut reader = fifo.reader();
        let mut readiness = fifo.controller();
        readiness.observe(post(1, 1, 1), evidence(), false);
        assert_eq!(readiness.phase, ReadyPhase::AwaitingFirst);
        readiness.observe(post(1, 1, 1), evidence(), true);
        assert!(readiness.needs_full_present());
        readiness.observe(post(2, 2, 2), evidence(), true);
        assert_eq!(readiness.phase, ReadyPhase::Sent);
        let first = read_message(&mut reader);
        readiness.poll();
        readiness.observe(post(3, 3, 1), evidence(), true);
        let mut extra = [0u8; 1];
        let second = reader.read(&mut extra);
        assert_eq!(first.matches("ready-v2").count(), 1);
        match second {
            Ok(0) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            other => panic!("unexpected second FIFO read: {other:?}"),
        }
    }
}
