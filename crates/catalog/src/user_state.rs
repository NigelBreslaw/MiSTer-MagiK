// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Durable launcher-owned favourites and play history.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::HashMap;
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SystemUserCounts {
    pub recent: usize,
    pub favourites: usize,
}

/// One coherent read of durable user data. Publication never queries the game catalog.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserStateSnapshot {
    pub favourite_launch_refs: Vec<String>,
    pub recent_launch_refs: Vec<String>,
    pub favourites_by_system: HashMap<String, Vec<String>>,
    pub recent_by_system: HashMap<String, Vec<String>>,
    pub system_counts: HashMap<String, SystemUserCounts>,
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
        ensure_summaries(&mut connection)?;
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
        self.connection()?
            .query_row(
                "SELECT favourite_count FROM user_system_counts WHERE system_id=?1",
                [system_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map(|count| count.unwrap_or(0) as usize)
            .map_err(|e| format!("read favourite count: {e}"))
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
        let sql = "SELECT system_id,stable_key,title,launch_ref,payload_path,last_played_at,play_count FROM user_recent_games WHERE system_id=?1 ORDER BY last_played_at DESC,last_session_id DESC LIMIT ?2";
        let mut statement = connection
            .prepare(sql)
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

    pub fn read_snapshot(&self) -> Result<UserStateSnapshot, String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|e| format!("begin user snapshot: {e}"))?;
        let mut snapshot = UserStateSnapshot::default();
        let mut statement = transaction.prepare("SELECT system_id,launch_ref FROM favourites ORDER BY favourited_at DESC,stable_key")
            .map_err(|e| format!("prepare favourite snapshot: {e}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("query favourite snapshot: {e}"))?;
        for row in rows {
            let (system, reference) = row.map_err(|e| format!("read favourite snapshot: {e}"))?;
            snapshot.favourite_launch_refs.push(reference.clone());
            snapshot
                .favourites_by_system
                .entry(system)
                .or_default()
                .push(reference);
        }
        drop(statement);
        let mut statement = transaction
            .prepare("SELECT system_id,recent_count,favourite_count FROM user_system_counts")
            .map_err(|e| format!("prepare count snapshot: {e}"))?;
        for row in statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    SystemUserCounts {
                        recent: row.get::<_, i64>(1)? as usize,
                        favourites: row.get::<_, i64>(2)? as usize,
                    },
                ))
            })
            .map_err(|e| format!("query count snapshot: {e}"))?
        {
            let (system, counts) = row.map_err(|e| format!("read count snapshot: {e}"))?;
            snapshot.system_counts.insert(system, counts);
        }
        drop(statement);
        snapshot.recent_launch_refs = snapshot_recent_refs(&transaction, None)?;
        for (system, counts) in &snapshot.system_counts {
            if counts.recent != 0 {
                snapshot.recent_by_system.insert(
                    system.clone(),
                    snapshot_recent_refs(&transaction, Some(system))?,
                );
            }
        }
        transaction
            .commit()
            .map_err(|e| format!("finish user snapshot: {e}"))?;
        Ok(snapshot)
    }

    /// Refresh only the affected system after a committed favourite mutation.
    pub fn refresh_favourites(
        &self,
        snapshot: &mut UserStateSnapshot,
        system_id: &str,
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|e| format!("begin changed user snapshot: {e}"))?;
        let mut statement = transaction.prepare("SELECT launch_ref FROM favourites WHERE system_id=?1 ORDER BY favourited_at DESC,stable_key")
            .map_err(|e| format!("prepare changed favourites: {e}"))?;
        let references = statement
            .query_map([system_id], |row| row.get(0))
            .map_err(|e| format!("query changed favourites: {e}"))?
            .collect::<Result<Vec<String>, _>>()
            .map_err(|e| format!("read changed favourites: {e}"))?;
        drop(statement);
        snapshot
            .favourites_by_system
            .insert(system_id.to_owned(), references);
        snapshot.favourite_launch_refs = snapshot
            .favourites_by_system
            .values()
            .flatten()
            .cloned()
            .collect();
        let counts = transaction
            .query_row(
                "SELECT recent_count,favourite_count FROM user_system_counts WHERE system_id=?1",
                [system_id],
                |row| {
                    Ok(SystemUserCounts {
                        recent: row.get::<_, i64>(0)? as usize,
                        favourites: row.get::<_, i64>(1)? as usize,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("read changed user counts: {e}"))?
            .unwrap_or_default();
        snapshot.system_counts.insert(system_id.to_owned(), counts);
        transaction
            .commit()
            .map_err(|e| format!("finish changed user snapshot: {e}"))?;
        Ok(())
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

fn snapshot_recent_refs(
    connection: &Connection,
    system: Option<&str>,
) -> Result<Vec<String>, String> {
    let sql = if system.is_some() {
        "SELECT launch_ref FROM user_recent_games WHERE system_id=?1 ORDER BY last_played_at DESC,last_session_id DESC LIMIT 16"
    } else {
        "SELECT launch_ref FROM user_recent_games WHERE ?1 IS NULL ORDER BY last_played_at DESC,last_session_id DESC LIMIT 16"
    };
    let mut statement = connection
        .prepare(sql)
        .map_err(|e| format!("prepare recent refs: {e}"))?;
    statement
        .query_map([system], |row| row.get(0))
        .map_err(|e| format!("query recent refs: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("read recent refs: {e}"))
}

/// Additive derived tables keep schema-v1 writers compatible: their existing
/// INSERT/DELETE statements execute the same transactional maintenance triggers.
fn ensure_summaries(connection: &mut Connection) -> Result<(), String> {
    let ready: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='user_system_counts')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("check user summaries: {error}"))?;
    if ready {
        return Ok(());
    }
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("begin user summaries: {error}"))?;
    if transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='user_system_counts')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|e| format!("recheck user summaries: {e}"))?
    {
        return transaction
            .commit()
            .map_err(|e| format!("finish user summaries: {e}"));
    }
    transaction.execute_batch("CREATE TABLE user_recent_games(
        system_id TEXT NOT NULL,stable_key TEXT NOT NULL,title TEXT NOT NULL,
        launch_ref TEXT NOT NULL,payload_path TEXT NOT NULL,last_played_at INTEGER NOT NULL,
        last_session_id INTEGER NOT NULL,play_count INTEGER NOT NULL,
        PRIMARY KEY(system_id,stable_key)) WITHOUT ROWID;
        CREATE INDEX user_recent_system ON user_recent_games(system_id,last_played_at DESC,last_session_id DESC);
        CREATE INDEX user_recent_global ON user_recent_games(last_played_at DESC,last_session_id DESC);
        CREATE TABLE user_system_counts(system_id TEXT PRIMARY KEY,recent_count INTEGER NOT NULL DEFAULT 0,favourite_count INTEGER NOT NULL DEFAULT 0) WITHOUT ROWID;
        INSERT INTO user_recent_games
        SELECT system_id,stable_key,title,launch_ref,payload_path,played_at,id,plays FROM (
          SELECT *,count(*) OVER(PARTITION BY system_id,stable_key) AS plays,
          row_number() OVER(PARTITION BY system_id,stable_key ORDER BY played_at DESC,id DESC) AS rank FROM play_sessions) WHERE rank=1;
        INSERT INTO user_system_counts SELECT system_id,sum(recent_count),sum(favourite_count) FROM (
          SELECT system_id,count(*) AS recent_count,0 AS favourite_count FROM user_recent_games GROUP BY system_id
          UNION ALL SELECT system_id,0,count(*) FROM favourites GROUP BY system_id) GROUP BY system_id;
        CREATE TRIGGER user_recent_added AFTER INSERT ON user_recent_games BEGIN
          INSERT INTO user_system_counts(system_id,recent_count) VALUES(NEW.system_id,1)
          ON CONFLICT(system_id) DO UPDATE SET recent_count=recent_count+1;
        END;
        CREATE TRIGGER user_favourite_added AFTER INSERT ON favourites BEGIN
          INSERT INTO user_system_counts(system_id,favourite_count) VALUES(NEW.system_id,1)
          ON CONFLICT(system_id) DO UPDATE SET favourite_count=favourite_count+1;
        END;
        CREATE TRIGGER user_favourite_removed AFTER DELETE ON favourites BEGIN
          UPDATE user_system_counts SET favourite_count=favourite_count-1 WHERE system_id=OLD.system_id;
        END;
        CREATE TRIGGER user_play_recorded AFTER INSERT ON play_sessions BEGIN
          INSERT INTO user_recent_games(system_id,stable_key,title,launch_ref,payload_path,last_played_at,last_session_id,play_count)
          VALUES(NEW.system_id,NEW.stable_key,NEW.title,NEW.launch_ref,NEW.payload_path,NEW.played_at,NEW.id,1)
          ON CONFLICT(system_id,stable_key) DO UPDATE SET
            title=CASE WHEN NEW.played_at>=last_played_at THEN NEW.title ELSE title END,
            launch_ref=CASE WHEN NEW.played_at>=last_played_at THEN NEW.launch_ref ELSE launch_ref END,
            payload_path=CASE WHEN NEW.played_at>=last_played_at THEN NEW.payload_path ELSE payload_path END,
            last_session_id=CASE WHEN NEW.played_at>=last_played_at THEN NEW.id ELSE last_session_id END,
            last_played_at=max(last_played_at,NEW.played_at),play_count=play_count+1;
        END;")
        .map_err(|error| format!("create user summaries: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("commit user summaries: {error}"))
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
    fn launches_maintain_global_and_system_recents_without_recounting_history() {
        let store = temporary_store("maintained");
        let snes = game("one");
        let nes = UserGameIdentity {
            system_id: "nes".into(),
            launch_ref: "/games/NES/one.nes".into(),
            ..game("one")
        };
        store.record_play(&snes, 10).unwrap();
        store.record_play(&nes, 20).unwrap();
        store.record_play(&snes, 30).unwrap();
        store.record_play(&snes, 5).unwrap();
        let mut snapshot = store.read_snapshot().unwrap();
        assert_eq!(snapshot.system_counts["snes"].recent, 1);
        assert_eq!(snapshot.system_counts["nes"].recent, 1);
        assert_eq!(
            snapshot.recent_launch_refs,
            [snes.launch_ref.clone(), nes.launch_ref.clone()]
        );
        assert_eq!(snapshot.recent_by_system["snes"], [snes.launch_ref.clone()]);
        assert_eq!(snapshot.recent_by_system["nes"], [nes.launch_ref.clone()]);
        let recent = store.recent_unique("snes", 16).unwrap();
        assert_eq!(recent[0].play_count, 3);
        assert_eq!(recent[0].last_played_at, 30);
        store.set_favourite(&snes, true, 40).unwrap();
        store.set_favourite(&snes, true, 41).unwrap();
        store.refresh_favourites(&mut snapshot, "snes").unwrap();
        assert_eq!(snapshot.system_counts["snes"].favourites, 1);
        store.set_favourite(&nes, true, 42).unwrap();
        store.set_favourite(&snes, false, 43).unwrap();
        store.set_favourite(&snes, false, 44).unwrap();
        let reopened = UserStateStore::open(store.path())
            .unwrap()
            .read_snapshot()
            .unwrap();
        assert_eq!(reopened.system_counts["snes"].favourites, 0);
        assert_eq!(reopened.system_counts["nes"].favourites, 1);
        assert_eq!(reopened.favourite_launch_refs, [nes.launch_ref]);
    }

    #[test]
    fn legacy_database_backfill_preserves_history_and_schema_v1_writers() {
        let store = temporary_store("summary-upgrade");
        let connection = store.connection().unwrap();
        connection.execute_batch("DROP TRIGGER user_play_recorded; DROP TRIGGER user_recent_added; DROP TRIGGER user_favourite_added; DROP TRIGGER user_favourite_removed; DROP TABLE user_recent_games; DROP TABLE user_system_counts;").unwrap();
        store.record_play(&game("one"), 10).unwrap();
        store.record_play(&game("one"), 30).unwrap();
        store.record_play(&game("two"), 20).unwrap();
        store.set_favourite(&game("one"), true, 1).unwrap();
        let upgraded = UserStateStore::open(store.path()).unwrap();
        assert_eq!(
            upgraded.read_snapshot().unwrap().system_counts["snes"],
            SystemUserCounts {
                recent: 2,
                favourites: 1
            }
        );
        assert_eq!(upgraded.recent_unique("snes", 16).unwrap()[0].play_count, 2);
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        // An old writer's existing statement still updates all maintained state.
        connection.execute("INSERT INTO play_sessions(system_id,stable_key,title,launch_ref,payload_path,played_at) VALUES('nes','three','Three','three.nes','three.nes',40)", []).unwrap();
        assert_eq!(
            upgraded.read_snapshot().unwrap().system_counts["nes"].recent,
            1
        );
        assert_eq!(
            upgraded.read_snapshot().unwrap().recent_launch_refs[0],
            "three.nes"
        );
    }

    #[test]
    fn rolled_back_launch_cannot_publish_partial_recent_or_count_updates() {
        let store = temporary_store("rollback-summary");
        let mut connection = store.connection().unwrap();
        let transaction = connection.transaction().unwrap();
        transaction.execute("INSERT INTO play_sessions(system_id,stable_key,title,launch_ref,payload_path,played_at) VALUES('snes','one','One','one.sfc','one.sfc',10)", []).unwrap();
        drop(transaction);
        let snapshot = store.read_snapshot().unwrap();
        assert!(snapshot.recent_launch_refs.is_empty());
        assert!(snapshot.system_counts.is_empty());
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
