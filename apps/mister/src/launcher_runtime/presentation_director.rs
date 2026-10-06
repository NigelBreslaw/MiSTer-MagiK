// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `PresentationDirector` owns what decides what the launcher shows during
//! a full-screen transition: the motion of each owner (the navigation runtime
//! and the orientation runtime), the `FullScreenTransitionStateChart` (who may
//! render) and the `UiCompositionController` (what is on screen, and which
//! direct layers the presenter still owns). The launcher loop calls its methods
//! instead of pairing them by hand, so each pairing is written once and the host
//! walks in this module can drive exactly what the loop drives.
//!
//! The relations between the three are measured, not assumed; see the walks in
//! `tests` and `presentation_scenarios` and `docs/presentation-refactor-roadmap.md`.

use super::composition::{
    DirectLayerPresentationReceipt, UiCompositionController, UiCompositionDecision,
    UiCompositionInput,
};
use super::full_screen_transition::{
    FullScreenTransitionOwner, FullScreenTransitionPolicy, FullScreenTransitionState,
    FullScreenTransitionStateChart,
};
use super::navigation_transition::{
    NavigationTransitionCompletion, NavigationTransitionEndpoint, NavigationTransitionPhase,
    NavigationTransitionRuntime,
};
use super::orientation_transition::OrientationTransitionRuntime;
use crate::launcher::{LauncherEvent, NavigationTransitionState, Screen};
use crate::ui_display::ScreenOrientation;
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::Instant;

/// Give `owner` the full-screen transition. Returns whether it now holds it;
/// the refusal is logged.
pub fn begin_full_screen_transition(
    chart: &mut FullScreenTransitionStateChart,
    owner: FullScreenTransitionOwner,
) -> bool {
    match chart.begin(owner) {
        Ok(_) => true,
        Err(error) => {
            crate::ui_errln!("{owner:?} full-screen transition begin rejected: {error:?}");
            false
        }
    }
}

/// Ask the chart to force the live frame that ends `owner`'s transition.
pub fn release_full_screen_transition(
    chart: &mut FullScreenTransitionStateChart,
    owner: FullScreenTransitionOwner,
) {
    if let Some(generation) = chart.generation_for(owner)
        && let Err(error) = chart.release(generation)
    {
        crate::ui_errln!("{owner:?} full-screen transition release rejected: {error:?}");
    }
}

/// What the loop knows about the screen when composition is decided. The
/// director adds what it knows about the transition (whether one plays, whether
/// its destination is committed and whether it is ready), so the loop cannot
/// supply those inconsistently with the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompositionRequest {
    pub screensaver_active: bool,
    pub navigation_destination_layers_ready: bool,
    pub return_screen: Option<Screen>,
    pub confirm_visible: bool,
    pub fullscreen_overlay_visible: bool,
    pub arcade_ready: bool,
    pub route_ok: bool,
    pub wants_arcade_list: bool,
    pub wants_preview: bool,
    pub preview_cache_exact: bool,
    pub preview_frame_ready: bool,
}

/// A navigation transition that is playing and the navigation it will commit.
pub struct PendingNavigation {
    /// The intent the destination commits when it is ready.
    pub event: LauncherEvent,
    /// What to restore if the transition ends back at the source.
    pub source_state: NavigationTransitionState,
    pub source_was_arcade: bool,
    /// Whether `event` has been applied to the navigation state.
    pub committed: bool,
    pub status_quiesce_started_at: Option<Instant>,
}

/// Why an orientation transition was started, and so what to do when it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrientationIntent {
    Confirm,
    Rollback,
    Benchmark,
}

/// What the presenter acknowledged for the frame composition decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationOutcome {
    /// The latch confirmed the frame is the one being scanned out.
    Confirmed {
        sequence: u16,
        slot: u8,
        route_epoch: u16,
    },
    /// Visible without a latch confirmation (a backend that has none).
    Visible { sequence: u16 },
    /// Nothing the presenter acknowledged.
    Unacknowledged,
}

impl PresentationOutcome {
    /// Pick the acknowledgement for a presented frame: the latch's own
    /// confirmation when the frame was accepted and active, else a visible-frame
    /// acknowledgement when no latch trace flush is deferred, else none.
    pub fn resolve(
        latch_confirmed: bool,
        confirmed: Self,
        visible_without_latch: bool,
        frame_sequence: u16,
    ) -> Self {
        if latch_confirmed {
            confirmed
        } else if visible_without_latch {
            Self::Visible {
                sequence: frame_sequence,
            }
        } else {
            Self::Unacknowledged
        }
    }
}

pub struct PresentationDirector {
    pub navigation: NavigationTransitionRuntime,
    pub orientation: OrientationTransitionRuntime,
    pub chart: FullScreenTransitionStateChart,
    pub composition: UiCompositionController,
    /// Set exactly while a navigation transition the director adopted plays.
    pub pending: Option<PendingNavigation>,
    /// Set exactly while an orientation transition the director started plays.
    pub orientation_intent: Option<OrientationIntent>,
}

