// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Durable launcher-owned favourites and play history.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::fs;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UserGameIdentity {
    pub system_id: String,
    pub stable_key: String,
    pub title: String,
    pub launch_ref: String,
    pub payload_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecentGame {
    pub game: UserGameIdentity,
    pub last_played_at: i64,
    pub play_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnresolvedImport {
    pub source: String,
    pub kind: String,
    pub path: String,
    pub title: String,
    pub observed_at: i64,
}

#[derive(Clone, Debug)]
pub struct UserStateStore {
    path: PathBuf,
}

impl UserStateStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let store = Self { path: path.into() };
        if let Some(parent) = store.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!("create user-state directory {}: {error}", parent.display())
            })?;
        }
        let mut connection = store.connection()?;
        migrate(&mut connection)?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_favourite(&self, game: &UserGameIdentity) -> Result<bool, String> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT 1 FROM favourites WHERE system_id=?1 AND stable_key=?2",
                params![game.system_id, game.stable_key],
                |_| Ok(()),
            )
            .optional()
            .map(|value| value.is_some())
            .map_err(|error| format!("read favourite: {error}"))
    }

    pub fn set_favourite(
        &self,
        game: &UserGameIdentity,
        favourite: bool,
        changed_at: i64,
    ) -> Result<(), String> {
        let connection = self.connection()?;
        if favourite {
            connection
                .execute(
                    "INSERT INTO favourites(
                        system_id,stable_key,title,launch_ref,payload_path,favourited_at
                     ) VALUES (?1,?2,?3,?4,?5,?6)
                     ON CONFLICT(system_id,stable_key) DO UPDATE SET
                        title=excluded.title,
                        launch_ref=excluded.launch_ref,
                        payload_path=excluded.payload_path",
                    params![
                        game.system_id,
                        game.stable_key,
                        game.title,
                        game.launch_ref,
                        game.payload_path,
                        changed_at,
                    ],
                )
                .map_err(|error| format!("write favourite: {error}"))?;
        } else {
            connection
                .execute(
                    "DELETE FROM favourites WHERE system_id=?1 AND stable_key=?2",
                    params![game.system_id, game.stable_key],
                )
                .map_err(|error| format!("remove favourite: {error}"))?;
        }
        Ok(())
    }

    pub fn record_play(&self, game: &UserGameIdentity, played_at: i64) -> Result<(), String> {
        self.connection()?
            .execute(
                "INSERT INTO play_sessions(
                    system_id,stable_key,title,launch_ref,payload_path,played_at
                 ) VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    game.system_id,
                    game.stable_key,
                    game.title,
                    game.launch_ref,
                    game.payload_path,
                    played_at,
                ],
            )
            .map(|_| ())
            .map_err(|error| format!("record play: {error}"))
    }

    pub fn favourite_count(&self, system_id: &str) -> Result<usize, String> {
        count_for_system(&self.connection()?, "favourites", system_id)
    }

    pub fn favourite_games(&self, system_id: &str) -> Result<Vec<UserGameIdentity>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT system_id,stable_key,title,launch_ref,payload_path
                 FROM favourites WHERE system_id=?1
                 ORDER BY favourited_at DESC,stable_key",
            )
            .map_err(|error| format!("prepare favourites: {error}"))?;
        let rows = statement
            .query_map([system_id], |row| {
                Ok(UserGameIdentity {
                    system_id: row.get(0)?,
                    stable_key: row.get(1)?,
                    title: row.get(2)?,
                    launch_ref: row.get(3)?,
                    payload_path: row.get(4)?,
                })
            })
            .map_err(|error| format!("query favourites: {error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("read favourites: {error}"))
    }

    pub fn recent_unique(&self, system_id: &str, limit: usize) -> Result<Vec<RecentGame>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT p.system_id,p.stable_key,p.title,p.launch_ref,p.payload_path,
                        p.played_at,
                        (SELECT count(*) FROM play_sessions c
                         WHERE c.system_id=p.system_id AND c.stable_key=p.stable_key)
                 FROM play_sessions p
                 WHERE p.system_id=?1 AND p.id=(
                    SELECT q.id FROM play_sessions q
                    WHERE q.system_id=p.system_id AND q.stable_key=p.stable_key
                    ORDER BY q.played_at DESC,q.id DESC LIMIT 1
                 )
                 ORDER BY p.played_at DESC,p.id DESC LIMIT ?2",
            )
            .map_err(|error| format!("prepare recent games: {error}"))?;
        let rows = statement
            .query_map(
                params![system_id, i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| {
                    Ok(RecentGame {
                        game: UserGameIdentity {
                            system_id: row.get(0)?,
                            stable_key: row.get(1)?,
                            title: row.get(2)?,
                            launch_ref: row.get(3)?,
                            payload_path: row.get(4)?,
                        },
                        last_played_at: row.get(5)?,
                        play_count: row.get::<_, i64>(6)?.max(0) as u64,
                    })
                },
            )
            .map_err(|error| format!("query recent games: {error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("read recent games: {error}"))
    }

    pub fn mark_imported(
        &self,
        source: &str,
        version: u32,
        imported_at: i64,
    ) -> Result<(), String> {
        self.connection()?
            .execute(
                "INSERT INTO import_sources(source,version,imported_at) VALUES (?1,?2,?3)
                 ON CONFLICT(source) DO UPDATE SET
                    version=excluded.version,imported_at=excluded.imported_at",
                params![source, version, imported_at],
            )
            .map(|_| ())
            .map_err(|error| format!("mark import source: {error}"))
    }

    pub fn imported_version(&self, source: &str) -> Result<Option<u32>, String> {
        self.connection()?
            .query_row(
                "SELECT version FROM import_sources WHERE source=?1",
                [source],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("read import source: {error}"))
    }

    pub fn add_unresolved_import(&self, entry: &UnresolvedImport) -> Result<(), String> {
        self.connection()?
            .execute(
                "INSERT INTO unresolved_imports(source,kind,path,title,observed_at)
                 VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(source,kind,path) DO UPDATE SET
                    title=excluded.title,observed_at=excluded.observed_at",
                params![
                    entry.source,
                    entry.kind,
                    entry.path,
                    entry.title,
                    entry.observed_at,
                ],
            )
            .map(|_| ())
            .map_err(|error| format!("store unresolved import: {error}"))
    }

    fn connection(&self) -> Result<Connection, String> {
        let connection = Connection::open(&self.path)
            .map_err(|error| format!("open user-state {}: {error}", self.path.display()))?;
        connection
            .execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=1000;")
            .map_err(|error| format!("configure user-state: {error}"))?;
        Ok(connection)
    }
}

fn migrate(connection: &mut Connection) -> Result<(), String> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("read user-state schema: {error}"))?;
    if version > SCHEMA_VERSION {
        return Err(format!(
            "user-state schema {version} is newer than supported {SCHEMA_VERSION}"
        ));
    }
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    let transaction = connection
        .transaction()
        .map_err(|error| format!("begin user-state migration: {error}"))?;
    create_schema(&transaction)?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|error| format!("set user-state schema: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("commit user-state migration: {error}"))
}

