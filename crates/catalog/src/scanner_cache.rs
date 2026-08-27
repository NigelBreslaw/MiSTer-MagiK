// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lossless scanner accelerators owned by Catalog V3.

use crate::library_db::LibraryScan;
use crate::software_identity::{SoftwareHashCache, SoftwareHashCacheKey};
#[cfg(test)]
use rusqlite::params;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::catalog_format::SCANNER_CACHE_SCHEMA_VERSION as SCHEMA_VERSION;
const FILE_NAME: &str = "scanner-cache.sqlite3";
#[cfg(test)]
const WRITE_BATCH_ROWS: usize = 8_192;

#[derive(Clone, Debug, Default)]
pub struct DiscoveryHistory {
    pub(crate) by_game_id: HashMap<String, Option<i64>>,
}

impl DiscoveryHistory {
    pub(crate) fn discovered_at_for(&self, game_id: &str, scan: &LibraryScan) -> Option<i64> {
        self.by_game_id
            .get(game_id)
            .copied()
            .unwrap_or(Some(scan.scanned_at_unix))
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScannerCacheState {
    pub(crate) discovery_history: Option<DiscoveryHistory>,
    pub(crate) software_hash_cache: SoftwareHashCache,
}

pub fn path_for_root(storage_root: &Path) -> PathBuf {
    crate::catalog_state::path_for_root(storage_root).with_file_name(FILE_NAME)
}

pub(crate) fn default_path() -> PathBuf {
    path_for_root(&crate::catalog_config::default_sharded_catalog_path())
}

pub(crate) fn load_default() -> ScannerCacheState {
    let path = default_path();
    match read(&path) {
        Ok(state) => state,
        Err(error) if path.exists() => {
            crate::catalog_errln!("scanner_cache_tsv\tstatus=read-failed\terror={error}");
            ScannerCacheState::default()
        }
        Err(_) => ScannerCacheState::default(),
    }
}

pub(crate) fn read(path: &Path) -> Result<ScannerCacheState, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("open scanner cache {}: {error}", path.display()))?;
    let version = conn
        .query_row(
            "SELECT value FROM scanner_cache_meta WHERE key='schema_version'",
            [],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|error| format!("read scanner cache schema: {error}"))?;
    if version != SCHEMA_VERSION {
        return Err(format!(
            "scanner cache schema {version} is unsupported; expected {SCHEMA_VERSION}"
        ));
    }
    read_rows(&conn)
}

