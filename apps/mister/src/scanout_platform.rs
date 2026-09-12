// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Resolve the exact installed scanout profile without trusting process environment.

use mister_magik_catalog::device_layout::DevicePaths;
use mister_magik_platform_manifest_contract::{Layout, ParsedManifest, ValidationProfile};
use mister_magik_scanout_contract::{
    DEVELOPMENT_KERNEL_RELEASE, DEVELOPMENT_KERNEL_REVISION, DEVELOPMENT_PLATFORM_CONTRACT_ID,
    DEVELOPMENT_PROFILE, DEVELOPMENT_PROVIDER_IDENTITY, LEGACY_KERNEL_RELEASE, LEGACY_PROFILE,
    PlatformProfile, resolve_profile,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const DEVELOPMENT_VERMAGIC: &str = "6.18.38-MiSTer SMP mod_unload ARMv7 p2v8 ";

pub fn current(kernel_release: &str) -> Result<PlatformProfile, String> {
    let paths = DevicePaths::current();
    resolve_installed(kernel_release, paths.layout(), &paths)
}

fn resolve_installed(
    kernel_release: &str,
    layout: Layout,
    paths: &DevicePaths,
) -> Result<PlatformProfile, String> {
    if kernel_release == LEGACY_KERNEL_RELEASE {
        let metadata = parse_metadata(&paths.scanout_metadata_path())?;
        if metadata.contains_key("kernel_release")
            || metadata.contains_key("platform_profile")
            || metadata.contains_key("provider_identity")
        {
            let metadata_release = metadata
                .get("kernel_release")
                .map(String::as_str)
                .unwrap_or(kernel_release);
            return resolve_profile(
                metadata_release,
                metadata.get("platform_profile").map(String::as_str),
                metadata.get("provider_identity").map(String::as_str),
                layout == Layout::Development,
            )
            .filter(|profile| *profile == LEGACY_PROFILE)
            .ok_or_else(|| "legacy scanout profile identity mismatch".to_owned());
        }
        return Ok(LEGACY_PROFILE);
    }
    if kernel_release != DEVELOPMENT_KERNEL_RELEASE || layout != Layout::Development {
        return Err(format!(
            "unsupported kernel/layout: {kernel_release} {layout:?}"
        ));
    }

    let manifest_text = fs::read_to_string(paths.manifest_path())
        .map_err(|error| format!("development platform manifest unavailable: {error}"))?;
    let manifest = mister_magik_platform_manifest_contract::parse(
        &manifest_text,
        Layout::Development,
        ValidationProfile::AgentStrict,
    )
    .map_err(|error| format!("development platform manifest invalid: {error}"))?;
    verify_artifact(
        &manifest,
        "scanout_module_sha256",
        &paths.scanout_module_path(),
    )?;
    verify_artifact(
        &manifest,
        "scanout_metadata_sha256",
        &paths.scanout_metadata_path(),
    )?;
    verify_artifact(&manifest, "gui_sha256", &paths.gui_path())?;

    let metadata = parse_metadata(&paths.scanout_metadata_path())?;
    require_metadata(&metadata, "kernel_release", DEVELOPMENT_KERNEL_RELEASE)?;
    require_metadata(&metadata, "kernel_revision", DEVELOPMENT_KERNEL_REVISION)?;
    require_metadata(
        &metadata,
        "platform_profile",
        DEVELOPMENT_PLATFORM_CONTRACT_ID,
    )?;
    require_metadata(
        &metadata,
        "provider_identity",
        DEVELOPMENT_PROVIDER_IDENTITY,
    )?;
    require_metadata(&metadata, "vermagic", DEVELOPMENT_VERMAGIC)?;
    require_metadata(
        &metadata,
        "module_sha256",
        manifest
            .required("scanout_module_sha256")
            .map_err(|error| error.to_string())?,
    )?;

    resolve_profile(
        kernel_release,
        metadata.get("platform_profile").map(String::as_str),
        metadata.get("provider_identity").map(String::as_str),
        true,
    )
    .filter(|profile| *profile == DEVELOPMENT_PROFILE)
    .ok_or_else(|| "development scanout profile identity mismatch".to_owned())
}

fn verify_artifact(manifest: &ParsedManifest, hash_field: &str, path: &Path) -> Result<(), String> {
    let expected = manifest
        .required(hash_field)
        .map_err(|error| error.to_string())?;
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut observed = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut observed, "{byte:02x}").expect("writing to a String cannot fail");
    }
    if observed != expected {
        return Err(format!("{hash_field} mismatch"));
    }
    Ok(())
}

fn parse_metadata(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("scanout metadata unavailable: {error}"))?;
    let mut values = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("malformed scanout metadata line {}", index + 1));
        };
        if key.is_empty()
            || value.is_empty()
            || values.insert(key.to_owned(), value.to_owned()).is_some()
        {
            return Err(format!(
                "duplicate or empty scanout metadata line {}",
                index + 1
            ));
        }
    }
    Ok(values)
}

