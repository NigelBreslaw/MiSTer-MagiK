// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Seeded random walks over the composition controller, with a model of the
//! physical presenter acknowledging each frame. The ownership contract from
//! `docs/architecture.md` ("Launcher Composition") is checked on every tick, not
//! only on the hand-picked sequences in the unit tests:
//!
//! * the state follows a fixed precedence (screensaver, full-screen overlay,
//!   confirmation, navigation, Arcade, Slint);
//! * direct Arcade layers are legal only in `MixedArcade`, and a layer the
//!   presenter still owns when its state ends is always retired, never left;
//! * a retirement is keyed by a generation, so a stale acknowledgement is refused;
//! * every state change repaints the complete frame, and entering the
//!   navigation destination forces a full raster;
//! * an invalid route enters `Recovering`, and the next valid tick leaves it.

use super::*;

const WALKS: usize = 300;
const STEPS: usize = 80;

use super::super::walk_rng::WalkRng as Rng;

/// The documented precedence, written independently of `requested_state`.
fn expected_state(input: &UiCompositionInput) -> UiCompositionState {
    use UiCompositionState::*;
    let arcade = input.return_screen == Some(Screen::Arcade) && input.arcade_ready;
    if input.screensaver_active {
        Screensaver
    } else if input.fullscreen_overlay_visible {
        ModalFullSlint
    } else if input.confirm_visible {
        if arcade {
            ModalOverArcade
        } else {
            ModalFullSlint
        }
    } else if input.navigation_transition_active {
        if input.navigation_destination_committed
            && !input.navigation_destination_ready
            && input.navigation_destination_layers_ready
        {
            NavigationDestination
        } else {
            NavigationTransition
        }
    } else if arcade {
        MixedArcade
    } else {
        FullSlint
    }
}

fn random_input(rng: &mut Rng) -> UiCompositionInput {
    let screen = match rng.below(4) {
        0 => Screen::Home,
        1 => Screen::Settings,
        _ => Screen::Arcade,
    };
    let mut input = UiCompositionInput {
        screensaver_active: rng.chance(8),
        navigation_transition_active: rng.chance(20),
        navigation_destination_committed: rng.chance(50),
        navigation_destination_ready: rng.chance(30),
        navigation_destination_layers_ready: rng.chance(60),
        return_screen: (!rng.chance(5)).then_some(screen),
        confirm_visible: rng.chance(10),
        fullscreen_overlay_visible: rng.chance(5),
        arcade_ready: rng.chance(85),
        route_ok: true,
        wants_arcade_list: false,
        wants_preview: false,
        preview_cache_exact: rng.chance(50),
        preview_frame_ready: rng.chance(50),
    };
    // Direct layers are only ever requested where the screen can show them;
    // asking for them elsewhere is the `direct-layer-outside-arcade` invariant.
    if expected_state(&input) == UiCompositionState::MixedArcade {
        input.wants_arcade_list = rng.chance(80);
        input.wants_preview = rng.chance(60);
    }
    input
}

#[derive(Clone, Copy, Default)]
struct Owned {
    arcade: bool,
    preview: bool,
}

impl Owned {
    fn from(obligations: DirectLayerObligations) -> Self {
        Self {
            arcade: obligations.contains_arcade(),
            preview: obligations.0 & DirectLayerObligations::PREVIEW != 0,
        }
    }

    fn exceeds(self, desired: Self) -> bool {
        (self.arcade && !desired.arcade) || (self.preview && !desired.preview)
    }
}