fn read_rows(conn: &Connection) -> Result<ScannerCacheState, String> {
    let mut history = DiscoveryHistory::default();
    let mut history_stmt = conn
        .prepare("SELECT game_id,discovered_at_unix FROM games")
        .map_err(|error| format!("prepare discovery history: {error}"))?;
    let history_rows = history_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .map_err(|error| format!("query discovery history: {error}"))?;
    for row in history_rows {
        let (game_id, discovered_at) =
            row.map_err(|error| format!("read discovery history: {error}"))?;
        history.by_game_id.insert(game_id, discovered_at);
    }

    let mut software_hash_cache = SoftwareHashCache::default();
    let mut cache_stmt = conn
        .prepare(
            "SELECT list_name,file_path,size,mtime_secs,software_name FROM software_hash_cache",
        )
        .map_err(|error| format!("prepare software hash cache: {error}"))?;
    let cache_rows = cache_stmt
        .query_map([], |row| {
            Ok((
                SoftwareHashCacheKey {
                    list_name: row.get(0)?,
                    file_path: row.get(1)?,
                    size: row.get::<_, i64>(2)?.max(0) as u64,
                    mtime_secs: row.get(3)?,
                },
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|error| format!("query software hash cache: {error}"))?;
    for row in cache_rows {
        let (key, value) = row.map_err(|error| format!("read software hash cache: {error}"))?;
        software_hash_cache.entries.insert(key, value);
    }
    Ok(ScannerCacheState {
        discovery_history: Some(history),
        software_hash_cache,
    })
}

#[cfg(test)]
fn validate_staged(path: &Path, expected: &ScannerCacheState) -> Result<(), String> {
    crate::cooperative_work::checkpoint();
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("open staged scanner cache {}: {error}", path.display()))?;
    let version = conn
        .query_row(
            "SELECT value FROM scanner_cache_meta WHERE key='schema_version'",
            [],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|error| format!("read staged scanner cache schema: {error}"))?;
    if version != SCHEMA_VERSION {
        return Err(format!(
            "staged scanner cache schema {version} is unsupported; expected {SCHEMA_VERSION}"
        ));
    }
    let history_rows = conn
        .query_row("SELECT COUNT(*) FROM games", [], |row| row.get::<_, i64>(0))
        .map_err(|error| format!("count staged discovery history: {error}"))?;
    let expected_history_rows = i64::try_from(
        expected
            .discovery_history
            .as_ref()
            .map_or(0, |history| history.by_game_id.len()),
    )
    .map_err(|_| "discovery history row count exceeds SQLite integer".to_string())?;
    if history_rows != expected_history_rows {
        return Err(format!(
            "staged discovery history has {history_rows} rows; expected {expected_history_rows}"
        ));
    }
    let hash_rows = conn
        .query_row("SELECT COUNT(*) FROM software_hash_cache", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| format!("count staged software hash cache: {error}"))?;
    let expected_hash_rows = i64::try_from(expected.software_hash_cache.entries.len())
        .map_err(|_| "software hash cache row count exceeds SQLite integer".to_string())?;
    if hash_rows != expected_hash_rows {
        return Err(format!(
            "staged software hash cache has {hash_rows} rows; expected {expected_hash_rows}"
        ));
    }
    conn.close()
        .map_err(|(_, error)| format!("close staged scanner cache: {error}"))?;
    crate::cooperative_work::checkpoint();
    Ok(())
}

#[cfg(test)]
pub(crate) struct StagedScannerCache {
    temp: Option<PathBuf>,
    final_path: PathBuf,
}

#[cfg(test)]
impl StagedScannerCache {
    pub(crate) fn publish(mut self) -> Result<(), String> {
        let temp = self.temp.as_ref().expect("staged scanner cache path");
        std::fs::File::open(temp)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("sync scanner cache: {error}"))?;
        std::fs::rename(temp, &self.final_path)
            .map_err(|error| format!("publish scanner cache: {error}"))?;
        crate::sqlite_catalog::sync_parent_dir(&self.final_path);
        self.temp = None;
        Ok(())
    }
}

#[cfg(test)]
impl Drop for StagedScannerCache {
    fn drop(&mut self) {
        if let Some(temp) = self.temp.take() {
            let _ = std::fs::remove_file(temp);
        }
    }
}

#[cfg(test)]
pub(crate) fn stage(path: &Path, state: &ScannerCacheState) -> Result<StagedScannerCache, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create scanner cache dir {}: {error}", parent.display()))?;
    }
    let temp = path.with_file_name(format!(".{FILE_NAME}.stage.{}", std::process::id()));
    let result = (|| {
        let _ = std::fs::remove_file(&temp);
        let mut conn = Connection::open(&temp)
            .map_err(|error| format!("create scanner cache {}: {error}", temp.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=OFF;
             PRAGMA synchronous=OFF;
             PRAGMA page_size=32768;
             PRAGMA cache_size=-8192;
             PRAGMA temp_store=MEMORY;
             PRAGMA locking_mode=EXCLUSIVE;
             CREATE TABLE scanner_cache_meta(key TEXT PRIMARY KEY,value INTEGER NOT NULL) WITHOUT ROWID;
             CREATE TABLE games(game_id TEXT PRIMARY KEY,discovered_at_unix INTEGER) WITHOUT ROWID;
             CREATE TABLE software_hash_cache(
                 list_name TEXT NOT NULL,file_path TEXT NOT NULL,size INTEGER NOT NULL,
                 mtime_secs INTEGER NOT NULL,software_name TEXT,
                 PRIMARY KEY(list_name,file_path,size,mtime_secs)
             ) WITHOUT ROWID;",
        )
        .map_err(|error| format!("create scanner cache schema: {error}"))?;
        conn.execute(
            "INSERT INTO scanner_cache_meta(key,value) VALUES ('schema_version',?1)",
            [SCHEMA_VERSION],
        )
        .map_err(|error| format!("write scanner cache schema: {error}"))?;
        if let Some(history) = &state.discovery_history {
            let mut rows = history.by_game_id.iter().peekable();
            while rows.peek().is_some() {
                crate::cooperative_work::checkpoint();
                let tx = conn
                    .transaction()
                    .map_err(|error| format!("begin discovery history batch: {error}"))?;
                {
                    let mut statement = tx
                        .prepare("INSERT INTO games(game_id,discovered_at_unix) VALUES (?1,?2)")
                        .map_err(|error| format!("prepare discovery history insert: {error}"))?;
                    for (game_id, discovered_at) in rows.by_ref().take(WRITE_BATCH_ROWS) {
                        statement
                            .execute(params![game_id, discovered_at])
                            .map_err(|error| format!("write discovery history: {error}"))?;
                    }
                }
                tx.commit()
                    .map_err(|error| format!("commit discovery history batch: {error}"))?;
            }
        }
        let mut rows = state.software_hash_cache.entries.iter().peekable();
        while rows.peek().is_some() {
            crate::cooperative_work::checkpoint();
            let tx = conn
                .transaction()
                .map_err(|error| format!("begin software hash cache batch: {error}"))?;
            {
                let mut statement = tx
                    .prepare(
                        "INSERT INTO software_hash_cache(
                         list_name,file_path,size,mtime_secs,software_name
                     ) VALUES (?1,?2,?3,?4,?5)",
                    )
                    .map_err(|error| format!("prepare software hash cache insert: {error}"))?;
                for (key, software_name) in rows.by_ref().take(WRITE_BATCH_ROWS) {
                    let size = i64::try_from(key.size).map_err(|_| {
                        "software hash cache size exceeds SQLite integer".to_string()
                    })?;
                    statement
                        .execute(params![
                            key.list_name,
                            key.file_path,
                            size,
                            key.mtime_secs,
                            software_name
                        ])
                        .map_err(|error| format!("write software hash cache: {error}"))?;
                }
            }
            tx.commit()
                .map_err(|error| format!("commit software hash cache batch: {error}"))?;
        }
        crate::cooperative_work::checkpoint();
        conn.close()
            .map_err(|(_, error)| format!("close scanner cache: {error}"))?;
        validate_staged(&temp, state)
            .map_err(|error| format!("validate staged scanner cache: {error}"))?;
        Ok(StagedScannerCache {
            temp: Some(temp.clone()),
            final_path: path.to_path_buf(),
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_cache_round_trips_without_a_library_database() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-scanner-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join(FILE_NAME);
        let mut state = ScannerCacheState::default();
        state
            .discovery_history
            .get_or_insert_with(DiscoveryHistory::default)
            .by_game_id
            .insert("game:one".into(), Some(123));
        state.software_hash_cache.entries.insert(
            SoftwareHashCacheKey {
                list_name: "nes".into(),
                file_path: "/games/One.nes".into(),
                size: 42,
                mtime_secs: 7,
            },
            Some("one".into()),
        );
        stage(&path, &state).unwrap().publish().unwrap();
        let loaded = read(&path).unwrap();
        assert_eq!(
            loaded.discovery_history.unwrap().by_game_id,
            state.discovery_history.unwrap().by_game_id
        );
        assert_eq!(
            loaded.software_hash_cache.entries,
            state.software_hash_cache.entries
        );
        assert!(!root.join("library.sqlite3").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn staged_scanner_cache_is_unreachable_until_publish() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-staged-scanner-cache-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join(FILE_NAME);
        let state = ScannerCacheState::default();

        let staged = stage(&path, &state).unwrap();
        assert!(!path.exists());
        staged.publish().unwrap();
        assert!(path.exists());
        read(&path).unwrap();

        let _ = std::fs::remove_dir_all(root);
    }
}
