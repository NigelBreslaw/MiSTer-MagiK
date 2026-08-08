// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::launcher::Screen;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiCompositionState {
    FullSlint,
    MixedArcade,
    NavigationTransition,
    NavigationDestination,
    Screensaver,
    ModalFullSlint,
    ModalOverArcade,
    Recovering,
}

impl UiCompositionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::FullSlint => "full-slint",
            Self::MixedArcade => "mixed-arcade",
            Self::NavigationTransition => "navigation-transition",
            Self::NavigationDestination => "navigation-destination",
            Self::Screensaver => "screensaver",
            Self::ModalFullSlint => "modal-full-slint",
            Self::ModalOverArcade => "modal-over-arcade",
            Self::Recovering => "recovering",
        }
    }

    fn allows_direct_layers(self) -> bool {
        matches!(self, Self::MixedArcade | Self::ModalOverArcade)
    }
}

#[derive(Clone, Debug)]
pub struct UiCompositionStatus {
    pub state: &'static str,
    pub preview_state: &'static str,
    pub preview_generation: u64,
    pub retirement_state: &'static str,
    pub retirement_generation: u64,
    pub retirement_obligations: &'static str,
    pub retirement_receipt: String,
    pub retirement_receipt_sequence: u16,
    pub retirement_receipt_slot: u8,
    pub retirement_receipt_route_epoch: u16,
    pub recovery_count: u64,
    pub last_invariant_kind: String,
    pub last_invariant_detail: String,
}

