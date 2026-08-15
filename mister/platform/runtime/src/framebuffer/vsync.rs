// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Vsync pacing policy and `/dev/fb0` wait worker.

use crate::boot_analytics;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

const FBIO_WAITFORVSYNC: libc::c_ulong = 0x4004_4620;
const DEFAULT_VSYNC_FALLBACK_US: u64 = 16_667;
const PAL_VSYNC_FALLBACK_US: u64 = 20_000;
const VSYNC_GRACE_US: u64 = 1_500;
const DEFAULT_FRESH_HIT_MAX_AGE_US: u64 = 500;
const DIRECT_WAIT_ARM_MARGIN_US: u64 = 4_000;
const PERIOD_ALPHA_NUM: u64 = 1;
const PERIOD_ALPHA_DEN: u64 = 8;
const VSYNC_WORKER_QUEUE_DEPTH: usize = 1;
const INTERRUPTIBLE_WAIT_SLICE: Duration = Duration::from_micros(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VsyncPaceSource {
    Vsync,
    Fallback,
    Timeout,
    Error,
}

impl VsyncPaceSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vsync => "vsync",
            Self::Fallback => "fallback",
            Self::Timeout => "timeout",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug)]
pub struct VsyncPacerModel {
    period_us: u64,
    last_hit_us: Option<u64>,
    miss_streak: u32,
    degraded_threshold: u32,
    observed_max_miss_streak: u32,
    hits: u64,
    timeouts: u64,
    errors: u64,
    fallback_frames: u64,
}

impl VsyncPacerModel {
    pub fn new(period_us: u64, degraded_threshold: u32) -> Self {
        Self {
            period_us,
            last_hit_us: None,
            miss_streak: 0,
            degraded_threshold,
            observed_max_miss_streak: 0,
            hits: 0,
            timeouts: 0,
            errors: 0,
            fallback_frames: 0,
        }
    }

    pub fn period_us(&self) -> u64 {
        self.period_us
    }

    pub fn miss_streak(&self) -> u32 {
        self.miss_streak
    }

    pub fn max_miss_streak(&self) -> u32 {
        self.observed_max_miss_streak
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn fallback_frames(&self) -> u64 {
        self.fallback_frames
    }

    pub fn record_hit_at_us(&mut self, at_us: u64) {
        self.hits += 1;
        self.miss_streak = 0;
        if let Some(prev) = self.last_hit_us {
            let observed = at_us.saturating_sub(prev);
            if (8_000..=25_000).contains(&observed) {
                self.period_us = ((self.period_us * 7) + observed) / 8;
            }
        }
        self.last_hit_us = Some(at_us);
    }

    pub fn record_miss(&mut self, source: VsyncPaceSource) -> bool {
        match source {
            VsyncPaceSource::Timeout => self.timeouts += 1,
            VsyncPaceSource::Error => self.errors += 1,
            VsyncPaceSource::Fallback | VsyncPaceSource::Vsync => {}
        }
        self.miss_streak += 1;
        self.observed_max_miss_streak = self.observed_max_miss_streak.max(self.miss_streak);
        self.fallback_frames += 1;
        self.miss_streak >= self.degraded_threshold
    }
}

#[derive(Clone, Debug)]
pub enum VsyncWaitStatus {
    Hit { wait_us: u64, at: Instant },
    Timeout { wait_us: u64 },
    Error { wait_us: u64, message: String },
}

#[derive(Clone, Debug)]
pub struct VsyncPace {
    pub source: VsyncPaceSource,
    pub wait_us: u64,
    pub period_us: u64,
    pub miss_streak: u32,
    pub hit_at: Option<Instant>,
    pub wait_start_age_us: u64,
    pub accepted_hit_age_us: u64,
    pub stale_hits: u32,
    pub message: Option<String>,
}

#[derive(Clone, Debug)]
pub enum VsyncWaitOutcome {
    Pace(VsyncPace),
    Interrupted,
}

pub struct VsyncPacer {
    rx: Receiver<VsyncWaitStatus>,
    direct_fb: Option<File>,
    default_period_us: u64,
    period_us: u64,
    last_hit_at: Option<Instant>,
    last_frame_at: Instant,
    miss_streak: u32,
    degraded_threshold: u32,
    observed_max_miss_streak: u32,
    hits: u64,
    timeouts: u64,
    errors: u64,
    fallback_frames: u64,
    fresh_hit_max_age_us: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VsyncPacerConfig {
    fallback_hz: Option<String>,
    degraded_threshold: u32,
    direct_wait: bool,
    fresh_hit_max_age_us: u64,
}

impl VsyncPacerConfig {
    pub fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        Self {
            fallback_hz: get("MISTER_VSYNC_FALLBACK_HZ").map(str::to_owned),
            degraded_threshold: get("MISTER_VSYNC_DEGRADED_MISSES")
                .and_then(|value| value.parse().ok())
                .unwrap_or(3),
            direct_wait: direct_wait_enabled_from(get("MISTER_VSYNC_DIRECT_WAIT")),
            fresh_hit_max_age_us: fresh_hit_max_age_us_from(get(
                "MISTER_VSYNC_FRESH_HIT_MAX_AGE_US",
            )),
        }
    }

