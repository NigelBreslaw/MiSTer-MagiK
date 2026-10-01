// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Process-wide permission with thread-local background-work scope.
//!
//! Production keeps permission open and constrains background phases through
//! CPU affinity and nice levels; background scopes also park while the UI is
//! in motion (see [`crate::ui_motion`]). Tests can close the permission bit to
//! verify that checkpoints do not restart work or leak scope. Foreground
//! first-visible work never enters a background scope.

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum CatalogWorkMode {
    #[default]
    Cpu0 = 0,
    Paused = 1,
    DualCoreBurst = 2,
}

impl CatalogWorkMode {
    fn from_raw(value: u8) -> Self {
        match value {
            1 => Self::Paused,
            2 => Self::DualCoreBurst,
            _ => Self::Cpu0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg(test)]
pub struct CatalogWorkGateSnapshot {
    pub mode: CatalogWorkMode,
    pub epoch: u64,
    pub checkpoints: u64,
    pub park_count: u64,
    pub parked_threads: u64,
}

static WORK_MODE: AtomicU8 = AtomicU8::new(CatalogWorkMode::Cpu0 as u8);
#[cfg(test)]
static WORK_EPOCH: AtomicU64 = AtomicU64::new(0);
static CHECKPOINTS: AtomicU64 = AtomicU64::new(0);
static PARK_COUNT: AtomicU64 = AtomicU64::new(0);
static PARKED_THREADS: AtomicU64 = AtomicU64::new(0);
static PAUSE_SIGNAL: OnceLock<(Mutex<()>, Condvar)> = OnceLock::new();
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    static BACKGROUND_SCOPE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn set_background_allowed(allowed: bool) {
    set_work_mode(if allowed {
        CatalogWorkMode::Cpu0
    } else {
        CatalogWorkMode::Paused
    });
}

#[cfg(test)]
pub(crate) fn set_work_mode(mode: CatalogWorkMode) -> u64 {
    let previous = WORK_MODE.swap(mode as u8, Ordering::AcqRel);
    let epoch = if previous == mode as u8 {
        WORK_EPOCH.load(Ordering::Acquire)
    } else {
        WORK_EPOCH.fetch_add(1, Ordering::AcqRel).saturating_add(1)
    };
    if mode != CatalogWorkMode::Paused {
        PAUSE_SIGNAL
            .get_or_init(|| (Mutex::new(()), Condvar::new()))
            .1
            .notify_all();
    }
    epoch
}

#[cfg(test)]
pub(crate) fn work_gate_snapshot() -> CatalogWorkGateSnapshot {
    CatalogWorkGateSnapshot {
        mode: CatalogWorkMode::from_raw(WORK_MODE.load(Ordering::Acquire)),
        epoch: WORK_EPOCH.load(Ordering::Acquire),
        checkpoints: CHECKPOINTS.load(Ordering::Relaxed),
        park_count: PARK_COUNT.load(Ordering::Relaxed),
        parked_threads: PARKED_THREADS.load(Ordering::Acquire),
    }
}

#[cfg(any(test, feature = "builder"))]
pub(crate) struct BackgroundScope;

#[cfg(any(test, feature = "builder"))]
impl BackgroundScope {
    pub(crate) fn enter() -> Self {
        BACKGROUND_SCOPE_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        Self
    }
}

#[cfg(any(test, feature = "builder"))]
impl Drop for BackgroundScope {
    fn drop(&mut self) {
        BACKGROUND_SCOPE_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

fn paused() -> bool {
    CatalogWorkMode::from_raw(WORK_MODE.load(Ordering::Acquire)) == CatalogWorkMode::Paused
}

/// Background work waits for UI motion to end, for at most the motion bound.
fn must_park(parked_at: std::time::Instant) -> bool {
    paused() || crate::ui_motion::active() && parked_at.elapsed() < crate::ui_motion::MAX_DEFERRAL
}

/// Wake background scopes parked at a checkpoint so they re-check permission.
pub(crate) fn wake_parked() {
    let (lock, signal) = PAUSE_SIGNAL.get_or_init(|| (Mutex::new(()), Condvar::new()));
    // Taking the lock orders this wake after a parking thread's last check.
    drop(lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    signal.notify_all();
}

pub(crate) fn checkpoint() {
    let background = in_background_scope();
    if !background {
        return;
    }
    CHECKPOINTS.fetch_add(1, Ordering::Relaxed);
    let mode = CatalogWorkMode::from_raw(WORK_MODE.load(Ordering::Acquire));
    crate::runtime_thread::apply_catalog_work_mode_affinity(mode);
    let parked_at = std::time::Instant::now();
    if !must_park(parked_at) {
        return;
    }
    let (lock, signal) = PAUSE_SIGNAL.get_or_init(|| (Mutex::new(()), Condvar::new()));
    let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    PARK_COUNT.fetch_add(1, Ordering::Relaxed);
    PARKED_THREADS.fetch_add(1, Ordering::AcqRel);
    while must_park(parked_at) {
        // Time out to re-check the motion bound; permission changes notify.
        guard = signal
            .wait_timeout(guard, std::time::Duration::from_millis(100))
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0;
    }
    PARKED_THREADS.fetch_sub(1, Ordering::AcqRel);
    crate::runtime_thread::apply_catalog_work_mode_affinity(CatalogWorkMode::from_raw(
        WORK_MODE.load(Ordering::Acquire),
    ));
}

pub(crate) fn in_background_scope() -> bool {
    BACKGROUND_SCOPE_DEPTH.with(|depth| depth.get() != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn background_scope_parks_during_ui_motion_and_resumes_when_it_ends() {
        let _test_lock = super::TEST_LOCK.lock().unwrap();
        let _motion = crate::ui_motion::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set_background_allowed(true);
        crate::ui_motion::set_active(true);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _scope = BackgroundScope::enter();
            entered_tx.send(()).unwrap();
            checkpoint();
            done_tx.send(()).unwrap();
        });
        entered_rx.recv().unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(250)).is_err());
        crate::ui_motion::set_active(false);
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        // Foreground work never waits for motion.
        crate::ui_motion::set_active(true);
        checkpoint();
        crate::ui_motion::set_active(false);
    }
    #[test]
    fn background_scope_pauses_and_resumes_without_restarting() {
        let _test_lock = super::TEST_LOCK.lock().unwrap();
        set_background_allowed(false);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _scope = BackgroundScope::enter();
            entered_tx.send(()).unwrap();
            checkpoint();
            done_tx.send(7).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(30)).is_err());
        set_background_allowed(true);
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 7);
        worker.join().unwrap();
    }

    #[test]
    fn foreground_checkpoint_ignores_background_permission() {
        let _test_lock = super::TEST_LOCK.lock().unwrap();
        set_background_allowed(false);
        checkpoint();
        set_background_allowed(true);
    }

    #[test]
    fn work_gate_epochs_change_only_when_mode_changes() {
        let _test_lock = super::TEST_LOCK.lock().unwrap();
        set_work_mode(CatalogWorkMode::Cpu0);
        let before = work_gate_snapshot();
        assert_eq!(set_work_mode(CatalogWorkMode::Cpu0), before.epoch);
        let burst_epoch = set_work_mode(CatalogWorkMode::DualCoreBurst);
        assert_eq!(burst_epoch, before.epoch + 1);
        assert_eq!(work_gate_snapshot().mode, CatalogWorkMode::DualCoreBurst);
        set_work_mode(CatalogWorkMode::Cpu0);
    }
}