impl Default for UiCompositionStatus {
    fn default() -> Self {
        Self {
            state: UiCompositionState::FullSlint.label(),
            preview_state: "detached",
            preview_generation: 0,
            retirement_state: "idle",
            retirement_generation: 0,
            retirement_obligations: "none",
            retirement_receipt: String::new(),
            retirement_receipt_sequence: 0,
            retirement_receipt_slot: 0,
            retirement_receipt_route_epoch: 0,
            recovery_count: 0,
            last_invariant_kind: String::new(),
            last_invariant_detail: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiCompositionInput {
    pub screensaver_active: bool,
    pub navigation_transition_active: bool,
    pub navigation_destination_committed: bool,
    pub navigation_destination_ready: bool,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiCompositionDecision {
    pub state: UiCompositionState,
    pub allow_arcade_list_blit: bool,
    pub allow_preview_blit: bool,
    pub transition_owns_full_frame: bool,
    pub prepare_navigation_destination: bool,
    pub force_full_slint_raster: bool,
    pub force_full_slint_present: bool,
    pub clear_direct_layers: bool,
    pub direct_layers_desired: DirectLayerObligations,
    pub retirement_generation: Option<u64>,
    pub retirement_carrier: DirectLayerCarrier,
    retirement_state: &'static str,
    retirement_obligations: DirectLayerObligations,
    retirement_receipt: Option<DirectLayerPresentationReceipt>,
    pub recovery_count: u64,
    pub last_invariant_kind: String,
    pub last_invariant_detail: String,
    pub events: Vec<UiCompositionEvent>,
}

impl UiCompositionDecision {
    pub fn status(&self) -> UiCompositionStatus {
        UiCompositionStatus {
            state: self.state.label(),
            preview_state: "detached",
            preview_generation: 0,
            retirement_state: self.retirement_state,
            retirement_generation: self.retirement_generation.unwrap_or(0),
            retirement_obligations: self.retirement_obligations.label(),
            retirement_receipt: self
                .retirement_receipt
                .map_or_else(String::new, DirectLayerPresentationReceipt::label),
            retirement_receipt_sequence: self
                .retirement_receipt
                .map_or(0, |receipt| receipt.sequence),
            retirement_receipt_slot: self.retirement_receipt.map_or(0, |receipt| receipt.slot),
            retirement_receipt_route_epoch: self
                .retirement_receipt
                .map_or(0, |receipt| receipt.route_epoch),
            recovery_count: self.recovery_count,
            last_invariant_kind: self.last_invariant_kind.clone(),
            last_invariant_detail: self.last_invariant_detail.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiCompositionEvent {
    pub name: &'static str,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirectLayerObligations(u8);

impl DirectLayerObligations {
    const ARCADE: u8 = 1;
    const PREVIEW: u8 = 2;

    pub const fn new(arcade: bool, preview: bool) -> Self {
        Self((arcade as u8) * Self::ARCADE | (preview as u8) * Self::PREVIEW)
    }

    const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    const fn is_empty(self) -> bool {
        self.0 == 0
    }

    const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn label(self) -> &'static str {
        match self.0 {
            Self::ARCADE => "arcade",
            Self::PREVIEW => "preview",
            Self::ARCADE | Self::PREVIEW => "arcade+preview",
            _ => "none",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectLayerCarrier {
    Navigation,
    Modal,
    Screensaver,
    Recovery,
    LiveSlint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectLayerPresentationReceipt {
    pub sequence: u16,
    pub slot: u8,
    pub route_epoch: u16,
    pub carrier: DirectLayerCarrier,
}

impl DirectLayerPresentationReceipt {
    fn valid(self) -> bool {
        self.sequence != 0 && matches!(self.slot, 0..=2)
    }

    fn label(self) -> String {
        format!(
            "sequence={} slot={} route_epoch={} carrier={:?}",
            self.sequence, self.slot, self.route_epoch, self.carrier
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirectLayerRetirementState {
    Idle,
    Pending,
    Reconciling,
}

impl DirectLayerRetirementState {
    const fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Pending => "pending",
            Self::Reconciling => "reconciling",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DirectLayerRetirement {
    state: DirectLayerRetirementState,
    generation: u64,
    obligations: DirectLayerObligations,
    receipt: Option<DirectLayerPresentationReceipt>,
}

#[derive(Debug)]
pub struct UiCompositionController {
    state: UiCompositionState,
    recovery_count: u64,
    last_invariant_kind: String,
    last_invariant_detail: String,
    owned_direct_layers: DirectLayerObligations,
    retirement_generation: u64,
    retirement: Option<DirectLayerRetirement>,
    last_retirement_receipt: Option<DirectLayerPresentationReceipt>,
}

impl UiCompositionController {
    pub fn new() -> Self {
        Self {
            state: UiCompositionState::FullSlint,
            recovery_count: 0,
            last_invariant_kind: String::new(),
            last_invariant_detail: String::new(),
            owned_direct_layers: DirectLayerObligations::default(),
            retirement_generation: 0,
            retirement: None,
            last_retirement_receipt: None,
        }
    }

    pub fn tick(&mut self, input: UiCompositionInput) -> UiCompositionDecision {
        let requested_state = requested_state(input);
        let invariant = composition_invariant(input, requested_state);
        let previous = self.state;
        let mut events = Vec::new();
        let recovered_from = (previous == UiCompositionState::Recovering && invariant.is_none())
            .then_some(requested_state);

        let (state, force_full_slint_present, force_full_slint_raster, clear_direct_layers) =
            if let Some(invariant) = invariant {
                self.state = UiCompositionState::Recovering;
                self.recovery_count = self.recovery_count.saturating_add(1);
                self.last_invariant_kind = invariant.kind.to_string();
                self.last_invariant_detail = invariant.detail;
                events.push(UiCompositionEvent {
                    name: "ui_composition_invariant_failed",
                    detail: format!(
                        "from={} kind={} detail={}",
                        previous.label(),
                        self.last_invariant_kind,
                        self.last_invariant_detail
                    ),
                });
                (self.state, true, false, true)
            } else {
                self.state = requested_state;
                if let Some(to) = recovered_from {
                    events.push(UiCompositionEvent {
                        name: "ui_composition_recovered",
                        detail: format!("to={}", to.label()),
                    });
                }
                let full_frame = previous != requested_state
                    || requested_state == UiCompositionState::ModalOverArcade;
                let entering_screensaver = previous != UiCompositionState::Screensaver
                    && requested_state == UiCompositionState::Screensaver;
                let clear_layers = entering_screensaver
                    || (previous.allows_direct_layers()
                        && (!requested_state.allows_direct_layers()
                            || requested_state == UiCompositionState::ModalOverArcade));
                let force_full_raster =
                    requested_state == UiCompositionState::NavigationDestination;
                (
                    self.state,
                    full_frame || force_full_raster,
                    force_full_raster,
                    clear_layers,
                )
            };

        if previous != self.state {
            events.push(UiCompositionEvent {
                name: "ui_composition_state_changed",
                detail: format!("from={} to={}", previous.label(), self.state.label()),
            });
        }

        let direct_layers_desired = if state == UiCompositionState::MixedArcade {
            DirectLayerObligations::new(input.wants_arcade_list, input.wants_preview)
        } else {
            DirectLayerObligations::default()
        };
        let illegal_layers = self.owned_direct_layers.without(direct_layers_desired);
        if !illegal_layers.is_empty() {
            match self.retirement.as_mut() {
                Some(retirement) => {
                    retirement.obligations = retirement.obligations.union(illegal_layers);
                }
                None => {
                    self.retirement_generation = self.retirement_generation.wrapping_add(1).max(1);
                    self.retirement = Some(DirectLayerRetirement {
                        state: DirectLayerRetirementState::Pending,
                        generation: self.retirement_generation,
                        obligations: illegal_layers,
                        receipt: None,
                    });
                    events.push(UiCompositionEvent {
                        name: "direct_layer_retirement_started",
                        detail: format!(
                            "generation={} obligations={}",
                            self.retirement_generation,
                            illegal_layers.label()
                        ),
                    });
                }
            }
        }
        let retirement_generation = self.retirement.map(|retirement| retirement.generation);
        let retirement_carrier = retirement_carrier(state);
        let retirement_state = self
            .retirement
            .map_or(DirectLayerRetirementState::Idle, |retirement| {
                retirement.state
            })
            .label();
        let retirement_obligations = self
            .retirement
            .map_or_else(DirectLayerObligations::default, |retirement| {
                retirement.obligations
            });
        let retirement_receipt = self
            .retirement
            .and_then(|retirement| retirement.receipt)
            .or(self.last_retirement_receipt);

        UiCompositionDecision {
            state,
            allow_arcade_list_blit: state == UiCompositionState::MixedArcade,
            allow_preview_blit: state == UiCompositionState::MixedArcade
                && (!input.preview_cache_exact || input.preview_frame_ready),
            transition_owns_full_frame: matches!(
                state,
                UiCompositionState::NavigationTransition
                    | UiCompositionState::NavigationDestination
            ),
            prepare_navigation_destination: state == UiCompositionState::NavigationDestination,
            force_full_slint_raster,
            force_full_slint_present,
            clear_direct_layers: clear_direct_layers || !illegal_layers.is_empty(),
            direct_layers_desired,
            retirement_generation,
            retirement_carrier,
            retirement_state,
            retirement_obligations,
            retirement_receipt,
            recovery_count: self.recovery_count,
            last_invariant_kind: self.last_invariant_kind.clone(),
            last_invariant_detail: self.last_invariant_detail.clone(),
            events,
        }
    }

    pub fn confirm_presented_layers(
        &mut self,
        generation: Option<u64>,
        desired: DirectLayerObligations,
        receipt: DirectLayerPresentationReceipt,
    ) -> bool {
        if !receipt.valid() {
            return false;
        }
        match (generation, self.retirement) {
            (Some(generation), Some(retirement)) if generation == retirement.generation => {
                self.owned_direct_layers = desired;
                self.last_retirement_receipt = Some(receipt);
                self.retirement = None;
                true
            }
            (None, None) => {
                self.owned_direct_layers = desired;
                false
            }
            _ => false,
        }
    }

    pub fn mark_retirement_uncertain(&mut self, generation: u64) -> bool {
        let Some(retirement) = self.retirement.as_mut() else {
            return false;
        };
        if retirement.generation != generation {
            return false;
        }
        retirement.state = DirectLayerRetirementState::Reconciling;
        true
    }

    pub fn reconcile_retirement(
        &mut self,
        generation: u64,
        desired: DirectLayerObligations,
        active_receipt: DirectLayerPresentationReceipt,
    ) -> bool {
        let Some(retirement) = self.retirement else {
            return false;
        };
        if retirement.state != DirectLayerRetirementState::Reconciling {
            return false;
        }
        self.confirm_presented_layers(Some(generation), desired, active_receipt)
    }
}

fn retirement_carrier(state: UiCompositionState) -> DirectLayerCarrier {
    match state {
        UiCompositionState::NavigationTransition | UiCompositionState::NavigationDestination => {
            DirectLayerCarrier::Navigation
        }
        UiCompositionState::Screensaver => DirectLayerCarrier::Screensaver,
        UiCompositionState::ModalFullSlint | UiCompositionState::ModalOverArcade => {
            DirectLayerCarrier::Modal
        }
        UiCompositionState::Recovering => DirectLayerCarrier::Recovery,
        UiCompositionState::FullSlint | UiCompositionState::MixedArcade => {
            DirectLayerCarrier::LiveSlint
        }
    }
}

struct CompositionInvariant {
    kind: &'static str,
    detail: String,
}

fn requested_state(input: UiCompositionInput) -> UiCompositionState {
    if input.screensaver_active {
        UiCompositionState::Screensaver
    } else if input.fullscreen_overlay_visible {
        UiCompositionState::ModalFullSlint
    } else if input.confirm_visible {
        if input.return_screen == Some(Screen::Arcade) && input.arcade_ready {
            UiCompositionState::ModalOverArcade
        } else {
            UiCompositionState::ModalFullSlint
        }
    } else if input.navigation_transition_active {
        if input.navigation_destination_committed
            && !input.navigation_destination_ready
            && input.navigation_destination_layers_ready
        {
            UiCompositionState::NavigationDestination
        } else {
            UiCompositionState::NavigationTransition
        }
    } else if input.return_screen == Some(Screen::Arcade) && input.arcade_ready {
        UiCompositionState::MixedArcade
    } else {
        UiCompositionState::FullSlint
    }
}

fn composition_invariant(
    input: UiCompositionInput,
    requested_state: UiCompositionState,
) -> Option<CompositionInvariant> {
    if !input.route_ok {
        return Some(CompositionInvariant {
            kind: "route-invalid",
            detail: format!("requested_state={}", requested_state.label()),
        });
    }

    let direct_requested = !input.screensaver_active
        && !input.fullscreen_overlay_visible
        && !input.navigation_transition_active
        && (input.wants_arcade_list || input.wants_preview);
    if direct_requested && !requested_state.allows_direct_layers() {
        return Some(CompositionInvariant {
            kind: "direct-layer-outside-arcade",
            detail: format!(
                "requested_state={} wants_arcade_list={} wants_preview={}",
                requested_state.label(),
                input.wants_arcade_list,
                input.wants_preview
            ),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(sequence: u16, carrier: DirectLayerCarrier) -> DirectLayerPresentationReceipt {
        DirectLayerPresentationReceipt {
            sequence,
            slot: if sequence % 2 == 0 { 2 } else { 1 },
            route_epoch: sequence,
            carrier,
        }
    }

    fn acquire_direct_layers(controller: &mut UiCompositionController) {
        let decision = controller.tick(UiCompositionInput {
            wants_arcade_list: true,
            wants_preview: true,
            preview_cache_exact: true,
            preview_frame_ready: true,
            ..input(Screen::Arcade)
        });
        assert!(!controller.confirm_presented_layers(
            None,
            decision.direct_layers_desired,
            receipt(1, DirectLayerCarrier::LiveSlint),
        ));
    }

    fn input(screen: Screen) -> UiCompositionInput {
        UiCompositionInput {
            screensaver_active: false,
            navigation_transition_active: false,
            navigation_destination_committed: false,
            navigation_destination_ready: false,
            navigation_destination_layers_ready: false,
            return_screen: Some(screen),
            confirm_visible: false,
            fullscreen_overlay_visible: false,
            arcade_ready: screen == Screen::Arcade,
            route_ok: true,
            wants_arcade_list: false,
            wants_preview: false,
            preview_cache_exact: false,
            preview_frame_ready: false,
        }
    }

    #[test]
    fn arcade_screen_allows_direct_layers() {
        let mut controller = UiCompositionController::new();
        let decision = controller.tick(UiCompositionInput {
            wants_arcade_list: true,
            wants_preview: true,
            preview_cache_exact: true,
            preview_frame_ready: true,
            ..input(Screen::Arcade)
        });

        assert_eq!(decision.state, UiCompositionState::MixedArcade);
        assert!(decision.allow_arcade_list_blit);
        assert!(decision.allow_preview_blit);
        assert_eq!(decision.recovery_count, 0);
    }

    #[test]
    fn steady_full_slint_does_not_force_full_present() {
        let mut controller = UiCompositionController::new();
        let decision = controller.tick(input(Screen::Home));

        assert_eq!(decision.state, UiCompositionState::FullSlint);
        assert!(!decision.force_full_slint_present);
        assert!(!decision.clear_direct_layers);
    }

    #[test]
    fn transition_from_arcade_to_full_slint_forces_full_present() {
        let mut controller = UiCompositionController::new();
        let _ = controller.tick(input(Screen::Arcade));
        let decision = controller.tick(input(Screen::Home));

        assert_eq!(decision.state, UiCompositionState::FullSlint);
        assert!(decision.force_full_slint_present);
        assert!(decision.clear_direct_layers);
    }

    #[test]
    fn navigation_transition_owns_full_frame_and_suppresses_direct_layers() {
        let mut controller = UiCompositionController::new();
        let _ = controller.tick(UiCompositionInput {
            wants_arcade_list: true,
            wants_preview: true,
            ..input(Screen::Arcade)
        });
        let decision = controller.tick(UiCompositionInput {
            navigation_transition_active: true,
            wants_arcade_list: true,
            wants_preview: true,
            ..input(Screen::Arcade)
        });

        assert_eq!(decision.state, UiCompositionState::NavigationTransition);
        assert!(decision.transition_owns_full_frame);
        assert!(!decision.allow_arcade_list_blit);
        assert!(!decision.allow_preview_blit);
        assert!(decision.force_full_slint_present);
        assert!(decision.clear_direct_layers);
        assert_eq!(decision.recovery_count, 0);
    }

    #[test]
    fn navigation_destination_preparation_is_an_explicit_state() {
        let mut controller = UiCompositionController::new();

        let source = controller.tick(UiCompositionInput {
            navigation_transition_active: true,
            ..input(Screen::Home)
        });
        assert_eq!(source.state, UiCompositionState::NavigationTransition);
        assert!(!source.prepare_navigation_destination);
        assert!(!source.force_full_slint_raster);

        let waiting_for_layers = controller.tick(UiCompositionInput {
            navigation_transition_active: true,
            navigation_destination_committed: true,
            ..input(Screen::Arcade)
        });
        assert_eq!(
            waiting_for_layers.state,
            UiCompositionState::NavigationTransition
        );

        let preparing = controller.tick(UiCompositionInput {
            navigation_transition_active: true,
            navigation_destination_committed: true,
            navigation_destination_layers_ready: true,
            ..input(Screen::Arcade)
        });
        assert_eq!(preparing.state, UiCompositionState::NavigationDestination);
        assert!(preparing.transition_owns_full_frame);
        assert!(preparing.prepare_navigation_destination);
        assert!(preparing.force_full_slint_raster);
        assert!(preparing.force_full_slint_present);

        let captured = controller.tick(UiCompositionInput {
            navigation_transition_active: true,
            navigation_destination_committed: true,
            navigation_destination_ready: true,
            navigation_destination_layers_ready: true,
            ..input(Screen::Arcade)
        });
        assert_eq!(captured.state, UiCompositionState::NavigationTransition);
        assert!(!captured.prepare_navigation_destination);
        assert!(!captured.force_full_slint_raster);

        let settled = controller.tick(input(Screen::Arcade));
        assert_eq!(settled.state, UiCompositionState::MixedArcade);
    }

    #[test]
    fn confirmation_modal_preempts_navigation_transition() {
        let mut controller = UiCompositionController::new();
        let decision = controller.tick(UiCompositionInput {
            navigation_transition_active: true,
            confirm_visible: true,
            ..input(Screen::Home)
        });

        assert_eq!(decision.state, UiCompositionState::ModalFullSlint);
        assert!(!decision.transition_owns_full_frame);
        assert!(decision.force_full_slint_present);
    }

    #[test]
    fn screensaver_takes_exclusive_composition_from_arcade() {
        let mut controller = UiCompositionController::new();
        let _ = controller.tick(UiCompositionInput {
            wants_arcade_list: true,
            wants_preview: true,
            preview_cache_exact: true,
            preview_frame_ready: true,
            ..input(Screen::Arcade)
        });

        let decision = controller.tick(UiCompositionInput {
            screensaver_active: true,
            return_screen: None,
            wants_arcade_list: true,
            wants_preview: true,
            ..input(Screen::Arcade)
        });

        assert_eq!(decision.state, UiCompositionState::Screensaver);
        assert!(!decision.allow_arcade_list_blit);
        assert!(!decision.allow_preview_blit);
        assert!(decision.force_full_slint_present);
        assert!(decision.clear_direct_layers);

        let steady = controller.tick(UiCompositionInput {
            screensaver_active: true,
            return_screen: None,
            wants_arcade_list: true,
            wants_preview: true,
            ..input(Screen::Arcade)
        });
        assert_eq!(steady.state, UiCompositionState::Screensaver);
        assert!(!steady.allow_arcade_list_blit);
        assert!(!steady.allow_preview_blit);
        assert!(!steady.force_full_slint_present);
        assert!(!steady.clear_direct_layers);
    }

    #[test]
    fn screensaver_entry_clears_layers_from_full_slint_defensively() {
        let mut controller = UiCompositionController::new();

        let decision = controller.tick(UiCompositionInput {
            screensaver_active: true,
            return_screen: None,
            ..input(Screen::Home)
        });

        assert_eq!(decision.state, UiCompositionState::Screensaver);
        assert!(decision.force_full_slint_present);
        assert!(decision.clear_direct_layers);
    }

    #[test]
    fn full_slint_modal_transition_forces_full_present_once() {
        let mut controller = UiCompositionController::new();
        let decision = controller.tick(UiCompositionInput {
            confirm_visible: true,
            ..input(Screen::Home)
        });
        let steady = controller.tick(UiCompositionInput {
            confirm_visible: true,
            ..input(Screen::Home)
        });

        assert_eq!(decision.state, UiCompositionState::ModalFullSlint);
        assert!(decision.force_full_slint_present);
        assert_eq!(steady.state, UiCompositionState::ModalFullSlint);
        assert!(!steady.force_full_slint_present);
    }

    #[test]
    fn full_slint_screen_rejects_direct_layers() {
        let mut controller = UiCompositionController::new();
        let decision = controller.tick(UiCompositionInput {
            wants_arcade_list: true,
            ..input(Screen::Home)
        });

        assert_eq!(decision.state, UiCompositionState::Recovering);
        assert!(decision.clear_direct_layers);
        assert!(decision.force_full_slint_present);
        assert_eq!(decision.recovery_count, 1);
        assert_eq!(decision.last_invariant_kind, "direct-layer-outside-arcade");
    }

    #[test]
    fn non_arcade_modal_rejects_direct_layers() {
        let mut controller = UiCompositionController::new();
        let decision = controller.tick(UiCompositionInput {
            confirm_visible: true,
            wants_preview: true,
            ..input(Screen::Settings)
        });

        assert_eq!(decision.state, UiCompositionState::Recovering);
        assert_eq!(decision.last_invariant_kind, "direct-layer-outside-arcade");
    }

    #[test]
    fn fullscreen_overlay_transitions_from_arcade_to_full_slint() {
        let mut controller = UiCompositionController::new();
        let _ = controller.tick(UiCompositionInput {
            wants_arcade_list: true,
            preview_cache_exact: true,
            preview_frame_ready: true,
            ..input(Screen::Arcade)
        });

        let decision = controller.tick(UiCompositionInput {
            fullscreen_overlay_visible: true,
            wants_arcade_list: true,
            wants_preview: true,
            preview_cache_exact: true,
            preview_frame_ready: true,
            ..input(Screen::Arcade)
        });

        assert_eq!(decision.state, UiCompositionState::ModalFullSlint);
        assert!(!decision.allow_arcade_list_blit);
        assert!(!decision.allow_preview_blit);
        assert!(decision.clear_direct_layers);
        assert!(decision.force_full_slint_present);
        assert_eq!(decision.recovery_count, 0);
        assert_eq!(decision.last_invariant_kind, "");

        let steady = controller.tick(UiCompositionInput {
            fullscreen_overlay_visible: true,
            wants_arcade_list: true,
            wants_preview: true,
            preview_cache_exact: true,
            preview_frame_ready: true,
            ..input(Screen::Arcade)
        });

        assert_eq!(steady.state, UiCompositionState::ModalFullSlint);
        assert!(!steady.allow_arcade_list_blit);
        assert!(!steady.allow_preview_blit);
        assert!(!steady.force_full_slint_present);
        assert!(!steady.clear_direct_layers);
        assert_eq!(steady.recovery_count, 0);
    }

    #[test]
    fn modal_over_arcade_clears_direct_layers_deterministically() {
        let mut controller = UiCompositionController::new();
        let _ = controller.tick(input(Screen::Arcade));
        let decision = controller.tick(UiCompositionInput {
            confirm_visible: true,
            wants_arcade_list: true,
            preview_cache_exact: true,
            preview_frame_ready: true,
            ..input(Screen::Arcade)
        });

        assert_eq!(decision.state, UiCompositionState::ModalOverArcade);
        assert!(!decision.allow_arcade_list_blit);
        assert!(!decision.allow_preview_blit);
        assert!(decision.clear_direct_layers);
        assert!(decision.force_full_slint_present);
        assert_eq!(decision.recovery_count, 0);
    }

    #[test]
    fn exact_preview_without_drawable_frame_keeps_arcade_list_enabled() {
        let mut controller = UiCompositionController::new();
        let decision = controller.tick(UiCompositionInput {
            wants_arcade_list: true,
            wants_preview: true,
            preview_cache_exact: true,
            preview_frame_ready: false,
            ..input(Screen::Arcade)
        });

        assert_eq!(decision.state, UiCompositionState::MixedArcade);
        assert!(decision.allow_arcade_list_blit);
        assert!(!decision.allow_preview_blit);
        assert_eq!(decision.recovery_count, 0);
        assert_eq!(decision.last_invariant_kind, "");
    }

    #[test]
    fn recovery_reports_when_state_becomes_valid_again() {
        let mut controller = UiCompositionController::new();
        let _ = controller.tick(UiCompositionInput {
            wants_arcade_list: true,
            ..input(Screen::Home)
        });
        let decision = controller.tick(input(Screen::Home));

        assert_eq!(decision.state, UiCompositionState::FullSlint);
        assert!(
            decision
                .events
                .iter()
                .any(|event| event.name == "ui_composition_recovered")
        );
    }

    #[test]
    fn every_authorized_complete_frame_can_carry_retirement() {
        let cases = [
            (
                UiCompositionInput {
                    navigation_transition_active: true,
                    ..input(Screen::Home)
                },
                DirectLayerCarrier::Navigation,
            ),
            (
                UiCompositionInput {
                    fullscreen_overlay_visible: true,
                    ..input(Screen::Home)
                },
                DirectLayerCarrier::Modal,
            ),
            (
                UiCompositionInput {
                    screensaver_active: true,
                    return_screen: None,
                    ..input(Screen::Home)
                },
                DirectLayerCarrier::Screensaver,
            ),
            (
                UiCompositionInput {
                    route_ok: false,
                    ..input(Screen::Home)
                },
                DirectLayerCarrier::Recovery,
            ),
            (input(Screen::Home), DirectLayerCarrier::LiveSlint),
        ];

        for (retirement_input, expected_carrier) in cases {
            let mut controller = UiCompositionController::new();
            acquire_direct_layers(&mut controller);
            let decision = controller.tick(retirement_input);
            let generation = decision
                .retirement_generation
                .expect("retirement generation");

            assert_eq!(decision.retirement_carrier, expected_carrier);
            assert_eq!(decision.retirement_obligations.label(), "arcade+preview");
            assert!(controller.confirm_presented_layers(
                Some(generation),
                decision.direct_layers_desired,
                receipt(2, expected_carrier),
            ));
            assert!(controller.retirement.is_none());
            assert!(controller.owned_direct_layers.is_empty());
        }
    }

    #[test]
    fn uncertain_retirement_reconciles_before_any_retry() {
        let mut controller = UiCompositionController::new();
        acquire_direct_layers(&mut controller);
        let decision = controller.tick(input(Screen::Home));
        let generation = decision.retirement_generation.expect("retirement");

        assert!(!controller.confirm_presented_layers(
            Some(generation.wrapping_add(1)),
            decision.direct_layers_desired,
            receipt(2, decision.retirement_carrier),
        ));
        assert!(controller.mark_retirement_uncertain(generation));
        assert_eq!(
            controller.retirement.expect("transaction").state,
            DirectLayerRetirementState::Reconciling
        );
        assert!(controller.reconcile_retirement(
            generation,
            decision.direct_layers_desired,
            receipt(2, decision.retirement_carrier),
        ));
        assert!(controller.retirement.is_none());
    }

    #[test]
    fn navigation_snapshot_lock_uses_navigation_carrier_without_live_raster() {
        let mut controller = UiCompositionController::new();
        acquire_direct_layers(&mut controller);
        let decision = controller.tick(UiCompositionInput {
            navigation_transition_active: true,
            ..input(Screen::Home)
        });

        assert_eq!(decision.retirement_carrier, DirectLayerCarrier::Navigation);
        assert!(decision.transition_owns_full_frame);
        assert!(!decision.force_full_slint_raster);
        assert!(decision.retirement_generation.is_some());
    }

    #[test]
    fn same_frame_retire_and_reacquire_keeps_replacement_layers() {
        let mut controller = UiCompositionController::new();
        acquire_direct_layers(&mut controller);
        let retirement = controller.tick(input(Screen::Home));
        let generation = retirement.retirement_generation.expect("retirement");

        let replacement = controller.tick(UiCompositionInput {
            wants_arcade_list: true,
            wants_preview: true,
            preview_cache_exact: true,
            preview_frame_ready: true,
            ..input(Screen::Arcade)
        });

        assert_eq!(replacement.retirement_generation, Some(generation));
        assert!(controller.confirm_presented_layers(
            Some(generation),
            replacement.direct_layers_desired,
            receipt(2, replacement.retirement_carrier),
        ));
        assert_eq!(
            controller.owned_direct_layers,
            DirectLayerObligations::new(true, true)
        );
        assert!(controller.retirement.is_none());
    }
}
