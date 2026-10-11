// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded MAME/HBMAME machine-family lookup shared by fast catalog sources.
//!
//! The resolver deliberately loads only the two columns needed for family
//! projection and keeps connections/cache state for the lifetime of one
//! source build.  It is therefore safe to use from both Arcade and Neo Geo
//! adapters without reopening the databases for each row.

use crate::library_db;
use crate::mra_header::RomNamespace;
use crate::runtime_metadata::{ArcadeShard, MetadataStore};
use std::collections::BTreeMap;
use std::path::Path;

type MachineLookupKey = (String, Option<RomNamespace>);
type MachineLookupResults = BTreeMap<MachineLookupKey, Option<ResolvedMachine>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedMachine {
    pub identity: String,
    pub family: String,
    pub source: MachineSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MachineSource {
    Mame,
    Hbmame,
}

#[derive(Default)]
pub(crate) struct MachineFamilyResolver {
    runtime_metadata: Option<MetadataStore>,
    runtime_arcade: Option<ArcadeShard>,
    runtime_unavailable: bool,
    cache: MachineLookupResults,
    pub(crate) requested: usize,
    pub(crate) cache_hits: usize,
    pub(crate) mame_matches: usize,
    pub(crate) hbmame_matches: usize,
    pub(crate) unresolved: usize,
}

impl MachineFamilyResolver {
    /// Select one installation root without opening its databases.
    ///
    /// A Dev MAME database and a stable HBMAME database are intentionally not
    /// mixed: both databases come from the selected root.
    #[cfg(any(test, feature = "builder"))]
    pub(crate) fn for_storage_root(storage_root: &Path) -> Result<Self, String> {
        let roots = [
            storage_root.join("mister-magik-dev"),
            storage_root.join("mister-magik"),
        ];
        let selected = roots
            .iter()
            .find(|root| root.join(crate::runtime_metadata::FILE_NAME).is_file())
            .cloned();
        let Some(root) = selected else {
            return Ok(Self::default());
        };
        Ok(Self {
            runtime_metadata: MetadataStore::open(&root.join(crate::runtime_metadata::FILE_NAME))
                .ok(),
            ..Self::default()
        })
    }

    #[cfg(test)]
    pub(crate) fn resolve(
        &mut self,
        identity: &str,
        namespace: Option<RomNamespace>,
    ) -> Result<Option<ResolvedMachine>, String> {
        let normalized = normalize(identity);
        if normalized.is_empty() {
            return Ok(None);
        }
        Ok(self
            .resolve_many([(normalized.clone(), namespace.clone())])?
            .remove(&(normalized, namespace))
            .and_then(|row| row))
    }

    pub(crate) fn resolve_many<I>(&mut self, identities: I) -> Result<MachineLookupResults, String>
    where
        I: IntoIterator<Item = (String, Option<RomNamespace>)>,
    {
        let mut requested = BTreeMap::<MachineLookupKey, ()>::new();
        for (identity, namespace) in identities {
            let identity = normalize(&identity);
            if identity.is_empty() {
                continue;
            }
            requested.insert((identity, namespace), ());
        }
        self.requested = self.requested.saturating_add(requested.len());
        let mut unresolved = Vec::new();
        let mut output = BTreeMap::new();
        for (identity, namespace) in requested.keys().cloned() {
            let cache_key = (identity.clone(), namespace.clone());
            if let Some(value) = self.cache.get(&cache_key) {
                self.cache_hits = self.cache_hits.saturating_add(1);
                output.insert(cache_key, value.clone());
            } else {
                unresolved.push((identity, namespace));
            }
        }
        if unresolved.is_empty() {
            return Ok(output);
        }
        if self.runtime_is_available() {
            let Some(arcade) = self.runtime_arcade.as_ref() else {
                return Ok(output);
            };
            for (identity, namespace) in unresolved {
                let row = runtime_row(arcade, &identity, namespace.as_ref());
                if let Some(row) = &row {
                    match row.source {
                        MachineSource::Mame => {
                            self.mame_matches = self.mame_matches.saturating_add(1)
                        }
                        MachineSource::Hbmame => {
                            self.hbmame_matches = self.hbmame_matches.saturating_add(1)
                        }
                    }
                } else {
                    self.unresolved = self.unresolved.saturating_add(1);
                }
                self.cache
                    .insert((identity.clone(), namespace.clone()), row.clone());
                output.insert((identity, namespace), row);
            }
            return Ok(output);
        }
        for (identity, namespace) in unresolved {
            self.unresolved = self.unresolved.saturating_add(1);
            self.cache
                .insert((identity.clone(), namespace.clone()), None);
            output.insert((identity, namespace), None);
        }
        Ok(output)
    }

    fn runtime_is_available(&mut self) -> bool {
        if self.runtime_unavailable {
            return false;
        }
        if self.runtime_arcade.is_none() {
            let Some(store) = self.runtime_metadata.as_ref() else {
                self.runtime_unavailable = true;
                return false;
            };
            match store.arcade_shard() {
                Ok(Some(shard)) => self.runtime_arcade = Some(shard),
                Ok(None) | Err(_) => {
                    self.runtime_unavailable = true;
                    return false;
                }
            }
        }
        true
    }

    #[cfg(feature = "builder")]
    pub(crate) fn finish_log(&self, system: &str) {
        crate::catalog_logln!(
            "fast_catalog_machine_family_tsv\tsystem={system}\trequested={}\tcache_hits={}\tmame_matches={}\thbmame_matches={}\tunresolved={}",
            self.requested,
            self.cache_hits,
            self.mame_matches,
            self.hbmame_matches,
            self.unresolved,
        );
    }
}

fn runtime_row(
    shard: &ArcadeShard,
    identity: &str,
    namespace: Option<&RomNamespace>,
) -> Option<ResolvedMachine> {
    let preferred = namespace == Some(&RomNamespace::Hbmame);
    for hbmame in [preferred, !preferred] {
        let Some(machine) = shard.machine(hbmame, identity) else {
            continue;
        };
        let mut family = machine;
        let mut seen = std::collections::BTreeSet::new();
        while let Some(parent) = family
            .parent_setname
            .as_deref()
            .filter(|key| !key.is_empty())
        {
            if !seen.insert(family.setname.as_str()) {
                return None;
            }
            let Some(next) = shard.machine(hbmame, parent) else {
                break;
            };
            family = next;
        }
        return Some(ResolvedMachine {
            identity: normalize(&machine.setname),
            family: family
                .parent_setname
                .as_deref()
                .filter(|parent| !parent.is_empty() && shard.machine(hbmame, parent).is_none())
                .map(normalize)
                .unwrap_or_else(|| normalize(&family.setname)),
            source: if hbmame {
                MachineSource::Hbmame
            } else {
                MachineSource::Mame
            },
        });
    }
    None
}

fn parse_reviewed_families(data: &str) -> Result<BTreeMap<String, &str>, String> {
    let mut families = BTreeMap::new();
    let valid_key = |key: &str| {
        !key.is_empty()
            && key.as_bytes()[0].is_ascii_alphanumeric()
            && key != "unknown"
            && !key.ends_with(".zip")
            && key.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.-".contains(&byte)
            })
    };
    for (index, line) in data.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 3 || !valid_key(fields[0]) || !valid_key(fields[1]) {
            return Err(format!(
                "invalid reviewed screenshot family row {}",
                index + 1
            ));
        }
        let [key, family, evidence] = [fields[0], fields[1], fields[2]];
        let host = evidence
            .strip_prefix("https://")
            .and_then(|url| url.split('/').next());
        if normalize(key) == normalize(family)
            || host.is_none_or(str::is_empty)
            || evidence.chars().any(char::is_whitespace)
            || families.insert(normalize(key), family).is_some()
        {
            return Err(format!(
                "duplicate, self-referencing or unverified screenshot family row {}",
                index + 1
            ));
        }
    }
    if families
        .values()
        .any(|family| families.contains_key(&normalize(family)))
    {
        return Err("reviewed screenshot families must point directly to parents".into());
    }
    Ok(families)
}

