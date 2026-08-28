// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Persisted FTS5 game search and autocomplete stored inside system shards.

use rusqlite::{Connection, OpenFlags};
#[cfg(feature = "builder")]
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Instant;

pub const SEARCH_SCHEMA_VERSION: u32 = 2;

const SEARCH_WEIGHTS: &str = "10.0,9.0,8.0,7.0,7.0,6.0,6.8,6.5,6.0,4.0,3.5";
#[cfg(feature = "builder")]
const SEARCH_PIPELINE_BATCH: usize = 256;

#[cfg(feature = "builder")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PersistedSearchDetail {
    Full,
    Column,
    None,
}

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
    pub sqlite_open_us: u64,
    pub statement_prepare_us: u64,
    pub sqlite_execute_us: u64,
    pub sqlite_us: u64,
    pub rust_finalize_us: u64,
    pub total_us: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PersistedSearchRuntimeMetrics {
    pub sqlite_opens: u64,
    pub statement_prepares: u64,
}

static SEARCH_SQLITE_OPENS: AtomicU64 = AtomicU64::new(0);
static SEARCH_STATEMENT_PREPARES: AtomicU64 = AtomicU64::new(0);

pub fn runtime_metrics() -> PersistedSearchRuntimeMetrics {
    PersistedSearchRuntimeMetrics {
        sqlite_opens: SEARCH_SQLITE_OPENS.load(AtomicOrdering::Relaxed),
        statement_prepares: SEARCH_STATEMENT_PREPARES.load(AtomicOrdering::Relaxed),
    }
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

/// An immutable, already validated view of the active search shards.
///
/// Callers own freshness: create a new snapshot whenever their catalog
/// generation changes. Searches through one snapshot never reread or reparse
/// the dual-slot registry manifest.
#[derive(Clone, Debug)]
pub struct PersistedSearchCatalog {
    storage_root: PathBuf,
    manifest: Arc<crate::shard_registry::CatalogManifest>,
}

impl PersistedSearchCatalog {
    pub fn open(
        storage_root: &Path,
        limits: crate::shard_registry::RegistryLimits,
    ) -> Result<Self, PersistedSearchError> {
        let manifest = crate::shard_registry::read_latest_manifest_lazy(storage_root, limits)
            .map_err(|error| PersistedSearchError::with("open catalog manifest", error))?;
        Ok(Self {
            storage_root: storage_root.to_path_buf(),
            manifest: Arc::new(manifest),
        })
    }

    pub fn contains_system(&self, system_id: &str) -> bool {
        self.manifest
            .systems
            .iter()
            .any(|system| system.system_id.as_str() == system_id)
    }

    pub fn search(
        &self,
        system_ids: &[String],
        query: &str,
    ) -> Result<PersistedCollectionSearchResult, PersistedSearchError> {
        search_system_shards_in_manifest(
            &self.storage_root,
            &self.manifest,
            system_ids,
            query,
            Instant::now(),
            0,
        )
    }
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
    search_system_shards_in_manifest(
        storage_root,
        &manifest,
        system_ids,
        query,
        total_started,
        manifest_prepare_us,
    )
}

fn search_system_shards_in_manifest(
    storage_root: &Path,
    manifest: &crate::shard_registry::CatalogManifest,
    system_ids: &[String],
    query: &str,
    total_started: Instant,
    manifest_prepare_us: u64,
) -> Result<PersistedCollectionSearchResult, PersistedSearchError> {
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
        result.timing.sqlite_open_us = result
            .timing
            .sqlite_open_us
            .saturating_add(shard.timing.sqlite_open_us);
        result.timing.statement_prepare_us = result
            .timing
            .statement_prepare_us
            .saturating_add(shard.timing.statement_prepare_us);
        result.timing.sqlite_execute_us = result
            .timing
            .sqlite_execute_us
            .saturating_add(shard.timing.sqlite_execute_us);
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
    let open_started = Instant::now();
    let connection = open_read_only(sqlite_path)?;
    let sqlite_open_us = elapsed_us(open_started);
    let sql = format!(
        "SELECT rowid - 1, bm25(game_search_fts,{SEARCH_WEIGHTS})
         FROM game_search_fts
         WHERE game_search_fts MATCH ?1
         ORDER BY 2 ASC, rowid ASC"
    );
    let prepare_started = Instant::now();
    SEARCH_STATEMENT_PREPARES.fetch_add(1, AtomicOrdering::Relaxed);
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| PersistedSearchError::with("prepare FTS query", error))?;
    let mut statement_prepare_us = elapsed_us(prepare_started);
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
        let autocomplete_prepare_started = Instant::now();
        SEARCH_STATEMENT_PREPARES.fetch_add(1, AtomicOrdering::Relaxed);
        let mut autocomplete_statement = connection
            .prepare(
                "SELECT word,source_rank,score
                 FROM autocomplete_words
                 WHERE word >= ?1 AND word < ?2
                 ORDER BY source_rank DESC,score DESC,word ASC
                 LIMIT 1",
            )
            .map_err(|error| PersistedSearchError::with("prepare autocomplete", error))?;
        statement_prepare_us =
            statement_prepare_us.saturating_add(elapsed_us(autocomplete_prepare_started));
        autocomplete_statement
            .query_row([&autocomplete_prefix, &upper_bound], |row| {
                Ok(PersistedAutocompleteCandidate {
                    word: row.get(0)?,
                    source_rank: row.get(1)?,
                    score: row.get(2)?,
                })
            })
            .optional()
            .map_err(|error| PersistedSearchError::with("query autocomplete", error))?
    } else {
        None
    };
    let sqlite_us = elapsed_us(sqlite_started);
    let sqlite_execute_us = sqlite_us
        .saturating_sub(sqlite_open_us)
        .saturating_sub(statement_prepare_us);
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
            sqlite_open_us,
            statement_prepare_us,
            sqlite_execute_us,
            sqlite_us,
            rust_finalize_us,
            total_us: elapsed_us(total_started),
        },
    })
}

