// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Typed platform-v3 manifest structure and installed-layout authority.
//!
//! Filesystem access and artifact hashing remain with callers. Constants in
//! this crate are generated at build time from the adjacent canonical schema.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstalledPaths {
    pub root: &'static str,
    pub manifest: &'static str,
    pub main: &'static str,
    pub gui: &'static str,
    pub manager: &'static str,
    pub scanout_module: &'static str,
    pub scanout_metadata: &'static str,
    pub latch_rbf: &'static str,
    pub latch_metadata: &'static str,
}

impl InstalledPaths {
    pub const fn components(self) -> [(&'static str, &'static str); 7] {
        [
            ("main", self.main),
            ("gui", self.gui),
            ("manager", self.manager),
            ("scanout_module", self.scanout_module),
            ("scanout_metadata", self.scanout_metadata),
            ("latch_rbf", self.latch_rbf),
            ("latch_metadata", self.latch_metadata),
        ]
    }
}

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    Public,
    Development,
}

impl Layout {
    pub fn parse(value: &str) -> Result<Self, ManifestError> {
        match value {
            "public" => Ok(Self::Public),
            "dev" => Ok(Self::Development),
            _ => Err(ManifestError::new("invalid_platform_layout", value)),
        }
    }

    pub const fn paths(self) -> InstalledPaths {
        match self {
            Self::Public => PUBLIC_PATHS,
            Self::Development => DEVELOPMENT_PATHS,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationProfile {
    AgentStrict,
    GuiLegacy,
    ManagerLegacy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestError {
    code: &'static str,
    detail: String,
}

impl ManifestError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for ManifestError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedManifest {
    values: BTreeMap<String, String>,
}

impl ParsedManifest {
    pub fn get(&self, field: &str) -> Option<&str> {
        self.values.get(field).map(String::as_str)
    }

    pub fn required(&self, field: &str) -> Result<&str, ManifestError> {
        self.get(field)
            .ok_or_else(|| ManifestError::new("invalid_platform_manifest_fields", field))
    }

    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.values
    }

    pub fn into_values(self) -> BTreeMap<String, String> {
        self.values
    }

    pub fn serialize(&self) -> Result<String, ManifestError> {
        serialize(&self.values)
    }
}

pub fn parse(
    text: &str,
    layout: Layout,
    profile: ValidationProfile,
) -> Result<ParsedManifest, ManifestError> {
    let values = parse_fields(text)?;
    validate(&values, layout, profile)?;
    Ok(ParsedManifest { values })
}

pub fn serialize(values: &BTreeMap<String, String>) -> Result<String, ManifestError> {
    require_exact_fields(values)?;
    Ok(FIELDS
        .iter()
        .map(|field| format!("{field}={}\n", values[*field]))
        .collect())
}

pub fn qualification_candidate_id(values: &BTreeMap<String, String>) -> String {
    let mut hash = Sha256::new();
    for field in FIELDS {
        if *field == "qualification_candidate_id" {
            continue;
        }
        if let Some(value) = values.get(*field) {
            hash.update(field.as_bytes());
            hash.update(b"=");
            hash.update(value.as_bytes());
            hash.update(b"\n");
        }
    }
    encode_hex(&hash.finalize())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn parse_fields(text: &str) -> Result<BTreeMap<String, String>, ManifestError> {
    let mut values = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| {
            ManifestError::new(
                "invalid_platform_manifest",
                format!("malformed line {}", index + 1),
            )
        })?;
        if key.is_empty() || value.is_empty() || values.insert(key.into(), value.into()).is_some() {
            return Err(ManifestError::new(
                "invalid_platform_manifest",
                format!("duplicate or empty line {}", index + 1),
            ));
        }
    }
    Ok(values)
}

fn validate(
    values: &BTreeMap<String, String>,
    layout: Layout,
    profile: ValidationProfile,
) -> Result<(), ManifestError> {
    require_exact_fields(values)?;
    if values["format"] != FORMAT {
        return Err(ManifestError::new(
            "unsupported_platform_manifest",
            values["format"].clone(),
        ));
    }
    let release_number = values["platform_release_number"]
        .parse::<u64>()
        .map_err(|_| {
            ManifestError::new(
                "invalid_platform_release",
                values["platform_release_number"].clone(),
            )
        })?;
    if release_number == 0 || values["platform_release"] != format!("platform-v0.{release_number}")
    {
        return Err(ManifestError::new(
            "invalid_platform_release",
            values["platform_release"].clone(),
        ));
    }
    if values["latch_protocol_version"] != LATCH_PROTOCOL_VERSION
        || values["latch_capability_mask"] != LATCH_CAPABILITY_MASK
    {
        return Err(ManifestError::new(
            "unsupported_latch_protocol",
            format!(
                "version={} capabilities={}",
                values["latch_protocol_version"], values["latch_capability_mask"]
            ),
        ));
    }
    validate_identity_encoding(values)?;
    if matches!(
        profile,
        ValidationProfile::AgentStrict | ValidationProfile::ManagerLegacy
    ) {
        validate_layout_paths(values, layout)?;
    }
    if values["qualification_candidate_id"] != qualification_candidate_id(values) {
        return Err(ManifestError::new(
            "platform_candidate_identity_mismatch",
            values["qualification_candidate_id"].clone(),
        ));
    }
    Ok(())
}

fn validate_identity_encoding(values: &BTreeMap<String, String>) -> Result<(), ManifestError> {
    require_hex("platform_bundle_id", &values["platform_bundle_id"], 64)?;
    require_hex(
        "qualification_candidate_id",
        &values["qualification_candidate_id"],
        64,
    )?;
    for (name, _) in Layout::Public.paths().components() {
        let hash_field = format!("{name}_sha256");
        require_hex(&hash_field, &values[&hash_field], 64)?;
    }
    require_hex(
        "platform_contract_sha256",
        &values["platform_contract_sha256"],
        64,
    )?;
    for field in ["main_revision", "magik_revision", "menu_revision"] {
        require_hex(field, &values[field], 40)?;
    }
    Ok(())
}

fn validate_layout_paths(
    values: &BTreeMap<String, String>,
    layout: Layout,
) -> Result<(), ManifestError> {
    for (name, expected) in layout.paths().components() {
        let path_field = format!("{name}_path");
        if values[&path_field] != expected {
            return Err(ManifestError::new("platform_path_mismatch", name));
        }
    }
    Ok(())
}

fn require_exact_fields(values: &BTreeMap<String, String>) -> Result<(), ManifestError> {
    if values.len() == FIELDS.len() && FIELDS.iter().all(|field| values.contains_key(*field)) {
        Ok(())
    } else {
        Err(ManifestError::new(
            "invalid_platform_manifest_fields",
            values.keys().cloned().collect::<Vec<_>>().join(","),
        ))
    }
}

fn require_hex(name: &str, value: &str, length: usize) -> Result<(), ManifestError> {
    if value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(ManifestError::new(
            "invalid_platform_identity",
            format!("{name}: {value}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(layout: Layout) -> String {
        let mut values = BTreeMap::new();
        values.insert("format".to_string(), FORMAT.to_string());
        values.insert("platform_release".to_string(), "platform-v0.16".to_string());
        values.insert("platform_release_number".to_string(), "16".to_string());
        values.insert("platform_bundle_id".to_string(), "c".repeat(64));
        values.insert(
            "latch_protocol_version".to_string(),
            LATCH_PROTOCOL_VERSION.to_string(),
        );
        values.insert(
            "latch_capability_mask".to_string(),
            LATCH_CAPABILITY_MASK.to_string(),
        );
        for (name, path) in layout.paths().components() {
            values.insert(format!("{name}_path"), path.to_string());
            values.insert(format!("{name}_sha256"), "a".repeat(64));
        }
        values.insert("platform_contract_sha256".to_string(), "d".repeat(64));
        for field in ["main_revision", "magik_revision", "menu_revision"] {
            values.insert(field.to_string(), "b".repeat(40));
        }
        values.insert(
            "qualification_candidate_id".to_string(),
            qualification_candidate_id(&values),
        );
        serialize(&values).unwrap()
    }

    #[test]
    fn schema_generated_public_and_development_fixtures_are_stable() {
        let public = canonical(Layout::Public);
        let development = canonical(Layout::Development);
        assert!(public.contains("main_path=/media/fat/MiSTer_MagiK\n"));
        assert!(development.contains("main_path=/media/fat/MiSTer_MagiKDev\n"));
        assert_eq!(public.lines().count(), FIELDS.len());
        assert_eq!(development.lines().count(), FIELDS.len());
        assert_eq!(
            parse(&public, Layout::Public, ValidationProfile::AgentStrict)
                .unwrap()
                .serialize()
                .unwrap(),
            public
        );
    }

    #[test]
    fn checked_in_non_rust_fixtures_match_the_typed_contract() {
        for (text, layout) in [
            (
                include_str!("../../generated/platform-v3.public.fixture"),
                Layout::Public,
            ),
            (
                include_str!("../../generated/platform-v3.development.fixture"),
                Layout::Development,
            ),
        ] {
            assert_eq!(
                parse(text, layout, ValidationProfile::AgentStrict)
                    .unwrap()
                    .serialize()
                    .unwrap(),
                text
            );
        }
    }

    #[test]
    fn strict_profile_preserves_current_rejection_classes() {
        let valid = canonical(Layout::Development);
        let cases = [
            (
                valid.lines().skip(1).collect::<Vec<_>>().join("\n"),
                "invalid_platform_manifest_fields",
            ),
            (
                format!("{valid}format={FORMAT}\n"),
                "invalid_platform_manifest",
            ),
            (
                format!("{valid}unexpected=value\n"),
                "invalid_platform_manifest_fields",
            ),
            (
                valid.replace(
                    "/media/fat/mister-magik-dev/mister-magik-manager",
                    "/tmp/manager",
                ),
                "platform_path_mismatch",
            ),
            (
                valid.replacen(&"a".repeat(64), &"A".repeat(64), 1),
                "invalid_platform_identity",
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(
                parse(&text, Layout::Development, ValidationProfile::AgentStrict)
                    .unwrap_err()
                    .code(),
                expected
            );
        }
    }

    #[test]
    fn every_profile_rejects_noncanonical_identity_before_candidate_mismatch() {
        let valid = canonical(Layout::Development);
        let noncanonical = valid.replace(
            &format!("platform_bundle_id={}", "c".repeat(64)),
            &format!("platform_bundle_id={}", "C".repeat(64)),
        );
        for profile in [
            ValidationProfile::AgentStrict,
            ValidationProfile::GuiLegacy,
            ValidationProfile::ManagerLegacy,
        ] {
            assert_eq!(
                parse(&noncanonical, Layout::Development, profile)
                    .unwrap_err()
                    .code(),
                "invalid_platform_identity"
            );
        }
    }

    #[test]
    fn every_profile_rejects_forged_candidate_identity() {
        let valid = canonical(Layout::Development);
        let candidate = valid
            .lines()
            .find(|line| line.starts_with("qualification_candidate_id="))
            .unwrap();
        let forged = valid.replace(
            candidate,
            &format!("qualification_candidate_id={}", "f".repeat(64)),
        );
        assert!(parse(&forged, Layout::Development, ValidationProfile::AgentStrict).is_err());
        assert!(
            parse(
                &forged,
                Layout::Development,
                ValidationProfile::ManagerLegacy
            )
            .is_err()
        );
        assert!(parse(&forged, Layout::Development, ValidationProfile::GuiLegacy).is_err());
    }

    #[test]
    fn gui_profile_rejects_missing_and_additional_fields() {
        let valid = canonical(Layout::Development);
        let missing = valid
            .lines()
            .filter(|line| !line.starts_with("manager_sha256="))
            .collect::<Vec<_>>()
            .join("\n");
        let additional = format!("{valid}unexpected=value\n");
        for text in [missing, additional] {
            assert_eq!(
                parse(&text, Layout::Development, ValidationProfile::GuiLegacy)
                    .unwrap_err()
                    .code(),
                "invalid_platform_manifest_fields"
            );
        }
    }
}
