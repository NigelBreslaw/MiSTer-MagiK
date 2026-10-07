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
    /// Never `Live`: a transition that has ended is no `ActiveTransition`.
    state: FullScreenTransitionState,
    generation: FullScreenTransitionGeneration,
    capture_issued: bool,
    retained_redraw: bool,
}

#[derive(Debug)]
pub struct FullScreenTransitionStateChart {
    next_generation: u64,
    active: Option<ActiveTransition>,
}

impl Default for FullScreenTransitionStateChart {
    fn default() -> Self {
        Self {
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
        if self.active.is_some() {
            return Err(FullScreenTransitionError::OwnerActive);
        }
        let generation = FullScreenTransitionGeneration(self.next_generation);
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.active = Some(ActiveTransition {
            owner,
            state: FullScreenTransitionState::CapturePending,
            generation,
            capture_issued: false,
            retained_redraw: false,
        });
        Ok(generation)
    }

    pub const fn state(&self) -> FullScreenTransitionState {
        match self.active {
            Some(active) => active.state,
            None => FullScreenTransitionState::Live,
        }
    }

    /// Whether Slint runs on its own: no transition owns the frame.
    pub const fn is_live(&self) -> bool {
        self.active.is_none()
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

    /// The active transition's generation when `owner` holds it, so a caller needs
    /// no copy of the generation that could outlive the transition.
    pub fn generation_for(
        &self,
        owner: FullScreenTransitionOwner,
    ) -> Option<FullScreenTransitionGeneration> {
        self.active
            .filter(|active| active.owner == owner)
            .map(|active| active.generation)
    }

    pub const fn capture_issued(&self) -> bool {
        match self.active {
            Some(active) => active.capture_issued,
            None => false,
        }
    }

    pub fn policy(&self) -> FullScreenTransitionPolicy {
        match self.state() {
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
        if self.state() != FullScreenTransitionState::CapturePending {
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
        if self.state() != FullScreenTransitionState::CapturePending {
            return Err(FullScreenTransitionError::InvalidState);
        }
        let active = self.active_mut(generation)?;
        if !active.capture_issued {
            return Err(FullScreenTransitionError::CaptureNotIssued);
        }
        active.state = FullScreenTransitionState::SnapshotLocked;
        Ok(())
    }

    pub fn capture_deferred(
        &mut self,
        generation: FullScreenTransitionGeneration,
    ) -> Result<(), FullScreenTransitionError> {
        if self.state() != FullScreenTransitionState::CapturePending {
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
        self.active_mut(generation)?.state = FullScreenTransitionState::Releasing;
        Ok(())
    }

    pub fn live_frame_presented(
        &mut self,
        generation: FullScreenTransitionGeneration,
    ) -> Result<bool, FullScreenTransitionError> {
        if self.state() != FullScreenTransitionState::Releasing {
            return Err(FullScreenTransitionError::InvalidState);
        }
        let retained_redraw = self.active_ref(generation)?.retained_redraw;
        self.active = None;
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
mod scenarios;

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

    /// The chart drawn in `docs/architecture.md` is the chart the code runs:
    /// every state change an operation can make is drawn, and every drawn
    /// edge is one the code can make.
    #[test]
    fn the_documented_chart_is_the_chart_the_code_runs() {
        use FullScreenTransitionState::*;
        use std::collections::BTreeSet;
        const DOC: &str = include_str!("../../../../docs/architecture.md");
        let block = DOC
            .split("```mermaid")
            .filter_map(|block| block.split("```").next())
            .find(|block| block.contains("[*] --> Live"))
            .expect("the transition chart is in the architecture doc");
        let drawn: BTreeSet<(String, String)> = block
            .lines()
            .filter_map(|line| {
                let edge = line.split_once(':').map_or(line, |(edge, _)| edge);
                let (from, to) = edge.trim().split_once(" --> ")?;
                (from != "[*]").then(|| (from.to_owned(), to.trim().to_owned()))
            })
            .collect();

        // A capture must have been issued before it can complete.
        let in_state = |state, issued: bool| {
            let mut chart = FullScreenTransitionStateChart::default();
            if state == Live {
                return (chart, FullScreenTransitionGeneration(0));
            }
            let generation = chart.begin(FullScreenTransitionOwner::Navigation).unwrap();
            if issued && state == CapturePending {
                chart.take_controlled_capture(generation).unwrap();
            }
            if state == SnapshotLocked {
                chart.take_controlled_capture(generation).unwrap();
                chart.capture_completed(generation).unwrap();
            } else if state == Releasing {
                chart.release(generation).unwrap();
            }
            (chart, generation)
        };
        type Operation =
            fn(&mut FullScreenTransitionStateChart, FullScreenTransitionGeneration) -> bool;
        let operations: [Operation; 7] = [
            |chart, _| chart.begin(FullScreenTransitionOwner::Orientation).is_ok(),
            |chart, g| chart.take_controlled_capture(g).is_ok(),
            |chart, g| chart.capture_completed(g).is_ok(),
            |chart, g| chart.capture_deferred(g).is_ok(),
            |chart, g| chart.release(g).is_ok(),
            |chart, g| chart.live_frame_presented(g).is_ok(),
            |chart, g| chart.retain_redraw(g).is_ok(),
        ];
        let mut runs = BTreeSet::new();
        for from in [Live, CapturePending, SnapshotLocked, Releasing] {
            for issued in [false, true] {
                for operation in operations {
                    let (mut chart, generation) = in_state(from, issued);
                    operation(&mut chart, generation);
                    if chart.state() != from {
                        runs.insert((format!("{from:?}"), format!("{:?}", chart.state())));
                    }
                }
            }
        }
        assert_eq!(drawn, runs, "docs/architecture.md and the chart disagree");
    }

    #[test]
    fn the_generation_is_visible_only_to_the_owner_holding_it() {
        let mut chart = FullScreenTransitionStateChart::default();
        assert_eq!(
            chart.generation_for(FullScreenTransitionOwner::Navigation),
            None
        );
        let generation = chart.begin(FullScreenTransitionOwner::Navigation).unwrap();
        assert_eq!(
            chart.generation_for(FullScreenTransitionOwner::Navigation),
            Some(generation)
        );
        assert_eq!(
            chart.generation_for(FullScreenTransitionOwner::Orientation),
            None
        );
        chart.release(generation).unwrap();
        assert_eq!(
            chart.generation_for(FullScreenTransitionOwner::Navigation),
            Some(generation),
            "a releasing transition still owns its generation until the live frame lands"
        );
        chart.live_frame_presented(generation).unwrap();
        assert_eq!(
            chart.generation_for(FullScreenTransitionOwner::Navigation),
            None
        );
    }

    #[test]
    fn deferred_raster_restores_controlled_capture_authorization() {
        let mut chart = FullScreenTransitionStateChart::default();
        let generation = chart.begin(FullScreenTransitionOwner::Navigation).unwrap();
        assert!(chart.take_controlled_capture(generation).unwrap());
        assert!(chart.capture_issued());
        assert!(!chart.policy().controlled_capture);
        chart.capture_deferred(generation).unwrap();
        assert!(!chart.capture_issued());
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
