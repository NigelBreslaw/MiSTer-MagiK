// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Schema-v1 per-system SQLite and bounded navigation artifacts.

use crate::catalog_classify::SystemId;
use crate::sharded_catalog::{NAVIGATION_SCHEMA_VERSION, SHARD_SCHEMA_VERSION};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SystemShardLimits {
    pub max_sqlite_bytes: u64,
    pub max_navigation_compressed_bytes: usize,
    pub max_navigation_decoded_bytes: usize,
    pub max_games: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SystemGame {
    pub stable_key: String,
    pub title: String,
    pub launch_ref: String,
    pub preview_archive_path: String,
    pub preview_asset_key: String,
    pub has_preview: bool,
    pub year: Option<u16>,
    pub manufacturer: String,
    pub players: Option<u8>,
    pub control: String,
    pub is_new: bool,
    pub launch_plan: Option<SystemLaunchPlan>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SystemLaunchPlan {
    pub launch_ref: String,
    pub title: String,
    pub system_id: String,
    pub core_path: String,
    pub payload_path: String,
    pub mount_kind: String,
    pub mount_index: u8,
    pub delay_secs: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemShardData {
    pub system_id: SystemId,
    pub generation: u64,
    pub games: Vec<SystemGame>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedSystemShard {
    pub system_id: SystemId,
    pub generation: u64,
    pub navigation_hash: String,
    pub games: Vec<SystemGame>,
}

#[cfg(feature = "builder")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShardDurability {
    Immediate,
    Deferred,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredNavigation {
    schema_version: u32,
    system_id: String,
    generation: u64,
    games: Vec<SystemGame>,
}

#[cfg(feature = "builder")]
pub fn write_system_shard(
    sqlite_path: &Path,
    navigation_path: &Path,
    data: &SystemShardData,
    limits: SystemShardLimits,
) -> Result<LoadedSystemShard, SystemShardError> {
    write_system_shard_with_durability(
        sqlite_path,
        navigation_path,
        data,
        limits,
        ShardDurability::Immediate,
    )
}

#[cfg(feature = "builder")]
pub(crate) fn write_system_shard_with_durability(
    sqlite_path: &Path,
    navigation_path: &Path,
    data: &SystemShardData,
    limits: SystemShardLimits,
    durability: ShardDurability,
) -> Result<LoadedSystemShard, SystemShardError> {
    validate_games(&data.games, limits.max_games)?;
    let stored = StoredNavigation {
        schema_version: NAVIGATION_SCHEMA_VERSION,
        system_id: data.system_id.as_str().to_string(),
        generation: data.generation,
        games: data.games.to_vec(),
    };
    let navigation = encode_navigation(&stored, limits)?;
    let navigation_hash = checksum_hex(&navigation);
    create_parent(sqlite_path)?;
    create_parent(navigation_path)?;
    if sqlite_path.exists() || navigation_path.exists() {
        return Err(SystemShardError::new(
            "write",
            "staging artifact already exists",
        ));
    }

    let mut connection = Connection::open(sqlite_path)
        .map_err(|error| SystemShardError::with("open staging SQLite", error))?;
    let durability_pragmas = match durability {
        ShardDurability::Immediate => "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;",
        ShardDurability::Deferred => "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;",
    };
    connection
        .execute_batch(durability_pragmas)
        .map_err(|error| SystemShardError::with("configure shard durability", error))?;
    connection
        .execute_batch(
            "PRAGMA page_size=4096;
             PRAGMA cache_size=-65536;
             PRAGMA temp_store=MEMORY;
             PRAGMA locking_mode=EXCLUSIVE;
             CREATE TABLE shard_meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             ) WITHOUT ROWID;
             CREATE TABLE games (
                 stable_key TEXT PRIMARY KEY,
                 ordinal INTEGER NOT NULL UNIQUE,
                 title TEXT NOT NULL,
                 launch_ref TEXT NOT NULL,
                 preview_archive_path TEXT NOT NULL,
                 preview_asset_key TEXT NOT NULL,
                 has_preview INTEGER NOT NULL,
                 year INTEGER,
                 manufacturer TEXT NOT NULL,
                 players INTEGER,
                 control TEXT NOT NULL,
                 is_new INTEGER NOT NULL,
                 launch_plan_json TEXT
             ) WITHOUT ROWID;
             CREATE TABLE navigation_payload (
                 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                 payload BLOB NOT NULL
             );",
        )
        .map_err(|error| SystemShardError::with("create shard schema", error))?;
    let generation = i64::try_from(data.generation)
        .map_err(|_| SystemShardError::new("write", "generation exceeds SQLite integer"))?;
    let transaction = connection
        .transaction()
        .map_err(|error| SystemShardError::with("begin shard transaction", error))?;
    for (key, value) in [
        ("schema_version", SHARD_SCHEMA_VERSION.to_string()),
        (
            "navigation_schema_version",
            NAVIGATION_SCHEMA_VERSION.to_string(),
        ),
        ("system_id", data.system_id.as_str().to_string()),
        ("generation", generation.to_string()),
        ("game_count", data.games.len().to_string()),
        ("navigation_hash", navigation_hash.clone()),
    ] {
        transaction
            .execute(
                "INSERT INTO shard_meta(key,value) VALUES (?1,?2)",
                rusqlite::params![key, value],
            )
            .map_err(|error| SystemShardError::with("insert shard metadata", error))?;
    }
    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO games(
                    stable_key,ordinal,title,launch_ref,preview_archive_path,
                    preview_asset_key,has_preview,year,manufacturer,players,
                    control,is_new,launch_plan_json
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            )
            .map_err(|error| SystemShardError::with("prepare shard games", error))?;
        let mut insertion_order = (0..data.games.len()).collect::<Vec<_>>();
        insertion_order.sort_unstable_by(|left, right| {
            data.games[*left]
                .stable_key
                .cmp(&data.games[*right].stable_key)
        });
        for ordinal in insertion_order {
            let game = &data.games[ordinal];
            statement
                .execute(rusqlite::params![
                    game.stable_key,
                    i64::try_from(ordinal).map_err(|_| SystemShardError::new(
                        "write",
                        "game ordinal exceeds SQLite integer"
                    ))?,
                    game.title,
                    game.launch_ref,
                    game.preview_archive_path,
                    game.preview_asset_key,
                    i64::from(game.has_preview),
                    game.year.map(i64::from),
                    game.manufacturer,
                    game.players.map(i64::from),
                    game.control,
                    i64::from(game.is_new),
                    game.launch_plan
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|error| SystemShardError::with("encode launch plan", error))?,
                ])
                .map_err(|error| SystemShardError::with("insert shard game", error))?;
        }
    }
    transaction
        .execute(
            "INSERT INTO navigation_payload(singleton,payload) VALUES (1,?1)",
            [&navigation],
        )
        .map_err(|error| SystemShardError::with("insert embedded navigation", error))?;
    transaction
        .commit()
        .map_err(|error| SystemShardError::with("commit shard", error))?;
    drop(connection);
    fs::write(navigation_path, &navigation)
        .map_err(|error| SystemShardError::with("write adjacent navigation", error))?;
    if durability == ShardDurability::Immediate {
        fs::File::open(sqlite_path)
            .and_then(|file| file.sync_all())
            .map_err(|error| SystemShardError::with("sync shard SQLite", error))?;
        fs::File::open(navigation_path)
            .and_then(|file| file.sync_all())
            .map_err(|error| SystemShardError::with("sync shard navigation", error))?;
    }
    open_system_shard(
        sqlite_path,
        navigation_path,
        &data.system_id,
        data.generation,
        limits,
    )
}

pub fn open_system_shard(
    sqlite_path: &Path,
    navigation_path: &Path,
    expected_system_id: &SystemId,
    expected_generation: u64,
    limits: SystemShardLimits,
) -> Result<LoadedSystemShard, SystemShardError> {
    let sqlite_size = fs::metadata(sqlite_path)
        .map_err(|error| SystemShardError::with("stat shard SQLite", error))?
        .len();
    if sqlite_size > limits.max_sqlite_bytes {
        return Err(SystemShardError::new(
            "read",
            "shard SQLite exceeds configured size limit",
        ));
    }
    let connection = Connection::open_with_flags(
        sqlite_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| SystemShardError::with("open shard SQLite read-only", error))?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|error| SystemShardError::with("set shard query-only", error))?;
    let schema_version = meta_u64(&connection, "schema_version")?;
    if schema_version != u64::from(SHARD_SCHEMA_VERSION) {
        return Err(SystemShardError::new(
            "read",
            "unsupported shard schema version",
        ));
    }
    let navigation_schema = meta_u64(&connection, "navigation_schema_version")?;
    if navigation_schema != u64::from(NAVIGATION_SCHEMA_VERSION) {
        return Err(SystemShardError::new(
            "read",
            "unsupported navigation schema version",
        ));
    }
    let system_id = SystemId::parse(&meta_text(&connection, "system_id")?)
        .map_err(|error| SystemShardError::new("read", error.to_string()))?;
    let generation = meta_u64(&connection, "generation")?;
    let game_count = usize::try_from(meta_u64(&connection, "game_count")?)
        .map_err(|_| SystemShardError::new("read", "game count exceeds platform size"))?;
    if game_count > limits.max_games {
        return Err(SystemShardError::new(
            "read",
            "shard game count exceeds configured limit",
        ));
    }
    if &system_id != expected_system_id || generation != expected_generation {
        return Err(SystemShardError::new(
            "read",
            "shard identity or generation does not match registry",
        ));
    }
    let stored_hash = meta_text(&connection, "navigation_hash")?;
    let navigation = read_bounded(navigation_path, limits.max_navigation_compressed_bytes)?;
    if checksum_hex(&navigation) != stored_hash {
        return Err(SystemShardError::new(
            "read",
            "adjacent navigation hash does not match shard",
        ));
    }
    let stored = decode_navigation(&navigation, limits)?;
    if stored.schema_version != NAVIGATION_SCHEMA_VERSION
        || stored.system_id != system_id.as_str()
        || stored.generation != generation
        || stored.games.len() != game_count
    {
        return Err(SystemShardError::new(
            "read",
            "navigation identity or count does not match shard",
        ));
    }
    let embedded: Vec<u8> = connection
        .query_row(
            "SELECT payload FROM navigation_payload WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| SystemShardError::with("read embedded navigation", error))?;
    if embedded != navigation {
        return Err(SystemShardError::new(
            "read",
            "embedded and adjacent navigation differ",
        ));
    }
    let mut statement = connection
        .prepare(
            "SELECT stable_key,title,launch_ref,preview_archive_path,
                    preview_asset_key,has_preview,year,manufacturer,players,
                    control,is_new,launch_plan_json
             FROM games ORDER BY ordinal",
        )
        .map_err(|error| SystemShardError::with("prepare canonical shard games", error))?;
    let canonical = statement
        .query_map([], |row| {
            let launch_plan_json: Option<String> = row.get(11)?;
            let launch_plan = launch_plan_json
                .map(|encoded| serde_json::from_str(&encoded))
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        11,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            Ok(SystemGame {
                stable_key: row.get(0)?,
                title: row.get(1)?,
                launch_ref: row.get(2)?,
                preview_archive_path: row.get(3)?,
                preview_asset_key: row.get(4)?,
                has_preview: row.get(5)?,
                year: row.get(6)?,
                manufacturer: row.get(7)?,
                players: row.get(8)?,
                control: row.get(9)?,
                is_new: row.get(10)?,
                launch_plan,
            })
        })
        .map_err(|error| SystemShardError::with("query canonical shard games", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| SystemShardError::with("read canonical shard game", error))?;
    if canonical != stored.games {
        return Err(SystemShardError::new(
            "read",
            "canonical and navigation games differ",
        ));
    }
    Ok(LoadedSystemShard {
        system_id,
        generation,
        navigation_hash: stored_hash,
        games: stored.games,
    })
}

/// Open the compact adjacent navigation without touching SQLite. This is the
/// launcher hot reader; full shard parity remains a builder/recovery concern.
pub fn open_system_navigation(
    navigation_path: &Path,
    expected_system_id: &SystemId,
    expected_generation: u64,
    limits: SystemShardLimits,
) -> Result<LoadedSystemShard, SystemShardError> {
    let navigation = read_bounded(navigation_path, limits.max_navigation_compressed_bytes)?;
    let navigation_hash = checksum_hex(&navigation);
    let stored = decode_navigation(&navigation, limits)?;
    if stored.schema_version != NAVIGATION_SCHEMA_VERSION
        || stored.system_id != expected_system_id.as_str()
        || stored.generation != expected_generation
    {
        return Err(SystemShardError::new(
            "read",
            "navigation identity or generation does not match registry",
        ));
    }
    let games = stored.games;
    validate_loaded_games(&games, limits.max_games)?;
    Ok(LoadedSystemShard {
        system_id: expected_system_id.clone(),
        generation: expected_generation,
        navigation_hash,
        games,
    })
}

#[cfg(feature = "builder")]
fn validate_games(games: &[SystemGame], max_games: usize) -> Result<(), SystemShardError> {
    if games.len() > max_games {
        return Err(SystemShardError::new(
            "write",
            "system game count exceeds configured limit",
        ));
    }
    let mut keys = BTreeSet::new();
    for game in games {
        if game.stable_key.is_empty()
            || game.title.is_empty()
            || game.launch_ref.is_empty()
            || !keys.insert(&game.stable_key)
        {
            return Err(SystemShardError::new(
                "write",
                "games need non-empty unique keys, titles, and launch references",
            ));
        }
    }
    Ok(())
}

fn validate_loaded_games(games: &[SystemGame], max_games: usize) -> Result<(), SystemShardError> {
    if games.len() > max_games {
        return Err(SystemShardError::new(
            "read",
            "system game count exceeds configured limit",
        ));
    }
    let mut keys = BTreeSet::new();
    if games.iter().any(|game| {
        game.stable_key.is_empty()
            || game.title.is_empty()
            || game.launch_ref.is_empty()
            || !keys.insert(&game.stable_key)
    }) {
        return Err(SystemShardError::new(
            "read",
            "navigation contains invalid or duplicate game rows",
        ));
    }
    Ok(())
}

#[cfg(feature = "builder")]
fn encode_navigation(
    stored: &StoredNavigation,
    limits: SystemShardLimits,
) -> Result<Vec<u8>, SystemShardError> {
    let decoded = serde_json::to_vec(stored)
        .map_err(|error| SystemShardError::with("encode navigation", error))?;
    if decoded.len() > limits.max_navigation_decoded_bytes {
        return Err(SystemShardError::new(
            "write",
            "decoded navigation exceeds configured limit",
        ));
    }
    let encoded = lz4_flex::compress_prepend_size(&decoded);
    if encoded.len() > limits.max_navigation_compressed_bytes {
        return Err(SystemShardError::new(
            "write",
            "compressed navigation exceeds configured limit",
        ));
    }
    Ok(encoded)
}

fn decode_navigation(
    encoded: &[u8],
    limits: SystemShardLimits,
) -> Result<StoredNavigation, SystemShardError> {
    let prefix = encoded
        .get(..4)
        .ok_or_else(|| SystemShardError::new("read", "navigation header is truncated"))?;
    let decoded_len = u32::from_le_bytes(prefix.try_into().expect("four-byte prefix")) as usize;
    if decoded_len > limits.max_navigation_decoded_bytes {
        return Err(SystemShardError::new(
            "read",
            "decoded navigation length exceeds configured limit",
        ));
    }
    let decoded = lz4_flex::decompress_size_prepended(encoded)
        .map_err(|error| SystemShardError::with("decode navigation", error))?;
    if decoded.len() != decoded_len {
        return Err(SystemShardError::new(
            "read",
            "decoded navigation length mismatch",
        ));
    }
    serde_json::from_slice(&decoded)
        .map_err(|error| SystemShardError::with("parse navigation", error))
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, SystemShardError> {
    let size = fs::metadata(path)
        .map_err(|error| SystemShardError::with("stat shard navigation", error))?
        .len();
    if size > max_bytes as u64 {
        return Err(SystemShardError::new(
            "read",
            "compressed navigation exceeds configured limit",
        ));
    }
    fs::read(path).map_err(|error| SystemShardError::with("read shard navigation", error))
}

fn meta_text(connection: &Connection, key: &str) -> Result<String, SystemShardError> {
    connection
        .query_row("SELECT value FROM shard_meta WHERE key=?1", [key], |row| {
            row.get(0)
        })
        .map_err(|error| SystemShardError::with("read shard metadata", error))
}

fn meta_u64(connection: &Connection, key: &str) -> Result<u64, SystemShardError> {
    meta_text(connection, key)?
        .parse()
        .map_err(|_| SystemShardError::new("read", "invalid numeric shard metadata"))
}

pub(crate) fn checksum_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(feature = "builder")]
fn create_parent(path: &Path) -> Result<(), SystemShardError> {
    let parent = path
        .parent()
        .ok_or_else(|| SystemShardError::new("write", "artifact path has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|error| SystemShardError::with("create artifact directory", error))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemShardError {
    stage: &'static str,
    message: String,
}

impl SystemShardError {
    fn new(stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }

    fn with(stage: &'static str, error: impl fmt::Display) -> Self {
        Self::new(stage, error.to_string())
    }

    pub fn stage(&self) -> &'static str {
        self.stage
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SystemShardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.stage, self.message)
    }
}

impl Error for SystemShardError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    #[cfg(feature = "builder")]
    fn schema_one_shard_round_trips_and_matches_navigation() {
        let root = temporary_root("round-trip");
        let sqlite = root.join("1.sqlite3");
        let navigation = root.join("1.nav.lz4b");
        let data = fixture_data();
        let loaded = write_system_shard(&sqlite, &navigation, &data, limits()).unwrap();
        assert_eq!(loaded.system_id, data.system_id);
        assert_eq!(loaded.generation, 1);
        assert_eq!(loaded.games, data.games);
        assert_eq!(loaded.navigation_hash.len(), 16);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn reader_rejects_schema_mismatch_and_corrupt_navigation() {
        let root = temporary_root("corrupt");
        let sqlite = root.join("1.sqlite3");
        let navigation = root.join("1.nav.lz4b");
        let data = fixture_data();
        write_system_shard(&sqlite, &navigation, &data, limits()).unwrap();
        let connection = Connection::open(&sqlite).unwrap();
        connection
            .execute(
                "UPDATE shard_meta SET value='2' WHERE key='schema_version'",
                [],
            )
            .unwrap();
        drop(connection);
        assert_eq!(
            open_system_shard(&sqlite, &navigation, &data.system_id, 1, limits())
                .unwrap_err()
                .message(),
            "unsupported shard schema version"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn reader_rejects_corrupt_adjacent_navigation() {
        let root = temporary_root("corrupt-navigation");
        let sqlite = root.join("1.sqlite3");
        let navigation = root.join("1.nav.lz4b");
        let data = fixture_data();
        write_system_shard(&sqlite, &navigation, &data, limits()).unwrap();
        fs::write(&navigation, b"corrupt").unwrap();
        assert_eq!(
            open_system_shard(&sqlite, &navigation, &data.system_id, 1, limits())
                .unwrap_err()
                .message(),
            "adjacent navigation hash does not match shard"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn reader_rejects_canonical_rows_that_differ_from_navigation() {
        let root = temporary_root("canonical-mismatch");
        let sqlite = root.join("1.sqlite3");
        let navigation = root.join("1.nav.lz4b");
        let data = fixture_data();
        write_system_shard(&sqlite, &navigation, &data, limits()).unwrap();
        let connection = Connection::open(&sqlite).unwrap();
        connection
            .execute(
                "UPDATE games SET title='Tampered' WHERE stable_key='one'",
                [],
            )
            .unwrap();
        drop(connection);
        assert_eq!(
            open_system_shard(&sqlite, &navigation, &data.system_id, 1, limits())
                .unwrap_err()
                .message(),
            "canonical and navigation games differ"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn navigation_rejects_oversized_decoded_prefix_before_allocation() {
        let encoded = (10_000u32).to_le_bytes().to_vec();
        let error = decode_navigation(
            &encoded,
            SystemShardLimits {
                max_navigation_decoded_bytes: 100,
                ..limits()
            },
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "decoded navigation length exceeds configured limit"
        );
    }

    fn fixture_data() -> SystemShardData {
        SystemShardData {
            system_id: SystemId::parse("snes").unwrap(),
            generation: 1,
            games: vec![
                SystemGame {
                    stable_key: "one".to_string(),
                    title: "Synthetic One".to_string(),
                    launch_ref: "/games/SNES/One.sfc".to_string(),
                    preview_archive_path: "/media/preview.zip".to_string(),
                    preview_asset_key: "Synthetic One".to_string(),
                    has_preview: true,
                    year: Some(1992),
                    manufacturer: "Fixture Corp".to_string(),
                    players: Some(2),
                    control: "Gamepad".to_string(),
                    is_new: true,
                    launch_plan: Some(SystemLaunchPlan {
                        launch_ref: "magik-plan:one".to_string(),
                        title: "Synthetic One".to_string(),
                        system_id: "snes".to_string(),
                        core_path: "SNES".to_string(),
                        payload_path: "/games/SNES/One.sfc".to_string(),
                        mount_kind: "load-file".to_string(),
                        mount_index: 0,
                        delay_secs: 1,
                    }),
                },
                SystemGame {
                    stable_key: "two".to_string(),
                    title: "Synthetic Two".to_string(),
                    launch_ref: "/games/SNES/Two.sfc".to_string(),
                    ..SystemGame::default()
                },
            ],
        }
    }

    fn limits() -> SystemShardLimits {
        SystemShardLimits {
            max_sqlite_bytes: 2 * 1024 * 1024,
            max_navigation_compressed_bytes: 256 * 1024,
            max_navigation_decoded_bytes: 1024 * 1024,
            max_games: 100,
        }
    }

    fn temporary_root(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mister-magik-system-shard-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
}
