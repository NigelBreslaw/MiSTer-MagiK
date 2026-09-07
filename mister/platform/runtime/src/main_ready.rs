// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Main's existing ready-v3 handshake for a custom RGB565 consumer.
//! Mirrors the real launcher's two confirmed, alternating, nonblank frames.
use crate::framebuffer::hidden_latch::{HiddenLatchPresentReceipt, HiddenLatchPresenter};
use crate::framebuffer::rgb565::Rgb565;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

fn advances(current: u16, previous: u16) -> bool {
    let delta = current.wrapping_sub(previous);
    delta != 0 && delta < 0x8000
}

fn valid_pair(a: HiddenLatchPresentReceipt, b: HiddenLatchPresentReceipt) -> bool {
    matches!(a.slot_index, 1 | 2)
        && matches!(b.slot_index, 1 | 2)
        && a.slot_index != b.slot_index
        && a.active_base != 0
        && b.active_base != 0
        && a.active_base != b.active_base
        && advances(b.sequence, a.sequence)
        && advances(b.route_epoch, a.route_epoch)
}

/// Startup only: never rebuilds textures or performs FIFO work on the motion path.
pub fn notify_main_ready(
    presenter: &mut HiddenLatchPresenter,
    pixels: &[Rgb565],
) -> Result<(), String> {
    let Ok(token) = std::env::var("MISTER_MAGIK_STARTUP_TOKEN") else {
        return Ok(());
    };
    if token.len() != 32
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || std::env::var("MISTER_MAGIK_READY_WIRE_VERSION").as_deref() != Ok("3")
    {
        return Err("invalid Main ready-v3 startup context".into());
    }
    let number = |name| -> Result<u64, String> {
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("missing Main startup field {name}"))
    };
    let main_pid = number("MISTER_MAGIK_MAIN_PID")?;
    let generation = number("MISTER_MAGIK_MAIN_GENERATION")?;
    let owner = number("MISTER_MAGIK_OWNER_EPOCH")?;
    let fifo = std::env::var("MISTER_MAGIK_READY_FIFO").map_err(|e| e.to_string())?;
    if fifo != "/tmp/mister-magik/launcher-ready-v2" {
        return Err("unexpected Main readiness FIFO".into());
    }
    let (width, height, stride) = (
        presenter.width(),
        presenter.height(),
        presenter.stride_pixels(),
    );
    if pixels.len() != width * height || !pixels.iter().any(|p| p.0 != 0) {
        return Err("Main readiness requires a complete nonblank source frame".into());
    }
    let mut present = || {
        for (dst, src) in presenter
            .pixels_mut()
            .chunks_exact_mut(stride)
            .zip(pixels.chunks_exact(width))
        {
            dst[..width].copy_from_slice(src);
        }
        presenter.present().map_err(|e| e.to_string())
    };
    let first = present()?;
    let second = present()?;
    if !valid_pair(first, second) {
        return Err("Main readiness receipts did not advance and alternate".into());
    }
    let pid = std::process::id();
    let line = format!(
        "ready-v3 token={token} pid={pid} main_pid={main_pid} main_generation={generation} owner_epoch={owner} protocol=5 capabilities=03ff base={:08x} width={width} height={height} stride={} first_sequence={} first_route_epoch={} first_slot={} first_receipt_crc={:04x} second_sequence={} second_route_epoch={} second_slot={} second_receipt_crc={:04x} source_nonblank=1\n",
        second.active_base,
        stride * 2,
        first.sequence,
        first.route_epoch,
        first.slot_index,
        first.receipt_crc,
        second.sequence,
        second.route_epoch,
        second.slot_index,
        second.receipt_crc
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let sent = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&fifo)
            .and_then(|mut f| f.write(line.as_bytes()))
            .is_ok_and(|n| n == line.len());
        if sent {
            break;
        }
        if Instant::now() >= deadline {
            return Err("Main readiness FIFO timed out".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    loop {
        let status = std::fs::read("/tmp/mister-magik/main-status.json")
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        if status.is_some_and(|s| {
            s["launcher_active"] == true
                && s["launcher_pid"] == pid
                && s["main_generation"] == generation
                && s["fpga_owner"] == "magik"
        }) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("Main did not activate Mini after readiness".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_requires_two_distinct_advancing_slots() {
        let a = HiddenLatchPresentReceipt {
            active_base: 4096,
            route_epoch: 1,
            receipt_crc: 10,
            slot_index: 1,
            sequence: 1,
            flip_count: 1,
            post_count: 1,
            drop_count: 0,
        };
        let b = HiddenLatchPresentReceipt {
            active_base: 8192,
            route_epoch: 2,
            slot_index: 2,
            sequence: 2,
            ..a
        };
        assert!(valid_pair(a, b));
        assert!(!valid_pair(a, a));
        assert!(!valid_pair(b, a));
        assert!(!valid_pair(
            a,
            HiddenLatchPresentReceipt {
                route_epoch: 1,
                ..b
            }
        ));
        assert!(!valid_pair(
            a,
            HiddenLatchPresentReceipt {
                active_base: 0,
                ..b
            }
        ));
    }
}