impl PresentationDirector {
    pub fn new(
        navigation: NavigationTransitionRuntime,
        orientation: OrientationTransitionRuntime,
    ) -> Self {
        Self {
            navigation,
            orientation,
            chart: FullScreenTransitionStateChart::default(),
            composition: UiCompositionController::new(),
            pending: None,
            orientation_intent: None,
        }
    }

    /// The presenter acknowledged the frame `decision` described: retire the
    /// direct layers it was waiting on. Returns whether a retirement completed.
    pub fn on_presented(
        &mut self,
        decision: &UiCompositionDecision,
        outcome: PresentationOutcome,
    ) -> bool {
        let (sequence, slot, route_epoch) = match outcome {
            PresentationOutcome::Confirmed {
                sequence,
                slot,
                route_epoch,
            } => (sequence, slot, route_epoch),
            PresentationOutcome::Visible { sequence } => (sequence, 0, 0),
            PresentationOutcome::Unacknowledged => return false,
        };
        self.composition.confirm_presented_layers(
            decision.retirement_generation,
            decision.direct_layers_desired,
            DirectLayerPresentationReceipt {
                sequence,
                slot,
                route_epoch,
                carrier: decision.retirement_carrier,
            },
        )
    }

    /// The presenter could not confirm the frame: whether its retirement
    /// completed is now uncertain.
    pub fn presentation_failed(&mut self, decision: &UiCompositionDecision) {
        if let Some(generation) = decision.retirement_generation {
            let _ = self.composition.mark_retirement_uncertain(generation);
        }
    }

    /// Start the orientation effect: the chart holds the frame, then the effect
    /// takes its source snapshot and the director remembers `intent`. `None`
    /// when the chart refuses; otherwise whether the effect animates. A
    /// transition that does not animate must be ended with `end_orientation` once
    /// the layout has changed.
    pub fn begin_orientation(
        &mut self,
        from: ScreenOrientation,
        to: ScreenOrientation,
        source: &[Rgb565Pixel],
        now: Instant,
        reduce_motion: bool,
        intent: OrientationIntent,
    ) -> Option<bool> {
        if !begin_full_screen_transition(&mut self.chart, FullScreenTransitionOwner::Orientation) {
            return None;
        }
        let animated = self.orientation.start(from, to, source, now, reduce_motion);
        self.orientation_intent = animated.then_some(intent);
        Some(animated)
    }

    /// Hand the orientation effect the captured destination and lock the
    /// chart's snapshot. If either refuses, the transition is aborted. Returns
    /// whether the capture took.
    pub fn capture_orientation_destination(&mut self, destination: &[Rgb565Pixel]) -> bool {
        let captured = self.orientation.capture_destination(destination)
            && match self
                .chart
                .generation_for(FullScreenTransitionOwner::Orientation)
            {
                Some(generation) => match self.chart.capture_completed(generation) {
                    Ok(()) => true,
                    Err(error) => {
                        crate::ui_errln!("orientation snapshot lock rejected: {error:?}");
                        false
                    }
                },
                None => true,
            };
        if !captured {
            self.abort_orientation();
        }
        captured
    }

    /// A Settings transition in the physical raster is waiting for its first
    /// controlled capture: the frame that takes the capture must also carry the
    /// source, so the hidden frame is rendered from the transition's snapshot.
    pub fn navigation_needs_source_carrier(&self, policy: FullScreenTransitionPolicy) -> bool {
        policy.controlled_capture
            && self.chart.owner() == Some(FullScreenTransitionOwner::Navigation)
            && self.navigation.frame().phase == NavigationTransitionPhase::Capture
            && self.navigation.settings_physical_space()
    }

    /// The orientation effect is waiting for its destination: until it has one,
    /// the controlled-capture frame carries the effect's source.
    pub fn orientation_needs_source_carrier(&self, policy: FullScreenTransitionPolicy) -> bool {
        policy.controlled_capture
            && self.chart.owner() == Some(FullScreenTransitionOwner::Orientation)
            && self.orientation.is_active()
            && !self.orientation.destination_ready()
    }

    /// The orientation effect is waiting for a controlled capture the frame did
    /// not take: abort it. Returns whether it aborted.
    pub fn abort_stalled_orientation_capture(&mut self, controlled_capture_rendered: bool) -> bool {
        let stalled = self.chart.owner() == Some(FullScreenTransitionOwner::Orientation)
            && self.chart.state() == FullScreenTransitionState::CapturePending
            && !self.chart.policy().controlled_capture
            && !controlled_capture_rendered;
        if stalled {
            self.abort_orientation();
        }
        stalled
    }