fn create_schema(transaction: &Transaction<'_>) -> Result<(), String> {
    transaction
        .execute_batch(
            "CREATE TABLE favourites(
                system_id TEXT NOT NULL,
                stable_key TEXT NOT NULL,
                title TEXT NOT NULL,
                launch_ref TEXT NOT NULL,
                payload_path TEXT NOT NULL,
                favourited_at INTEGER NOT NULL,
                PRIMARY KEY(system_id,stable_key)
             ) WITHOUT ROWID;
             CREATE TABLE play_sessions(
                id INTEGER PRIMARY KEY,
                system_id TEXT NOT NULL,
                stable_key TEXT NOT NULL,
                title TEXT NOT NULL,
                launch_ref TEXT NOT NULL,
                payload_path TEXT NOT NULL,
                played_at INTEGER NOT NULL
             );
             CREATE INDEX play_sessions_recent
                ON play_sessions(system_id,played_at DESC,id DESC);
             CREATE TABLE import_sources(
                source TEXT PRIMARY KEY,
                version INTEGER NOT NULL,
                imported_at INTEGER NOT NULL
             ) WITHOUT ROWID;
             CREATE TABLE unresolved_imports(
                source TEXT NOT NULL,
                kind TEXT NOT NULL,
                path TEXT NOT NULL,
                title TEXT NOT NULL,
                observed_at INTEGER NOT NULL,
                PRIMARY KEY(source,kind,path)
             ) WITHOUT ROWID;",
        )
        .map_err(|error| format!("create user-state schema: {error}"))
}

