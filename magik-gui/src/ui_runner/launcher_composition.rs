use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UiCompositionState {
    FullSlint,
    MixedArcade,
    ModalFullSlint,
    ModalOverArcade,
    Recovering,
}

impl UiCompositionState {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::FullSlint => "full-slint",
            Self::MixedArcade => "mixed-arcade",
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
pub(super) struct UiCompositionStatus {
    pub(super) state: &'static str,
    pub(super) recovery_count: u64,
    pub(super) last_invariant_kind: String,
    pub(super) last_invariant_detail: String,
}

impl Default for UiCompositionStatus {
    fn default() -> Self {
        Self {
            state: UiCompositionState::FullSlint.label(),
            recovery_count: 0,
            last_invariant_kind: String::new(),
            last_invariant_detail: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct UiCompositionInput {
    pub(super) screen: Screen,
    pub(super) confirm_visible: bool,
    pub(super) fullscreen_overlay_visible: bool,
    pub(super) arcade_ready: bool,
    pub(super) route_ok: bool,
    pub(super) wants_arcade_list: bool,
    pub(super) wants_preview: bool,
    pub(super) preview_cache_state: &'static str,
    pub(super) preview_frame_status: PreviewRawFrameStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UiCompositionDecision {
    pub(super) state: UiCompositionState,
    pub(super) allow_arcade_list_blit: bool,
    pub(super) allow_preview_blit: bool,
    pub(super) force_full_slint_present: bool,
    pub(super) clear_direct_layers: bool,
    pub(super) recovery_count: u64,
    pub(super) last_invariant_kind: String,
    pub(super) last_invariant_detail: String,
    pub(super) events: Vec<UiCompositionEvent>,
}

impl UiCompositionDecision {
    pub(super) fn status(&self) -> UiCompositionStatus {
        UiCompositionStatus {
            state: self.state.label(),
            recovery_count: self.recovery_count,
            last_invariant_kind: self.last_invariant_kind.clone(),
            last_invariant_detail: self.last_invariant_detail.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UiCompositionEvent {
    pub(super) name: &'static str,
    pub(super) detail: String,
}

#[derive(Debug)]
pub(super) struct UiCompositionController {
    state: UiCompositionState,
    recovery_count: u64,
    last_invariant_kind: String,
    last_invariant_detail: String,
}

impl UiCompositionController {
    pub(super) fn new() -> Self {
        Self {
            state: UiCompositionState::FullSlint,
            recovery_count: 0,
            last_invariant_kind: String::new(),
            last_invariant_detail: String::new(),
        }
    }

    pub(super) fn tick(&mut self, input: UiCompositionInput) -> UiCompositionDecision {
        let requested_state = requested_state(input);
        let invariant = composition_invariant(input, requested_state);
        let previous = self.state;
        let mut events = Vec::new();
        let recovered_from = (previous == UiCompositionState::Recovering && invariant.is_none())
            .then_some(requested_state);

        let (state, force_full_slint_present, clear_direct_layers) =
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
                (self.state, true, true)
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
                let clear_layers = previous.allows_direct_layers()
                    && (!requested_state.allows_direct_layers()
                        || requested_state == UiCompositionState::ModalOverArcade);
                (self.state, full_frame, clear_layers)
            };

        if previous != self.state {
            events.push(UiCompositionEvent {
                name: "ui_composition_state_changed",
                detail: format!("from={} to={}", previous.label(), self.state.label()),
            });
        }

        UiCompositionDecision {
            state,
            allow_arcade_list_blit: state == UiCompositionState::MixedArcade,
            allow_preview_blit: state == UiCompositionState::MixedArcade,
            force_full_slint_present,
            clear_direct_layers,
            recovery_count: self.recovery_count,
            last_invariant_kind: self.last_invariant_kind.clone(),
            last_invariant_detail: self.last_invariant_detail.clone(),
            events,
        }
    }
}

struct CompositionInvariant {
    kind: &'static str,
    detail: String,
}

fn requested_state(input: UiCompositionInput) -> UiCompositionState {
    if input.fullscreen_overlay_visible {
        UiCompositionState::ModalFullSlint
    } else if input.confirm_visible {
        if input.screen == Screen::Arcade && input.arcade_ready {
            UiCompositionState::ModalOverArcade
        } else {
            UiCompositionState::ModalFullSlint
        }
    } else if input.screen == Screen::Arcade && input.arcade_ready {
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

    let direct_requested =
        !input.fullscreen_overlay_visible && (input.wants_arcade_list || input.wants_preview);
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

    if input.preview_cache_state == "exact"
        && input.preview_frame_status != PreviewRawFrameStatus::Ready
    {
        return Some(CompositionInvariant {
            kind: "exact-preview-not-drawable",
            detail: format!("preview_frame_status={:?}", input.preview_frame_status),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(screen: Screen) -> UiCompositionInput {
        UiCompositionInput {
            screen,
            confirm_visible: false,
            fullscreen_overlay_visible: false,
            arcade_ready: screen == Screen::Arcade,
            route_ok: true,
            wants_arcade_list: false,
            wants_preview: false,
            preview_cache_state: "empty",
            preview_frame_status: PreviewRawFrameStatus::Empty,
        }
    }

    #[test]
    fn arcade_screen_allows_direct_layers() {
        let mut controller = UiCompositionController::new();
        let decision = controller.tick(UiCompositionInput {
            wants_arcade_list: true,
            wants_preview: true,
            preview_cache_state: "exact",
            preview_frame_status: PreviewRawFrameStatus::Ready,
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
            preview_cache_state: "exact",
            preview_frame_status: PreviewRawFrameStatus::Ready,
            ..input(Screen::Arcade)
        });

        let decision = controller.tick(UiCompositionInput {
            fullscreen_overlay_visible: true,
            wants_arcade_list: true,
            wants_preview: true,
            preview_cache_state: "exact",
            preview_frame_status: PreviewRawFrameStatus::Ready,
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
            preview_cache_state: "exact",
            preview_frame_status: PreviewRawFrameStatus::Ready,
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
            preview_cache_state: "exact",
            preview_frame_status: PreviewRawFrameStatus::Ready,
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
    fn exact_preview_must_have_drawable_frame() {
        let mut controller = UiCompositionController::new();
        let decision = controller.tick(UiCompositionInput {
            wants_preview: true,
            preview_cache_state: "exact",
            preview_frame_status: PreviewRawFrameStatus::Empty,
            ..input(Screen::Arcade)
        });

        assert_eq!(decision.state, UiCompositionState::Recovering);
        assert_eq!(decision.last_invariant_kind, "exact-preview-not-drawable");
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
        assert!(decision
            .events
            .iter()
            .any(|event| event.name == "ui_composition_recovered"));
    }
}