    /// The orientation effect ended: drop its completion, let the chart force
    /// the live frame, and hand back why it was started.
    pub fn end_orientation(&mut self) -> Option<OrientationIntent> {
        let _ = self.orientation.take_completion();
        release_full_screen_transition(&mut self.chart, FullScreenTransitionOwner::Orientation);
        self.orientation_intent.take()
    }

    /// The orientation effect cannot play (no capture, no snapshot lock): cancel
    /// it, forget why it was started, and let the chart force the live frame.
    pub fn abort_orientation(&mut self) {
        self.orientation.cancel();
        self.orientation_intent = None;
        release_full_screen_transition(&mut self.chart, FullScreenTransitionOwner::Orientation);
    }

    /// Whether the playing transition's destination has been committed.
    pub fn destination_committed(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.committed)
    }

    /// Decide what composition shows, with the transition facts taken from the
    /// runtime the director owns.
    pub fn compose(&mut self, request: CompositionRequest) -> UiCompositionDecision {
        let CompositionRequest {
            screensaver_active,
            navigation_destination_layers_ready,
            return_screen,
            confirm_visible,
            fullscreen_overlay_visible,
            arcade_ready,
            route_ok,
            wants_arcade_list,
            wants_preview,
            preview_cache_exact,
            preview_frame_ready,
        } = request;
        self.composition.tick(UiCompositionInput {
            screensaver_active,
            navigation_transition_active: self.navigation.is_active(),
            navigation_destination_committed: self.destination_committed(),
            navigation_destination_ready: self.navigation.destination_ready(),
            navigation_destination_layers_ready,
            return_screen,
            confirm_visible,
            fullscreen_overlay_visible,
            arcade_ready,
            route_ok,
            wants_arcade_list,
            wants_preview,
            preview_cache_exact,
            preview_frame_ready,
        })
    }

    /// The runtime has started a navigation transition: have the chart hold the
    /// frame for it and record what it will commit. When the chart refuses, the
    /// caller unwinds the transition.
    pub fn adopt_navigation(&mut self, pending: impl FnOnce() -> PendingNavigation) -> bool {
        if !begin_full_screen_transition(&mut self.chart, FullScreenTransitionOwner::Navigation) {
            return false;
        }
        self.pending = Some(pending());
        true
    }

    /// A navigation transition the runtime started but the chart refused to host:
    /// settle it where it is going and drop it, as if it had never played.
    pub fn unwind_navigation(&mut self) {
        self.navigation.settle_at_destination();
        let _ = self.navigation.complete();
    }

    /// Hand the runtime the captured destination and lock the chart's snapshot.
    /// If either refuses, the transition settles at its destination. Returns
    /// whether the capture took.
    pub fn capture_navigation_destination(
        &mut self,
        destination: &[Rgb565Pixel],
        now_us: u64,
    ) -> bool {
        let captured = self
            .navigation
            .capture_destination(destination, now_us)
            .is_ok()
            && self
                .chart
                .generation_for(FullScreenTransitionOwner::Navigation)
                .is_none_or(|generation| self.chart.capture_completed(generation).is_ok());
        if !captured {
            self.navigation.settle_at_destination();
        }
        captured
    }

    /// End a settled navigation transition: take its completion and, if there was
    /// one, release the chart.
    pub fn finish_navigation(&mut self) -> Option<NavigationTransitionCompletion> {
        let completion = self.navigation.complete();
        if completion.is_some() {
            release_full_screen_transition(&mut self.chart, FullScreenTransitionOwner::Navigation);
        }
        completion
    }

    /// A screensaver or confirmation now covers a playing navigation transition.
    /// It cannot keep playing under the cover: settle it at the destination if
    /// that was already committed, otherwise cancel it back to the source, then
    /// end it, release the chart and forget what it would have committed, all
    /// before composition is asked. Returns the endpoint it ended at.
    pub fn cover_navigation(&mut self) -> Option<NavigationTransitionEndpoint> {
        let endpoint = if self.destination_committed() {
            self.navigation.settle_at_destination();
            Some(NavigationTransitionEndpoint::Destination)
        } else {
            self.navigation.cancel_for_exclusive_view()
        };
        let _ = self.finish_navigation();
        self.pending = None;
        endpoint
    }
}

#[cfg(test)]
mod presentation_scenarios;

#[cfg(test)]
mod tests {
    use super::super::composition::UiCompositionState;
    use super::super::full_screen_transition::FullScreenTransitionState;
    use super::super::navigation_transition::{
        NavigationTransitionDirection, NavigationTransitionEdge, NavigationTransitionPhase,
    };
    use super::super::transition_scenarios::{DIRECTIONS, EDGES, HEIGHT, WIDTH, begin, frame};
    use super::super::walk_rng::WalkRng as Rng;
    use super::*;
    use crate::launcher::{LauncherAction, LauncherNav};
    use std::collections::BTreeSet;
    use std::time::Duration;

    const WALKS: usize = 300;
    const STEPS: usize = 80;

