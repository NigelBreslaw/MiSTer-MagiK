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
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

const STATE_SCHEMA_VERSION: u32 = 1;
const STATE_FILE_NAME: &str = "catalog-state.sqlite3";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogState {
    pub stamp: CatalogStamp,
    pub checkpoint: CatalogDiscoveryCheckpoint,
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
    Ok(CatalogState { stamp, checkpoint })
}

pub fn read_legacy(path: &Path) -> Result<CatalogState, String> {
    let conn = sqlite_catalog::open_sqlite_read_only(path)
        .map_err(|error| format!("open legacy catalog state {}: {error}", path.display()))?;
    let stamp = catalog_store::read_catalog_stamp(&conn)?
        .ok_or_else(|| "legacy catalog is missing its stamp".to_string())?;
    let checkpoint = catalog_store::read_catalog_discovery_checkpoint(&conn)?
        .ok_or_else(|| "legacy catalog is missing its discovery checkpoint".to_string())?;
    Ok(CatalogState { stamp, checkpoint })
}

pub fn write(path: &Path, state: &CatalogState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create catalog state dir {}: {error}", parent.display()))?;
    }
    let temp_path = path.with_file_name(format!(".{STATE_FILE_NAME}.tmp"));
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
