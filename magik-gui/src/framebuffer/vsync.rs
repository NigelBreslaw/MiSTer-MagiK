//! Vsync pacing policy and `/dev/fb0` wait worker.

use crate::boot_analytics;
use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

const FBIO_WAITFORVSYNC: libc::c_ulong = 0x4004_4620;
const DEFAULT_VSYNC_FALLBACK_US: u64 = 16_667;
const PAL_VSYNC_FALLBACK_US: u64 = 20_000;
const VSYNC_GRACE_US: u64 = 1_500;
const PERIOD_ALPHA_NUM: u64 = 1;
const PERIOD_ALPHA_DEN: u64 = 8;
const VSYNC_WORKER_QUEUE_DEPTH: usize = 1;

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
    pub message: Option<String>,
}

pub struct VsyncPacer {
    rx: Receiver<VsyncWaitStatus>,
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
}

pub fn wait_vsync_fd(fd: std::os::unix::io::RawFd) -> VsyncWaitStatus {
    let arg: u32 = 0;
    let start = Instant::now();
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
        let period_us = configured_fallback_period_us();
        let degraded_threshold = std::env::var("MISTER_VSYNC_DEGRADED_MISSES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);
        let (tx, rx) = mpsc::sync_channel(VSYNC_WORKER_QUEUE_DEPTH);
        thread::Builder::new()
            .name("mister-vsync".into())
            .spawn(move || run_vsync_worker(tx, period_us))
            .expect("spawn vsync worker");

        Self {
            rx,
            period_us,
            last_hit_at: None,
            last_frame_at: Instant::now(),
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

    pub fn max_miss_streak(&self) -> u32 {
        self.observed_max_miss_streak
    }

    pub fn wait(&mut self) -> VsyncPace {
        let deadline = Duration::from_micros(self.period_us + VSYNC_GRACE_US);
        let status = self
            .drain_ready()
            .or_else(|| self.rx.recv_timeout(deadline).ok());

        match status {
            Some(VsyncWaitStatus::Hit { wait_us, at }) => {
                self.record_hit(at);
                self.last_frame_at = at;
                VsyncPace {
                    source: VsyncPaceSource::Vsync,
                    wait_us,
                    period_us: self.period_us,
                    miss_streak: self.miss_streak,
                    message: None,
                }
            }
            Some(VsyncWaitStatus::Timeout { wait_us }) => {
                self.timeouts += 1;
                self.fallback_after_miss(VsyncPaceSource::Timeout, wait_us, None)
            }
            Some(VsyncWaitStatus::Error {
                wait_us, message, ..
            }) => {
                self.errors += 1;
                self.fallback_after_miss(VsyncPaceSource::Error, wait_us, Some(message))
            }
            None => self.fallback_after_miss(VsyncPaceSource::Fallback, self.period_us, None),
        }
    }

    fn drain_ready(&mut self) -> Option<VsyncWaitStatus> {
        let mut latest = None;
        while let Ok(status) = self.rx.try_recv() {
            latest = Some(status);
        }
        latest
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
            message,
        }
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
    if let Some(period_us) = std::env::var("MISTER_VSYNC_FALLBACK_HZ")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|hz| *hz > 1.0)
        .map(|hz| (1_000_000.0 / hz).round() as u64)
    {
        return period_us;
    }

    if mister_ini_menu_pal_enabled() {
        PAL_VSYNC_FALLBACK_US
    } else {
        DEFAULT_VSYNC_FALLBACK_US
    }
}

fn mister_ini_menu_pal_enabled() -> bool {
    let Ok(ini) = std::fs::read_to_string("/media/fat/MiSTer.ini") else {
        return false;
    };
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
    fn repeated_vsync_errors_use_fallback_pace() {
        let (tx, rx) = mpsc::sync_channel(VSYNC_WORKER_QUEUE_DEPTH);
        let mut pacer = VsyncPacer {
            rx,
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
    fn isolated_misses_create_one_fallback_frame_each_without_degraded_streak() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        let mut at = Instant::now();
        pacer.record_hit(at);

        for _ in 0..10 {
            pacer.last_frame_at = Instant::now() - Duration::from_micros(pacer.period_us());
            let pace = pacer.fallback_after_miss(VsyncPaceSource::Timeout, 16_667, None);
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
            let pace = pacer.fallback_after_miss(VsyncPaceSource::Timeout, 16_667, None);
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
            pacer.fallback_after_miss(VsyncPaceSource::Timeout, 16_667, None);
        }
        assert_eq!(pacer.miss_streak, 3);

        pacer.record_hit(Instant::now());

        assert_eq!(pacer.miss_streak, 0);
        assert_eq!(pacer.hits(), 1);
        assert_eq!(pacer.max_miss_streak(), 3);
    }
}