    pub fn capture_process() -> Self {
        let values = std::env::vars().collect::<std::collections::HashMap<_, _>>();
        Self::capture_with(|name| values.get(name).map(String::as_str))
    }
}

pub fn wait_vsync_fd(fd: std::os::unix::io::RawFd) -> VsyncWaitStatus {
    let arg: u32 = 0;
    let start = Instant::now();
    // SAFETY: fd is a live framebuffer descriptor owned by the caller, and the
    // ioctl only reads the u32 argument during the call.
    let rc = unsafe { libc::ioctl(fd, FBIO_WAITFORVSYNC, &arg as *const u32) };
    let wait_us = start.elapsed().as_micros() as u64;
    let at = Instant::now();
    if rc == 0 {
        return VsyncWaitStatus::Hit { wait_us, at };
    }

    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ETIMEDOUT) {
        VsyncWaitStatus::Timeout { wait_us }
    } else {
        VsyncWaitStatus::Error {
            wait_us,
            message: err.to_string(),
        }
    }
}

impl VsyncPacer {
    pub fn from_env() -> Self {
        Self::from_env_with_default_period(configured_fallback_period_us())
    }

    pub fn from_config(config: &VsyncPacerConfig) -> Self {
        let default_period_us = if mister_ini_menu_pal_enabled() {
            PAL_VSYNC_FALLBACK_US
        } else {
            DEFAULT_VSYNC_FALLBACK_US
        };
        Self::from_config_with_default_period(config, default_period_us)
    }

    pub fn from_env_with_default_period(default_period_us: u64) -> Self {
        Self::from_config_with_default_period(
            &VsyncPacerConfig::capture_process(),
            default_period_us,
        )
    }

    pub fn from_config_with_default_period(
        config: &VsyncPacerConfig,
        default_period_us: u64,
    ) -> Self {
        let period_us =
            fallback_period_us_from_default(config.fallback_hz.as_deref(), default_period_us);
        let direct_fb = if config.direct_wait {
            match OpenOptions::new().read(true).write(true).open("/dev/fb0") {
                Ok(file) => Some(file),
                Err(e) => {
                    boot_analytics::event("vsync_direct_wait_unavailable", e);
                    None
                }
            }
        } else {
            None
        };
        let (tx, rx) = mpsc::sync_channel(VSYNC_WORKER_QUEUE_DEPTH);
        if direct_fb.is_none() {
            thread::Builder::new()
                .name("mister-vsync".into())
                .spawn(move || run_vsync_worker(tx, period_us))
                .expect("spawn vsync worker");
        }

        Self {
            rx,
            direct_fb,
            default_period_us,
            period_us,
            last_hit_at: None,
            last_frame_at: Instant::now(),
            miss_streak: 0,
            degraded_threshold: config.degraded_threshold,
            observed_max_miss_streak: 0,
            hits: 0,
            timeouts: 0,
            errors: 0,
            fallback_frames: 0,
            fresh_hit_max_age_us: config.fresh_hit_max_age_us,
        }
    }

    pub fn period_us(&self) -> u64 {
        self.period_us
    }