/// Reviewed custom identities; invalid shared data must fail catalog construction.
pub(crate) fn reviewed_screenshot_family(identity: &str) -> Result<Option<&'static str>, String> {
    static FAMILIES: std::sync::LazyLock<Result<BTreeMap<String, &'static str>, String>> =
        std::sync::LazyLock::new(|| {
            parse_reviewed_families(include_str!("../data/arcade-screenshot-families.tsv"))
        });
    FAMILIES
        .as_ref()
        .map(|families| families.get(&normalize(identity)).copied())
        .map_err(Clone::clone)
}

impl MachineFamilyResolver {
    pub(crate) fn for_metadata_path(path: &Path) -> Self {
        Self {
            runtime_metadata: MetadataStore::open(path).ok(),
            ..Self::default()
        }
    }

    /// Canonical families and available representatives are separate from game identity.
    pub(crate) fn screenshot_families(
        &mut self,
        keys: impl IntoIterator<Item = String>,
    ) -> Result<BTreeMap<String, String>, String> {
        let keys = keys.into_iter().collect::<Vec<_>>();
        let mut targets = BTreeMap::new();
        for key in &keys {
            targets.insert(
                key.clone(),
                reviewed_screenshot_family(key)?.unwrap_or(key).to_string(),
            );
        }
        let resolved = self.resolve_many(targets.values().map(|key| (key.clone(), None)))?;
        Ok(targets
            .into_iter()
            .map(|(key, target)| {
                let target = normalize(&target);
                let family = resolved
                    .get(&(target.clone(), None))
                    .and_then(Option::as_ref)
                    .map(|machine| machine.family.clone())
                    .unwrap_or(target);
                (key, family)
            })
            .collect())
    }
}

