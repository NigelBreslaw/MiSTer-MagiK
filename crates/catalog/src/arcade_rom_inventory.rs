// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Filename-only Arcade ROM presence used by startup and authoritative scans.

use crate::mra_header::{PrimaryRomRequirement, RomNamespace};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArcadeRomInventory {
    mame: BTreeSet<String>,
    hbmame: BTreeSet<String>,
    fingerprint: String,
    pub(crate) scan_us: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RomEligibility {
    Eligible,
    Missing,
    Ambiguous,
}

impl ArcadeRomInventory {
    pub(crate) fn from_library_roots(roots: &[String]) -> Self {
        let directories = crate::prepared_collections::storage_roots_for_library_roots(roots)
            .into_iter()
            .flat_map(|root| {
                [
                    (RomNamespace::Mame, root.join("games/mame")),
                    (RomNamespace::Hbmame, root.join("games/hbmame")),
                    (RomNamespace::Mame, root.join("_Arcade/mame")),
                    (RomNamespace::Hbmame, root.join("_Arcade/hbmame")),
                ]
            })
            .collect::<Vec<_>>();
        Self::from_directories(&directories)
    }

    fn from_directories(directories: &[(RomNamespace, PathBuf)]) -> Self {
        let started = Instant::now();
        let mut mame = BTreeSet::new();
        let mut hbmame = BTreeSet::new();
        for (namespace, directory) in directories {
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            let names = match namespace {
                RomNamespace::Mame => &mut mame,
                RomNamespace::Hbmame => &mut hbmame,
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
                    && let Some(name) = path.file_name().and_then(|name| name.to_str())
                {
                    names.insert(name.to_ascii_lowercase());
                }
            }
        }
        let fingerprint = inventory_fingerprint(&mame, &hbmame);
        Self {
            mame,
            hbmame,
            fingerprint,
            scan_us: started.elapsed().as_micros() as u64,
        }
    }

    pub(crate) fn eligibility(&self, requirement: &PrimaryRomRequirement) -> RomEligibility {
        match requirement {
            PrimaryRomRequirement::None => RomEligibility::Eligible,
            PrimaryRomRequirement::Ambiguous => RomEligibility::Ambiguous,
            PrimaryRomRequirement::Archive { namespace, setname } => {
                let filename = format!("{}.zip", setname.to_ascii_lowercase());
                let present = match namespace {
                    RomNamespace::Mame => self.mame.contains(&filename),
                    RomNamespace::Hbmame => self.hbmame.contains(&filename),
                };
                if present {
                    RomEligibility::Eligible
                } else {
                    RomEligibility::Missing
                }
            }
        }
    }

    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn counts(&self) -> (usize, usize) {
        (self.mame.len(), self.hbmame.len())
    }
}

fn inventory_fingerprint(mame: &BTreeSet<String>, hbmame: &BTreeSet<String>) -> String {
    let mut hash = Sha256::new();
    for (namespace, names) in [("mame", mame), ("hbmame", hbmame)] {
        for name in names {
            hash.update(namespace.as_bytes());
            hash.update([0]);
            hash.update(name.as_bytes());
            hash.update([0xff]);
        }
    }
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_is_case_insensitive_and_namespace_specific() {
        let root = crate::test_support::unique_temp_dir("arcade-rom-inventory");
        let mame = root.join("mame");
        let hbmame = root.join("hbmame");
        std::fs::create_dir_all(&mame).unwrap();
        std::fs::create_dir_all(&hbmame).unwrap();
        std::fs::write(mame.join("PuckMan.ZIP"), b"not inspected").unwrap();
        std::fs::write(mame.join("parent.zip"), b"not inspected").unwrap();
        std::fs::write(mame.join("bios.zip"), b"not inspected").unwrap();
        std::fs::write(hbmame.join("hack.zip"), b"not inspected").unwrap();
        let inventory = ArcadeRomInventory::from_directories(&[
            (RomNamespace::Mame, mame),
            (RomNamespace::Hbmame, hbmame),
        ]);

        assert_eq!(
            inventory.eligibility(&PrimaryRomRequirement::Archive {
                namespace: RomNamespace::Mame,
                setname: "puckman".to_string(),
            }),
            RomEligibility::Eligible
        );
        assert_eq!(
            inventory.eligibility(&PrimaryRomRequirement::Archive {
                namespace: RomNamespace::Mame,
                setname: "clone".to_string(),
            }),
            RomEligibility::Missing
        );
        assert_eq!(
            inventory.eligibility(&PrimaryRomRequirement::Archive {
                namespace: RomNamespace::Hbmame,
                setname: "hack".to_string(),
            }),
            RomEligibility::Eligible
        );
        assert_eq!(
            inventory.eligibility(&PrimaryRomRequirement::None),
            RomEligibility::Eligible
        );
        assert_eq!(
            inventory.eligibility(&PrimaryRomRequirement::Ambiguous),
            RomEligibility::Ambiguous
        );
        let before_add = inventory.fingerprint().to_string();
        std::fs::write(root.join("mame/clone.zip"), b"corrupt is still present").unwrap();
        let with_clone = ArcadeRomInventory::from_directories(&[
            (RomNamespace::Mame, root.join("mame")),
            (RomNamespace::Hbmame, root.join("hbmame")),
        ]);
        assert_ne!(with_clone.fingerprint(), before_add);
        assert_eq!(
            with_clone.eligibility(&PrimaryRomRequirement::Archive {
                namespace: RomNamespace::Mame,
                setname: "clone".to_string(),
            }),
            RomEligibility::Eligible
        );
        std::fs::remove_file(root.join("mame/clone.zip")).unwrap();
        assert_eq!(
            ArcadeRomInventory::from_directories(&[
                (RomNamespace::Mame, root.join("mame")),
                (RomNamespace::Hbmame, root.join("hbmame")),
            ])
            .fingerprint(),
            before_add
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
