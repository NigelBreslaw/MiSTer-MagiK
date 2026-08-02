// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::build::{BuildRecipe, BuildSpec};
use crate::device::DeviceClient;
use crate::error::AgentResult;
use crate::progress::{EventKind, Reporter};
use crate::transport::{DeviceOperations, DeviceRequest};
use std::fs;
use std::path::Path;

const RUNTIME_REMOTE: &str = "/media/fat/mister-magik-dev/mister-magik-fb";
const MANIFEST_REMOTE: &str = "/media/fat/mister-magik-dev/platform-v3.manifest";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveParticlesExecution {
    pub app_revision: String,
    pub gui_sha256: String,
    pub qualification_candidate_id: String,
}

pub fn execute(
    repository: &Path,
    app_revision: &str,
    reporter: &mut Reporter<'_>,
) -> AgentResult<LiveParticlesExecution> {
    require_clean_revision(repository, app_revision)?;
    let spec = BuildSpec::for_recipe(BuildRecipe::LiveParticles);
    reporter.emit(
        EventKind::Progress,
        "live-particles-build",
        "building the exact experimental runtime commit",
        Some(10),
    )?;
    crate::build::execute(repository, &spec, reporter)?;
    require_clean_revision(repository, app_revision)?;
    let receipt = spec.verify(repository)?;
    if receipt.source_commit != app_revision || receipt.source_dirty {
        return Err("live-particle build receipt is not from the exact clean commit".into());
    }

    reporter.emit(
        EventKind::Progress,
        "live-particles-install",
        "installing the experimental runtime into the development layout",
        Some(85),
    )?;
    execute_install_transaction(
        repository,
        app_revision,
        spec.artifact(),
        DeviceClient::default(),
    )
}

fn execute_install_transaction<D: DeviceOperations>(
    repository: &Path,
    app_revision: &str,
    artifact: &Path,
    mut device: DeviceClient<D>,
) -> AgentResult<LiveParticlesExecution> {
    device.execute(DeviceRequest::Discover)?;
    device.execute(DeviceRequest::VerifyDevelopmentPlatform)?;
    let installed = device.execute(DeviceRequest::ReadDevelopmentManifest)?;
    let stage = repository
        .join("build/agent-deploy/live-particles")
        .join(app_revision);
    if stage.exists() {
        fs::remove_dir_all(&stage)
            .map_err(|error| format!("cannot clear {}: {error}", stage.display()))?;
    }
    fs::create_dir_all(&stage)
        .map_err(|error| format!("cannot create {}: {error}", stage.display()))?;
    let manifest = stage.join(crate::platform_manifest::FILE_NAME);
    let artifact = repository.join(artifact);
    let identity = crate::platform_manifest::write_live_particles_overlay(
        &manifest,
        &installed,
        &artifact,
        app_revision,
    )?;
    device.execute(DeviceRequest::DeliverRuntimeTransaction {
        local: artifact,
        remote: RUNTIME_REMOTE.into(),
        manifest_local: manifest,
        manifest_remote: MANIFEST_REMOTE.into(),
        expected_sha256: identity.gui_sha256.clone(),
    })?;
    Ok(LiveParticlesExecution {
        app_revision: app_revision.into(),
        gui_sha256: identity.gui_sha256,
        qualification_candidate_id: identity.qualification_candidate_id,
    })
}

fn require_clean_revision(repository: &Path, expected: &str) -> AgentResult<()> {
    let head = crate::git::value(repository, &["rev-parse", "HEAD"])?;
    if head != expected {
        return Err(format!(
            "app source identity changed during live-particle installation: expected={expected} actual={head}"
        )
        .into());
    }
    let dirty = crate::git::value(repository, &["status", "--porcelain"])?;
    if !dirty.is_empty() {
        return Err("app worktree must be clean for live-particle installation".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{DeviceResponse, FakeDevice};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

    fn manifest(magik_revision: &str) -> String {
        let fields = crate::platform_manifest::FIELDS;
        let mut values = BTreeMap::new();
        values.insert("format".to_owned(), "mister-magik-platform-v3".to_owned());
        values.insert("platform_release".to_owned(), "platform-v0.16".to_owned());
        values.insert("platform_release_number".to_owned(), "16".to_owned());
        values.insert("platform_bundle_id".to_owned(), "c".repeat(64));
        values.insert("latch_protocol_version".to_owned(), "4".to_owned());
        values.insert("latch_capability_mask".to_owned(), "0x01ff".to_owned());
        for (name, path) in crate::platform_manifest::Layout::Development.paths() {
            values.insert(format!("{name}_path"), path.into());
        }
        for name in [
            "main",
            "gui",
            "manager",
            "scanout_module",
            "scanout_metadata",
            "latch_rbf",
            "latch_metadata",
            "platform_contract",
        ] {
            values.insert(format!("{name}_sha256"), "d".repeat(64));
        }
        values.insert("main_revision".to_owned(), "e".repeat(40));
        values.insert("magik_revision".to_owned(), magik_revision.into());
        values.insert("menu_revision".to_owned(), "f".repeat(40));
        let mut hash = Sha256::new();
        for field in fields {
            if *field != "qualification_candidate_id" {
                hash.update(field.as_bytes());
                hash.update(b"=");
                hash.update(values[*field].as_bytes());
                hash.update(b"\n");
            }
        }
        values.insert(
            "qualification_candidate_id".to_owned(),
            hash.finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        );
        fields
            .iter()
            .map(|field| format!("{field}={}\n", values[*field]))
            .collect()
    }

    #[test]
    fn installation_targets_only_the_canonical_development_runtime() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-live-particles-device-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let artifact = root.join("runtime");
        fs::write(&artifact, b"runtime").unwrap();
        let revision = "a".repeat(40);
        let fake = FakeDevice::with_results([
            Ok(DeviceResponse {
                operation: "discover",
                detail: "connected".into(),
            }),
            Ok(DeviceResponse {
                operation: "verify-development-platform",
                detail: "verified".into(),
            }),
            Ok(DeviceResponse {
                operation: "read-development-manifest",
                detail: manifest(&"b".repeat(40)),
            }),
            Ok(DeviceResponse {
                operation: "deliver-runtime-transaction",
                detail: "healthy".into(),
            }),
        ]);

        let execution = execute_install_transaction(
            &root,
            &revision,
            Path::new("runtime"),
            DeviceClient::new(fake),
        )
        .unwrap();
        assert_eq!(execution.app_revision, revision);
        let _ = fs::remove_dir_all(root);
    }
}