pub(crate) fn available_family_screenshots(
    families: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut available = BTreeMap::new();
    for (key, family) in families {
        if normalize(key) == *family || !available.contains_key(family) {
            available.insert(family.clone(), key.clone());
        }
    }
    available
}

fn normalize(value: &str) -> String {
    let value = library_db::normalize_id(value);
    if value == "unknown" {
        String::new()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn reviewed_family_rows_are_strict_and_shared_table_is_valid() {
        assert_eq!(
            reviewed_screenshot_family("dfeveronnd").unwrap(),
            Some("feversos")
        );
        for data in [
            "key\tparent",
            "Key\tparent\thttps://example.test/mra",
            "key\tkey\thttps://example.test/mra",
            "key\tparent\thttp://example.test/mra",
            "key\tparent\thttps:///mra",
            "key\tparent\thttps://example.test/mra\textra",
            "key\tparent\thttps://example.test/a\nkey\tother\thttps://example.test/b",
            "custom_key\tparent\thttps://example.test/a\ncustom-key\tother\thttps://example.test/b",
            "key\tcustom_key\thttps://example.test/a\ncustom-key\tparent\thttps://example.test/b",
            "unknown\tparent\thttps://example.test/a",
        ] {
            assert!(parse_reviewed_families(data).is_err(), "{data}");
        }
    }

    fn fixture() -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-family-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("mister-magik-dev")).unwrap();
        let path = root
            .join("mister-magik-dev")
            .join(crate::runtime_metadata::FILE_NAME);
        let mut builder = crate::runtime_metadata::MetadataFileBuilder::new();
        builder
            .add_arcade(&crate::runtime_metadata::ArcadeShard {
                mame: vec![
                    crate::runtime_metadata::ArcadeMachine {
                        setname: "clone".into(),
                        parent_setname: Some("parent".into()),
                        title: "Clone".into(),
                        year: None,
                        manufacturer: None,
                        players: None,
                        control: None,
                    },
                    crate::runtime_metadata::ArcadeMachine {
                        setname: "parent".into(),
                        parent_setname: None,
                        title: "Parent".into(),
                        year: None,
                        manufacturer: None,
                        players: None,
                        control: None,
                    },
                    crate::runtime_metadata::ArcadeMachine {
                        setname: "same".into(),
                        parent_setname: Some(String::new()),
                        title: "Same".into(),
                        year: None,
                        manufacturer: None,
                        players: None,
                        control: None,
                    },
                ],
                ..crate::runtime_metadata::ArcadeShard::default()
            })
            .unwrap();
        builder.write_to(&path).unwrap();
        (root, path)
    }

    #[test]
    fn resolves_parent_and_self_family_and_caches_misses() {
        let (root, _path) = fixture();
        let mut resolver = MachineFamilyResolver::for_storage_root(&root).unwrap();
        let rows = resolver
            .resolve_many([
                ("clone".to_string(), None),
                ("parent".to_string(), None),
                ("same".to_string(), None),
                ("missing".to_string(), None),
            ])
            .unwrap();
        assert_eq!(
            rows[&("clone".to_string(), None)].as_ref().unwrap().family,
            "parent"
        );
        assert_eq!(
            rows[&("parent".to_string(), None)].as_ref().unwrap().family,
            "parent"
        );
        assert_eq!(
            rows[&("same".to_string(), None)].as_ref().unwrap().family,
            "same"
        );
        assert!(rows[&("missing".to_string(), None)].is_none());
        assert!(resolver.resolve("missing", None).unwrap().is_none());
        assert_eq!(resolver.cache_hits, 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn namespace_keeps_same_identity_rows_separate() {
        let (root, path) = fixture();
        let mut builder = crate::runtime_metadata::MetadataFileBuilder::new();
        builder
            .add_arcade(&crate::runtime_metadata::ArcadeShard {
                mame: vec![crate::runtime_metadata::ArcadeMachine {
                    setname: "clone".into(),
                    parent_setname: Some("parent".into()),
                    title: "Clone".into(),
                    year: None,
                    manufacturer: None,
                    players: None,
                    control: None,
                }],
                hbmame: vec![crate::runtime_metadata::ArcadeMachine {
                    setname: "clone".into(),
                    parent_setname: Some("hbmame-parent".into()),
                    title: "Clone".into(),
                    year: None,
                    manufacturer: None,
                    players: None,
                    control: None,
                }],
                ..crate::runtime_metadata::ArcadeShard::default()
            })
            .unwrap();
        builder.write_to(&path).unwrap();

        let mut resolver = MachineFamilyResolver::for_storage_root(&root).unwrap();
        let rows = resolver
            .resolve_many([
                ("clone".to_string(), None),
                ("clone".to_string(), Some(RomNamespace::Hbmame)),
            ])
            .unwrap();

        assert_eq!(
            rows[&("clone".to_string(), None)].as_ref().unwrap().family,
            "parent"
        );
        assert_eq!(
            rows[&("clone".to_string(), Some(RomNamespace::Hbmame))]
                .as_ref()
                .unwrap()
                .family,
            "hbmame-parent"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selecting_compact_metadata_is_lazy_until_a_non_empty_batch() {
        let (root, _path) = fixture();
        let mut resolver = MachineFamilyResolver::for_storage_root(&root).unwrap();
        assert!(resolver.runtime_arcade.is_none());
        assert!(
            resolver
                .resolve_many(std::iter::empty::<(String, Option<RomNamespace>)>())
                .unwrap()
                .is_empty()
        );
        assert!(resolver.runtime_arcade.is_none());
        let rows = resolver
            .resolve_many([(String::from("parent"), None)])
            .unwrap();
        assert!(rows[&(String::from("parent"), None)].is_some());
        assert!(resolver.runtime_arcade.is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compact_arcade_shard_resolves_machine_family() {
        let (root, _path) = fixture();
        let metadata_path = root
            .join("mister-magik-dev")
            .join(crate::runtime_metadata::FILE_NAME);
        let mut builder = crate::runtime_metadata::MetadataFileBuilder::new();
        builder
            .add_arcade(&crate::runtime_metadata::ArcadeShard {
                mame: vec![crate::runtime_metadata::ArcadeMachine {
                    setname: "clone".into(),
                    parent_setname: Some("compact-parent".into()),
                    title: "Clone".into(),
                    year: None,
                    manufacturer: None,
                    players: None,
                    control: None,
                }],
                ..crate::runtime_metadata::ArcadeShard::default()
            })
            .unwrap();
        builder.write_to(&metadata_path).unwrap();

        let mut resolver = MachineFamilyResolver::for_storage_root(&root).unwrap();
        let resolved = resolver.resolve("clone", None).unwrap().unwrap();
        assert_eq!(resolved.family, "compact-parent");
        assert_eq!(resolved.source, MachineSource::Mame);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_compact_metadata_without_sqlite_is_nonfatal() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-family-compact-corrupt-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let app = root.join("mister-magik-dev");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join(crate::runtime_metadata::FILE_NAME),
            b"corrupt compact metadata",
        )
        .unwrap();
        let mut resolver = MachineFamilyResolver::for_storage_root(&root).unwrap();
        assert!(resolver.resolve("missing", None).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
