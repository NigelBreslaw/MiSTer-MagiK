// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Immutable installed-platform identity attached to operational evidence.

use mister_magik_platform_manifest_contract::{
    Layout, ManifestError, ParsedManifest, ValidationProfile,
};
use serde::Serialize;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityClassification {
    QualifiedRelease,
    QualifiedPlatformDevelopmentRuntime,
    Candidate,
    MixedInvalid,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticIdentity {
    pub classification: IdentityClassification,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_failure: Option<String>,
    pub runtime: RuntimeIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<PlatformIdentity>,
    pub device_boot_id: String,
    pub launcher_session_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeIdentity {
    pub version: &'static str,
    pub build_number: &'static str,
    pub source_revision: &'static str,
    pub source_dirty: Option<bool>,
    pub binary_sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlatformIdentity {
    pub release_tag: String,
    pub release_number: u64,
    pub bundle_id: String,
    pub qualification_candidate_id: String,
    pub manifest_sha256: String,
    pub main_sha256: String,
    pub runtime_sha256: String,
    pub scanout_module_sha256: String,
    pub latch_rbf_sha256: String,
    pub main_revision: String,
    pub magik_revision: String,
    pub menu_revision: String,
    pub latch_protocol_version: u16,
    pub latch_capability_mask: String,
}

static IDENTITY: OnceLock<DiagnosticIdentity> = OnceLock::new();
type IdentityLoadError = (String, Option<Box<PlatformIdentity>>);

pub fn current() -> &'static DiagnosticIdentity {
    IDENTITY.get_or_init(load_current)
}

impl DiagnosticIdentity {
    #[must_use]
    pub fn namespace(&self) -> String {
        let build = safe_component(self.runtime.build_number);
        let boot = short_component(&self.device_boot_id);
        let session = short_component(&self.launcher_session_id);
        match &self.platform {
            Some(platform) => format!(
                "{}/bundle-{}/build-{}/binary-{}/boot-{}/session-{}",
                safe_component(&platform.release_tag),
                short_component(&platform.bundle_id),
                build,
                short_component(self.runtime.binary_sha256.as_deref().unwrap_or("unknown")),
                boot,
                session
            ),
            None => format!("unknown/build-{build}/boot-{boot}/session-{session}"),
        }
    }
}

fn load_current() -> DiagnosticIdentity {
    let build = crate::build_identity::BuildIdentity::current();
    let binary_sha256 = std::env::current_exe()
        .ok()
        .and_then(|path| digest(&path).ok());
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|value| value.trim().to_owned())
        .unwrap_or_else(|_| "unknown-boot".to_owned());
    let session_id = format!(
        "{}-{}-{}",
        boot_id,
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or(0)
    );
    let runtime = RuntimeIdentity {
        version: build.version,
        build_number: build.build_number,
        source_revision: build.source_revision,
        source_dirty: build.source_dirty,
        binary_sha256,
    };
    let manifest_path = mister_magik_catalog::device_layout::current_app_path(
        mister_magik_platform_manifest_contract::FILE_NAME,
    );
    match load_platform(&manifest_path, &runtime) {
        Ok(platform) => DiagnosticIdentity {
            classification: IdentityClassification::Candidate,
            validation_failure: None,
            runtime,
            platform: Some(platform),
            device_boot_id: boot_id,
            launcher_session_id: session_id,
        },
        Err((failure, platform)) => DiagnosticIdentity {
            classification: if platform.is_some() {
                IdentityClassification::MixedInvalid
            } else {
                IdentityClassification::Unknown
            },
            validation_failure: Some(failure),
            runtime,
            platform: platform.map(|platform| *platform),
            device_boot_id: boot_id,
            launcher_session_id: session_id,
        },
    }
}

fn load_platform(
    manifest_path: &Path,
    runtime: &RuntimeIdentity,
) -> Result<PlatformIdentity, IdentityLoadError> {
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|error| (format!("manifest unavailable: {error}"), None))?;
    let manifest_sha256 =
        digest(manifest_path).map_err(|error| (format!("manifest hash failed: {error}"), None))?;
    let layout = match mister_magik_catalog::device_layout::DeviceLayout::current() {
        mister_magik_catalog::device_layout::DeviceLayout::Public => Layout::Public,
        mister_magik_catalog::device_layout::DeviceLayout::Dev => Layout::Development,
    };
    let values =
        mister_magik_platform_manifest_contract::parse(&text, layout, ValidationProfile::GuiLegacy)
            .map_err(|error| (legacy_parse_error(&error), None))?;
    let platform = PlatformIdentity {
        release_tag: required(&values, "platform_release")?.to_owned(),
        release_number: required(&values, "platform_release_number")?
            .parse()
            .map_err(|_| ("invalid platform release number".to_owned(), None))?,
        bundle_id: required(&values, "platform_bundle_id")?.to_owned(),
        qualification_candidate_id: required(&values, "qualification_candidate_id")?.to_owned(),
        manifest_sha256,
        main_sha256: required(&values, "main_sha256")?.to_owned(),
        runtime_sha256: required(&values, "gui_sha256")?.to_owned(),
        scanout_module_sha256: required(&values, "scanout_module_sha256")?.to_owned(),
        latch_rbf_sha256: required(&values, "latch_rbf_sha256")?.to_owned(),
        main_revision: required(&values, "main_revision")?.to_owned(),
        magik_revision: required(&values, "magik_revision")?.to_owned(),
        menu_revision: required(&values, "menu_revision")?.to_owned(),
        latch_protocol_version: required(&values, "latch_protocol_version")?
            .parse()
            .map_err(|_| ("invalid latch protocol version".to_owned(), None))?,
        latch_capability_mask: required(&values, "latch_capability_mask")?.to_owned(),
    };
    let invalid = runtime.binary_sha256.as_ref() != Some(&platform.runtime_sha256)
        || runtime.source_revision != platform.magik_revision;
    if invalid {
        return Err((
            "installed tuple does not match its v3 manifest".to_owned(),
            Some(Box::new(platform)),
        ));
    }
    for (path_field, hash_field) in [
        ("main_path", "main_sha256"),
        ("scanout_module_path", "scanout_module_sha256"),
        ("latch_rbf_path", "latch_rbf_sha256"),
    ] {
        let path = PathBuf::from(required(&values, path_field)?);
        let expected = required(&values, hash_field)?;
        if digest(&path).ok().as_deref() != Some(expected) {
            return Err((
                format!("{path_field} artifact hash mismatch"),
                Some(Box::new(platform)),
            ));
        }
    }
    Ok(platform)
}

fn required<'a>(values: &'a ParsedManifest, field: &str) -> Result<&'a str, IdentityLoadError> {
    values
        .get(field)
        .ok_or_else(|| (format!("platform manifest missing {field}"), None))
}

fn legacy_parse_error(error: &ManifestError) -> String {
    if error.detail().starts_with("duplicate or empty") {
        "duplicate or empty platform manifest field".to_owned()
    } else if error.code() == "invalid_platform_manifest" {
        "malformed platform manifest".to_owned()
    } else {
        "installed tuple does not match its v3 manifest".to_owned()
    }
}

fn digest(path: &Path) -> io::Result<String> {
    let output = Command::new("sha256sum").arg(path).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "sha256sum failed for {}",
            path.display()
        )));
    }
    let digest = String::from_utf8(output.stdout)
        .ok()
        .and_then(|text| text.split_whitespace().next().map(str::to_owned))
        .filter(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
        .ok_or_else(|| io::Error::other("sha256sum returned an invalid digest"))?;
    Ok(digest)
}

fn safe_component(value: &str) -> String {
    let safe = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        "unknown".to_owned()
    } else {
        safe
    }
}

fn short_component(value: &str) -> String {
    safe_component(value).chars().take(16).collect()
}
