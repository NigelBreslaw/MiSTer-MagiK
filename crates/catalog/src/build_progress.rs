// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Disposable durable progress for an interrupted catalog build.
//!
//! This database is never catalog authority. A caller must still publish the
//! normal shard manifest, binding, scanner cache, and catalog state in order.

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

const FILE_NAME: &str = "build-progress-v3";
// This cache must not share a path with `incremental_inputs::InputFactStore`.
// They are independent SQLite schemas with different lifecycle owners.
const COMMITTED_FILE_NAME: &str = "target-output-cache-v3";
const METADATA_FILE_NAME: &str = "metadata.sqlite3";
const FRAME_FILE_NAME: &str = "target-outputs.lz4";
const LEGACY_PROGRESS_FILE_NAME: &str = "build-progress.sqlite3";
const LEGACY_COMMITTED_FILE_NAME: &str = "target-output-cache.sqlite3";
const SCHEMA_VERSION: u32 = 4;
const MAX_TARGET_OUTPUT_BYTES: usize = 256 * 1024 * 1024;
const TARGET_FRAME_CHUNK_BYTES: usize = 256 * 1024;
const MAX_ENCODED_CHUNK_BYTES: usize = TARGET_FRAME_CHUNK_BYTES * 2;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildContract {
    pub active_manifest_generation: Option<u64>,
    pub roots: Vec<String>,
    pub path_mapping: Vec<(String, String)>,
    pub scanner_version: u32,
    pub profile_version: String,
    pub taxonomy_version: String,
    pub namespace_backend: String,
    pub projection_contract: String,
    pub rom_inventory_fingerprint: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildStats {
    pub normal_files: u64,
    pub containers: u64,
    pub entries: u64,
    pub audit_rows: u64,
    pub discoveries: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanTarget {
    pub ordinal: u32,
    pub key: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompletedTarget {
    pub target: ScanTarget,
    pub input_fingerprint: String,
    /// Versioned scanner-owned JSON. The journal deliberately does not
    /// interpret payload/container/entry/discovery facts.
    pub output_json: String,
    pub accumulated_stats: BuildStats,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompletedShard {
    pub system_id: String,
    pub generation: u64,
    pub sqlite_path: String,
    pub navigation_path: String,
    pub content_hash: String,
    pub manifest_system_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuildProgressSummary {
    pub build_id: String,
    pub total_targets: u64,
    pub completed_targets: u64,
    pub completed_shards: u64,
    pub last_completed_ordinal: Option<u32>,
    pub last_completed_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenStatus {
    Created,
    Resumed,
    Recreated { reason: String },
}

pub struct BuildProgressJournal {
    /// Bundle directory containing metadata and sequential target frames.
    path: PathBuf,
    conn: Connection,
    build_id: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CheckpointWriteAttribution {
    pub targets: usize,
    pub raw_bytes: usize,
    pub frame_bytes: usize,
    pub begin_us: u64,
    pub compress_us: u64,
    pub append_us: u64,
    pub sync_us: u64,
    pub rows_us: u64,
    pub commit_us: u64,
    pub total_us: u64,
}

pub fn path_for_root(storage_root: &Path) -> PathBuf {
    storage_root.join("state").join(FILE_NAME)
}

pub fn committed_path_for_root(storage_root: &Path) -> PathBuf {
    storage_root.join("state").join(COMMITTED_FILE_NAME)
}

fn metadata_path(bundle: &Path) -> PathBuf {
    bundle.join(METADATA_FILE_NAME)
}

fn frame_path(bundle: &Path) -> PathBuf {
    bundle.join(FRAME_FILE_NAME)
}

fn remove_legacy_siblings(bundle: &Path) {
    let Some(parent) = bundle.parent() else {
        return;
    };
    for name in [LEGACY_PROGRESS_FILE_NAME, LEGACY_COMMITTED_FILE_NAME] {
        let path = parent.join(name);
        if path.is_file() {
            let _ = fs::remove_file(path);
        }
    }
}

/// Move the last successfully published scan facts into the resumable journal.
///
/// Both paths must share a directory so the rename is atomic. An interrupted
/// run leaves the journal at `progress_path`, where `open_or_create` resumes it.
pub fn seed_from_committed(committed_path: &Path, progress_path: &Path) -> Result<bool, String> {
    if progress_path.exists() || !committed_path.exists() {
        return Ok(false);
    }
    if let Some(parent) = progress_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create build progress dir {}: {error}", parent.display()))?;
    }
    require_shared_parent(committed_path, progress_path)?;
    std::fs::rename(committed_path, progress_path).map_err(|error| {
        format!(
            "move build progress {} from {}: {error}",
            progress_path.display(),
            committed_path.display()
        )
    })?;
    crate::sqlite_catalog::sync_parent_dir(progress_path);
    Ok(true)
}

/// Atomically retain the completed target facts from a successful run.
///
/// Completed shards belong to one intended generation and are deliberately
/// omitted from committed planner state. They remain available in the mutable
/// progress journal until the authoritative publication has completed.
pub fn commit_successful_state(
    progress_path: &Path,
    committed_path: &Path,
    published_generation: u64,
) -> Result<bool, String> {
    if !progress_path.exists() {
        return Ok(false);
    }
    if let Some(parent) = committed_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "create committed builder state dir {}: {error}",
                parent.display()
            )
        })?;
    }
    let conn = Connection::open(metadata_path(progress_path))
        .map_err(|error| format!("open successful build progress: {error}"))?;
    configure(&conn)?;
    let mut contract: BuildContract = serde_json::from_str(&meta(&conn, "contract")?)
        .map_err(|error| format!("decode successful build contract: {error}"))?;
    contract.active_manifest_generation = Some(published_generation);
    let contract_json = serde_json::to_string(&contract)
        .map_err(|error| format!("encode successful build contract: {error}"))?;
    conn.execute(
        "UPDATE progress_meta SET value=?1 WHERE key='contract'",
        [&contract_json],
    )
    .map_err(|error| format!("bind successful build generation: {error}"))?;
    conn.execute("DELETE FROM completed_shards", [])
        .map_err(|error| format!("clear generation-specific shard checkpoints: {error}"))?;
    drop(conn);

    require_shared_parent(progress_path, committed_path)?;
    let progress = OpenOptions::new()
        .read(true)
        .open(metadata_path(progress_path))
        .map_err(|error| format!("open build progress {}: {error}", progress_path.display()))?;
    progress
        .sync_all()
        .map_err(|error| format!("sync build progress {}: {error}", progress_path.display()))?;
    let frames = OpenOptions::new()
        .read(true)
        .open(frame_path(progress_path))
        .map_err(|error| format!("open target frames {}: {error}", progress_path.display()))?;
    frames
        .sync_all()
        .map_err(|error| format!("sync target frames {}: {error}", progress_path.display()))?;
    std::fs::rename(progress_path, committed_path).map_err(|error| {
        format!(
            "publish committed builder state {}: {error}",
            committed_path.display()
        )
    })?;
    crate::sqlite_catalog::sync_parent_dir(committed_path);
    Ok(true)
}

fn require_shared_parent(left: &Path, right: &Path) -> Result<(), String> {
    if left.parent() == right.parent() && left.parent().is_some() {
        Ok(())
    } else {
        Err(format!(
            "build progress paths must share a parent: {} and {}",
            left.display(),
            right.display()
        ))
    }
}

pub fn remove(path: &Path) -> Result<(), String> {
    let result = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match result {
        Ok(()) => {
            crate::sqlite_catalog::sync_parent_dir(path);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove build progress {}: {error}", path.display())),
    }
}

/// Reads bounded journal counters without taking write ownership of disposable
/// build progress. This is diagnostics only; catalog authority remains the
/// published manifest and state.
pub fn read_summary(path: &Path) -> Result<Option<BuildProgressSummary>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(
        metadata_path(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("open build progress summary {}: {error}", path.display()))?;
    let version: String = meta(&conn, "schema_version")?;
    if version != SCHEMA_VERSION.to_string() {
        return Err(format!("unsupported build progress schema {version}"));
    }
    let last = conn
        .query_row(
            "SELECT t.ordinal,t.path
             FROM scan_targets t JOIN completed_targets c USING(ordinal)
             ORDER BY t.ordinal DESC LIMIT 1",
            [],
            |row| Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| format!("read last completed target: {error}"))?;
    Ok(Some(BuildProgressSummary {
        build_id: meta(&conn, "build_id")?,
        total_targets: row_count(&conn, "scan_targets")?,
        completed_targets: row_count(&conn, "completed_targets")?,
        completed_shards: row_count(&conn, "completed_shards")?,
        last_completed_ordinal: last.as_ref().map(|(ordinal, _)| *ordinal),
        last_completed_path: last.map(|(_, path)| path),
    }))
}

fn row_count(conn: &Connection, table: &str) -> Result<u64, String> {
    if !matches!(
        table,
        "scan_targets" | "completed_targets" | "completed_shards"
    ) {
        return Err("unsupported build progress count table".to_string());
    }
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|count| count.max(0) as u64)
    .map_err(|error| format!("count build progress {table}: {error}"))
}

impl BuildProgressJournal {
    pub fn open_for_projection(path: &Path) -> Result<Self, String> {
        let conn =
            Connection::open_with_flags(metadata_path(path), OpenFlags::SQLITE_OPEN_READ_WRITE)
                .map_err(|error| format!("open build progress {}: {error}", path.display()))?;
        configure(&conn)?;
        let version: String = meta(&conn, "schema_version")?;
        if version != SCHEMA_VERSION.to_string() {
            return Err(format!("unsupported build progress schema {version}"));
        }
        let journal = Self {
            path: path.to_path_buf(),
            build_id: meta(&conn, "build_id")?,
            conn,
        };
        journal.completed_targets()?;
        journal.completed_shards()?;
        Ok(journal)
    }
    pub fn open_or_create(
        path: &Path,
        contract: &BuildContract,
        targets: &[ScanTarget],
    ) -> Result<(Self, OpenStatus), String> {
        remove_legacy_siblings(path);
        if path.exists() {
            match Self::open_existing(path, contract, targets) {
                Ok(journal) => return Ok((journal, OpenStatus::Resumed)),
                Err(reason) => {
                    remove(path)?;
                    let journal = Self::create(path, contract, targets)?;
                    return Ok((journal, OpenStatus::Recreated { reason }));
                }
            }
        }
        Ok((Self::create(path, contract, targets)?, OpenStatus::Created))
    }

    pub fn build_id(&self) -> &str {
        &self.build_id
    }

    pub fn completed_targets(&self) -> Result<Vec<CompletedTarget>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT t.ordinal,t.target_key,t.path,c.input_fingerprint,
                    c.frame_offset,c.frame_len,c.raw_len,c.frame_sha256,c.stats_json
             FROM scan_targets t JOIN completed_targets c USING(ordinal) ORDER BY t.ordinal",
            )
            .map_err(|error| format!("prepare completed targets: {error}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })
            .map_err(|error| format!("read completed targets: {error}"))?;
        rows.map(|row| {
            let (
                ordinal,
                key,
                path,
                input_fingerprint,
                frame_offset,
                frame_len,
                raw_len,
                frame_sha256,
                stats_json,
            ) = row.map_err(|error| format!("read completed target: {error}"))?;
            let accumulated_stats = serde_json::from_str(&stats_json)
                .map_err(|error| format!("decode completed target stats: {error}"))?;
            let output_json = self.read_target_frame(
                checked_frame_value(frame_offset, "offset")?,
                checked_frame_value(frame_len, "length")?,
                checked_frame_value(raw_len, "raw length")?,
                &frame_sha256,
            )?;
            Ok(CompletedTarget {
                target: ScanTarget { ordinal, key, path },
                input_fingerprint,
                output_json,
                accumulated_stats,
            })
        })
        .collect()
    }

    fn read_target_frame(
        &self,
        offset: u64,
        frame_len: u64,
        raw_len: u64,
        expected_sha256: &str,
    ) -> Result<String, String> {
        let frame_len = usize::try_from(frame_len)
            .map_err(|_| "target frame length exceeds address space".to_string())?;
        let raw_len = usize::try_from(raw_len)
            .map_err(|_| "target raw length exceeds address space".to_string())?;
        if frame_len == 0 || raw_len > MAX_TARGET_OUTPUT_BYTES {
            return Err("target frame has invalid bounds".to_string());
        }
        let mut frames = File::open(frame_path(&self.path))
            .map_err(|error| format!("open target frames: {error}"))?;
        let available = frames
            .metadata()
            .map_err(|error| format!("stat target frames: {error}"))?
            .len();
        if offset
            .checked_add(frame_len as u64)
            .is_none_or(|end| end > available)
        {
            return Err("target frame extends past committed data".to_string());
        }
        frames
            .seek(SeekFrom::Start(offset))
            .map_err(|error| format!("seek target frame: {error}"))?;
        let mut remaining = frame_len;
        let mut decoded = Vec::with_capacity(raw_len);
        let mut digest = Sha256::new();
        while remaining != 0 {
            crate::cooperative_work::checkpoint();
            if remaining < size_of::<u32>() {
                return Err("target frame has a truncated chunk header".to_string());
            }
            let mut header = [0u8; size_of::<u32>()];
            frames
                .read_exact(&mut header)
                .map_err(|error| format!("read target frame chunk header: {error}"))?;
            digest.update(header);
            remaining -= header.len();
            let encoded_len = u32::from_le_bytes(header) as usize;
            if encoded_len == 0 || encoded_len > MAX_ENCODED_CHUNK_BYTES || encoded_len > remaining
            {
                return Err("target frame has invalid chunk bounds".to_string());
            }
            let mut encoded = vec![0; encoded_len];
            frames
                .read_exact(&mut encoded)
                .map_err(|error| format!("read target frame chunk: {error}"))?;
            digest.update(&encoded);
            remaining -= encoded_len;
            let chunk = lz4_flex::decompress_size_prepended(&encoded)
                .map_err(|error| format!("decompress target frame chunk: {error}"))?;
            if chunk.len() > TARGET_FRAME_CHUNK_BYTES
                || decoded.len().saturating_add(chunk.len()) > raw_len
            {
                return Err("target frame chunk exceeds decoded bounds".to_string());
            }
            decoded.extend_from_slice(&chunk);
        }
        if hex_bytes(&digest.finalize()) != expected_sha256 {
            return Err("target frame checksum mismatch".to_string());
        }
        if decoded.len() != raw_len {
            return Err("target frame decoded length mismatch".to_string());
        }
        String::from_utf8(decoded).map_err(|error| format!("decode target frame UTF-8: {error}"))
    }

    /// Atomically makes all output for one target resumable.
    pub fn checkpoint_target(
        &mut self,
        completed: &CompletedTarget,
    ) -> Result<CheckpointWriteAttribution, String> {
        self.checkpoint_targets(std::slice::from_ref(completed))
    }

    /// Atomically makes a bounded group of target outputs resumable with one
    /// durable SQLite commit. Callers bound both row count and encoded bytes.
    pub fn checkpoint_targets(
        &mut self,
        completed: &[CompletedTarget],
    ) -> Result<CheckpointWriteAttribution, String> {
        let total_started = Instant::now();
        if completed.is_empty() {
            return Ok(CheckpointWriteAttribution::default());
        }
        let raw_bytes = completed
            .iter()
            .map(|target| target.output_json.len())
            .sum();
        let mut compress_us = 0u64;
        let mut append_us = 0u64;
        let mut frame_bytes = 0usize;
        let mut frames = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(frame_path(&self.path))
            .map_err(|error| format!("open target frame append: {error}"))?;
        let mut offset = frames
            .seek(SeekFrom::End(0))
            .map_err(|error| format!("seek target frame append: {error}"))?;
        let mut frame_rows = Vec::with_capacity(completed.len());
        for target in completed {
            let frame_offset = offset;
            let mut digest = Sha256::new();
            for chunk in target
                .output_json
                .as_bytes()
                .chunks(TARGET_FRAME_CHUNK_BYTES)
            {
                crate::cooperative_work::checkpoint();
                let compress_started = Instant::now();
                let encoded = lz4_flex::compress_prepend_size(chunk);
                compress_us = compress_us.saturating_add(elapsed_us(compress_started));
                let encoded_len = u32::try_from(encoded.len())
                    .map_err(|_| "encoded target frame chunk is too large".to_string())?;
                let header = encoded_len.to_le_bytes();
                let append_started = Instant::now();
                frames
                    .write_all(&header)
                    .and_then(|()| frames.write_all(&encoded))
                    .map_err(|error| format!("append target frame chunk: {error}"))?;
                append_us = append_us.saturating_add(elapsed_us(append_started));
                digest.update(header);
                digest.update(&encoded);
                let record_len = header.len().saturating_add(encoded.len());
                frame_bytes = frame_bytes.saturating_add(record_len);
                offset = offset.saturating_add(record_len as u64);
            }
            frame_rows.push((
                target,
                frame_offset,
                offset.saturating_sub(frame_offset),
                target.output_json.len() as u64,
                hex_bytes(&digest.finalize()),
            ));
        }
        let sync_started = Instant::now();
        frames
            .sync_data()
            .map_err(|error| format!("sync target frames: {error}"))?;
        let sync_us = elapsed_us(sync_started);
        let begin_started = Instant::now();
        let tx = self
            .conn
            .transaction()
            .map_err(|error| format!("begin target checkpoint batch: {error}"))?;
        let begin_us = elapsed_us(begin_started);
        let rows_started = Instant::now();
        for (completed, frame_offset, frame_len, raw_len, frame_sha256) in frame_rows {
            let frame_offset = sqlite_frame_value(frame_offset, "offset")?;
            let frame_len = sqlite_frame_value(frame_len, "length")?;
            let raw_len = sqlite_frame_value(raw_len, "raw length")?;
            let expected = tx
                .query_row(
                    "SELECT target_key,path FROM scan_targets WHERE ordinal=?1",
                    [completed.target.ordinal],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|error| format!("read checkpoint target: {error}"))?
                .ok_or_else(|| {
                    format!("unknown scan target ordinal {}", completed.target.ordinal)
                })?;
            if expected != (completed.target.key.clone(), completed.target.path.clone()) {
                return Err(format!(
                    "scan target {} does not match journal contract",
                    completed.target.ordinal
                ));
            }
            let stats_json = serde_json::to_string(&completed.accumulated_stats)
                .map_err(|error| format!("encode target stats: {error}"))?;
            tx.execute(
                "INSERT INTO completed_targets(
                     ordinal,input_fingerprint,frame_offset,frame_len,raw_len,frame_sha256,stats_json
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(ordinal) DO UPDATE SET input_fingerprint=excluded.input_fingerprint,
                     frame_offset=excluded.frame_offset,frame_len=excluded.frame_len,
                     raw_len=excluded.raw_len,frame_sha256=excluded.frame_sha256,
                     stats_json=excluded.stats_json",
                params![
                    completed.target.ordinal,
                    completed.input_fingerprint,
                    frame_offset,
                    frame_len,
                    raw_len,
                    frame_sha256,
                    stats_json
                ],
            )
            .map_err(|error| format!("write target checkpoint: {error}"))?;
        }
        let rows_us = elapsed_us(rows_started);
        let commit_started = Instant::now();
        tx.commit()
            .map_err(|error| format!("commit target checkpoint batch: {error}"))?;
        Ok(CheckpointWriteAttribution {
            targets: completed.len(),
            raw_bytes,
            frame_bytes,
            begin_us,
            compress_us,
            append_us,
            sync_us,
            rows_us,
            commit_us: elapsed_us(commit_started),
            total_us: elapsed_us(total_started),
        })
    }

    pub fn record_shard(&mut self, shard: &CompletedShard) -> Result<(), String> {
        self.record_shards(std::slice::from_ref(shard))
    }

    pub fn record_shards(&mut self, shards: &[CompletedShard]) -> Result<(), String> {
        if shards.is_empty() {
            return Ok(());
        }
        let encoded = shards
            .iter()
            .map(|shard| {
                serde_json::to_string(shard)
                    .map(|json| (shard.system_id.as_str(), json))
                    .map_err(|error| format!("encode completed shard: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let tx = self
            .conn
            .transaction()
            .map_err(|error| format!("begin shard checkpoint batch: {error}"))?;
        for (system_id, json) in encoded {
            tx.execute(
                "INSERT INTO completed_shards(system_id,shard_json) VALUES(?1,?2)
                        ON CONFLICT(system_id) DO UPDATE SET shard_json=excluded.shard_json",
                params![system_id, json],
            )
            .map_err(|error| format!("write shard checkpoint: {error}"))?;
        }
        tx.commit()
            .map_err(|error| format!("commit shard checkpoint batch: {error}"))
    }

    pub fn completed_shards(&self) -> Result<Vec<CompletedShard>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT shard_json FROM completed_shards ORDER BY system_id")
            .map_err(|error| format!("prepare completed shards: {error}"))?;
        stmt.query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("read completed shards: {error}"))?
            .map(|row| {
                serde_json::from_str(
                    &row.map_err(|error| format!("read completed shard: {error}"))?,
                )
                .map_err(|error| format!("decode completed shard: {error}"))
            })
            .collect()
    }

    fn create(
        path: &Path,
        contract: &BuildContract,
        targets: &[ScanTarget],
    ) -> Result<Self, String> {
        validate_targets(targets)?;
        fs::create_dir_all(path)
            .map_err(|error| format!("create build progress bundle {}: {error}", path.display()))?;
        File::create(frame_path(path))
            .and_then(|frames| frames.sync_all())
            .map_err(|error| format!("create target frame file {}: {error}", path.display()))?;
        let conn = Connection::open(metadata_path(path))
            .map_err(|error| format!("create build progress {}: {error}", path.display()))?;
        configure(&conn)?;
        conn.execute_batch(
            "CREATE TABLE progress_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL) WITHOUT ROWID;
             CREATE TABLE scan_targets(ordinal INTEGER PRIMARY KEY,target_key TEXT NOT NULL UNIQUE,path TEXT NOT NULL);
             CREATE TABLE completed_targets(ordinal INTEGER PRIMARY KEY REFERENCES scan_targets(ordinal),
                 input_fingerprint TEXT NOT NULL,frame_offset INTEGER NOT NULL,
                 frame_len INTEGER NOT NULL,raw_len INTEGER NOT NULL,
                 frame_sha256 TEXT NOT NULL,stats_json TEXT NOT NULL);
             CREATE TABLE completed_shards(system_id TEXT PRIMARY KEY,shard_json TEXT NOT NULL) WITHOUT ROWID;"
        ).map_err(|error| format!("create build progress schema: {error}"))?;
        let build_id = new_build_id();
        let contract_json = serde_json::to_string(contract)
            .map_err(|error| format!("encode build contract: {error}"))?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|error| format!("begin build progress: {error}"))?;
        tx.execute(
            "INSERT INTO progress_meta(key,value) VALUES('schema_version',?1)",
            [SCHEMA_VERSION.to_string()],
        )
        .map_err(|error| format!("write progress schema: {error}"))?;
        tx.execute(
            "INSERT INTO progress_meta(key,value) VALUES('build_id',?1)",
            [&build_id],
        )
        .map_err(|error| format!("write progress build id: {error}"))?;
        tx.execute(
            "INSERT INTO progress_meta(key,value) VALUES('contract',?1)",
            [&contract_json],
        )
        .map_err(|error| format!("write progress contract: {error}"))?;
        for target in targets {
            tx.execute(
                "INSERT INTO scan_targets(ordinal,target_key,path) VALUES(?1,?2,?3)",
                params![target.ordinal, target.key, target.path],
            )
            .map_err(|error| format!("write scan target: {error}"))?;
        }
        tx.commit()
            .map_err(|error| format!("commit build progress: {error}"))?;
        Ok(Self {
            path: path.to_path_buf(),
            conn,
            build_id,
        })
    }

    fn open_existing(
        path: &Path,
        contract: &BuildContract,
        targets: &[ScanTarget],
    ) -> Result<Self, String> {
        validate_targets(targets)?;
        let mut conn = Connection::open(metadata_path(path))
            .map_err(|error| format!("open build progress {}: {error}", path.display()))?;
        configure(&conn)?;
        let version: String = meta(&conn, "schema_version")?;
        if version != SCHEMA_VERSION.to_string() {
            return Err(format!("unsupported build progress schema {version}"));
        }
        let stored_contract: BuildContract = serde_json::from_str(&meta(&conn, "contract")?)
            .map_err(|error| format!("decode build contract: {error}"))?;
        if &stored_contract != contract {
            return Err("build contract changed".to_string());
        }
        let stored_targets = read_targets(&conn)?;
        if stored_targets != targets {
            reconcile_targets(&mut conn, &stored_targets, targets)?;
        }
        let build_id = meta(&conn, "build_id")?;
        if !frame_path(path).is_file() {
            return Err("target frame file is missing".to_string());
        }
        // Decode every durable row now; corrupt recovery data is disposable.
        let journal = Self {
            path: path.to_path_buf(),
            conn,
            build_id,
        };
        journal.completed_targets()?;
        journal.completed_shards()?;
        Ok(journal)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn reconcile_targets(
    conn: &mut Connection,
    stored: &[ScanTarget],
    current: &[ScanTarget],
) -> Result<(), String> {
    let mut saved = std::collections::HashMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT ordinal,input_fingerprint,frame_offset,frame_len,raw_len,frame_sha256,stats_json
                 FROM completed_targets",
            )
            .map_err(|error| format!("prepare target reconciliation: {error}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(|error| format!("read target reconciliation: {error}"))?;
        for row in rows {
            let (ordinal, fingerprint, offset, frame_len, raw_len, frame_sha256, stats) =
                row.map_err(|error| format!("read target reconciliation row: {error}"))?;
            if let Some(target) = stored.iter().find(|target| target.ordinal == ordinal) {
                saved.insert(
                    (target.key.clone(), target.path.clone()),
                    (fingerprint, offset, frame_len, raw_len, frame_sha256, stats),
                );
            }
        }
    }
    let tx = conn
        .transaction()
        .map_err(|error| format!("begin target reconciliation: {error}"))?;
    tx.execute("DELETE FROM completed_targets", [])
        .map_err(|error| format!("clear completed targets: {error}"))?;
    tx.execute("DELETE FROM scan_targets", [])
        .map_err(|error| format!("clear scan targets: {error}"))?;
    for target in current {
        tx.execute(
            "INSERT INTO scan_targets(ordinal,target_key,path) VALUES(?1,?2,?3)",
            params![target.ordinal, target.key, target.path],
        )
        .map_err(|error| format!("reconcile scan target: {error}"))?;
        if let Some((fingerprint, offset, frame_len, raw_len, frame_sha256, stats)) =
            saved.get(&(target.key.clone(), target.path.clone()))
        {
            tx.execute(
                "INSERT INTO completed_targets(
                     ordinal,input_fingerprint,frame_offset,frame_len,raw_len,frame_sha256,stats_json
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    target.ordinal,
                    fingerprint,
                    offset,
                    frame_len,
                    raw_len,
                    frame_sha256,
                    stats
                ],
            )
            .map_err(|error| format!("restore reconciled target: {error}"))?;
        }
    }
    tx.commit()
        .map_err(|error| format!("commit target reconciliation: {error}"))
}

fn configure(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;",
    )
    .map_err(|error| format!("configure build progress: {error}"))?;
    Ok(())
}

fn checked_frame_value(value: i64, label: &str) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("target frame {label} is negative"))
}

fn sqlite_frame_value(value: u64, label: &str) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("target frame {label} exceeds SQLite integer"))
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn meta(conn: &Connection, key: &str) -> Result<String, String> {
    conn.query_row(
        "SELECT value FROM progress_meta WHERE key=?1",
        [key],
        |row| row.get(0),
    )
    .map_err(|error| format!("read build progress {key}: {error}"))
}

