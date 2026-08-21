// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Schema-v3 per-system SQLite and bounded navigation artifacts.

use crate::catalog_classify::SystemId;
use crate::sharded_catalog::{NAVIGATION_SCHEMA_VERSION, SHARD_SCHEMA_VERSION};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;
#[cfg(all(feature = "builder", target_os = "linux"))]
use std::time::Instant;

const NAVIGATION_HEADER_MAX_BYTES: usize = 256;
const NAVIGATION_SCHEMA_KEY: &[u8] = b"schema_version";

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
    pub category: String,
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
    pub projection_stats: Option<SystemShardProjectionStats>,
    pub games: Vec<SystemGame>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SystemShardProjectionStats {
    pub source_games: usize,
    pub visible_families: usize,
    pub collapsed_variants: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedSystemShard {
    pub system_id: SystemId,
    pub generation: u64,
    pub navigation_hash: String,
    pub projection_stats: Option<SystemShardProjectionStats>,
    pub navigation_indexes: SystemNavigationIndexes,
    pub games: Vec<SystemGame>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SystemNavigationIndexes {
    pub title_ordinals: Vec<u32>,
    pub preview_ordinals: Vec<u32>,
    pub launch_ordinals: Vec<u32>,
    pub categories: Vec<(String, Vec<u32>)>,
    pub decades: Vec<(u16, Vec<u32>)>,
    pub manufacturers: Vec<(String, Vec<u32>)>,
    pub players: Vec<(u8, Vec<u32>)>,
    pub controls: Vec<(String, Vec<u32>)>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SystemNavigationOpenTiming {
    pub read_us: u64,
    pub hash_us: u64,
    pub decompress_us: u64,
    pub envelope_parse_us: u64,
    pub typed_parse_us: u64,
    pub validation_us: u64,
    pub compressed_bytes: u64,
    pub decoded_bytes: u64,
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
    indexes: SystemNavigationIndexes,
    games: Vec<SystemGame>,
}

#[cfg(feature = "builder")]
#[derive(Serialize)]
struct StoredNavigationRef<'a> {
    schema_version: u32,
    system_id: &'a str,
    generation: u64,
    indexes: &'a SystemNavigationIndexes,
    games: &'a [SystemGame],
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
        data.clone(),
        limits,
        ShardDurability::Immediate,
    )
}

pub fn navpack_path_for_navigation(navigation_path: &Path) -> std::path::PathBuf {
    let filename = navigation_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("navigation.nav.lz4b");
    let stem = filename.strip_suffix(".nav.lz4b").unwrap_or(filename);
    navigation_path.with_file_name(format!("{stem}.navpack"))
}

#[cfg(feature = "builder")]
pub(crate) fn write_system_shard_with_durability(
    sqlite_path: &Path,
    navigation_path: &Path,
    data: SystemShardData,
    limits: SystemShardLimits,
    durability: ShardDurability,
) -> Result<LoadedSystemShard, SystemShardError> {
    validate_games(&data.games, limits.max_games)?;
    let navigation_indexes = build_navigation_indexes(&data.games)?;
    let navigation_pmu = mister_magik_perf_events::sampled_span(crate::pmu_phase::SHARD_NAVIGATION);
    let navigation = {
        let stored = StoredNavigationRef {
            schema_version: NAVIGATION_SCHEMA_VERSION,
            system_id: data.system_id.as_str(),
            generation: data.generation,
            indexes: &navigation_indexes,
            games: &data.games,
        };
        encode_navigation(&stored, limits)?
    };
    let navigation_hash = checksum_hex(&navigation);
    let navpack = crate::navpack::encode(
        data.system_id.as_str(),
        data.generation,
        &data.games,
        &navigation_indexes,
    )
    .map_err(|error| SystemShardError::new("write NavPack", error))?;
    let navpack_path = navpack_path_for_navigation(navigation_path);
    let preview_archive_default = common_preview_archive_path(&data.games);
    drop(navigation_pmu);
    create_parent(sqlite_path)?;
    create_parent(navigation_path)?;
    if sqlite_path.exists() || navigation_path.exists() || navpack_path.exists() {
        return Err(SystemShardError::new(
            "write",
            "staging artifact already exists",
        ));
    }

    let sqlite_schema_pmu =
        mister_magik_perf_events::sampled_span(crate::pmu_phase::SHARD_SQLITE_SCHEMA);
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
             PRAGMA cache_size=-2048;
             PRAGMA temp_store=FILE;
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
                 preview_archive_path TEXT,
                 preview_asset_key TEXT NOT NULL,
                 has_preview INTEGER NOT NULL,
                 year INTEGER,
                 manufacturer TEXT NOT NULL,
                 category TEXT NOT NULL,
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
    crate::persisted_search::create_schema(&connection)
        .map_err(|error| SystemShardError::new("write", error.to_string()))?;
    let generation = i64::try_from(data.generation)
        .map_err(|_| SystemShardError::new("write", "generation exceeds SQLite integer"))?;
    let transaction = connection
        .transaction()
        .map_err(|error| SystemShardError::with("begin shard transaction", error))?;
    drop(sqlite_schema_pmu);
    let games_pmu = mister_magik_perf_events::sampled_span(crate::pmu_phase::SHARD_GAMES);
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
        ("preview_archive_path", preview_archive_default.clone()),
    ] {
        transaction
            .execute(
                "INSERT INTO shard_meta(key,value) VALUES (?1,?2)",
                rusqlite::params![key, value],
            )
            .map_err(|error| SystemShardError::with("insert shard metadata", error))?;
    }
    if let Some(stats) = data.projection_stats {
        for (key, value) in [
            ("source_game_count", stats.source_games),
            ("visible_family_count", stats.visible_families),
            ("collapsed_variant_count", stats.collapsed_variants),
        ] {
            transaction
                .execute(
                    "INSERT INTO shard_meta(key,value) VALUES (?1,?2)",
                    rusqlite::params![key, value.to_string()],
                )
                .map_err(|error| {
                    SystemShardError::with("insert shard projection metadata", error)
                })?;
        }
    }
    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO games(
                    stable_key,ordinal,title,launch_ref,preview_archive_path,
                    preview_asset_key,has_preview,year,manufacturer,category,players,
                    control,is_new,launch_plan_json
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
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
                    (game.preview_archive_path != preview_archive_default)
                        .then_some(game.preview_archive_path.as_str()),
                    game.preview_asset_key,
                    i64::from(game.has_preview),
                    game.year.map(i64::from),
                    game.manufacturer,
                    game.category,
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
    drop(games_pmu);
    let search_index_pmu =
        mister_magik_perf_events::sampled_span(crate::pmu_phase::SHARD_SEARCH_INDEX);
    let search = crate::persisted_search::populate(&transaction, &data.games)
        .map_err(|error| SystemShardError::new("write", error.to_string()))?;
    crate::catalog_logln!(
        "catalog_search_build_tsv\tsystem={}\tdocuments={}\twords={}\tbatches={}\tdocument_build_us={}\tfts_insert_us={}\tpipeline_wait_us={}\trow_loop_us={}\tautocomplete_sort_us={}\tautocomplete_insert_us={}\toptimize_us={}\tautomerge_restore_us={}\tintegrity_us={}\ttotal_us={}",
        data.system_id.as_str(),
        data.games.len(),
        search.words,
        search.batches,
        search.document_build_us,
        search.fts_insert_us,
        search.pipeline_wait_us,
        search.row_loop_us,
        search.autocomplete_sort_us,
        search.autocomplete_insert_us,
        search.optimize_us,
        search.automerge_restore_us,
        search.integrity_us,
        search.total_us,
    );
    for (key, value) in [
        (
            "search_schema_version",
            crate::persisted_search::SEARCH_SCHEMA_VERSION.to_string(),
        ),
        ("search_document_count", data.games.len().to_string()),
        ("autocomplete_word_count", search.words.to_string()),
    ] {
        transaction
            .execute(
                "INSERT INTO shard_meta(key,value) VALUES (?1,?2)",
                rusqlite::params![key, value],
            )
            .map_err(|error| SystemShardError::with("insert search metadata", error))?;
    }
    drop(search_index_pmu);
    let commit_pmu = mister_magik_perf_events::sampled_span(crate::pmu_phase::SHARD_COMMIT);
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
    fs::write(&navpack_path, &navpack)
        .map_err(|error| SystemShardError::with("write adjacent NavPack", error))?;
    if durability == ShardDurability::Immediate {
        fs::File::open(sqlite_path)
            .and_then(|file| file.sync_all())
            .map_err(|error| SystemShardError::with("sync shard SQLite", error))?;
        fs::File::open(navigation_path)
            .and_then(|file| file.sync_all())
            .map_err(|error| SystemShardError::with("sync shard navigation", error))?;
        fs::File::open(&navpack_path)
            .and_then(|file| file.sync_all())
            .map_err(|error| SystemShardError::with("sync shard NavPack", error))?;
    }
    drop(commit_pmu);
    let system_id = data.system_id.clone();
    let generation = data.generation;
    drop(data);
    #[cfg(target_os = "linux")]
    {
        let trim_started = Instant::now();
        let trim_pmu =
            mister_magik_perf_events::sampled_span(crate::pmu_phase::SHARD_ALLOCATOR_TRIM);
        let released = unsafe { libc::malloc_trim(0) };
        drop(trim_pmu);
        crate::catalog_logln!(
            "catalog_shard_allocator_trim_tsv\tsystem={}\telapsed_us={}\treleased={released}",
            system_id,
            trim_started.elapsed().as_micros(),
        );
    }
    let validate_pmu = mister_magik_perf_events::sampled_span(crate::pmu_phase::SHARD_VALIDATE);
    let loaded = open_system_shard(sqlite_path, navigation_path, &system_id, generation, limits);
    drop(validate_pmu);
    loaded
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
        return Err(SystemShardError::unsupported_schema(
            "shard",
            u64::from(SHARD_SCHEMA_VERSION),
            schema_version,
            expected_system_id,
            expected_generation,
        ));
    }
    let navigation_schema = meta_u64(&connection, "navigation_schema_version")?;
    if navigation_schema != u64::from(NAVIGATION_SCHEMA_VERSION) {
        return Err(SystemShardError::unsupported_schema(
            "navigation",
            u64::from(NAVIGATION_SCHEMA_VERSION),
            navigation_schema,
            expected_system_id,
            expected_generation,
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
    crate::persisted_search::validate(&connection, game_count)
        .map_err(|error| SystemShardError::new("read", error.to_string()))?;
    let stored_hash = meta_text(&connection, "navigation_hash")?;
    let preview_archive_default = meta_text(&connection, "preview_archive_path")?;
    let projection_stats = optional_projection_stats(&connection, game_count)?;
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
    let embedded_matches: bool = connection
        .query_row(
            "SELECT payload = ?1 FROM navigation_payload WHERE singleton=1",
            [&navigation],
            |row| row.get(0),
        )
        .map_err(|error| SystemShardError::with("compare embedded navigation", error))?;
    if !embedded_matches {
        return Err(SystemShardError::new(
            "read",
            "embedded and adjacent navigation differ",
        ));
    }
    let mut statement = connection
        .prepare(
            "SELECT stable_key,title,launch_ref,preview_archive_path,
                    preview_asset_key,has_preview,year,manufacturer,category,players,
                    control,is_new,launch_plan_json
             FROM games ORDER BY ordinal",
        )
        .map_err(|error| SystemShardError::with("prepare canonical shard games", error))?;
    let mut rows = statement
        .query([])
        .map_err(|error| SystemShardError::with("query canonical shard games", error))?;
    for expected in &stored.games {
        let row = rows
            .next()
            .map_err(|error| SystemShardError::with("read canonical shard game", error))?
            .ok_or_else(|| {
                SystemShardError::new("read", "canonical shard has fewer games than navigation")
            })?;
        let launch_plan_json: Option<String> = row
            .get(12)
            .map_err(|error| SystemShardError::with("read canonical launch plan", error))?;
        let launch_plan = launch_plan_json
            .map(|encoded| serde_json::from_str(&encoded))
            .transpose()
            .map_err(|error| SystemShardError::with("decode canonical launch plan", error))?;
        let canonical = SystemGame {
            stable_key: row
                .get(0)
                .map_err(|error| SystemShardError::with("read stable key", error))?,
            title: row
                .get(1)
                .map_err(|error| SystemShardError::with("read title", error))?,
            launch_ref: row
                .get(2)
                .map_err(|error| SystemShardError::with("read launch ref", error))?,
            preview_archive_path: row
                .get::<_, Option<String>>(3)
                .map_err(|error| SystemShardError::with("read preview archive", error))?
                .unwrap_or_else(|| preview_archive_default.clone()),
            preview_asset_key: row
                .get(4)
                .map_err(|error| SystemShardError::with("read preview key", error))?,
            has_preview: row
                .get(5)
                .map_err(|error| SystemShardError::with("read preview flag", error))?,
            year: row
                .get(6)
                .map_err(|error| SystemShardError::with("read year", error))?,
            manufacturer: row
                .get(7)
                .map_err(|error| SystemShardError::with("read manufacturer", error))?,
            category: row
                .get(8)
                .map_err(|error| SystemShardError::with("read category", error))?,
            players: row
                .get(9)
                .map_err(|error| SystemShardError::with("read players", error))?,
            control: row
                .get(10)
                .map_err(|error| SystemShardError::with("read control", error))?,
            is_new: row
                .get(11)
                .map_err(|error| SystemShardError::with("read new flag", error))?,
            launch_plan,
        };
        if &canonical != expected {
            return Err(SystemShardError::new(
                "read",
                "canonical and navigation games differ",
            ));
        }
    }
    if rows
        .next()
        .map_err(|error| SystemShardError::with("read trailing canonical game", error))?
        .is_some()
    {
        return Err(SystemShardError::new(
            "read",
            "canonical shard has more games than navigation",
        ));
    }
    Ok(LoadedSystemShard {
        system_id,
        generation,
        navigation_hash: stored_hash,
        projection_stats,
        navigation_indexes: stored.indexes,
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
    open_system_navigation_with_timing(
        navigation_path,
        expected_system_id,
        expected_generation,
        limits,
    )
    .map(|(loaded, _)| loaded)
}

pub fn open_system_navigation_with_timing(
    navigation_path: &Path,
    expected_system_id: &SystemId,
    expected_generation: u64,
    limits: SystemShardLimits,
) -> Result<(LoadedSystemShard, SystemNavigationOpenTiming), SystemShardError> {
    open_system_navigation_with_validation(
        navigation_path,
        expected_system_id,
        expected_generation,
        limits,
        NavigationValidation::Full,
    )
}

/// Open navigation whose checksum is bound to an already accepted manifest generation.
///
/// The publishing path rejects duplicate stable keys before switching the manifest. Once the
/// payload checksum matches that immutable descriptor, repeating the set construction during
/// every system entry adds no safety. Unbound readers continue to perform the full duplicate
/// check.
pub fn open_verified_system_navigation_with_timing(
    navigation_path: &Path,
    expected_system_id: &SystemId,
    expected_generation: u64,
    expected_navigation_hash: &str,
    limits: SystemShardLimits,
) -> Result<(LoadedSystemShard, SystemNavigationOpenTiming), SystemShardError> {
    open_system_navigation_with_validation(
        navigation_path,
        expected_system_id,
        expected_generation,
        limits,
        NavigationValidation::ManifestBound(expected_navigation_hash),
    )
}

#[derive(Clone, Copy)]
enum NavigationValidation<'a> {
    Full,
    ManifestBound(&'a str),
}

fn open_system_navigation_with_validation(
    navigation_path: &Path,
    expected_system_id: &SystemId,
    expected_generation: u64,
    limits: SystemShardLimits,
    validation: NavigationValidation<'_>,
) -> Result<(LoadedSystemShard, SystemNavigationOpenTiming), SystemShardError> {
    let read_started = std::time::Instant::now();
    let navigation = read_bounded(navigation_path, limits.max_navigation_compressed_bytes)?;
    let read_us = elapsed_us(read_started);
    let hash_started = std::time::Instant::now();
    let navigation_hash = checksum_hex(&navigation);
    let hash_us = elapsed_us(hash_started);
    if let NavigationValidation::ManifestBound(expected_hash) = validation
        && navigation_hash != expected_hash
    {
        return Err(SystemShardError::new(
            "read",
            "navigation checksum does not match verified manifest generation",
        ));
    }
    let (stored, mut timing) = decode_navigation_with_timing(&navigation, limits)?;
    if stored.system_id != expected_system_id.as_str() || stored.generation != expected_generation {
        return Err(SystemShardError::new(
            "read",
            "navigation identity or generation does not match registry",
        ));
    }
    let games = stored.games;
    let navigation_indexes = stored.indexes;
    let validation_started = std::time::Instant::now();
    match validation {
        NavigationValidation::Full => validate_loaded_games(&games, limits.max_games)?,
        NavigationValidation::ManifestBound(_) => {
            validate_loaded_game_shapes(&games, limits.max_games)?
        }
    }
    validate_navigation_index_bounds(&navigation_indexes, games.len())?;
    if matches!(validation, NavigationValidation::Full)
        && navigation_indexes != build_navigation_indexes(&games)?
    {
        return Err(SystemShardError::new(
            "read",
            "persisted navigation indexes do not match game rows",
        ));
    }
    timing.read_us = read_us;
    timing.hash_us = hash_us;
    timing.validation_us = elapsed_us(validation_started);
    timing.compressed_bytes = navigation.len().try_into().unwrap_or(u64::MAX);
    Ok((
        LoadedSystemShard {
            system_id: expected_system_id.clone(),
            generation: expected_generation,
            navigation_hash,
            projection_stats: None,
            navigation_indexes,
            games,
        },
        timing,
    ))
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
    validate_loaded_game_shapes(games, max_games)?;
    let mut keys = BTreeSet::new();
    if games.iter().any(|game| !keys.insert(&game.stable_key)) {
        return Err(SystemShardError::new(
            "read",
            "navigation contains duplicate game rows",
        ));
    }
    Ok(())
}

fn validate_loaded_game_shapes(
    games: &[SystemGame],
    max_games: usize,
) -> Result<(), SystemShardError> {
    if games.len() > max_games {
        return Err(SystemShardError::new(
            "read",
            "system game count exceeds configured limit",
        ));
    }
    if games.iter().any(|game| {
        game.stable_key.is_empty() || game.title.is_empty() || game.launch_ref.is_empty()
    }) {
        return Err(SystemShardError::new(
            "read",
            "navigation contains invalid game rows",
        ));
    }
    Ok(())
}

pub(crate) fn build_navigation_indexes(
    games: &[SystemGame],
) -> Result<SystemNavigationIndexes, SystemShardError> {
    let ordinal = |index: usize| {
        u32::try_from(index).map_err(|_| SystemShardError::new("write", "game ordinal exceeds u32"))
    };
    let mut title_ordinals = (0..games.len()).collect::<Vec<_>>();
    title_ordinals.sort_unstable_by(|left, right| {
        games[*left]
            .title
            .to_ascii_lowercase()
            .cmp(&games[*right].title.to_ascii_lowercase())
            .then_with(|| games[*left].stable_key.cmp(&games[*right].stable_key))
    });
    let mut preview_ordinals = Vec::new();
    let mut launch_ordinals = Vec::new();
    let mut categories = BTreeMap::<String, Vec<u32>>::new();
    let mut decades = BTreeMap::<u16, Vec<u32>>::new();
    let mut manufacturers = BTreeMap::<String, Vec<u32>>::new();
    let mut players = BTreeMap::<u8, Vec<u32>>::new();
    let mut controls = BTreeMap::<String, Vec<u32>>::new();
    for (index, game) in games.iter().enumerate() {
        let ordinal = ordinal(index)?;
        if game.has_preview
            && !game.preview_archive_path.is_empty()
            && !game.preview_asset_key.is_empty()
        {
            preview_ordinals.push(ordinal);
        }
        if game.launch_plan.is_some() {
            launch_ordinals.push(ordinal);
        }
        let category = game.category.trim();
        if !category.is_empty() {
            categories
                .entry(category.to_owned())
                .or_default()
                .push(ordinal);
        }
        if let Some(year) = game.year {
            decades.entry((year / 10) * 10).or_default().push(ordinal);
        }
        let manufacturer = game.manufacturer.trim();
        if !manufacturer.is_empty() {
            manufacturers
                .entry(manufacturer.to_owned())
                .or_default()
                .push(ordinal);
        }
        if let Some(player_count) = game.players {
            players.entry(player_count).or_default().push(ordinal);
        }
        let control = game.control.trim();
        if !control.is_empty() {
            controls
                .entry(control.to_owned())
                .or_default()
                .push(ordinal);
        }
    }
    launch_ordinals.sort_unstable_by(|left, right| {
        games[*left as usize]
            .launch_ref
            .cmp(&games[*right as usize].launch_ref)
    });
    Ok(SystemNavigationIndexes {
        title_ordinals: title_ordinals
            .into_iter()
            .map(ordinal)
            .collect::<Result<_, _>>()?,
        preview_ordinals,
        launch_ordinals,
        categories: categories.into_iter().collect(),
        decades: decades.into_iter().collect(),
        manufacturers: manufacturers.into_iter().collect(),
        players: players.into_iter().collect(),
        controls: controls.into_iter().collect(),
    })
}

fn validate_navigation_index_bounds(
    indexes: &SystemNavigationIndexes,
    game_count: usize,
) -> Result<(), SystemShardError> {
    let valid = |ordinal: &u32| (*ordinal as usize) < game_count;
    let postings_valid = |postings: &[(String, Vec<u32>)]| {
        postings
            .iter()
            .all(|(_, ordinals)| ordinals.iter().all(valid))
    };
    if indexes.title_ordinals.len() != game_count
        || indexes.title_ordinals.iter().any(|ordinal| !valid(ordinal))
        || indexes
            .title_ordinals
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != game_count
        || indexes
            .preview_ordinals
            .iter()
            .any(|ordinal| !valid(ordinal))
        || indexes
            .launch_ordinals
            .iter()
            .any(|ordinal| !valid(ordinal))
        || !postings_valid(&indexes.categories)
        || !postings_valid(&indexes.manufacturers)
        || !postings_valid(&indexes.controls)
        || indexes
            .decades
            .iter()
            .any(|(_, ordinals)| ordinals.iter().any(|ordinal| !valid(ordinal)))
        || indexes
            .players
            .iter()
            .any(|(_, ordinals)| ordinals.iter().any(|ordinal| !valid(ordinal)))
    {
        return Err(SystemShardError::new(
            "read",
            "persisted navigation index contains invalid ordinals",
        ));
    }
    Ok(())
}

#[cfg(feature = "builder")]
fn common_preview_archive_path(games: &[SystemGame]) -> String {
    let mut counts = BTreeMap::<&str, usize>::new();
    for game in games {
        *counts.entry(&game.preview_archive_path).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by(|(left_path, left_count), (right_path, right_count)| {
            left_count
                .cmp(right_count)
                .then_with(|| right_path.cmp(left_path))
        })
        .map_or_else(String::new, |(path, _)| path.to_string())
}

#[cfg(feature = "builder")]
fn encode_navigation(
    stored: &impl Serialize,
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
    decode_navigation_with_timing(encoded, limits).map(|(stored, _)| stored)
}

fn decode_navigation_with_timing(
    encoded: &[u8],
    limits: SystemShardLimits,
) -> Result<(StoredNavigation, SystemNavigationOpenTiming), SystemShardError> {
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
    let decompress_started = std::time::Instant::now();
    let decoded = lz4_flex::decompress_size_prepended(encoded)
        .map_err(|error| SystemShardError::with("decode navigation", error))?;
    let decompress_us = elapsed_us(decompress_started);
    if decoded.len() != decoded_len {
        return Err(SystemShardError::new(
            "read",
            "decoded navigation length mismatch",
        ));
    }
    let envelope_started = std::time::Instant::now();
    let schema = decode_navigation_schema_header(&decoded)?;
    let envelope_parse_us = elapsed_us(envelope_started);
    let typed_parse_started = std::time::Instant::now();
    let stored = match schema {
        NAVIGATION_SCHEMA_VERSION => serde_json::from_slice(&decoded)
            .map_err(|error| SystemShardError::with("parse navigation", error)),
        _ => Err(SystemShardError::new(
            "read",
            format!("navigation schema {schema} is unsupported"),
        )),
    }?;
    Ok((
        stored,
        SystemNavigationOpenTiming {
            decompress_us,
            envelope_parse_us,
            typed_parse_us: elapsed_us(typed_parse_started),
            decoded_bytes: decoded.len().try_into().unwrap_or(u64::MAX),
            ..SystemNavigationOpenTiming::default()
        },
    ))
}

fn decode_navigation_schema_header(decoded: &[u8]) -> Result<u32, SystemShardError> {
    let header = decoded
        .get(..decoded.len().min(NAVIGATION_HEADER_MAX_BYTES))
        .unwrap_or(decoded);
    let mut cursor = 0usize;
    skip_json_whitespace(header, &mut cursor);
    if header.get(cursor) != Some(&b'{') {
        return Err(SystemShardError::new(
            "read",
            "navigation header is not a JSON object",
        ));
    }
    cursor += 1;
    skip_json_whitespace(header, &mut cursor);
    if header.get(cursor) != Some(&b'\"') {
        return Err(SystemShardError::new(
            "read",
            "navigation schema header is missing",
        ));
    }
    cursor += 1;
    let key_end = cursor
        .checked_add(NAVIGATION_SCHEMA_KEY.len())
        .filter(|end| header.get(cursor..*end) == Some(NAVIGATION_SCHEMA_KEY))
        .ok_or_else(|| SystemShardError::new("read", "navigation schema header is missing"))?;
    cursor = key_end;
    if header.get(cursor) != Some(&b'\"') {
        return Err(SystemShardError::new(
            "read",
            "navigation schema header is malformed",
        ));
    }
    cursor += 1;
    skip_json_whitespace(header, &mut cursor);
    if header.get(cursor) != Some(&b':') {
        return Err(SystemShardError::new(
            "read",
            "navigation schema header is malformed",
        ));
    }
    cursor += 1;
    skip_json_whitespace(header, &mut cursor);
    let number_start = cursor;
    while header.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    if cursor == number_start {
        return Err(SystemShardError::new(
            "read",
            "navigation schema header is malformed",
        ));
    }
    let schema = std::str::from_utf8(&header[number_start..cursor])
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| SystemShardError::new("read", "navigation schema header is invalid"))?;
    skip_json_whitespace(header, &mut cursor);
    if !matches!(header.get(cursor), Some(b',' | b'}')) {
        return Err(SystemShardError::new(
            "read",
            "navigation schema header is malformed",
        ));
    }
    Ok(schema)
}

fn skip_json_whitespace(bytes: &[u8], cursor: &mut usize) {
    while bytes
        .get(*cursor)
        .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
    {
        *cursor += 1;
    }
}

fn elapsed_us(started: std::time::Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
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

fn optional_projection_stats(
    connection: &Connection,
    game_count: usize,
) -> Result<Option<SystemShardProjectionStats>, SystemShardError> {
    fn optional_usize(
        connection: &Connection,
        key: &str,
    ) -> Result<Option<usize>, SystemShardError> {
        match connection.query_row("SELECT value FROM shard_meta WHERE key=?1", [key], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(value) => value
                .parse()
                .map(Some)
                .map_err(|_| SystemShardError::new("read", "invalid projection shard metadata")),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(SystemShardError::with(
                "read optional projection shard metadata",
                error,
            )),
        }
    }

    let values = [
        optional_usize(connection, "source_game_count")?,
        optional_usize(connection, "visible_family_count")?,
        optional_usize(connection, "collapsed_variant_count")?,
    ];
    let [
        Some(source_games),
        Some(visible_families),
        Some(collapsed_variants),
    ] = values
    else {
        if values.iter().all(Option::is_none) {
            return Ok(None);
        }
        return Err(SystemShardError::new(
            "read",
            "incomplete projection shard metadata",
        ));
    };
    if visible_families != game_count
        || source_games < visible_families
        || collapsed_variants != source_games - visible_families
    {
        return Err(SystemShardError::new(
            "read",
            "inconsistent projection shard metadata",
        ));
    }
    Ok(Some(SystemShardProjectionStats {
        source_games,
        visible_families,
        collapsed_variants,
    }))
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
pub enum SystemShardErrorKind {
    Other,
    UnsupportedSchema {
        component: &'static str,
        expected: u64,
        actual: u64,
        system_id: String,
        generation: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemShardError {
    stage: &'static str,
    message: String,
    kind: SystemShardErrorKind,
}

impl SystemShardError {
    fn new(stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
            kind: SystemShardErrorKind::Other,
        }
    }

    fn unsupported_schema(
        component: &'static str,
        expected: u64,
        actual: u64,
        system_id: &SystemId,
        generation: u64,
    ) -> Self {
        Self {
            stage: "read",
            message: format!(
                "unsupported {component} schema version for {system_id} generation {generation}: expected {expected}, found {actual}"
            ),
            kind: SystemShardErrorKind::UnsupportedSchema {
                component,
                expected,
                actual,
                system_id: system_id.as_str().to_string(),
                generation,
            },
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

    pub fn kind(&self) -> &SystemShardErrorKind {
        &self.kind
    }

    pub fn is_older_schema(&self) -> bool {
        matches!(
            self.kind,
            SystemShardErrorKind::UnsupportedSchema {
                expected,
                actual,
                ..
            } if actual < expected
        )
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
    #[cfg(feature = "builder")]
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    #[cfg(feature = "builder")]
    fn schema_three_shard_round_trips_and_matches_navigation() {
        let root = temporary_root("round-trip");
        let sqlite = root.join("1.sqlite3");
        let navigation = root.join("1.nav.lz4b");
        let data = fixture_data();
        let loaded = write_system_shard(&sqlite, &navigation, &data, limits()).unwrap();
        assert_eq!(loaded.system_id, data.system_id);
        assert_eq!(loaded.generation, 1);
        assert_eq!(loaded.games, data.games);
        assert_eq!(
            loaded.navigation_indexes,
            build_navigation_indexes(&data.games).unwrap()
        );
        assert_eq!(loaded.projection_stats, None);
        assert_eq!(loaded.navigation_hash.len(), 16);
        let connection = Connection::open(&sqlite).unwrap();
        let defaults: i64 = connection
            .query_row(
                "SELECT count(*) FROM games WHERE preview_archive_path IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(defaults, 1);
        let (_, timing) = open_system_navigation_with_timing(
            &navigation,
            &data.system_id,
            data.generation,
            limits(),
        )
        .unwrap();
        assert!(timing.compressed_bytes > 0);
        assert!(timing.decoded_bytes > 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn verified_navigation_requires_the_published_payload_hash() {
        let root = temporary_root("verified-navigation");
        let sqlite = root.join("1.sqlite3");
        let navigation = root.join("1.nav.lz4b");
        let data = fixture_data();
        let published = write_system_shard(&sqlite, &navigation, &data, limits()).unwrap();

        let loaded = open_verified_system_navigation_with_timing(
            &navigation,
            &data.system_id,
            data.generation,
            &published.navigation_hash,
            limits(),
        )
        .unwrap()
        .0;
        assert_eq!(loaded.games, data.games);
        assert!(
            open_verified_system_navigation_with_timing(
                &navigation,
                &data.system_id,
                data.generation + 1,
                &published.navigation_hash,
                limits(),
            )
            .unwrap_err()
            .message()
            .contains("identity or generation")
        );
        assert_eq!(
            open_verified_system_navigation_with_timing(
                &navigation,
                &data.system_id,
                data.generation,
                "wrong-hash",
                limits(),
            )
            .unwrap_err()
            .message(),
            "navigation checksum does not match verified manifest generation"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn navigation_rejects_out_of_bounds_persisted_indexes() {
        let root = temporary_root("corrupt-index");
        let navigation = root.join("1.nav.lz4b");
        fs::create_dir_all(&root).unwrap();
        let data = fixture_data();
        let mut indexes = build_navigation_indexes(&data.games).unwrap();
        indexes.preview_ordinals.push(u32::MAX);
        let stored = StoredNavigationRef {
            schema_version: NAVIGATION_SCHEMA_VERSION,
            system_id: data.system_id.as_str(),
            generation: data.generation,
            indexes: &indexes,
            games: &data.games,
        };
        fs::write(&navigation, encode_navigation(&stored, limits()).unwrap()).unwrap();

        assert_eq!(
            open_system_navigation(&navigation, &data.system_id, data.generation, limits())
                .unwrap_err()
                .message(),
            "persisted navigation index contains invalid ordinals"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn publication_rejects_duplicate_stable_keys_before_manifest_binding() {
        let root = temporary_root("duplicate-publication");
        let sqlite = root.join("1.sqlite3");
        let navigation = root.join("1.nav.lz4b");
        let mut data = fixture_data();
        let mut duplicate = data.games[0].clone();
        duplicate.title = "Duplicate".into();
        duplicate.launch_ref = "/games/Duplicate.rom".into();
        data.games.push(duplicate);

        assert_eq!(
            write_system_shard(&sqlite, &navigation, &data, limits())
                .unwrap_err()
                .message(),
            "games need non-empty unique keys, titles, and launch references"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn projection_stats_round_trip_without_changing_the_games_schema() {
        let root = temporary_root("projection-stats");
        let sqlite = root.join("1.sqlite3");
        let navigation = root.join("1.nav.lz4b");
        let mut data = fixture_data();
        data.projection_stats = Some(SystemShardProjectionStats {
            source_games: data.games.len() + 4,
            visible_families: data.games.len(),
            collapsed_variants: 4,
        });

        let loaded = write_system_shard(&sqlite, &navigation, &data, limits()).unwrap();

        assert_eq!(loaded.projection_stats, data.projection_stats);
        let connection = Connection::open(&sqlite).unwrap();
        let columns: i64 = connection
            .query_row(
                "SELECT count(*) FROM pragma_table_info('games')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(columns, 14);
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
                "UPDATE shard_meta SET value=?1 WHERE key='schema_version'",
                [SHARD_SCHEMA_VERSION + 1],
            )
            .unwrap();
        drop(connection);
        assert_eq!(
            open_system_shard(&sqlite, &navigation, &data.system_id, 1, limits())
                .unwrap_err()
                .message(),
            format!(
                "unsupported shard schema version for snes generation 1: expected {}, found {}",
                SHARD_SCHEMA_VERSION,
                SHARD_SCHEMA_VERSION + 1
            )
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
    fn reader_rejects_incomplete_persisted_search() {
        let root = temporary_root("corrupt-search");
        let sqlite = root.join("1.sqlite3");
        let navigation = root.join("1.nav.lz4b");
        let data = fixture_data();
        write_system_shard(&sqlite, &navigation, &data, limits()).unwrap();
        let connection = Connection::open(&sqlite).unwrap();
        connection
            .execute("DROP TABLE autocomplete_words", [])
            .unwrap();
        drop(connection);

        assert!(
            open_system_shard(&sqlite, &navigation, &data.system_id, 1, limits())
                .unwrap_err()
                .message()
                .starts_with("check persisted autocomplete table:")
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

    #[test]
    fn bounded_navigation_header_accepts_canonical_whitespace() {
        assert_eq!(
            decode_navigation_schema_header(b" \n { \"schema_version\" : 3 , \"games\": [] }")
                .unwrap(),
            3
        );
    }

    #[test]
    fn bounded_navigation_header_rejects_reordered_malformed_and_overflowing_schema() {
        for malformed in [
            br#"{"system_id":"c64","schema_version":3,"games":[]}"#.as_slice(),
            br#"{"schema_version":"3","games":[]}"#.as_slice(),
            br#"{"schema_version":4294967296,"games":[]}"#.as_slice(),
            br#"[]"#.as_slice(),
        ] {
            assert!(decode_navigation_schema_header(malformed).is_err());
        }
    }

    #[test]
    fn current_navigation_decodes_a_c64_sized_fixture_in_one_typed_pass() {
        let games = (0..15_089)
            .map(|index| SystemGame {
                stable_key: format!("game-{index:05}"),
                title: format!("Synthetic C64 Game {index:05}"),
                launch_ref: format!("/games/C64/game-{index:05}.d64"),
                ..SystemGame::default()
            })
            .collect::<Vec<_>>();
        let stored = StoredNavigation {
            schema_version: NAVIGATION_SCHEMA_VERSION,
            system_id: "c64".to_string(),
            generation: 7,
            indexes: build_navigation_indexes(&games).unwrap(),
            games,
        };
        let encoded = lz4_flex::compress_prepend_size(&serde_json::to_vec(&stored).unwrap());
        let limits = SystemShardLimits {
            max_navigation_compressed_bytes: 64 * 1024 * 1024,
            max_navigation_decoded_bytes: 64 * 1024 * 1024,
            max_games: 20_000,
            ..limits()
        };

        let (decoded, timing) = decode_navigation_with_timing(&encoded, limits).unwrap();

        assert_eq!(decoded.games.len(), 15_089);
        assert_eq!(decoded.games[0].stable_key, "game-00000");
        assert_eq!(decoded.games[15_088].stable_key, "game-15088");
        assert!(timing.typed_parse_us > 0);
    }

    #[test]
    fn bounded_navigation_header_preserves_future_schema_rejection() {
        let decoded = br#"{"schema_version":99,"system_id":"c64","generation":1,"games":[]}"#;
        let encoded = lz4_flex::compress_prepend_size(decoded);
        let error = decode_navigation(&encoded, limits()).unwrap_err();
        assert_eq!(error.message(), "navigation schema 99 is unsupported");
    }

    #[cfg(feature = "builder")]
    fn fixture_data() -> SystemShardData {
        SystemShardData {
            system_id: SystemId::parse("snes").unwrap(),
            generation: 1,
            projection_stats: None,
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
                    category: "Platform".to_string(),
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

    #[cfg(feature = "builder")]
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
