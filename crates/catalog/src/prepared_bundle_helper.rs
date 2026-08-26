// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Prototype exact-release helpers for prepared computer-game collections.
//!
//! A helper contains precomputed catalog rows plus a bounded receipt for the
//! installed collection. Exact receipts can publish those rows directly. Any
//! disagreement is fail-open only toward the normal collection scanner: the
//! helper never hides custom, older, partial, or newly added content.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path};

pub const PREPARED_BUNDLE_HELPER_SCHEMA: &str = "mister-magik-prepared-bundle-helper-v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PreparedBundleHelper {
    pub schema: String,
    pub collection_id: String,
    pub release_id: String,
    pub entries: Vec<PreparedBundleEntry>,
    pub exact_files: Vec<ExactFileReceipt>,
    pub payloads: Vec<PayloadReceipt>,
    pub inventories: Vec<InventoryReceipt>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PreparedBundleEntry {
    pub system_id: String,
    pub title: String,
    pub launch_ref: String,
    pub category: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ExactFileReceipt {
    pub relative_path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PayloadReceipt {
    pub relative_path: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryReceipt {
    pub relative_root: String,
    pub extensions: Vec<String>,
    pub excluded_components: Vec<String>,
    pub relative_paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryRule {
    pub relative_root: String,
    pub extensions: Vec<String>,
    pub excluded_components: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreparedBundlePath {
    Exact,
    Fallback,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PreparedBundleActivation {
    pub path: PreparedBundlePath,
    pub collection_id: String,
    pub release_id: String,
    pub reason: Option<String>,
    pub entries: Vec<PreparedBundleEntry>,
}

impl PreparedBundleHelper {
    pub fn capture(
        root: &Path,
        collection_id: impl Into<String>,
        release_id: impl Into<String>,
        mut entries: Vec<PreparedBundleEntry>,
        exact_relative_paths: &[String],
        payload_relative_paths: &[String],
        inventory_rules: &[InventoryRule],
    ) -> Result<Self, String> {
        entries.sort();
        reject_duplicate_entries(&entries)?;

        let mut exact_files = exact_relative_paths
            .iter()
            .map(|relative| capture_exact_file(root, relative))
            .collect::<Result<Vec<_>, _>>()?;
        exact_files.sort();
        reject_duplicate_paths(
            exact_files
                .iter()
                .map(|receipt| receipt.relative_path.as_str()),
            "exact file",
        )?;

        let mut payloads = payload_relative_paths
            .iter()
            .map(|relative| capture_payload(root, relative))
            .collect::<Result<Vec<_>, _>>()?;
        payloads.sort();
        reject_duplicate_paths(
            payloads
                .iter()
                .map(|receipt| receipt.relative_path.as_str()),
            "payload",
        )?;

        let inventories = inventory_rules
            .iter()
            .map(|rule| capture_inventory(root, rule))
            .collect::<Result<Vec<_>, _>>()?;
        let helper = Self {
            schema: PREPARED_BUNDLE_HELPER_SCHEMA.to_string(),
            collection_id: collection_id.into(),
            release_id: release_id.into(),
            entries,
            exact_files,
            payloads,
            inventories,
        };
        helper.validate()?;
        Ok(helper)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let helper: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("decode prepared bundle helper: {error}"))?;
        helper.validate()?;
        Ok(helper)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| format!("encode prepared bundle helper: {error}"))
    }

    pub fn fingerprint(&self) -> Result<String, String> {
        let bytes = self.to_json()?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub fn activate_with_fallback(
        &self,
        root: &Path,
        fallback: impl FnOnce() -> Result<Vec<PreparedBundleEntry>, String>,
    ) -> Result<PreparedBundleActivation, String> {
        self.validate()?;
        match self.exact_match(root) {
            Ok(()) => Ok(PreparedBundleActivation {
                path: PreparedBundlePath::Exact,
                collection_id: self.collection_id.clone(),
                release_id: self.release_id.clone(),
                reason: None,
                entries: self.entries.clone(),
            }),
            Err(reason) => {
                let mut entries = fallback()?;
                entries.sort();
                reject_duplicate_entries(&entries)?;
                Ok(PreparedBundleActivation {
                    path: PreparedBundlePath::Fallback,
                    collection_id: self.collection_id.clone(),
                    release_id: self.release_id.clone(),
                    reason: Some(reason),
                    entries,
                })
            }
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != PREPARED_BUNDLE_HELPER_SCHEMA {
            return Err(format!(
                "unsupported prepared bundle helper schema {}",
                self.schema
            ));
        }
        if self.collection_id.trim().is_empty() || self.release_id.trim().is_empty() {
            return Err("prepared bundle helper identity is empty".to_string());
        }
        reject_duplicate_entries(&self.entries)?;
        reject_duplicate_paths(
            self.exact_files
                .iter()
                .map(|receipt| receipt.relative_path.as_str()),
            "exact file",
        )?;
        reject_duplicate_paths(
            self.payloads
                .iter()
                .map(|receipt| receipt.relative_path.as_str()),
            "payload",
        )?;
        for receipt in &self.exact_files {
            validate_relative_path(&receipt.relative_path)?;
            if receipt.sha256.len() != 64
                || !receipt.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(format!(
                    "prepared bundle exact-file hash is invalid: {}",
                    receipt.relative_path
                ));
            }
        }
        for receipt in &self.payloads {
            validate_relative_path(&receipt.relative_path)?;
        }
        for inventory in &self.inventories {
            validate_relative_path(&inventory.relative_root)?;
            reject_duplicate_paths(
                inventory.relative_paths.iter().map(String::as_str),
                "inventory",
            )?;
            for relative in &inventory.relative_paths {
                validate_relative_path(relative)?;
            }
        }
        Ok(())
    }

    fn exact_match(&self, root: &Path) -> Result<(), String> {
        for expected in &self.exact_files {
            let actual = capture_exact_file(root, &expected.relative_path)?;
            if actual != *expected {
                return Err(format!("changed exact file: {}", expected.relative_path));
            }
        }
        for expected in &self.payloads {
            let actual = capture_payload(root, &expected.relative_path)?;
            if actual != *expected {
                return Err(format!("changed payload: {}", expected.relative_path));
            }
        }
        for expected in &self.inventories {
            let rule = InventoryRule {
                relative_root: expected.relative_root.clone(),
                extensions: expected.extensions.clone(),
                excluded_components: expected.excluded_components.clone(),
            };
            let actual = capture_inventory(root, &rule)?;
            if actual.relative_paths != expected.relative_paths {
                return Err(format!("changed {} inventory", expected.relative_root));
            }
        }
        Ok(())
    }
}

fn capture_exact_file(root: &Path, relative: &str) -> Result<ExactFileReceipt, String> {
    validate_relative_path(relative)?;
    let path = root.join(relative);
    let bytes = fs::read(&path)
        .map_err(|error| format!("read exact bundle file {}: {error}", path.display()))?;
    let len = u64::try_from(bytes.len()).map_err(|error| error.to_string())?;
    Ok(ExactFileReceipt {
        relative_path: normalize_relative_path(relative),
        bytes: len,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    })
}

fn capture_payload(root: &Path, relative: &str) -> Result<PayloadReceipt, String> {
    validate_relative_path(relative)?;
    let path = root.join(relative);
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("inspect bundle payload {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("bundle payload is not a file: {}", path.display()));
    }
    Ok(PayloadReceipt {
        relative_path: normalize_relative_path(relative),
        bytes: metadata.len(),
    })
}

fn capture_inventory(root: &Path, rule: &InventoryRule) -> Result<InventoryReceipt, String> {
    validate_relative_path(&rule.relative_root)?;
    let inventory_root = root.join(&rule.relative_root);
    let extensions = rule
        .extensions
        .iter()
        .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    if extensions.is_empty() {
        return Err(format!(
            "prepared bundle inventory has no extensions: {}",
            rule.relative_root
        ));
    }
    let excluded = rule
        .excluded_components
        .iter()
        .map(|component| component.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mut relative_paths = Vec::new();
    for entry in walkdir::WalkDir::new(&inventory_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.path() == inventory_root
                || !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| excluded.contains(&name.to_ascii_lowercase()))
        })
    {
        let entry = entry.map_err(|error| {
            format!(
                "scan prepared bundle inventory {}: {error}",
                inventory_root.display()
            )
        })?;
        if entry.path() == inventory_root || !entry.file_type().is_file() {
            continue;
        }
        let matches = entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extensions.contains(&extension.to_ascii_lowercase()));
        if !matches {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|error| format!("make prepared bundle inventory path relative: {error}"))?;
        relative_paths.push(path_to_slash(relative)?);
    }
    relative_paths.sort_by_cached_key(|path| path.to_ascii_lowercase());
    reject_case_folded_duplicates(&relative_paths, "inventory")?;
    Ok(InventoryReceipt {
        relative_root: normalize_relative_path(&rule.relative_root),
        extensions: extensions.into_iter().collect(),
        excluded_components: excluded.into_iter().collect(),
        relative_paths,
    })
}

fn validate_relative_path(relative: &str) -> Result<(), String> {
    let path = Path::new(relative);
    if relative.trim().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("invalid prepared bundle relative path: {relative}"));
    }
    Ok(())
}

fn normalize_relative_path(relative: &str) -> String {
    relative
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

fn path_to_slash(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(normalize_relative_path)
        .ok_or_else(|| format!("prepared bundle path is not UTF-8: {}", path.display()))
}

fn reject_duplicate_entries(entries: &[PreparedBundleEntry]) -> Result<(), String> {
    let mut keys = BTreeSet::new();
    for entry in entries {
        let key = format!(
            "{}\0{}\0{}",
            entry.system_id.to_ascii_lowercase(),
            entry.title.to_ascii_lowercase(),
            entry.launch_ref.to_ascii_lowercase()
        );
        if !keys.insert(key) {
            return Err(format!(
                "duplicate prepared bundle entry: {} / {}",
                entry.system_id, entry.title
            ));
        }
    }
    Ok(())
}

fn reject_duplicate_paths<'a>(
    paths: impl IntoIterator<Item = &'a str>,
    label: &str,
) -> Result<(), String> {
    let paths = paths.into_iter().map(str::to_string).collect::<Vec<_>>();
    reject_case_folded_duplicates(&paths, label)
}

fn reject_case_folded_duplicates(paths: &[String], label: &str) -> Result<(), String> {
    let mut folded = BTreeSet::new();
    for path in paths {
        validate_relative_path(path)?;
        if !folded.insert(path.to_ascii_lowercase()) {
            return Err(format!("duplicate {label} path: {path}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "prepared-bundle-helper-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn entry(title: &str) -> PreparedBundleEntry {
        PreparedBundleEntry {
            system_id: "dos".to_string(),
            title: title.to_string(),
            launch_ref: format!("_DOS Games/{title}.mgl"),
            category: "Computer".to_string(),
        }
    }

    fn helper(root: &Path) -> PreparedBundleHelper {
        PreparedBundleHelper::capture(
            root,
            "0mhz",
            "2026.04.26",
            vec![entry("Doom")],
            &["_DOS Games/Doom.mgl".to_string()],
            &["games/AO486/Doom.vhd".to_string()],
            &[InventoryRule {
                relative_root: "_DOS Games".to_string(),
                extensions: vec!["mgl".to_string()],
                excluded_components: Vec::new(),
            }],
        )
        .unwrap()
    }

    fn write_fixture(root: &Path) {
        fs::create_dir_all(root.join("_DOS Games")).unwrap();
        fs::create_dir_all(root.join("games/AO486")).unwrap();
        fs::write(
            root.join("_DOS Games/Doom.mgl"),
            b"<mistergamedescription/>",
        )
        .unwrap();
        fs::write(root.join("games/AO486/Doom.vhd"), b"payload").unwrap();
    }

    #[test]
    fn exact_release_uses_precomputed_entries_without_fallback() {
        let root = fixture_root("exact");
        write_fixture(&root);
        let helper = helper(&root);
        let activation = helper
            .activate_with_fallback(&root, || panic!("fallback must not run"))
            .unwrap();
        fs::remove_dir_all(root).unwrap();

        assert_eq!(activation.path, PreparedBundlePath::Exact);
        assert_eq!(activation.entries, vec![entry("Doom")]);
    }

    #[test]
    fn changed_known_file_uses_fallback() {
        let root = fixture_root("changed");
        write_fixture(&root);
        let helper = helper(&root);
        fs::write(root.join("_DOS Games/Doom.mgl"), b"changed").unwrap();
        let activation = helper
            .activate_with_fallback(&root, || Ok(vec![entry("Custom Doom")]))
            .unwrap();
        fs::remove_dir_all(root).unwrap();

        assert_eq!(activation.path, PreparedBundlePath::Fallback);
        assert_eq!(activation.entries, vec![entry("Custom Doom")]);
        assert!(activation.reason.unwrap().contains("changed exact file"));
    }

    #[test]
    fn additional_game_uses_fallback_so_custom_content_is_not_hidden() {
        let root = fixture_root("additional");
        write_fixture(&root);
        let helper = helper(&root);
        fs::write(root.join("_DOS Games/Homebrew.mgl"), b"custom").unwrap();
        let activation = helper
            .activate_with_fallback(&root, || Ok(vec![entry("Doom"), entry("Homebrew")]))
            .unwrap();
        fs::remove_dir_all(root).unwrap();

        assert_eq!(activation.path, PreparedBundlePath::Fallback);
        assert_eq!(activation.entries.len(), 2);
        assert!(
            activation
                .reason
                .unwrap()
                .contains("changed _DOS Games inventory")
        );
    }

    #[test]
    fn missing_payload_uses_fallback() {
        let root = fixture_root("missing-payload");
        write_fixture(&root);
        let helper = helper(&root);
        fs::remove_file(root.join("games/AO486/Doom.vhd")).unwrap();
        let activation = helper.activate_with_fallback(&root, Vec::new).unwrap();
        fs::remove_dir_all(root).unwrap();

        assert_eq!(activation.path, PreparedBundlePath::Fallback);
        assert!(activation.reason.unwrap().contains("bundle payload"));
    }

    #[test]
    fn helper_round_trips_with_a_stable_fingerprint() {
        let root = fixture_root("round-trip");
        write_fixture(&root);
        let helper = helper(&root);
        let bytes = helper.to_json().unwrap();
        let decoded = PreparedBundleHelper::from_json(&bytes).unwrap();
        fs::remove_dir_all(root).unwrap();

        assert_eq!(decoded, helper);
        assert_eq!(
            decoded.fingerprint().unwrap(),
            helper.fingerprint().unwrap()
        );
    }
}
