// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Seeded random walks over `FullScreenTransitionStateChart` against an
//! independent model. Every operation is tried with the live generation, an
//! older one and one never issued, from every state, for every owner. The
//! contract being checked:
//!
//! * one owner at a time; a second `begin` is refused until the first has been
//!   released and its live frame confirmed;
//! * a stale generation never changes anything;
//! * capture can be authorized once, deferred and re-authorized, and completes
//!   only after it was issued;
//! * release is possible from any non-`Live` state and `Live` returns only after
//!   the forced live frame is confirmed, handing back the retained redraw;
//! * the render policy of each state: Slint timers and automatic raster run only
//!   in `Live`, and every other state is frame driven.

use super::*;

const WALKS: usize = 400;
const STEPS: usize = 120;

#[derive(Clone, Copy, Debug)]
struct Model {
    state: FullScreenTransitionState,
    owner: Option<FullScreenTransitionOwner>,
    generation: Option<FullScreenTransitionGeneration>,
    capture_issued: bool,
    retained_redraw: bool,
}

impl Model {
    fn live() -> Self {
        Self {
            state: FullScreenTransitionState::Live,
            owner: None,
            generation: None,
            capture_issued: false,
            retained_redraw: false,
        }
    }

    fn current(&self, generation: FullScreenTransitionGeneration) -> bool {
        self.generation == Some(generation)
    }
}

use super::super::walk_rng::WalkRng as Rng;

const OWNERS: [FullScreenTransitionOwner; 2] = [
    FullScreenTransitionOwner::Navigation,
    FullScreenTransitionOwner::Orientation,
];

fn check_policy(chart: &FullScreenTransitionStateChart, model: &Model, at: &str) {
    use FullScreenTransitionState::*;
    let policy = chart.policy();
    assert_eq!(chart.state(), model.state, "{at}");
    assert_eq!(chart.is_live(), model.state == Live, "{at}");
    assert_eq!(chart.owner(), model.owner, "{at}");
    assert_eq!(chart.generation(), model.generation, "{at}");
    assert_eq!(policy.advance_slint_timers, model.state == Live, "{at}");
    assert_eq!(policy.automatic_slint_raster, model.state == Live, "{at}");
    assert_eq!(policy.frame_driven_motion, model.state != Live, "{at}");
    assert_eq!(
        policy.controlled_capture,
        model.state == CapturePending && !model.capture_issued,
        "{at}"
    );
    assert_eq!(
        policy.snapshot_locked,
        model.state == SnapshotLocked,
        "{at}"
    );
    assert_eq!(policy.force_live_raster, model.state == Releasing, "{at}");
    assert_eq!(
        chart.capture_issued(),
        model.owner.is_some() && model.capture_issued,
        "{at}"
    );
}

