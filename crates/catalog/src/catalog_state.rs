// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Durable scan facts owned by the sharded catalog.
//!
//! This deliberately contains no game rows or UI projections. It is the small
//! authority used to decide whether the live filesystem still matches the
//! published catalog generation.

use crate::catalog_checkpoint::CatalogDiscoveryCheckpoint;
use crate::catalog_stamp::CatalogStamp;
use crate::{catalog_store, sqlite_catalog};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

use crate::catalog_format::CATALOG_STATE_SCHEMA_VERSION as STATE_SCHEMA_VERSION;
const STATE_FILE_NAME: &str = "catalog-state.sqlite3";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogState {
    pub stamp: CatalogStamp,
    pub checkpoint: CatalogDiscoveryCheckpoint,
    pub stats: CatalogStateStats,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogStateStats {
    pub normal_files: usize,
    pub containers: usize,
    pub entries: usize,
    pub audit_rows: usize,
    pub discoveries: usize,
}

pub fn default_path() -> PathBuf {
    path_for_root(&crate::catalog_config::default_sharded_catalog_path())
}

pub fn path_for_root(storage_root: &Path) -> PathBuf {
    storage_root.join("state").join(STATE_FILE_NAME)
}

pub fn read(path: &Path) -> Result<CatalogState, String> {
    let conn = sqlite_catalog::open_sqlite_read_only(path)
        .map_err(|error| format!("open catalog state {}: {error}", path.display()))?;
    validate_connection(&conn, path)?;
    read_from_connection(&conn)
}

pub(crate) fn validate_connection(conn: &Connection, path: &Path) -> Result<(), String> {
    let version = conn
        .query_row(
            "SELECT value FROM catalog_state_meta WHERE key='schema_version'",
            [],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|error| format!("read catalog state schema {}: {error}", path.display()))?;
    if version != STATE_SCHEMA_VERSION {
        return Err(format!(
            "catalog state schema {version} is unsupported; expected {STATE_SCHEMA_VERSION}"
        ));
    }
    Ok(())
}

fn read_from_connection(conn: &Connection) -> Result<CatalogState, String> {
    let stamp = catalog_store::read_catalog_stamp(conn)?
        .ok_or_else(|| "catalog state is missing its stamp".to_string())?;
    let checkpoint = catalog_store::read_catalog_discovery_checkpoint(conn)?
        .ok_or_else(|| "catalog state is missing its discovery checkpoint".to_string())?;
    let stored_stats = conn
        .query_row(
            "SELECT normal_files,containers,entries,audit_rows,discoveries
             FROM catalog_state_stats WHERE id=0",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .map_err(|error| format!("read catalog state stats: {error}"))?;
    let stats = CatalogStateStats {
        normal_files: stored_stat(stored_stats.0, "normal_files")?,
        containers: stored_stat(stored_stats.1, "containers")?,
        entries: stored_stat(stored_stats.2, "entries")?,
        audit_rows: stored_stat(stored_stats.3, "audit_rows")?,
        discoveries: stored_stat(stored_stats.4, "discoveries")?,
    };
    Ok(CatalogState {
        stamp,
        checkpoint,
        stats,
    })
}

#[cfg(test)]
pub fn read_legacy(path: &Path) -> Result<CatalogState, String> {
    let conn = sqlite_catalog::open_sqlite_read_only(path)
        .map_err(|error| format!("open legacy catalog state {}: {error}", path.display()))?;
    let stamp = catalog_store::read_catalog_stamp(&conn)?
        .ok_or_else(|| "legacy catalog is missing its stamp".to_string())?;
    let checkpoint = catalog_store::read_catalog_discovery_checkpoint(&conn)?
        .ok_or_else(|| "legacy catalog is missing its discovery checkpoint".to_string())?;
    let stats = CatalogStateStats {
        normal_files: read_legacy_stat(&conn, "normal_files")?,
        containers: read_legacy_stat(&conn, "containers")?,
        entries: read_legacy_stat(&conn, "entries")?,
        audit_rows: read_legacy_stat(&conn, "audit_rows")?,
        discoveries: read_legacy_stat(&conn, "discoveries")?,
    };
    Ok(CatalogState {
        stamp,
        checkpoint,
        stats,
    })
}

#[cfg(test)]
fn read_legacy_stat(conn: &Connection, key: &str) -> Result<usize, String> {
    let value = conn
        .query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| format!("read legacy catalog stat {key}: {error}"))?;
    stored_stat(value, key)
}

fn stored_stat(value: i64, key: &str) -> Result<usize, String> {
    usize::try_from(value).map_err(|_| format!("catalog stat {key} is out of range"))
}

pub fn write(path: &Path, state: &CatalogState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create catalog state dir {}: {error}", parent.display()))?;
    }
    let temp_path = path.with_file_name(format!(".{STATE_FILE_NAME}.tmp"));
    let stored_stats = [
        ("normal_files", state.stats.normal_files),
        ("containers", state.stats.containers),
        ("entries", state.stats.entries),
        ("audit_rows", state.stats.audit_rows),
        ("discoveries", state.stats.discoveries),
    ]
    .map(|(name, value)| {
        i64::try_from(value).map_err(|_| format!("catalog stat {name} exceeds SQLite integer"))
    })
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
    let result = (|| {
        let _ = std::fs::remove_file(&temp_path);
        let conn = Connection::open(&temp_path)
            .map_err(|error| format!("create catalog state {}: {error}", temp_path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=OFF;
             PRAGMA synchronous=OFF;
             CREATE TABLE catalog_state_meta (
                 key TEXT PRIMARY KEY,
                 value INTEGER NOT NULL
             ) WITHOUT ROWID;
             CREATE TABLE catalog_state_stats (
                 id INTEGER PRIMARY KEY CHECK (id=0),
                 normal_files INTEGER NOT NULL,
                 containers INTEGER NOT NULL,
                 entries INTEGER NOT NULL,
                 audit_rows INTEGER NOT NULL,
                 discoveries INTEGER NOT NULL
             ) WITHOUT ROWID;",
        )
        .map_err(|error| format!("create catalog state schema: {error}"))?;
        catalog_store::create_catalog_stamp_schema(&conn)?;
        conn.execute(
            "INSERT INTO catalog_state_meta(key,value) VALUES ('schema_version',?1)",
            params![STATE_SCHEMA_VERSION],
        )
        .map_err(|error| format!("write catalog state schema: {error}"))?;
        catalog_store::write_catalog_stamp(&conn, &state.stamp)?;
        catalog_store::write_catalog_discovery_checkpoint(&conn, &state.checkpoint)?;
        conn.execute(
            "INSERT INTO catalog_state_stats(
                 id,normal_files,containers,entries,audit_rows,discoveries
             ) VALUES (0,?1,?2,?3,?4,?5)",
            params![
                stored_stats[0],
                stored_stats[1],
                stored_stats[2],
                stored_stats[3],
                stored_stats[4],
            ],
        )
        .map_err(|error| format!("write catalog state stats: {error}"))?;
        conn.close()
            .map_err(|(_, error)| format!("close catalog state: {error}"))?;
        std::fs::File::open(&temp_path)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("sync catalog state {}: {error}", temp_path.display()))?;
        std::fs::rename(&temp_path, path).map_err(|error| {
            format!(
                "publish catalog state {} from {}: {error}",
                path.display(),
                temp_path.display()
            )
        })?;
        sqlite_catalog::sync_parent_dir(path);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "mister-magik-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn state(line: &str) -> CatalogState {
        CatalogState {
            stamp: CatalogStamp::from_lines(vec![format!("stamp-{line}")]),
            checkpoint: CatalogDiscoveryCheckpoint::from_lines(vec![format!("checkpoint-{line}")]),
            stats: CatalogStateStats {
                discoveries: 42,
                ..CatalogStateStats::default()
            },
        }
    }

    #[test]
    fn state_round_trips_without_a_library_database() {
        let dir = test_dir("catalog-state-round-trip");
        let path = dir.join(STATE_FILE_NAME);
        let expected = state("one");
        write(&path, &expected).unwrap();
        assert_eq!(read(&path).unwrap(), expected);
        assert!(!dir.join("library.sqlite3").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn replacement_is_atomic_and_removes_the_temp_file() {
        let dir = test_dir("catalog-state-replace");
        let path = dir.join(STATE_FILE_NAME);
        write(&path, &state("one")).unwrap();
        write(&path, &state("two")).unwrap();
        assert_eq!(read(&path).unwrap(), state("two"));
        assert!(!dir.join(format!(".{STATE_FILE_NAME}.tmp")).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_state_is_rejected_instead_of_treated_as_absent() {
        let dir = test_dir("catalog-state-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(STATE_FILE_NAME);
        std::fs::write(&path, b"not sqlite").unwrap();
        assert!(read(&path).unwrap_err().contains("schema"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
