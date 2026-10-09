// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structural checks for the current launcher frame pipeline.
//!
//! These checks preserve source boundary ordering during cleanup. They do not
//! execute the production frame pipeline or establish runtime behavior.
//! The `record_launcher_frame_phase!` markers feed the phase profile; they are also
//! text these checks read. `FrameLoop::frame` calls its phase methods in order, and
//! the methods are declared in that order, so marker order in the source is frame order.

#[cfg(test)]
mod tests {
    const FRAME_LOOP: &str = include_str!("launcher_loop/frame_loop.rs");
    const HELPERS: &str = include_str!("launcher_loop.rs");

    fn compact(text: &str) -> String {
        text.split_whitespace().collect()
    }

    /// The text of the function or method `name`, whitespace removed.
    fn function(name: &str) -> String {
        for (source, opens, close) in [
            (
                FRAME_LOOP,
                ["\n    fn ", "\n    pub(super) fn "],
                "\n    }\n",
            ),
            (HELPERS, ["\nfn ", "\npub(super) fn "], "\n}\n"),
        ] {
            for open in opens {
                if let Some(start) = source.find(&format!("{open}{name}")) {
                    let end = source[start + 1..]
                        .find(close)
                        .map(|offset| start + 1 + offset)
                        .unwrap_or_else(|| panic!("unterminated function {name}"));
                    return compact(&source[start..end]);
                }
            }
        }
        panic!("missing function {name}");
    }

    fn marker(phase: &str) -> String {
        format!("record_launcher_frame_phase!(LauncherFramePhase::{phase})")
    }

    /// Asserts that every needle occurs in `text` after the previous one.
    fn assert_ordered(text: &str, needles: &[String], context: &str) {
        let mut previous = 0;
        for needle in needles {
            let offset = text[previous..]
                .find(needle.as_str())
                .unwrap_or_else(|| panic!("{context}: missing or misordered {needle}"));
            previous += offset + needle.len();
        }
    }

    #[test]
    fn production_hooks_keep_the_core_boundaries_ordered() {
        assert_ordered(
            &function("frame"),
            &[
                "self.begin(".to_owned(),
                "self.pre_input(".to_owned(),
                "self.input(".to_owned(),
                "self.project(".to_owned(),
                "self.render(".to_owned(),
                "self.present(".to_owned(),
            ],
            "FrameLoop::frame",
        );
        assert_ordered(
            &compact(FRAME_LOOP),
            &[
                marker("Begin"),
                marker("PreInputMaintenance"),
                marker("InputCaptured"),
                marker("InputRouted"),
                marker("FramePlanned"),
                marker("FrameSubmitted"),
            ],
            "frame_loop.rs",
        );
        assert_ordered(
            &function("present"),
            &["finish_presented_frame(".to_owned()],
            "present",
        );
        assert_ordered(
            &function("finish_presented_frame"),
            &[
                marker("FrameAccounted"),
                marker("PresentationAcknowledged"),
                marker("FrameFinished"),
            ],
            "finish_presented_frame",
        );
    }

    #[test]
    fn launcher_input_phase_keeps_capture_route_and_yield_inside_one_boundary() {
        let input = function("input");
        assert_ordered(
            &input,
            &[
                "'input_phase:{".to_owned(),
                marker("InputCaptured"),
                marker("InputConsumed"),
                marker("InputRouted"),
            ],
            "input",
        );
        assert!(input.contains("(false,input_batch_empty)"));
        let project = function("project");
        assert!(
            project.contains("ifinput.input_phase_yielded{returnErr(EndFrame);}"),
            "a yielded input phase must end the frame before projection"
        );
    }

    #[test]
    fn launcher_input_phase_preserves_router_and_state_parity_operations() {
        let phase = function("input");
        for operation in [
            "pad.drain_input_batch()",
            "input_router.accept_batch(&input_batch)",
            "input_router.consume_remaining_batch(",
            "input_router.set_focus(",
            "input_router.route_event(",
            "input_router.tick_repeat(",
        ] {
            assert!(phase.contains(operation), "input phase omitted {operation}");
        }
    }

    #[test]
    fn production_latch_hooks_preserve_account_confirm_and_readiness_order() {
        assert!(function("present").contains(&marker("FrameSubmitted")));
        assert!(
            function("post_accounting_and_latch_wait").contains(&marker("PostSubmitAccounted"))
        );
        assert_ordered(
            &function("account_confirmed_present"),
            &[
                marker("ActiveConfirmed"),
                marker("ReadinessSourceAcknowledged"),
            ],
            "account_confirmed_present",
        );
        assert!(function("finish_presented_frame").contains(&marker("FrameAccounted")));
    }
}
