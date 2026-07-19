// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Process-wide permission with thread-local opt-in for interactive-safe work.
//!
//! The launcher owns the permission bit, while catalog code marks only the
//! background phases and worker threads that must cooperate with it. Foreground
//! first-visible work never enters a background scope and therefore never
//! waits on interactive policy.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static BACKGROUND_ALLOWED: AtomicBool = AtomicBool::new(true);
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    static BACKGROUND_SCOPE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

#[cfg(any(feature = "builder", test))]
pub(crate) fn set_background_allowed(allowed: bool) {
    BACKGROUND_ALLOWED.store(allowed, Ordering::Release);
}

pub(crate) struct BackgroundScope;

impl BackgroundScope {
    pub(crate) fn enter() -> Self {
        BACKGROUND_SCOPE_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        Self
    }
}

impl Drop for BackgroundScope {
    fn drop(&mut self) {
        BACKGROUND_SCOPE_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

pub(crate) fn checkpoint() {
    let background = in_background_scope();
    if !background {
        return;
    }
    while !BACKGROUND_ALLOWED.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(4));
    }
}

pub(crate) fn in_background_scope() -> bool {
    BACKGROUND_SCOPE_DEPTH.with(|depth| depth.get() != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

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
}