#[test]
fn random_walks_hold_the_composition_ownership_contract() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut recoveries = 0;
    let mut retirements = 0;
    let mut visited = std::collections::BTreeSet::new();
    for walk in 0..WALKS {
        let mut controller = UiCompositionController::new();
        let mut previous = UiCompositionState::FullSlint;
        let mut owned = Owned::default();
        let mut sequence = 0u16;
        let mut expected_recoveries = 0u64;
        for step in 0..STEPS {
            let mut input = random_input(&mut rng);
            let route_ok = !rng.chance(3);
            input.route_ok = route_ok;
            let at = format!("walk {walk} step {step}");
            let decision = controller.tick(input);
            visited.insert(decision.state.label());

            // The state follows the documented precedence, or recovers.
            if route_ok {
                assert_eq!(decision.state, expected_state(&input), "{at}");
            } else {
                assert_eq!(decision.state, UiCompositionState::Recovering, "{at}");
                expected_recoveries += 1;
                recoveries += 1;
                assert!(decision.clear_direct_layers, "{at}: recovery clears layers");
                assert!(decision.force_full_slint_present, "{at}: recovery repaints");
            }
            assert_eq!(decision.recovery_count, expected_recoveries, "{at}");
            if previous == UiCompositionState::Recovering && route_ok {
                assert!(
                    decision
                        .events
                        .iter()
                        .any(|event| event.name == "ui_composition_recovered"),
                    "{at}: leaving recovery is reported"
                );
            }

            // What each state lets the renderer do.
            let mixed = decision.state == UiCompositionState::MixedArcade;
            assert_eq!(decision.allow_arcade_list_blit, mixed, "{at}");
            assert!(!decision.allow_preview_blit || mixed, "{at}");
            assert_eq!(
                decision.transition_owns_full_frame,
                matches!(
                    decision.state,
                    UiCompositionState::NavigationTransition
                        | UiCompositionState::NavigationDestination
                ),
                "{at}"
            );
            assert_eq!(
                decision.force_full_slint_raster,
                decision.state == UiCompositionState::NavigationDestination,
                "{at}"
            );
            if !mixed {
                assert_eq!(
                    decision.direct_layers_desired,
                    DirectLayerObligations::default(),
                    "{at}: no direct layers outside MixedArcade"
                );
            }
            if decision.state != previous {
                assert!(
                    decision.force_full_slint_present,
                    "{at}: state change repaints"
                );
            }
            if decision.state == UiCompositionState::Screensaver && previous != decision.state {
                assert!(
                    decision.clear_direct_layers,
                    "{at}: screensaver owns the frame"
                );
            }

            // A layer the presenter still owns that is no longer desired must be
            // retired under a generation.
            let desired = Owned::from(decision.direct_layers_desired);
            if owned.exceeds(desired) {
                retirements += 1;
                assert!(decision.clear_direct_layers, "{at}: stale layer cleared");
                assert!(
                    decision.retirement_generation.is_some(),
                    "{at}: retirement keyed"
                );
            }
            if decision.retirement_generation.is_some() {
                assert!(
                    decision.clear_direct_layers,
                    "{at}: pending retirement clears"
                );
            }
            // The frame that carries a retirement is the one the state owns.
            assert_eq!(
                decision.retirement_carrier,
                match decision.state {
                    UiCompositionState::NavigationTransition
                    | UiCompositionState::NavigationDestination => DirectLayerCarrier::Navigation,
                    UiCompositionState::ModalFullSlint | UiCompositionState::ModalOverArcade => {
                        DirectLayerCarrier::Modal
                    }
                    UiCompositionState::Screensaver => DirectLayerCarrier::Screensaver,
                    UiCompositionState::Recovering => DirectLayerCarrier::Recovery,
                    UiCompositionState::FullSlint | UiCompositionState::MixedArcade => {
                        DirectLayerCarrier::LiveSlint
                    }
                },
                "{at}"
            );

            // The presenter acknowledges the frame. A stale generation is refused;
            // the matching one retires the layers and transfers ownership.
            sequence += 1;
            let receipt = DirectLayerPresentationReceipt {
                sequence,
                slot: (sequence % 3) as u8,
                route_epoch: 1,
                carrier: decision.retirement_carrier,
            };
            if let Some(generation) = decision.retirement_generation {
                assert!(
                    !controller.confirm_presented_layers(
                        Some(generation.wrapping_add(1)),
                        decision.direct_layers_desired,
                        receipt
                    ),
                    "{at}: stale generation refused"
                );
                assert!(
                    controller.confirm_presented_layers(
                        Some(generation),
                        decision.direct_layers_desired,
                        receipt
                    ),
                    "{at}: matching generation accepted"
                );
            } else {
                assert!(
                    !controller.confirm_presented_layers(
                        None,
                        decision.direct_layers_desired,
                        receipt
                    ),
                    "{at}: nothing was retiring"
                );
            }
            owned = desired;
            previous = decision.state;
        }
    }
    // The walk must actually reach every state, or it proves little.
    for state in [
        "full-slint",
        "mixed-arcade",
        "navigation-transition",
        "navigation-destination",
        "screensaver",
        "modal-full-slint",
        "modal-over-arcade",
        "recovering",
    ] {
        assert!(
            visited.contains(state),
            "never reached {state}: {visited:?}"
        );
    }
    assert!(
        recoveries > 100 && retirements > 100,
        "{recoveries} {retirements}"
    );
}

