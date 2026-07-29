// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::AgentResult;
use std::fs;
use std::path::{Path, PathBuf};

const PLATFORM_MANIFEST: &str = "platform-v3.manifest";

pub(super) fn stage_published_platform_components(
    extracted: &Path,
    stage: &Path,
) -> AgentResult<()> {
    for (from, to) in [
        (
            crate::platform_bundle::MANIFEST,
            crate::platform_bundle::MANIFEST,
        ),
        ("main/MiSTer_MagiK", "MiSTer_MagiKDev"),
        (
            "scanout/mister_magik_scanout_slots.ko",
            "mister_magik_scanout_slots.ko",
        ),
        (
            "scanout/provenance.txt",
            "mister_magik_scanout_slots.metadata.txt",
        ),
        (
            "fpga/patched/menu-magik-vblank-latch.rbf",
            "fpga/menu-magik-vblank-latch.rbf",
        ),
        (
            "fpga/patched/menu-magik-vblank-latch.metadata.txt",
            "fpga/menu-magik-vblank-latch.metadata.txt",
        ),
    ] {
        copy(extracted.join(from), stage.join(to))?;
    }
    Ok(())
}

pub(super) fn generate_platform_manifest(
    repository: &Path,
    stage: &Path,
    main_revision: &str,
) -> AgentResult<()> {
    debug_assert_eq!(PLATFORM_MANIFEST, crate::platform_manifest::FILE_NAME);
    let release = crate::platform_manifest::ReleaseIdentity::from_bundle_manifest(
        &stage.join(crate::platform_bundle::MANIFEST),
    )?;
    crate::platform_manifest::generate(
        &stage.join(PLATFORM_MANIFEST),
        &crate::platform_manifest::Artifacts {
            main: stage.join("MiSTer_MagiKDev"),
            gui: stage.join("mister-magik-fb"),
            manager: stage.join("mister-magik-manager"),
            scanout_module: stage.join("mister_magik_scanout_slots.ko"),
            scanout_metadata: stage.join("mister_magik_scanout_slots.metadata.txt"),
            latch_rbf: stage.join("fpga/menu-magik-vblank-latch.rbf"),
            latch_metadata: stage.join("fpga/menu-magik-vblank-latch.metadata.txt"),
        },
        &release,
        main_revision,
        &crate::git::value(repository, &["rev-parse", "HEAD"])?,
        crate::platform_manifest::Layout::Development,
    )
}

fn copy(from: PathBuf, to: PathBuf) -> AgentResult<()> {
    Ok(fs::copy(&from, &to)
        .map(|_| ())
        .map_err(|error| format!("cannot copy {}: {error}", from.display()))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_platform_components_are_staged_as_one_bundle() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-published-platform-stage-{}",
            std::process::id()
        ));
        let extracted = root.join("candidate");
        let stage = root.join("stage");
        fs::create_dir_all(extracted.join("main")).unwrap();
        fs::create_dir_all(extracted.join("scanout")).unwrap();
        fs::create_dir_all(extracted.join("fpga/patched")).unwrap();
        fs::create_dir_all(stage.join("fpga")).unwrap();
        fs::write(extracted.join("main/MiSTer_MagiK"), b"github-main").unwrap();
        fs::write(
            extracted.join(crate::platform_bundle::MANIFEST),
            format!(
                "{{\"format\":\"{}\",\"release_version\":16,\"bundle_id\":\"{}\"}}\n",
                crate::platform_bundle::FORMAT,
                "a".repeat(64)
            ),
        )
        .unwrap();
        fs::write(
            extracted.join("scanout/mister_magik_scanout_slots.ko"),
            b"github-kernel",
        )
        .unwrap();
        fs::write(
            extracted.join("scanout/provenance.txt"),
            b"github-kernel-metadata",
        )
        .unwrap();
        fs::write(
            extracted.join("fpga/patched/menu-magik-vblank-latch.rbf"),
            b"github-rbf",
        )
        .unwrap();
        fs::write(
            extracted.join("fpga/patched/menu-magik-vblank-latch.metadata.txt"),
            b"github-rbf-metadata",
        )
        .unwrap();

        stage_published_platform_components(&extracted, &stage).unwrap();

        assert_eq!(
            fs::read(stage.join("MiSTer_MagiKDev")).unwrap(),
            b"github-main"
        );
        assert_eq!(
            fs::read(stage.join("mister_magik_scanout_slots.ko")).unwrap(),
            b"github-kernel"
        );
        assert_eq!(
            fs::read(stage.join("mister_magik_scanout_slots.metadata.txt")).unwrap(),
            b"github-kernel-metadata"
        );
        assert_eq!(
            fs::read(stage.join("fpga/menu-magik-vblank-latch.rbf")).unwrap(),
            b"github-rbf"
        );
        assert_eq!(
            fs::read(stage.join("fpga/menu-magik-vblank-latch.metadata.txt")).unwrap(),
            b"github-rbf-metadata"
        );
        let _ = fs::remove_dir_all(root);
    }
}
