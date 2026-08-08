// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared render-policy state for full-screen launcher transitions.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FullScreenTransitionState {
    Live,
    CapturePending,
    SnapshotLocked,
    Releasing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FullScreenTransitionOwner {
    Navigation,
    Orientation,
    StartupReveal,
    Screensaver,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FullScreenTransitionGeneration(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FullScreenTransitionError {
    OwnerActive,
    StaleGeneration,
    InvalidState,
    CaptureNotIssued,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FullScreenTransitionPolicy {
    pub advance_slint_timers: bool,
    pub automatic_slint_raster: bool,
    pub controlled_capture: bool,
    pub snapshot_locked: bool,
    pub force_live_raster: bool,
    pub frame_driven_motion: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveTransition {
    owner: FullScreenTransitionOwner,
    generation: FullScreenTransitionGeneration,
    capture_issued: bool,
    retained_redraw: bool,
}

#[derive(Debug)]
pub struct FullScreenTransitionStateChart {
    state: FullScreenTransitionState,
    next_generation: u64,
    active: Option<ActiveTransition>,
}

impl Default for FullScreenTransitionStateChart {
    fn default() -> Self {
        Self {
            state: FullScreenTransitionState::Live,
            next_generation: 1,
            active: None,
        }
    }
}

impl FullScreenTransitionStateChart {
    pub fn begin(
        &mut self,
        owner: FullScreenTransitionOwner,
    ) -> Result<FullScreenTransitionGeneration, FullScreenTransitionError> {
        if self.active.is_some() || self.state != FullScreenTransitionState::Live {
            return Err(FullScreenTransitionError::OwnerActive);
        }
        let generation = FullScreenTransitionGeneration(self.next_generation);
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.active = Some(ActiveTransition {
            owner,
            generation,
            capture_issued: false,
            retained_redraw: false,
        });
        self.state = FullScreenTransitionState::CapturePending;
        Ok(generation)
    }

    pub const fn state(&self) -> FullScreenTransitionState {
        self.state
    }

    pub const fn owner(&self) -> Option<FullScreenTransitionOwner> {
        match self.active {
            Some(active) => Some(active.owner),
            None => None,
        }
    }

    pub const fn generation(&self) -> Option<FullScreenTransitionGeneration> {
        match self.active {
            Some(active) => Some(active.generation),
            None => None,
        }
    }

    pub fn policy(&self) -> FullScreenTransitionPolicy {
        match self.state {
            FullScreenTransitionState::Live => FullScreenTransitionPolicy {
                advance_slint_timers: true,
                automatic_slint_raster: true,
                controlled_capture: false,
                snapshot_locked: false,
                force_live_raster: false,
                frame_driven_motion: false,
            },
            FullScreenTransitionState::CapturePending => FullScreenTransitionPolicy {
                advance_slint_timers: false,
                automatic_slint_raster: false,
                controlled_capture: self.active.is_some_and(|active| !active.capture_issued),
                snapshot_locked: false,
                force_live_raster: false,
                frame_driven_motion: true,
            },
            FullScreenTransitionState::SnapshotLocked => FullScreenTransitionPolicy {
                advance_slint_timers: false,
                automatic_slint_raster: false,
                controlled_capture: false,
                snapshot_locked: true,
                force_live_raster: false,
                frame_driven_motion: true,
            },
            FullScreenTransitionState::Releasing => FullScreenTransitionPolicy {
                advance_slint_timers: false,
                automatic_slint_raster: false,
                controlled_capture: false,
                snapshot_locked: false,
                force_live_raster: true,
                frame_driven_motion: true,
            },
        }
    }

    pub fn retain_redraw(
        &mut self,
        generation: FullScreenTransitionGeneration,
    ) -> Result<(), FullScreenTransitionError> {
        self.active_mut(generation)?.retained_redraw = true;
        Ok(())
    }

    pub fn take_controlled_capture(
        &mut self,
        generation: FullScreenTransitionGeneration,
    ) -> Result<bool, FullScreenTransitionError> {
        if self.state != FullScreenTransitionState::CapturePending {
            return Err(FullScreenTransitionError::InvalidState);
        }
        let active = self.active_mut(generation)?;
        if active.capture_issued {
            return Ok(false);
        }
        active.capture_issued = true;
        Ok(true)
    }

    pub fn capture_completed(
        &mut self,
        generation: FullScreenTransitionGeneration,
    ) -> Result<(), FullScreenTransitionError> {
        if self.state != FullScreenTransitionState::CapturePending {
            return Err(FullScreenTransitionError::InvalidState);
        }
        if !self.active_ref(generation)?.capture_issued {
            return Err(FullScreenTransitionError::CaptureNotIssued);
        }
        self.state = FullScreenTransitionState::SnapshotLocked;
        Ok(())
    }

    pub fn capture_deferred(
        &mut self,
        generation: FullScreenTransitionGeneration,
    ) -> Result<(), FullScreenTransitionError> {
        if self.state != FullScreenTransitionState::CapturePending {
            return Err(FullScreenTransitionError::InvalidState);
        }
        let active = self.active_mut(generation)?;
        if !active.capture_issued {
            return Err(FullScreenTransitionError::CaptureNotIssued);
        }
        active.capture_issued = false;
        Ok(())
    }

    pub fn release(
        &mut self,
        generation: FullScreenTransitionGeneration,
    ) -> Result<(), FullScreenTransitionError> {
        self.active_ref(generation)?;
        if !matches!(
            self.state,
            FullScreenTransitionState::CapturePending
                | FullScreenTransitionState::SnapshotLocked
                | FullScreenTransitionState::Releasing
        ) {
            return Err(FullScreenTransitionError::InvalidState);
        }
        self.state = FullScreenTransitionState::Releasing;
        Ok(())
    }

    pub fn live_frame_presented(
        &mut self,
        generation: FullScreenTransitionGeneration,
    ) -> Result<bool, FullScreenTransitionError> {
        if self.state != FullScreenTransitionState::Releasing {
            return Err(FullScreenTransitionError::InvalidState);
        }
        let retained_redraw = self.active_ref(generation)?.retained_redraw;
        self.active = None;
        self.state = FullScreenTransitionState::Live;
        Ok(retained_redraw)
    }

    fn active_ref(
        &self,
        generation: FullScreenTransitionGeneration,
    ) -> Result<&ActiveTransition, FullScreenTransitionError> {
        self.active
            .as_ref()
            .filter(|active| active.generation == generation)
            .ok_or(FullScreenTransitionError::StaleGeneration)
    }

    fn active_mut(
        &mut self,
        generation: FullScreenTransitionGeneration,
    ) -> Result<&mut ActiveTransition, FullScreenTransitionError> {
        self.active
            .as_mut()
            .filter(|active| active.generation == generation)
            .ok_or(FullScreenTransitionError::StaleGeneration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_lock_release_requires_physical_confirmation() {
        let mut chart = FullScreenTransitionStateChart::default();
        let generation = chart.begin(FullScreenTransitionOwner::Navigation).unwrap();
        assert!(!chart.policy().advance_slint_timers);
        assert!(chart.policy().controlled_capture);
        assert!(chart.take_controlled_capture(generation).unwrap());
        assert!(!chart.take_controlled_capture(generation).unwrap());
        chart.capture_completed(generation).unwrap();
        assert!(chart.policy().snapshot_locked);
        chart.retain_redraw(generation).unwrap();
        chart.release(generation).unwrap();
        assert_eq!(chart.state(), FullScreenTransitionState::Releasing);
        assert!(chart.policy().force_live_raster);
        assert!(chart.live_frame_presented(generation).unwrap());
        assert_eq!(chart.state(), FullScreenTransitionState::Live);
    }

    #[test]
    fn deferred_raster_restores_controlled_capture_authorization() {
        let mut chart = FullScreenTransitionStateChart::default();
        let generation = chart.begin(FullScreenTransitionOwner::Navigation).unwrap();
        assert!(chart.take_controlled_capture(generation).unwrap());
        assert!(!chart.policy().controlled_capture);
        chart.capture_deferred(generation).unwrap();
        assert!(chart.policy().controlled_capture);
        assert!(chart.take_controlled_capture(generation).unwrap());
        chart.capture_completed(generation).unwrap();
        assert_eq!(chart.state(), FullScreenTransitionState::SnapshotLocked);
    }

    #[test]
    fn cancellation_during_capture_and_playback_releases() {
        for lock_snapshot in [false, true] {
            let mut chart = FullScreenTransitionStateChart::default();
            let generation = chart.begin(FullScreenTransitionOwner::Navigation).unwrap();
            if lock_snapshot {
                assert!(chart.take_controlled_capture(generation).unwrap());
                chart.capture_completed(generation).unwrap();
            }
            chart.release(generation).unwrap();
            assert_eq!(chart.state(), FullScreenTransitionState::Releasing);
            chart.live_frame_presented(generation).unwrap();
            assert_eq!(chart.state(), FullScreenTransitionState::Live);
        }
    }

    #[test]
    fn nested_owners_and_stale_generations_are_rejected() {
        let mut chart = FullScreenTransitionStateChart::default();
        let first = chart.begin(FullScreenTransitionOwner::Navigation).unwrap();
        assert_eq!(
            chart.begin(FullScreenTransitionOwner::Orientation),
            Err(FullScreenTransitionError::OwnerActive)
        );
        chart.release(first).unwrap();
        chart.live_frame_presented(first).unwrap();
        let second = chart.begin(FullScreenTransitionOwner::Navigation).unwrap();
        assert_eq!(
            chart.capture_completed(first),
            Err(FullScreenTransitionError::StaleGeneration)
        );
        assert_ne!(first, second);
    }

    #[test]
    fn reversal_keeps_the_snapshot_locked() {
        let mut chart = FullScreenTransitionStateChart::default();
        let generation = chart.begin(FullScreenTransitionOwner::Navigation).unwrap();
        chart.take_controlled_capture(generation).unwrap();
        chart.capture_completed(generation).unwrap();
        assert_eq!(chart.state(), FullScreenTransitionState::SnapshotLocked);
        assert!(chart.policy().frame_driven_motion);
        assert!(chart.policy().snapshot_locked);
    }
}
