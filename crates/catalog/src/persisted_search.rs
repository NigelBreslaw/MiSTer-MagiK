// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Persisted FTS5 game search and autocomplete stored inside system shards.

use rusqlite::{Connection, OpenFlags};
use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::time::Instant;

pub const SEARCH_SCHEMA_VERSION: u32 = 1;

const SEARCH_WEIGHTS: &str = "10.0,9.0,8.0,7.0,7.0,6.0,6.8,6.5,6.0,4.0,3.5";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PersistedSearchMatch {
    pub ordinal: usize,
    pub rank: f64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedAutocompleteCandidate {
    pub word: String,
    pub source_rank: u8,
    pub score: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PersistedSearchTiming {
    pub rust_prepare_us: u64,
    pub sqlite_us: u64,
    pub rust_finalize_us: u64,
    pub total_us: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PersistedSearchResult {
    pub matches: Vec<PersistedSearchMatch>,
    pub autocomplete: Option<PersistedAutocompleteCandidate>,
    pub timing: PersistedSearchTiming,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PersistedCollectionMatch {
    pub system_id: String,
    pub ordinal: usize,
    pub rank: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PersistedCollectionSearchResult {
    pub matches: Vec<PersistedCollectionMatch>,
    pub autocomplete: Option<PersistedAutocompleteCandidate>,
    pub timing: PersistedSearchTiming,
}

pub fn search_system_shards(
    storage_root: &Path,
    system_ids: &[String],
    query: &str,
    limits: crate::shard_registry::RegistryLimits,
) -> Result<PersistedCollectionSearchResult, PersistedSearchError> {
    let total_started = Instant::now();
    let prepare_started = Instant::now();
    let manifest_pmu = mister_magik_perf_events::sampled_span("search.manifest");
    let manifest = crate::shard_registry::read_latest_manifest_lazy(storage_root, limits)
        .map_err(|error| PersistedSearchError::with("open catalog manifest", error))?;
    drop(manifest_pmu);
    let manifest_prepare_us = elapsed_us(prepare_started);
    let mut result = PersistedCollectionSearchResult::default();
    for system_id in system_ids {
        let system = manifest
            .systems
            .iter()
            .find(|system| system.system_id.as_str() == system_id)
            .ok_or_else(|| {
                PersistedSearchError::new(format!(
                    "search system {system_id} is absent from the manifest"
                ))
            })?;
        let shard = search_system_shard(&storage_root.join(&system.active.sqlite_path), query)?;
        result.matches.extend(
            shard
                .matches
                .into_iter()
                .map(|entry| PersistedCollectionMatch {
                    system_id: system_id.clone(),
                    ordinal: entry.ordinal,
                    rank: entry.rank,
                }),
        );
        if let Some(candidate) = shard.autocomplete {
            let replace = result.autocomplete.as_ref().is_none_or(|current| {
                autocomplete_candidate_order(&candidate, current) == Ordering::Greater
            });
            if replace {
                result.autocomplete = Some(candidate);
            }
        }
        result.timing.rust_prepare_us = result
            .timing
            .rust_prepare_us
            .saturating_add(shard.timing.rust_prepare_us);
        result.timing.sqlite_us = result
            .timing
            .sqlite_us
            .saturating_add(shard.timing.sqlite_us);
    }
    result.matches.sort_by(|left, right| {
        left.rank
            .partial_cmp(&right.rank)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.system_id.cmp(&right.system_id))
            .then_with(|| left.ordinal.cmp(&right.ordinal))
    });
    result.timing.rust_prepare_us = result
        .timing
        .rust_prepare_us
        .saturating_add(manifest_prepare_us);
    result.timing.total_us = elapsed_us(total_started);
    result.timing.rust_finalize_us = result
        .timing
        .total_us
        .saturating_sub(result.timing.rust_prepare_us)
        .saturating_sub(result.timing.sqlite_us);
    Ok(result)
}

pub fn search_system_shard(
    sqlite_path: &Path,
    query: &str,
) -> Result<PersistedSearchResult, PersistedSearchError> {
    let total_started = Instant::now();
    let prepare_started = Instant::now();
    let prepare_pmu = mister_magik_perf_events::sampled_span("search.prepare");
    let match_query = fts_match_query(query);
    let fragment = current_search_word(query);
    let autocomplete_prefix = normalize_search_text(fragment);
    let rust_prepare_us = elapsed_us(prepare_started);
    drop(prepare_pmu);

    if match_query.is_empty() {
        return Ok(PersistedSearchResult {
            timing: PersistedSearchTiming {
                rust_prepare_us,
                total_us: elapsed_us(total_started),
                ..PersistedSearchTiming::default()
            },
            ..PersistedSearchResult::default()
        });
    }

    let sqlite_started = Instant::now();
    let sqlite_pmu = mister_magik_perf_events::sampled_span("search.sqlite");
    let connection = open_read_only(sqlite_path)?;
    let sql = format!(
        "SELECT rowid - 1, bm25(game_search_fts,{SEARCH_WEIGHTS})
         FROM game_search_fts
         WHERE game_search_fts MATCH ?1
         ORDER BY 2 ASC, rowid ASC"
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| PersistedSearchError::with("prepare FTS query", error))?;
    let rows = statement
        .query_map([&match_query], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })
        .map_err(|error| PersistedSearchError::with("query FTS index", error))?;
    let mut raw_matches = Vec::new();
    for row in rows {
        raw_matches
            .push(row.map_err(|error| PersistedSearchError::with("read FTS result", error))?);
    }

    let autocomplete = if autocomplete_prefix.len() >= 2 {
        let upper_bound = format!("{autocomplete_prefix}\u{10ffff}");
        connection
            .query_row(
                "SELECT word,source_rank,score
                 FROM autocomplete_words
                 WHERE word >= ?1 AND word < ?2
                 ORDER BY source_rank DESC,score DESC,word ASC
                 LIMIT 1",
                [&autocomplete_prefix, &upper_bound],
                |row| {
                    Ok(PersistedAutocompleteCandidate {
                        word: row.get(0)?,
                        source_rank: row.get(1)?,
                        score: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(|error| PersistedSearchError::with("query autocomplete", error))?
    } else {
        None
    };
    let sqlite_us = elapsed_us(sqlite_started);
    drop(sqlite_pmu);

    let finalize_started = Instant::now();
    let finalize_pmu = mister_magik_perf_events::sampled_span("search.finalize");
    let matches = raw_matches
        .into_iter()
        .map(|(ordinal, rank)| {
            usize::try_from(ordinal)
                .map(|ordinal| PersistedSearchMatch { ordinal, rank })
                .map_err(|_| PersistedSearchError::new("FTS result ordinal is invalid"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let rust_finalize_us = elapsed_us(finalize_started);
    drop(finalize_pmu);

    Ok(PersistedSearchResult {
        matches,
        autocomplete,
        timing: PersistedSearchTiming {
            rust_prepare_us,
            sqlite_us,
            rust_finalize_us,
            total_us: elapsed_us(total_started),
        },
    })
}

#[cfg(feature = "builder")]
pub(crate) fn create_schema(connection: &Connection) -> Result<(), PersistedSearchError> {
    connection
        .execute_batch(
            "CREATE VIRTUAL TABLE game_search_fts USING fts5(
                 title,
                 compact_title,
                 manufacturer,
                 compact_manufacturer,
                 control,
                 compact_control,
                 players,
                 year,
                 decade,
                 path,
                 compact_path,
                 content='',
                 tokenize='unicode61 remove_diacritics 2',
                 prefix='1 2 3'
             );
             CREATE TABLE autocomplete_words (
                 word TEXT PRIMARY KEY,
                 source_rank INTEGER NOT NULL,
                 score INTEGER NOT NULL
             ) WITHOUT ROWID;",
        )
        .map_err(|error| PersistedSearchError::with("create search schema", error))
}

#[cfg(feature = "builder")]
pub(crate) fn populate(
    connection: &Connection,
    games: &[crate::system_shard::SystemGame],
) -> Result<usize, PersistedSearchError> {
    use std::collections::BTreeMap;

    let mut insert_search = connection
        .prepare(
            "INSERT INTO game_search_fts(
                 rowid,title,compact_title,manufacturer,compact_manufacturer,
                 control,compact_control,players,year,decade,path,compact_path
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        )
        .map_err(|error| PersistedSearchError::with("prepare search rows", error))?;
    let mut words = BTreeMap::<String, AutocompleteStats>::new();
    for (ordinal, game) in games.iter().enumerate() {
        let title = normalize_search_text(&game.title);
        let manufacturer = normalize_search_text(&game.manufacturer);
        let control = normalize_search_text(&crate::arcade_catalog::canonical_control_label(
            &game.control,
        ));
        let players = game
            .players
            .map(crate::arcade_catalog::player_count_label)
            .unwrap_or_default();
        let year = game.year.map(|year| year.to_string()).unwrap_or_default();
        let decade = game
            .year
            .map(|year| format!("{}0s", year / 10))
            .unwrap_or_default();
        let path = normalize_search_text(game_basename(&game.launch_ref));
        insert_search
            .execute(rusqlite::params![
                i64::try_from(ordinal + 1).map_err(|_| PersistedSearchError::new(
                    "game ordinal exceeds SQLite integer"
                ))?,
                title,
                compact_if_different(&title),
                manufacturer,
                compact_if_different(&manufacturer),
                control,
                compact_if_different(&control),
                players,
                year,
                decade,
                path,
                compact_if_different(&path),
            ])
            .map_err(|error| PersistedSearchError::with("insert search row", error))?;

        add_words(&mut words, &game.title, AutocompleteSource::Title);
        add_words(&mut words, &game.manufacturer, AutocompleteSource::Metadata);
        add_words(
            &mut words,
            &crate::arcade_catalog::canonical_control_label(&game.control),
            AutocompleteSource::Metadata,
        );
        if let Some(players) = game.players {
            add_word(
                &mut words,
                &crate::arcade_catalog::player_count_label(players),
                AutocompleteSource::Metadata,
            );
        }
        add_words(
            &mut words,
            game_basename(&game.launch_ref),
            AutocompleteSource::Path,
        );
        if let Some(year) = game.year {
            add_word(&mut words, &year.to_string(), AutocompleteSource::Metadata);
            add_word(
                &mut words,
                &format!("{}0s", year / 10),
                AutocompleteSource::Metadata,
            );
        }
    }
    drop(insert_search);

    let mut insert_word = connection
        .prepare("INSERT INTO autocomplete_words(word,source_rank,score) VALUES (?1,?2,?3)")
        .map_err(|error| PersistedSearchError::with("prepare autocomplete rows", error))?;
    for (word, stats) in &words {
        insert_word
            .execute(rusqlite::params![word, stats.source_rank, stats.score])
            .map_err(|error| PersistedSearchError::with("insert autocomplete row", error))?;
    }
    drop(insert_word);
    connection
        .execute(
            "INSERT INTO game_search_fts(game_search_fts) VALUES ('optimize')",
            [],
        )
        .map_err(|error| PersistedSearchError::with("optimize FTS index", error))?;
    connection
        .execute(
            "INSERT INTO game_search_fts(game_search_fts) VALUES ('integrity-check')",
            [],
        )
        .map_err(|error| PersistedSearchError::with("check FTS integrity", error))?;
    Ok(words.len())
}

pub(crate) fn validate(
    connection: &Connection,
    expected_documents: usize,
) -> Result<(), PersistedSearchError> {
    if search_meta_u64(connection, "search_schema_version")? != u64::from(SEARCH_SCHEMA_VERSION) {
        return Err(PersistedSearchError::new(
            "unsupported persisted search schema version",
        ));
    }
    let stored_documents = search_meta_usize(connection, "search_document_count")?;
    let _stored_words = search_meta_usize(connection, "autocomplete_word_count")?;
    if stored_documents != expected_documents {
        return Err(PersistedSearchError::new(
            "persisted search document count does not match shard",
        ));
    }
    let indexed_documents: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM game_search_fts LIMIT 1)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| PersistedSearchError::with("check persisted search table", error))?;
    if expected_documents > 0 && !indexed_documents {
        return Err(PersistedSearchError::new(
            "persisted search table is unexpectedly empty",
        ));
    }
    connection
        .prepare("SELECT word FROM autocomplete_words LIMIT 1")
        .map_err(|error| PersistedSearchError::with("check persisted autocomplete table", error))?;
    Ok(())
}

fn search_meta_u64(connection: &Connection, key: &str) -> Result<u64, PersistedSearchError> {
    let value: String = connection
        .query_row("SELECT value FROM shard_meta WHERE key=?1", [key], |row| {
            row.get(0)
        })
        .map_err(|error| PersistedSearchError::with("read search metadata", error))?;
    value
        .parse()
        .map_err(|error| PersistedSearchError::with("parse search metadata", error))
}

fn search_meta_usize(connection: &Connection, key: &str) -> Result<usize, PersistedSearchError> {
    usize::try_from(search_meta_u64(connection, key)?)
        .map_err(|_| PersistedSearchError::new("search metadata exceeds platform size"))
}

fn open_read_only(path: &Path) -> Result<Connection, PersistedSearchError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| PersistedSearchError::with("open search shard", error))?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|error| PersistedSearchError::with("set search shard query-only", error))?;
    Ok(connection)
}

fn fts_match_query(query: &str) -> String {
    normalize_search_text(query)
        .split_whitespace()
        .map(|token| format!("\"{token}\"*"))
        .collect::<Vec<_>>()
        .join(" AND ")
}

pub(crate) fn normalize_search_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(feature = "builder")]
fn compact_if_different(value: &str) -> String {
    let compact = value.replace(' ', "");
    if compact != value {
        compact
    } else {
        String::new()
    }
}

fn current_search_word(query: &str) -> &str {
    query
        .rsplit_once(char::is_whitespace)
        .map(|(_, word)| word)
        .unwrap_or(query)
}

#[cfg(feature = "builder")]
fn game_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(feature = "builder")]
#[derive(Clone, Copy)]
enum AutocompleteSource {
    Title,
    Metadata,
    Path,
}

#[cfg(feature = "builder")]
#[derive(Default)]
struct AutocompleteStats {
    source_rank: u8,
    score: u32,
}

#[cfg(feature = "builder")]
fn add_words(
    words: &mut std::collections::BTreeMap<String, AutocompleteStats>,
    value: &str,
    source: AutocompleteSource,
) {
    for word in normalize_search_text(value).split_whitespace() {
        add_word(words, word, source);
    }
}

#[cfg(feature = "builder")]
fn add_word(
    words: &mut std::collections::BTreeMap<String, AutocompleteStats>,
    value: &str,
    source: AutocompleteSource,
) {
    let word = normalize_search_text(value);
    if word.len() < 2 || is_noisy_autocomplete_word(&word) {
        return;
    }
    let (score, source_rank) = match source {
        AutocompleteSource::Title => (5, 2),
        AutocompleteSource::Metadata => (4, 2),
        AutocompleteSource::Path => (1, 1),
    };
    let stats = words.entry(word).or_default();
    stats.score += score;
    stats.source_rank = stats.source_rank.max(source_rank);
}

#[cfg(feature = "builder")]
fn is_noisy_autocomplete_word(word: &str) -> bool {
    matches!(
        word,
        "a" | "an" | "and" | "the" | "of" | "in" | "on" | "to" | "for" | "with" | "world"
    )
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn autocomplete_candidate_order(
    left: &PersistedAutocompleteCandidate,
    right: &PersistedAutocompleteCandidate,
) -> Ordering {
    left.source_rank
        .cmp(&right.source_rank)
        .then_with(|| left.score.cmp(&right.score))
        .then_with(|| right.word.cmp(&left.word))
}

#[derive(Debug)]
pub struct PersistedSearchError {
    message: String,
}

impl PersistedSearchError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn with(stage: &str, error: impl fmt::Display) -> Self {
        Self::new(format!("{stage}: {error}"))
    }
}

impl fmt::Display for PersistedSearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for PersistedSearchError {}

trait OptionalRow<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalRow<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(all(test, feature = "builder"))]
mod tests {
    use super::*;
    use crate::system_shard::{SystemGame, SystemShardData, SystemShardLimits, write_system_shard};
    use crate::{catalog_classify::SystemId, system_shard::SystemLaunchPlan};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn fts_search_uses_prefixes_compact_titles_and_metadata() {
        let (root, sqlite) = fixture();

        let pacman = search_system_shard(&sqlite, "pacman").unwrap();
        assert_eq!(pacman.matches[0].ordinal, 0);
        let capcom = search_system_shard(&sqlite, "cap").unwrap();
        assert_eq!(capcom.matches[0].ordinal, 1);
        let multi = search_system_shard(&sqlite, "street f").unwrap();
        assert_eq!(multi.matches[0].ordinal, 1);
        assert!(
            search_system_shard(&sqlite, "ighter")
                .unwrap()
                .matches
                .is_empty()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn autocomplete_prefers_rank_then_score_then_word() {
        let (root, sqlite) = fixture();
        let result = search_system_shard(&sqlite, "str").unwrap();
        assert_eq!(
            result.autocomplete.map(|candidate| candidate.word),
            Some("street".to_string())
        );
        assert!(
            search_system_shard(&sqlite, "x")
                .unwrap()
                .autocomplete
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn fixture() -> (std::path::PathBuf, std::path::PathBuf) {
        let root = temporary_root();
        let sqlite = root.join("1.sqlite3");
        let navigation = root.join("1.nav.lz4b");
        let games = vec![
            SystemGame {
                stable_key: "pac-man".to_string(),
                title: "Pac-Man".to_string(),
                launch_ref: "/media/fat/_Arcade/Pac-Man.mra".to_string(),
                manufacturer: "Namco".to_string(),
                ..SystemGame::default()
            },
            SystemGame {
                stable_key: "street-fighter".to_string(),
                title: "Street Fighter II".to_string(),
                launch_ref: "/media/fat/_Arcade/Street Fighter II.mra".to_string(),
                manufacturer: "Capcom".to_string(),
                control: "doublejoy".to_string(),
                launch_plan: Some(SystemLaunchPlan {
                    launch_ref: "street-fighter".to_string(),
                    title: "Street Fighter II".to_string(),
                    system_id: "arcade".to_string(),
                    core_path: "Arcade".to_string(),
                    payload_path: "/media/fat/_Arcade/Street Fighter II.mra".to_string(),
                    mount_kind: "mra".to_string(),
                    mount_index: 0,
                    delay_secs: 0,
                }),
                ..SystemGame::default()
            },
        ];
        write_system_shard(
            &sqlite,
            &navigation,
            &SystemShardData {
                system_id: SystemId::parse("arcade").unwrap(),
                generation: 1,
                projection_stats: None,
                games,
            },
            SystemShardLimits {
                max_sqlite_bytes: 8 * 1024 * 1024,
                max_navigation_compressed_bytes: 1024 * 1024,
                max_navigation_decoded_bytes: 1024 * 1024,
                max_games: 100,
            },
        )
        .unwrap();
        (root, sqlite)
    }

    fn temporary_root() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mister-magik-persisted-search-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
}
