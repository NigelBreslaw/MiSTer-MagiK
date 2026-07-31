// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::archive::read_distribution_zip;
use crate::error::{AgentError, AgentResult};
use crate::platform_manifest::{self, Layout};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const ASSET_FORMAT: &str = "mister-magik-release-assets-v1";
const RELEASE_FORMAT: &str = "mister-magik-release-v1";

#[derive(Clone, Debug, Deserialize)]
struct AssetReceipt {
    format: String,
    version: String,
    build_number: u64,
    archive: String,
    archive_sha256: String,
    files: Vec<AssetFile>,
}

#[derive(Clone, Debug, Deserialize)]
struct AssetFile {
    path: String,
    asset: String,
    size: u64,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CandidateIdentity {
    pub format: &'static str,
    pub version: String,
    pub build_number: u64,
    pub archive: String,
    pub archive_sha256: String,
    pub release_assets_sha256: String,
    pub magik_revision: String,
    pub gui_sha256: String,
    pub platform_manifest_sha256: String,
    pub platform_bundle_id: String,
    pub qualification_candidate_id: String,
    pub component_sha256: BTreeMap<String, String>,
}

pub fn verify_candidate(root: &Path) -> AgentResult<CandidateIdentity> {
    let receipt_path = root.join("release-assets.json");
    let receipt_bytes = fs::read(&receipt_path)
        .map_err(|error| format!("cannot read {}: {error}", receipt_path.display()))?;
    let receipt: AssetReceipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|error| format!("cannot parse {}: {error}", receipt_path.display()))?;
    if receipt.format != ASSET_FORMAT || receipt.version != format!("0.2.{}", receipt.build_number)
    {
        return classified(
            "invalid_alpha_candidate",
            "release identity is inconsistent",
        );
    }
    require_leaf("archive", &receipt.archive)?;
    require_sha("archive_sha256", &receipt.archive_sha256)?;
    verify_checksums(root)?;

    let archive_path = root.join(&receipt.archive);
    if digest_file(&archive_path)? != receipt.archive_sha256 {
        return classified("alpha_candidate_hash_mismatch", receipt.archive);
    }
    let archive = read_distribution_zip(&archive_path)?;
    let mut expected = BTreeSet::new();
    for entry in &receipt.files {
        require_relative(&entry.path)?;
        require_leaf("asset", &entry.asset)?;
        require_sha("asset_sha256", &entry.sha256)?;
        if !expected.insert(entry.path.clone()) {
            return classified(
                "invalid_alpha_candidate",
                format!("duplicate {}", entry.path),
            );
        }
        let bytes = archive
            .get(&entry.path)
            .ok_or_else(|| AgentError::Classified {
                code: "alpha_candidate_archive_mismatch",
                detail: format!("archive is missing {}", entry.path),
            })?;
        if bytes.len() as u64 != entry.size || digest(bytes) != entry.sha256 {
            return classified("alpha_candidate_hash_mismatch", entry.path.clone());
        }
        let asset_path = if root.join("files").is_dir() {
            root.join("files").join(&entry.asset)
        } else {
            root.join(&entry.asset)
        };
        if fs::metadata(&asset_path).map(|value| value.len()).ok() != Some(entry.size)
            || digest_file(&asset_path)? != entry.sha256
        {
            return classified("alpha_candidate_asset_mismatch", entry.asset.clone());
        }
    }
    if archive.keys().cloned().collect::<BTreeSet<_>>() != expected {
        return classified(
            "alpha_candidate_archive_mismatch",
            "archive and release receipt contain different files",
        );
    }

    let release = parse_fields(member(&archive, "mister-magik/release-v1.txt")?)?;
    if release.get("format").map(String::as_str) != Some(RELEASE_FORMAT)
        || release.get("version") != Some(&receipt.version)
        || release.get("build_number") != Some(&receipt.build_number.to_string())
    {
        return classified("invalid_alpha_candidate", "release-v1 identity disagrees");
    }
    let manifest_bytes = member(&archive, "mister-magik/platform-v3.manifest")?;
    let manifest_text = std::str::from_utf8(manifest_bytes)
        .map_err(|error| format!("platform manifest is not UTF-8: {error}"))?;
    let manifest = platform_manifest::parse_installed(manifest_text, Layout::Public)?;
    let gui = member(&archive, "mister-magik/mister-magik-fb")?;
    if digest(gui) != manifest.gui_sha256()
        || release.get("magik_revision").map(String::as_str) != Some(manifest.magik_revision())
    {
        return classified(
            "alpha_candidate_identity_mismatch",
            "runtime and platform manifest disagree",
        );
    }