    /// Drop the current worker channel and open a fresh framebuffer wait after
    /// Main has completed a display-mode transition. A worker still blocked in
    /// the old ioctl cannot block the render thread and exits when it returns
    /// to the disconnected channel.
    pub fn rearm_after_display_mode_change(&mut self) {
        *self = Self::from_env_with_default_period(self.default_period_us);
        boot_analytics::event("vsync_rearmed", "reason=display_mode_change");
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn timeouts(&self) -> u64 {
        self.timeouts
    }

    pub fn errors(&self) -> u64 {
        self.errors
    }

    pub fn fallback_frames(&self) -> u64 {
        self.fallback_frames
    }

    pub fn fresh_hit_max_age_us(&self) -> u64 {
        self.fresh_hit_max_age_us
    }

    pub fn max_miss_streak(&self) -> u32 {
        self.observed_max_miss_streak
    }

    pub fn age_since_last_hit_us(&self, at: Instant) -> u64 {
        self.last_hit_at
            .map(|hit| at.saturating_duration_since(hit).as_micros() as u64)
            .unwrap_or(0)
    }

    pub fn wait(&mut self) -> VsyncPace {
        match self.wait_interruptible(|| false) {
            VsyncWaitOutcome::Pace(pace) => pace,
            VsyncWaitOutcome::Interrupted => unreachable!("non-interruptible vsync wait"),
        }
    }

    pub fn wait_interruptible(
        &mut self,
        mut should_interrupt: impl FnMut() -> bool,
    ) -> VsyncWaitOutcome {
        let wait_started_at = Instant::now();
        let wait_start_age_us = self.age_since_last_hit_us(wait_started_at);
        if let Some(fd) = self.direct_fb.as_ref().map(AsRawFd::as_raw_fd) {
            return VsyncWaitOutcome::Pace(self.wait_direct(
                fd,
                wait_started_at,
                wait_start_age_us,
            ));
        }
        let deadline = Duration::from_micros(self.period_us + VSYNC_GRACE_US);
        let deadline_at = wait_started_at + deadline;
        let mut stale_hits = 0u32;
        let status = loop {
            if should_interrupt() {
                return VsyncWaitOutcome::Interrupted;
            }
            if let Some(status) = self.drain_ready() {
                if self.is_stale_hit(&status, wait_started_at) {
                    stale_hits += 1;
                    continue;
                }
                break Some(status);
            }

            let now = Instant::now();
            if now >= deadline_at {
                break None;
            }
            let wait_for = (deadline_at - now).min(INTERRUPTIBLE_WAIT_SLICE);
            let status = match self.rx.recv_timeout(wait_for) {
                Ok(status) => status,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break None,
            };
            if self.is_stale_hit(&status, wait_started_at) {
                stale_hits += 1;
                continue;
            }
            break Some(status);
        };

        VsyncWaitOutcome::Pace(match status {
            Some(VsyncWaitStatus::Hit { wait_us, at }) => {
                let accepted_hit_age_us =
                    wait_started_at.saturating_duration_since(at).as_micros() as u64;
                self.record_hit(at);
                self.last_frame_at = at;
                VsyncPace {
                    source: VsyncPaceSource::Vsync,
                    wait_us,
                    period_us: self.period_us,
                    miss_streak: self.miss_streak,
                    hit_at: Some(at),
                    wait_start_age_us,
                    accepted_hit_age_us,
                    stale_hits,
                    message: None,
                }
            }
            Some(VsyncWaitStatus::Timeout { wait_us }) => {
                self.timeouts += 1;
                let mut pace = self.fallback_after_miss(
                    VsyncPaceSource::Timeout,
                    wait_us,
                    wait_start_age_us,
                    None,
                );
                pace.stale_hits = stale_hits;
                pace
            }
            Some(VsyncWaitStatus::Error {
                wait_us, message, ..
            }) => {
                self.errors += 1;
                let mut pace = self.fallback_after_miss(
                    VsyncPaceSource::Error,
                    wait_us,
                    wait_start_age_us,
                    Some(message),
                );
                pace.stale_hits = stale_hits;
                pace
            }
            None => {
                let mut pace = self.fallback_after_miss(
                    VsyncPaceSource::Fallback,
                    self.period_us,
                    wait_start_age_us,
                    None,
                );
                pace.stale_hits = stale_hits;
                pace
            }
        })
    }

    fn drain_ready(&mut self) -> Option<VsyncWaitStatus> {
        let mut latest = None;
        while let Ok(status) = self.rx.try_recv() {
            latest = Some(status);
        }
        latest
    }

    fn is_stale_hit(&self, status: &VsyncWaitStatus, wait_started_at: Instant) -> bool {
        let VsyncWaitStatus::Hit { at, .. } = status else {
            return false;
        };
        wait_started_at.saturating_duration_since(*at).as_micros()
            > u128::from(self.fresh_hit_max_age_us)
    }

    fn record_hit(&mut self, at: Instant) {
        self.hits += 1;
        self.miss_streak = 0;
        if let Some(prev) = self.last_hit_at {
            let observed = at.saturating_duration_since(prev).as_micros() as u64;
            if (8_000..=25_000).contains(&observed) {
                self.period_us = ((self.period_us * (PERIOD_ALPHA_DEN - PERIOD_ALPHA_NUM))
                    + observed * PERIOD_ALPHA_NUM)
                    / PERIOD_ALPHA_DEN;
            }
        }
        self.last_hit_at = Some(at);
    }

    fn fallback_after_miss(
        &mut self,
        source: VsyncPaceSource,
        wait_us: u64,
        wait_start_age_us: u64,
        message: Option<String>,
    ) -> VsyncPace {
        self.miss_streak += 1;
        self.observed_max_miss_streak = self.observed_max_miss_streak.max(self.miss_streak);
        self.fallback_frames += 1;

        let target = self.last_frame_at + Duration::from_micros(self.period_us);
        let now = Instant::now();
        if target > now {
            thread::sleep(target - now);
        }
        self.last_frame_at = Instant::now();

        if self.miss_streak == self.degraded_threshold {
            boot_analytics::event(
                "vsync_degraded",
                format!(
                    "miss_streak={} period_us={} source={}",
                    self.miss_streak,
                    self.period_us,
                    source.label()
                ),
            );
        }

        VsyncPace {
            source,
            wait_us,
            period_us: self.period_us,
            miss_streak: self.miss_streak,
            hit_at: None,
            wait_start_age_us,
            accepted_hit_age_us: 0,
            stale_hits: 0,
            message,
        }
    }

    fn wait_direct(
        &mut self,
        fd: std::os::unix::io::RawFd,
        wait_started_at: Instant,
        wait_start_age_us: u64,
    ) -> VsyncPace {
        // The MiSTer fb ioctl can occasionally return one vblank late when it
        // is armed very early in the period. Sleep in userspace until close to
        // the predicted vblank, but keep the full wait in the trace timing.
        if self.last_hit_at.is_some()
            && let Some(sleep_us) = direct_wait_pre_arm_sleep_us(wait_start_age_us, self.period_us)
        {
            thread::sleep(Duration::from_micros(sleep_us));
        }
        match wait_vsync_fd(fd) {
            VsyncWaitStatus::Hit { at, .. } => {
                let wait_us = wait_started_at.elapsed().as_micros() as u64;
                let accepted_hit_age_us =
                    wait_started_at.saturating_duration_since(at).as_micros() as u64;
                self.record_hit(at);
                self.last_frame_at = at;
                VsyncPace {
                    source: VsyncPaceSource::Vsync,
                    wait_us,
                    period_us: self.period_us,
                    miss_streak: self.miss_streak,
                    hit_at: Some(at),
                    wait_start_age_us,
                    accepted_hit_age_us,
                    stale_hits: 0,
                    message: None,
                }
            }
            VsyncWaitStatus::Timeout { .. } => {
                let wait_us = wait_started_at.elapsed().as_micros() as u64;
                self.timeouts += 1;
                self.fallback_after_miss(VsyncPaceSource::Timeout, wait_us, wait_start_age_us, None)
            }
            VsyncWaitStatus::Error { message, .. } => {
                let wait_us = wait_started_at.elapsed().as_micros() as u64;
                self.errors += 1;
                self.fallback_after_miss(
                    VsyncPaceSource::Error,
                    wait_us,
                    wait_start_age_us,
                    Some(message),
                )
            }
        }
    }
}

fn direct_wait_pre_arm_sleep_us(wait_start_age_us: u64, period_us: u64) -> Option<u64> {
    let arm_age_us = period_us.checked_sub(DIRECT_WAIT_ARM_MARGIN_US)?;
    if wait_start_age_us < arm_age_us {
        Some(arm_age_us - wait_start_age_us)
    } else {
        None
    }
}

fn run_vsync_worker(tx: SyncSender<VsyncWaitStatus>, fallback_period_us: u64) {
    let fb0 = match OpenOptions::new().read(true).write(true).open("/dev/fb0") {
        Ok(f) => f,
        Err(e) => {
            let _ = tx.send(VsyncWaitStatus::Error {
                wait_us: 0,
                message: format!("open /dev/fb0: {e}"),
            });
            return;
        }
    };
    loop {
        let status = wait_vsync_fd(fb0.as_raw_fd());
        let backoff = vsync_worker_backoff(&status, fallback_period_us);
        if tx.send(status).is_err() {
            break;
        }
        if let Some(backoff) = backoff {
            thread::sleep(backoff);
        }
    }
}

fn vsync_worker_backoff(status: &VsyncWaitStatus, fallback_period_us: u64) -> Option<Duration> {
    match status {
        VsyncWaitStatus::Error { .. } => Some(Duration::from_micros(fallback_period_us)),
        VsyncWaitStatus::Hit { .. } | VsyncWaitStatus::Timeout { .. } => None,
    }
}

fn configured_fallback_period_us() -> u64 {
    let default_period_us = if mister_ini_menu_pal_enabled() {
        PAL_VSYNC_FALLBACK_US
    } else {
        DEFAULT_VSYNC_FALLBACK_US
    };
    configured_fallback_period_us_with_default(default_period_us)
}

fn configured_fallback_period_us_with_default(default_period_us: u64) -> u64 {
    fallback_period_us_from_default(
        VsyncPacerConfig::capture_process().fallback_hz.as_deref(),
        default_period_us,
    )
}

#[cfg(test)]
fn fallback_period_us_from(hz: Option<&str>, menu_pal: bool) -> u64 {
    fallback_period_us_from_default(
        hz,
        if menu_pal {
            PAL_VSYNC_FALLBACK_US
        } else {
            DEFAULT_VSYNC_FALLBACK_US
        },
    )
}

fn fallback_period_us_from_default(hz: Option<&str>, default_period_us: u64) -> u64 {
    if let Some(period_us) = hz
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|hz| *hz > 1.0)
        .map(|hz| (1_000_000.0 / hz).round() as u64)
    {
        return period_us;
    }
    default_period_us
}