#[test]
fn an_uncertain_retirement_reconciles_without_losing_the_layers_it_retires() {
    let mut controller = UiCompositionController::new();
    let arcade = |controller: &mut UiCompositionController| {
        controller.tick(UiCompositionInput {
            return_screen: Some(Screen::Arcade),
            arcade_ready: true,
            wants_arcade_list: true,
            wants_preview: true,
            preview_cache_exact: true,
            preview_frame_ready: true,
            ..quiet_input()
        })
    };
    let live = arcade(&mut controller);
    let receipt = |sequence, carrier| DirectLayerPresentationReceipt {
        sequence,
        slot: 1,
        route_epoch: 1,
        carrier,
    };
    assert!(!controller.confirm_presented_layers(
        None,
        live.direct_layers_desired,
        receipt(1, DirectLayerCarrier::LiveSlint)
    ));

    // The screensaver takes the frame; its retirement outcome is uncertain.
    let saver = controller.tick(UiCompositionInput {
        screensaver_active: true,
        ..quiet_input()
    });
    let generation = saver.retirement_generation.expect("layers must retire");
    assert!(controller.mark_retirement_uncertain(generation));
    assert!(
        !controller.confirm_presented_layers(
            Some(generation + 1),
            DirectLayerObligations::default(),
            receipt(2, DirectLayerCarrier::Screensaver)
        ),
        "a stale generation cannot settle an uncertain retirement"
    );
    assert!(controller.reconcile_retirement(
        generation,
        DirectLayerObligations::default(),
        receipt(3, DirectLayerCarrier::Screensaver)
    ));

    // Returning to Arcade acquires fresh layers under a new generation.
    let back = arcade(&mut controller);
    assert_eq!(back.state, UiCompositionState::MixedArcade);
    assert!(back.force_full_slint_present);
    assert_eq!(back.retirement_generation, None);
}

#[test]
fn requesting_direct_layers_outside_arcade_is_an_invariant_failure_that_recovers() {
    for screen in [Screen::Home, Screen::Settings] {
        let mut controller = UiCompositionController::new();
        let bad = controller.tick(UiCompositionInput {
            return_screen: Some(screen),
            wants_arcade_list: true,
            wants_preview: true,
            ..quiet_input()
        });
        assert_eq!(bad.state, UiCompositionState::Recovering, "{screen:?}");
        assert_eq!(bad.last_invariant_kind, "direct-layer-outside-arcade");
        assert!(bad.clear_direct_layers && bad.force_full_slint_present);
        assert!(!bad.allow_arcade_list_blit && !bad.allow_preview_blit);

        let good = controller.tick(UiCompositionInput {
            return_screen: Some(screen),
            ..quiet_input()
        });
        assert_eq!(good.state, UiCompositionState::FullSlint, "{screen:?}");
        assert_eq!(good.recovery_count, 1);
    }
}

fn quiet_input() -> UiCompositionInput {
    UiCompositionInput {
        screensaver_active: false,
        navigation_transition_active: false,
        navigation_destination_committed: false,
        navigation_destination_ready: false,
        navigation_destination_layers_ready: false,
        return_screen: Some(Screen::Home),
        confirm_visible: false,
        fullscreen_overlay_visible: false,
        arcade_ready: false,
        route_ok: true,
        wants_arcade_list: false,
        wants_preview: false,
        preview_cache_exact: false,
        preview_frame_ready: false,
    }
}