#[test]
fn random_walks_match_the_model_for_every_owner_and_generation() {
    use FullScreenTransitionError::*;
    use FullScreenTransitionState::*;

    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let mut reached = std::collections::BTreeSet::new();
    let mut handed_back_redraws = 0;
    for walk in 0..WALKS {
        let mut chart = FullScreenTransitionStateChart::default();
        let mut model = Model::live();
        let mut issued: Vec<FullScreenTransitionGeneration> = Vec::new();
        for step in 0..STEPS {
            let at = format!("walk {walk} step {step}");
            // A generation: the live one, an older one, or one never issued.
            let generation = match rng.below(4) {
                0 | 1 if model.generation.is_some() => model.generation.unwrap(),
                2 if !issued.is_empty() => issued[rng.below(issued.len())],
                _ => FullScreenTransitionGeneration(1_000_000 + rng.below(50) as u64),
            };
            match rng.below(8) {
                0 => {
                    let owner = OWNERS[rng.below(OWNERS.len())];
                    let result = chart.begin(owner);
                    if model.state == Live && model.generation.is_none() {
                        let started = result.expect("begin from Live");
                        assert!(!issued.contains(&started), "{at}: generations are unique");
                        issued.push(started);
                        model = Model {
                            state: CapturePending,
                            owner: Some(owner),
                            generation: Some(started),
                            capture_issued: false,
                            retained_redraw: false,
                        };
                    } else {
                        assert_eq!(result, Err(OwnerActive), "{at}: nested owner refused");
                    }
                }
                1 => {
                    let result = chart.take_controlled_capture(generation);
                    if model.state != CapturePending {
                        assert_eq!(result, Err(InvalidState), "{at}");
                    } else if !model.current(generation) {
                        assert_eq!(result, Err(StaleGeneration), "{at}");
                    } else {
                        assert_eq!(result, Ok(!model.capture_issued), "{at}");
                        model.capture_issued = true;
                    }
                }
                2 => {
                    let result = chart.capture_completed(generation);
                    if model.state != CapturePending {
                        assert_eq!(result, Err(InvalidState), "{at}");
                    } else if !model.current(generation) {
                        assert_eq!(result, Err(StaleGeneration), "{at}");
                    } else if !model.capture_issued {
                        assert_eq!(result, Err(CaptureNotIssued), "{at}");
                    } else {
                        assert_eq!(result, Ok(()), "{at}");
                        model.state = SnapshotLocked;
                    }
                }
                3 => {
                    let result = chart.capture_deferred(generation);
                    if model.state != CapturePending {
                        assert_eq!(result, Err(InvalidState), "{at}");
                    } else if !model.current(generation) {
                        assert_eq!(result, Err(StaleGeneration), "{at}");
                    } else if !model.capture_issued {
                        assert_eq!(result, Err(CaptureNotIssued), "{at}");
                    } else {
                        assert_eq!(result, Ok(()), "{at}");
                        model.capture_issued = false;
                    }
                }
                4 => {
                    let result = chart.release(generation);
                    if !model.current(generation) {
                        assert_eq!(result, Err(StaleGeneration), "{at}");
                    } else {
                        // Release is legal from every non-Live state, and is idempotent.
                        assert_eq!(result, Ok(()), "{at}");
                        model.state = Releasing;
                    }
                }
                5 => {
                    let result = chart.live_frame_presented(generation);
                    if model.state != Releasing {
                        assert_eq!(result, Err(InvalidState), "{at}");
                    } else if !model.current(generation) {
                        assert_eq!(result, Err(StaleGeneration), "{at}");
                    } else {
                        assert_eq!(result, Ok(model.retained_redraw), "{at}");
                        handed_back_redraws += usize::from(model.retained_redraw);
                        model = Model::live();
                    }
                }
                6 => {
                    let result = chart.retain_redraw(generation);
                    if model.current(generation) {
                        assert_eq!(result, Ok(()), "{at}");
                        model.retained_redraw = true;
                    } else {
                        assert_eq!(result, Err(StaleGeneration), "{at}");
                    }
                }
                _ => {
                    // Quiescent step: only the policy is checked below.
                }
            }
            reached.insert(format!("{:?}", model.state));
            check_policy(&chart, &model, &at);
        }
    }
    for state in ["Live", "CapturePending", "SnapshotLocked", "Releasing"] {
        assert!(
            reached.contains(state),
            "never reached {state}: {reached:?}"
        );
    }
    assert!(handed_back_redraws > 50, "{handed_back_redraws}");
}

#[test]
fn every_owner_completes_the_same_lifecycle() {
    for owner in OWNERS {
        let mut chart = FullScreenTransitionStateChart::default();
        let generation = chart.begin(owner).unwrap();
        assert_eq!(chart.owner(), Some(owner));
        assert!(chart.take_controlled_capture(generation).unwrap());
        chart.capture_completed(generation).unwrap();
        chart.release(generation).unwrap();
        chart.live_frame_presented(generation).unwrap();
        assert_eq!(chart.owner(), None);
        assert_eq!(chart.state(), FullScreenTransitionState::Live);
    }
}
