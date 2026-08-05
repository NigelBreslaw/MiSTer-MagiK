// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lab-only independent framebuffer-vsync observation.

use crate::card_assessment::{VSYNC_EVENT_SCHEMA, VsyncEvent};
use mister_magik_mister_runtime::framebuffer::vsync::{VsyncWaitStatus, wait_vsync_fd};
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

pub struct VsyncObserver {
    stop: Arc<AtomicBool>,
    worker: JoinHandle<Vec<VsyncEvent>>,
}

impl VsyncObserver {
    pub fn start() -> Result<Self, String> {
        let framebuffer = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fb0")
            .map_err(|error| format!("open independent vsync observer: {error}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("scene-vsync-observer".into())
            .spawn(move || {
                let mut events = Vec::with_capacity(4_096);
                let mut ordinal = 0_u64;
                while !worker_stop.load(Ordering::Acquire) {
                    let status = wait_vsync_fd(framebuffer.as_raw_fd());
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    ordinal = ordinal.saturating_add(1);
                    let (status, wait_us, message, terminal) = match status {
                        VsyncWaitStatus::Hit { wait_us, .. } => ("hit", wait_us, None, false),
                        VsyncWaitStatus::Timeout { wait_us } => ("timeout", wait_us, None, false),
                        VsyncWaitStatus::Error { wait_us, message } => {
                            ("error", wait_us, Some(message), true)
                        }
                    };
                    events.push(VsyncEvent {
                        schema: VSYNC_EVENT_SCHEMA,
                        ordinal,
                        status,
                        monotonic_us: super::monotonic_time_us(),
                        wait_us,
                        message,
                    });
                    if terminal {
                        break;
                    }
                }
                events
            })
            .map_err(|error| format!("start independent vsync observer: {error}"))?;
        Ok(Self { stop, worker })
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn finish(self) -> Result<Vec<VsyncEvent>, String> {
        self.request_stop();
        self.worker
            .join()
            .map_err(|_| "independent vsync observer thread panicked".to_owned())
    }
}
