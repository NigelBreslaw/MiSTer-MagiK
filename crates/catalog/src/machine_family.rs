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
use rusqlite::{Connection, params_from_iter};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const QUERY_CHUNK: usize = 400;

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

struct MachineDatabase {
    source: MachineSource,
    connection: Connection,
}

#[derive(Default)]
pub(crate) struct MachineFamilyResolver {
    mame: Option<MachineDatabase>,
    hbmame: Option<MachineDatabase>,
    cache: BTreeMap<(String, Option<RomNamespace>), Option<ResolvedMachine>>,
    pub(crate) requested: usize,
    pub(crate) cache_hits: usize,
    pub(crate) mame_matches: usize,
    pub(crate) hbmame_matches: usize,
    pub(crate) unresolved: usize,
}

impl MachineFamilyResolver {
    /// Select one installation root and open each database lazily once.
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
            .find(|root| root.join("mame.sqlite3").is_file())
            .cloned();
        let Some(root) = selected else {
            return Ok(Self::default());
        };
        let mame_path = root.join("mame.sqlite3");
        let mame = Some(open_database(&mame_path, MachineSource::Mame)?);
        let hbmame_path = root.join("hbmame.sqlite3");
        let hbmame = hbmame_path
            .is_file()
            .then(|| open_database(&hbmame_path, MachineSource::Hbmame))
            .transpose()?;
        Ok(Self {
            mame,
            hbmame,
            ..Self::default()
        })
    }

    pub(crate) fn resolve(
        &mut self,
        identity: &str,
        namespace: Option<RomNamespace>,
    ) -> Option<ResolvedMachine> {
        let normalized = normalize(identity);
        if normalized.is_empty() {
            return None;
        }
        self.resolve_many([(normalized.clone(), namespace.clone())])
            .remove(&(normalized, namespace))
            .and_then(|row| row)
    }

    pub(crate) fn resolve_many<I>(
        &mut self,
        identities: I,
    ) -> BTreeMap<(String, Option<RomNamespace>), Option<ResolvedMachine>>
    where
        I: IntoIterator<Item = (String, Option<RomNamespace>)>,
    {
        let mut requested = BTreeMap::<(String, Option<RomNamespace>), ()>::new();
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
        let mut mame_ids = Vec::new();
        let mut hbmame_ids = Vec::new();
        for (identity, namespace) in &unresolved {
            if *namespace == Some(RomNamespace::Hbmame) {
                hbmame_ids.push(identity.clone());
            } else {
                mame_ids.push(identity.clone());
            }
        }
        let mut mame_rows = HashMap::new();
        let mut hbmame_rows = HashMap::new();
        if let Some(database) = self.hbmame.as_ref() {
            hbmame_rows.extend(query_database(database, &hbmame_ids));
        }
        if let Some(database) = self.mame.as_ref() {
            mame_rows.extend(query_database(database, &mame_ids));
        }
        // Unnamespaced identities prefer MAME, then fall back to HBMAME when
        // the set is absent from the main database.
        let hbmame_fallback = mame_ids
            .iter()
            .filter(|identity| !mame_rows.contains_key(*identity))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(database) = self.hbmame.as_ref() {
            hbmame_rows.extend(query_database(database, &hbmame_fallback));
        }
        // Explicit HBMAME requests may still be absent there.  Falling back
        // to MAME preserves useful family projection for shared set names.
        let mame_fallback = hbmame_ids
            .iter()
            .filter(|identity| !hbmame_rows.contains_key(*identity))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(database) = self.mame.as_ref() {
            mame_rows.extend(query_database(database, &mame_fallback));
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
                    MachineSource::Mame => self.mame_matches = self.mame_matches.saturating_add(1),
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
        output
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
    Ok(MachineDatabase { source, connection })
}

fn query_database(
    database: &MachineDatabase,
    identities: &[String],
) -> HashMap<String, ResolvedMachine> {
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
        let Ok(mut statement) = database.connection.prepare(&sql) else {
            continue;
        };
        let Ok(rows) = statement.query_map(params_from_iter(chunk.iter()), |row| {
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
        }) else {
            continue;
        };
        for row in rows.flatten() {
            output.insert(row.identity.clone(), row);
        }
    }
    output
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-family-{}-{}",
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
        let rows = resolver.resolve_many([
            ("clone".to_string(), None),
            ("parent".to_string(), None),
            ("same".to_string(), None),
            ("missing".to_string(), None),
        ]);
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
        assert!(resolver.resolve("missing", None).is_none());
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
        let rows = resolver.resolve_many([
            ("clone".to_string(), None),
            ("clone".to_string(), Some(RomNamespace::Hbmame)),
        ]);

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
}