fn require_metadata(
    metadata: &BTreeMap<String, String>,
    field: &str,
    expected: &str,
) -> Result<(), String> {
    match metadata.get(field).map(String::as_str) {
        Some(observed) if observed == expected => Ok(()),
        Some(observed) => Err(format!(
            "scanout metadata {field} mismatch: observed={observed} expected={expected}"
        )),
        None => Err(format!("scanout metadata missing {field}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_platform_manifest_contract::qualification_candidate_id;

    fn digest_bytes(bytes: &[u8]) -> String {
        let mut digest = String::with_capacity(64);
        for byte in Sha256::digest(bytes) {
            write!(&mut digest, "{byte:02x}").unwrap();
        }
        digest
    }

    fn development_fixture(root: &Path) -> DevicePaths {
        let paths = DevicePaths::remapped(Layout::Development, root);
        fs::create_dir_all(paths.app_dir()).unwrap();
        fs::write(paths.scanout_module_path(), b"module").unwrap();
        fs::write(paths.gui_path(), b"runtime").unwrap();
        let module_hash = digest_bytes(b"module");
        let metadata = format!(
            "kernel_release={DEVELOPMENT_KERNEL_RELEASE}\n\
             kernel_revision={DEVELOPMENT_KERNEL_REVISION}\n\
             platform_profile={DEVELOPMENT_PLATFORM_CONTRACT_ID}\n\
             provider_identity={DEVELOPMENT_PROVIDER_IDENTITY}\n\
             development_only=1\n\
             platform_contract_sha256={}\n\
             module_sha256={module_hash}\n\
             vermagic={DEVELOPMENT_VERMAGIC}\n",
            "a".repeat(64)
        );
        fs::write(paths.scanout_metadata_path(), &metadata).unwrap();

        let mut values = BTreeMap::new();
        values.insert("format".to_owned(), "mister-magik-platform-v3".to_owned());
        values.insert("platform_release".to_owned(), "platform-v0.42".to_owned());
        values.insert("platform_release_number".to_owned(), "42".to_owned());
        values.insert("platform_bundle_id".to_owned(), "b".repeat(64));
        values.insert("qualification_candidate_id".to_owned(), "0".repeat(64));
        values.insert("latch_protocol_version".to_owned(), "5".to_owned());
        values.insert("latch_capability_mask".to_owned(), "0x03ff".to_owned());
        for (name, installed) in Layout::Development.paths().components() {
            values.insert(format!("{name}_path"), installed.to_owned());
            values.insert(format!("{name}_sha256"), "c".repeat(64));
        }
        values.insert("scanout_module_sha256".to_owned(), module_hash);
        values.insert(
            "scanout_metadata_sha256".to_owned(),
            digest_bytes(metadata.as_bytes()),
        );
        values.insert("gui_sha256".to_owned(), digest_bytes(b"runtime"));
        values.insert("platform_contract_sha256".to_owned(), "a".repeat(64));
        values.insert("main_revision".to_owned(), "d".repeat(40));
        values.insert("magik_revision".to_owned(), "e".repeat(40));
        values.insert("menu_revision".to_owned(), "f".repeat(40));
        values.insert(
            "qualification_candidate_id".to_owned(),
            qualification_candidate_id(&values),
        );
        fs::write(
            paths.manifest_path(),
            mister_magik_platform_manifest_contract::serialize(&values).unwrap(),
        )
        .unwrap();
        paths
    }

    #[test]
    fn legacy_release_does_not_require_new_metadata() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-scanout-profile-legacy-{}",
            std::process::id()
        ));
        let paths = DevicePaths::remapped(Layout::Public, &root);
        fs::create_dir_all(paths.app_dir()).unwrap();
        fs::write(
            paths.scanout_metadata_path(),
            "module_sha256=legacy\nvermagic=5.15.1-MiSTer SMP\n",
        )
        .unwrap();
        assert_eq!(
            resolve_installed(LEGACY_KERNEL_RELEASE, Layout::Public, &paths),
            Ok(LEGACY_PROFILE)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn development_release_is_never_accepted_in_public_layout() {
        let paths = DevicePaths::remapped(Layout::Public, "/missing");
        assert!(
            resolve_installed(DEVELOPMENT_KERNEL_RELEASE, Layout::Public, &paths)
                .unwrap_err()
                .contains("unsupported kernel/layout")
        );
    }

    #[test]
    fn legacy_kernel_rejects_explicit_development_metadata() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-scanout-profile-legacy-mixed-{}",
            std::process::id()
        ));
        let paths = development_fixture(&root);
        assert_eq!(
            resolve_installed(LEGACY_KERNEL_RELEASE, Layout::Development, &paths).unwrap_err(),
            "legacy scanout profile identity mismatch"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_development_tuple_resolves_without_qualification_environment() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-scanout-profile-valid-{}",
            std::process::id()
        ));
        let paths = development_fixture(&root);
        assert_eq!(
            resolve_installed(DEVELOPMENT_KERNEL_RELEASE, Layout::Development, &paths),
            Ok(DEVELOPMENT_PROFILE)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn development_tuple_rejects_changed_provider_identity() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-scanout-profile-mixed-{}",
            std::process::id()
        ));
        let paths = development_fixture(&root);
        let metadata_path = paths.scanout_metadata_path();
        let metadata = fs::read_to_string(&metadata_path)
            .unwrap()
            .replace(DEVELOPMENT_PROVIDER_IDENTITY, "different-provider");
        fs::write(&metadata_path, metadata).unwrap();
        assert!(
            resolve_installed(DEVELOPMENT_KERNEL_RELEASE, Layout::Development, &paths)
                .unwrap_err()
                .contains("scanout_metadata_sha256 mismatch")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn metadata_parser_rejects_duplicate_fields() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-scanout-profile-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("metadata.txt");
        fs::write(&path, "kernel_release=one\nkernel_release=two\n").unwrap();
        assert!(parse_metadata(&path).unwrap_err().contains("duplicate"));
        fs::remove_dir_all(root).unwrap();
    }
}