fn read_targets(conn: &Connection) -> Result<Vec<ScanTarget>, String> {
    let mut stmt = conn
        .prepare("SELECT ordinal,target_key,path FROM scan_targets ORDER BY ordinal")
        .map_err(|error| format!("prepare scan targets: {error}"))?;
    stmt.query_map([], |row| {
        Ok(ScanTarget {
            ordinal: row.get(0)?,
            key: row.get(1)?,
            path: row.get(2)?,
        })
    })
    .map_err(|error| format!("read scan targets: {error}"))?
    .map(|row| row.map_err(|error| format!("read scan target: {error}")))
    .collect()
}

fn validate_targets(targets: &[ScanTarget]) -> Result<(), String> {
    for (index, target) in targets.iter().enumerate() {
        if target.ordinal as usize != index {
            return Err("scan target ordinals must be contiguous from zero".to_string());
        }
        if target.key.is_empty() || target.path.is_empty() {
            return Err("scan target key and path must not be empty".to_string());
        }
    }
    Ok(())
}

fn new_build_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}-{:x}", std::process::id(), nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mister-magik-{name}-{}-{}.sqlite3",
            std::process::id(),
            new_build_id()
        ))
    }
    fn contract() -> BuildContract {
        BuildContract {
            active_manifest_generation: None,
            roots: vec!["/games".into()],
            path_mapping: vec![],
            scanner_version: 12,
            profile_version: "p1".into(),
            taxonomy_version: "t1".into(),
            namespace_backend: "native".into(),
            projection_contract: "v3".into(),
            rom_inventory_fingerprint: "roms-v1".into(),
        }
    }
    fn targets() -> Vec<ScanTarget> {
        vec![ScanTarget {
            ordinal: 0,
            key: "arcade".into(),
            path: "/games/arcade".into(),
        }]
    }
    fn completed() -> CompletedTarget {
        CompletedTarget {
            target: targets().remove(0),
            input_fingerprint: "abc".into(),
            output_json: r#"{"files":1}"#.into(),
            accumulated_stats: BuildStats {
                normal_files: 1,
                ..BuildStats::default()
            },
        }
    }

    #[test]
    fn committed_target_cache_has_unique_schema_ownership() {
        let root = Path::new("/catalog-v3");

        assert_eq!(
            committed_path_for_root(root),
            root.join("state/target-output-cache-v3")
        );
        assert_ne!(
            committed_path_for_root(root),
            root.join("state/builder-state.sqlite3")
        );
    }

    #[test]
    fn projection_probe_does_not_create_a_missing_journal() {
        let path = temp_path("missing-projection-journal");

        assert!(BuildProgressJournal::open_for_projection(&path).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn projection_probe_keeps_an_existing_journal_writable() {
        let path = temp_path("writable-projection-journal");
        drop(BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap());
        let mut journal = BuildProgressJournal::open_for_projection(&path).unwrap();
        journal
            .record_shard(&CompletedShard {
                system_id: "arcade".into(),
                generation: 1,
                sqlite_path: "systems/arcade/1.sqlite3".into(),
                navigation_path: "systems/arcade/1.nav".into(),
                content_hash: "abc".into(),
                manifest_system_json: "{}".into(),
            })
            .unwrap();
        drop(journal);
        remove(&path).unwrap();
    }

    #[test]
    fn journal_round_trip_retains_id_targets_and_shards() {
        let path = temp_path("build-progress-round-trip");
        let (mut journal, status) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        assert_eq!(status, OpenStatus::Created);
        let id = journal.build_id().to_string();
        let attribution = journal.checkpoint_target(&completed()).unwrap();
        assert_eq!(attribution.targets, 1);
        assert_eq!(attribution.raw_bytes, completed().output_json.len());
        assert!(attribution.frame_bytes > 0);
        let shard = CompletedShard {
            system_id: "arcade".into(),
            generation: 4,
            sqlite_path: "systems/arcade/4.sqlite3".into(),
            navigation_path: "systems/arcade/4.nav".into(),
            content_hash: "def".into(),
            manifest_system_json: "{}".into(),
        };
        let snes = CompletedShard {
            system_id: "snes".into(),
            generation: 4,
            sqlite_path: "systems/snes/4.sqlite3".into(),
            navigation_path: "systems/snes/4.nav".into(),
            content_hash: "ghi".into(),
            manifest_system_json: "{}".into(),
        };
        journal
            .record_shards(&[shard.clone(), snes.clone()])
            .unwrap();
        drop(journal);
        let (journal, status) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        assert_eq!(status, OpenStatus::Resumed);
        assert_eq!(journal.build_id(), id);
        assert_eq!(journal.completed_targets().unwrap(), vec![completed()]);
        assert_eq!(journal.completed_shards().unwrap(), vec![shard, snes]);
        remove(&path).unwrap();
    }

    #[test]
    fn large_target_frame_round_trips_as_bounded_records() {
        let path = temp_path("build-progress-chunked-frame");
        let (mut journal, _) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        let mut large = completed();
        large.output_json = "x".repeat(TARGET_FRAME_CHUNK_BYTES * 2 + 17);
        let attribution = journal.checkpoint_target(&large).unwrap();
        drop(journal);

        let mut frames = File::open(frame_path(&path)).unwrap();
        let mut records = 0usize;
        loop {
            let mut header = [0u8; size_of::<u32>()];
            match frames.read_exact(&mut header) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(error) => panic!("read chunk header: {error}"),
            }
            let encoded_len = u32::from_le_bytes(header) as usize;
            assert!(encoded_len <= MAX_ENCODED_CHUNK_BYTES);
            frames.seek(SeekFrom::Current(encoded_len as i64)).unwrap();
            records += 1;
        }
        assert_eq!(records, 3);
        assert_eq!(attribution.raw_bytes, large.output_json.len());
        let (journal, status) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        assert_eq!(status, OpenStatus::Resumed);
        assert_eq!(journal.completed_targets().unwrap(), vec![large]);
        remove(&path).unwrap();
    }

    #[test]
    fn uncommitted_frame_tail_is_ignored() {
        let path = temp_path("build-progress-frame-tail");
        let (mut journal, _) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        journal.checkpoint_target(&completed()).unwrap();
        drop(journal);
        OpenOptions::new()
            .append(true)
            .open(frame_path(&path))
            .unwrap()
            .write_all(b"uncommitted-tail")
            .unwrap();

        let (journal, status) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        assert_eq!(status, OpenStatus::Resumed);
        assert_eq!(journal.completed_targets().unwrap(), vec![completed()]);
        remove(&path).unwrap();
    }

    #[test]
    fn truncated_committed_frame_recreates_disposable_bundle() {
        let path = temp_path("build-progress-frame-truncated");
        let (mut journal, _) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        journal.checkpoint_target(&completed()).unwrap();
        drop(journal);
        OpenOptions::new()
            .write(true)
            .open(frame_path(&path))
            .unwrap()
            .set_len(1)
            .unwrap();

        let (journal, status) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        assert!(matches!(status, OpenStatus::Recreated { .. }));
        assert!(journal.completed_targets().unwrap().is_empty());
        remove(&path).unwrap();
    }

    #[test]
    fn read_only_summary_reports_durable_counters_and_last_target() {
        let path = temp_path("build-progress-summary");
        let (mut journal, _) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        let build_id = journal.build_id().to_string();
        journal.checkpoint_target(&completed()).unwrap();
        drop(journal);

        let summary = read_summary(&path).unwrap().unwrap();
        assert_eq!(summary.build_id, build_id);
        assert_eq!(summary.total_targets, 1);
        assert_eq!(summary.completed_targets, 1);
        assert_eq!(summary.completed_shards, 0);
        assert_eq!(summary.last_completed_ordinal, Some(0));
        assert_eq!(
            summary.last_completed_path.as_deref(),
            Some("/games/arcade")
        );
        remove(&path).unwrap();
    }

    #[test]
    fn failed_target_checkpoint_is_atomic() {
        let path = temp_path("build-progress-atomic");
        let (mut journal, _) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        let mut invalid = completed();
        invalid.target.key = "wrong".into();
        assert!(journal.checkpoint_target(&invalid).is_err());
        assert!(journal.completed_targets().unwrap().is_empty());
        remove(&path).unwrap();
    }

    #[test]
    fn failed_target_batch_rolls_back_every_target() {
        let path = temp_path("build-progress-batch-atomic");
        let targets = vec![
            ScanTarget {
                ordinal: 0,
                key: "first".into(),
                path: "/games/first".into(),
            },
            ScanTarget {
                ordinal: 1,
                key: "second".into(),
                path: "/games/second".into(),
            },
        ];
        let (mut journal, _) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets).unwrap();
        let batch = vec![
            CompletedTarget {
                target: targets[0].clone(),
                input_fingerprint: "first-fingerprint".into(),
                output_json: "{}".into(),
                accumulated_stats: BuildStats::default(),
            },
            CompletedTarget {
                target: ScanTarget {
                    key: "wrong".into(),
                    ..targets[1].clone()
                },
                input_fingerprint: "second-fingerprint".into(),
                output_json: "{}".into(),
                accumulated_stats: BuildStats::default(),
            },
        ];

        assert!(journal.checkpoint_targets(&batch).is_err());
        assert!(journal.completed_targets().unwrap().is_empty());
        remove(&path).unwrap();
    }

    #[test]
    fn corrupt_journal_is_discarded() {
        let path = temp_path("build-progress-corrupt");
        fs::create_dir_all(&path).unwrap();
        fs::write(metadata_path(&path), b"not sqlite").unwrap();
        let (journal, status) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        assert!(matches!(status, OpenStatus::Recreated { .. }));
        assert!(journal.completed_targets().unwrap().is_empty());
        drop(journal);
        remove(&path).unwrap();
    }

    #[test]
    fn schema_mismatch_is_discarded() {
        let path = temp_path("build-progress-schema");
        let (journal, _) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        drop(journal);
        let conn = Connection::open(metadata_path(&path)).unwrap();
        conn.execute(
            "UPDATE progress_meta SET value='999' WHERE key='schema_version'",
            [],
        )
        .unwrap();
        drop(conn);
        let (_, status) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        assert!(
            matches!(status, OpenStatus::Recreated { reason } if reason.contains("unsupported"))
        );
        remove(&path).unwrap();
    }

    #[test]
    fn changed_contract_starts_a_new_build() {
        let path = temp_path("build-progress-stale");
        let (journal, _) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        let old_id = journal.build_id().to_string();
        drop(journal);
        let mut changed = contract();
        changed.scanner_version += 1;
        let (journal, status) =
            BuildProgressJournal::open_or_create(&path, &changed, &targets()).unwrap();
        assert!(matches!(status, OpenStatus::Recreated { reason } if reason.contains("contract")));
        assert_ne!(journal.build_id(), old_id);
        drop(journal);
        remove(&path).unwrap();
    }

    #[test]
    fn target_addition_preserves_matching_completed_work_at_its_new_ordinal() {
        let path = temp_path("build-progress-target-reconcile");
        let (mut journal, _) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets()).unwrap();
        journal.checkpoint_target(&completed()).unwrap();
        let build_id = journal.build_id().to_string();
        drop(journal);
        let changed_targets = vec![
            ScanTarget {
                ordinal: 0,
                key: "new".into(),
                path: "/games/new".into(),
            },
            ScanTarget {
                ordinal: 1,
                key: "arcade".into(),
                path: "/games/arcade".into(),
            },
        ];
        let (journal, status) =
            BuildProgressJournal::open_or_create(&path, &contract(), &changed_targets).unwrap();
        assert_eq!(status, OpenStatus::Resumed);
        assert_eq!(journal.build_id(), build_id);
        let completed = journal.completed_targets().unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].target.ordinal, 1);
        assert_eq!(completed[0].target.key, "arcade");
        remove(&path).unwrap();
    }

    #[test]
    fn three_process_equivalent_interruptions_keep_one_build_and_accumulate_work() {
        let path = temp_path("build-progress-three-interruptions");
        let targets = (0..4)
            .map(|ordinal| ScanTarget {
                ordinal,
                key: format!("target-{ordinal}"),
                path: format!("/games/{ordinal}"),
            })
            .collect::<Vec<_>>();
        let mut build_id = String::new();
        for ordinal in 0..3 {
            let (mut journal, status) =
                BuildProgressJournal::open_or_create(&path, &contract(), &targets).unwrap();
            if ordinal == 0 {
                assert_eq!(status, OpenStatus::Created);
                build_id = journal.build_id().to_string();
            } else {
                assert_eq!(status, OpenStatus::Resumed);
                assert_eq!(journal.build_id(), build_id);
            }
            assert_eq!(journal.completed_targets().unwrap().len(), ordinal as usize);
            journal
                .checkpoint_target(&CompletedTarget {
                    target: targets[ordinal as usize].clone(),
                    input_fingerprint: format!("fingerprint-{ordinal}"),
                    output_json: format!(r#"{{"target":{ordinal}}}"#),
                    accumulated_stats: BuildStats {
                        discoveries: (ordinal + 1) as u64,
                        ..BuildStats::default()
                    },
                })
                .unwrap();
        }
        let (journal, status) =
            BuildProgressJournal::open_or_create(&path, &contract(), &targets).unwrap();
        assert_eq!(status, OpenStatus::Resumed);
        assert_eq!(journal.build_id(), build_id);
        assert_eq!(journal.completed_targets().unwrap().len(), 3);
        remove(&path).unwrap();
    }

    #[test]
    fn successful_state_seeds_targets_without_generation_specific_shards() {
        let progress = temp_path("build-progress-success");
        let committed = temp_path("builder-state-success");
        let resumed = temp_path("build-progress-seeded");
        let (mut journal, _) =
            BuildProgressJournal::open_or_create(&progress, &contract(), &targets()).unwrap();
        journal.checkpoint_target(&completed()).unwrap();
        journal
            .record_shard(&CompletedShard {
                system_id: "arcade".into(),
                generation: 3,
                sqlite_path: "systems/arcade/3.sqlite3".into(),
                navigation_path: "systems/arcade/3.nav.lz4b".into(),
                content_hash: "abc".into(),
                manifest_system_json: "{}".into(),
            })
            .unwrap();
        drop(journal);

        assert!(commit_successful_state(&progress, &committed, 3).unwrap());
        assert!(!progress.exists());
        assert!(committed.exists());
        assert!(seed_from_committed(&committed, &resumed).unwrap());
        assert!(!committed.exists());
        assert!(resumed.exists());
        let mut bound_contract = contract();
        bound_contract.active_manifest_generation = Some(3);
        let journal =
            BuildProgressJournal::open_existing(&resumed, &bound_contract, &targets()).unwrap();
        assert_eq!(journal.completed_targets(), Ok(vec![completed()]));
        assert!(journal.completed_shards().unwrap().is_empty());

        drop(journal);
        for path in [&progress, &committed, &resumed] {
            remove(path).unwrap();
        }
    }
}