    Ok(CandidateIdentity {
        format: "mister-magik-alpha-candidate-v1",
        version: receipt.version,
        build_number: receipt.build_number,
        archive: receipt.archive,
        archive_sha256: receipt.archive_sha256,
        release_assets_sha256: digest(&receipt_bytes),
        magik_revision: manifest.magik_revision().to_owned(),
        gui_sha256: manifest.gui_sha256().to_owned(),
        platform_manifest_sha256: digest(manifest_bytes),
        platform_bundle_id: manifest.platform_bundle_id().to_owned(),
        qualification_candidate_id: manifest.qualification_candidate_id().to_owned(),
        component_sha256: BTreeMap::from([
            ("main".into(), manifest.main_sha256().into()),
            ("gui".into(), manifest.gui_sha256().into()),
            ("manager".into(), manifest.manager_sha256().into()),
            (
                "scanout_module".into(),
                manifest.scanout_module_sha256().into(),
            ),
            (
                "scanout_metadata".into(),
                manifest.scanout_metadata_sha256().into(),
            ),
            ("latch_rbf".into(), manifest.latch_rbf_sha256().into()),
            (
                "latch_metadata".into(),
                manifest.latch_metadata_sha256().into(),
            ),
        ]),
    })
}

fn verify_checksums(root: &Path) -> AgentResult<()> {
    let path = root.join("SHA256SUMS");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let (sha, relative) = line
            .split_once("  ")
            .ok_or_else(|| "invalid SHA256SUMS line".to_owned())?;
        require_sha("checksum", sha)?;
        require_relative(relative)?;
        if !seen.insert(relative) || digest_file(&root.join(relative))? != sha {
            return classified("alpha_candidate_checksum_mismatch", relative);
        }
    }
    if !seen.contains("release-assets.json") {
        return classified(
            "invalid_alpha_candidate",
            "SHA256SUMS does not cover release-assets.json",
        );
    }
    Ok(())
}

fn parse_fields(bytes: &[u8]) -> AgentResult<BTreeMap<String, String>> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| "invalid release-v1 field".to_owned())?;
        if key.is_empty() || value.is_empty() || fields.insert(key.into(), value.into()).is_some() {
            return classified("invalid_alpha_candidate", "invalid release-v1 fields");
        }
    }
    Ok(fields)
}

fn member<'a>(archive: &'a BTreeMap<String, Vec<u8>>, path: &str) -> AgentResult<&'a [u8]> {
    archive
        .get(path)
        .map(Vec::as_slice)
        .ok_or_else(|| AgentError::Classified {
            code: "invalid_alpha_candidate",
            detail: format!("missing {path}"),
        })
}

fn require_leaf(field: &'static str, value: &str) -> AgentResult<()> {
    if value.is_empty() || Path::new(value).components().count() != 1 {
        classified(
            "invalid_alpha_candidate",
            format!("unsafe {field}: {value}"),
        )
    } else {
        Ok(())
    }
}

fn require_relative(value: &str) -> AgentResult<()> {
    let path = PathBuf::from(value);
    if value.is_empty()
        || value.starts_with('/')
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        classified("invalid_alpha_candidate", format!("unsafe path: {value}"))
    } else {
        Ok(())
    }
}

fn require_sha(field: &'static str, value: &str) -> AgentResult<()> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        classified("invalid_alpha_candidate", format!("invalid {field}"))
    }
}

fn digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    digest.iter().fold(
        String::with_capacity(digest.len() * 2),
        |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        },
    )
}

fn digest_file(path: &Path) -> AgentResult<String> {
    fs::read(path)
        .map(|bytes| digest(&bytes))
        .map_err(|error| format!("cannot read {}: {error}", path.display()).into())
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

    #[test]
    fn release_fields_reject_duplicates() {
        assert!(parse_fields(b"format=x\nformat=y\n").is_err());
    }

    #[test]
    fn candidate_paths_are_bounded() {
        assert!(require_relative("mister-magik/release-v1.txt").is_ok());
        assert!(require_relative("../release-v1.txt").is_err());
        assert!(require_leaf("archive", "candidate.zip").is_ok());
        assert!(require_leaf("archive", "nested/candidate.zip").is_err());
    }
}