#[cfg(feature = "builder")]
pub(crate) fn create_schema_with_detail(
    connection: &Connection,
    detail: PersistedSearchDetail,
) -> Result<(), PersistedSearchError> {
    let detail = match detail {
        PersistedSearchDetail::Full => "full",
        PersistedSearchDetail::Column => "column",
        PersistedSearchDetail::None => "none",
    };
    connection
        .execute_batch(&format!(
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
                 prefix='1 2 3',
                 detail='{detail}'
             );
             CREATE TABLE autocomplete_words (
                 word TEXT PRIMARY KEY,
                 source_rank INTEGER NOT NULL,
                 score INTEGER NOT NULL
             ) WITHOUT ROWID;"
        ))
        .map_err(|error| PersistedSearchError::with("create search schema", error))
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PersistedSearchBuildOutcome {
    pub(crate) words: usize,
    pub(crate) batches: usize,
    pub(crate) document_build_us: u64,
    pub(crate) fts_insert_us: u64,
    pub(crate) pipeline_wait_us: u64,
    pub(crate) row_loop_us: u64,
    pub(crate) autocomplete_sort_us: u64,
    pub(crate) autocomplete_insert_us: u64,
    pub(crate) optimize_us: u64,
    pub(crate) optimize_mode: &'static str,
    pub(crate) automerge_restore_us: u64,
    pub(crate) integrity_us: u64,
    pub(crate) integrity_mode: &'static str,
    pub(crate) source_checksum: String,
    pub(crate) total_us: u64,
}

#[cfg(feature = "builder")]
struct PreparedSearchDocument {
    ordinal: usize,
    title: String,
    compact_title: String,
    manufacturer: String,
    compact_manufacturer: String,
    control: String,
    compact_control: String,
    players: String,
    year: String,
    decade: String,
    path: String,
    compact_path: String,
}

#[cfg(feature = "builder")]
struct PreparedSearchBatch {
    documents: Vec<PreparedSearchDocument>,
    words: std::collections::HashMap<String, AutocompleteStats>,
    build_us: u64,
}

