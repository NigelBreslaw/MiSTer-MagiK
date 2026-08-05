// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::{AgentError, AgentResult};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "mister-magik-platform-v3";
pub const FILE_NAME: &str = "platform-v3.manifest";
pub const LATCH_PROTOCOL_VERSION: &str = "5";
pub const LATCH_CAPABILITY_MASK: &str = "0x03ff";
pub(crate) const FIELDS: &[&str] = &[
    "format",
    "platform_release",
    "platform_release_number",
    "platform_bundle_id",
    "qualification_candidate_id",
    "latch_protocol_version",
    "latch_capability_mask",
    "main_path",
    "gui_path",
    "manager_path",
    "scanout_module_path",
    "scanout_metadata_path",
    "latch_rbf_path",
    "latch_metadata_path",
    "main_sha256",
    "gui_sha256",
    "manager_sha256",
    "scanout_module_sha256",
    "scanout_metadata_sha256",
    "latch_rbf_sha256",
    "latch_metadata_sha256",
    "platform_contract_sha256",
    "main_revision",
    "magik_revision",
    "menu_revision",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    Public,
    Development,
}

impl Layout {
    pub fn parse(value: &str) -> AgentResult<Self> {
        match value {
            "public" => Ok(Self::Public),
            "dev" => Ok(Self::Development),
            _ => Err(AgentError::Classified {
                code: "invalid_platform_layout",
                detail: value.into(),
            }),
        }
    }

    pub(crate) fn paths(self) -> [(&'static str, &'static str); 7] {
        let main = match self {
            Self::Public => "/media/fat/MiSTer_MagiK",
            Self::Development => "/media/fat/MiSTer_MagiKDev",
        };
        [
            ("main", main),
            (
                "gui",
                if self == Self::Public {
                    "/media/fat/mister-magik/mister-magik-fb"
                } else {
                    "/media/fat/mister-magik-dev/mister-magik-fb"
                },
            ),
            (
                "manager",
                if self == Self::Public {
                    "/media/fat/mister-magik/mister-magik-manager"
                } else {
                    "/media/fat/mister-magik-dev/mister-magik-manager"
                },
            ),
            (
                "scanout_module",
                if self == Self::Public {
                    "/media/fat/mister-magik/mister_magik_scanout_slots.ko"
                } else {
                    "/media/fat/mister-magik-dev/mister_magik_scanout_slots.ko"
                },
            ),
            (
                "scanout_metadata",
                if self == Self::Public {
                    "/media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt"
                } else {
                    "/media/fat/mister-magik-dev/mister_magik_scanout_slots.metadata.txt"
                },
            ),
            (
                "latch_rbf",
                if self == Self::Public {
                    "/media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf"
                } else {
                    "/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf"
                },
            ),
            (
                "latch_metadata",
                if self == Self::Public {
                    "/media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt"
                } else {
                    "/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.metadata.txt"
                },
            ),
        ]
    }
}

pub struct Artifacts {
    pub main: PathBuf,
    pub gui: PathBuf,
    pub manager: PathBuf,
    pub scanout_module: PathBuf,
    pub scanout_metadata: PathBuf,
    pub latch_rbf: PathBuf,
    pub latch_metadata: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseIdentity {
    pub release_number: u64,
    pub bundle_id: String,
}

#[derive(Deserialize)]
struct BundleManifest {
    format: String,
    release_version: u64,
    bundle_id: String,
}

impl ReleaseIdentity {
    pub fn from_bundle_manifest(path: &Path) -> AgentResult<Self> {
        let bytes =
            fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let manifest: BundleManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
        if manifest.format != crate::platform_bundle::FORMAT {
            return classified("unsupported_platform_bundle", manifest.format);
        }
        if manifest.release_version == 0 {
            return classified("invalid_platform_release", "release number is zero");
        }
        require_hex("platform_bundle_id", &manifest.bundle_id, 64)?;
        Ok(Self {
            release_number: manifest.release_version,
            bundle_id: manifest.bundle_id,
        })
    }

