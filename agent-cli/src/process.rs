// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

pub fn wait(
    child: &mut Child,
    deadline: Option<Duration>,
    label: &str,
    mut heartbeat: impl FnMut() -> Result<(), String>,
) -> Result<ExitStatus, String> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if let Some(limit) = deadline.filter(|limit| started.elapsed() >= *limit) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{label} exceeded its {}s deadline",
                        limit.as_secs()
                    ));
                }
                thread::sleep(Duration::from_millis(100));
                if let Err(error) = heartbeat() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for {label}: {error}"));
            }
        }
    }
}
