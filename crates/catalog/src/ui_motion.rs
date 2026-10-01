// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Process-wide "the UI is in motion" signal.
//!
//! The presenting loop publishes it once per frame: an animation, transition,
//! scroll, held direction or screensaver is on screen. Deferrable work yields
//! while it is set, because on a two-core Cortex-A9 any task woken on a
//! rendering core can push a frame past its refresh deadline:
//!
//! - background scopes park at their next cooperative checkpoint;
//! - periodic work asks a [`Deferral`] before running, and runs at least once
//!   every [`MAX_DEFERRAL`] so long motion (the screensaver) cannot starve it.
//!
//! Foreground work requested by the user never consults this signal.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Longest a periodic job is postponed while the UI stays in motion.
pub const MAX_DEFERRAL: Duration = Duration::from_secs(5);

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Serializes tests that set or depend on the process-wide motion state.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Publish whether the UI is in motion. Ending motion wakes parked background work.
pub fn set_active(active: bool) {
    if ACTIVE.swap(active, Ordering::AcqRel) && !active {
        crate::cooperative_work::wake_parked();
        crate::work_coordinator::wake_motion_waiters();
    }
}

/// Whether the UI is currently in motion.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Postpones one periodic job while the UI is in motion, for at most
/// [`MAX_DEFERRAL`] at a time.
#[derive(Clone, Copy, Debug, Default)]
pub struct Deferral {
    deferred_since: Option<Instant>,
}

impl Deferral {
    /// Whether due work may run now. Call only when the work is due; a `true`
    /// result means it runs, and the next deferral starts afresh.
    pub fn allows(&mut self, now: Instant) -> bool {
        self.allows_during(active(), now)
    }

    fn allows_during(&mut self, motion: bool, now: Instant) -> bool {
        let since = *self.deferred_since.get_or_insert(now);
        if motion && now.saturating_duration_since(since) < MAX_DEFERRAL {
            return false;
        }
        self.deferred_since = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferral_waits_for_idle_but_never_beyond_the_bound() {
        let start = Instant::now();
        let mut deferral = Deferral::default();
        assert!(
            deferral.allows_during(false, start),
            "idle work runs at once"
        );
        assert!(!deferral.allows_during(true, start));
        assert!(!deferral.allows_during(true, start + Duration::from_millis(4_999)));
        assert!(
            deferral.allows_during(true, start + MAX_DEFERRAL),
            "bounded"
        );
        // The next due period is deferred afresh from its own first request.
        let later = start + Duration::from_secs(9);
        assert!(!deferral.allows_during(true, later));
        assert!(deferral.allows_during(false, later + Duration::from_millis(1)));
    }
}