#[cfg(feature = "builder")]
fn prepare_search_batch(
    first_ordinal: usize,
    games: &[crate::system_shard::SystemGame],
) -> PreparedSearchBatch {
    use std::collections::HashMap;

    let started = Instant::now();
    let mut documents = Vec::with_capacity(games.len());
    let mut words = HashMap::<String, AutocompleteStats>::new();
    for (offset, game) in games.iter().enumerate() {
        let title = normalize_search_text(&game.title);
        let manufacturer = normalize_search_text(&game.manufacturer);
        let control = normalize_search_text(&crate::arcade_catalog::canonical_control_label(
            &game.control,
        ));
        let players = game
            .players
            .map(crate::arcade_catalog::player_count_label)
            .unwrap_or_default();
        let autocomplete_players = normalize_search_text(&players);
        let year = game.year.map(|year| year.to_string()).unwrap_or_default();
        let decade = game
            .year
            .map(|year| format!("{}0s", year / 10))
            .unwrap_or_default();
        let path = normalize_search_text(game_basename(&game.launch_ref));

        add_normalized_words(&mut words, &title, AutocompleteSource::Title);
        add_normalized_words(&mut words, &manufacturer, AutocompleteSource::Metadata);
        add_normalized_words(&mut words, &control, AutocompleteSource::Metadata);
        if !autocomplete_players.is_empty() {
            add_normalized_word(
                &mut words,
                &autocomplete_players,
                AutocompleteSource::Metadata,
            );
        }
        add_normalized_words(&mut words, &path, AutocompleteSource::Path);
        if !year.is_empty() {
            add_normalized_word(&mut words, &year, AutocompleteSource::Metadata);
        }
        if !decade.is_empty() {
            add_normalized_word(&mut words, &decade, AutocompleteSource::Metadata);
        }

        documents.push(PreparedSearchDocument {
            ordinal: first_ordinal.saturating_add(offset),
            compact_title: compact_if_different(&title),
            title,
            compact_manufacturer: compact_if_different(&manufacturer),
            manufacturer,
            compact_control: compact_if_different(&control),
            control,
            players,
            year,
            decade,
            compact_path: compact_if_different(&path),
            path,
        });
    }
    PreparedSearchBatch {
        documents,
        words,
        build_us: elapsed_us(started),
    }
}

#[cfg(feature = "builder")]
fn merge_autocomplete_words(
    destination: &mut std::collections::HashMap<String, AutocompleteStats>,
    source: std::collections::HashMap<String, AutocompleteStats>,
) {
    for (word, source_stats) in source {
        let stats = destination.entry(word).or_default();
        stats.score += source_stats.score;
        stats.source_rank = stats.source_rank.max(source_stats.source_rank);
    }
}