    pub(super) fn director() -> PresentationDirector {
        PresentationDirector::new(
            NavigationTransitionRuntime::new(WIDTH, HEIGHT, true),
            OrientationTransitionRuntime::new(WIDTH, HEIGHT),
        )
    }

    /// Start the way the loop does: the runtime first, then the chart, and a
    /// transition the chart refuses is unwound.
    pub(super) fn start(
        director: &mut PresentationDirector,
        edge: NavigationTransitionEdge,
        direction: NavigationTransitionDirection,
        source: &[Rgb565Pixel],
        now_us: u64,
    ) -> bool {
        if !begin(&mut director.navigation, edge, direction, source, now_us) {
            return false;
        }
        if director.adopt_navigation(|| PendingNavigation {
            event: LauncherEvent {
                action: LauncherAction::NavigateBack,
                path: None,
                settings: None,
            },
            source_state: LauncherNav::new().navigation_transition_state(),
            source_was_arcade: false,
            committed: false,
            status_quiesce_started_at: None,
        }) {
            true
        } else {
            director.unwind_navigation();
            false
        }
    }

    /// The runtime and the chart agree on who is playing and which stage it is in.
    fn check(director: &PresentationDirector, seen: &mut BTreeSet<String>, at: &str) {
        use FullScreenTransitionState::*;
        let (runtime, chart) = (&director.navigation, &director.chart);
        let phase = runtime.frame().phase;
        seen.insert(format!("{:?} + {phase:?}", chart.state()));
        if runtime.is_active() {
            assert_eq!(
                chart.owner(),
                Some(FullScreenTransitionOwner::Navigation),
                "{at}: an active transition has the chart"
            );
            assert!(
                matches!(chart.state(), CapturePending | SnapshotLocked),
                "{at}: {:?} while playing",
                chart.state()
            );
        } else {
            assert!(
                matches!(chart.state(), Live | Releasing),
                "{at}: {:?} with nothing playing",
                chart.state()
            );
        }
        // The timeline may run ahead of the snapshot lock (an immediate start
        // plays while the destination is prepared), but it never reveals one.
        if chart.state() == CapturePending {
            assert_ne!(
                phase,
                NavigationTransitionPhase::Reveal,
                "{at}: revealed a destination the chart has not locked"
            );
        }
        if chart.state() == SnapshotLocked {
            assert_ne!(
                phase,
                NavigationTransitionPhase::Capture,
                "{at}: a locked snapshot is past capture"
            );
        }
    }