fn count_for_system(
    connection: &Connection,
    table: &str,
    system_id: &str,
) -> Result<usize, String> {
    let sql = match table {
        "favourites" => "SELECT count(*) FROM favourites WHERE system_id=?1",
        _ => return Err("unsupported user-state count table".to_string()),
    };
    connection
        .query_row(sql, [system_id], |row| row.get::<_, i64>(0))
        .map(|count| usize::try_from(count.max(0)).unwrap_or(usize::MAX))
        .map_err(|error| format!("count {table}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_store(label: &str) -> UserStateStore {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        UserStateStore::open(std::env::temp_dir().join(format!(
            "mister-magik-user-state-{label}-{}-{nonce}.sqlite3",
            std::process::id()
        )))
        .unwrap()
    }

    fn game(key: &str) -> UserGameIdentity {
        UserGameIdentity {
            system_id: "snes".to_string(),
            stable_key: key.to_string(),
            title: format!("Game {key}"),
            launch_ref: format!("/games/SNES/{key}.sfc"),
            payload_path: format!("/games/SNES/{key}.sfc"),
        }
    }

    #[test]
    fn creates_schema_and_persists_favourites() {
        let store = temporary_store("favourites");
        let first = game("one");
        store.set_favourite(&first, true, 10).unwrap();
        assert!(store.is_favourite(&first).unwrap());
        assert_eq!(store.favourite_count("snes").unwrap(), 1);

        let reopened = UserStateStore::open(store.path()).unwrap();
        assert!(reopened.is_favourite(&first).unwrap());
        reopened.set_favourite(&first, false, 20).unwrap();
        assert!(!reopened.is_favourite(&first).unwrap());
    }

    #[test]
    fn retains_sessions_and_returns_unique_mru() {
        let store = temporary_store("recents");
        store.record_play(&game("one"), 10).unwrap();
        store.record_play(&game("two"), 20).unwrap();
        store.record_play(&game("one"), 30).unwrap();

        let recent = store.recent_unique("snes", 16).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].game.stable_key, "one");
        assert_eq!(recent[0].play_count, 2);
        assert_eq!(recent[0].last_played_at, 30);
        assert_eq!(recent[1].game.stable_key, "two");
    }

    #[test]
    fn tracks_import_versions_and_unresolved_rows_idempotently() {
        let store = temporary_store("imports");
        assert_eq!(store.imported_version("main-recents").unwrap(), None);
        store.mark_imported("main-recents", 1, 100).unwrap();
        assert_eq!(store.imported_version("main-recents").unwrap(), Some(1));
        let unresolved = UnresolvedImport {
            source: "legacy".to_string(),
            kind: "recent".to_string(),
            path: "/missing.sfc".to_string(),
            title: "Missing".to_string(),
            observed_at: 100,
        };
        store.add_unresolved_import(&unresolved).unwrap();
        store.add_unresolved_import(&unresolved).unwrap();
    }

    #[test]
    fn rejects_future_schema() {
        let store = temporary_store("future");
        let connection = Connection::open(store.path()).unwrap();
        connection.pragma_update(None, "user_version", 99).unwrap();
        drop(connection);
        assert!(UserStateStore::open(store.path()).is_err());
    }
}