#[cfg(feature = "builder")]
pub(crate) fn populate_with_options(
    connection: &Connection,
    games: &[crate::system_shard::SystemGame],
    optimize: bool,
) -> Result<PersistedSearchBuildOutcome, PersistedSearchError> {
    use std::collections::HashMap;

    let total_started = Instant::now();
    let source_checksum = search_source_checksum(games);
    connection
        .execute(
            "INSERT INTO game_search_fts(game_search_fts,rank) VALUES ('automerge',0)",
            [],
        )
        .map_err(|error| PersistedSearchError::with("suspend FTS automerge", error))?;
    let mut insert_search = connection
        .prepare(
            "INSERT INTO game_search_fts(
                 rowid,title,compact_title,manufacturer,compact_manufacturer,
                 control,compact_control,players,year,decade,path,compact_path
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        )
        .map_err(|error| PersistedSearchError::with("prepare search rows", error))?;
    let row_loop_started = Instant::now();
    let row_loop_pmu = mister_magik_perf_events::sampled_span(crate::pmu_phase::SEARCH_ROWS);
    let mut words = HashMap::<String, AutocompleteStats>::new();
    let mut batches = 0usize;
    let mut document_build_us = 0u64;
    let mut fts_insert_us = 0u64;
    let mut pipeline_wait_us = 0u64;
    let main_background_scope = crate::cooperative_work::BackgroundScope::enter();
    std::thread::scope(|scope| -> Result<(), PersistedSearchError> {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<PreparedSearchBatch>(1);
        let producer = std::thread::Builder::new()
            .name("catalog-search-docs".to_string())
            .spawn_scoped(scope, move || {
                crate::runtime_thread::apply_runtime_thread_policy(
                    crate::runtime_thread::RuntimeThreadRole::LibraryWalker,
                );
                let _background_scope = crate::cooperative_work::BackgroundScope::enter();
                for (batch_index, chunk) in games.chunks(SEARCH_PIPELINE_BATCH).enumerate() {
                    crate::cooperative_work::checkpoint();
                    let batch = prepare_search_batch(
                        batch_index.saturating_mul(SEARCH_PIPELINE_BATCH),
                        chunk,
                    );
                    if sender.send(batch).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| PersistedSearchError::with("spawn search document producer", error))?;
        let mut failure = None;
        loop {
            let wait_started = Instant::now();
            let batch = match receiver.recv() {
                Ok(batch) => batch,
                Err(_) => break,
            };
            pipeline_wait_us = pipeline_wait_us.saturating_add(elapsed_us(wait_started));
            crate::cooperative_work::checkpoint();
            batches = batches.saturating_add(1);
            document_build_us = document_build_us.saturating_add(batch.build_us);
            if failure.is_some() {
                continue;
            }
            merge_autocomplete_words(&mut words, batch.words);
            let insert_started = Instant::now();
            for document in batch.documents {
                let rowid = match i64::try_from(document.ordinal.saturating_add(1)) {
                    Ok(rowid) => rowid,
                    Err(_) => {
                        failure = Some(PersistedSearchError::new(
                            "game ordinal exceeds SQLite integer",
                        ));
                        break;
                    }
                };
                if let Err(error) = insert_search.execute(rusqlite::params![
                    rowid,
                    document.title,
                    document.compact_title,
                    document.manufacturer,
                    document.compact_manufacturer,
                    document.control,
                    document.compact_control,
                    document.players,
                    document.year,
                    document.decade,
                    document.path,
                    document.compact_path,
                ]) {
                    failure = Some(PersistedSearchError::with("insert search row", error));
                    break;
                }
            }
            fts_insert_us = fts_insert_us.saturating_add(elapsed_us(insert_started));
        }
        producer
            .join()
            .map_err(|_| PersistedSearchError::new("search document producer panicked"))?;
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    })?;
    drop(main_background_scope);
    drop(insert_search);
    drop(row_loop_pmu);
    let row_loop_us = elapsed_us(row_loop_started);
    let word_count = words.len();
    let autocomplete_sort_started = Instant::now();
    let autocomplete_sort_pmu =
        mister_magik_perf_events::sampled_span(crate::pmu_phase::SEARCH_AUTOCOMPLETE_SORT);
    let mut ordered_words = words.into_iter().collect::<Vec<_>>();
    ordered_words.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    drop(autocomplete_sort_pmu);
    let autocomplete_sort_us = elapsed_us(autocomplete_sort_started);

    let autocomplete_insert_started = Instant::now();
    let autocomplete_insert_pmu =
        mister_magik_perf_events::sampled_span(crate::pmu_phase::SEARCH_AUTOCOMPLETE_INSERT);
    let mut insert_word = connection
        .prepare("INSERT INTO autocomplete_words(word,source_rank,score) VALUES (?1,?2,?3)")
        .map_err(|error| PersistedSearchError::with("prepare autocomplete rows", error))?;
    for (word, stats) in &ordered_words {
        insert_word
            .execute(rusqlite::params![word, stats.source_rank, stats.score])
            .map_err(|error| PersistedSearchError::with("insert autocomplete row", error))?;
    }
    drop(insert_word);
    drop(autocomplete_insert_pmu);
    let autocomplete_insert_us = elapsed_us(autocomplete_insert_started);
    let optimize_started = Instant::now();
    let optimize_pmu = mister_magik_perf_events::sampled_span(crate::pmu_phase::SEARCH_OPTIMIZE);
    if optimize {
        connection
            .execute(
                "INSERT INTO game_search_fts(game_search_fts) VALUES ('optimize')",
                [],
            )
            .map_err(|error| PersistedSearchError::with("optimize FTS index", error))?;
    }
    drop(optimize_pmu);
    let optimize_us = elapsed_us(optimize_started);
    let optimize_mode = if optimize { "enabled" } else { "disabled" };
    let automerge_restore_started = Instant::now();
    connection
        .execute(
            "INSERT INTO game_search_fts(game_search_fts,rank) VALUES ('automerge',4)",
            [],
        )
        .map_err(|error| PersistedSearchError::with("restore FTS automerge", error))?;
    let automerge_restore_us = elapsed_us(automerge_restore_started);
    let integrity_started = Instant::now();
    let integrity_pmu = mister_magik_perf_events::sampled_span(crate::pmu_phase::SEARCH_INTEGRITY);
    let integrity_mode = if std::env::var("MISTER_CATALOG_FTS_INTEGRITY")
        .is_ok_and(|value| value.eq_ignore_ascii_case("full"))
    {
        connection
            .execute(
                "INSERT INTO game_search_fts(game_search_fts) VALUES ('integrity-check')",
                [],
            )
            .map_err(|error| PersistedSearchError::with("check FTS integrity", error))?;
        "full"
    } else {
        bounded_integrity_check(connection, games)?;
        "bounded"
    };
    drop(integrity_pmu);
    Ok(PersistedSearchBuildOutcome {
        words: word_count,
        batches,
        document_build_us,
        fts_insert_us,
        pipeline_wait_us,
        row_loop_us,
        autocomplete_sort_us,
        autocomplete_insert_us,
        optimize_us,
        optimize_mode,
        automerge_restore_us,
        integrity_us: elapsed_us(integrity_started),
        integrity_mode,
        source_checksum,
        total_us: elapsed_us(total_started),
    })
}

#[cfg(feature = "builder")]
fn bounded_integrity_check(
    connection: &Connection,
    games: &[crate::system_shard::SystemGame],
) -> Result<(), PersistedSearchError> {
    if games.is_empty() {
        return Ok(());
    }
    let first_rowid: i64 = connection
        .query_row(
            "SELECT rowid FROM game_search_fts WHERE rowid=1 LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| PersistedSearchError::with("probe first FTS row", error))?;
    if first_rowid != 1 {
        return Err(PersistedSearchError::new("first FTS rowid is invalid"));
    }
    let last_rowid = i64::try_from(games.len())
        .map_err(|_| PersistedSearchError::new("FTS document count exceeds SQLite integer"))?;
    let stored_last: i64 = connection
        .query_row(
            "SELECT rowid FROM game_search_fts WHERE rowid=?1 LIMIT 1",
            [last_rowid],
            |row| row.get(0),
        )
        .map_err(|error| PersistedSearchError::with("probe last FTS row", error))?;
    if stored_last != last_rowid {
        return Err(PersistedSearchError::new("last FTS rowid is invalid"));
    }
    let probe = games.iter().find_map(|game| {
        normalize_search_text(&game.title)
            .split_whitespace()
            .find(|token| !token.is_empty())
            .map(str::to_owned)
    });
    if let Some(token) = probe {
        let matched: bool = connection
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM game_search_fts
                     WHERE game_search_fts MATCH ?1
                     LIMIT 1
                 )",
                [token],
                |row| row.get(0),
            )
            .map_err(|error| PersistedSearchError::with("probe FTS search", error))?;
        if !matched {
            return Err(PersistedSearchError::new(
                "bounded FTS search probe returned no row",
            ));
        }
    }
    Ok(())
}

#[cfg(feature = "builder")]
fn search_source_checksum(games: &[crate::system_shard::SystemGame]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"mister-magik-search-source-v1\0");
    for game in games {
        digest.update(game.stable_key.as_bytes());
        digest.update([0]);
        digest.update(game.title.as_bytes());
        digest.update([0]);
        digest.update(game.launch_ref.as_bytes());
        digest.update([0]);
    }
    let digest = digest.finalize();
    crate::library_db::hex_lower(&digest)
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
    let source_checksum = search_meta_text(connection, "search_source_sha256")?;
    if source_checksum.len() != 64 || !source_checksum.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(PersistedSearchError::new(
            "persisted search source checksum is invalid",
        ));
    }
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

