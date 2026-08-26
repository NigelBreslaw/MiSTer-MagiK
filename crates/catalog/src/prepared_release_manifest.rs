// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checked-in release knowledge for prepared collections.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const ZERO_MHZ_SCHEMA: &str = "mister-magik-0mhz-release-manifest-v1";
const ZERO_MHZ_BYTES: &[u8] = include_bytes!("../data/prepared/0mhz-v004.json");

#[derive(Debug, Deserialize)]
struct ZeroMhzManifest {
    schema: String,
    release_id: String,
    packages: Vec<ZeroMhzPackage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ZeroMhzPackage {
    pub(crate) title: String,
    launcher_path: String,
    pub(crate) launcher_bytes: u64,
    launcher_sha256: String,
    pub(crate) payloads: Vec<ZeroMhzPayload>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ZeroMhzPayload {
    pub(crate) relative_path: String,
}

struct ZeroMhzIndex {
    release_id: String,
    packages: Vec<ZeroMhzPackage>,
    by_launcher: HashMap<String, usize>,
}

pub(crate) struct KnownZeroMhzLaunch {
    pub(crate) storage_root: PathBuf,
    pub(crate) package: &'static ZeroMhzPackage,
}

fn zero_mhz_index() -> Option<&'static ZeroMhzIndex> {
    static INDEX: OnceLock<Option<ZeroMhzIndex>> = OnceLock::new();
    INDEX
        .get_or_init(|| {
            let manifest: ZeroMhzManifest = serde_json::from_slice(ZERO_MHZ_BYTES).ok()?;
            if manifest.schema != ZERO_MHZ_SCHEMA || manifest.release_id.trim().is_empty() {
                return None;
            }
            let by_launcher = manifest
                .packages
                .iter()
                .enumerate()
                .map(|(index, package)| (package.launcher_path.to_ascii_lowercase(), index))
                .collect::<HashMap<_, _>>();
            (by_launcher.len() == manifest.packages.len()).then_some(ZeroMhzIndex {
                release_id: manifest.release_id,
                packages: manifest.packages,
                by_launcher,
            })
        })
        .as_ref()
}

pub(crate) fn known_0mhz_launch(path: &Path) -> Option<KnownZeroMhzLaunch> {
    let (storage_root, relative_path) = relative_to_named_ancestor(path, "_DOS Games")?;
    let index = zero_mhz_index()?;
    let package_index = *index.by_launcher.get(&relative_path.to_ascii_lowercase())?;
    Some(KnownZeroMhzLaunch {
        storage_root,
        package: &index.packages[package_index],
    })
}

pub(crate) fn launcher_matches(package: &ZeroMhzPackage, path: &Path, size: u64) -> bool {
    if size != package.launcher_bytes {
        return false;
    }
    std::fs::read(path).ok().is_some_and(|bytes| {
        let digest = Sha256::digest(bytes);
        let mut actual = String::with_capacity(64);
        for byte in digest {
            let _ = write!(actual, "{byte:02x}");
        }
        actual == package.launcher_sha256
    })
}

pub(crate) fn zero_mhz_release_id() -> Option<&'static str> {
    zero_mhz_index().map(|index| index.release_id.as_str())
}

fn relative_to_named_ancestor(path: &Path, name: &str) -> Option<(PathBuf, String)> {
    let ancestor = path.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(name))
    })?;
    let storage_root = ancestor.parent()?.to_path_buf();
    let relative = path
        .strip_prefix(&storage_root)
        .ok()?
        .to_str()?
        .replace('\\', "/");
    Some((storage_root, relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_0mhz_manifest_is_complete_and_addressable() {
        let index = zero_mhz_index().unwrap();
        assert_eq!(index.release_id, "internet-archive-0mhz-dos-v0.04");
        assert_eq!(index.packages.len(), 319);
        assert_eq!(index.by_launcher.len(), 319);
        assert!(index.packages.iter().all(|package| {
            package.launcher_bytes > 0
                && !package.title.is_empty()
                && package.launcher_sha256.len() == 64
                && !package.payloads.is_empty()
                && package
                    .payloads
                    .iter()
                    .all(|payload| !payload.relative_path.is_empty())
        }));
    }

    #[test]
    fn launcher_lookup_recovers_storage_root() {
        let launch = known_0mhz_launch(Path::new(
            "/media/fat/_DOS Games/4D Sports Driving (MT-32).mgl",
        ))
        .unwrap();
        assert_eq!(launch.storage_root, Path::new("/media/fat"));
        assert_eq!(launch.package.title, "4D Sports Driving (MT-32)");
    }
}
