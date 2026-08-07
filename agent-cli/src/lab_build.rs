// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared host build policy for runnable lab artifacts.

use std::path::{Path, PathBuf};
use std::process::Command;

pub const RUNNABLE_LAB_PROFILE: &str = "release-live";

pub fn command(lab: &Path, binary: Option<&str>) -> Command {
    let mut command = Command::new("cargo");
    command
        .current_dir(lab)
        .args(["build", "--locked", "--profile", RUNNABLE_LAB_PROFILE]);
    if let Some(binary) = binary {
        command.args(["--bin", binary]);
    }
    command
}

#[must_use]
pub fn artifact(lab: &Path, binary: &str) -> PathBuf {
    lab.join("target").join(RUNNABLE_LAB_PROFILE).join(binary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runnable_lab_build_and_artifact_share_the_release_live_profile() {
        let lab = Path::new("/tmp/scene-lab");
        let command = command(lab, Some("scene-lab"));
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "build",
                "--locked",
                "--profile",
                RUNNABLE_LAB_PROFILE,
                "--bin",
                "scene-lab",
            ]
        );
        assert_eq!(
            artifact(lab, "scene-lab"),
            lab.join("target/release-live/scene-lab")
        );
    }
}