fn search_meta_text(connection: &Connection, key: &str) -> Result<String, PersistedSearchError> {
    connection
        .query_row("SELECT value FROM shard_meta WHERE key=?1", [key], |row| {
            row.get(0)
        })
        .map_err(|error| PersistedSearchError::with("read search metadata", error))
}

fn open_read_only(path: &Path) -> Result<Connection, PersistedSearchError> {
    SEARCH_SQLITE_OPENS.fetch_add(1, AtomicOrdering::Relaxed);
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
#[derive(Debug, Default, Eq, PartialEq)]
struct AutocompleteStats {
    source_rank: u8,
    score: u32,
}

#[cfg(feature = "builder")]
fn add_normalized_words(
    words: &mut std::collections::HashMap<String, AutocompleteStats>,
    normalized: &str,
    source: AutocompleteSource,
) {
    for word in normalized.split_whitespace() {
        add_normalized_word(words, word, source);
    }
}

#[cfg(feature = "builder")]
fn add_normalized_word(
    words: &mut std::collections::HashMap<String, AutocompleteStats>,
    normalized: &str,
    source: AutocompleteSource,
) {
    if normalized.len() < 2 || is_noisy_autocomplete_word(normalized) {
        return;
    }
    let (score, source_rank) = match source {
        AutocompleteSource::Title => (5, 2),
        AutocompleteSource::Metadata => (4, 2),
        AutocompleteSource::Path => (1, 1),
    };
    let stats = words.entry(normalized.to_string()).or_default();
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
    use crate::shard_registry::{CatalogManifest, ManifestSystem, PublishedGeneration};
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

    #[test]
    fn bulk_population_restores_default_fts_automerge() {
        let (root, sqlite) = fixture();
        let connection = Connection::open(sqlite).unwrap();
        let automerge: u32 = connection
            .query_row(
                "SELECT v FROM game_search_fts_config WHERE k='automerge'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(automerge, 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_search_pipeline_preserves_batch_boundary_ordinals_and_words() {
        let games = (0..=SEARCH_PIPELINE_BATCH)
            .map(|ordinal| SystemGame {
                stable_key: format!("game-{ordinal}"),
                title: format!("Pokémon Game-{ordinal}"),
                launch_ref: format!("/media/fat/games/C64/Game-{ordinal}.d64"),
                manufacturer: "Commodore, Inc.".to_string(),
                ..SystemGame::default()
            })
            .collect::<Vec<_>>();
        let first = prepare_search_batch(0, &games[..SEARCH_PIPELINE_BATCH]);
        let tail = prepare_search_batch(SEARCH_PIPELINE_BATCH, &games[SEARCH_PIPELINE_BATCH..]);

        assert_eq!(first.documents.len(), SEARCH_PIPELINE_BATCH);
        assert_eq!(first.documents[0].ordinal, 0);
        assert_eq!(first.documents[SEARCH_PIPELINE_BATCH - 1].ordinal, 255);
        assert_eq!(tail.documents.len(), 1);
        assert_eq!(tail.documents[0].ordinal, 256);
        assert_eq!(tail.documents[0].title, "pok mon game 256");

        let mut merged = first.words;
        merge_autocomplete_words(&mut merged, tail.words);
        let complete = prepare_search_batch(0, &games);
        assert_eq!(merged, complete.words);
    }

    #[test]
    fn snapshot_search_reuses_the_supplied_manifest() {
        let (root, sqlite) = fixture();
        let sqlite_bytes = fs::metadata(&sqlite).unwrap().len();
        let catalog = PersistedSearchCatalog {
            storage_root: root.clone(),
            manifest: Arc::new(CatalogManifest {
                format: None,
                generation: 1,
                systems: vec![ManifestSystem {
                    system_id: SystemId::parse("arcade").unwrap(),
                    display_title: "Arcade".to_string(),
                    section: "Arcade".to_string(),
                    family: "Arcade".to_string(),
                    order: 0,
                    producers: Vec::new(),
                    active: PublishedGeneration {
                        generation: 1,
                        sqlite_path: sqlite.file_name().unwrap().into(),
                        navigation_path: "1.nav.lz4b".into(),
                        sqlite_bytes,
                        navigation_bytes: 0,
                        sqlite_hash: String::new(),
                        navigation_hash: String::new(),
                        games: 2,
                        navpack: None,
                    },
                    previous: None,
                }],
            }),
        };

        assert!(catalog.contains_system("arcade"));
        assert!(!catalog.contains_system("console"));
        let result = catalog.search(&["arcade".to_string()], "pacman").unwrap();
        assert_eq!(result.matches[0].ordinal, 0);

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