fn fresh_hit_max_age_us_from(value: Option<&str>) -> u64 {
    value
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(DEFAULT_FRESH_HIT_MAX_AGE_US)
        .min(10_000)
}

fn direct_wait_enabled_from(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "on" | "true" | "yes"
        )
    })
}

fn mister_ini_menu_pal_enabled() -> bool {
    let Ok(ini) = std::fs::read_to_string("/media/fat/MiSTer.ini") else {
        return false;
    };
    menu_pal_enabled_from(&ini)
}

fn menu_pal_enabled_from(ini: &str) -> bool {
    ini.lines().any(|line| {
        let line = line.split(';').next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        key.trim().eq_ignore_ascii_case("menu_pal") && value.trim() == "1"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FALLBACK_60_US: u64 = 16_667;

    fn worker_pacer_for_test() -> (VsyncPacer, SyncSender<VsyncWaitStatus>) {
        let (tx, rx) = mpsc::sync_channel(1);
        (
            VsyncPacer {
                rx,
                direct_fb: None,
                default_period_us: FALLBACK_60_US,
                period_us: FALLBACK_60_US,
                last_hit_at: None,
                last_frame_at: Instant::now(),
                miss_streak: 0,
                degraded_threshold: 3,
                observed_max_miss_streak: 0,
                hits: 0,
                timeouts: 0,
                errors: 0,
                fallback_frames: 0,
                fresh_hit_max_age_us: DEFAULT_FRESH_HIT_MAX_AGE_US,
            },
            tx,
        )
    }

    #[test]
    fn worker_wait_can_be_interrupted_without_waiting_for_vblank() {
        let (mut pacer, _tx) = worker_pacer_for_test();
        let started = Instant::now();

        assert!(matches!(
            pacer.wait_interruptible(|| true),
            VsyncWaitOutcome::Interrupted
        ));
        assert!(started.elapsed() < Duration::from_millis(5));
    }

    #[test]
    fn vsync_configuration_parsers_bound_and_default_invalid_values() {
        assert_eq!(fallback_period_us_from(Some("50"), false), 20_000);
        assert_eq!(
            fallback_period_us_from(Some("invalid"), true),
            PAL_VSYNC_FALLBACK_US
        );
        assert_eq!(
            fallback_period_us_from(Some("1"), false),
            DEFAULT_VSYNC_FALLBACK_US
        );
        assert_eq!(fresh_hit_max_age_us_from(Some("250")), 250);
        assert_eq!(fresh_hit_max_age_us_from(Some("999999")), 10_000);
        assert_eq!(
            fresh_hit_max_age_us_from(Some("bad")),
            DEFAULT_FRESH_HIT_MAX_AGE_US
        );
        assert!(!direct_wait_enabled_from(Some("off")));
        assert!(!direct_wait_enabled_from(Some("0")));
        assert!(direct_wait_enabled_from(Some("1")));
        assert!(direct_wait_enabled_from(Some("TRUE")));
        assert!(!direct_wait_enabled_from(None));
        assert_eq!(fallback_period_us_from_default(None, 19_830), 19_830);
        assert_eq!(fallback_period_us_from_default(Some("50"), 19_830), 20_000);
    }

    #[test]
    fn menu_pal_parser_ignores_comments_whitespace_and_other_sections() {
        assert!(menu_pal_enabled_from(
            ";menu_pal=1\n menu_pal = 1 ; enabled\n"
        ));
        assert!(!menu_pal_enabled_from("menu_pal=0\nother=1\n"));
        assert!(!menu_pal_enabled_from("menu_pal\nmenu_pal=true\n"));
    }

    #[test]
    fn learns_perfect_60hz() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 3);
        for i in 0..120 {
            model.record_hit_at_us(i * 16_667);
        }

        let inferred_hz = 1_000_000.0 / model.period_us() as f64;
        assert!(
            (59.5..=60.5).contains(&inferred_hz),
            "inferred_hz={inferred_hz}"
        );
    }

    #[test]
    fn learns_perfect_pal_50hz_without_snapping_to_60hz() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 3);
        for i in 0..120 {
            model.record_hit_at_us(i * 20_000);
        }

        let inferred_hz = 1_000_000.0 / model.period_us() as f64;
        assert!(
            (49.5..=50.5).contains(&inferred_hz),
            "inferred_hz={inferred_hz}"
        );
    }

    #[test]
    fn ten_isolated_misses_over_two_minutes_do_not_degrade() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 3);
        let mut now_us = 0;
        model.record_hit_at_us(now_us);

        for _ in 0..10 {
            now_us += 12_000_000;
            let degraded = model.record_miss(VsyncPaceSource::Timeout);
            assert!(!degraded);
            assert_eq!(model.miss_streak(), 1);
            now_us += FALLBACK_60_US;
            model.record_hit_at_us(now_us);
        }

        assert_eq!(model.fallback_frames(), 10);
        assert_eq!(model.max_miss_streak(), 1);
        assert_eq!(model.miss_streak(), 0);
    }

    #[test]
    fn three_consecutive_misses_enter_degraded_state() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 3);
        assert!(!model.record_miss(VsyncPaceSource::Timeout));
        assert!(!model.record_miss(VsyncPaceSource::Timeout));
        assert!(model.record_miss(VsyncPaceSource::Timeout));

        assert_eq!(model.fallback_frames(), 3);
        assert_eq!(model.max_miss_streak(), 3);
    }

    #[test]
    fn successful_hit_recovers_after_degraded_state() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 3);
        model.record_miss(VsyncPaceSource::Timeout);
        model.record_miss(VsyncPaceSource::Timeout);
        model.record_miss(VsyncPaceSource::Timeout);
        assert_eq!(model.miss_streak(), 3);

        model.record_hit_at_us(50_000);

        assert_eq!(model.miss_streak(), 0);
        assert_eq!(model.hits(), 1);
        assert_eq!(model.max_miss_streak(), 3);
    }

    #[test]
    fn source_labels_match_runtime_status_values() {
        assert_eq!(VsyncPaceSource::Vsync.label(), "vsync");
        assert_eq!(VsyncPaceSource::Fallback.label(), "fallback");
        assert_eq!(VsyncPaceSource::Timeout.label(), "timeout");
        assert_eq!(VsyncPaceSource::Error.label(), "error");
    }

    #[test]
    fn error_misses_count_as_fallback_frames() {
        let mut model = VsyncPacerModel::new(FALLBACK_60_US, 2);

        assert!(!model.record_miss(VsyncPaceSource::Error));
        assert!(model.record_miss(VsyncPaceSource::Fallback));

        assert_eq!(model.fallback_frames(), 2);
        assert_eq!(model.miss_streak(), 2);
        assert_eq!(model.max_miss_streak(), 2);
    }

    fn test_pacer(period_us: u64) -> VsyncPacer {
        let (_tx, rx) = mpsc::channel();
        VsyncPacer {
            rx,
            direct_fb: None,
            default_period_us: period_us,
            period_us,
            last_hit_at: None,
            last_frame_at: Instant::now() - Duration::from_micros(period_us),
            miss_streak: 0,
            degraded_threshold: 3,
            observed_max_miss_streak: 0,
            hits: 0,
            timeouts: 0,
            errors: 0,
            fallback_frames: 0,
            fresh_hit_max_age_us: DEFAULT_FRESH_HIT_MAX_AGE_US,
        }
    }

    #[test]
    fn vsync_worker_channel_is_bounded() {
        let (tx, _rx) = mpsc::sync_channel::<VsyncWaitStatus>(VSYNC_WORKER_QUEUE_DEPTH);
        tx.try_send(VsyncWaitStatus::Timeout { wait_us: 1 })
            .expect("first status fits");
        assert!(
            tx.try_send(VsyncWaitStatus::Timeout { wait_us: 2 })
                .is_err(),
            "second status must not queue behind an unread frame"
        );
    }

    #[test]
    fn missing_worker_result_falls_back_within_one_frame_deadline() {
        let mut pacer = test_pacer(1_000);
        let started = Instant::now();

        let pace = pacer.wait();

        assert_eq!(pace.source, VsyncPaceSource::Fallback);
        assert!(started.elapsed() < Duration::from_millis(20));
    }

    #[test]
    fn vsync_worker_backs_off_only_after_errors() {
        assert_eq!(
            vsync_worker_backoff(
                &VsyncWaitStatus::Error {
                    wait_us: 0,
                    message: "ioctl failed".to_string()
                },
                DEFAULT_VSYNC_FALLBACK_US
            ),
            Some(Duration::from_micros(DEFAULT_VSYNC_FALLBACK_US))
        );
        assert_eq!(
            vsync_worker_backoff(&VsyncWaitStatus::Timeout { wait_us: 16_667 }, 123),
            None
        );
        assert_eq!(
            vsync_worker_backoff(
                &VsyncWaitStatus::Hit {
                    wait_us: 16_000,
                    at: Instant::now()
                },
                123
            ),
            None
        );
    }

    #[test]
    fn direct_wait_sleeps_until_arm_margin_when_called_early() {
        assert_eq!(
            direct_wait_pre_arm_sleep_us(3_000, DEFAULT_VSYNC_FALLBACK_US),
            Some(9_667)
        );
    }

    #[test]
    fn direct_wait_does_not_pre_sleep_inside_arm_margin() {
        assert_eq!(
            direct_wait_pre_arm_sleep_us(13_000, DEFAULT_VSYNC_FALLBACK_US),
            None
        );
        assert_eq!(direct_wait_pre_arm_sleep_us(1_000, 4_000), None);
    }

    #[test]
    fn repeated_vsync_errors_use_fallback_pace() {
        let (tx, rx) = mpsc::sync_channel(VSYNC_WORKER_QUEUE_DEPTH);
        let mut pacer = VsyncPacer {
            rx,
            direct_fb: None,
            default_period_us: DEFAULT_VSYNC_FALLBACK_US,
            period_us: DEFAULT_VSYNC_FALLBACK_US,
            last_hit_at: None,
            last_frame_at: Instant::now() - Duration::from_micros(DEFAULT_VSYNC_FALLBACK_US),
            miss_streak: 0,
            degraded_threshold: 3,
            observed_max_miss_streak: 0,
            hits: 0,
            timeouts: 0,
            errors: 0,
            fallback_frames: 0,
            fresh_hit_max_age_us: DEFAULT_FRESH_HIT_MAX_AGE_US,
        };

        for expected in 1..=3 {
            tx.try_send(VsyncWaitStatus::Error {
                wait_us: 0,
                message: format!("ioctl failed {expected}"),
            })
            .expect("queue is drained every wait");
            pacer.last_frame_at = Instant::now() - Duration::from_micros(pacer.period_us());
            let pace = pacer.wait();
            assert_eq!(pace.source, VsyncPaceSource::Error);
            assert_eq!(pace.miss_streak, expected);
            let expected_message = format!("ioctl failed {expected}");
            assert_eq!(pace.message.as_deref(), Some(expected_message.as_str()));
        }

        assert_eq!(pacer.errors(), 3);
        assert_eq!(pacer.fallback_frames(), 3);
        assert_eq!(pacer.max_miss_streak(), 3);
    }

    #[test]
    fn runtime_pacer_learns_pal_50hz_from_successful_hits() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        let mut at = Instant::now();
        pacer.record_hit(at);
        for _ in 0..48 {
            at += Duration::from_micros(20_000);
            pacer.record_hit(at);
        }

        let inferred_hz = 1_000_000.0 / pacer.period_us() as f64;
        assert!(
            (49.5..=50.5).contains(&inferred_hz),
            "inferred_hz={inferred_hz}"
        );
    }

    #[test]
    fn runtime_pacer_stays_near_60hz_from_successful_hits() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        let mut at = Instant::now();
        pacer.record_hit(at);
        for _ in 0..24 {
            at += Duration::from_micros(16_667);
            pacer.record_hit(at);
        }

        let inferred_hz = 1_000_000.0 / pacer.period_us() as f64;
        assert!(
            (59.5..=60.5).contains(&inferred_hz),
            "inferred_hz={inferred_hz}"
        );
    }

    #[test]
    fn runtime_pacer_reports_age_since_last_hit() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        let at = Instant::now();
        assert_eq!(pacer.age_since_last_hit_us(at), 0);

        pacer.record_hit(at);

        assert_eq!(pacer.age_since_last_hit_us(at), 0);
        assert_eq!(
            pacer.age_since_last_hit_us(at + Duration::from_micros(12_345)),
            12_345
        );
    }

    #[test]
    fn isolated_misses_create_one_fallback_frame_each_without_degraded_streak() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        let mut at = Instant::now();
        pacer.record_hit(at);

        for _ in 0..10 {
            pacer.last_frame_at = Instant::now() - Duration::from_micros(pacer.period_us());
            let pace = pacer.fallback_after_miss(VsyncPaceSource::Timeout, 16_667, 0, None);
            assert_eq!(pace.source, VsyncPaceSource::Timeout);
            assert_eq!(pace.miss_streak, 1);
            at += Duration::from_micros(16_667);
            pacer.record_hit(at);
        }

        assert_eq!(pacer.timeouts(), 0);
        assert_eq!(pacer.fallback_frames(), 10);
        assert_eq!(pacer.max_miss_streak(), 1);
        assert_eq!(pacer.miss_streak, 0);
    }

    #[test]
    fn three_consecutive_runtime_misses_reach_degraded_threshold() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        for expected in 1..=3 {
            pacer.last_frame_at = Instant::now() - Duration::from_micros(pacer.period_us());
            let pace = pacer.fallback_after_miss(VsyncPaceSource::Timeout, 16_667, 0, None);
            assert_eq!(pace.miss_streak, expected);
        }

        assert_eq!(pacer.fallback_frames(), 3);
        assert_eq!(pacer.max_miss_streak(), 3);
    }

    #[test]
    fn successful_hit_recovers_after_runtime_degraded_streak() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        for _ in 0..3 {
            pacer.last_frame_at = Instant::now() - Duration::from_micros(pacer.period_us());
            pacer.fallback_after_miss(VsyncPaceSource::Timeout, 16_667, 0, None);
        }
        assert_eq!(pacer.miss_streak, 3);

        pacer.record_hit(Instant::now());

        assert_eq!(pacer.miss_streak, 0);
        assert_eq!(pacer.hits(), 1);
        assert_eq!(pacer.max_miss_streak(), 3);
    }

    #[test]
    fn stale_queued_hit_is_discarded_before_waiting_for_fresh_hit() {
        let (tx, rx) = mpsc::sync_channel(VSYNC_WORKER_QUEUE_DEPTH);
        let mut pacer = VsyncPacer {
            rx,
            direct_fb: None,
            default_period_us: DEFAULT_VSYNC_FALLBACK_US,
            period_us: DEFAULT_VSYNC_FALLBACK_US,
            last_hit_at: None,
            last_frame_at: Instant::now() - Duration::from_micros(DEFAULT_VSYNC_FALLBACK_US),
            miss_streak: 0,
            degraded_threshold: 3,
            observed_max_miss_streak: 0,
            hits: 0,
            timeouts: 0,
            errors: 0,
            fallback_frames: 0,
            fresh_hit_max_age_us: DEFAULT_FRESH_HIT_MAX_AGE_US,
        };
        tx.try_send(VsyncWaitStatus::Hit {
            wait_us: 16_000,
            at: Instant::now() - Duration::from_micros(DEFAULT_FRESH_HIT_MAX_AGE_US + 1_000),
        })
        .expect("stale hit queued");
        let sender = std::thread::spawn(move || {
            let fresh_at = Instant::now();
            tx.send(VsyncWaitStatus::Hit {
                wait_us: 16_000,
                at: fresh_at,
            })
            .expect("fresh hit queued after stale one is drained");
            fresh_at
        });

        let pace = pacer.wait();
        let fresh_at = sender.join().expect("fresh sender joins");

        assert_eq!(pace.source, VsyncPaceSource::Vsync);
        assert_eq!(pace.hit_at, Some(fresh_at));
        assert_eq!(pace.stale_hits, 1);
        assert_eq!(pacer.hits(), 1);
    }
}
