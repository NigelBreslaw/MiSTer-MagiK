// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{Operation, Risk};

pub const SCRIPT_REVIEW: &[(&str, &str)] = &[
    (
        "fpga-vblank-latch-one-shot.sh",
        "unreferenced FPGA diagnostic",
    ),
    ("profile-scene-report.sh", "unreferenced scene report"),
    (
        "device-arcade-filter-navigation.sh",
        "unreferenced device acceptance flow",
    ),
    (
        "mister-early-dhcpcd-service.sh",
        "unreferenced boot service installer",
    ),
    (
        "profile-screensaver-preview.sh",
        "unreferenced preview profiler",
    ),
    ("audit-idle-cpu.sh", "unreferenced device audit"),
    (
        "qualify-fpga-latch-release.sh",
        "unreferenced FPGA qualification flow",
    ),
    (
        "capture-launcher-home-pan-video.sh",
        "unreferenced capture workflow",
    ),
    (
        "profile-analytics-overhead.sh",
        "unreferenced analytics profiler",
    ),
    (
        "device-catalog-resume-acceptance.sh",
        "unreferenced catalog acceptance flow",
    ),
    (
        "device-resource-exhaustion.sh",
        "unreferenced destructive device test",
    ),
];

#[must_use]
pub fn operation(id: &str) -> Option<Operation> {
    match id {
        "repo.diff-check" => Some(Operation {
            id: "repo.diff-check".into(),
            title: "Check patch whitespace".into(),
            risk: Risk::ReadOnly,
            program: "git".into(),
            args: vec!["diff".into(), "--check".into()],
            reason: "all patches require whitespace validation".into(),
            failure_hint: "run scripts/agent run show RUN_ID".into(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_identity_is_stable() {
        let left = operation("repo.diff-check").unwrap();
        let right = operation("repo.diff-check").unwrap();
        assert_eq!(left, right);
        assert_eq!(left.id, "repo.diff-check");
    }
}
