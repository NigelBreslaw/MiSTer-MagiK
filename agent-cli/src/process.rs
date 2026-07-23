// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

pub fn wait(
    child: &mut Child,
    deadline: Option<Duration>,
    label: &str,
    heartbeat_interval: Option<Duration>,
    mut heartbeat: impl FnMut() -> Result<(), String>,
) -> Result<ExitStatus, String> {
    if deadline.is_none() && heartbeat_interval.is_none() {
        return child
            .wait()
            .map_err(|error| format!("cannot wait for {label}: {error}"));
    }
    let started = Instant::now();
    let mut last_heartbeat = started;
    let mut delay = Duration::from_millis(1);
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
                let elapsed = started.elapsed();
                let mut sleep_for = delay;
                if let Some(limit) = deadline {
                    sleep_for = sleep_for.min(limit.saturating_sub(elapsed));
                }
                if let Some(interval) = heartbeat_interval {
                    sleep_for = sleep_for.min(interval.saturating_sub(last_heartbeat.elapsed()));
                }
                if !sleep_for.is_zero() {
                    thread::sleep(sleep_for);
                }
                if heartbeat_interval.is_some_and(|interval| last_heartbeat.elapsed() >= interval) {
                    if let Err(error) = heartbeat() {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error);
                    }
                    last_heartbeat = Instant::now();
                }
                delay = (delay * 2).min(Duration::from_millis(50));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for {label}: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn blocking_wait_has_low_short_process_overhead() {
        let started = Instant::now();
        for _ in 0..100 {
            let mut child = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
            assert!(
                wait(&mut child, None, "short process", None, || Ok(()))
                    .unwrap()
                    .success()
            );
        }
        // A 50 ms polling delay would take at least five seconds here. Allow
        // slower process spawning on loaded hosts without masking that regression.
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn supervised_wait_emits_heartbeats_and_enforces_deadline() {
        let mut heartbeats = 0;
        let mut child = Command::new("sh")
            .args(["-c", "sleep 0.08"])
            .spawn()
            .unwrap();
        assert!(
            wait(
                &mut child,
                Some(Duration::from_secs(1)),
                "heartbeat process",
                Some(Duration::from_millis(10)),
                || {
                    heartbeats += 1;
                    Ok(())
                },
            )
            .unwrap()
            .success()
        );
        assert!(heartbeats >= 2);

        let mut child = Command::new("sh").args(["-c", "sleep 1"]).spawn().unwrap();
        let error = wait(
            &mut child,
            Some(Duration::from_millis(20)),
            "timed process",
            None,
            || Ok(()),
        )
        .unwrap_err();
        assert!(error.contains("exceeded its 0s deadline"));
    }
}