    #[test]
    fn random_walks_keep_the_runtime_and_the_chart_in_step() {
        let source = frame(0x1111);
        let destination = frame(0x2222);
        let mut rng = Rng(0x7E57_0DD5_1CE5_BEEF);
        let mut seen = BTreeSet::new();
        let mut finished = 0;
        let mut unwound = 0;
        for walk in 0..WALKS {
            let mut d = director();
            let mut now_us = 0u64;
            for step in 0..STEPS {
                let at = format!("walk {walk} step {step}");
                now_us += [0, 5_000, 40_000, 400_000, 10_000_000][rng.below(5)];
                match rng.below(10) {
                    9 => {
                        // The loop authorizes the controlled capture before it renders it.
                        if let Some(generation) = d
                            .chart
                            .generation_for(FullScreenTransitionOwner::Navigation)
                            && d.chart.state() == FullScreenTransitionState::CapturePending
                        {
                            d.chart.take_controlled_capture(generation).unwrap();
                        }
                    }
                    0 | 1 => {
                        let edge = EDGES[rng.below(EDGES.len())];
                        let direction = DIRECTIONS[rng.below(DIRECTIONS.len())];
                        let chart_was_free = d.chart.is_live();
                        let started = start(&mut d, edge, direction, &source, now_us);
                        if !started && !d.navigation.is_active() && !chart_was_free {
                            unwound += 1;
                        }
                        assert!(
                            !started || chart_was_free,
                            "{at}: began over a releasing chart"
                        );
                    }
                    2 | 3 => {
                        d.capture_navigation_destination(&destination, now_us);
                    }
                    4 => {
                        d.navigation.tick(now_us);
                    }
                    5 => {
                        d.navigation.request_reverse(now_us);
                    }
                    6 => {
                        if rng.chance(50) {
                            d.navigation.settle_at_destination();
                        } else {
                            d.navigation.cancel_for_exclusive_view();
                        }
                    }
                    7 => {
                        let before = d.navigation.frame().phase;
                        if d.finish_navigation().is_some() {
                            finished += 1;
                            assert_eq!(before, NavigationTransitionPhase::Settled, "{at}");
                            assert_eq!(
                                d.chart.state(),
                                FullScreenTransitionState::Releasing,
                                "{at}"
                            );
                        }
                    }
                    _ => {
                        if let Some(generation) = d.chart.generation()
                            && d.chart.state() == FullScreenTransitionState::Releasing
                        {
                            d.chart.live_frame_presented(generation).unwrap();
                        }
                    }
                }
                check(&d, &mut seen, &at);
            }
        }
        assert!(finished > 100, "finished {finished}");
        assert!(unwound > 5, "unwound {unwound}");
        // The chart's state is not a function of the timeline's phase: an immediate
        // start plays Expand through Settled while the chart still awaits its
        // snapshot, and a locked snapshot spans every phase from Expand on.
        let expected: BTreeSet<String> = [
            "Live + Idle",
            "CapturePending + Capture",
            "CapturePending + Expand",
            "CapturePending + Covered",
            "CapturePending + Reversing",
            "CapturePending + Settled",
            "SnapshotLocked + Expand",
            "SnapshotLocked + Covered",
            "SnapshotLocked + Reveal",
            "SnapshotLocked + Reversing",
            "SnapshotLocked + Settled",
            "Releasing + Idle",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn covering_a_playing_transition_ends_it_at_the_right_endpoint_and_releases_the_chart() {
        let source = frame(0x1111);
        let destination = frame(0x2222);
        for committed in [false, true] {
            let mut d = director();
            assert!(start(
                &mut d,
                NavigationTransitionEdge::HomeToConsoles,
                NavigationTransitionDirection::Forward,
                &source,
                0,
            ));
            let generation = d
                .chart
                .generation_for(FullScreenTransitionOwner::Navigation)
                .unwrap();
            d.chart.take_controlled_capture(generation).unwrap();
            assert!(d.capture_navigation_destination(&destination, 10_000));
            d.pending.as_mut().unwrap().committed = committed;
            let endpoint = d.cover_navigation();
            assert_eq!(
                endpoint,
                Some(if committed {
                    NavigationTransitionEndpoint::Destination
                } else {
                    NavigationTransitionEndpoint::Source
                })
            );
            assert!(!d.navigation.is_active(), "committed={committed}");
            assert!(d.pending.is_none(), "committed={committed}");
            assert_eq!(
                d.chart.state(),
                FullScreenTransitionState::Releasing,
                "committed={committed}"
            );
        }
    }

    /// The orientation effect and the chart agree on who is playing, whether the
    /// destination is captured and why the transition was started.
    fn check_orientation(d: &PresentationDirector, seen: &mut BTreeSet<String>, at: &str) {
        use FullScreenTransitionState::*;
        let (effect, chart) = (&d.orientation, &d.chart);
        // The destination flag is only meaningful while the effect plays; it is
        // left stale after one ends, which nothing reads.
        seen.insert(format!(
            "{:?}/{:?} + {}",
            chart.owner(),
            chart.state(),
            match (effect.is_active(), effect.destination_ready()) {
                (false, _) => "idle",
                (true, false) => "playing, awaiting its destination",
                (true, true) => "playing, destination captured",
            }
        ));
        assert_eq!(
            effect.is_active(),
            d.orientation_intent.is_some(),
            "{at}: the intent lives exactly while the effect plays"
        );
        if effect.is_active() {
            assert_eq!(
                chart.owner(),
                Some(FullScreenTransitionOwner::Orientation),
                "{at}"
            );
            assert!(
                matches!(chart.state(), CapturePending | SnapshotLocked),
                "{at}: {:?} while playing",
                chart.state()
            );
            assert_eq!(
                effect.destination_ready(),
                chart.state() == SnapshotLocked,
                "{at}: the destination is captured exactly when the snapshot is locked"
            );
        } else if chart.owner() == Some(FullScreenTransitionOwner::Orientation) {
            assert_eq!(
                chart.state(),
                Releasing,
                "{at}: nothing plays but the chart waits"
            );
        }
    }

    #[test]
    fn random_walks_keep_the_orientation_effect_and_the_chart_in_step() {
        // A small effect keeps the walk quick; the contract does not depend on size.
        const W: usize = 64;
        const H: usize = 36;
        let navigation_source = frame(0x1111);
        let source = vec![Rgb565Pixel(0x3333); W * H];
        let destination = vec![Rgb565Pixel(0x4444); W * H];
        let short = vec![Rgb565Pixel(0x5555); 16];
        let mut rng = Rng(0x0DD5_B16B_00B5_C0DE);
        let mut seen = BTreeSet::new();
        let (mut finished, mut aborted, mut refused) = (0, 0, 0);
        for walk in 0..WALKS {
            let mut d = director();
            d.orientation = OrientationTransitionRuntime::new(W, H);
            let mut now = Instant::now();
            for step in 0..STEPS {
                let at = format!("walk {walk} step {step}");
                now += [
                    Duration::ZERO,
                    Duration::from_millis(40),
                    Duration::from_secs(10),
                ][rng.below(3)];
                match rng.below(9) {
                    0 | 1 => {
                        let chart_was_free = d.chart.is_live();
                        let intent = [
                            OrientationIntent::Confirm,
                            OrientationIntent::Rollback,
                            OrientationIntent::Benchmark,
                        ][rng.below(3)];
                        let began = d.begin_orientation(
                            ScreenOrientation::Normal,
                            ScreenOrientation::MonitorClockwise,
                            &source,
                            now,
                            rng.chance(20),
                            intent,
                        );
                        match began {
                            None => {
                                assert!(!chart_was_free, "{at}: refused a free chart");
                                refused += 1;
                            }
                            Some(animated) => {
                                assert!(chart_was_free, "{at}: began over an owner");
                                // The loop ends a transition that does not animate
                                // once the layout has changed.
                                if !animated {
                                    assert!(
                                        d.orientation_intent.is_none(),
                                        "{at}: a transition that does not animate has no intent"
                                    );
                                    d.end_orientation();
                                }
                            }
                        }
                    }
                    2 => {
                        if let Some(generation) = d
                            .chart
                            .generation_for(FullScreenTransitionOwner::Orientation)
                            && d.chart.state() == FullScreenTransitionState::CapturePending
                        {
                            d.chart.take_controlled_capture(generation).unwrap();
                        }
                    }
                    3 | 4 => {
                        let pixels = if rng.chance(15) { &short } else { &destination };
                        let was_active = d.orientation.is_active();
                        let took = d.capture_orientation_destination(pixels);
                        if was_active && !took {
                            aborted += 1;
                        }
                    }
                    5 => {
                        let rendered = rng.chance(50);
                        let stalled = d.abort_stalled_orientation_capture(rendered);
                        assert!(
                            !(stalled && rendered),
                            "{at}: a rendered capture is not stalled"
                        );
                        if stalled {
                            aborted += 1;
                            assert!(
                                !d.orientation.is_active(),
                                "{at}: a stalled capture stops the effect"
                            );
                        }
                    }
                    6 => {
                        let mut output = vec![Rgb565Pixel(0); W * H];
                        if let Some((true, ..)) = d.orientation.render_into(&mut output, now) {
                            assert!(
                                d.end_orientation().is_some(),
                                "{at}: done without an intent"
                            );
                            finished += 1;
                        }
                    }
                    7 => {
                        if let Some(generation) = d.chart.generation()
                            && d.chart.state() == FullScreenTransitionState::Releasing
                        {
                            d.chart.live_frame_presented(generation).unwrap();
                        }
                    }
                    _ => {
                        // A navigation transition cannot begin over an owner.
                        let owner_held = !d.chart.is_live();
                        let started = start(
                            &mut d,
                            NavigationTransitionEdge::HomeToConsoles,
                            NavigationTransitionDirection::Forward,
                            &navigation_source,
                            0,
                        );
                        assert!(
                            !(started && owner_held),
                            "{at}: navigation began over an owner"
                        );
                        if started {
                            // Keep the walk about orientation: end it as a cover does.
                            d.cover_navigation();
                            let generation = d.chart.generation().unwrap();
                            d.chart.live_frame_presented(generation).unwrap();
                        }
                    }
                }
                check_orientation(&d, &mut seen, &at);
            }
        }
        assert!(finished > 20, "finished {finished}");
        assert!(aborted > 20, "aborted {aborted}");
        assert!(refused > 20, "refused {refused}");
        // Unlike navigation, the orientation effect maps one-to-one onto the
        // chart: awaiting the destination is `CapturePending`, a captured one is
        // `SnapshotLocked`, and an ended effect leaves the chart `Releasing`.
        let expected: BTreeSet<String> = [
            "None/Live + idle",
            "Some(Orientation)/CapturePending + playing, awaiting its destination",
            "Some(Orientation)/SnapshotLocked + playing, destination captured",
            "Some(Orientation)/Releasing + idle",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(seen, expected);
    }

    fn started_orientation() -> PresentationDirector {
        let mut d = director();
        let frame = vec![Rgb565Pixel(0x1111); WIDTH * HEIGHT];
        assert_eq!(
            d.begin_orientation(
                ScreenOrientation::Normal,
                ScreenOrientation::MonitorClockwise,
                &frame,
                Instant::now(),
                false,
                OrientationIntent::Confirm,
            ),
            Some(true)
        );
        d
    }

    #[test]
    fn a_physical_settings_capture_uses_one_source_carrier_only_while_capture_is_pending() {
        use super::super::navigation_transition::NavigationTransitionRoute;
        use super::super::transition_spec::TransitionStart;
        use mister_magik_framebuffer_scenes::navigation::SettingsPageTransitionAxis;
        let source = frame(0x1111);
        let mut d = director();
        let policy_before = d.chart.policy();
        assert!(
            !d.navigation_needs_source_carrier(policy_before),
            "nothing plays"
        );

        assert!(
            d.navigation
                .begin(TransitionStart::settings_page_physical(
                    NavigationTransitionRoute::HomeToSettings,
                    NavigationTransitionDirection::Forward,
                    SettingsPageTransitionAxis::Horizontal,
                    WIDTH,
                    HEIGHT,
                    &source,
                    0,
                ))
                .unwrap()
        );
        assert!(begin_full_screen_transition(
            &mut d.chart,
            FullScreenTransitionOwner::Navigation
        ));
        let capture_policy = d.chart.policy();
        assert!(d.navigation_needs_source_carrier(capture_policy));
        assert!(
            !d.orientation_needs_source_carrier(capture_policy),
            "navigation owns the frame, not orientation"
        );
        // The policy a caller holds is what decides; a stale one without the
        // capture authorization asks for nothing.
        assert!(!d.navigation_needs_source_carrier(policy_before));

        // The frame must belong to navigation: the same runtime state under
        // another owner needs no navigation carrier.
        let mut other = director();
        assert!(
            other
                .navigation
                .begin(TransitionStart::settings_page_physical(
                    NavigationTransitionRoute::HomeToSettings,
                    NavigationTransitionDirection::Forward,
                    SettingsPageTransitionAxis::Horizontal,
                    WIDTH,
                    HEIGHT,
                    &source,
                    0,
                ))
                .unwrap()
        );
        assert!(begin_full_screen_transition(
            &mut other.chart,
            FullScreenTransitionOwner::Orientation
        ));
        assert!(!other.navigation_needs_source_carrier(other.chart.policy()));

        // Only the physical raster needs it: a logical Settings slide does not.
        let mut logical = director();
        assert!(
            logical
                .navigation
                .begin(TransitionStart::settings_page(
                    NavigationTransitionRoute::HomeToSettings,
                    NavigationTransitionDirection::Forward,
                    &source,
                    0,
                ))
                .unwrap()
        );
        assert!(begin_full_screen_transition(
            &mut logical.chart,
            FullScreenTransitionOwner::Navigation
        ));
        assert!(!logical.navigation_needs_source_carrier(logical.chart.policy()));

        let generation = d
            .chart
            .generation_for(FullScreenTransitionOwner::Navigation)
            .unwrap();
        assert!(d.chart.take_controlled_capture(generation).unwrap());
        assert!(
            !d.navigation_needs_source_carrier(d.chart.policy()),
            "once the capture is taken the carrier is no longer needed"
        );
    }

    #[test]
    fn an_orientation_capture_uses_the_source_carrier_until_its_destination_is_ready() {
        let mut d = started_orientation();
        let capture_policy = d.chart.policy();
        assert!(d.orientation_needs_source_carrier(capture_policy));
        assert!(
            !d.navigation_needs_source_carrier(capture_policy),
            "orientation owns the frame, not navigation"
        );
        let generation = d
            .chart
            .generation_for(FullScreenTransitionOwner::Orientation)
            .unwrap();
        assert!(d.chart.take_controlled_capture(generation).unwrap());
        assert!(
            !d.orientation_needs_source_carrier(d.chart.policy()),
            "the capture is taken"
        );
        // The effect must be playing and the frame must be orientation's.
        let mut idle = director();
        assert_eq!(
            idle.begin_orientation(
                ScreenOrientation::Normal,
                ScreenOrientation::MonitorClockwise,
                &vec![Rgb565Pixel(0x1111); WIDTH * HEIGHT],
                Instant::now(),
                true, // reduce motion: the effect does not play
                OrientationIntent::Confirm,
            ),
            Some(false)
        );
        assert!(!idle.orientation_needs_source_carrier(idle.chart.policy()));
        let mut foreign = director();
        assert!(foreign.orientation.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &vec![Rgb565Pixel(0x1111); WIDTH * HEIGHT],
            Instant::now(),
            false,
        ));
        assert!(begin_full_screen_transition(
            &mut foreign.chart,
            FullScreenTransitionOwner::Navigation
        ));
        assert!(!foreign.orientation_needs_source_carrier(foreign.chart.policy()));

        // Deferred: the capture comes back and the carrier with it, until the
        // destination is in.
        d.chart.capture_deferred(generation).unwrap();
        assert!(d.orientation_needs_source_carrier(d.chart.policy()));
        assert!(d.chart.take_controlled_capture(generation).unwrap());
        assert!(d.capture_orientation_destination(&vec![Rgb565Pixel(0x4444); WIDTH * HEIGHT]));
        assert!(
            !d.orientation_needs_source_carrier(capture_policy),
            "with its destination ready the effect needs no carrier"
        );
    }

    #[test]
    fn aborting_an_orientation_transition_stops_the_effect_and_releases_the_chart() {
        let mut d = started_orientation();
        assert!(d.orientation.is_active());
        d.abort_orientation();
        assert!(!d.orientation.is_active());
        assert_eq!(d.chart.state(), FullScreenTransitionState::Releasing);
        assert_eq!(
            d.chart.owner(),
            Some(FullScreenTransitionOwner::Orientation)
        );
    }

    #[test]
    fn ending_an_orientation_transition_releases_the_chart() {
        let mut d = started_orientation();
        d.end_orientation();
        assert_eq!(d.chart.state(), FullScreenTransitionState::Releasing);
    }

    #[test]
    fn a_second_owner_cannot_begin_until_the_first_is_confirmed_live() {
        let mut d = started_orientation();
        assert!(!begin_full_screen_transition(
            &mut d.chart,
            FullScreenTransitionOwner::Navigation
        ));
        d.abort_orientation();
        assert!(!begin_full_screen_transition(
            &mut d.chart,
            FullScreenTransitionOwner::Navigation
        ));
        let generation = d
            .chart
            .generation_for(FullScreenTransitionOwner::Orientation)
            .unwrap();
        d.chart.live_frame_presented(generation).unwrap();
        assert!(begin_full_screen_transition(
            &mut d.chart,
            FullScreenTransitionOwner::Navigation
        ));
    }

    #[test]
    fn orientation_cannot_begin_over_a_navigation_transition() {
        let source = frame(0x1111);
        let mut d = director();
        assert!(start(
            &mut d,
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
            &source,
            0,
        ));
        assert_eq!(
            d.begin_orientation(
                ScreenOrientation::Normal,
                ScreenOrientation::MonitorClockwise,
                &source,
                Instant::now(),
                false,
                OrientationIntent::Confirm,
            ),
            None
        );
        assert!(!d.orientation.is_active());
    }

    fn request(screensaver: bool, arcade: bool) -> CompositionRequest {
        CompositionRequest {
            screensaver_active: screensaver,
            navigation_destination_layers_ready: false,
            return_screen: Some(if arcade { Screen::Arcade } else { Screen::Home }),
            confirm_visible: false,
            fullscreen_overlay_visible: false,
            arcade_ready: arcade,
            route_ok: true,
            wants_arcade_list: arcade && !screensaver,
            wants_preview: false,
            preview_cache_exact: false,
            preview_frame_ready: false,
        }
    }

    #[test]
    fn the_director_retires_direct_layers_only_on_an_acknowledged_frame() {
        let confirmed = PresentationOutcome::Confirmed {
            sequence: 5,
            slot: 1,
            route_epoch: 1,
        };
        let mut d = director();
        let live = d.compose(request(false, true));
        assert_eq!(live.state, UiCompositionState::MixedArcade);
        assert!(!d.on_presented(&live, confirmed), "nothing was retiring");

        // The screensaver takes the frame: the Arcade layer must retire.
        let covered = d.compose(request(true, true));
        assert_eq!(covered.state, UiCompositionState::Screensaver);
        assert!(covered.retirement_generation.is_some());
        assert!(
            !d.on_presented(&covered, PresentationOutcome::Unacknowledged),
            "an unacknowledged frame retires nothing"
        );
        d.presentation_failed(&covered);
        let still = d.compose(request(true, true));
        assert!(
            still.retirement_generation.is_some(),
            "the retirement stays pending until a frame is acknowledged"
        );
        let retired = d.on_presented(
            &still,
            PresentationOutcome::Confirmed {
                sequence: 6,
                slot: 2,
                route_epoch: 1,
            },
        );
        assert_eq!(
            still.status().retirement_state,
            "reconciling",
            "a failed presentation leaves the retirement uncertain"
        );
        assert!(retired, "an acknowledged frame retires the layer");
        let settled = d.compose(request(true, true));
        assert_eq!(
            settled.retirement_generation, None,
            "nothing is left retiring"
        );
        let status = settled.status();
        assert_eq!(status.retirement_state, "idle");
        assert!(
            status.retirement_receipt.contains("carrier=Screensaver"),
            "the receipt names the frame that carried the retirement: {}",
            status.retirement_receipt
        );
    }

    #[test]
    fn the_acknowledgement_for_a_presented_frame_is_the_latchs_when_it_confirmed() {
        let confirmed = PresentationOutcome::Confirmed {
            sequence: 9,
            slot: 2,
            route_epoch: 4,
        };
        assert_eq!(
            PresentationOutcome::resolve(true, confirmed, true, 7),
            confirmed
        );
        assert_eq!(
            PresentationOutcome::resolve(false, confirmed, true, 7),
            PresentationOutcome::Visible { sequence: 7 }
        );
        assert_eq!(
            PresentationOutcome::resolve(false, confirmed, false, 7),
            PresentationOutcome::Unacknowledged
        );
        // A latch confirmation that carries no receipt acknowledges nothing.
        assert_eq!(
            PresentationOutcome::resolve(true, PresentationOutcome::Unacknowledged, true, 7),
            PresentationOutcome::Unacknowledged
        );
    }
}
