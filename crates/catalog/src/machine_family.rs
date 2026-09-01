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
#[cfg(test)]
use rusqlite::{Connection, params_from_iter};
use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::HashMap;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

#[cfg(test)]
const QUERY_CHUNK: usize = 400;

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

#[cfg(test)]
struct MachineDatabase {
    source: MachineSource,
    path: PathBuf,
    connection: Connection,
}

#[derive(Default)]
pub(crate) struct MachineFamilyResolver {
    runtime_metadata: Option<MetadataStore>,
    runtime_arcade: Option<ArcadeShard>,
    runtime_unavailable: bool,
    #[cfg(test)]
    mame_path: Option<PathBuf>,
    #[cfg(test)]
    hbmame_path: Option<PathBuf>,
    #[cfg(test)]
    mame: Option<MachineDatabase>,
    #[cfg(test)]
    hbmame: Option<MachineDatabase>,
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
    pub(crate) fn for_storage_root(storage_root: &Path) -> Result<Self, String> {
        let roots = [
            storage_root.join("mister-magik-dev"),
            storage_root.join("mister-magik"),
        ];
        let selected = roots
            .iter()
            .find(|root| {
                root.join(crate::runtime_metadata::FILE_NAME).is_file()
                    || cfg!(test) && root.join("mame.sqlite3").is_file()
            })
            .cloned();
        let Some(root) = selected else {
            return Ok(Self::default());
        };
        Ok(Self {
            runtime_metadata: MetadataStore::open(&root.join(crate::runtime_metadata::FILE_NAME))
                .ok(),
            #[cfg(test)]
            mame_path: root
                .join("mame.sqlite3")
                .is_file()
                .then(|| root.join("mame.sqlite3")),
            #[cfg(test)]
            hbmame_path: root
                .join("hbmame.sqlite3")
                .is_file()
                .then(|| root.join("hbmame.sqlite3")),
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
        #[cfg(not(test))]
        {
            for (identity, namespace) in unresolved {
                self.unresolved = self.unresolved.saturating_add(1);
                self.cache
                    .insert((identity.clone(), namespace.clone()), None);
                output.insert((identity, namespace), None);
            }
            Ok(output)
        }
        #[cfg(test)]
        let mut mame_ids = Vec::new();
        #[cfg(test)]
        let mut hbmame_ids = Vec::new();
        #[cfg(test)]
        for (identity, namespace) in &unresolved {
            if *namespace == Some(RomNamespace::Hbmame) {
                hbmame_ids.push(identity.clone());
            } else {
                mame_ids.push(identity.clone());
            }
        }
        #[cfg(test)]
        {
            let mut mame_rows = HashMap::new();
            let mut hbmame_rows = HashMap::new();
            if !hbmame_ids.is_empty() {
                self.ensure_hbmame()?;
                if let Some(database) = self.hbmame.as_ref() {
                    hbmame_rows.extend(query_database(database, &hbmame_ids)?);
                }
            }
            if !mame_ids.is_empty() {
                self.ensure_mame()?;
                if let Some(database) = self.mame.as_ref() {
                    mame_rows.extend(query_database(database, &mame_ids)?);
                }
            }
            // Unnamespaced identities prefer MAME, then fall back to HBMAME when
            // the set is absent from the main database.
            let hbmame_fallback = mame_ids
                .iter()
                .filter(|identity| !mame_rows.contains_key(*identity))
                .cloned()
                .collect::<Vec<_>>();
            if !hbmame_fallback.is_empty() {
                self.ensure_hbmame()?;
                if let Some(database) = self.hbmame.as_ref() {
                    hbmame_rows.extend(query_database(database, &hbmame_fallback)?);
                }
            }
            // Explicit HBMAME requests may still be absent there.  Falling back
            // to MAME preserves useful family projection for shared set names.
            let mame_fallback = hbmame_ids
                .iter()
                .filter(|identity| !hbmame_rows.contains_key(*identity))
                .cloned()
                .collect::<Vec<_>>();
            if !mame_fallback.is_empty() {
                self.ensure_mame()?;
                if let Some(database) = self.mame.as_ref() {
                    mame_rows.extend(query_database(database, &mame_fallback)?);
                }
            }
            for (identity, namespace) in unresolved {
                let row = match namespace {
                    Some(RomNamespace::Hbmame) => hbmame_rows
                        .get(&identity)
                        .or_else(|| mame_rows.get(&identity)),
                    Some(RomNamespace::Mame) | None => mame_rows
                        .get(&identity)
                        .or_else(|| hbmame_rows.get(&identity)),
                }
                .cloned();
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
            Ok(output)
        }
    }

    #[cfg(test)]
    fn ensure_mame(&mut self) -> Result<(), String> {
        if let Some(path) = self.mame_path.take() {
            self.mame = Some(open_database(&path, MachineSource::Mame)?);
        }
        Ok(())
    }

    #[cfg(test)]
    fn ensure_hbmame(&mut self) -> Result<(), String> {
        if let Some(path) = self.hbmame_path.take() {
            self.hbmame = Some(open_database(&path, MachineSource::Hbmame)?);
        }
        Ok(())
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
    let preferred_hbmame = namespace == Some(&RomNamespace::Hbmame);
    let preferred_mame = if preferred_hbmame {
        shard.machine(true, identity)
    } else {
        shard.machine(false, identity)
    };
    preferred_mame
        .map(|machine| ResolvedMachine {
            identity: normalize(&machine.setname),
            family: machine
                .parent_setname
                .as_deref()
                .map(normalize)
                .filter(|parent| !parent.is_empty())
                .unwrap_or_else(|| normalize(&machine.setname)),
            source: if preferred_hbmame {
                MachineSource::Hbmame
            } else {
                MachineSource::Mame
            },
        })
        .or_else(|| {
            let fallback_hbmame = !preferred_hbmame;
            shard
                .machine(fallback_hbmame, identity)
                .map(|machine| ResolvedMachine {
                    identity: normalize(&machine.setname),
                    family: machine
                        .parent_setname
                        .as_deref()
                        .map(normalize)
                        .filter(|parent| !parent.is_empty())
                        .unwrap_or_else(|| normalize(&machine.setname)),
                    source: if fallback_hbmame {
                        MachineSource::Hbmame
                    } else {
                        MachineSource::Mame
                    },
                })
        })
}

#[cfg(test)]
fn open_database(path: &Path, source: MachineSource) -> Result<MachineDatabase, String> {
    let connection = library_db::open_sqlite_read_only(path).map_err(|error| {
        format!(
            "open {:?} family database {}: {error}",
            source,
            path.display()
        )
    })?;
    let exists = library_db::sqlite_table_exists(&connection, "mame_machines")
        .map_err(|error| format!("inspect family database {}: {error}", path.display()))?;
    if !exists {
        return Err(format!(
            "family database {} is missing mame_machines",
            path.display()
        ));
    }
    Ok(MachineDatabase {
        source,
        path: path.to_path_buf(),
        connection,
    })
}

#[cfg(test)]
fn query_database(
    database: &MachineDatabase,
    identities: &[String],
) -> Result<HashMap<String, ResolvedMachine>, String> {
    let mut output = HashMap::with_capacity(identities.len());
    let mut identities = identities.to_vec();
    identities.sort_unstable();
    identities.dedup();
    for chunk in identities.chunks(QUERY_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT setname,parent_setname FROM mame_machines WHERE setname IN ({placeholders})"
        );
        let mut statement = database.connection.prepare(&sql).map_err(|error| {
            format!("prepare family query {}: {error}", database.path.display())
        })?;
        let rows = statement
            .query_map(params_from_iter(chunk.iter()), |row| {
                let identity = normalize(&row.get::<_, String>(0)?);
                let parent = row
                    .get::<_, Option<String>>(1)?
                    .map(|value| normalize(&value));
                let family = parent
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| identity.clone());
                Ok(ResolvedMachine {
                    identity,
                    family,
                    source: database.source,
                })
            })
            .map_err(|error| {
                format!("query family database {}: {error}", database.path.display())
            })?;
        for row in rows {
            let row = row
                .map_err(|error| format!("read family row {}: {error}", database.path.display()))?;
            output.insert(row.identity.clone(), row);
        }
    }
    Ok(output)
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
    use rusqlite::Connection;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

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
        let path = root.join("mister-magik-dev/mame.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE mame_machines(setname TEXT PRIMARY KEY,parent_setname TEXT);
             INSERT INTO mame_machines VALUES ('parent',NULL),('clone','parent'),('same','');",
            )
            .unwrap();
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
        let (root, _path) = fixture();
        let hbmame = root.join("mister-magik-dev/hbmame.sqlite3");
        let connection = Connection::open(&hbmame).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE mame_machines(setname TEXT PRIMARY KEY,parent_setname TEXT);
                 INSERT INTO mame_machines VALUES ('clone','hbmame-parent');",
            )
            .unwrap();
        drop(connection);

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
    fn selecting_a_database_is_lazy_until_a_non_empty_batch() {
        let (root, _path) = fixture();
        let mut resolver = MachineFamilyResolver::for_storage_root(&root).unwrap();
        assert!(resolver.mame.is_none());
        assert!(
            resolver
                .resolve_many(std::iter::empty::<(String, Option<RomNamespace>)>())
                .unwrap()
                .is_empty()
        );
        assert!(resolver.mame.is_none());
        let rows = resolver
            .resolve_many([(String::from("parent"), None)])
            .unwrap();
        assert!(rows[&(String::from("parent"), None)].is_some());
        assert!(resolver.mame.is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compact_arcade_shard_is_preferred_over_legacy_sqlite() {
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

    #[test]
    fn malformed_selected_schema_is_reported_on_query() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-family-malformed-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("mister-magik-dev")).unwrap();
        let path = root.join("mister-magik-dev/mame.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE mame_machines(setname TEXT PRIMARY KEY);")
            .unwrap();
        drop(connection);
        let mut resolver = MachineFamilyResolver::for_storage_root(&root).unwrap();
        assert!(
            resolver
                .resolve_many([(String::from("broken"), None)])
                .expect_err("missing parent_setname must not be flattened")
                .contains("family")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