    #[must_use]
    pub fn release_tag(&self) -> String {
        format!("platform-v0.{}", self.release_number)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledManifest {
    values: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalMainIdentity {
    pub main_sha256: String,
    pub gui_sha256: String,
    pub qualification_candidate_id: String,
}

impl InstalledManifest {
    #[must_use]
    pub fn qualification_candidate_id(&self) -> &str {
        &self.values["qualification_candidate_id"]
    }

    #[must_use]
    pub fn platform_bundle_id(&self) -> &str {
        &self.values["platform_bundle_id"]
    }

    #[must_use]
    pub fn main_sha256(&self) -> &str {
        &self.values["main_sha256"]
    }

    #[must_use]
    pub fn main_revision(&self) -> &str {
        &self.values["main_revision"]
    }

    #[must_use]
    pub fn magik_revision(&self) -> &str {
        &self.values["magik_revision"]
    }

    #[must_use]
    pub fn gui_sha256(&self) -> &str {
        &self.values["gui_sha256"]
    }

    #[must_use]
    pub fn manager_sha256(&self) -> &str {
        &self.values["manager_sha256"]
    }

    #[must_use]
    pub fn scanout_module_sha256(&self) -> &str {
        &self.values["scanout_module_sha256"]
    }

    #[must_use]
    pub fn scanout_metadata_sha256(&self) -> &str {
        &self.values["scanout_metadata_sha256"]
    }

    #[must_use]
    pub fn latch_rbf_sha256(&self) -> &str {
        &self.values["latch_rbf_sha256"]
    }

    #[must_use]
    pub fn latch_metadata_sha256(&self) -> &str {
        &self.values["latch_metadata_sha256"]
    }
}

pub fn parse_installed(text: &str, layout: Layout) -> AgentResult<InstalledManifest> {
    let values = parse_text_fields(text, Some(FIELDS), "installed platform manifest")?;
    validate_manifest_fields(&values, layout)?;
    Ok(InstalledManifest { values })
}

pub fn write_local_main_overlay(
    output: &Path,
    installed_text: &str,
    main: &Path,
    main_revision: &str,
    expected_magik_revision: &str,
) -> AgentResult<LocalMainIdentity> {
    require_hex("main_revision", main_revision, 40)?;
    require_hex("magik_revision", expected_magik_revision, 40)?;
    if !main.is_file() {
        return classified(
            "platform_artifact_missing",
            format!("main: {}", main.display()),
        );
    }
    let installed = parse_installed(installed_text, Layout::Development)?;
    if installed.magik_revision() != expected_magik_revision {
        return classified(
            "installed_magik_revision_mismatch",
            format!(
                "expected={expected_magik_revision} installed={}",
                installed.magik_revision()
            ),
        );
    }

    let mut values = installed.values;
    let main_sha256 = digest(main)?;
    values.insert("main_sha256".into(), main_sha256.clone());
    values.insert("main_revision".into(), main_revision.into());
    values.insert(
        "qualification_candidate_id".into(),
        qualification_candidate_id(&values),
    );
    let text = FIELDS
        .iter()
        .map(|field| format!("{field}={}\n", values[*field]))
        .collect::<String>();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(output, text)
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    verify(output, None, Layout::Development)?;
    Ok(LocalMainIdentity {
        main_sha256,
        gui_sha256: values["gui_sha256"].clone(),
        qualification_candidate_id: values["qualification_candidate_id"].clone(),
    })
}

pub fn update_runtime(
    output: &Path,
    installed: &str,
    gui: &Path,
    magik_revision: &str,
) -> AgentResult<()> {
    require_hex("magik_revision", magik_revision, 40)?;
    if !gui.is_file() {
        return classified(
            "platform_artifact_missing",
            format!("gui: {}", gui.display()),
        );
    }
    let mut values = parse_installed(installed, Layout::Development)?.values;
    values.insert("gui_sha256".into(), digest(gui)?);
    values.insert("magik_revision".into(), magik_revision.into());
    values.insert(
        "qualification_candidate_id".into(),
        qualification_candidate_id(&values),
    );
    let text = FIELDS
        .iter()
        .map(|field| format!("{field}={}\n", values[*field]))
        .collect::<String>();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(output, text)
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    verify(output, None, Layout::Development)
}

impl Artifacts {
    fn values(&self) -> [(&'static str, &Path); 7] {
        [
            ("main", &self.main),
            ("gui", &self.gui),
            ("manager", &self.manager),
            ("scanout_module", &self.scanout_module),
            ("scanout_metadata", &self.scanout_metadata),
            ("latch_rbf", &self.latch_rbf),
            ("latch_metadata", &self.latch_metadata),
        ]
    }
}

pub fn generate(
    output: &Path,
    artifacts: &Artifacts,
    release: &ReleaseIdentity,
    main_revision: &str,
    magik_revision: &str,
    layout: Layout,
) -> AgentResult<()> {
    require_hex("main_revision", main_revision, 40)?;
    require_hex("magik_revision", magik_revision, 40)?;
    for (name, path) in artifacts.values() {
        if !path.is_file() {
            return classified(
                "platform_artifact_missing",
                format!("{name}: {}", path.display()),
            );
        }
    }
    let (contract, menu_revision) = validate_metadata(artifacts)?;
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    values.insert("format".into(), FORMAT.to_owned());
    values.insert("platform_release".into(), release.release_tag());
    values.insert(
        "platform_release_number".into(),
        release.release_number.to_string(),
    );
    values.insert("platform_bundle_id".into(), release.bundle_id.clone());
    values.insert(
        "latch_protocol_version".into(),
        LATCH_PROTOCOL_VERSION.into(),
    );
    values.insert("latch_capability_mask".into(), LATCH_CAPABILITY_MASK.into());
    for (name, path) in layout.paths() {
        values.insert(format!("{name}_path"), path.to_owned());
    }
    for (name, path) in artifacts.values() {
        values.insert(format!("{name}_sha256"), digest(path)?);
    }
    values.insert("platform_contract_sha256".into(), contract);
    values.insert("main_revision".into(), main_revision.into());
    values.insert("magik_revision".into(), magik_revision.into());
    values.insert("menu_revision".into(), menu_revision);
    values.insert(
        "qualification_candidate_id".into(),
        qualification_candidate_id(&values),
    );
    let text = FIELDS
        .iter()
        .map(|field| format!("{field}={}\n", values[*field]))
        .collect::<String>();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(output, text)
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    verify(output, None, layout)
}

pub fn verify(manifest: &Path, artifact_root: Option<&Path>, layout: Layout) -> AgentResult<()> {
    let fields = parse_fields(manifest, Some(FIELDS))?;
    validate_manifest_fields(&fields, layout)?;
    let Some(root) = artifact_root else {
        return Ok(());
    };
    verify_artifacts(&fields, root, layout)
}

fn validate_manifest_fields(fields: &BTreeMap<String, String>, layout: Layout) -> AgentResult<()> {
    if fields["format"] != FORMAT {
        return classified("unsupported_platform_manifest", fields["format"].clone());
    }
    let release_number = fields["platform_release_number"]
        .parse::<u64>()
        .map_err(|_| AgentError::Classified {
            code: "invalid_platform_release",
            detail: fields["platform_release_number"].clone(),
        })?;
    if release_number == 0 || fields["platform_release"] != format!("platform-v0.{release_number}")
    {
        return classified(
            "invalid_platform_release",
            fields["platform_release"].clone(),
        );
    }
    require_hex("platform_bundle_id", &fields["platform_bundle_id"], 64)?;
    require_hex(
        "qualification_candidate_id",
        &fields["qualification_candidate_id"],
        64,
    )?;
    if fields["latch_protocol_version"] != LATCH_PROTOCOL_VERSION
        || fields["latch_capability_mask"] != LATCH_CAPABILITY_MASK
    {
        return classified(
            "unsupported_latch_protocol",
            format!(
                "version={} capabilities={}",
                fields["latch_protocol_version"], fields["latch_capability_mask"]
            ),
        );
    }
    for (name, expected) in layout.paths() {
        if fields[&format!("{name}_path")] != expected {
            return classified("platform_path_mismatch", name);
        }
        require_hex(
            &format!("{name}_sha256"),
            &fields[&format!("{name}_sha256")],
            64,
        )?;
    }
    require_hex(
        "platform_contract_sha256",
        &fields["platform_contract_sha256"],
        64,
    )?;
    for name in ["main_revision", "magik_revision", "menu_revision"] {
        require_hex(name, &fields[name], 40)?;
    }
    if fields["qualification_candidate_id"] != qualification_candidate_id(fields) {
        return classified(
            "platform_candidate_identity_mismatch",
            fields["qualification_candidate_id"].clone(),
        );
    }
    Ok(())
}

fn verify_artifacts(
    fields: &BTreeMap<String, String>,
    root: &Path,
    layout: Layout,
) -> AgentResult<()> {
    let paths = layout.paths();
    let artifact = |name: &str| -> AgentResult<PathBuf> {
        let device = paths.iter().find(|(field, _)| *field == name).unwrap().1;
        let relative = device
            .strip_prefix("/media/fat/")
            .ok_or("platform path is outside /media/fat")?;
        Ok(root.join(relative))
    };
    let artifacts = Artifacts {
        main: artifact("main")?,
        gui: artifact("gui")?,
        manager: artifact("manager")?,
        scanout_module: artifact("scanout_module")?,
        scanout_metadata: artifact("scanout_metadata")?,
        latch_rbf: artifact("latch_rbf")?,
        latch_metadata: artifact("latch_metadata")?,
    };
    for (name, path) in artifacts.values() {
        if digest(path)? != fields[&format!("{name}_sha256")] {
            return classified("installed_artifact_mismatch", name);
        }
    }
    let (contract, menu) = validate_metadata(&artifacts)?;
    if contract != fields["platform_contract_sha256"] || menu != fields["menu_revision"] {
        return classified(
            "platform_metadata_mismatch",
            "manifest does not match component metadata",
        );
    }
    Ok(())
}

fn validate_metadata(artifacts: &Artifacts) -> AgentResult<(String, String)> {
    let module = parse_fields(&artifacts.scanout_metadata, None)?;
    let latch = parse_fields(&artifacts.latch_metadata, None)?;
    if module.get("module_sha256") != Some(&digest(&artifacts.scanout_module)?) {
        return classified("scanout_metadata_mismatch", "module hash");
    }
    if latch.get("rbf_sha256") != Some(&digest(&artifacts.latch_rbf)?) {
        return classified("latch_metadata_mismatch", "RBF hash");
    }
    let contract = module
        .get("platform_contract_sha256")
        .ok_or("scanout metadata has no platform contract")?;
    if latch.get("platform_contract_sha256") != Some(contract) {
        return classified(
            "platform_contract_mismatch",
            "component metadata uses mixed contracts",
        );
    }
    if latch.get("latch_protocol_version").map(String::as_str) != Some(LATCH_PROTOCOL_VERSION)
        || latch.get("latch_capability_mask").map(String::as_str) != Some(LATCH_CAPABILITY_MASK)
    {
        return classified(
            "unsupported_latch_protocol",
            format!(
                "version={} capabilities={}",
                latch
                    .get("latch_protocol_version")
                    .map(String::as_str)
                    .unwrap_or("missing"),
                latch
                    .get("latch_capability_mask")
                    .map(String::as_str)
                    .unwrap_or("missing")
            ),
        );
    }
    require_hex("platform_contract_sha256", contract, 64)?;
    let menu = latch
        .get("source_commit")
        .ok_or("latch metadata has no source commit")?;
    require_hex("menu_revision", menu, 40)?;
    Ok((contract.clone(), menu.clone()))
}

pub(crate) fn qualification_candidate_id(values: &BTreeMap<String, String>) -> String {
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
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn parse_fields(path: &Path, exact: Option<&[&str]>) -> AgentResult<BTreeMap<String, String>> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse_text_fields(&text, exact, &path.display().to_string())
}

fn parse_text_fields(
    text: &str,
    exact: Option<&[&str]>,
    label: &str,
) -> AgentResult<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("{label}:{}: malformed field", index + 1))?;
        if key.is_empty() || value.is_empty() || fields.insert(key.into(), value.into()).is_some() {
            return classified(
                "invalid_platform_manifest",
                format!("{label}:{}", index + 1),
            );
        }
    }
    if let Some(exact) = exact {
        let actual: Vec<_> = fields.keys().map(String::as_str).collect();
        if !exact.iter().all(|field| fields.contains_key(*field)) || actual.len() != exact.len() {
            return classified("invalid_platform_manifest_fields", actual.join(","));
        }
    }
    Ok(fields)
}

fn digest(path: &Path) -> AgentResult<String> {
    let mut file =
        File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn require_hex(name: &str, value: &str, length: usize) -> AgentResult<()> {
    if value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        classified("invalid_platform_identity", format!("{name}: {value}"))
    }
}

fn classified<T>(code: &'static str, detail: impl Into<String>) -> AgentResult<T> {
    Err(AgentError::Classified {
        code,
        detail: detail.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical_manifest() -> String {
        let mut values = BTreeMap::new();
        values.insert("format".to_owned(), FORMAT.to_owned());
        values.insert("platform_release".to_owned(), "platform-v0.16".to_owned());
        values.insert("platform_release_number".to_owned(), "16".to_owned());
        values.insert("platform_bundle_id".to_owned(), "c".repeat(64));
        values.insert(
            "latch_protocol_version".to_owned(),
            LATCH_PROTOCOL_VERSION.to_owned(),
        );
        values.insert(
            "latch_capability_mask".to_owned(),
            LATCH_CAPABILITY_MASK.to_owned(),
        );
        for (name, path) in Layout::Development.paths() {
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
            values.insert(format!("{name}_sha256"), "a".repeat(64));
        }
        for name in ["main_revision", "magik_revision", "menu_revision"] {
            values.insert(name.to_owned(), "b".repeat(40));
        }
        values.insert(
            "qualification_candidate_id".to_owned(),
            qualification_candidate_id(&values),
        );
        FIELDS
            .iter()
            .map(|field| format!("{field}={}\n", values[*field]))
            .collect()
    }

    #[test]
    fn layouts_are_closed_and_use_canonical_paths() {
        assert_eq!(Layout::parse("public").unwrap(), Layout::Public);
        assert_eq!(Layout::parse("dev").unwrap(), Layout::Development);
        assert!(Layout::parse("custom").is_err());
        assert_eq!(Layout::Public.paths()[0].1, "/media/fat/MiSTer_MagiK");
    }

    #[test]
    fn identities_require_lowercase_hex() {
        assert!(require_hex("sha", &"a".repeat(40), 40).is_ok());
        assert!(require_hex("sha", &"A".repeat(40), 40).is_err());
        assert!(require_hex("sha", "abc", 40).is_err());
    }

    #[test]
    fn installed_manifest_requires_exact_unique_canonical_fields() {
        let valid = canonical_manifest();
        assert!(parse_installed(&valid, Layout::Development).is_ok());
        for invalid in [
            valid.lines().skip(1).collect::<Vec<_>>().join("\n"),
            format!("{valid}format={FORMAT}\n"),
            format!("{valid}unexpected=value\n"),
            valid.replace(
                "/media/fat/mister-magik-dev/mister-magik-manager",
                "/tmp/manager",
            ),
            valid.replacen(&"a".repeat(64), &"A".repeat(64), 1),
        ] {
            assert!(parse_installed(&invalid, Layout::Development).is_err());
        }
    }

    #[test]
    fn changing_any_component_invalidates_candidate_identity() {
        let valid = canonical_manifest();
        let invalid = valid.replace(
            &format!("gui_sha256={}", "a".repeat(64)),
            &format!("gui_sha256={}", "d".repeat(64)),
        );
        assert!(parse_installed(&invalid, Layout::Development).is_err());
    }

    #[test]
    fn local_main_overlay_preserves_the_verified_dev_platform() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-local-main-overlay-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let main = root.join("MiSTer");
        fs::write(&main, b"local main").unwrap();
        let output = root.join(FILE_NAME);
        let revision = "d".repeat(40);
        let identity = write_local_main_overlay(
            &output,
            &canonical_manifest(),
            &main,
            &revision,
            &"b".repeat(40),
        )
        .unwrap();
        let overlay =
            parse_installed(&fs::read_to_string(&output).unwrap(), Layout::Development).unwrap();
        assert_eq!(overlay.main_revision(), revision);
        assert_eq!(overlay.main_sha256(), identity.main_sha256);
        assert_eq!(overlay.gui_sha256(), "a".repeat(64));
        assert_eq!(
            overlay.qualification_candidate_id(),
            identity.qualification_candidate_id
        );
        assert_eq!(overlay.manager_sha256(), "a".repeat(64));
        assert_eq!(overlay.latch_rbf_sha256(), "a".repeat(64));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn local_main_overlay_rejects_a_different_installed_app_commit() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-local-main-overlay-reject-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let main = root.join("MiSTer");
        fs::write(&main, b"local main").unwrap();
        assert!(
            write_local_main_overlay(
                &root.join(FILE_NAME),
                &canonical_manifest(),
                &main,
                &"d".repeat(40),
                &"e".repeat(40),
            )
            .is_err()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_update_changes_only_gui_identity_and_magik_revision() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-runtime-overlay-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let gui = root.join("mister-magik-fb");
        let output = root.join(FILE_NAME);
        fs::write(&gui, b"new gui").unwrap();

        update_runtime(&output, &canonical_manifest(), &gui, &"d".repeat(40)).unwrap();

        let updated =
            parse_installed(&fs::read_to_string(&output).unwrap(), Layout::Development).unwrap();
        assert_eq!(updated.magik_revision(), "d".repeat(40));
        assert_eq!(updated.gui_sha256(), digest(&gui).unwrap());
        assert_eq!(updated.main_revision(), "b".repeat(40));
        assert_eq!(updated.main_sha256(), "a".repeat(64));
        let _ = fs::remove_dir_all(root);
    }
}
