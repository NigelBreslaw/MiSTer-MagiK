// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! SQLite catalog import, publish, and loading.

use crate::arcade_catalog::{
    self, ArcadeCatalog, ArcadeGameEntry, ArcadeGameMetadataKey, PlatformKind,
    StructuredLaunchPlan, SystemProjectionStats,
};
use crate::catalog_checkpoint::{self, CatalogDriftSummary};
use crate::catalog_config::{
    DEFAULT_SQLITE_BUILD_DIR, SCHEMA_VERSION, default_hbmame_sqlite_path, default_mame_sqlite_path,
    default_sqlite_path,
};
use crate::catalog_discovery;
use crate::catalog_load_metrics;
use crate::catalog_navigation;
use crate::catalog_progress::{CatalogProgress, report_catalog_progress};
use crate::catalog_projection::{
    self, ArcadeCompatibilityRow, ArcadePreviewProjection, CanonicalLaunchIdIndex,
    CatalogProjectionRow, CatalogProjectionSource, LauncherPreviewAsset,
};
use crate::catalog_stamp;
use crate::catalog_store;
use crate::catalog_summary;
use crate::core_audit;
#[cfg(test)]
use crate::game_discovery::unique_discovery_count;
use crate::game_discovery::{
    DiscoverySourceKind, GameDiscovery, catalog_system_id_for_discovery, confidence_str,
    covered_payload_paths, is_launcher_launch_ref, is_raw_arcade_zip_set_discovery,
    launch_kind_for_discovery, launch_ref_for_discovery, preferred_playable_discoveries_by_key,
    profile_id_for_discovery,
};
use crate::launch_profiles::{self, LaunchProfile, MountKind, MountSpec, RuleSourceKind};
use crate::library_db::{
    self, BenchConfig, CatalogStampCheckSummary, FileSignature, LibraryCatalogLoad,
    LibraryRefreshSummary, LibraryScan, ProgressCallback, VirtualLaunchPlan,
};
use crate::media_identity;
use crate::media_metadata;
use crate::preview_worker;
use crate::software_identity::{
    PreviewArchivePaths, SoftwareHashCache, console_preview_asset,
    load_arcade_machine_metadata_for_setnames, load_mame_software_metadata,
    mame_identity_for_discovery, mame_identity_projection, mame_software_identity_for_discovery,
    mister_arcade_metadata_for_discovery,
};
use rusqlite::functions::FunctionFlags;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const NEW_GAME_BADGE_SECS: i64 = 14 * 24 * 60 * 60;
const SQLITE_PUBLISH_COPY_CHUNK_BYTES: usize = 256 * 1024;
const SQLITE_INNER_PROGRESS_BYTES: u64 = 8 * 1024 * 1024;
const SQLITE_PATH_CHUNK_BYTES: usize = 256 * 1024;
const MAX_SQLITE_PATH_CHUNK_BYTES: usize = SQLITE_PATH_CHUNK_BYTES + 64 * 1024;

#[derive(Default)]
struct SqlitePathInterner {
    values: HashMap<String, i64>,
    rows: Vec<(i64, String)>,
    next_path_id: i64,
}

#[derive(Default)]
struct SqliteStringInterner {
    values: HashMap<String, i64>,
    rows: Vec<(i64, String)>,
    next_id: i64,
}

impl SqliteStringInterner {
    fn intern(&mut self, value: &str) -> i64 {
        if let Some(id) = self.values.get(value).copied() {
            return id;
        }
        self.next_id += 1;
        let value_id = self.next_id;
        self.values.insert(value.to_string(), value_id);
        self.rows.push((value_id, value.to_string()));
        value_id
    }

    fn intern_optional(&mut self, value: Option<&str>) -> Option<i64> {
        value.map(|value| self.intern(value))
    }

    fn flush(&self, tx: &rusqlite::Transaction<'_>) -> Result<(), String> {
        let mut stmt = tx
            .prepare("INSERT INTO string_values(string_id,value) VALUES (?1,?2)")
            .map_err(|e| format!("prepare string value insert: {e}"))?;
        for (string_id, value) in &self.rows {
            stmt.execute(params![string_id, value])
                .map_err(|e| format!("insert string value: {e}"))?;
        }
        Ok(())
    }
}

impl SqlitePathInterner {
    fn intern_optional(&mut self, value: Option<&str>) -> Option<i64> {
        match value {
            Some(value) if !value.is_empty() => Some(self.intern(value)),
            _ => None,
        }
    }

    fn intern(&mut self, value: &str) -> i64 {
        if let Some(id) = self.values.get(value).copied() {
            return id;
        }
        self.next_path_id += 1;
        let path_id = self.next_path_id;
        self.values.insert(value.to_string(), path_id);
        self.rows.push((path_id, value.to_string()));
        path_id
    }

    fn flush(&self, tx: &rusqlite::Transaction<'_>) -> Result<(), String> {
        let mut value_stmt = tx
            .prepare("INSERT INTO path_values(path_id,chunk_id,offset,len) VALUES (?1,?2,?3,?4)")
            .map_err(|e| format!("prepare path value insert: {e}"))?;
        let mut chunk_stmt = tx
            .prepare("INSERT INTO path_chunks(chunk_id,uncompressed_len,bytes) VALUES (?1,?2,?3)")
            .map_err(|e| format!("prepare path chunk insert: {e}"))?;
        let mut chunk_id = 0_i64;
        let mut chunk = Vec::with_capacity(SQLITE_PATH_CHUNK_BYTES);
        let mut pending_values = Vec::new();
        for (path_id, path) in &self.rows {
            let bytes = path.as_bytes();
            if !chunk.is_empty() && chunk.len() + bytes.len() > SQLITE_PATH_CHUNK_BYTES {
                flush_path_chunk(
                    &mut chunk_stmt,
                    &mut value_stmt,
                    chunk_id,
                    &chunk,
                    &pending_values,
                )?;
                chunk.clear();
                pending_values.clear();
            }
            if chunk.is_empty() {
                chunk_id += 1;
            }
            let offset =
                i64::try_from(chunk.len()).map_err(|_| "path chunk offset overflow".to_string())?;
            let len = i64::try_from(bytes.len()).map_err(|_| "path length overflow".to_string())?;
            chunk.extend_from_slice(bytes);
            pending_values.push((*path_id, chunk_id, offset, len));
        }
        if !chunk.is_empty() {
            flush_path_chunk(
                &mut chunk_stmt,
                &mut value_stmt,
                chunk_id,
                &chunk,
                &pending_values,
            )?;
        }
        Ok(())
    }
}

fn flush_path_chunk(
    chunk_stmt: &mut rusqlite::Statement<'_>,
    value_stmt: &mut rusqlite::Statement<'_>,
    chunk_id: i64,
    chunk: &[u8],
    values: &[(i64, i64, i64, i64)],
) -> Result<(), String> {
    let compressed = lz4_flex::compress_prepend_size(chunk);
    let uncompressed_len =
        i64::try_from(chunk.len()).map_err(|_| "path chunk length overflow".to_string())?;
    chunk_stmt
        .execute(params![chunk_id, uncompressed_len, compressed])
        .map_err(|e| format!("insert path chunk: {e}"))?;
    for (path_id, chunk_id, offset, len) in values {
        value_stmt
            .execute(params![path_id, chunk_id, offset, len])
            .map_err(|e| format!("insert path value: {e}"))?;
    }
    Ok(())
}

fn register_sqlite_catalog_functions(conn: &Connection) -> rusqlite::Result<()> {
    let cache: Mutex<HashMap<i64, Vec<u8>>> = Mutex::new(HashMap::new());
    conn.create_scalar_function(
        "magik_path",
        5,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            let chunk_id: i64 = ctx.get(0)?;
            let offset: i64 = ctx.get(1)?;
            let len: i64 = ctx.get(2)?;
            let expected_len: i64 = ctx.get(3)?;
            let compressed = ctx.get_raw(4).as_blob()?;
            let offset = usize::try_from(offset).map_err(sqlite_function_error)?;
            let len = usize::try_from(len).map_err(sqlite_function_error)?;
            let expected_len = usize::try_from(expected_len).map_err(sqlite_function_error)?;
            if expected_len > MAX_SQLITE_PATH_CHUNK_BYTES {
                return Err(sqlite_function_error(format!(
                    "path chunk {chunk_id} declared size {expected_len} exceeds max {MAX_SQLITE_PATH_CHUNK_BYTES}"
                )));
            }
            let mut cache = cache
                .lock()
                .map_err(|_| sqlite_function_error("path chunk cache lock poisoned"))?;
            let chunk = match cache.get(&chunk_id) {
                Some(chunk) => chunk,
                None => {
                    let decoded = crate::bounded_lz4::decompress_size_prepended(
                        compressed,
                        MAX_SQLITE_PATH_CHUNK_BYTES,
                        "SQLite path chunk",
                    )
                    .map_err(sqlite_function_error)?;
                    if decoded.len() != expected_len {
                        return Err(sqlite_function_error(format!(
                            "path chunk {chunk_id} decoded to {} bytes, expected {expected_len}",
                            decoded.len()
                        )));
                    }
                    cache.insert(chunk_id, decoded);
                    cache.get(&chunk_id).expect("inserted path chunk")
                }
            };
            let end = offset
                .checked_add(len)
                .ok_or_else(|| sqlite_function_error("path slice overflow"))?;
            let slice = chunk
                .get(offset..end)
                .ok_or_else(|| sqlite_function_error("path slice out of range"))?;
            String::from_utf8(slice.to_vec()).map_err(sqlite_function_error)
        },
    )
}

fn sqlite_function_error(
    error: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
) -> rusqlite::Error {
    rusqlite::Error::UserFunctionError(error.into())
}

#[cfg(test)]
fn split_path_for_storage(value: &str) -> (&str, &str) {
    match value.rfind('/') {
        Some(idx) => value.split_at(idx + 1),
        None => ("", value),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SqliteBuildTempSource {
    EnvOverride,
    DefaultTmpfs,
    BesideFinal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SqliteBuildTempPlan {
    pub(crate) build_tmp_path: PathBuf,
    pub(crate) final_tmp_path: PathBuf,
    pub(crate) source: SqliteBuildTempSource,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SqlitePublishMetrics {
    pub(crate) bytes: u64,
    pub(crate) build_sync_ms: u64,
    pub(crate) copy_ms: u64,
    pub(crate) final_sync_ms: u64,
    pub(crate) rename_ms: u64,
    pub(crate) parent_sync_ms: u64,
    pub(crate) total_ms: u64,
    pub(crate) progress_events: u64,
}

#[cfg(test)]
pub(crate) struct SqliteSavedCatalog {
    pub(crate) bytes: u64,
    pub(crate) catalog: LibraryCatalogLoad,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewIndexRefreshRow {
    pub label: String,
    pub system_id: String,
    pub pack_path: String,
    pub index_path: String,
    pub index_entries: usize,
    pub candidate_rows: usize,
    pub updated_rows: usize,
    pub index_read_us: u64,
    pub sql_update_us: u64,
    pub total_us: u64,
    pub result: String,
    pub error: String,
}

impl PreviewIndexRefreshRow {
    pub fn to_tsv(&self) -> String {
        format!(
            "preview_index_refresh_tsv\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            tsv_field(&self.label),
            tsv_field(&self.system_id),
            tsv_field(&self.pack_path),
            tsv_field(&self.index_path),
            self.index_entries,
            self.candidate_rows,
            self.updated_rows,
            self.index_read_us,
            self.sql_update_us,
            self.total_us,
            tsv_field(&self.result),
            tsv_field(&self.error)
        )
    }
}

pub const PREVIEW_INDEX_REFRESH_TSV_HEADER: &str = "preview_index_refresh_tsv\tlabel\tsystem_id\tpack_path\tindex_path\tindex_entries\tcandidate_rows\tupdated_rows\tindex_read_us\tsql_update_us\ttotal_us\tresult\terror";

pub(crate) use crate::scanner_cache::DiscoveryHistory;

impl DiscoveryHistory {
    pub(crate) fn load(path: &Path) -> Option<Self> {
        let conn = open_sqlite_read_only(path).ok()?;
        if !sqlite_table_exists(&conn, "games").ok()? {
            return None;
        }
        let has_discovered_at = sqlite_column_exists(&conn, "games", "discovered_at_unix").ok()?;
        let mut by_game_id = HashMap::new();
        if has_discovered_at {
            let mut stmt = conn
                .prepare("SELECT game_id, discovered_at_unix FROM games")
                .ok()?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
                })
                .ok()?;
            for row in rows {
                let (game_id, discovered_at_unix) = row.ok()?;
                by_game_id.insert(game_id, discovered_at_unix);
            }
        } else {
            let mut stmt = conn.prepare("SELECT game_id FROM games").ok()?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0)).ok()?;
            for row in rows {
                by_game_id.insert(row.ok()?, None);
            }
        }
        Some(Self { by_game_id })
    }
}

pub(crate) fn remove_default_sqlite_database() -> Result<(), String> {
    let path = default_sqlite_path();
    remove_sqlite_database_at(&path)
}

pub(crate) fn remove_default_catalog_artifacts() -> Result<usize, String> {
    let sqlite_path = default_sqlite_path();
    remove_catalog_artifacts_with_cache_paths(&sqlite_path, Path::new(DEFAULT_SQLITE_BUILD_DIR))
}

pub(crate) fn remove_catalog_artifacts_with_cache_paths(
    sqlite_path: &Path,
    build_dir: &Path,
) -> Result<usize, String> {
    let snapshot_path = std::env::var_os("MISTER_CATALOG_READY_SNAPSHOT").map(PathBuf::from);
    remove_catalog_artifacts_at(
        sqlite_path,
        build_dir,
        snapshot_path.as_deref(),
        Path::new("/tmp/mister-magik"),
        &crate::device_layout::current_app_path("rebuild-on-next-boot"),
    )
}

pub(crate) fn remove_catalog_artifacts_at(
    sqlite_path: &Path,
    build_dir: &Path,
    configured_snapshot: Option<&Path>,
    default_snapshot_dir: &Path,
    rebuild_marker: &Path,
) -> Result<usize, String> {
    let mut removed = 0usize;
    for (path, label) in [
        (sqlite_path.to_path_buf(), "database"),
        (
            catalog_summary::summary_path_for_sqlite(sqlite_path),
            "catalog summary",
        ),
        (
            catalog_navigation::navigation_path_for_sqlite(sqlite_path),
            "catalog navigation",
        ),
        (
            crate::catalog_build_record::duration_path_for_sqlite(sqlite_path),
            "catalog build duration",
        ),
        (rebuild_marker.to_path_buf(), "catalog rebuild marker"),
    ] {
        removed += usize::from(remove_file_if_exists_counted(&path, label)?);
    }

    let sqlite_name = sqlite_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library.sqlite3");
    if let Some(parent) = sqlite_path.parent() {
        let sqlite_tmp_prefix = format!(".{sqlite_name}.tmp.");
        let sqlite_journal = format!("{sqlite_name}-journal");
        let sqlite_wal = format!("{sqlite_name}-wal");
        let sqlite_shm = format!("{sqlite_name}-shm");
        removed += remove_matching_files(parent, "catalog adjacent temp", |name| {
            name.starts_with(&sqlite_tmp_prefix)
                || name == sqlite_journal
                || name == sqlite_wal
                || name == sqlite_shm
                || name == ".library.summary.json.tmp"
                || name == ".library.nav.lz4b.tmp"
                || name == ".library-build-seconds.tmp"
        })?;
    }
    let build_prefix = format!(".{sqlite_name}.build.");
    removed += remove_matching_files(build_dir, "catalog build temp", |name| {
        name.starts_with(&build_prefix)
    })?;
    if let Some(snapshot) = configured_snapshot {
        removed += usize::from(remove_file_if_exists_counted(
            snapshot,
            "configured catalog ready snapshot",
        )?);
    }
    removed += remove_matching_files(default_snapshot_dir, "catalog ready snapshot", |name| {
        name.starts_with("catalog-ready-") && name.ends_with(".nav.lz4b")
    })?;
    Ok(removed)
}

fn remove_matching_files(
    dir: &Path,
    label: &str,
    mut matches: impl FnMut(&str) -> bool,
) -> Result<usize, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("read {label} directory {}: {error}", dir.display())),
    };
    let mut removed = 0usize;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {label} entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("stat {label} {}: {error}", entry.path().display()))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if (file_type.is_file() || file_type.is_symlink()) && matches(&name) {
            removed += usize::from(remove_file_if_exists_counted(&entry.path(), label)?);
        }
    }
    Ok(removed)
}

fn remove_file_if_exists_counted(path: &Path, label: &str) -> Result<bool, String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("remove {label} {}: {error}", path.display())),
    }
}

fn remove_sqlite_database_at(path: &Path) -> Result<(), String> {
    let mut fault_control = crate::fs_fault::NoopDirectResetFaultControl;
    remove_sqlite_database_at_with_fault_control(path, &mut fault_control)
}

fn remove_sqlite_database_at_with_fault_control(
    path: &Path,
    fault_control: &mut dyn crate::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
    remove_file_if_exists(path, "database", fault_control)?;
    let summary_path = catalog_summary::summary_path_for_sqlite(path);
    remove_file_if_exists(&summary_path, "catalog summary", fault_control)?;
    let navigation_path = catalog_navigation::navigation_path_for_sqlite(path);
    remove_file_if_exists(&navigation_path, "catalog navigation", fault_control)?;
    let duration_path = crate::catalog_build_record::duration_path_for_sqlite(path);
    remove_file_if_exists(&duration_path, "catalog build duration", fault_control)
}

fn remove_file_if_exists(
    path: &Path,
    label: &str,
    fault_control: &mut dyn crate::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            match label {
                "database" => crate::fs_fault::maybe_fault_with_control(
                    "reset_delete.database.after_remove",
                    path,
                    fault_control,
                ),
                "catalog summary" => crate::fs_fault::maybe_fault_with_control(
                    "reset_delete.summary.after_remove",
                    path,
                    fault_control,
                ),
                "catalog navigation" => crate::fs_fault::maybe_fault_with_control(
                    "reset_delete.navigation.after_remove",
                    path,
                    fault_control,
                ),
                _ => crate::fs_fault::DirectResetFaultOutcome::Noop,
            };
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("failed to delete {label} {}: {e}", path.display())),
    }
    Ok(())
}

pub(crate) fn load_virtual_launch_plans_for_system(
    system_id: &str,
    limit: usize,
) -> Result<Vec<VirtualLaunchPlan>, String> {
    let path = default_sqlite_path();
    let conn = open_sqlite_read_only(&path).map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    ensure_sqlite_schema_current(&conn)?;
    load_virtual_launch_plans_for_system_from_conn(&conn, system_id, limit)
}

pub(crate) fn load_virtual_launch_plans_for_system_from_conn(
    conn: &Connection,
    system_id: &str,
    limit: usize,
) -> Result<Vec<VirtualLaunchPlan>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT CASE launch_targets.launch_ref_kind
                        WHEN 'payload' THEN 'magik-plan:payload:' || payload_paths.path
                        WHEN 'archive' THEN 'magik-plan:archive:' || payload_paths.path
                        WHEN 'same-payload' THEN payload_paths.path
                        ELSE launch_paths.path
                    END AS launch_ref,
                    games.title,
                    games.system_id,
                    COALESCE(profiles.core_path, launch_targets.core_id),
                    COALESCE(payload_paths.path, ''),
                    COALESCE(launch_targets.mount_kind, 'mount-image'),
                    COALESCE(launch_targets.mount_index, 0),
                    COALESCE(launch_targets.delay_secs, 1)
             FROM launch_targets
             JOIN games ON games.game_key_id = launch_targets.game_key_id
             LEFT JOIN path_values_text launch_paths
                    ON launch_paths.path_id = launch_targets.launch_path_id
             LEFT JOIN path_values_text payload_paths
                    ON payload_paths.path_id = launch_targets.payload_path_id
             LEFT JOIN profiles ON profiles.profile_id = launch_targets.profile_id
             WHERE launch_targets.launch_kind = 'virtual-mgl'
               AND games.system_id = ?1
             ORDER BY games.sort_title, launch_ref
             LIMIT ?2",
        )
        .map_err(|e| format!("prepare virtual launch list query: {e}"))?;
    let rows = stmt
        .query_map(
            params![system_id, limit as i64],
            virtual_launch_plan_from_row,
        )
        .map_err(|e| format!("query virtual launch list: {e}"))?;
    rows.map(|row| row.map_err(|e| format!("read virtual launch row: {e}")))
        .collect()
}

pub(crate) fn load_amigavision_launch_refs(limit: usize) -> Result<Vec<String>, String> {
    let path = default_sqlite_path();
    let conn = open_sqlite_read_only(&path).map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    ensure_sqlite_schema_current(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT launch_ref
             FROM launchables
             WHERE launch_ref LIKE 'magik-amigavision:%'
             ORDER BY title, launch_ref
             LIMIT ?1",
        )
        .map_err(|e| format!("prepare AmigaVision launch query: {e}"))?;
    let rows = stmt
        .query_map([limit as i64], |row| row.get::<_, String>(0))
        .map_err(|e| format!("query AmigaVision launches: {e}"))?;
    rows.map(|row| row.map_err(|e| format!("read AmigaVision launch_ref: {e}")))
        .collect()
}

pub(crate) fn virtual_launch_plan_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<VirtualLaunchPlan> {
    Ok(VirtualLaunchPlan {
        launch_ref: row.get(0)?,
        title: row.get(1)?,
        system_id: row.get(2)?,
        core_path: row.get(3)?,
        payload_path: row.get(4)?,
        mount_kind: row.get(5)?,
        mount_index: row.get::<_, i64>(6)?.clamp(0, u8::MAX as i64) as u8,
        mount_delay_secs: row.get::<_, i64>(7)?.clamp(0, u8::MAX as i64) as u8,
    })
}

pub(crate) fn load_arcade_catalog_from_sqlite(
    root: impl AsRef<Path>,
) -> Result<LibraryCatalogLoad, String> {
    let path = default_sqlite_path();
    load_arcade_catalog_from_sqlite_at(root, &path)
}

pub(crate) fn load_arcade_catalog_from_materialized_sqlite(
    root: impl AsRef<Path>,
) -> Result<LibraryCatalogLoad, String> {
    let path = default_sqlite_path();
    load_arcade_catalog_from_materialized_sqlite_at(root, &path)
}

pub(crate) fn load_arcade_catalog_from_sqlite_at(
    root: impl AsRef<Path>,
    path: &Path,
) -> Result<LibraryCatalogLoad, String> {
    load_arcade_catalog_from_sqlite_at_with_projection(root, path, true)
}

pub(crate) fn load_arcade_catalog_from_materialized_sqlite_at(
    root: impl AsRef<Path>,
    path: &Path,
) -> Result<LibraryCatalogLoad, String> {
    load_arcade_catalog_from_sqlite_at_with_projection(root, path, false)
}

fn load_arcade_catalog_from_sqlite_at_with_projection(
    root: impl AsRef<Path>,
    path: &Path,
    allow_embedded_navigation: bool,
) -> Result<LibraryCatalogLoad, String> {
    let t = Instant::now();
    let open_t = Instant::now();
    catalog_load_metrics::record_sqlite_open();
    let conn = open_sqlite_read_only(path).map_err(|e| format!("open library db: {e}"))?;
    let open_us = open_t.elapsed().as_micros() as u64;
    let schema_t = Instant::now();
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    ensure_sqlite_schema_current(&conn)?;
    let schema_check_us = schema_t.elapsed().as_micros() as u64;
    let stamp = catalog_store::read_catalog_stamp(&conn)?;
    load_arcade_catalog_from_connection(
        root,
        &conn,
        t,
        open_us,
        schema_check_us,
        stamp,
        allow_embedded_navigation,
    )
}

fn load_arcade_catalog_from_connection(
    root: impl AsRef<Path>,
    conn: &Connection,
    started: Instant,
    open_us: u64,
    schema_check_us: u64,
    stamp: Option<catalog_stamp::CatalogStamp>,
    allow_embedded_navigation: bool,
) -> Result<LibraryCatalogLoad, String> {
    let root = root.as_ref().to_path_buf();
    let query_t = Instant::now();
    if allow_embedded_navigation
        && let Some(stamp) = stamp.as_ref()
        && let Some(projection) = load_embedded_catalog_navigation(conn, stamp)?
    {
        let query_us = query_t.elapsed().as_micros() as u64;
        let rows = projection.games.len();
        let catalog_t = Instant::now();
        let catalog = ArcadeCatalog::from_navigation_projection(root, projection);
        let catalog_us = catalog_t.elapsed().as_micros() as u64;
        return Ok(LibraryCatalogLoad {
            catalog,
            stamp: Some(stamp.clone()),
            projection_repair_safe: true,
            us: started.elapsed().as_micros() as u64,
            open_us,
            schema_check_us,
            query_us,
            query_prepare_us: 0,
            query_first_row_us: 0,
            query_row_read_us: 0,
            query_row_hydrate_us: 0,
            launch_plans_us: 0,
            systems_us: 0,
            catalog_us,
            navigation_file_read_us: 0,
            navigation_decompress_us: 0,
            navigation_decode_us: 0,
            rows,
        });
    }
    let mut query_timing = CatalogSqlQueryTiming::default();
    let (games, projection_repair_safe) = match load_materialized_launcher_catalog(conn) {
        Ok(Some(result)) => {
            query_timing = result.timing;
            (result.games, true)
        }
        Ok(None) => match load_materialized_ui_catalog(conn) {
            Ok(Some(games)) => (games, true),
            Ok(None) => (load_joined_launcher_catalog(conn)?, false),
            Err(e) => return Err(e),
        },
        Err(e) => return Err(e),
    };
    let query_us = query_t.elapsed().as_micros() as u64;
    let rows = games.len();
    let launch_plans_t = Instant::now();
    let launch_plans = load_launcher_launch_plans(conn)?;
    let launch_plans_us = launch_plans_t.elapsed().as_micros() as u64;
    let systems_t = Instant::now();
    let systems = arcade_catalog::systems_from_games(&games);
    let platform_kinds = load_system_platform_kinds(conn)?;
    let systems_us = systems_t.elapsed().as_micros() as u64;
    let catalog_t = Instant::now();
    let mut catalog = ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
        root,
        games,
        systems,
        launch_plans,
        platform_kinds,
    );
    let lynx_source_games = conn
        .query_row(
            "SELECT count(*)
             FROM game_rows
             JOIN string_values systems ON systems.string_id=game_rows.system_string_id
             JOIN launch_target_rows ON launch_target_rows.launch_id=game_rows.game_key_id
             WHERE systems.value='atarilynx'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .ok()
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(0);
    if lynx_source_games > 0 {
        let visible_families = catalog.system_game_count("atarilynx");
        catalog = catalog.with_projection_stats(HashMap::from([(
            "atarilynx".to_string(),
            SystemProjectionStats {
                source_games: lynx_source_games,
                visible_families,
                collapsed_variants: lynx_source_games.saturating_sub(visible_families),
            },
        )]));
    }
    let catalog_us = catalog_t.elapsed().as_micros() as u64;
    Ok(LibraryCatalogLoad {
        catalog,
        stamp,
        projection_repair_safe,
        us: started.elapsed().as_micros() as u64,
        open_us,
        schema_check_us,
        query_us,
        query_prepare_us: query_timing.prepare_us,
        query_first_row_us: query_timing.first_row_us,
        query_row_read_us: query_timing.row_read_us,
        query_row_hydrate_us: query_timing.row_hydrate_us,
        launch_plans_us,
        systems_us,
        catalog_us,
        navigation_file_read_us: 0,
        navigation_decompress_us: 0,
        navigation_decode_us: 0,
        rows,
    })
}

fn load_embedded_catalog_navigation(
    conn: &Connection,
    expected_stamp: &catalog_stamp::CatalogStamp,
) -> Result<Option<catalog_navigation::CatalogNavigationProjection>, String> {
    load_embedded_catalog_navigation_with_limit(
        conn,
        expected_stamp,
        catalog_navigation::MAX_CATALOG_NAVIGATION_COMPRESSED_BYTES,
    )
}

fn load_embedded_catalog_navigation_with_limit(
    conn: &Connection,
    expected_stamp: &catalog_stamp::CatalogStamp,
    max_compressed_bytes: u64,
) -> Result<Option<catalog_navigation::CatalogNavigationProjection>, String> {
    if !sqlite_physical_table_exists(conn, "catalog_navigation_projection")? {
        return Ok(None);
    }
    // SQLite can answer length(blob) without materializing the BLOB. Check it
    // before row.get::<Vec<u8>>() so corrupt databases cannot force an
    // unbounded allocation before the navigation decoder's own limits run.
    let compressed_len = conn
        .query_row(
            "SELECT length(bytes) FROM catalog_navigation_projection WHERE id=0",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| format!("inspect embedded catalog navigation: {e}"))?;
    let Some(compressed_len) = compressed_len else {
        return Ok(None);
    };
    if compressed_len < 0 || compressed_len as u64 > max_compressed_bytes {
        return Ok(None);
    }
    let bytes = conn
        .query_row(
            "SELECT bytes FROM catalog_navigation_projection WHERE id=0",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(|e| format!("read embedded catalog navigation: {e}"))?;
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    // The embedded payload is a recovery cache. Corruption or a stale payload
    // must fall through to retained materialized tables (or, for legacy
    // databases, the explicitly unsafe joined fallback) instead of making the
    // otherwise readable source-fact database unusable.
    match catalog_navigation::decode_catalog_navigation_from_storage(&bytes, expected_stamp) {
        Ok(projection) => Ok(projection),
        Err(_) => Ok(None),
    }
}

fn load_system_platform_kinds(conn: &Connection) -> Result<HashMap<String, PlatformKind>, String> {
    let mut stmt = conn
        .prepare("SELECT system_id, platform_kind FROM systems")
        .map_err(|e| format!("prepare system platform kind query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let system_id = row.get::<_, String>(0)?;
            let platform_kind = row.get::<_, String>(1)?;
            Ok((system_id, platform_kind))
        })
        .map_err(|e| format!("query system platform kinds: {e}"))?;
    let mut kinds = HashMap::new();
    for row in rows {
        let (system_id, stored_kind) =
            row.map_err(|e| format!("read system platform kind row: {e}"))?;
        let kind = PlatformKind::from_stored(&stored_kind)?;
        kinds.insert(system_id, kind);
    }
    Ok(kinds)
}

fn core_location_category(core_path: Option<&str>) -> Option<&'static str> {
    let path = core_path?.replace('\\', "/").to_ascii_lowercase();
    if path.split('/').any(|part| part == "_arcade") {
        Some("Arcade")
    } else if path.split('/').any(|part| part == "_computer") {
        Some("Computer")
    } else if path.split('/').any(|part| part == "_console") {
        Some("Console")
    } else {
        None
    }
}

fn load_launcher_launch_plans(conn: &Connection) -> Result<Vec<StructuredLaunchPlan>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT launch_ref,
                    title,
                    system_id,
                    core_path,
                    payload_path,
                    mount_kind,
                    mount_index,
                    delay_secs
             FROM launcher_launch_plans_text
             ORDER BY launch_id",
        )
        .map_err(|e| format!("prepare launcher launch plans query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(StructuredLaunchPlan {
                launch_ref: row.get::<_, String>(0)?.into(),
                title: row.get::<_, String>(1)?.into(),
                system_id: row.get::<_, String>(2)?.into(),
                core_path: row.get::<_, String>(3)?.into(),
                payload_path: row.get::<_, String>(4)?.into(),
                mount_kind: row.get::<_, String>(5)?.into(),
                mount_index: row.get::<_, i64>(6)?.clamp(0, u8::MAX as i64) as u8,
                delay_secs: row.get::<_, i64>(7)?.clamp(0, u8::MAX as i64) as u8,
            })
        })
        .map_err(|e| format!("query launcher launch plans: {e}"))?;
    rows.map(|row| row.map_err(|e| format!("read launcher launch plan row: {e}")))
        .collect()
}

#[derive(Clone, Copy, Debug, Default)]
struct CatalogSqlQueryTiming {
    prepare_us: u64,
    first_row_us: u64,
    row_read_us: u64,
    row_hydrate_us: u64,
}

struct CatalogSqlQueryResult {
    games: Vec<ArcadeGameEntry>,
    timing: CatalogSqlQueryTiming,
}

const CATALOG_GAME_ENTRY_COLUMNS: [&str; 11] = [
    "title",
    "launch_ref",
    "preview_asset_key",
    "has_preview",
    "system_id",
    "year",
    "manufacturer",
    "category",
    "players",
    "control",
    "discovered_at_unix",
];

const GAME_ENTRY_TITLE: usize = 0;
const GAME_ENTRY_LAUNCH_REF: usize = 1;
const GAME_ENTRY_PREVIEW_ASSET_KEY: usize = 2;
const GAME_ENTRY_HAS_PREVIEW: usize = 3;
const GAME_ENTRY_SYSTEM_ID: usize = 4;
const GAME_ENTRY_YEAR: usize = 5;
const GAME_ENTRY_MANUFACTURER: usize = 6;
const GAME_ENTRY_CATEGORY: usize = 7;
const GAME_ENTRY_PLAYERS: usize = 8;
const GAME_ENTRY_CONTROL: usize = 9;
const GAME_ENTRY_DISCOVERED_AT_UNIX: usize = 10;

fn catalog_game_entry_select_sql(source: &str, where_sql: &str, order_by: &str) -> String {
    format!(
        "SELECT {}
         FROM {source}{where_sql}
         ORDER BY {order_by}",
        CATALOG_GAME_ENTRY_COLUMNS.join(",\n                ")
    )
}

#[cfg(test)]
fn catalog_game_entry_column_names() -> &'static [&'static str] {
    &CATALOG_GAME_ENTRY_COLUMNS
}

pub(crate) fn load_materialized_ui_catalog(
    conn: &Connection,
) -> Result<Option<Vec<ArcadeGameEntry>>, String> {
    if !sqlite_table_exists(conn, "ui_arcade_preferred")? {
        return Ok(None);
    }
    let mut games = query_game_entries_from_source(
        conn,
        "ui_arcade_preferred_text",
        "",
        "ordinal",
        "ui arcade preferred",
    )?;
    if sqlite_table_exists(conn, "launcher_catalog")? {
        games.extend(query_game_entries_from_source(
            conn,
            "launcher_catalog_text",
            " WHERE system_id NOT IN ('arcade','neogeo')",
            "ordinal",
            "launcher catalog extras",
        )?);
    }
    // The schema creates these projection tables even for source-fact or
    // legacy writers that skip materialization. An empty table is therefore
    // not evidence of a complete, repair-safe projection.
    Ok((!games.is_empty()).then_some(games))
}

fn load_materialized_launcher_catalog(
    conn: &Connection,
) -> Result<Option<CatalogSqlQueryResult>, String> {
    if !sqlite_table_exists(conn, "launcher_catalog")? {
        return Ok(None);
    }
    let result = query_game_entries_from_source_with_timing(
        conn,
        "launcher_catalog_text",
        "",
        "ordinal",
        "launcher catalog",
    )?;
    Ok((!result.games.is_empty()).then_some(result))
}

fn query_game_entries_from_source(
    conn: &Connection,
    source: &str,
    where_sql: &str,
    order_by: &str,
    label: &str,
) -> Result<Vec<ArcadeGameEntry>, String> {
    query_game_entries_from_source_with_timing(conn, source, where_sql, order_by, label)
        .map(|result| result.games)
}

fn query_game_entries_from_source_with_timing(
    conn: &Connection,
    source: &str,
    where_sql: &str,
    order_by: &str,
    label: &str,
) -> Result<CatalogSqlQueryResult, String> {
    let sql = catalog_game_entry_select_sql(source, where_sql, order_by);
    query_game_entries_with_timing(conn, &sql, label)
}

fn query_game_entries_with_timing(
    conn: &Connection,
    sql: &str,
    label: &str,
) -> Result<CatalogSqlQueryResult, String> {
    query_game_entries_with_timing_and_params(conn, sql, [], label)
}

fn query_game_entries_with_timing_and_params<const N: usize>(
    conn: &Connection,
    sql: &str,
    params: [&str; N],
    label: &str,
) -> Result<CatalogSqlQueryResult, String> {
    let now = library_db::unix_now_secs();
    let prepare_t = Instant::now();
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("prepare {label} query: {e}"))?;
    let prepare_us = prepare_t.elapsed().as_micros() as u64;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params))
        .map_err(|e| format!("query {label}: {e}"))?;
    let row_read_t = Instant::now();
    let mut first_row_us = 0;
    let mut row_hydrate_us = 0;
    let mut games = Vec::new();
    let mut first_row_seen = false;
    let mut preview_archive_paths_by_system = HashMap::<String, std::sync::Arc<str>>::new();
    loop {
        let next_t = Instant::now();
        let row = rows.next().map_err(|e| format!("read {label} row: {e}"))?;
        if !first_row_seen {
            first_row_us = next_t.elapsed().as_micros() as u64;
            first_row_seen = true;
        }
        let Some(row) = row else {
            break;
        };
        let hydrate_t = Instant::now();
        let game = game_entry_from_row(row, now, &mut preview_archive_paths_by_system)
            .map_err(|e| format!("hydrate {label} row: {e}"))?;
        row_hydrate_us += hydrate_t.elapsed().as_micros() as u64;
        games.push(game);
    }
    Ok(CatalogSqlQueryResult {
        games,
        timing: CatalogSqlQueryTiming {
            prepare_us,
            first_row_us,
            row_read_us: row_read_t.elapsed().as_micros() as u64,
            row_hydrate_us,
        },
    })
}

fn game_entry_from_row(
    row: &rusqlite::Row<'_>,
    now_unix: i64,
    preview_archive_paths_by_system: &mut HashMap<String, std::sync::Arc<str>>,
) -> rusqlite::Result<ArcadeGameEntry> {
    let system_id: String = row.get(GAME_ENTRY_SYSTEM_ID)?;
    let preview_asset_key: String = row.get(GAME_ENTRY_PREVIEW_ASSET_KEY)?;
    let has_preview = row.get::<_, i64>(GAME_ENTRY_HAS_PREVIEW)? != 0;
    let preview_archive_path = if !has_preview || preview_asset_key.is_empty() {
        std::sync::Arc::<str>::from("")
    } else {
        preview_archive_paths_by_system
            .entry(system_id.clone())
            .or_insert_with(|| preview_worker::preview_archive_path_for_system(&system_id).into())
            .clone()
    };
    let discovered_at_unix = row.get::<_, Option<i64>>(GAME_ENTRY_DISCOVERED_AT_UNIX)?;
    Ok(ArcadeGameEntry {
        title: row.get::<_, String>(GAME_ENTRY_TITLE)?.into(),
        mra_path: row.get::<_, String>(GAME_ENTRY_LAUNCH_REF)?.into(),
        preview_archive_path,
        preview_asset_key: preview_asset_key.into(),
        has_preview,
        system_id: system_id.into(),
        year: optional_year_from_row(row, GAME_ENTRY_YEAR)?,
        manufacturer: row
            .get::<_, Option<String>>(GAME_ENTRY_MANUFACTURER)?
            .unwrap_or_default()
            .into(),
        category: row
            .get::<_, Option<String>>(GAME_ENTRY_CATEGORY)?
            .unwrap_or_default()
            .into(),
        players: row
            .get::<_, Option<i64>>(GAME_ENTRY_PLAYERS)?
            .and_then(|value| u8::try_from(value).ok()),
        control: row
            .get::<_, Option<String>>(GAME_ENTRY_CONTROL)?
            .unwrap_or_default()
            .into(),
        is_new: is_new_discovery(discovered_at_unix, now_unix),
    })
}

pub(crate) fn repair_catalog_projections_for_catalog(
    sqlite_path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &catalog_stamp::CatalogStamp,
) -> Result<(), String> {
    if catalog_projection_pair_current(sqlite_path, stamp).unwrap_or(false)
        && catalog_projection_filter_mismatches(sqlite_path, catalog, stamp)?.is_empty()
    {
        return Ok(());
    }
    rewrite_catalog_projections_for_catalog(sqlite_path, catalog, stamp)
}

pub(crate) fn catalog_projection_filter_mismatches(
    sqlite_path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &catalog_stamp::CatalogStamp,
) -> Result<Vec<String>, String> {
    let mut mismatches = Vec::new();
    let navigation_path = catalog_navigation::navigation_path_for_sqlite(sqlite_path);
    match catalog_navigation::read_catalog_navigation_projection(&navigation_path, stamp)? {
        Some(projection) => {
            let projected = ArcadeCatalog::from_navigation_projection(
                Path::new(arcade_catalog::DEFAULT_ARCADE_ROOT),
                projection,
            );
            mismatches.extend(
                catalog
                    .filter_option_mismatches(&projected)
                    .into_iter()
                    .map(|detail| format!("external {detail}")),
            );
        }
        None => mismatches.push("external navigation projection missing or stale".to_string()),
    }

    let conn = open_sqlite_read_only(sqlite_path)
        .map_err(|e| format!("open embedded navigation for parity: {e}"))?;
    match load_embedded_catalog_navigation(&conn, stamp)? {
        Some(projection) => {
            let projected = ArcadeCatalog::from_navigation_projection(
                Path::new(arcade_catalog::DEFAULT_ARCADE_ROOT),
                projection,
            );
            mismatches.extend(
                catalog
                    .filter_option_mismatches(&projected)
                    .into_iter()
                    .map(|detail| format!("embedded {detail}")),
            );
        }
        None => mismatches.push("embedded navigation projection missing or stale".to_string()),
    }
    Ok(mismatches)
}

pub(crate) fn rewrite_catalog_projections_for_catalog(
    sqlite_path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &catalog_stamp::CatalogStamp,
) -> Result<(), String> {
    let summary = catalog_summary::CatalogSummaryProjection::from_catalog(catalog, stamp);
    let navigation = catalog_navigation::CatalogNavigationProjection::from_catalog(catalog, stamp);
    let embedded = catalog_navigation::encode_catalog_navigation_for_storage(catalog, stamp)?;
    // Each projection is replaced atomically. Write navigation first so a
    // failed repair never removes the previous usable projection.
    catalog_navigation::write_catalog_navigation_projection_for_sqlite(sqlite_path, &navigation)?;
    catalog_summary::write_catalog_summary_projection(sqlite_path, &summary)?;
    let conn = Connection::open(sqlite_path)
        .map_err(|e| format!("open embedded navigation for repair: {e}"))?;
    conn.execute(
        "INSERT OR REPLACE INTO catalog_navigation_projection(id,bytes) VALUES(0,?1)",
        [&embedded],
    )
    .map_err(|e| format!("atomically repair embedded catalog navigation: {e}"))?;
    Ok(())
}

pub(crate) fn catalog_projection_pair_current(
    sqlite_path: &Path,
    stamp: &catalog_stamp::CatalogStamp,
) -> Result<bool, String> {
    let summary_path = catalog_summary::summary_path_for_sqlite(sqlite_path);
    let Some(summary) = catalog_summary::read_catalog_summary(&summary_path)? else {
        return Ok(false);
    };
    if summary.catalog_stamp_fingerprint != stamp.fingerprint_hex() {
        return Ok(false);
    }
    let navigation_path = catalog_navigation::navigation_path_for_sqlite(sqlite_path);
    catalog_navigation::read_catalog_navigation_projection(&navigation_path, stamp)
        .map(|projection| projection.is_some())
}

pub(crate) fn sqlite_table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type IN ('table','view') AND name=?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists != 0)
    .map_err(|e| format!("check sqlite table {table}: {e}"))
}

fn sqlite_physical_table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists != 0)
    .map_err(|e| format!("check sqlite physical table {table}: {e}"))
}

fn optional_year_from_row(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<u16>> {
    match row.get_ref(index)? {
        ValueRef::Null => Ok(None),
        ValueRef::Integer(year) => Ok(u16::try_from(year).ok()),
        ValueRef::Real(year) if year.fract() == 0.0 => Ok(u16::try_from(year as i64).ok()),
        ValueRef::Real(_) | ValueRef::Blob(_) => Ok(None),
        ValueRef::Text(bytes) => {
            let Ok(text) = std::str::from_utf8(bytes) else {
                return Ok(None);
            };
            Ok(parse_catalog_year(text))
        }
    }
}

fn parse_catalog_year(text: &str) -> Option<u16> {
    let trimmed = text.trim();
    if trimmed.len() != 4 || !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    trimmed.parse().ok()
}

pub(crate) fn sqlite_column_exists(
    conn: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| format!("prepare sqlite column check {table}.{column}: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("query sqlite column check {table}.{column}: {e}"))?;
    for row in rows {
        if row.map_err(|e| format!("read sqlite column check {table}.{column}: {e}"))? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn is_new_discovery(discovered_at_unix: Option<i64>, now_unix: i64) -> bool {
    discovered_at_unix.is_some_and(|discovered_at_unix| {
        discovered_at_unix <= now_unix && now_unix - discovered_at_unix <= NEW_GAME_BADGE_SECS
    })
}

pub(crate) fn open_sqlite_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let uri = format!("file:{}?mode=ro&immutable=1", sqlite_uri_path(path));
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    register_sqlite_catalog_functions(&conn)?;
    Ok(conn)
}

pub(crate) fn read_sqlite_catalog_stamp(
    path: &Path,
) -> Result<Option<catalog_stamp::CatalogStamp>, String> {
    let conn = open_sqlite_read_only(path)
        .map_err(|e| format!("open catalog stamp db {}: {e}", path.display()))?;
    catalog_store::read_catalog_stamp(&conn)
}

pub(crate) fn sqlite_uri_path(path: &Path) -> String {
    path.to_string_lossy()
        .bytes()
        .flat_map(|byte| match byte {
            b'%' => "%25".bytes().collect::<Vec<_>>(),
            b'?' => "%3F".bytes().collect(),
            b'#' => "%23".bytes().collect(),
            b' ' => "%20".bytes().collect(),
            other => vec![other],
        })
        .map(char::from)
        .collect()
}

pub(crate) fn load_joined_launcher_catalog(
    conn: &Connection,
) -> Result<Vec<ArcadeGameEntry>, String> {
    let now = library_db::unix_now_secs();
    let mut stmt = conn
        .prepare(
            "SELECT games.title,
                    launch_plans.launch_ref,
                    '',
                    '',
                    0,
                    COALESCE(games.system_id,'unknown'),
                    games.year,
                    games.manufacturer,
                    NULL,
                    NULL,
                    NULL,
                    games.discovered_at_unix,
                    launch_plans.launch_kind,
                    COALESCE(launch_plans.setname,''),
                    COALESCE(launch_plans.parent,'')
             FROM games
             JOIN launch_plans ON launch_plans.game_id = games.game_id
             WHERE launch_plans.launch_ref != ''
               AND launch_plans.launch_kind IN ('mra','mgl','virtual-mgl','catalog-entry')
               AND (
                 lower(launch_plans.launch_ref) LIKE '%.mra'
                 OR lower(launch_plans.launch_ref) LIKE '%.mgl'
                 OR launch_plans.launch_kind='virtual-mgl'
                 OR launch_plans.launch_kind='catalog-entry'
               )
             ORDER BY lower(games.title)",
        )
        .map_err(|e| format!("prepare arcade catalog query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let discovered_at_unix = row.get::<_, Option<i64>>(11)?;
            let preview =
                LauncherPreviewAsset::new(row.get::<_, String>(2)?, row.get::<_, String>(3)?);
            Ok(CatalogProjectionRow::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(5)?,
                preview,
                ArcadeGameMetadataKey {
                    year: optional_year_from_row(row, 6)?,
                    manufacturer: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                    category: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                    players: row
                        .get::<_, Option<i64>>(9)?
                        .and_then(|value| u8::try_from(value).ok()),
                    control: row.get::<_, Option<String>>(10)?.unwrap_or_default(),
                },
                is_new_discovery(discovered_at_unix, now),
                CatalogProjectionSource {
                    source_kind: row.get::<_, String>(12)?,
                    setname: row.get::<_, String>(13)?,
                    parent: row.get::<_, String>(14)?,
                    family_key: None,
                    identity_matched: false,
                    prepared: None,
                },
            ))
        })
        .map_err(|e| format!("query arcade catalog: {e}"))?;
    let mut rows_out = Vec::new();
    for row in rows {
        rows_out.push(row.map_err(|e| format!("read arcade catalog row: {e}"))?);
    }
    rows_out.retain(|row| is_launcher_launch_ref(&row.game.mra_path));
    Ok(catalog_projection::collapse_catalog_variants(rows_out))
}

pub(crate) fn sqlite_catalog_stamp_check(
    cfg: &BenchConfig,
) -> Result<CatalogStampCheckSummary, String> {
    catalog_stamp_check_at(cfg, &cfg.sqlite_path, false)
}

pub(crate) fn catalog_state_stamp_check(
    cfg: &BenchConfig,
    state_path: &Path,
) -> Result<CatalogStampCheckSummary, String> {
    catalog_stamp_check_at(cfg, state_path, true)
}

fn catalog_stamp_check_at(
    cfg: &BenchConfig,
    state_path: &Path,
    validate_state_schema: bool,
) -> Result<CatalogStampCheckSummary, String> {
    let started = Instant::now();
    let open_t = Instant::now();
    let conn = open_sqlite_read_only(state_path)
        .map_err(|e| format!("open catalog stamp db {}: {e}", state_path.display()))?;
    let open_us = open_t.elapsed().as_micros() as u64;
    if validate_state_schema {
        crate::catalog_state::validate_connection(&conn, state_path)?;
    }
    let read_t = Instant::now();
    let stored = catalog_store::read_catalog_stamp(&conn)?;
    let read_us = read_t.elapsed().as_micros() as u64;
    let checkpoint_read_t = Instant::now();
    let stored_checkpoint = catalog_store::read_catalog_discovery_checkpoint(&conn)?;
    let checkpoint_read_us = checkpoint_read_t.elapsed().as_micros() as u64;
    let compute_t = Instant::now();
    let installed_cores = catalog_discovery::installed_cores_for_roots(&cfg.roots);
    let game_dir_headers = catalog_discovery::top_level_game_dir_headers_for_roots_excluding(
        &cfg.roots,
        &std::collections::BTreeSet::new(),
    );
    catalog_checkpoint::report_checkpoint_timing(
        "coverage_audit",
        0,
        "skipped=retained-directory-signatures",
    );
    let current_live_stamp = catalog_stamp::compute_default_catalog_stamp(&cfg.roots);
    let empty_checkpoint = catalog_checkpoint::CatalogDiscoveryCheckpoint::from_lines(Vec::new());
    let current_checkpoint = catalog_checkpoint::compute_catalog_discovery_checkpoint_probe(
        &cfg.roots,
        &default_mame_sqlite_path(),
        &default_hbmame_sqlite_path(),
        &installed_cores,
        &game_dir_headers,
        stored_checkpoint.as_ref().unwrap_or(&empty_checkpoint),
    );
    let probe_compute_us = compute_t.elapsed().as_micros() as u64;
    let compare_t = Instant::now();
    let (stored_fingerprint, stored_lines, stamp_unchanged) = match &stored {
        Some(stored) => {
            let stored_fingerprint = stored.fingerprint_hex();
            let stored_lines = stored.lines().len();
            let unchanged = stored.has_same_live_inputs(&current_live_stamp);
            (Some(stored_fingerprint), stored_lines, unchanged)
        }
        None => (None, 0, false),
    };
    let checkpoint_compare_t = Instant::now();
    let probe_drift =
        CatalogDriftSummary::from_checkpoints(stored_checkpoint.as_ref(), &current_checkpoint);
    let mut checkpoint_compare_us = checkpoint_compare_t.elapsed().as_micros() as u64;
    let (stored_checkpoint_fingerprint, stored_checkpoint_lines) =
        stored_checkpoint.as_ref().map_or((None, 0), |checkpoint| {
            (Some(checkpoint.fingerprint_hex()), checkpoint.lines().len())
        });
    if stamp_unchanged && probe_drift.unchanged {
        catalog_checkpoint::report_drift_summary(&probe_drift);
        let compare_us = compare_t.elapsed().as_micros() as u64;
        return Ok(CatalogStampCheckSummary {
            unchanged: true,
            check_us: started.elapsed().as_micros() as u64,
            compute_us: probe_compute_us,
            open_us,
            read_us,
            checkpoint_read_us,
            checkpoint_compare_us,
            compare_us,
            stored_fingerprint: stored_fingerprint.clone(),
            current_fingerprint: stored_fingerprint.unwrap_or_default(),
            stored_checkpoint_fingerprint,
            current_checkpoint_fingerprint: current_checkpoint.fingerprint_hex(),
            stored_lines,
            current_lines: stored_lines,
            stored_checkpoint_lines,
            current_checkpoint_lines: current_checkpoint.lines().len(),
            drift: probe_drift,
        });
    }

    // A probe miss is not a semantic change by itself. Recompute the exact
    // depth-two facts and audit only on that exceptional path, preserving the
    // established drift/fingerprint contract for metadata churn and errors.
    let fallback_t = Instant::now();
    let game_dirs = catalog_discovery::top_level_game_dirs_for_roots(&cfg.roots);
    let profiles =
        launch_profiles::active_profiles_for_roots_with_facts(&installed_cores, &game_dirs);
    let audit_t = Instant::now();
    let audit_rows = core_audit::audit_catalog_coverage_from_facts(
        &cfg.roots,
        &profiles,
        &installed_cores,
        &game_dirs,
    );
    catalog_checkpoint::report_checkpoint_timing(
        "coverage_audit_fallback",
        audit_t.elapsed().as_micros() as u64,
        format!("rows={}", audit_rows.len()),
    );
    let exact_stamp =
        catalog_stamp::compute_default_catalog_stamp_with_audit(&cfg.roots, &audit_rows);
    let exact_checkpoint = catalog_checkpoint::compute_catalog_discovery_checkpoint_from_facts(
        &cfg.roots,
        &default_mame_sqlite_path(),
        &default_hbmame_sqlite_path(),
        &audit_rows,
        &installed_cores,
        &game_dirs,
    );
    let stored_semantic_checkpoint = stored_checkpoint
        .as_ref()
        .map(catalog_checkpoint::without_probe_lines);
    let exact_semantic_checkpoint = catalog_checkpoint::without_probe_lines(&exact_checkpoint);
    let exact_checkpoint_compare_t = Instant::now();
    let drift = CatalogDriftSummary::from_checkpoints(
        stored_semantic_checkpoint.as_ref(),
        &exact_semantic_checkpoint,
    );
    checkpoint_compare_us = checkpoint_compare_us
        .saturating_add(exact_checkpoint_compare_t.elapsed().as_micros() as u64);
    catalog_checkpoint::report_drift_summary(&drift);
    let stamp_unchanged = stored.as_ref().is_some_and(|stored| stored == &exact_stamp);
    let unchanged = stamp_unchanged && drift.unchanged;
    let compute_us = probe_compute_us.saturating_add(fallback_t.elapsed().as_micros() as u64);
    let compare_us = compare_t.elapsed().as_micros() as u64;
    Ok(CatalogStampCheckSummary {
        unchanged,
        check_us: started.elapsed().as_micros() as u64,
        compute_us,
        open_us,
        read_us,
        checkpoint_read_us,
        checkpoint_compare_us,
        compare_us,
        stored_fingerprint,
        current_fingerprint: exact_stamp.fingerprint_hex(),
        stored_checkpoint_fingerprint,
        current_checkpoint_fingerprint: exact_semantic_checkpoint.fingerprint_hex(),
        stored_lines,
        current_lines: exact_stamp.lines().len(),
        stored_checkpoint_lines,
        current_checkpoint_lines: exact_semantic_checkpoint.lines().len(),
        drift,
    })
}

#[cfg(test)]
pub(crate) fn save_sqlite_scan(path: &Path, scan: &LibraryScan) -> Result<u64, String> {
    save_sqlite_scan_with_progress(path, scan, None)
}

#[cfg(test)]
pub(crate) fn save_sqlite_scan_with_progress(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
) -> Result<u64, String> {
    save_sqlite_scan_with_progress_and_stamp(path, scan, None, progress)
}

#[cfg(test)]
pub(crate) fn save_sqlite_scan_with_progress_and_stamp(
    path: &Path,
    scan: &LibraryScan,
    stamp: Option<&catalog_stamp::CatalogStamp>,
    progress: ProgressCallback<'_>,
) -> Result<u64, String> {
    if stamp.is_some() {
        return save_sqlite_scan_with_progress_and_stamp_and_catalog(
            path,
            scan,
            stamp,
            arcade_catalog::DEFAULT_ARCADE_ROOT,
            progress,
        )
        .map(|saved| saved.bytes);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create sqlite dir: {e}"))?;
    }

    let discovery_history = DiscoveryHistory::load(path);
    let mut writer =
        |build_path: &Path, scan: &LibraryScan, progress: &mut ProgressCallback<'_>| {
            let software_hash_cache = SoftwareHashCache::load(path);
            write_sqlite_scan(
                build_path,
                scan,
                reborrow_progress(progress),
                software_hash_cache,
                discovery_history.clone(),
                stamp,
            )
        };
    let bytes = save_sqlite_scan_with_progress_using_writer(
        path,
        scan,
        progress,
        sqlite_build_temp_plan(path),
        &mut writer,
    )?;
    Ok(bytes)
}

#[cfg(test)]
pub(crate) fn save_sqlite_scan_with_progress_and_stamp_and_catalog(
    path: &Path,
    scan: &LibraryScan,
    stamp: Option<&catalog_stamp::CatalogStamp>,
    root: impl AsRef<Path>,
    progress: ProgressCallback<'_>,
) -> Result<SqliteSavedCatalog, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create sqlite dir: {e}"))?;
    }

    let root = root.as_ref().to_path_buf();
    let discovery_history = DiscoveryHistory::load(path);
    let mut saved_catalog = None;
    let bytes = {
        let mut writer =
            |build_path: &Path, scan: &LibraryScan, progress: &mut ProgressCallback<'_>| {
                let software_hash_cache = SoftwareHashCache::load(path);
                let catalog = write_sqlite_scan_with_catalog(
                    build_path,
                    scan,
                    &root,
                    reborrow_progress(progress),
                    software_hash_cache,
                    discovery_history.clone(),
                    stamp,
                )?;
                saved_catalog = Some(catalog);
                Ok(())
            };
        save_sqlite_scan_with_progress_using_writer(
            path,
            scan,
            progress,
            sqlite_build_temp_plan(path),
            &mut writer,
        )?
    };
    let catalog = saved_catalog.ok_or_else(|| "saved catalog was not returned".to_string())?;
    if let Some(stamp) = stamp {
        catalog_summary::write_catalog_summary_for_catalog(path, &catalog.catalog, stamp)?;
        catalog_navigation::write_catalog_navigation_projection_for_catalog(
            path,
            &catalog.catalog,
            stamp,
        )?;
    }
    Ok(SqliteSavedCatalog { bytes, catalog })
}

pub(crate) fn save_sqlite_scan_with_progress_and_stamp_and_projections(
    path: &Path,
    scan: &LibraryScan,
    stamp: &catalog_stamp::CatalogStamp,
    root: &Path,
    progress: ProgressCallback<'_>,
) -> Result<u64, String> {
    save_sqlite_scan_with_progress_and_stamp_and_projections_with_bench_iteration(
        path,
        scan,
        stamp,
        root,
        progress,
        None,
        sqlite_build_temp_plan(path),
    )
}

pub(crate) fn save_sqlite_scan_with_progress_and_stamp_and_projections_with_bench_iteration(
    path: &Path,
    scan: &LibraryScan,
    stamp: &catalog_stamp::CatalogStamp,
    root: &Path,
    progress: ProgressCallback<'_>,
    bench_iteration: Option<usize>,
    initial_plan: SqliteBuildTempPlan,
) -> Result<u64, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create sqlite dir: {e}"))?;
    }

    let discovery_history = DiscoveryHistory::load(path);
    let mut projections = None;
    let bytes = {
        let mut writer =
            |build_path: &Path, scan: &LibraryScan, progress: &mut ProgressCallback<'_>| {
                let software_hash_cache = SoftwareHashCache::load(path);
                write_sqlite_scan_without_catalog_rebuild(
                    build_path,
                    scan,
                    reborrow_progress(progress),
                    software_hash_cache,
                    discovery_history.clone(),
                    Some(stamp),
                    None,
                    None,
                    true,
                )?;
                projections = Some(build_catalog_projections_from_materialized_sqlite(
                    build_path, root, stamp,
                )?);
                Ok(())
            };
        let mut fault_control = crate::fs_fault::NoopDirectResetFaultControl;
        save_sqlite_scan_with_progress_using_writer_and_bench_iteration(
            path,
            scan,
            progress,
            initial_plan,
            &mut writer,
            bench_iteration,
            &mut fault_control,
        )?
    };
    let projections = projections
        .ok_or_else(|| "catalog projections were not built before publish".to_string())?;
    write_catalog_projection_pair(path, projections)?;
    Ok(bytes)
}

pub(crate) fn save_sqlite_scan_with_progress_and_stamp_and_catalog_projection(
    path: &Path,
    scan: &LibraryScan,
    stamp: &catalog_stamp::CatalogStamp,
    catalog: &ArcadeCatalog,
    progress: ProgressCallback<'_>,
) -> Result<u64, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create sqlite dir: {e}"))?;
    }

    let discovery_history = DiscoveryHistory::load(path);
    let canonical_navigation =
        catalog_navigation::encode_catalog_navigation_for_storage(catalog, stamp)?;
    let bytes = {
        let mut writer =
            |build_path: &Path, scan: &LibraryScan, progress: &mut ProgressCallback<'_>| {
                let software_hash_cache = SoftwareHashCache::load(path);
                write_sqlite_scan_without_catalog_rebuild(
                    build_path,
                    scan,
                    reborrow_progress(progress),
                    software_hash_cache,
                    discovery_history.clone(),
                    Some(stamp),
                    Some(&canonical_navigation),
                    Some(catalog),
                    // The embedded navigation payload is canonical. Retain the
                    // materialized compatibility tables until every release,
                    // acceptance, diagnostic, and benchmark selector has moved
                    // to that canonical contract.
                    true,
                )
            };
        save_sqlite_scan_with_progress_using_writer(
            path,
            scan,
            progress,
            sqlite_build_temp_plan(path),
            &mut writer,
        )?
    };
    repair_catalog_projections_for_catalog(path, catalog, stamp)?;
    Ok(bytes)
}

struct CatalogProjectionPair {
    summary: catalog_summary::CatalogSummaryProjection,
    navigation: catalog_navigation::CatalogNavigationProjection,
}

fn build_catalog_projections_from_materialized_sqlite(
    sqlite_path: &Path,
    root: &Path,
    stamp: &catalog_stamp::CatalogStamp,
) -> Result<CatalogProjectionPair, String> {
    let loaded = load_arcade_catalog_from_materialized_sqlite_at(root, sqlite_path)?;
    let summary = catalog_summary::CatalogSummaryProjection::from_catalog(&loaded.catalog, stamp);
    let navigation =
        catalog_navigation::CatalogNavigationProjection::from_catalog(&loaded.catalog, stamp);
    Ok(CatalogProjectionPair {
        summary,
        navigation,
    })
}

fn write_catalog_projection_pair(
    sqlite_path: &Path,
    projections: CatalogProjectionPair,
) -> Result<(), String> {
    catalog_navigation::write_catalog_navigation_projection_for_sqlite(
        sqlite_path,
        &projections.navigation,
    )?;
    catalog_summary::write_catalog_summary_projection(sqlite_path, &projections.summary)
}

pub(crate) fn save_sqlite_scan_with_progress_using_writer(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
    initial_plan: SqliteBuildTempPlan,
    writer: &mut dyn FnMut(&Path, &LibraryScan, &mut ProgressCallback<'_>) -> Result<(), String>,
) -> Result<u64, String> {
    let mut fault_control = crate::fs_fault::NoopDirectResetFaultControl;
    save_sqlite_scan_with_progress_using_writer_and_bench_iteration(
        path,
        scan,
        progress,
        initial_plan,
        writer,
        None,
        &mut fault_control,
    )
}

fn save_sqlite_scan_with_progress_using_writer_and_bench_iteration(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
    initial_plan: SqliteBuildTempPlan,
    writer: &mut dyn FnMut(&Path, &LibraryScan, &mut ProgressCallback<'_>) -> Result<(), String>,
    bench_iteration: Option<usize>,
    fault_control: &mut dyn crate::fs_fault::DirectResetFaultControl,
) -> Result<u64, String> {
    let mut progress = progress;
    let first = save_sqlite_scan_attempt_with_writer_and_bench_iteration(
        path,
        scan,
        &mut progress,
        &initial_plan,
        writer,
        bench_iteration,
        fault_control,
    );
    match first {
        Ok(bytes) => Ok(bytes),
        Err(e)
            if initial_plan.source == SqliteBuildTempSource::DefaultTmpfs
                && sqlite_build_error_should_retry_beside_final(&e) =>
        {
            crate::catalog_errln!(
                "library sqlite build temp failed at {}; retrying beside final DB: {e}",
                initial_plan.build_tmp_path.display()
            );
            let fallback_plan = sqlite_build_temp_plan_beside_final(path);
            save_sqlite_scan_attempt_with_writer_and_bench_iteration(
                path,
                scan,
                &mut progress,
                &fallback_plan,
                writer,
                bench_iteration,
                fault_control,
            )
        }
        Err(e) => Err(e),
    }
}

fn save_sqlite_scan_attempt_with_writer_and_bench_iteration(
    path: &Path,
    scan: &LibraryScan,
    progress: &mut ProgressCallback<'_>,
    plan: &SqliteBuildTempPlan,
    writer: &mut dyn FnMut(&Path, &LibraryScan, &mut ProgressCallback<'_>) -> Result<(), String>,
    bench_iteration: Option<usize>,
    fault_control: &mut dyn crate::fs_fault::DirectResetFaultControl,
) -> Result<u64, String> {
    if let Some(parent) = plan.build_tmp_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create sqlite build dir {}: {e}", parent.display()))?;
    }
    if let Some(parent) = plan.final_tmp_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create sqlite final temp dir {}: {e}", parent.display()))?;
    }
    for tmp_path in [&plan.build_tmp_path, &plan.final_tmp_path] {
        match std::fs::remove_file(tmp_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("remove stale sqlite temp: {e}")),
        }
    }

    if let Err(e) = writer(&plan.build_tmp_path, scan, progress) {
        let _ = std::fs::remove_file(&plan.build_tmp_path);
        return Err(e);
    }
    let metrics = publish_sqlite_temp(path, plan, progress, fault_control).inspect_err(|_| {
        let _ = std::fs::remove_file(&plan.final_tmp_path);
        let _ = std::fs::remove_file(&plan.build_tmp_path);
    })?;
    report_sqlite_publish_metrics(&metrics, "bench-ok", bench_iteration);
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("stat sqlite: {e}"))
}

fn publish_sqlite_temp(
    final_path: &Path,
    plan: &SqliteBuildTempPlan,
    progress: &mut ProgressCallback<'_>,
    fault_control: &mut dyn crate::fs_fault::DirectResetFaultControl,
) -> Result<SqlitePublishMetrics, String> {
    let started = Instant::now();
    let mut metrics = SqlitePublishMetrics {
        bytes: std::fs::metadata(&plan.build_tmp_path)
            .map(|m| m.len())
            .map_err(|e| format!("stat sqlite build temp: {e}"))?,
        ..Default::default()
    };

    let build_sync_t = Instant::now();
    sync_file_best_effort(&plan.build_tmp_path, "sqlite build temp")?;
    crate::fs_fault::maybe_fault_with_control(
        "catalog.sqlite.after_build_temp_sync",
        final_path,
        fault_control,
    );
    metrics.build_sync_ms = elapsed_ms(build_sync_t.elapsed());

    if plan.build_tmp_path != plan.final_tmp_path {
        let copy_t = Instant::now();
        metrics.progress_events =
            copy_sqlite_temp_with_progress(&plan.build_tmp_path, &plan.final_tmp_path, progress)?;
        crate::fs_fault::maybe_fault_with_control(
            "catalog.sqlite.after_final_temp_copy",
            final_path,
            fault_control,
        );
        metrics.copy_ms = elapsed_ms(copy_t.elapsed());
        let _ = std::fs::remove_file(&plan.build_tmp_path);
    } else {
        emit_sqlite_save_progress(progress, metrics.bytes, metrics.bytes);
        metrics.progress_events = metrics.progress_events.saturating_add(1);
    }

    let final_sync_t = Instant::now();
    sync_file_best_effort(&plan.final_tmp_path, "sqlite temp")?;
    crate::fs_fault::maybe_fault_with_control(
        "catalog.sqlite.after_final_temp_sync",
        final_path,
        fault_control,
    );
    metrics.final_sync_ms = elapsed_ms(final_sync_t.elapsed());

    let rename_t = Instant::now();
    std::fs::rename(&plan.final_tmp_path, final_path)
        .map_err(|e| format!("replace sqlite: {e}"))?;
    crate::fs_fault::maybe_fault_with_control(
        "catalog.sqlite.after_rename_before_parent_sync",
        final_path,
        fault_control,
    );
    metrics.rename_ms = elapsed_ms(rename_t.elapsed());

    let parent_sync_t = Instant::now();
    sync_parent_dir(final_path);
    metrics.parent_sync_ms = elapsed_ms(parent_sync_t.elapsed());
    metrics.total_ms = elapsed_ms(started.elapsed());
    Ok(metrics)
}

fn copy_sqlite_temp_with_progress(
    source: &Path,
    destination: &Path,
    progress: &mut ProgressCallback<'_>,
) -> Result<u64, String> {
    let total = std::fs::metadata(source)
        .map(|m| m.len())
        .map_err(|e| format!("stat sqlite source: {e}"))?;
    let mut input = File::open(source).map_err(|e| format!("open sqlite source: {e}"))?;
    let mut output = File::create(destination).map_err(|e| format!("create sqlite temp: {e}"))?;
    let mut progress_events = 0u64;
    let mut bytes_done = 0u64;
    let mut next_inner_progress = SQLITE_INNER_PROGRESS_BYTES;
    let mut buffer = vec![0u8; SQLITE_PUBLISH_COPY_CHUNK_BYTES];
    emit_sqlite_save_progress(progress, 0, total);
    progress_events += 1;
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|e| format!("read sqlite source: {e}"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|e| format!("write sqlite temp: {e}"))?;
        bytes_done += read as u64;
        if bytes_done >= next_inner_progress {
            crate::catalog_progress::report_inner_progress();
            next_inner_progress = bytes_done.saturating_add(SQLITE_INNER_PROGRESS_BYTES);
        }
        emit_sqlite_save_progress(progress, bytes_done, total);
        progress_events += 1;
    }
    Ok(progress_events)
}

fn emit_sqlite_save_progress(progress: &mut ProgressCallback<'_>, done: u64, total: u64) {
    report_catalog_progress(
        progress,
        CatalogProgress::saving_sqlite_publish(done, total),
    );
}

fn report_sqlite_publish_metrics(
    metrics: &SqlitePublishMetrics,
    result: &str,
    bench_iteration: Option<usize>,
) {
    let label =
        std::env::var("MISTER_LIBRARY_BENCH_LABEL").unwrap_or_else(|_| "LIB-BENCH".to_string());
    let iteration = bench_iteration.unwrap_or_default();
    crate::catalog_logln!(
        "library_sqlite_publish_tsv\t{label}\t{iteration}\tprogress\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        metrics.bytes,
        metrics.build_sync_ms,
        metrics.copy_ms,
        metrics.final_sync_ms,
        metrics.rename_ms,
        metrics.parent_sync_ms,
        metrics.total_ms,
        metrics.progress_events,
        result
    );
}

fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn reborrow_progress<'a>(progress: &'a mut ProgressCallback<'_>) -> ProgressCallback<'a> {
    progress
        .as_mut()
        .map(|callback| &mut **callback as &mut dyn FnMut(&str, &str))
}

pub(crate) fn sqlite_build_error_should_retry_beside_final(error: &str) -> bool {
    [
        "database or disk is full",
        "disk I/O error",
        "No space left on device",
        "Read-only file system",
        "Permission denied",
        "Input/output error",
        "Not a directory",
        "No such file or directory",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

pub(crate) fn sqlite_build_temp_plan(path: &Path) -> SqliteBuildTempPlan {
    sqlite_build_temp_plan_for(path, None)
}

pub(crate) fn sqlite_build_temp_plan_for(
    path: &Path,
    build_dir_override: Option<&Path>,
) -> SqliteBuildTempPlan {
    if let Some(build_dir) = build_dir_override {
        return SqliteBuildTempPlan {
            build_tmp_path: sqlite_build_temp_path_in_dir(path, build_dir),
            final_tmp_path: sqlite_temp_path(path),
            source: SqliteBuildTempSource::EnvOverride,
        };
    }
    if is_media_fat_path(path) {
        return SqliteBuildTempPlan {
            build_tmp_path: sqlite_build_temp_path_in_dir(
                path,
                Path::new(DEFAULT_SQLITE_BUILD_DIR),
            ),
            final_tmp_path: sqlite_temp_path(path),
            source: SqliteBuildTempSource::DefaultTmpfs,
        };
    }
    sqlite_build_temp_plan_beside_final(path)
}

pub(crate) fn sqlite_build_temp_plan_beside_final(path: &Path) -> SqliteBuildTempPlan {
    let final_tmp_path = sqlite_temp_path(path);
    SqliteBuildTempPlan {
        build_tmp_path: final_tmp_path.clone(),
        final_tmp_path,
        source: SqliteBuildTempSource::BesideFinal,
    }
}

pub(crate) fn sqlite_build_temp_path_in_dir(path: &Path, build_dir: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library.sqlite3");
    build_dir.join(format!(".{name}.build.{}", std::process::id()))
}

pub(crate) fn is_media_fat_path(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(std::path::Component::RootDir))
        && matches!(
            components.next(),
            Some(std::path::Component::Normal(component)) if component == "media"
        )
        && matches!(
            components.next(),
            Some(std::path::Component::Normal(component)) if component == "fat"
        )
}

pub(crate) fn sqlite_temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library.sqlite3");
    path.with_file_name(format!(".{name}.tmp.{}", std::process::id()))
}

pub(crate) fn sync_parent_dir(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
}

pub(crate) fn sync_file_best_effort(path: &Path, label: &str) -> Result<(), String> {
    match File::open(path).and_then(|f| f.sync_all()) {
        Ok(()) => Ok(()),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        Err(e) => Err(format!("sync {label}: {e}")),
    }
}

pub(crate) fn file_signature(path: &Path) -> FileSignature {
    std::fs::metadata(path)
        .map(|metadata| FileSignature {
            size: metadata.len(),
            mtime_secs: library_db::mtime_secs(&metadata),
        })
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn write_sqlite_scan(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
    software_hash_cache: SoftwareHashCache,
    discovery_history: Option<DiscoveryHistory>,
    stamp: Option<&catalog_stamp::CatalogStamp>,
) -> Result<(), String> {
    write_sqlite_scan_with_catalog(
        path,
        scan,
        arcade_catalog::DEFAULT_ARCADE_ROOT,
        progress,
        software_hash_cache,
        discovery_history,
        stamp,
    )
    .map(|_| ())
}

#[cfg(test)]
pub(crate) fn write_sqlite_scan_with_catalog(
    path: &Path,
    scan: &LibraryScan,
    root: impl AsRef<Path>,
    progress: ProgressCallback<'_>,
    software_hash_cache: SoftwareHashCache,
    discovery_history: Option<DiscoveryHistory>,
    stamp: Option<&catalog_stamp::CatalogStamp>,
) -> Result<LibraryCatalogLoad, String> {
    let preview_paths = PreviewArchivePaths::from_paths_with_sidecar_entries(
        preview_worker::preview_archive_paths_for_catalog_projection(),
    );
    let mame_sqlite_path = default_mame_sqlite_path();
    let hbmame_sqlite_path = default_hbmame_sqlite_path();
    write_sqlite_scan_with_sources(
        path,
        scan,
        SqliteScanSources {
            mame_sqlite_path: &mame_sqlite_path,
            hbmame_sqlite_path: &hbmame_sqlite_path,
            preview_paths: &preview_paths,
            software_hash_cache,
            discovery_history,
            stamp,
        },
        root.as_ref(),
        progress,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_sqlite_scan_without_catalog_rebuild(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
    software_hash_cache: SoftwareHashCache,
    discovery_history: Option<DiscoveryHistory>,
    stamp: Option<&catalog_stamp::CatalogStamp>,
    canonical_navigation: Option<&[u8]>,
    canonical_catalog: Option<&ArcadeCatalog>,
    materialize_runtime_catalog: bool,
) -> Result<(), String> {
    let preview_paths = PreviewArchivePaths::from_paths_with_sidecar_entries(
        preview_worker::preview_archive_paths_for_catalog_projection(),
    );
    let mame_sqlite_path = default_mame_sqlite_path();
    let hbmame_sqlite_path = default_hbmame_sqlite_path();
    write_sqlite_scan_with_sources_inner(
        path,
        scan,
        SqliteScanSources {
            mame_sqlite_path: &mame_sqlite_path,
            hbmame_sqlite_path: &hbmame_sqlite_path,
            preview_paths: &preview_paths,
            software_hash_cache,
            discovery_history,
            stamp,
        },
        None,
        progress,
        canonical_navigation,
        canonical_catalog,
        materialize_runtime_catalog,
    )
    .map(|_| ())
}

#[cfg(test)]
pub(crate) fn write_sqlite_scan_with_mame(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
) -> Result<(), String> {
    write_sqlite_scan_with_sources(
        path,
        scan,
        SqliteScanSources {
            mame_sqlite_path,
            hbmame_sqlite_path: &PathBuf::new(),
            preview_paths: &PreviewArchivePaths::default(),
            software_hash_cache: SoftwareHashCache::load(path),
            discovery_history: DiscoveryHistory::load(path),
            stamp: None,
        },
        Path::new(arcade_catalog::DEFAULT_ARCADE_ROOT),
        None,
    )
    .map(|_| ())
}

#[cfg(test)]
pub(crate) fn write_sqlite_scan_with_mame_and_hbmame(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
) -> Result<(), String> {
    write_sqlite_scan_with_sources(
        path,
        scan,
        SqliteScanSources {
            mame_sqlite_path,
            hbmame_sqlite_path,
            preview_paths: &PreviewArchivePaths::default(),
            software_hash_cache: SoftwareHashCache::load(path),
            discovery_history: DiscoveryHistory::load(path),
            stamp: None,
        },
        Path::new(arcade_catalog::DEFAULT_ARCADE_ROOT),
        None,
    )
    .map(|_| ())
}

#[cfg(test)]
pub(crate) fn write_sqlite_scan_with_mame_and_preview_pack(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
    preview_asset_pack: &preview_worker::PreviewArchiveIndex,
) -> Result<(), String> {
    let preview_paths =
        PreviewArchivePaths::from_preview_indexes(std::slice::from_ref(preview_asset_pack));
    write_sqlite_scan_with_sources(
        path,
        scan,
        SqliteScanSources {
            mame_sqlite_path,
            hbmame_sqlite_path: &PathBuf::new(),
            preview_paths: &preview_paths,
            software_hash_cache: SoftwareHashCache::load(path),
            discovery_history: DiscoveryHistory::load(path),
            stamp: None,
        },
        Path::new(arcade_catalog::DEFAULT_ARCADE_ROOT),
        None,
    )
    .map(|_| ())
}

struct SqliteScanSources<'a> {
    mame_sqlite_path: &'a Path,
    hbmame_sqlite_path: &'a Path,
    preview_paths: &'a PreviewArchivePaths,
    software_hash_cache: SoftwareHashCache,
    discovery_history: Option<DiscoveryHistory>,
    stamp: Option<&'a catalog_stamp::CatalogStamp>,
}

#[cfg(test)]
fn write_sqlite_scan_with_sources(
    path: &Path,
    scan: &LibraryScan,
    sources: SqliteScanSources<'_>,
    root: &Path,
    progress: ProgressCallback<'_>,
) -> Result<LibraryCatalogLoad, String> {
    write_sqlite_scan_with_sources_inner(
        path,
        scan,
        sources,
        Some(root),
        progress,
        None,
        None,
        true,
    )?
    .ok_or_else(|| "saved catalog was not returned".to_string())
}

#[allow(clippy::too_many_arguments)]
fn write_sqlite_scan_with_sources_inner(
    path: &Path,
    scan: &LibraryScan,
    mut sources: SqliteScanSources<'_>,
    root: Option<&Path>,
    mut progress: ProgressCallback<'_>,
    canonical_navigation: Option<&[u8]>,
    canonical_catalog: Option<&ArcadeCatalog>,
    materialize_runtime_catalog: bool,
) -> Result<Option<LibraryCatalogLoad>, String> {
    let total_t = Instant::now();
    let mut conn = Connection::open(path).map_err(|e| format!("open sqlite: {e}"))?;
    register_sqlite_catalog_functions(&conn)
        .map_err(|e| format!("register sqlite catalog functions: {e}"))?;
    let schema_t = Instant::now();
    conn.execute_batch(
        r#"
        PRAGMA page_size=1024;
        PRAGMA journal_mode=OFF;
        PRAGMA synchronous=OFF;
        PRAGMA temp_store=MEMORY;
        PRAGMA locking_mode=EXCLUSIVE;
        CREATE TABLE profiles (
            profile_id TEXT PRIMARY KEY,
            system_id TEXT NOT NULL,
            title TEXT NOT NULL,
            core_name TEXT NOT NULL,
            core_path TEXT,
            source_kind TEXT NOT NULL,
            source_detail TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE systems (
            system_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            platform_kind TEXT NOT NULL CHECK(platform_kind IN ('arcade','console','handheld','computer','unknown')),
            classification_source TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE system_classification_diagnostics (
            ordinal INTEGER PRIMARY KEY,
            system_id TEXT NOT NULL,
            accepted_kind TEXT NOT NULL,
            accepted_source TEXT NOT NULL,
            rejected_kind TEXT NOT NULL,
            rejected_source TEXT NOT NULL,
            reason TEXT NOT NULL
        );
        CREATE TABLE catalog_audit (
            ordinal INTEGER PRIMARY KEY,
            core_id TEXT NOT NULL,
            core_path TEXT NOT NULL,
            expected_game_dir TEXT NOT NULL,
            extensions TEXT NOT NULL,
            mount_kind TEXT NOT NULL,
            source TEXT NOT NULL,
            catalog_status TEXT NOT NULL,
            reason TEXT NOT NULL,
            evidence_source TEXT NOT NULL,
            evidence_confidence TEXT NOT NULL,
            content_role TEXT NOT NULL,
            suppression_reason TEXT NOT NULL
        );
        CREATE TABLE game_rows (
            game_key_id INTEGER PRIMARY KEY,
            game_id_kind_string_id INTEGER NOT NULL,
            game_id_path_id INTEGER,
            game_id_text TEXT,
            title TEXT NOT NULL,
            system_string_id INTEGER NOT NULL,
            manufacturer_string_id INTEGER,
            genre_string_id INTEGER
        );
        CREATE TABLE game_detail_rows (
            game_key_id INTEGER PRIMARY KEY,
            year INTEGER,
            discovered_at_unix INTEGER
        ) WITHOUT ROWID;
        CREATE TABLE path_chunks (
            chunk_id INTEGER PRIMARY KEY,
            uncompressed_len INTEGER NOT NULL,
            bytes BLOB NOT NULL
        );
        CREATE TABLE path_values (
            path_id INTEGER PRIMARY KEY,
            chunk_id INTEGER NOT NULL,
            offset INTEGER NOT NULL,
            len INTEGER NOT NULL
        );
        CREATE TABLE string_values (
            string_id INTEGER PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE launch_target_rows (
            launch_id INTEGER PRIMARY KEY,
            profile_string_id INTEGER,
            launch_kind_string_id INTEGER NOT NULL,
            source_path_id INTEGER,
            launch_ref_kind_string_id INTEGER NOT NULL,
            launch_path_id INTEGER,
            launcher_path_id INTEGER,
            payload_path_id INTEGER,
            core_string_id INTEGER NOT NULL,
            hardware_string_id INTEGER NOT NULL,
            setname TEXT,
            parent TEXT,
            mount_kind_string_id INTEGER,
            mount_index INTEGER,
            delay_secs INTEGER,
            confidence_string_id INTEGER NOT NULL
        );
        CREATE TABLE prepared_launch_rows (
            launch_id INTEGER PRIMARY KEY,
            collection_id TEXT NOT NULL,
            launch_quality TEXT NOT NULL,
            adapter_version INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE prepared_launch_diagnostic_rows (
            launch_id INTEGER PRIMARY KEY,
            collection_id TEXT NOT NULL,
            status TEXT NOT NULL,
            reason TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE VIEW launch_targets AS
            SELECT launch_target_rows.launch_id,
                   launch_target_rows.launch_id AS game_key_id,
                   profile_values.value AS profile_id,
                   launch_kind_values.value AS launch_kind,
                   COALESCE(
                       launch_target_rows.source_path_id,
                       CASE launch_ref_kind_values.value
                           WHEN 'payload' THEN launch_target_rows.payload_path_id
                           WHEN 'archive' THEN launch_target_rows.source_path_id
                           WHEN 'same-payload' THEN launch_target_rows.source_path_id
                           ELSE launch_target_rows.launch_path_id
                       END
                   ) AS source_path_id,
                   launch_ref_kind_values.value AS launch_ref_kind,
                   launch_target_rows.launch_path_id,
                   COALESCE(launch_target_rows.launcher_path_id, launch_target_rows.launch_path_id) AS launcher_path_id,
                   launch_target_rows.payload_path_id,
                   core_values.value AS core_id,
                   hardware_values.value AS hardware_id,
                   launch_target_rows.setname,
                   launch_target_rows.parent,
                   mount_kind_values.value AS mount_kind,
                   launch_target_rows.mount_index,
                   launch_target_rows.delay_secs,
                   confidence_values.value AS confidence
            FROM launch_target_rows
            LEFT JOIN string_values profile_values
                   ON profile_values.string_id = launch_target_rows.profile_string_id
            JOIN string_values launch_kind_values
                   ON launch_kind_values.string_id = launch_target_rows.launch_kind_string_id
            JOIN string_values launch_ref_kind_values
                   ON launch_ref_kind_values.string_id = launch_target_rows.launch_ref_kind_string_id
            JOIN string_values core_values
                   ON core_values.string_id = launch_target_rows.core_string_id
            JOIN string_values hardware_values
                   ON hardware_values.string_id = launch_target_rows.hardware_string_id
            LEFT JOIN string_values mount_kind_values
                   ON mount_kind_values.string_id = launch_target_rows.mount_kind_string_id
            JOIN string_values confidence_values
                   ON confidence_values.string_id = launch_target_rows.confidence_string_id;
        CREATE VIEW path_values_text AS
            SELECT path_values.path_id AS path_id,
                   magik_path(
                       path_values.chunk_id,
                       path_values.offset,
                       path_values.len,
                       path_chunks.uncompressed_len,
                       path_chunks.bytes
                   ) AS path
            FROM path_values
            JOIN path_chunks ON path_chunks.chunk_id = path_values.chunk_id;
        CREATE VIEW games AS
            SELECT game_rows.game_key_id,
                   CASE game_id_kind_values.value
                       WHEN 'payload' THEN 'payload:' || game_id_paths.path
                       WHEN 'archive' THEN 'archive:' || game_id_paths.path
                       ELSE game_rows.game_id_text
                   END AS game_id,
                   game_rows.title,
                   lower(game_rows.title) AS sort_title,
                   system_values.value AS system_id,
                   manufacturer_values.value AS manufacturer,
                   genre_values.value AS genre,
                   game_detail_rows.year,
                   game_detail_rows.discovered_at_unix
            FROM game_rows
            JOIN string_values game_id_kind_values
                 ON game_id_kind_values.string_id = game_rows.game_id_kind_string_id
            LEFT JOIN path_values_text game_id_paths
                 ON game_id_paths.path_id = game_rows.game_id_path_id
            JOIN string_values system_values
                 ON system_values.string_id = game_rows.system_string_id
            LEFT JOIN string_values manufacturer_values
                 ON manufacturer_values.string_id = game_rows.manufacturer_string_id
            LEFT JOIN string_values genre_values
                 ON genre_values.string_id = game_rows.genre_string_id
            LEFT JOIN game_detail_rows
                 ON game_detail_rows.game_key_id = game_rows.game_key_id;
        CREATE VIEW launch_plans AS
            WITH expanded AS (
                SELECT lt.*,
                       source_paths.path AS source_path,
                       CASE lt.launch_ref_kind
                           WHEN 'payload' THEN 'magik-plan:payload:' || payload_paths.path
                           WHEN 'archive' THEN 'magik-plan:archive:' || payload_paths.path
                           WHEN 'same-payload' THEN payload_paths.path
                           ELSE launch_paths.path
                       END AS launch_ref,
                       launcher_paths.path AS launcher_path,
                       payload_paths.path AS payload_path
                FROM launch_targets lt
                JOIN path_values_text source_paths ON source_paths.path_id = lt.source_path_id
                LEFT JOIN path_values_text launch_paths ON launch_paths.path_id = lt.launch_path_id
                LEFT JOIN path_values_text launcher_paths ON launcher_paths.path_id = lt.launcher_path_id
                LEFT JOIN path_values_text payload_paths ON payload_paths.path_id = lt.payload_path_id
            )
            SELECT 'plan:' || games.game_id AS plan_id,
                   expanded.launch_id,
                   games.game_id,
                   expanded.profile_id,
                   expanded.launch_kind,
                   expanded.source_path,
                   expanded.launch_ref,
                   expanded.launcher_path,
                   expanded.payload_path,
                   expanded.core_id,
                   expanded.hardware_id,
                   expanded.setname,
                   expanded.parent,
                   expanded.confidence
            FROM expanded
            JOIN games ON games.game_key_id = expanded.game_key_id;
        CREATE VIEW launchables AS
            SELECT games.game_id AS launchable_id,
                   lt.launch_id AS launch_id,
                   games.title AS title,
                   games.system_id AS system_id,
                   lt.launch_kind AS launch_kind,
                   source_paths.path AS source_path,
                   CASE lt.launch_ref_kind
                       WHEN 'payload' THEN 'magik-plan:payload:' || payload_paths.path
                       WHEN 'archive' THEN 'magik-plan:archive:' || payload_paths.path
                       WHEN 'same-payload' THEN payload_paths.path
                       ELSE launch_paths.path
                   END AS launch_ref,
                   lt.setname AS setname,
                   lt.core_id AS core_id,
                   lt.hardware_id AS hardware_id,
                   lt.confidence AS confidence
            FROM launch_targets lt
            JOIN games ON games.game_key_id = lt.launch_id
            JOIN path_values_text source_paths ON source_paths.path_id = lt.source_path_id
            LEFT JOIN path_values_text launch_paths ON launch_paths.path_id = lt.launch_path_id
            LEFT JOIN path_values_text payload_paths ON payload_paths.path_id = lt.payload_path_id;
        CREATE VIEW prepared_launches AS
            SELECT prepared_launch_rows.launch_id,
                   games.game_id,
                   games.title,
                   games.system_id,
                   launch_plans.launch_ref,
                   prepared_launch_rows.collection_id,
                   prepared_launch_rows.launch_quality,
                   prepared_launch_rows.adapter_version
            FROM prepared_launch_rows
            JOIN games ON games.game_key_id = prepared_launch_rows.launch_id
            JOIN launch_plans ON launch_plans.launch_id = prepared_launch_rows.launch_id;
        CREATE VIEW launch_provenance AS
            SELECT launch_target_rows.launch_id,
                   COALESCE(prepared_launch_rows.collection_id, '') AS collection_id,
                   COALESCE(prepared_launch_rows.launch_quality, 'generic') AS launch_quality,
                   COALESCE(prepared_launch_rows.adapter_version, 0) AS adapter_version
            FROM launch_target_rows
            LEFT JOIN prepared_launch_rows
                   ON prepared_launch_rows.launch_id = launch_target_rows.launch_id;
        CREATE VIEW prepared_launch_diagnostics AS
            SELECT prepared_launch_diagnostic_rows.launch_id,
                   games.game_id,
                   games.title,
                   games.system_id,
                   prepared_launch_diagnostic_rows.collection_id,
                   prepared_launch_diagnostic_rows.status,
                   prepared_launch_diagnostic_rows.reason
            FROM prepared_launch_diagnostic_rows
            JOIN games ON games.game_key_id = prepared_launch_diagnostic_rows.launch_id;
        CREATE VIEW launch_plans_text AS SELECT * FROM launch_plans;
        CREATE TABLE launchable_identity_rows (
            game_key_id INTEGER NOT NULL,
            namespace_string_id INTEGER NOT NULL,
            identity_string_id INTEGER NOT NULL,
            family_string_id INTEGER,
            metadata_title_string_id INTEGER,
            year_string_id INTEGER,
            manufacturer_string_id INTEGER,
            category_string_id INTEGER,
            players INTEGER,
            control_string_id INTEGER,
            source_string_id INTEGER NOT NULL,
            PRIMARY KEY(game_key_id, namespace_string_id, identity_string_id)
        ) WITHOUT ROWID;
        CREATE VIEW launchable_identities AS
            SELECT launchable_identity_rows.game_key_id,
                   games.game_id AS launchable_id,
                   namespace_values.value AS namespace,
                   identity_values.value AS identity_id,
                   family_values.value AS family_id,
                   metadata_title_values.value AS metadata_title,
                   year_values.value AS year,
                   manufacturer_values.value AS manufacturer,
                   category_values.value AS category,
                   launchable_identity_rows.players,
                   control_values.value AS control,
                   source_values.value AS source
            FROM launchable_identity_rows
            JOIN games ON games.game_key_id = launchable_identity_rows.game_key_id
            JOIN string_values namespace_values
                 ON namespace_values.string_id = launchable_identity_rows.namespace_string_id
            JOIN string_values identity_values
                 ON identity_values.string_id = launchable_identity_rows.identity_string_id
            LEFT JOIN string_values family_values
                 ON family_values.string_id = launchable_identity_rows.family_string_id
            LEFT JOIN string_values metadata_title_values
                 ON metadata_title_values.string_id = launchable_identity_rows.metadata_title_string_id
            LEFT JOIN string_values year_values
                 ON year_values.string_id = launchable_identity_rows.year_string_id
            LEFT JOIN string_values manufacturer_values
                 ON manufacturer_values.string_id = launchable_identity_rows.manufacturer_string_id
            LEFT JOIN string_values category_values
                 ON category_values.string_id = launchable_identity_rows.category_string_id
            LEFT JOIN string_values control_values
                 ON control_values.string_id = launchable_identity_rows.control_string_id
            JOIN string_values source_values
                 ON source_values.string_id = launchable_identity_rows.source_string_id;
        CREATE TABLE ui_arcade_preferred (
            ordinal INTEGER PRIMARY KEY,
            family_id TEXT NOT NULL,
            variant_ordinal INTEGER NOT NULL,
            UNIQUE(family_id, variant_ordinal)
        );
        CREATE TABLE ui_arcade_variants (
            family_id TEXT NOT NULL,
            variant_ordinal INTEGER NOT NULL,
            launch_id INTEGER NOT NULL,
            preview_asset_key TEXT NOT NULL,
            has_preview INTEGER NOT NULL,
            asset_link_reason TEXT NOT NULL,
            preferred INTEGER NOT NULL,
            preferred_reason TEXT NOT NULL,
            PRIMARY KEY(family_id, variant_ordinal)
        ) WITHOUT ROWID;
        CREATE TABLE launcher_catalog_rows (
            ordinal INTEGER PRIMARY KEY,
            launch_id INTEGER NOT NULL,
            preview_asset_key TEXT NOT NULL,
            has_preview INTEGER NOT NULL
        );
        CREATE VIEW launcher_catalog AS
            SELECT ui_arcade_preferred_text.ordinal,
                   ui_arcade_preferred_text.launch_id,
                   ui_arcade_preferred_text.title,
                   ui_arcade_preferred_text.sort_title,
                   ui_arcade_preferred_text.preview_asset_key,
                   ui_arcade_preferred_text.has_preview,
                   ui_arcade_preferred_text.system_id,
                   ui_arcade_preferred_text.year,
                   ui_arcade_preferred_text.manufacturer,
                   ui_arcade_preferred_text.category,
                   ui_arcade_preferred_text.players,
                   ui_arcade_preferred_text.control,
                   ui_arcade_preferred_text.discovered_at_unix
            FROM ui_arcade_preferred_text
            UNION ALL
            SELECT launcher_catalog_rows.ordinal,
                   launcher_catalog_rows.launch_id,
                   game_rows.title,
                   lower(game_rows.title) AS sort_title,
                   launcher_catalog_rows.preview_asset_key,
                   launcher_catalog_rows.has_preview,
                   games.system_id,
                   game_detail_rows.year,
                   games.manufacturer,
                   NULL AS category,
                   NULL AS players,
                   NULL AS control,
                   game_detail_rows.discovered_at_unix
            FROM launcher_catalog_rows
            JOIN launch_target_rows ON launch_target_rows.launch_id = launcher_catalog_rows.launch_id
            JOIN game_rows ON game_rows.game_key_id = launch_target_rows.launch_id
            JOIN games ON games.game_key_id = launch_target_rows.launch_id
            LEFT JOIN game_detail_rows ON game_detail_rows.game_key_id = launch_target_rows.launch_id;
        CREATE VIEW launcher_launch_plans AS
            SELECT launcher_catalog.launch_id,
                   launcher_catalog.title,
                   launcher_catalog.system_id,
                   COALESCE(profiles.core_path, core_values.value) AS core_path,
                   COALESCE(mount_kind_values.value, 'mount-image') AS mount_kind,
                   COALESCE(launch_target_rows.mount_index, 0) AS mount_index,
                   COALESCE(launch_target_rows.delay_secs, 1) AS delay_secs
            FROM launcher_catalog
            JOIN launch_target_rows ON launch_target_rows.launch_id = launcher_catalog.launch_id
            JOIN string_values launch_kind_values
                 ON launch_kind_values.string_id = launch_target_rows.launch_kind_string_id
            JOIN string_values core_values
                 ON core_values.string_id = launch_target_rows.core_string_id
            LEFT JOIN string_values profile_values
                 ON profile_values.string_id = launch_target_rows.profile_string_id
            LEFT JOIN string_values mount_kind_values
                 ON mount_kind_values.string_id = launch_target_rows.mount_kind_string_id
            LEFT JOIN profiles ON profiles.profile_id = profile_values.value
            WHERE launch_kind_values.value = 'virtual-mgl';
        CREATE VIEW ui_arcade_variants_text AS
            SELECT ui_arcade_variants.family_id,
                   ui_arcade_variants.variant_ordinal,
                   games.game_id AS launchable_id,
                   ui_arcade_variants.launch_id,
                   game_rows.title,
                   lower(game_rows.title) AS sort_title,
                   ui_arcade_variants.preview_asset_key,
                   ui_arcade_variants.has_preview,
                   games.system_id,
                   COALESCE(i.year, game_detail_rows.year) AS year,
                   COALESCE(i.manufacturer, games.manufacturer) AS manufacturer,
                   i.category,
                   i.players,
                   i.control,
                   game_detail_rows.discovered_at_unix,
                   i.identity_id,
                   CASE
                       WHEN i.identity_id IS NOT NULL
                        AND i.family_id IS NOT NULL
                        AND i.identity_id != i.family_id
                       THEN i.family_id
                       ELSE NULL
                   END AS parent_setname,
                   ui_arcade_variants.preview_asset_key AS asset_key,
                   ui_arcade_variants.asset_link_reason,
                   ui_arcade_variants.preferred,
                   ui_arcade_variants.preferred_reason,
                   launch_plans.launch_ref AS launch_ref
            FROM ui_arcade_variants
            JOIN launch_target_rows lt ON lt.launch_id = ui_arcade_variants.launch_id
            JOIN game_rows ON game_rows.game_key_id = lt.launch_id
            JOIN games ON games.game_key_id = lt.launch_id
            LEFT JOIN launchable_identities i
              ON i.game_key_id = lt.launch_id
             AND i.namespace = 'mame'
            LEFT JOIN game_detail_rows
              ON game_detail_rows.game_key_id = lt.launch_id
            JOIN launch_plans ON launch_plans.launch_id = ui_arcade_variants.launch_id;
        CREATE VIEW ui_arcade_preferred_text AS
            SELECT ui_arcade_preferred.ordinal,
                   ui_arcade_variants_text.launchable_id,
                   ui_arcade_variants_text.launch_id,
                   ui_arcade_variants_text.title,
                   ui_arcade_variants_text.sort_title,
                   ui_arcade_variants_text.preview_asset_key,
                   ui_arcade_variants_text.has_preview,
                   ui_arcade_variants_text.system_id,
                   ui_arcade_variants_text.year,
                   ui_arcade_variants_text.manufacturer,
                   ui_arcade_variants_text.category,
                   ui_arcade_variants_text.players,
                   ui_arcade_variants_text.control,
                   ui_arcade_variants_text.discovered_at_unix,
                   ui_arcade_variants_text.identity_id,
                   ui_arcade_variants_text.family_id,
                   ui_arcade_variants_text.parent_setname,
                   ui_arcade_variants_text.asset_key,
                   ui_arcade_variants_text.asset_link_reason,
                   ui_arcade_variants_text.preferred_reason,
                   ui_arcade_variants_text.launch_ref
            FROM ui_arcade_preferred
            JOIN ui_arcade_variants_text
              ON ui_arcade_variants_text.family_id = ui_arcade_preferred.family_id
             AND ui_arcade_variants_text.variant_ordinal = ui_arcade_preferred.variant_ordinal;
        CREATE VIEW launcher_catalog_text AS
            SELECT launcher_catalog.*,
                   CASE launch_ref_kind_values.value
                       WHEN 'payload' THEN 'magik-plan:payload:' || payload_paths.path
                       WHEN 'archive' THEN 'magik-plan:archive:' || payload_paths.path
                       WHEN 'same-payload' THEN payload_paths.path
                       ELSE launch_paths.path
                   END AS launch_ref
            FROM launcher_catalog
            JOIN launch_target_rows ON launch_target_rows.launch_id = launcher_catalog.launch_id
            JOIN string_values launch_ref_kind_values
                 ON launch_ref_kind_values.string_id = launch_target_rows.launch_ref_kind_string_id
            LEFT JOIN path_values_text launch_paths
                 ON launch_paths.path_id = launch_target_rows.launch_path_id
            LEFT JOIN path_values_text payload_paths
                 ON payload_paths.path_id = launch_target_rows.payload_path_id;
        CREATE VIEW launcher_launch_plans_text AS
            SELECT launcher_launch_plans.*,
                   CASE launch_ref_kind_values.value
                       WHEN 'payload' THEN 'magik-plan:payload:' || payload_paths.path
                       WHEN 'archive' THEN 'magik-plan:archive:' || payload_paths.path
                       WHEN 'same-payload' THEN payload_paths.path
                       ELSE launch_paths.path
                   END AS launch_ref,
                   COALESCE(payload_paths.path, '') AS payload_path
            FROM launcher_launch_plans
            JOIN launch_target_rows ON launch_target_rows.launch_id = launcher_launch_plans.launch_id
            JOIN string_values launch_ref_kind_values
                 ON launch_ref_kind_values.string_id = launch_target_rows.launch_ref_kind_string_id
            LEFT JOIN path_values_text launch_paths
                 ON launch_paths.path_id = launch_target_rows.launch_path_id
            LEFT JOIN path_values_text payload_paths
                 ON payload_paths.path_id = launch_target_rows.payload_path_id;
        CREATE TABLE region_metadata_rows (
            game_key_id INTEGER PRIMARY KEY,
            inferred_region TEXT,
            confidence TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE VIEW region_metadata AS
            SELECT games.game_id AS game_id,
                   region_metadata_rows.inferred_region,
                   COALESCE(region_metadata_rows.confidence, 'unknown') AS confidence
            FROM games
            LEFT JOIN region_metadata_rows
                   ON region_metadata_rows.game_key_id = games.game_key_id;
        CREATE TABLE meta (
            key TEXT PRIMARY KEY,
            value INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE software_hash_cache (
            list_name TEXT NOT NULL,
            file_path TEXT NOT NULL,
            size INTEGER NOT NULL,
            mtime_secs INTEGER NOT NULL,
            software_name TEXT,
            PRIMARY KEY(list_name, file_path, size, mtime_secs)
        ) WITHOUT ROWID;
        CREATE TABLE catalog_stamp (
            id INTEGER PRIMARY KEY CHECK (id=0),
            bytes BLOB NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE catalog_discovery_checkpoint (
            id INTEGER PRIMARY KEY CHECK (id=0),
            bytes BLOB NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE catalog_navigation_projection (
            id INTEGER PRIMARY KEY CHECK (id=0),
            bytes BLOB NOT NULL
        ) WITHOUT ROWID;
        "#,
    )
    .map_err(|e| format!("create sqlite schema: {e}"))?;
    report_library_import_timing("schema", schema_t, "tables=15");

    let metadata_t = Instant::now();
    let mame_signature = library_db::file_signature(sources.mame_sqlite_path);
    let hbmame_signature = library_db::file_signature(sources.hbmame_sqlite_path);
    let covered_payloads = covered_payload_paths(&scan.discoveries);
    let discoveries = preferred_playable_discoveries_by_key(&scan.discoveries, &covered_payloads);
    let discovery_total = discoveries.len();
    let arcade_setnames = arcade_metadata_setnames(discoveries.values().copied());
    let software_metadata = load_mame_software_metadata(sources.mame_sqlite_path);
    let arcade_metadata = load_arcade_machine_metadata_for_setnames(
        sources.mame_sqlite_path,
        sources.hbmame_sqlite_path,
        &arcade_setnames,
    );
    report_library_import_timing(
        "metadata_load",
        metadata_t,
        format!(
            "mame={} hbmame={} mame_needed={} software_lists={} preview_paths={}",
            arcade_metadata.mame.len(),
            arcade_metadata.hbmame.len(),
            arcade_setnames.len(),
            software_metadata.items.len(),
            sources.preview_paths.len()
        ),
    );
    let tx_t = Instant::now();
    let tx = conn
        .transaction()
        .map_err(|e| format!("begin sqlite tx: {e}"))?;
    report_library_import_timing("begin_tx", tx_t, "");
    let mut path_interner = SqlitePathInterner::default();
    let mut string_interner = SqliteStringInterner::default();
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare(
                "INSERT INTO profiles(profile_id,system_id,title,core_name,core_path,source_kind,source_detail)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| format!("prepare profile insert: {e}"))?;
        for profile in &scan.profiles {
            stmt.execute(params![
                profile.id.as_str(),
                profile.system_id.as_str(),
                profile.title.as_str(),
                profile.core_name.as_str(),
                profile.core_path.as_deref(),
                source_kind_name(profile.provenance.kind),
                profile.provenance.detail.as_str()
            ])
            .map_err(|e| format!("insert profile: {e}"))?;
        }
        report_library_import_timing(
            "insert_profiles",
            stage_t,
            format!("rows={}", scan.profiles.len()),
        );
    }
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO system_classification_diagnostics(ordinal,system_id,accepted_kind,accepted_source,rejected_kind,rejected_source,reason)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| format!("prepare system classification diagnostic insert: {e}"))?;
        let mut ordinal = 0_i64;
        let mut seen = HashSet::new();
        for profile in &scan.profiles {
            let profile_core = crate::catalog_discovery::compact_system_name(&profile.core_name);
            for core in scan.installed_cores.iter().filter(|core| {
                crate::catalog_discovery::compact_system_name(&core.core_id) == profile_core
            }) {
                let observed = core_location_category(core.path.to_str());
                let resolution = crate::catalog_classify::classify_system(
                    &profile.system_id,
                    observed,
                    "core-location",
                );
                if let Some(diagnostic) = resolution.diagnostic {
                    let key = (
                        diagnostic.system_id.clone(),
                        diagnostic.rejected_kind,
                        diagnostic.rejected_source.clone(),
                    );
                    if !seen.insert(key) {
                        continue;
                    }
                    stmt.execute(params![
                        ordinal,
                        diagnostic.system_id,
                        diagnostic.accepted_kind.as_str(),
                        diagnostic.accepted_source,
                        diagnostic.rejected_kind.as_str(),
                        diagnostic.rejected_source,
                        diagnostic.reason,
                    ])
                    .map_err(|e| format!("insert system classification diagnostic: {e}"))?;
                    ordinal += 1;
                }
            }
        }
    }
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare(
                "INSERT INTO catalog_audit(ordinal,core_id,core_path,expected_game_dir,extensions,mount_kind,source,catalog_status,reason,evidence_source,evidence_confidence,content_role,suppression_reason)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            )
            .map_err(|e| format!("prepare catalog audit insert: {e}"))?;
        for (idx, row) in scan.audit_rows.iter().enumerate() {
            stmt.execute(params![
                idx as i64,
                row.core_id.as_str(),
                row.core_path.as_str(),
                row.expected_game_dir.as_str(),
                row.extensions.as_str(),
                row.mount_kind.as_str(),
                row.source.as_str(),
                row.catalog_status.as_str(),
                row.reason.as_str(),
                row.evidence_source(),
                row.evidence_confidence(),
                row.content_role(),
                row.suppression_reason()
            ])
            .map_err(|e| format!("insert catalog audit: {e}"))?;
        }
        report_library_import_timing(
            "insert_catalog_audit",
            stage_t,
            format!("rows={}", scan.audit_rows.len()),
        );
    }
    {
        let stage_t = Instant::now();
        let mut launcher_rows = Vec::<CatalogProjectionRow>::new();
        let mut arcade_compatibility_rows = Vec::<ArcadeCompatibilityRow>::new();
        let canonical_launch_refs = canonical_catalog.map(|catalog| {
            catalog
                .games
                .iter()
                .map(|game| game.mra_path.as_ref())
                .collect::<HashSet<_>>()
        });
        let canonical_preview_flags = canonical_catalog.map(|catalog| {
            catalog
                .games
                .iter()
                .map(|game| {
                    (
                        (
                            game.mra_path.to_string(),
                            game.title.to_string(),
                            game.system_id.to_string(),
                        ),
                        game.has_preview,
                    )
                })
                .collect::<HashMap<_, _>>()
        });
        let mut canonical_launch_ids = CanonicalLaunchIdIndex::default();
        let mut system_stmt = tx
            .prepare("INSERT OR IGNORE INTO systems(system_id,title,platform_kind,classification_source) VALUES (?1,?2,?3,?4)")
            .map_err(|e| format!("prepare system insert: {e}"))?;
        let mut game_stmt = tx
            .prepare(
                "INSERT INTO game_rows(game_key_id,game_id_kind_string_id,game_id_path_id,game_id_text,title,system_string_id,manufacturer_string_id,genre_string_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(|e| format!("prepare game insert: {e}"))?;
        let mut game_detail_stmt = tx
            .prepare(
                "INSERT INTO game_detail_rows(game_key_id,year,discovered_at_unix)
                 VALUES (?1,?2,?3)",
            )
            .map_err(|e| format!("prepare game detail insert: {e}"))?;
        let mut target_stmt = tx
            .prepare(
                "INSERT INTO launch_target_rows(launch_id,profile_string_id,launch_kind_string_id,source_path_id,launch_ref_kind_string_id,launch_path_id,launcher_path_id,payload_path_id,core_string_id,hardware_string_id,setname,parent,mount_kind_string_id,mount_index,delay_secs,confidence_string_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            )
            .map_err(|e| format!("prepare launch target insert: {e}"))?;
        let mut prepared_stmt = tx
            .prepare(
                "INSERT INTO prepared_launch_rows(launch_id,collection_id,launch_quality,adapter_version)
                 VALUES (?1,?2,?3,?4)",
            )
            .map_err(|e| format!("prepare prepared launch insert: {e}"))?;
        let mut prepared_diagnostic_stmt = tx
            .prepare(
                "INSERT INTO prepared_launch_diagnostic_rows(launch_id,collection_id,status,reason)
                 VALUES (?1,?2,?3,?4)",
            )
            .map_err(|e| format!("prepare prepared launch diagnostic insert: {e}"))?;
        let mut identity_stmt = tx
            .prepare(
                "INSERT INTO launchable_identity_rows(game_key_id,namespace_string_id,identity_string_id,family_string_id,metadata_title_string_id,year_string_id,manufacturer_string_id,category_string_id,players,control_string_id,source_string_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            )
            .map_err(|e| format!("prepare launchable identity insert: {e}"))?;
        let mut region_stmt = tx
            .prepare(
                "INSERT INTO region_metadata_rows(game_key_id,inferred_region,confidence)
                 VALUES (?1,?2,?3)",
            )
            .map_err(|e| format!("prepare region metadata insert: {e}"))?;
        let mut playable_counts = HashMap::<String, usize>::new();
        let manifest_backed_systems = scan
            .profiles
            .iter()
            .filter(|profile| profile.provenance.kind != RuleSourceKind::ConfStr)
            .map(|profile| profile.system_id.clone())
            .collect::<HashSet<_>>();
        for discovery in discoveries.values() {
            if is_raw_arcade_zip_set_discovery(discovery) {
                continue;
            }
            let system_id = catalog_system_id_for_discovery(discovery);
            *playable_counts.entry(system_id.clone()).or_default() += 1;
        }
        let promoted_systems = playable_counts
            .iter()
            .filter_map(|(system_id, count)| {
                (*count >= 2
                    || manifest_backed_systems.contains(system_id)
                    || matches!(system_id.as_str(), "arcade" | "neogeo"))
                .then_some(system_id.clone())
            })
            .collect::<HashSet<_>>();
        let mut system_rows = HashMap::<String, (String, String, String)>::new();
        for discovery in discoveries.values() {
            let system_id = catalog_system_id_for_discovery(discovery);
            if !promoted_systems.contains(&system_id) {
                continue;
            }
            system_rows.entry(system_id.clone()).or_insert_with(|| {
                let classification = crate::catalog_classify::classify_system(
                    &system_id,
                    None,
                    "catalog-association",
                )
                .classification;
                (
                    crate::catalog_classify::system_title(&system_id),
                    classification.platform_kind.as_str().to_string(),
                    classification.source,
                )
            });
        }
        for (system_id, (title, platform_kind, classification_source)) in system_rows {
            system_stmt
                .execute(params![
                    system_id.as_str(),
                    title.as_str(),
                    platform_kind.as_str(),
                    classification_source.as_str()
                ])
                .map_err(|e| format!("insert system: {e}"))?;
        }
        report_sqlite_import_progress(&mut progress, 0, discovery_total);
        let mut chunk_t = Instant::now();
        let mut chunk_start = 0usize;
        for (idx, (key, discovery)) in discoveries.into_iter().enumerate() {
            if is_raw_arcade_zip_set_discovery(discovery) {
                continue;
            }
            let game_key_id = (idx + 1) as i64;
            let launch_id = game_key_id;
            if idx > 0 && idx % 250 == 0 {
                report_sqlite_import_progress(&mut progress, idx, discovery_total);
            }
            let system_id = catalog_system_id_for_discovery(discovery);
            let discovered_at_unix = sources
                .discovery_history
                .as_ref()
                .and_then(|history| history.discovered_at_for(&key, scan));
            let software_identity = mame_software_identity_for_discovery(
                discovery,
                &software_metadata,
                &mut sources.software_hash_cache,
            );
            let arcade_identity_id = mame_identity_for_discovery(discovery);
            let mister_arcade = arcade_identity_id.as_deref().and_then(|identity_id| {
                mister_arcade_metadata_for_discovery(&arcade_metadata, discovery, identity_id)
            });
            let preview_asset = software_identity
                .as_ref()
                .and_then(|identity| console_preview_asset(identity, sources.preview_paths));
            let game_id_storage = game_id_storage_for(&key, discovery);
            game_stmt
                .execute(params![
                    game_key_id,
                    string_interner.intern(game_id_storage.kind),
                    game_id_storage.path.map(|path| path_interner.intern(path)),
                    game_id_storage.text,
                    mister_arcade
                        .filter(|metadata| !metadata.title.is_empty())
                        .map(|metadata| metadata.title.as_str())
                        .unwrap_or(discovery.title.as_str()),
                    string_interner.intern(system_id.as_str()),
                    string_interner.intern_optional(
                        mister_arcade
                            .filter(|metadata| !metadata.manufacturer.is_empty())
                            .map(|metadata| metadata.manufacturer.as_str())
                            .or(discovery.manufacturer.as_deref())
                    ),
                    string_interner.intern_optional(
                        mister_arcade
                            .filter(|metadata| !metadata.category.is_empty())
                            .map(|metadata| metadata.category.as_str())
                            .or(discovery.genre.as_deref())
                    )
                ])
                .map_err(|e| format!("insert game: {e}"))?;
            if discovery.year.is_some() || discovered_at_unix.is_some() {
                game_detail_stmt
                    .execute(params![
                        game_key_id,
                        discovery.year.map(|n| n as i64),
                        discovered_at_unix
                    ])
                    .map_err(|e| format!("insert game detail: {e}"))?;
            }
            let launcher_path = match discovery.source_kind {
                DiscoverySourceKind::Mra | DiscoverySourceKind::Mgl => {
                    Some(discovery.launch_ref.as_str())
                }
                DiscoverySourceKind::PayloadFile
                | DiscoverySourceKind::ArchiveEntry
                | DiscoverySourceKind::CatalogEntry => None,
            };
            let payload_path = if launcher_path.is_none() {
                Some(discovery.launch_ref.as_str())
            } else {
                None
            };
            let plan_launch_ref = launch_ref_for_discovery(&key, discovery);
            if canonical_catalog.is_none()
                && is_launcher_launch_ref(&plan_launch_ref)
                && system_id != "arcade"
                && system_id != "neogeo"
                && promoted_systems.contains(&system_id)
            {
                let software_family_key = software_identity
                    .as_ref()
                    .map(|identity| format!("mame-software:{}", identity.family_id));
                launcher_rows.push(
                    CatalogProjectionRow::new(
                        discovery.title.clone(),
                        plan_launch_ref.clone(),
                        system_id.clone(),
                        LauncherPreviewAsset::from_console_asset(preview_asset.as_ref()),
                        ArcadeGameMetadataKey {
                            year: discovery.year,
                            manufacturer: discovery.manufacturer.clone().unwrap_or_default(),
                            category: discovery.category.clone(),
                            players: None,
                            control: String::new(),
                        },
                        false,
                        CatalogProjectionSource {
                            source_kind: launch_kind_for_discovery(discovery).to_string(),
                            setname: discovery.setname.clone().unwrap_or_default(),
                            parent: discovery.parent.clone().unwrap_or_default(),
                            family_key: software_family_key,
                            identity_matched: software_identity.is_some(),
                            prepared: discovery.prepared,
                        },
                    )
                    .with_launch_id(launch_id),
                );
            }
            let payload_path_id = path_interner.intern_optional(payload_path);
            let launch_ref_storage = launch_ref_storage_for(
                plan_launch_ref.as_str(),
                payload_path,
                launch_kind_for_discovery(discovery),
            );
            let launch_path_id = path_interner.intern_optional(launch_ref_storage.path);
            let source_path_id = if Some(discovery.source_path.as_str()) == payload_path
                || Some(discovery.source_path.as_str()) == launch_ref_storage.path
            {
                None
            } else {
                Some(path_interner.intern(discovery.source_path.as_str()))
            };
            let launcher_path_id = if launcher_path == launch_ref_storage.path {
                None
            } else {
                path_interner.intern_optional(launcher_path)
            };
            let owning_profile = crate::catalog_scan::profile_for_path(
                &scan.profiles,
                Path::new(discovery.source_path.as_str()),
            );
            let profile_id = owning_profile
                .map(|profile| profile.id.as_str())
                .or_else(|| profile_id_for_discovery(discovery));
            let mount = launch_target_mount_for_discovery(discovery, profile_id, &scan.profiles);
            let mount_kind = mount.map(|mount| mount_kind_str(mount.kind));
            target_stmt
                .execute(params![
                    launch_id,
                    string_interner.intern_optional(profile_id),
                    string_interner.intern(launch_kind_for_discovery(discovery)),
                    source_path_id,
                    string_interner.intern(launch_ref_storage.kind),
                    launch_path_id,
                    launcher_path_id,
                    payload_path_id,
                    string_interner.intern(discovery.core_id.as_str()),
                    string_interner.intern(discovery.hardware_id.as_str()),
                    discovery.setname.as_deref(),
                    discovery.parent.as_deref(),
                    string_interner.intern_optional(mount_kind),
                    mount.map(|mount| mount.index as i64),
                    mount.map(|mount| mount.delay_secs as i64),
                    string_interner.intern(confidence_str(discovery.confidence))
                ])
                .map_err(|e| format!("insert launch target: {e}"))?;
            if let Some(prepared) = discovery.prepared {
                prepared_stmt
                    .execute(params![
                        launch_id,
                        prepared.collection_id.as_str(),
                        prepared.launch_quality.as_str(),
                        prepared.adapter_version as i64
                    ])
                    .map_err(|e| format!("insert prepared launch: {e}"))?;
            } else if let Some(diagnostic) = crate::prepared_collections::diagnostic_for_candidate(
                Path::new(&discovery.source_path),
                &discovery.platform_id,
            ) {
                prepared_diagnostic_stmt
                    .execute(params![
                        launch_id,
                        diagnostic.collection_id.as_str(),
                        diagnostic.status,
                        diagnostic.reason
                    ])
                    .map_err(|e| format!("insert prepared launch diagnostic: {e}"))?;
            }
            let mut arcade_identity = None;
            if let Some(identity_id) = arcade_identity_id {
                let (family_id, title, year, manufacturer, players, control, source) =
                    mame_identity_projection(
                        &identity_id,
                        &arcade_metadata,
                        discovery.parent.as_deref(),
                        &discovery.title,
                    );
                identity_stmt
                    .execute(params![
                        game_key_id,
                        string_interner.intern("mame"),
                        string_interner.intern(identity_id.as_str()),
                        string_interner.intern_optional(Some(family_id.as_str())),
                        string_interner.intern_optional(
                            mister_arcade
                                .filter(|metadata| !metadata.title.is_empty())
                                .map(|metadata| metadata.title.as_str())
                                .or(title)
                        ),
                        string_interner.intern_optional(
                            mister_arcade
                                .and_then(|metadata| metadata.year)
                                .map(|value| value.to_string())
                                .as_deref()
                                .or(year)
                        ),
                        string_interner.intern_optional(
                            mister_arcade
                                .filter(|metadata| !metadata.manufacturer.is_empty())
                                .map(|metadata| metadata.manufacturer.as_str())
                                .or(manufacturer)
                        ),
                        string_interner.intern_optional(
                            mister_arcade
                                .filter(|metadata| !metadata.category.is_empty())
                                .map(|metadata| metadata.category.as_str())
                        ),
                        mister_arcade
                            .and_then(|metadata| metadata.players)
                            .or(players)
                            .map(i64::from),
                        string_interner.intern_optional(
                            mister_arcade
                                .filter(|metadata| !metadata.control.is_empty())
                                .map(|metadata| metadata.control.as_str())
                                .or(control)
                        ),
                        string_interner.intern(source)
                    ])
                    .map_err(|e| format!("insert launchable identity: {e}"))?;
                arcade_identity = Some((identity_id, family_id));
            }
            if let Some(identity) = software_identity.as_ref() {
                let identity_id = format!("{}:{}", identity.list_name, identity.software_name);
                identity_stmt
                    .execute(params![
                        game_key_id,
                        string_interner.intern("mame-software"),
                        string_interner.intern(identity_id.as_str()),
                        string_interner.intern(identity.family_id.as_str()),
                        string_interner.intern_optional(identity.metadata_title.as_deref()),
                        string_interner.intern_optional(identity.year.as_deref()),
                        string_interner.intern_optional(identity.manufacturer.as_deref()),
                        Option::<i64>::None,
                        Option::<i64>::None,
                        Option::<i64>::None,
                        string_interner.intern(identity.source)
                    ])
                    .map_err(|e| format!("insert software launchable identity: {e}"))?;
            }
            if canonical_catalog.is_some()
                && matches!(system_id.as_str(), "arcade" | "neogeo")
                && is_launcher_launch_ref(&plan_launch_ref)
                && promoted_systems.contains(&system_id)
            {
                let (identity_id, family_id) = arcade_identity
                    .map(|(identity_id, family_id)| (Some(identity_id), family_id))
                    .unwrap_or_else(|| (None, expanded_game_id(&key, discovery)));
                let preview_asset_key = arcade_compatibility_preview_asset_key(
                    &system_id,
                    identity_id.as_deref(),
                    &family_id,
                    discovery,
                );
                let canonical_key = (
                    plan_launch_ref.clone(),
                    discovery.title.clone(),
                    system_id.clone(),
                );
                let has_preview = canonical_preview_flags
                    .as_ref()
                    .and_then(|flags| flags.get(&canonical_key))
                    .copied()
                    .unwrap_or_else(|| {
                        !preview_asset_key.is_empty()
                            && sources
                                .preview_paths
                                .has_entry(&system_id, preview_asset_key.as_str())
                    });
                arcade_compatibility_rows.push(ArcadeCompatibilityRow {
                    launch_id,
                    family_id,
                    identity_id,
                    title: discovery.title.clone(),
                    system_id: system_id.clone(),
                    launch_ref: plan_launch_ref.clone(),
                    has_preview,
                    preview_asset_key,
                });
            }
            let region = software_identity
                .as_ref()
                .and_then(|identity| {
                    identity
                        .region
                        .as_deref()
                        .and_then(media_metadata::canonical_region_static)
                        .map(|region| media_metadata::RegionInference {
                            region: Some(region),
                            confidence: identity.source,
                        })
                })
                .unwrap_or_else(|| media_metadata::infer_region_metadata(discovery));
            if region.region.is_some() || region.confidence != "unknown" {
                region_stmt
                    .execute(params![game_key_id, region.region, region.confidence])
                    .map_err(|e| format!("insert region metadata: {e}"))?;
            }
            if canonical_launch_refs
                .as_ref()
                .is_some_and(|refs| refs.contains(plan_launch_ref.as_str()))
            {
                canonical_launch_ids.insert(
                    plan_launch_ref,
                    &discovery.title,
                    &system_id,
                    launch_id,
                );
            }
            let written = idx + 1;
            if written % 1000 == 0 || written == discovery_total {
                report_library_import_timing(
                    "insert_games_chunk",
                    chunk_t,
                    format!(
                        "from={} to={} total={discovery_total}",
                        chunk_start, written
                    ),
                );
                chunk_t = Instant::now();
                chunk_start = written;
            }
        }
        report_sqlite_import_progress(&mut progress, discovery_total, discovery_total);
        drop(target_stmt);
        drop(prepared_stmt);
        drop(prepared_diagnostic_stmt);
        path_interner.flush(&tx)?;
        string_interner.flush(&tx)?;
        drop(region_stmt);
        drop(identity_stmt);
        drop(game_stmt);
        drop(system_stmt);
        report_library_import_timing(
            "insert_games_total",
            stage_t,
            format!(
                "rows={discovery_total} launcher_rows={} canonical_launch_ids={} arcade_compatibility_rows={}",
                launcher_rows.len(),
                canonical_launch_ids.len(),
                arcade_compatibility_rows.len(),
            ),
        );
        report_sqlite_import_finalizing(&mut progress);
        if materialize_runtime_catalog {
            if let Some(catalog) = canonical_catalog {
                let projection_t = Instant::now();
                let arcade_count = catalog_projection::materialize_arcade_ui_projection_rows(
                    &tx,
                    arcade_compatibility_rows,
                    catalog,
                )?;
                report_library_import_timing(
                    "materialize_arcade_ui",
                    projection_t,
                    format!("rows={arcade_count} source=canonical_ram_catalog"),
                );
                report_library_import_timing(
                    "insert_launcher_arcade",
                    Instant::now(),
                    format!("rows={arcade_count} source=canonical_ram_catalog"),
                );
                let launcher_console_t = Instant::now();
                let stats = catalog_projection::insert_canonical_launcher_catalog(
                    &tx,
                    catalog,
                    &canonical_launch_ids,
                    arcade_count,
                )?;
                report_library_import_timing(
                    "insert_launcher_console",
                    launcher_console_t,
                    format!("rows={} source=canonical_ram_catalog", stats.rows),
                );
                report_library_import_timing(
                    "insert_launcher_launch_plans",
                    Instant::now(),
                    format!("rows={} source=canonical_ram_catalog", stats.launch_plans),
                );
            } else {
                let projection_t = Instant::now();
                let arcade_preview_projection = ArcadePreviewProjection::new(
                    sources
                        .preview_paths
                        .archive_for_platform("arcade")
                        .unwrap_or_default(),
                    sources
                        .preview_paths
                        .archive_for_platform("neogeo")
                        .unwrap_or_default(),
                );
                catalog_projection::materialize_arcade_ui_projections(
                    &tx,
                    &arcade_preview_projection,
                )?;
                report_library_import_timing("materialize_arcade_ui", projection_t, "");
                let launcher_arcade_t = Instant::now();
                catalog_projection::insert_arcade_launcher_catalog(&tx)?;
                report_library_import_timing("insert_launcher_arcade", launcher_arcade_t, "");
                let launcher_console_t = Instant::now();
                let launcher_game_count =
                    catalog_projection::insert_console_launcher_catalog(&tx, launcher_rows)?;
                report_library_import_timing(
                    "insert_launcher_console",
                    launcher_console_t,
                    format!("rows={launcher_game_count}"),
                );
                let launcher_plans_t = Instant::now();
                let launcher_plan_count =
                    catalog_projection::materialize_launcher_launch_plans(&tx)?;
                report_library_import_timing(
                    "insert_launcher_launch_plans",
                    launcher_plans_t,
                    format!("rows={launcher_plan_count}"),
                );
            }
        } else {
            report_library_import_timing(
                "materialize_runtime_catalog",
                Instant::now(),
                "skipped=precomputed_ram_catalog",
            );
        }
    }
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare("INSERT INTO meta(key,value) VALUES (?1,?2)")
            .map_err(|e| format!("prepare meta insert: {e}"))?;
        stmt.execute(params!["version", scan.version as i64])
            .map_err(|e| format!("insert version: {e}"))?;
        stmt.execute(params!["scanned_at_unix", scan.scanned_at_unix])
            .map_err(|e| format!("insert scanned_at_unix: {e}"))?;
        stmt.execute(params!["normal_files", scan.normal_files.len() as i64])
            .map_err(|e| format!("insert normal count: {e}"))?;
        stmt.execute(params!["containers", scan.containers.len() as i64])
            .map_err(|e| format!("insert container count: {e}"))?;
        stmt.execute(params!["entries", scan.entries.len() as i64])
            .map_err(|e| format!("insert entry count: {e}"))?;
        stmt.execute(params!["audit_rows", scan.audit_rows.len() as i64])
            .map_err(|e| format!("insert audit row count: {e}"))?;
        stmt.execute(params!["ignored_files", scan.ignored_files as i64])
            .map_err(|e| format!("insert ignored count: {e}"))?;
        stmt.execute(params!["discoveries", discovery_total as i64])
            .map_err(|e| format!("insert discovery count: {e}"))?;
        stmt.execute(params!["mame_metadata_size", mame_signature.size as i64])
            .map_err(|e| format!("insert mame metadata size: {e}"))?;
        stmt.execute(params!["mame_metadata_mtime", mame_signature.mtime_secs])
            .map_err(|e| format!("insert mame metadata mtime: {e}"))?;
        stmt.execute(params![
            "hbmame_metadata_size",
            hbmame_signature.size as i64
        ])
        .map_err(|e| format!("insert hbmame metadata size: {e}"))?;
        stmt.execute(params![
            "hbmame_metadata_mtime",
            hbmame_signature.mtime_secs
        ])
        .map_err(|e| format!("insert hbmame metadata mtime: {e}"))?;
        report_library_import_timing("insert_meta", stage_t, "rows=12");
    }
    if let Some(stamp) = sources.stamp {
        let stage_t = Instant::now();
        catalog_store::write_catalog_stamp(&tx, stamp)?;
        let checkpoint = catalog_checkpoint::compute_catalog_discovery_checkpoint_from_facts(
            &scan.roots,
            sources.mame_sqlite_path,
            sources.hbmame_sqlite_path,
            &scan.audit_rows,
            &scan.installed_cores,
            &scan.game_dir_facts,
        );
        catalog_store::write_catalog_discovery_checkpoint(&tx, &checkpoint)?;
        report_library_import_timing(
            "insert_catalog_stamp",
            stage_t,
            format!(
                "stamp_rows={} checkpoint_rows={}",
                stamp.lines().len(),
                checkpoint.lines().len()
            ),
        );
    }
    if let Some(canonical_navigation) = canonical_navigation {
        let stage_t = Instant::now();
        tx.execute(
            "INSERT INTO catalog_navigation_projection(id,bytes) VALUES (0,?1)",
            [canonical_navigation],
        )
        .map_err(|e| format!("insert embedded catalog navigation: {e}"))?;
        report_library_import_timing(
            "insert_catalog_navigation",
            stage_t,
            format!("bytes={}", canonical_navigation.len()),
        );
    }
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare(
                "INSERT INTO software_hash_cache(list_name,file_path,size,mtime_secs,software_name)
                 VALUES (?1,?2,?3,?4,?5)",
            )
            .map_err(|e| format!("prepare software hash cache insert: {e}"))?;
        for (key, software_name) in &sources.software_hash_cache.entries {
            stmt.execute(params![
                key.list_name.as_str(),
                key.file_path.as_str(),
                key.size as i64,
                key.mtime_secs,
                software_name.as_deref()
            ])
            .map_err(|e| format!("insert software hash cache: {e}"))?;
        }
        report_library_import_timing(
            "insert_software_hash_cache",
            stage_t,
            format!("rows={}", sources.software_hash_cache.entries.len()),
        );
    }
    let saved_catalog_t = Instant::now();
    let saved_catalog = match root {
        Some(root) => {
            let saved_catalog = load_arcade_catalog_from_connection(
                root,
                &tx,
                saved_catalog_t,
                0,
                0,
                sources.stamp.cloned(),
                true,
            )?;
            report_library_import_timing(
                "build_saved_catalog",
                saved_catalog_t,
                format!("rows={}", saved_catalog.rows),
            );
            Some(saved_catalog)
        }
        None => {
            report_library_import_timing(
                "build_saved_catalog",
                saved_catalog_t,
                "skipped=precomputed_projection",
            );
            None
        }
    };
    let commit_t = Instant::now();
    tx.commit().map_err(|e| format!("commit sqlite tx: {e}"))?;
    report_library_import_timing("commit", commit_t, "");
    report_library_import_timing("total", total_t, format!("path={}", path.display()));
    Ok(saved_catalog)
}

pub(crate) fn report_library_import_timing(
    stage: &str,
    started: Instant,
    detail: impl std::fmt::Display,
) {
    crate::catalog_logln!(
        "library_import_timing\t{stage}\t{}\t{detail}",
        started.elapsed().as_micros()
    );
}

fn tsv_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn report_sqlite_import_progress(
    progress: &mut ProgressCallback<'_>,
    written: usize,
    total: usize,
) {
    report_catalog_progress(
        progress,
        CatalogProgress::saving_sqlite_import(written, total),
    );
}

fn report_sqlite_import_finalizing(progress: &mut ProgressCallback<'_>) {
    report_catalog_progress(progress, CatalogProgress::saving_finalizing());
}

pub(crate) fn sqlite_cached_summary(
    path: &Path,
    scan_us: u64,
) -> Result<LibraryRefreshSummary, String> {
    catalog_load_metrics::record_sqlite_open();
    let conn = open_sqlite_read_only(path).map_err(|e| format!("open cached summary: {e}"))?;
    ensure_sqlite_schema_current(&conn)?;
    let bytes = std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("stat cached summary db: {e}"))?;
    Ok(LibraryRefreshSummary {
        skipped: true,
        scan_us,
        discover_us: 0,
        classify_us: 0,
        import_us: 0,
        bytes,
        normal_files: sqlite_meta_usize(&conn, "normal_files").unwrap_or(0),
        containers: sqlite_meta_usize(&conn, "containers").unwrap_or(0),
        entries: sqlite_meta_usize(&conn, "entries").unwrap_or(0),
        audit_rows: sqlite_meta_usize(&conn, "audit_rows").unwrap_or(0),
        discoveries: sqlite_meta_usize(&conn, "discoveries").unwrap_or(0),
    })
}

pub(crate) fn refresh_preview_index_flags(
    label: &str,
) -> Result<Vec<PreviewIndexRefreshRow>, String> {
    refresh_preview_index_flags_at(&default_sqlite_path(), label)
}

pub(crate) fn refresh_preview_index_flags_at(
    path: &Path,
    label: &str,
) -> Result<Vec<PreviewIndexRefreshRow>, String> {
    let mut conn = Connection::open(path).map_err(|e| format!("open library db: {e}"))?;
    ensure_sqlite_schema_current(&conn)?;
    let packs = preview_index_refresh_packs();
    let mut rows = Vec::with_capacity(packs.len());
    for (system_id, pack_path) in packs {
        rows.push(refresh_preview_index_flags_for_system(
            &mut conn, label, &system_id, &pack_path,
        ));
    }
    Ok(rows)
}

fn preview_index_refresh_packs() -> Vec<(String, String)> {
    let preview_paths = PreviewArchivePaths::from_paths(
        preview_worker::preview_archive_paths_for_catalog_projection(),
    );
    media_identity::supported_screenshot_pack_ids()
        .map(|system_id| {
            let pack_path = preview_paths
                .archive_for_platform(system_id)
                .map(preview_worker::resolved_preview_archive_path)
                .unwrap_or_default();
            (system_id.to_string(), pack_path)
        })
        .collect()
}

fn refresh_preview_index_flags_for_system(
    conn: &mut Connection,
    label: &str,
    system_id: &str,
    pack_path: &str,
) -> PreviewIndexRefreshRow {
    let total_t = Instant::now();
    let archive_path = Path::new(pack_path);
    let index_path = if pack_path.is_empty() {
        String::new()
    } else {
        preview_worker::preview_archive_sidecar_path_for_archive(archive_path)
            .display()
            .to_string()
    };
    let base_row = |result: &str,
                    error: String,
                    index_entries: usize,
                    candidate_rows: usize,
                    updated_rows: usize,
                    index_read_us: u64,
                    sql_update_us: u64| {
        PreviewIndexRefreshRow {
            label: label.to_string(),
            system_id: system_id.to_string(),
            pack_path: pack_path.to_string(),
            index_path: index_path.clone(),
            index_entries,
            candidate_rows,
            updated_rows,
            index_read_us,
            sql_update_us,
            total_us: total_t.elapsed().as_micros() as u64,
            result: result.to_string(),
            error,
        }
    };
    if pack_path.is_empty() || !archive_path.is_file() {
        let sql_t = Instant::now();
        return match set_preview_flags_for_system(conn, system_id, None) {
            Ok((candidate_rows, updated_rows)) => base_row(
                "missing-pack",
                String::new(),
                0,
                candidate_rows,
                updated_rows,
                0,
                sql_t.elapsed().as_micros() as u64,
            ),
            Err(error) => base_row(
                "error",
                error,
                0,
                0,
                0,
                0,
                sql_t.elapsed().as_micros() as u64,
            ),
        };
    }
    let index_t = Instant::now();
    let stems = match preview_worker::preview_archive_sidecar_entry_stems(archive_path) {
        Ok(Some(stems)) => stems,
        Ok(None) => {
            let index_read_us = index_t.elapsed().as_micros() as u64;
            let sql_t = Instant::now();
            return match set_preview_flags_for_system(conn, system_id, None) {
                Ok((candidate_rows, updated_rows)) => base_row(
                    "missing-index",
                    String::new(),
                    0,
                    candidate_rows,
                    updated_rows,
                    index_read_us,
                    sql_t.elapsed().as_micros() as u64,
                ),
                Err(error) => base_row(
                    "error",
                    error,
                    0,
                    0,
                    0,
                    index_read_us,
                    sql_t.elapsed().as_micros() as u64,
                ),
            };
        }
        Err(error) => {
            return base_row(
                "error",
                error,
                0,
                0,
                0,
                index_t.elapsed().as_micros() as u64,
                0,
            );
        }
    };
    let index_read_us = index_t.elapsed().as_micros() as u64;
    let sql_t = Instant::now();
    match set_preview_flags_for_system(conn, system_id, Some(&stems.entries)) {
        Ok((candidate_rows, updated_rows)) => base_row(
            "ok",
            String::new(),
            stems.entries.len(),
            candidate_rows,
            updated_rows,
            index_read_us,
            sql_t.elapsed().as_micros() as u64,
        ),
        Err(error) => base_row(
            "error",
            error,
            stems.entries.len(),
            0,
            0,
            index_read_us,
            sql_t.elapsed().as_micros() as u64,
        ),
    }
}

fn set_preview_flags_for_system(
    conn: &mut Connection,
    system_id: &str,
    entries: Option<&[String]>,
) -> Result<(usize, usize), String> {
    let launcher_table = if sqlite_physical_table_exists(conn, "launcher_catalog_rows")? {
        "launcher_catalog_rows"
    } else {
        "launcher_catalog"
    };
    let tables = [launcher_table, "ui_arcade_variants"];
    let tx = conn
        .transaction()
        .map_err(|e| format!("begin preview index refresh tx: {e}"))?;
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS preview_index_keys(asset_key TEXT PRIMARY KEY) WITHOUT ROWID;
         DELETE FROM preview_index_keys;",
    )
    .map_err(|e| format!("prepare preview index keys: {e}"))?;
    if let Some(entries) = entries {
        let mut stmt = tx
            .prepare("INSERT OR IGNORE INTO preview_index_keys(asset_key) VALUES (?1)")
            .map_err(|e| format!("prepare preview index key insert: {e}"))?;
        for entry in entries {
            stmt.execute([entry.as_str()])
                .map_err(|e| format!("insert preview index key: {e}"))?;
        }
    }
    let mut candidate_rows = 0usize;
    let mut updated_rows = 0usize;
    for table in tables {
        candidate_rows += count_preview_candidates(&tx, table, system_id)?;
        updated_rows += update_preview_candidates(&tx, table, system_id, entries.is_some())?;
    }
    tx.commit()
        .map_err(|e| format!("commit preview index refresh tx: {e}"))?;
    Ok((candidate_rows, updated_rows))
}

fn count_preview_candidates(
    conn: &Connection,
    table: &str,
    system_id: &str,
) -> Result<usize, String> {
    let sql = if table == "launcher_catalog_rows" {
        "SELECT count(*)
         FROM launcher_catalog_rows
         JOIN launch_targets ON launch_targets.launch_id = launcher_catalog_rows.launch_id
         JOIN games ON games.game_key_id = launch_targets.game_key_id
         WHERE games.system_id=?1
           AND launcher_catalog_rows.preview_asset_key != ''"
            .to_string()
    } else {
        format!(
            "SELECT count(*)
             FROM {table}
             WHERE system_id=?1
               AND preview_asset_key != ''"
        )
    };
    conn.query_row(&sql, [system_id], |row| row.get::<_, i64>(0))
        .map(|count| count.max(0) as usize)
        .map_err(|e| format!("count preview candidates in {table}: {e}"))
}

fn update_preview_candidates(
    conn: &Connection,
    table: &str,
    system_id: &str,
    has_index: bool,
) -> Result<usize, String> {
    let sql = if has_index {
        if table == "launcher_catalog_rows" {
            "UPDATE launcher_catalog_rows
             SET has_preview = CASE
                 WHEN EXISTS (
                     SELECT 1
                     FROM preview_index_keys k
                     WHERE k.asset_key = lower(launcher_catalog_rows.preview_asset_key)
                 )
                 THEN 1 ELSE 0 END
             WHERE launcher_catalog_rows.preview_asset_key != ''
               AND EXISTS (
                   SELECT 1
                   FROM launch_targets
                   JOIN games ON games.game_key_id = launch_targets.game_key_id
                   WHERE launch_targets.launch_id = launcher_catalog_rows.launch_id
                     AND games.system_id=?1
               )"
            .to_string()
        } else {
            format!(
                "UPDATE {table}
                 SET has_preview = CASE
                     WHEN EXISTS (
                         SELECT 1
                         FROM preview_index_keys k
                         WHERE k.asset_key = lower({table}.preview_asset_key)
                     )
                     THEN 1 ELSE 0 END
                 WHERE system_id=?1
                   AND preview_asset_key != ''"
            )
        }
    } else {
        if table == "launcher_catalog_rows" {
            "UPDATE launcher_catalog_rows
             SET has_preview = 0
             WHERE launcher_catalog_rows.preview_asset_key != ''
               AND EXISTS (
                   SELECT 1
                   FROM launch_targets
                   JOIN games ON games.game_key_id = launch_targets.game_key_id
                   WHERE launch_targets.launch_id = launcher_catalog_rows.launch_id
                     AND games.system_id=?1
               )"
            .to_string()
        } else {
            format!(
                "UPDATE {table}
                 SET has_preview = 0
                 WHERE system_id=?1
                   AND preview_asset_key != ''"
            )
        }
    };
    conn.execute(&sql, [system_id])
        .map_err(|e| format!("update preview candidates in {table}: {e}"))
}

fn ensure_sqlite_schema_current(conn: &Connection) -> Result<(), String> {
    match sqlite_meta_usize(conn, "version") {
        Some(version) if version == SCHEMA_VERSION as usize => Ok(()),
        Some(version) => Err(format!(
            "catalog schema mismatch: expected {SCHEMA_VERSION}, found {version}"
        )),
        None => Err(format!(
            "catalog schema mismatch: expected {SCHEMA_VERSION}, found missing"
        )),
    }
}

fn sqlite_meta_usize(conn: &Connection, key: &str) -> Option<usize> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| {
        r.get::<_, i64>(0)
    })
    .ok()
    .map(|n| n.max(0) as usize)
}

pub(crate) fn source_kind_name(kind: RuleSourceKind) -> &'static str {
    match kind {
        RuleSourceKind::MainSource => "main-source",
        RuleSourceKind::Mgl => "mgl",
        RuleSourceKind::Mra => "mra",
        RuleSourceKind::ConfStr => "conf-str",
        RuleSourceKind::MagikProfile => "magik-profile",
    }
}

fn arcade_metadata_setnames<'a>(
    discoveries: impl Iterator<Item = &'a GameDiscovery>,
) -> HashSet<String> {
    discoveries
        .filter_map(mame_identity_for_discovery)
        .collect()
}

pub(crate) fn mount_kind_str(kind: MountKind) -> &'static str {
    match kind {
        MountKind::Launcher => "launcher",
        MountKind::LoadFile => "load-file",
        MountKind::MountImage => "mount-image",
        MountKind::Core => "core",
    }
}

struct LaunchRefStorage<'a> {
    kind: &'static str,
    path: Option<&'a str>,
}

struct GameIdStorage<'a> {
    kind: &'static str,
    path: Option<&'a str>,
    text: Option<&'a str>,
}

fn game_id_storage_for<'a>(game_id: &'a str, discovery: &'a GameDiscovery) -> GameIdStorage<'a> {
    match discovery.source_kind {
        DiscoverySourceKind::Mgl | DiscoverySourceKind::PayloadFile => GameIdStorage {
            kind: "payload",
            path: Some(discovery.launch_ref.as_str()),
            text: None,
        },
        DiscoverySourceKind::ArchiveEntry => GameIdStorage {
            kind: "archive",
            path: Some(discovery.launch_ref.as_str()),
            text: None,
        },
        DiscoverySourceKind::Mra | DiscoverySourceKind::CatalogEntry => GameIdStorage {
            kind: "text",
            path: None,
            text: Some(game_id),
        },
    }
}

fn expanded_game_id(game_id: &str, discovery: &GameDiscovery) -> String {
    match discovery.source_kind {
        DiscoverySourceKind::Mgl | DiscoverySourceKind::PayloadFile => {
            format!("payload:{}", discovery.launch_ref)
        }
        DiscoverySourceKind::ArchiveEntry => format!("archive:{}", discovery.launch_ref),
        DiscoverySourceKind::Mra | DiscoverySourceKind::CatalogEntry => game_id.to_string(),
    }
}

fn arcade_compatibility_preview_asset_key(
    system_id: &str,
    identity_id: Option<&str>,
    family_id: &str,
    discovery: &GameDiscovery,
) -> String {
    let setname = discovery.setname.as_deref().unwrap_or_default();
    if system_id == "neogeo" {
        return setname.to_string();
    }
    if let Some(identity_id) = identity_id {
        return if family_id.is_empty() {
            identity_id.to_string()
        } else {
            family_id.to_string()
        };
    }
    discovery
        .parent
        .as_deref()
        .filter(|parent| !parent.trim().is_empty())
        .unwrap_or(setname)
        .to_string()
}

fn launch_ref_storage_for<'a>(
    launch_ref: &'a str,
    payload_path: Option<&'a str>,
    launch_kind: &str,
) -> LaunchRefStorage<'a> {
    if let Some(payload_path) = payload_path {
        if launch_kind == "virtual-mgl" {
            let payload_ref = format!("magik-plan:payload:{payload_path}");
            if launch_ref == payload_ref {
                return LaunchRefStorage {
                    kind: "payload",
                    path: None,
                };
            }
            let archive_ref = format!("magik-plan:archive:{payload_path}");
            if launch_ref == archive_ref {
                return LaunchRefStorage {
                    kind: "archive",
                    path: None,
                };
            }
        }
        if launch_kind == "catalog-entry" && launch_ref == payload_path {
            return LaunchRefStorage {
                kind: "same-payload",
                path: None,
            };
        }
    }
    LaunchRefStorage {
        kind: "path",
        path: Some(launch_ref),
    }
}

fn launch_target_mount_for_discovery(
    discovery: &GameDiscovery,
    profile_id: Option<&str>,
    profiles: &[LaunchProfile],
) -> Option<MountSpec> {
    if launch_kind_for_discovery(discovery) != "virtual-mgl" {
        return None;
    }
    let mount =
        crate::catalog_scan::profile_for_path(profiles, Path::new(discovery.source_path.as_str()))
            .or_else(|| {
                profile_id.and_then(|profile_id| {
                    launch_profiles::profile_for_launch_target_id(profiles, profile_id)
                })
            })
            .and_then(|profile| match discovery.source_kind {
                DiscoverySourceKind::ArchiveEntry => {
                    let member_path = crate::archive_member::decode_archive_member_ref(
                        discovery.launch_ref.as_str(),
                    )
                    .ok()
                    .flatten()
                    .map(|member| member.member_path)
                    .unwrap_or_else(|| discovery.launch_ref.clone());
                    profile
                        .classify_archive_entry(Path::new(&member_path))
                        .map(|rule| rule.mount)
                }
                DiscoverySourceKind::PayloadFile => {
                    match profile.classify_path(Path::new(discovery.launch_ref.as_str())) {
                        launch_profiles::ProfilePathClass::Payload { rule } => Some(rule.mount),
                        _ => None,
                    }
                }
                _ => None,
            })
            .unwrap_or_else(|| MountSpec::mount_image(0));
    Some(mount)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arcade_catalog::GameSystemEntry;
    use crate::catalog_config::{DEFAULT_SQLITE_BUILD_DIR, SCHEMA_VERSION};
    use crate::library_db::{
        BenchConfig, ProgressCallback, save_scan_artifact_to_sqlite, scan_library_artifact,
    };

    #[test]
    fn stored_platform_kind_is_strictly_validated() {
        let conn = Connection::open_in_memory().expect("memory sqlite");
        conn.execute_batch(
            "CREATE TABLE systems(system_id TEXT PRIMARY KEY, platform_kind TEXT NOT NULL);
             INSERT INTO systems VALUES ('bad-system', 'cabinet');",
        )
        .expect("seed invalid stored kind");
        let error =
            load_system_platform_kinds(&conn).expect_err("invalid kind must fail hydration");
        assert!(error.contains("invalid stored platform kind"), "{error}");
    }

    #[test]
    fn core_location_parser_is_diagnostic_only_and_component_exact() {
        assert_eq!(
            core_location_category(Some("_Arcade/cores/SMS")),
            Some("Arcade")
        );
        assert_eq!(
            core_location_category(Some("_Computer/Amiga")),
            Some("Computer")
        );
        assert_eq!(core_location_category(Some("cores/My_Arcade_Core")), None);
    }

    #[test]
    fn sqlite_persists_actual_installed_core_location_disagreements() {
        let root = unique_temp_dir("sqlite-system-classification-diagnostics");
        let db = root.join("library.sqlite3");
        let mut scan = sqlite_scan_with_discoveries(Vec::new());
        let mut astrocade = scan
            .profiles
            .iter()
            .find(|profile| profile.system_id == "sms")
            .expect("SMS profile template")
            .clone();
        astrocade.id = "runtime-astrocade".to_string();
        astrocade.system_id = "astrocade".to_string();
        astrocade.core_name = "Astrocade".to_string();
        scan.profiles.push(astrocade);
        scan.installed_cores = ["SMS", "GameGear", "Astrocade"]
            .into_iter()
            .map(|core_id| crate::catalog_discovery::InstalledCore {
                core_id: core_id.to_string(),
                path: PathBuf::from(format!("/media/fat/_Arcade/cores/{core_id}.rbf")),
            })
            .collect();

        save_sqlite_scan(&db, &scan).expect("persist classification diagnostics");
        let conn = Connection::open(&db).expect("open diagnostics database");
        let rows = conn
            .query_row(
                "SELECT count(*) FROM system_classification_diagnostics
                 WHERE rejected_source='core-location' AND rejected_kind='arcade'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count classification diagnostics");
        assert_eq!(rows, 3);
        let _ = std::fs::remove_dir_all(root);
    }
    use crate::preview_worker;
    use crate::test_support::*;
    use rusqlite::Connection;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    #[derive(Default)]
    struct RecordingFaultControl {
        requests: Vec<crate::fs_fault::DirectResetFaultRequest>,
    }

    impl crate::fs_fault::DirectResetFaultControl for RecordingFaultControl {
        fn request_direct_reset(
            &mut self,
            request: &crate::fs_fault::DirectResetFaultRequest,
        ) -> crate::fs_fault::DirectResetFaultOutcome {
            self.requests.push(request.clone());
            crate::fs_fault::DirectResetFaultOutcome::Noop
        }
    }

    #[test]
    fn sqlite_fault_hook_preserves_publish_and_reset_delete_order() {
        let root = unique_temp_dir("sqlite-fault-hook-order");
        let final_path = root.join("library.sqlite3");
        let build_path = root.join("build.sqlite3");
        let final_temp_path = root.join(".library.sqlite3.tmp");
        std::fs::write(&build_path, b"sqlite fixture").expect("write build temp");
        let plan = SqliteBuildTempPlan {
            build_tmp_path: build_path,
            final_tmp_path: final_temp_path,
            source: SqliteBuildTempSource::BesideFinal,
        };
        let mut control = RecordingFaultControl::default();
        let mut progress = None;
        publish_sqlite_temp(&final_path, &plan, &mut progress, &mut control)
            .expect("publish sqlite fixture");

        let summary_path = catalog_summary::summary_path_for_sqlite(&final_path);
        let navigation_path = catalog_navigation::navigation_path_for_sqlite(&final_path);
        let duration_path = crate::catalog_build_record::duration_path_for_sqlite(&final_path);
        std::fs::write(&summary_path, b"summary").expect("write summary");
        std::fs::write(&navigation_path, b"navigation").expect("write navigation");
        std::fs::write(&duration_path, b"duration").expect("write duration");
        remove_sqlite_database_at_with_fault_control(&final_path, &mut control)
            .expect("remove sqlite fixture");

        assert_eq!(
            control
                .requests
                .iter()
                .map(|request| request.point())
                .collect::<Vec<_>>(),
            vec![
                "catalog.sqlite.after_build_temp_sync",
                "catalog.sqlite.after_final_temp_copy",
                "catalog.sqlite.after_final_temp_sync",
                "catalog.sqlite.after_rename_before_parent_sync",
                "reset_delete.database.after_remove",
                "reset_delete.summary.after_remove",
                "reset_delete.navigation.after_remove",
            ]
        );
        assert!(control.requests.iter().all(|request| {
            request
                .target()
                .starts_with(root.to_str().expect("temp root"))
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    fn discovered_at_for_title(db: &Path, title: &str) -> Option<i64> {
        let conn = open_sqlite_read_only(db).expect("open discovery db");
        let mut stmt = conn
            .prepare("SELECT discovered_at_unix FROM games WHERE title=?1")
            .expect("prepare discovery query");
        let mut rows = stmt.query([title]).expect("query discovery time");
        let row = rows
            .next()
            .expect("read discovery row")
            .expect("discovery row");
        row.get::<_, Option<i64>>(0).expect("read discovered_at")
    }

    fn write_schema31_games_fixture(db: &Path, game_ids: &[&str]) {
        let conn = Connection::open(db).expect("open schema31 fixture");
        conn.execute_batch("CREATE TABLE games(game_id TEXT PRIMARY KEY) WITHOUT ROWID;")
            .expect("create schema31 games");
        let mut stmt = conn
            .prepare("INSERT INTO games(game_id) VALUES (?1)")
            .expect("prepare schema31 games");
        for game_id in game_ids {
            stmt.execute([*game_id]).expect("insert schema31 game");
        }
    }

    #[test]
    fn path_storage_splits_prefix_and_leaf() {
        assert_eq!(
            split_path_for_storage(
                "/media/fat/games/NEOGEO/Neo Geo Mister FGPA Ultra Pack.zip/Neo Geo Mister FGPA Ultra Pack/ World A-Z/Game.neo"
            ),
            (
                "/media/fat/games/NEOGEO/Neo Geo Mister FGPA Ultra Pack.zip/Neo Geo Mister FGPA Ultra Pack/ World A-Z/",
                "Game.neo"
            )
        );
        assert_eq!(
            split_path_for_storage("magik-amigavision:Alien%20Breed"),
            ("", "magik-amigavision:Alien%20Breed")
        );
    }

    fn write_schema32_games_fixture(db: &Path, games: &[(&str, Option<i64>)]) {
        let conn = Connection::open(db).expect("open schema32 fixture");
        conn.execute_batch(
            "CREATE TABLE games(
                game_id TEXT PRIMARY KEY,
                discovered_at_unix INTEGER
            ) WITHOUT ROWID;",
        )
        .expect("create schema32 games");
        let mut stmt = conn
            .prepare("INSERT INTO games(game_id,discovered_at_unix) VALUES (?1,?2)")
            .expect("prepare schema32 games");
        for (game_id, discovered_at_unix) in games {
            stmt.execute(params![*game_id, discovered_at_unix])
                .expect("insert schema32 game");
        }
    }

    fn write_preview_refresh_fixture(db: &Path, rows: &[(&str, &str, &str, bool)]) {
        let conn = Connection::open(db).expect("open preview refresh fixture");
        conn.execute_batch(&format!(
            "
            CREATE TABLE meta(key TEXT PRIMARY KEY, value INTEGER NOT NULL) WITHOUT ROWID;
            INSERT INTO meta(key,value) VALUES ('version', {SCHEMA_VERSION});
            CREATE TABLE launcher_catalog(
                system_id TEXT NOT NULL,
                preview_asset_key TEXT NOT NULL,
                has_preview INTEGER NOT NULL
            );
            CREATE TABLE ui_arcade_preferred(
                system_id TEXT NOT NULL,
                preview_asset_key TEXT NOT NULL,
                has_preview INTEGER NOT NULL
            );
            CREATE TABLE ui_arcade_variants(
                system_id TEXT NOT NULL,
                preview_asset_key TEXT NOT NULL,
                has_preview INTEGER NOT NULL
            );
            "
        ))
        .expect("create preview refresh fixture");
        for table in [
            "launcher_catalog",
            "ui_arcade_preferred",
            "ui_arcade_variants",
        ] {
            let mut stmt = conn
                .prepare(&format!(
                    "INSERT INTO {table}(system_id,preview_asset_key,has_preview)
                     VALUES (?1,?2,?3)"
                ))
                .expect("prepare preview refresh row");
            for (system_id, archive_path, asset_key, has_preview) in rows {
                let _ = archive_path;
                stmt.execute(params![
                    *system_id,
                    *asset_key,
                    if *has_preview { 1 } else { 0 }
                ])
                .expect("insert preview refresh row");
            }
        }
    }

    fn write_preview_sidecar_index(pack: &Path, names: &[&str]) {
        let width = 256u32;
        let height = 240u32;
        let stride_bytes = width * 2;
        let raw_len = stride_bytes * height;
        let index_len = 8
            + 4
            + names
                .iter()
                .map(|name| 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len())
                .sum::<usize>();
        let mut archive = Vec::new();
        archive.extend_from_slice(b"MMPX2B1\0");
        archive.extend_from_slice(&(names.len() as u32).to_le_bytes());
        for (idx, name) in names.iter().enumerate() {
            archive.extend_from_slice(&(name.len() as u16).to_le_bytes());
            archive.extend_from_slice(&width.to_le_bytes());
            archive.extend_from_slice(&height.to_le_bytes());
            archive.extend_from_slice(&stride_bytes.to_le_bytes());
            archive.extend_from_slice(&raw_len.to_le_bytes());
            archive.push(1);
            archive.extend_from_slice(&raw_len.to_le_bytes());
            archive.extend_from_slice(&((index_len + idx * raw_len as usize) as u64).to_le_bytes());
            archive.extend_from_slice(name.as_bytes());
        }
        for _ in names {
            archive.resize(archive.len() + raw_len as usize, 0);
        }
        std::fs::write(pack, archive).expect("write preview pack fixture");
        let archive_bytes = std::fs::metadata(pack).expect("stat preview pack").len();
        let mut index = Vec::new();
        index.extend_from_slice(b"MMIDX02\0");
        index.extend_from_slice(&archive_bytes.to_le_bytes());
        index
            .extend_from_slice(b"0000000000000000000000000000000000000000000000000000000000000000");
        index.extend_from_slice(&(names.len() as u32).to_le_bytes());
        for (idx, name) in names.iter().enumerate() {
            index.extend_from_slice(&(name.len() as u16).to_le_bytes());
            index.extend_from_slice(&width.to_le_bytes());
            index.extend_from_slice(&height.to_le_bytes());
            index.extend_from_slice(&stride_bytes.to_le_bytes());
            index.extend_from_slice(&raw_len.to_le_bytes());
            index.push(1);
            index.extend_from_slice(&raw_len.to_le_bytes());
            index.extend_from_slice(&((index_len + idx * raw_len as usize) as u64).to_le_bytes());
            index.extend_from_slice(name.as_bytes());
        }
        std::fs::write(
            preview_worker::preview_archive_sidecar_path_for_archive(pack),
            index,
        )
        .expect("write preview sidecar index");
    }

    fn preview_flag_counts(db: &Path, table: &str, system_id: &str) -> (i64, i64) {
        let conn = Connection::open(db).expect("open preview refresh db");
        conn.query_row(
            &format!(
                "SELECT count(*), COALESCE(sum(has_preview), 0)
                 FROM {table}
                 WHERE system_id=?1"
            ),
            [system_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("query preview flags")
    }

    #[test]
    fn preview_index_refresh_updates_only_members_in_system() {
        let root = unique_temp_dir("preview-index-refresh");
        let db = root.join("library.sqlite3");
        let pack = root.join("nes-screenshots-320x320.mmlz4b");
        let pack_text = pack.display().to_string();
        write_preview_sidecar_index(&pack, &["present.rgb565"]);
        write_preview_refresh_fixture(
            &db,
            &[
                ("nes", &pack_text, "present", false),
                ("nes", &pack_text, "missing", true),
                ("snes", &pack_text, "missing", true),
            ],
        );
        let mut conn = Connection::open(&db).expect("open preview refresh db");

        let row = refresh_preview_index_flags_for_system(&mut conn, "TEST", "nes", &pack_text);

        assert_eq!(row.result, "ok");
        assert_eq!(row.system_id, "nes");
        assert_eq!(row.index_entries, 1);
        assert_eq!(row.candidate_rows, 4);
        assert_eq!(preview_flag_counts(&db, "launcher_catalog", "nes"), (2, 1));
        assert_eq!(
            preview_flag_counts(&db, "ui_arcade_variants", "nes"),
            (2, 1)
        );
        assert_eq!(preview_flag_counts(&db, "launcher_catalog", "snes"), (1, 1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preview_index_refresh_missing_index_clears_system_without_error() {
        let root = unique_temp_dir("preview-index-missing");
        let db = root.join("library.sqlite3");
        let pack = root.join("nes-screenshots-320x320.mmlz4b");
        std::fs::write(&pack, vec![0u8; 16]).expect("write pack without index");
        let pack_text = pack.display().to_string();
        write_preview_refresh_fixture(&db, &[("nes", &pack_text, "present", true)]);
        let mut conn = Connection::open(&db).expect("open preview refresh db");

        let row = refresh_preview_index_flags_for_system(&mut conn, "TEST", "nes", &pack_text);

        assert_eq!(row.result, "missing-index");
        assert!(row.error.is_empty());
        assert_eq!(row.candidate_rows, 2);
        assert_eq!(preview_flag_counts(&db, "launcher_catalog", "nes"), (1, 0));
        assert_eq!(
            preview_flag_counts(&db, "ui_arcade_variants", "nes"),
            (1, 0)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preview_index_refresh_tsv_has_stable_shape() {
        let row = PreviewIndexRefreshRow {
            label: "LABEL".to_string(),
            system_id: "nes".to_string(),
            pack_path: "/tmp/nes pack.mmlz4b".to_string(),
            index_path: "/tmp/nes pack.mmlz4b.idx".to_string(),
            index_entries: 10,
            candidate_rows: 3,
            updated_rows: 2,
            index_read_us: 4,
            sql_update_us: 5,
            total_us: 9,
            result: "ok".to_string(),
            error: String::new(),
        };

        assert_eq!(
            PREVIEW_INDEX_REFRESH_TSV_HEADER,
            "preview_index_refresh_tsv\tlabel\tsystem_id\tpack_path\tindex_path\tindex_entries\tcandidate_rows\tupdated_rows\tindex_read_us\tsql_update_us\ttotal_us\tresult\terror"
        );
        assert_eq!(row.to_tsv().split('\t').count(), 13);
    }

    #[test]
    fn sqlite_save_keeps_previous_database_when_replacement_fails() {
        let root = unique_temp_dir("sqlite-atomic-replace");
        let db = root.join("library.sqlite3");
        save_sqlite_scan(&db, &sqlite_scan_with_normal_files(&["/old/game.mra"]))
            .expect("write old database");
        let old_summary = sqlite_cached_summary(&db, 0).expect("old database readable");
        assert_eq!(old_summary.normal_files, 1);

        let build_tmp = root
            .join("tmpfs-build")
            .join(format!(".library.sqlite3.build.{}", std::process::id()));
        let initial_plan = SqliteBuildTempPlan {
            build_tmp_path: build_tmp,
            final_tmp_path: sqlite_temp_path(&db),
            source: SqliteBuildTempSource::DefaultTmpfs,
        };
        let mut writer = |_path: &Path,
                          _scan: &LibraryScan,
                          _progress: &mut ProgressCallback<'_>|
         -> Result<(), String> {
            Err("insert launch target: UNIQUE constraint failed".to_string())
        };
        let err = save_sqlite_scan_with_progress_using_writer(
            &db,
            &sqlite_scan_with_normal_files(&["/new/game.mra"]),
            None,
            initial_plan,
            &mut writer,
        )
        .expect_err("logical import error should fail temp import");

        assert!(
            err.contains("insert launch target"),
            "unexpected error: {err}"
        );
        let still_old = sqlite_cached_summary(&db, 0).expect("old database survived failed import");
        assert_eq!(still_old.normal_files, 1);
        assert!(
            !sqlite_temp_path(&db).exists(),
            "failed temp database should be cleaned up"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_previous_database_baselines_discoveries_without_new_badges() {
        let root = unique_temp_dir("sqlite-discovery-first-scan");
        let db = root.join("library.sqlite3");

        save_sqlite_scan(
            &db,
            &sqlite_scan_with_discoveries(vec![mra_discovery(1, "Baseline")]),
        )
        .expect("write first catalog");

        assert_eq!(discovered_at_for_title(&db, "Baseline"), None);
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");
        assert_eq!(loaded.catalog.games.len(), 1);
        assert!(!loaded.catalog.games[0].is_new);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn schema31_previous_database_seeds_baseline_and_marks_new_games() {
        let root = unique_temp_dir("sqlite-discovery-schema31");
        let db = root.join("library.sqlite3");
        write_schema31_games_fixture(&db, &["mra:set:game00001"]);
        let mut scan =
            sqlite_scan_with_discoveries(vec![mra_discovery(1, "Known"), mra_discovery(2, "New")]);
        scan.scanned_at_unix = 12_345;

        save_sqlite_scan(&db, &scan).expect("write catalog from schema31 history");

        assert_eq!(discovered_at_for_title(&db, "Known"), None);
        assert_eq!(discovered_at_for_title(&db, "New"), Some(12_345));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn schema32_previous_database_preserves_timestamps_and_marks_new_games() {
        let root = unique_temp_dir("sqlite-discovery-schema32");
        let db = root.join("library.sqlite3");
        write_schema32_games_fixture(&db, &[("mra:set:game00001", Some(111))]);
        let mut scan =
            sqlite_scan_with_discoveries(vec![mra_discovery(1, "Known"), mra_discovery(2, "New")]);
        scan.scanned_at_unix = 222;

        save_sqlite_scan(&db, &scan).expect("write catalog from schema32 history");

        assert_eq!(discovered_at_for_title(&db, "Known"), Some(111));
        assert_eq!(discovered_at_for_title(&db, "New"), Some(222));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn is_new_discovery_uses_fourteen_day_cutoff() {
        let now = 1_000_000;

        assert!(is_new_discovery(Some(now), now));
        assert!(is_new_discovery(Some(now - NEW_GAME_BADGE_SECS), now));
        assert!(!is_new_discovery(Some(now - NEW_GAME_BADGE_SECS - 1), now));
        assert!(!is_new_discovery(Some(now + 1), now));
        assert!(!is_new_discovery(None, now));
    }

    #[test]
    fn sqlite_catalog_stamp_check_detects_match_and_root_change() {
        let root = unique_temp_dir("sqlite-catalog-stamp-check");
        let db = root.join("library.sqlite3");
        let games = root.join("games");
        let system = games.join("NES");
        std::fs::create_dir_all(&system).expect("create system dir");
        set_file_mtime_for_test(&games, 10, 0);
        let cfg = BenchConfig {
            roots: vec![games.display().to_string()],
            sqlite_path: db.clone(),
        };
        let artifact = scan_library_artifact(&cfg, None);
        save_scan_artifact_to_sqlite(&cfg, artifact, None).expect("save artifact");

        let unchanged = sqlite_catalog_stamp_check(&cfg).expect("check unchanged stamp");
        assert!(unchanged.unchanged);
        assert!(unchanged.stored_checkpoint_fingerprint.is_some());
        assert_eq!(
            unchanged.stored_checkpoint_fingerprint,
            Some(unchanged.current_checkpoint_fingerprint.clone())
        );
        assert!(unchanged.current_checkpoint_lines > 0);
        let summary = sqlite_cached_summary(&db, unchanged.check_us).expect("cached summary");
        assert!(summary.skipped);

        set_file_mtime_for_test(&games, 20, 0);
        let changed = sqlite_catalog_stamp_check(&cfg).expect("check changed stamp");

        assert!(!changed.unchanged);
        assert_ne!(
            changed.stored_fingerprint,
            Some(changed.current_fingerprint)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn catalog_state_stamp_check_does_not_need_the_library_database() {
        let root = unique_temp_dir("catalog-state-stamp-check");
        let db = root.join("library.sqlite3");
        let state_path = root.join("catalog-v3/state/catalog-state.sqlite3");
        let games = root.join("games");
        std::fs::create_dir_all(games.join("NES")).expect("create system dir");
        let cfg = BenchConfig {
            roots: vec![games.display().to_string()],
            sqlite_path: db.clone(),
        };
        let artifact = scan_library_artifact(&cfg, None);
        save_scan_artifact_to_sqlite(&cfg, artifact, None).expect("save artifact");
        let state = crate::catalog_state::read_legacy(&db).expect("read migration state");
        crate::catalog_state::write(&state_path, &state).expect("write catalog state");
        std::fs::remove_file(&db).expect("remove legacy library database");

        let unchanged = catalog_state_stamp_check(&cfg, &state_path)
            .expect("check state without legacy database");

        assert!(unchanged.unchanged);
        assert!(!db.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_catalog_stamp_check_detects_nested_directory_change() {
        let root = unique_temp_dir("sqlite-catalog-stamp-nested-change");
        let db = root.join("library.sqlite3");
        let games = root.join("games");
        let nested = games.join("NES/Action");
        std::fs::create_dir_all(&nested).expect("create nested system dir");
        std::fs::write(nested.join("First.nes"), b"first").expect("write initial game");
        let cfg = BenchConfig {
            roots: vec![games.display().to_string()],
            sqlite_path: db.clone(),
        };
        let artifact = scan_library_artifact(&cfg, None);
        save_scan_artifact_to_sqlite(&cfg, artifact, None).expect("save artifact");
        assert!(
            sqlite_catalog_stamp_check(&cfg)
                .expect("check unchanged stamp")
                .unchanged
        );

        std::fs::remove_file(nested.join("First.nes")).expect("remove nested game");
        let changed = sqlite_catalog_stamp_check(&cfg).expect("check nested change");

        assert!(!changed.unchanged);
        assert!(changed.drift.changed_game_dirs > 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_catalog_stamp_check_detects_missing_checkpoint() {
        let root = unique_temp_dir("sqlite-catalog-missing-checkpoint");
        let db = root.join("library.sqlite3");
        let games = root.join("games");
        std::fs::create_dir_all(games.join("NES")).expect("create system dir");
        let cfg = BenchConfig {
            roots: vec![games.display().to_string()],
            sqlite_path: db.clone(),
        };
        let artifact = scan_library_artifact(&cfg, None);
        save_scan_artifact_to_sqlite(&cfg, artifact, None).expect("save artifact");
        let conn = Connection::open(&db).expect("open db");
        conn.execute("DROP TABLE catalog_discovery_checkpoint", [])
            .expect("drop checkpoint");
        drop(conn);

        let changed = sqlite_catalog_stamp_check(&cfg).expect("check missing checkpoint");

        assert!(!changed.unchanged);
        assert!(changed.stored_checkpoint_fingerprint.is_none());
        assert_eq!(changed.drift.detail, "checkpoint missing");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_catalog_stamp_check_detects_installed_known_core_change() {
        let root = unique_temp_dir("sqlite-catalog-stamp-known-core");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };
        let artifact = scan_library_artifact(&cfg, None);
        save_scan_artifact_to_sqlite(&cfg, artifact, None).expect("save artifact");

        install_test_console_core(&root, "ColecoVision");

        let changed = sqlite_catalog_stamp_check(&cfg).expect("check changed stamp");

        assert!(!changed.unchanged);
        assert_ne!(
            changed.stored_fingerprint,
            Some(changed.current_fingerprint)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_catalog_stamp_check_detects_installed_unknown_core_change() {
        let root = unique_temp_dir("sqlite-catalog-stamp-unknown-core");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };
        let artifact = scan_library_artifact(&cfg, None);
        save_scan_artifact_to_sqlite(&cfg, artifact, None).expect("save artifact");

        install_test_console_core(&root, "ChannelF");

        let changed = sqlite_catalog_stamp_check(&cfg).expect("check changed stamp");

        assert!(!changed.unchanged);
        assert_ne!(
            changed.stored_fingerprint,
            Some(changed.current_fingerprint)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn generated_profile_populates_sqlite_profiles_games_launch_targets_and_audit() {
        let root = unique_temp_dir("sqlite-generated-profile-e2e");
        install_test_console_core(&root, "ColecoVision");
        let coleco_dir = root.join("games/ColecoVision");
        std::fs::create_dir_all(&coleco_dir).expect("create colecovision dir");
        std::fs::write(coleco_dir.join("Mouse Trap.col"), b"rom").expect("write coleco rom");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };
        let artifact = scan_library_artifact(&cfg, None);
        assert!(
            artifact
                .scan
                .profiles
                .iter()
                .any(|profile| profile.id == "colecovision")
        );

        save_scan_artifact_to_sqlite(&cfg, artifact, None).expect("save artifact");

        let conn = open_sqlite_read_only(&db).expect("open sqlite");
        let profile_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM profiles
                 WHERE profile_id='colecovision'
                   AND system_id='colecovision'
                   AND source_kind='main-source'",
                [],
                |row| row.get(0),
            )
            .expect("query profiles");
        let game_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM games WHERE system_id='colecovision'",
                [],
                |row| row.get(0),
            )
            .expect("query games");
        let target_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM launch_targets
                 WHERE profile_id='colecovision'
                   AND core_id='ColecoVision'
                   AND mount_kind='load-file'
                   AND mount_index=1",
                [],
                |row| row.get(0),
            )
            .expect("query launch targets");
        let audit_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM catalog_audit
                 WHERE core_id='ColecoVision'
                   AND expected_game_dir='games/ColecoVision'
                   AND catalog_status='cataloged'",
                [],
                |row| row.get(0),
            )
            .expect("query audit");

        assert_eq!(profile_rows, 1);
        assert_eq!(game_rows, 1);
        assert_eq!(target_rows, 1);
        assert_eq!(audit_rows, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_build_temp_defaults_to_tmpfs_for_media_fat_database() {
        let path = Path::new("/media/fat/mister-magik/library.sqlite3");
        let plan = sqlite_build_temp_plan_for(path, None);

        assert_eq!(plan.source, SqliteBuildTempSource::DefaultTmpfs);
        assert!(
            plan.build_tmp_path
                .starts_with(Path::new(DEFAULT_SQLITE_BUILD_DIR))
        );
        let expected_name = format!(".library.sqlite3.build.{}", std::process::id());
        assert_eq!(
            plan.build_tmp_path
                .file_name()
                .and_then(|name| name.to_str()),
            Some(expected_name.as_str())
        );
        assert_eq!(
            plan.final_tmp_path,
            PathBuf::from(format!(
                "/media/fat/mister-magik/.library.sqlite3.tmp.{}",
                std::process::id()
            ))
        );
    }

    #[test]
    fn sqlite_build_temp_env_override_wins_for_media_fat_database() {
        let override_dir = Path::new("/custom/sqlite-build");
        let path = Path::new("/media/fat/mister-magik/library.sqlite3");
        let plan = sqlite_build_temp_plan_for(path, Some(override_dir));

        assert_eq!(plan.source, SqliteBuildTempSource::EnvOverride);
        assert!(plan.build_tmp_path.starts_with(override_dir));
        assert_ne!(plan.build_tmp_path, plan.final_tmp_path);
    }

    #[test]
    fn sqlite_build_temp_stays_beside_non_media_fat_database() {
        let root = unique_temp_dir("sqlite-build-host-path");
        let db = root.join("library.sqlite3");
        let plan = sqlite_build_temp_plan_for(&db, None);

        assert_eq!(plan.source, SqliteBuildTempSource::BesideFinal);
        assert_eq!(plan.build_tmp_path, sqlite_temp_path(&db));
        assert_eq!(plan.build_tmp_path, plan.final_tmp_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_save_retries_beside_final_after_tmpfs_filesystem_error() {
        let root = unique_temp_dir("sqlite-build-fallback");
        let db = root.join("library.sqlite3");
        let build_tmp = root
            .join("tmpfs-build")
            .join(format!(".library.sqlite3.build.{}", std::process::id()));
        let initial_plan = SqliteBuildTempPlan {
            build_tmp_path: build_tmp.clone(),
            final_tmp_path: sqlite_temp_path(&db),
            source: SqliteBuildTempSource::DefaultTmpfs,
        };
        let mut attempts = Vec::<PathBuf>::new();
        let mut writer = |path: &Path,
                          _scan: &LibraryScan,
                          _progress: &mut ProgressCallback<'_>|
         -> Result<(), String> {
            attempts.push(path.to_path_buf());
            if path == build_tmp {
                return Err("database or disk is full".to_string());
            }
            std::fs::write(path, b"fallback-db").map_err(|e| e.to_string())
        };

        let bytes = save_sqlite_scan_with_progress_using_writer(
            &db,
            &sqlite_scan_with_normal_files(&[]),
            None,
            initial_plan,
            &mut writer,
        )
        .expect("fallback save");

        assert_eq!(bytes, b"fallback-db".len() as u64);
        assert_eq!(attempts, vec![build_tmp, sqlite_temp_path(&db)]);
        assert_eq!(
            std::fs::read(&db).expect("read fallback db"),
            b"fallback-db"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_save_does_not_retry_logical_import_error() {
        let root = unique_temp_dir("sqlite-build-no-logical-retry");
        let db = root.join("library.sqlite3");
        let build_tmp = root
            .join("tmpfs-build")
            .join(format!(".library.sqlite3.build.{}", std::process::id()));
        let initial_plan = SqliteBuildTempPlan {
            build_tmp_path: build_tmp.clone(),
            final_tmp_path: sqlite_temp_path(&db),
            source: SqliteBuildTempSource::DefaultTmpfs,
        };
        let mut attempts = 0usize;
        let mut writer = |_path: &Path,
                          _scan: &LibraryScan,
                          _progress: &mut ProgressCallback<'_>|
         -> Result<(), String> {
            attempts += 1;
            Err("insert launch target: UNIQUE constraint failed".to_string())
        };

        let err = save_sqlite_scan_with_progress_using_writer(
            &db,
            &sqlite_scan_with_normal_files(&[]),
            None,
            initial_plan,
            &mut writer,
        )
        .expect_err("logical import error should not retry");

        assert!(
            err.contains("insert launch target"),
            "unexpected error: {err}"
        );
        assert_eq!(attempts, 1);
        assert!(!db.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_save_does_not_retry_explicit_build_dir_failure() {
        let root = unique_temp_dir("sqlite-build-env-no-retry");
        let db = root.join("library.sqlite3");
        let build_tmp = root
            .join("explicit-build")
            .join(format!(".library.sqlite3.build.{}", std::process::id()));
        let initial_plan = SqliteBuildTempPlan {
            build_tmp_path: build_tmp,
            final_tmp_path: sqlite_temp_path(&db),
            source: SqliteBuildTempSource::EnvOverride,
        };
        let mut attempts = 0usize;
        let mut writer = |_path: &Path,
                          _scan: &LibraryScan,
                          _progress: &mut ProgressCallback<'_>|
         -> Result<(), String> {
            attempts += 1;
            Err("database or disk is full".to_string())
        };

        let err = save_sqlite_scan_with_progress_using_writer(
            &db,
            &sqlite_scan_with_normal_files(&[]),
            None,
            initial_plan,
            &mut writer,
        )
        .expect_err("explicit build dir failure should not retry");

        assert!(
            err.contains("database or disk is full"),
            "unexpected error: {err}"
        );
        assert_eq!(attempts, 1);
        assert!(!db.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_arcade_load_returns_launchables_beyond_old_cap() {
        const ROWS: usize = 20_005;
        let root = unique_temp_dir("sqlite-arcade-no-cap");
        let db = root.join("library.sqlite3");
        let discoveries = (0..ROWS)
            .map(|idx| mra_discovery(idx, &format!("Game {idx:05}")))
            .collect::<Vec<_>>();
        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(discoveries))
            .expect("write large arcade database");

        let loaded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load arcade catalog");

        assert_eq!(loaded.rows, ROWS);
        assert_eq!(loaded.catalog.games.len(), ROWS);
        assert!(
            loaded
                .catalog
                .games
                .iter()
                .any(|game| game.title.as_ref() == "Game 20004")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_save_persists_catalog_audit_rows() {
        let root = unique_temp_dir("sqlite-audit-rows");
        let db = root.join("library.sqlite3");
        let unknown = root.join("games/ChannelF");
        std::fs::create_dir_all(&unknown).expect("create unknown dir");
        write_stored_zip(
            &unknown.join("Packed ChannelF Games.zip"),
            &[("Alien Invasion.chf", b"rom")],
        );
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let artifact = scan_library_artifact(&cfg, None);
        assert!(artifact.scan.audit_rows.iter().any(|row| {
            row.expected_game_dir == "games/ChannelF" && row.catalog_status == "uncataloged"
        }));
        save_scan_artifact_to_sqlite(&cfg, artifact, None).expect("save artifact");

        let conn = open_sqlite_read_only(&db).expect("open sqlite");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM catalog_audit WHERE expected_game_dir='games/ChannelF' AND catalog_status='uncataloged'",
                [],
                |row| row.get(0),
            )
            .expect("query audit count");
        assert_eq!(count, 1);
        let meta_count: i64 = conn
            .query_row("SELECT value FROM meta WHERE key='audit_rows'", [], |row| {
                row.get(0)
            })
            .expect("query audit meta");
        assert!(meta_count >= 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn catalog_summary_publish_matches_sqlite_counts() {
        let root = unique_temp_dir("sqlite-catalog-summary");
        let db = root.join("library.sqlite3");
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\ttest".to_string(),
            "root\t0\tfixture".to_string(),
        ]);
        let saved = save_sqlite_scan_with_progress_and_stamp_and_catalog(
            &db,
            &sqlite_scan_with_discoveries(vec![
                mra_discovery(1, "Summary Alpha"),
                mra_discovery(2, "Summary Beta"),
            ]),
            Some(&stamp),
            "/media/fat/_Arcade",
            None,
        )
        .expect("write catalog and summary");

        let summary_path = catalog_summary::summary_path_for_sqlite(&db);
        let navigation_path = catalog_navigation::navigation_path_for_sqlite(&db);
        assert!(summary_path.exists(), "summary should be published");
        assert!(navigation_path.exists(), "navigation should be published");
        assert!(
            !root.join(".library.summary.json.tmp").exists(),
            "summary temp should not remain after successful publish"
        );

        let summary = catalog_summary::read_catalog_summary(&summary_path)
            .expect("read summary")
            .expect("current summary");
        let stored_stamp = read_sqlite_catalog_stamp(&db)
            .expect("read sqlite stamp")
            .expect("stored sqlite stamp");
        let navigation =
            catalog_navigation::read_catalog_navigation_projection(&navigation_path, &stamp)
                .expect("read navigation")
                .expect("current navigation");
        let navigation_catalog =
            ArcadeCatalog::from_navigation_projection("/media/fat/_Arcade", navigation.clone());
        let loaded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load sqlite catalog");

        assert_eq!(saved.catalog.rows, loaded.rows);
        assert_eq!(saved.catalog.catalog.len(), loaded.catalog.len());
        assert_eq!(saved.catalog.catalog.systems, loaded.catalog.systems);
        assert_eq!(navigation.games.len(), loaded.catalog.games.len());
        assert_eq!(navigation_catalog.games.len(), loaded.catalog.games.len());
        assert_eq!(navigation_catalog.systems, loaded.catalog.systems);
        assert_eq!(summary.catalog_stamp_fingerprint, stamp.fingerprint_hex());
        assert_eq!(summary.catalog_generation, stamp.fingerprint_hex());
        assert_eq!(summary.catalog_stamp_lines, stamp.lines());
        assert_eq!(stored_stamp, stamp);
        assert_eq!(summary.total_game_count, saved.catalog.catalog.games.len());
        assert_eq!(
            summary.hot_games.len(),
            saved.catalog.catalog.system_game_count("arcade")
        );
        assert!(
            !summary.hot_games.is_empty(),
            "warm summary must include Arcade hot rows"
        );
        assert_summary_matches_sqlite_catalog(&summary, &loaded.catalog);
        assert_navigation_catalog_matches_sqlite(&loaded.catalog, &navigation_catalog);
        assert_eq!(summary.systems.len(), saved.catalog.catalog.systems.len());
        for (summary_system, sqlite_system) in summary.systems.iter().zip(&loaded.catalog.systems) {
            assert_eq!(summary_system.id, sqlite_system.id);
            assert_eq!(summary_system.title, sqlite_system.title);
            assert_eq!(summary_system.count, sqlite_system.count);
            assert_eq!(
                summary_system.platform_kind,
                loaded.catalog.platform_kind(&sqlite_system.id)
            );
        }
        let arcade = summary
            .systems
            .iter()
            .find(|system| system.id == "arcade")
            .expect("arcade summary system");
        assert_eq!(arcade.platform_kind, PlatformKind::Arcade);
        assert_eq!(arcade.supported_media, vec!["screenshots".to_string()]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ComparableCatalogGame {
        title: String,
        launch_ref: String,
        preview_archive_path: String,
        preview_asset_key: String,
        has_preview: bool,
        system_id: String,
        year: Option<u16>,
        manufacturer: String,
        players: Option<u8>,
        control: String,
        is_new: bool,
    }

    fn comparable_catalog_games(catalog: &ArcadeCatalog) -> Vec<ComparableCatalogGame> {
        catalog
            .games
            .iter()
            .map(|game| ComparableCatalogGame {
                title: game.title.to_string(),
                launch_ref: game.mra_path.to_string(),
                preview_archive_path: game.preview_archive_path.to_string(),
                preview_asset_key: game.preview_asset_key.to_string(),
                has_preview: game.has_preview,
                system_id: game.system_id.to_string(),
                year: game.year,
                manufacturer: game.manufacturer.to_string(),
                players: game.players,
                control: game.control.to_string(),
                is_new: game.is_new,
            })
            .collect()
    }

    fn catalog_titles_for_system(catalog: &ArcadeCatalog, system_id: &str) -> Vec<String> {
        catalog
            .system_game_view(system_id)
            .iter()
            .map(|game| game.title.to_string())
            .collect()
    }

    fn assert_summary_matches_sqlite_catalog(
        summary: &catalog_summary::CatalogSummaryProjection,
        sqlite_catalog: &ArcadeCatalog,
    ) {
        assert_eq!(summary.total_game_count, sqlite_catalog.games.len());
        let summary_arcade_titles = summary
            .hot_games
            .iter()
            .map(|game| game.title.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            summary_arcade_titles,
            catalog_titles_for_system(sqlite_catalog, "arcade"),
            "summary hot_games must match materialized Arcade catalog order"
        );

        let summary_arcade_refs = summary
            .hot_games
            .iter()
            .map(|game| game.launch_ref.clone())
            .collect::<Vec<_>>();
        let sqlite_arcade_refs = sqlite_catalog
            .system_game_view("arcade")
            .iter()
            .map(|game| game.mra_path.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            summary_arcade_refs, sqlite_arcade_refs,
            "summary hot_games must match materialized Arcade catalog membership"
        );
    }

    fn player_options_as_set(catalog: &ArcadeCatalog, system_id: &str) -> BTreeSet<String> {
        catalog
            .player_options(system_id)
            .into_iter()
            .map(|option| option.label)
            .collect()
    }

    fn assert_navigation_catalog_matches_sqlite(
        sqlite_catalog: &ArcadeCatalog,
        navigation_catalog: &ArcadeCatalog,
    ) {
        assert_eq!(navigation_catalog.len(), sqlite_catalog.len());
        assert_eq!(navigation_catalog.systems, sqlite_catalog.systems);
        assert_eq!(
            comparable_catalog_games(navigation_catalog),
            comparable_catalog_games(sqlite_catalog)
        );

        for system in &sqlite_catalog.systems {
            let sqlite_titles = catalog_titles_for_system(sqlite_catalog, &system.id);
            let navigation_titles = catalog_titles_for_system(navigation_catalog, &system.id);
            assert_eq!(
                navigation_titles, sqlite_titles,
                "navigation titles diverged for system {}",
                system.id
            );
            assert_eq!(navigation_titles.len(), system.count);
            assert_eq!(
                navigation_catalog.system_game_count(&system.id),
                sqlite_catalog.system_game_count(&system.id)
            );
            assert_eq!(
                navigation_catalog.system_preview_game_count(&system.id),
                sqlite_catalog.system_preview_game_count(&system.id)
            );
        }

        for game in sqlite_catalog.games.iter() {
            assert_eq!(
                navigation_catalog.title_for_path(game.mra_path.as_ref()),
                sqlite_catalog.title_for_path(game.mra_path.as_ref())
            );
            assert_eq!(
                navigation_catalog.launch_target_for_ref(game.mra_path.as_ref()),
                sqlite_catalog.launch_target_for_ref(game.mra_path.as_ref())
            );
        }
    }

    #[test]
    fn navigation_projection_publish_matches_materialized_sqlite_catalog() {
        let root = unique_temp_dir("sqlite-navigation-materialized-catalog");
        let db = root.join("library.sqlite3");
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\tnav-catalog".to_string(),
            "root\t0\tfixture".to_string(),
        ]);
        let mut saturn = saturn_payload("/media/fat/games/Saturn/Nights.chd");
        saturn.year = Some(1996);
        saturn.manufacturer = Some("Sega".to_string());
        saturn.genre = Some("Action".to_string());
        let mut snes = payload("/media/fat/games/SNES/F-Zero.sfc");
        snes.platform_id = "snes".to_string();
        snes.core_id = "SNES".to_string();
        snes.hardware_id = "snes".to_string();
        snes.category = "Console".to_string();
        snes.year = Some(1991);
        snes.manufacturer = Some("Nintendo".to_string());
        snes.genre = Some("Racing".to_string());

        save_sqlite_scan_with_progress_and_stamp_and_projections(
            &db,
            &sqlite_scan_with_discoveries(vec![
                mra_discovery(1, "Puck Man"),
                mra_discovery(2, "Phantasm (Japan)"),
                saturn,
                snes,
            ]),
            &stamp,
            Path::new("/media/fat/_Arcade"),
            None,
        )
        .expect("write catalog and materialized projections");

        let loaded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load sqlite catalog");
        let navigation_path = catalog_navigation::navigation_path_for_sqlite(&db);
        let navigation =
            catalog_navigation::read_catalog_navigation_projection(&navigation_path, &stamp)
                .expect("read navigation")
                .expect("current navigation");
        assert_eq!(navigation.games.len(), loaded.catalog.games.len());
        assert_eq!(navigation.systems.len(), loaded.catalog.systems.len());
        assert_eq!(
            loaded.catalog.platform_kind("saturn"),
            PlatformKind::Console,
            "SQLite systems.category must survive runtime hydration"
        );
        assert_eq!(
            navigation
                .systems
                .iter()
                .find(|system| system.id == "snes")
                .map(|system| system.platform_kind),
            Some(PlatformKind::Console)
        );
        let navigation_catalog =
            ArcadeCatalog::from_navigation_projection("/media/fat/_Arcade", navigation);

        assert_eq!(
            catalog_titles_for_system(&loaded.catalog, "arcade"),
            vec!["Phantasm (Japan)".to_string(), "Puck Man".to_string()]
        );
        assert_eq!(
            loaded.catalog.systems,
            vec![
                GameSystemEntry {
                    id: "arcade".to_string(),
                    title: "Arcade".to_string(),
                    count: 2,
                },
                GameSystemEntry {
                    id: "saturn".to_string(),
                    title: "Saturn".to_string(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "snes".to_string(),
                    title: "SNES".to_string(),
                    count: 1,
                },
            ]
        );
        assert_navigation_catalog_matches_sqlite(&loaded.catalog, &navigation_catalog);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn production_projection_embeds_exact_navigation_for_sqlite_recovery() {
        let root = unique_temp_dir("sqlite-embedded-navigation-recovery");
        let db = root.join("library.sqlite3");
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\tembedded-navigation".to_string(),
            "root\t0\tfixture".to_string(),
        ]);
        let arcade_path = "/media/fat/_Arcade/Canonical Alpha.mra";
        let saturn_path = "magik-plan:payload:/media/fat/games/Saturn/Nights.chd";
        let canonical_arcade_game = arcade_game("Canonical Alpha")
            .path(arcade_path)
            .preview("game00001")
            .year(1983)
            .manufacturer("Example")
            .build();
        let mut saturn_game = arcade_game("Nights into Dreams")
            .path(saturn_path)
            .preview("nights-into-dreams")
            .system_id("saturn")
            .year(1996)
            .manufacturer("Sega")
            .build();
        saturn_game.preview_archive_path =
            preview_worker::preview_archive_path_for_system("saturn").into();
        let games = vec![canonical_arcade_game, saturn_game];
        let systems = vec![
            GameSystemEntry {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 1,
            },
            GameSystemEntry {
                id: "saturn".to_string(),
                title: "Saturn".to_string(),
                count: 1,
            },
        ];
        let launch_plans = vec![StructuredLaunchPlan {
            launch_ref: saturn_path.into(),
            title: "Nights into Dreams".into(),
            system_id: "saturn".into(),
            core_path: "_Console/Saturn".into(),
            payload_path: "/media/fat/games/Saturn/Nights.chd".into(),
            mount_kind: "mount-image".into(),
            mount_index: 0,
            delay_secs: 1,
        }];
        let expected = ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
            PathBuf::from(arcade_catalog::DEFAULT_ARCADE_ROOT),
            games,
            systems,
            launch_plans,
            HashMap::from([
                ("arcade".to_string(), PlatformKind::Arcade),
                ("saturn".to_string(), PlatformKind::Console),
            ]),
        );

        let mut arcade = mra_discovery(1, "Canonical Alpha");
        arcade.year = Some(1983);
        arcade.manufacturer = Some("Example".to_string());
        arcade.genre = Some("Maze".to_string());
        let mut saturn = saturn_payload("/media/fat/games/Saturn/Nights.chd");
        saturn.title = "Nights into Dreams".to_string();
        saturn.year = Some(1996);
        saturn.manufacturer = Some("Sega".to_string());
        saturn.genre = Some("Action".to_string());
        save_sqlite_scan_with_progress_and_stamp_and_catalog_projection(
            &db,
            &sqlite_scan_with_discoveries(vec![arcade, saturn]),
            &stamp,
            &expected,
            None,
        )
        .expect("save production catalog projection");
        let conn = Connection::open(&db).expect("open production catalog");
        register_sqlite_catalog_functions(&conn)
            .expect("register production catalog SQL functions");
        assert_eq!(
            conn.query_row("SELECT count(*) FROM ui_arcade_preferred", [], |row| row
                .get::<_, i64>(0))
                .expect("count compatibility arcade rows"),
            1,
            "production publication must retain selector compatibility tables"
        );
        assert_eq!(
            conn.query_row(
                "SELECT (SELECT count(*) FROM ui_arcade_preferred) +
                        (SELECT count(*) FROM launcher_catalog_rows)",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count compatibility launcher rows"),
            2
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM launcher_launch_plans", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count canonical structured launch plans"),
            1
        );
        let compatibility_rows = conn
            .prepare(
                "SELECT title,launch_ref,system_id,preview_asset_key,has_preview,
                        COALESCE(year,-1),COALESCE(manufacturer,''),COALESCE(players,-1),
                        COALESCE(control,'')
                 FROM launcher_catalog_text ORDER BY ordinal",
            )
            .expect("prepare canonical compatibility row oracle")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })
            .expect("query canonical compatibility row oracle")
            .collect::<Result<Vec<_>, _>>()
            .expect("read canonical compatibility row oracle");
        let canonical_rows = expected
            .games
            .iter()
            .map(|game| {
                (
                    game.title.to_string(),
                    game.mra_path.to_string(),
                    game.system_id.to_string(),
                    game.preview_asset_key.to_string(),
                    i64::from(game.has_preview),
                    game.year.map_or(-1, i64::from),
                    game.manufacturer.to_string(),
                    game.players.map_or(-1, i64::from),
                    game.control.to_string(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(compatibility_rows, canonical_rows);
        assert!(
            load_embedded_catalog_navigation_with_limit(&conn, &stamp, 1)
                .expect("size-limited embedded navigation probe")
                .is_none(),
            "embedded BLOB length must be rejected before reading its payload"
        );
        drop(conn);
        std::fs::remove_file(catalog_summary::summary_path_for_sqlite(&db))
            .expect("remove summary sidecar");
        std::fs::remove_file(catalog_navigation::navigation_path_for_sqlite(&db))
            .expect("remove navigation sidecar");

        let recovered =
            load_arcade_catalog_from_sqlite_at(arcade_catalog::DEFAULT_ARCADE_ROOT, &db)
                .expect("recover embedded canonical navigation");

        assert!(recovered.projection_repair_safe);
        assert_navigation_catalog_matches_sqlite(&expected, &recovered.catalog);
        assert_eq!(
            recovered.catalog.platform_kind("arcade"),
            PlatformKind::Arcade
        );
        assert_eq!(
            recovered.catalog.platform_kind("saturn"),
            PlatformKind::Console
        );

        let conn = Connection::open(&db).expect("open embedded projection for corruption");
        conn.execute(
            "UPDATE catalog_navigation_projection SET bytes=?1 WHERE id=0",
            [b"not-an-lz4-navigation".as_slice()],
        )
        .expect("corrupt embedded projection");
        drop(conn);
        let compatibility_fallback =
            load_arcade_catalog_from_sqlite_at(arcade_catalog::DEFAULT_ARCADE_ROOT, &db)
                .expect("fall through corrupt embedded projection");
        assert!(compatibility_fallback.projection_repair_safe);
        assert_navigation_catalog_matches_sqlite(&expected, &compatibility_fallback.catalog);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn canonical_compatibility_rows_keep_shared_launch_refs_distinct() {
        let root = unique_temp_dir("sqlite-canonical-shared-launch-ref");
        let db = root.join("library.sqlite3");
        let shared_ref = "magik-amigavision:shared-collection";
        let mut agony = payload("/media/fat/games/Amiga/AmigaVision.hdf::Agony");
        agony.source_kind = DiscoverySourceKind::CatalogEntry;
        agony.launch_ref = shared_ref.to_string();
        agony.title = "Agony".to_string();
        agony.platform_id = "amiga".to_string();
        agony.core_id = "Minimig".to_string();
        agony.hardware_id = "amiga".to_string();
        agony.genre = Some("AmigaVision".to_string());
        let mut alien_breed = agony.clone();
        alien_breed.source_path = "/media/fat/games/Amiga/AmigaVision.hdf::Alien Breed".to_string();
        alien_breed.title = "Alien Breed".to_string();

        let games = vec![
            arcade_game("Agony")
                .path(shared_ref)
                .system_id("amiga")
                .build(),
            arcade_game("Alien Breed")
                .path(shared_ref)
                .system_id("amiga")
                .build(),
        ];
        let expected = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from(arcade_catalog::DEFAULT_ARCADE_ROOT),
            games,
            vec![GameSystemEntry {
                id: "amiga".to_string(),
                title: "Amiga".to_string(),
                count: 2,
            }],
            Vec::new(),
        );
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\tshared-launch-ref".to_string(),
        ]);

        save_sqlite_scan_with_progress_and_stamp_and_catalog_projection(
            &db,
            &sqlite_scan_with_discoveries(vec![agony, alien_breed]),
            &stamp,
            &expected,
            None,
        )
        .expect("save shared-ref canonical projection");

        let conn = Connection::open(&db).expect("open shared-ref catalog");
        register_sqlite_catalog_functions(&conn).expect("register shared-ref SQL functions");
        let rows = conn
            .prepare("SELECT title,launch_id FROM launcher_catalog_text ORDER BY ordinal")
            .expect("prepare shared-ref compatibility query")
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .expect("query shared-ref compatibility rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("read shared-ref compatibility rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "Agony");
        assert_eq!(rows[1].0, "Alien Breed");
        assert_ne!(rows[0].1, rows[1].1);
        conn.execute(
            "UPDATE catalog_navigation_projection SET bytes=?1 WHERE id=0",
            [b"corrupt".as_slice()],
        )
        .expect("corrupt embedded projection");
        drop(conn);

        let compatibility =
            load_arcade_catalog_from_sqlite_at(arcade_catalog::DEFAULT_ARCADE_ROOT, &db)
                .expect("load shared-ref compatibility projection");
        assert_navigation_catalog_matches_sqlite(&expected, &compatibility.catalog);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn canonical_compatibility_rejects_mismatched_scan_generation() {
        let root = unique_temp_dir("sqlite-canonical-generation-mismatch");
        let db = root.join("library.sqlite3");
        let missing_ref = "magik-plan:payload:/media/fat/games/Saturn/Missing.chd";
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from(arcade_catalog::DEFAULT_ARCADE_ROOT),
            vec![
                arcade_game("Missing")
                    .path(missing_ref)
                    .system_id("saturn")
                    .build(),
            ],
            vec![GameSystemEntry {
                id: "saturn".to_string(),
                title: "Saturn".to_string(),
                count: 1,
            }],
            Vec::new(),
        );
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\tgeneration-mismatch".to_string(),
        ]);

        let error = save_sqlite_scan_with_progress_and_stamp_and_catalog_projection(
            &db,
            &sqlite_scan_with_discoveries(Vec::new()),
            &stamp,
            &catalog,
            None,
        )
        .expect_err("mismatched canonical generation must be rejected");

        assert!(error.contains("canonical launcher row has no source launch id"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn canonical_arcade_preferred_row_follows_ram_catalog_selection() {
        let root = unique_temp_dir("sqlite-canonical-arcade-preferred");
        let db = root.join("library.sqlite3");
        let mut first = mra_discovery(1, "1942 (First Version)");
        first.setname = Some("1942b".to_string());
        first.parent = Some("1942".to_string());
        let mut world = mra_discovery(2, "1942 (World)");
        world.setname = Some("1942w".to_string());
        world.parent = Some("1942".to_string());
        let world_ref = world.launch_ref.clone();
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from(arcade_catalog::DEFAULT_ARCADE_ROOT),
            vec![
                arcade_game("1942 (World)")
                    .path(&world_ref)
                    .preview("1942")
                    .build(),
            ],
            vec![GameSystemEntry {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 1,
            }],
            Vec::new(),
        );
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\tcanonical-arcade-preferred".to_string(),
        ]);

        save_sqlite_scan_with_progress_and_stamp_and_catalog_projection(
            &db,
            &sqlite_scan_with_discoveries(vec![first, world]),
            &stamp,
            &catalog,
            None,
        )
        .expect("save canonical Arcade preferred projection");

        let conn = Connection::open(&db).expect("open canonical Arcade preferred projection");
        register_sqlite_catalog_functions(&conn)
            .expect("register canonical Arcade preferred SQL functions");
        let preferred: (String, String, i64) = conn
            .query_row(
                "SELECT title,family_id,has_preview FROM ui_arcade_preferred_text",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("query canonical Arcade preferred row");
        assert_eq!(
            preferred,
            ("1942 (World)".to_string(), "1942".to_string(), 1)
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM ui_arcade_variants", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count canonical Arcade variants"),
            2
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM ui_arcade_variants WHERE preferred=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count canonical Arcade preferred flags"),
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn joined_sqlite_fallback_is_marked_unsafe_for_projection_repair() {
        let root = unique_temp_dir("sqlite-degraded-joined-fallback");
        let db = root.join("library.sqlite3");
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\tjoined-fallback".to_string(),
        ]);
        let scan = sqlite_scan_with_discoveries(vec![mra_discovery(1, "Fallback Alpha")]);
        write_sqlite_scan_without_catalog_rebuild(
            &db,
            &scan,
            None,
            SoftwareHashCache::load(&db),
            None,
            Some(&stamp),
            None,
            None,
            false,
        )
        .expect("write source-fact-only sqlite");

        let loaded = load_arcade_catalog_from_sqlite_at(arcade_catalog::DEFAULT_ARCADE_ROOT, &db)
            .expect("load joined fallback");

        assert!(!loaded.projection_repair_safe);
        assert!(loaded.catalog.games.iter().all(|game| !game.has_preview));
        let repair_error = library_db::rewrite_catalog_projections_from_sqlite(
            arcade_catalog::DEFAULT_ARCADE_ROOT,
            &db,
        )
        .expect_err("degraded joined fallback must not be republished");
        assert!(repair_error.contains("refusing to rewrite catalog projections"));
        assert!(!catalog_summary::summary_path_for_sqlite(&db).exists());
        assert!(!catalog_navigation::navigation_path_for_sqlite(&db).exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn canonical_arcade_preview_uses_mra_parent_when_setname_is_missing() {
        let root = unique_temp_dir("sqlite-canonical-arcade-parent-preview");
        let db = root.join("library.sqlite3");
        let path = "/media/fat/_Arcade/Pac-Manic Miner.mra";
        let mut discovery = mra_discovery(1, "Pac-Manic Miner");
        discovery.setname = None;
        discovery.parent = Some("puckman".to_string());
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from(arcade_catalog::DEFAULT_ARCADE_ROOT),
            vec![
                arcade_game("Pac-Manic Miner")
                    .path(path)
                    .preview("puckman")
                    .build(),
            ],
            vec![GameSystemEntry {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 1,
            }],
            Vec::new(),
        );
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\tcanonical-parent-preview".to_string(),
        ]);

        save_sqlite_scan_with_progress_and_stamp_and_catalog_projection(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &stamp,
            &catalog,
            None,
        )
        .expect("save MRA parent-derived canonical preview");

        let conn = Connection::open(&db).expect("open MRA parent-derived canonical preview");
        register_sqlite_catalog_functions(&conn)
            .expect("register MRA parent-derived canonical preview functions");
        let preview: (String, i64) = conn
            .query_row(
                "SELECT preview_asset_key,has_preview FROM ui_arcade_preferred_text",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query MRA parent-derived canonical preview");
        assert_eq!(preview, ("puckman".to_string(), 1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn meta_discovery_count_matches_inserted_preferred_set_semantics() {
        let root = unique_temp_dir("sqlite-meta-retained-discovery-count");
        let db = root.join("library.sqlite3");
        let mut first_variant = mra_discovery(1, "Shared Family (World)");
        first_variant.setname = Some("shared-family".to_string());
        let mut second_variant = mra_discovery(2, "Shared Family (Japan)");
        second_variant.setname = Some("shared-family".to_string());
        let payload_path = "/media/fat/games/NES/Covered.nes";
        let mut launcher = mgl(
            "/media/fat/_Console/Covered.mgl",
            "/media/fat/_Console/Covered.mgl",
        );
        launcher.platform_id = "nes".to_string();
        launcher.covered_payload_path = Some(payload_path.to_string());
        let mut covered_payload = payload(payload_path);
        covered_payload.platform_id = "nes".to_string();
        let scan = sqlite_scan_with_discoveries(vec![
            first_variant,
            second_variant,
            launcher,
            covered_payload,
        ]);
        let expected = unique_discovery_count(&scan.discoveries);
        assert_eq!(expected, 2, "fixture must exercise collapse and coverage");

        save_sqlite_scan(&db, &scan).expect("save retained discovery count fixture");

        let conn = Connection::open(&db).expect("open retained discovery count fixture");
        let stored = conn
            .query_row(
                "SELECT value FROM meta WHERE key='discoveries'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("query retained discovery count");
        let inserted = conn
            .query_row("SELECT count(*) FROM game_rows", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count inserted preferred discoveries");
        assert_eq!(stored, expected as i64);
        assert_eq!(stored, inserted);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_checkpoint_persistence_reuses_retained_scan_facts() {
        let root = unique_temp_dir("sqlite-checkpoint-retained-facts");
        std::fs::create_dir_all(root.join("games/NES")).expect("create initial game dir");
        std::fs::write(root.join("games/NES/Mario.nes"), b"nes").expect("write initial game");
        let mut scan = sqlite_scan_with_discoveries(Vec::new());
        scan.roots = vec![root.display().to_string()];
        scan.installed_cores = catalog_discovery::installed_cores_for_roots(&scan.roots);
        scan.game_dir_facts = catalog_discovery::top_level_game_dirs_for_roots(&scan.roots);

        std::fs::create_dir_all(root.join("games/SNES")).expect("create post-scan game dir");
        std::fs::write(root.join("games/SNES/F-Zero.sfc"), b"snes").expect("write post-scan game");
        let expected = catalog_checkpoint::compute_catalog_discovery_checkpoint_from_facts(
            &scan.roots,
            &default_mame_sqlite_path(),
            &default_hbmame_sqlite_path(),
            &scan.audit_rows,
            &scan.installed_cores,
            &scan.game_dir_facts,
        );
        let db = root.with_extension("sqlite3");
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\tretained-checkpoint-facts".to_string(),
        ]);
        write_sqlite_scan_without_catalog_rebuild(
            &db,
            &scan,
            None,
            SoftwareHashCache::load(&db),
            None,
            Some(&stamp),
            None,
            None,
            false,
        )
        .expect("write retained-fact checkpoint");

        let conn = Connection::open(&db).expect("open retained-fact checkpoint");
        let stored = catalog_store::read_catalog_discovery_checkpoint(&conn)
            .expect("read retained-fact checkpoint")
            .expect("stored retained-fact checkpoint");
        assert_eq!(stored, expected);
        let live = catalog_checkpoint::compute_catalog_discovery_checkpoint(
            &scan.roots,
            &default_mame_sqlite_path(),
            &default_hbmame_sqlite_path(),
            &scan.audit_rows,
        );
        assert_ne!(stored, live, "writer must not rediscover post-scan facts");
        let _ = std::fs::remove_file(db);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn projection_publish_failure_preserves_atomic_sidecar_files() {
        let root = unique_temp_dir("sqlite-projection-publish-failure");
        let db = root.join("library.sqlite3");
        let old_stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\told".to_string(),
            "root\t0\told".to_string(),
        ]);
        save_sqlite_scan_with_progress_and_stamp_and_catalog(
            &db,
            &sqlite_scan_with_discoveries(vec![mra_discovery(1, "Old Alpha")]),
            Some(&old_stamp),
            "/media/fat/_Arcade",
            None,
        )
        .expect("write old catalog and projections");

        let summary_path = catalog_summary::summary_path_for_sqlite(&db);
        let navigation_path = catalog_navigation::navigation_path_for_sqlite(&db);
        assert!(summary_path.exists(), "old summary fixture");
        assert!(navigation_path.exists(), "old navigation fixture");
        std::fs::create_dir(root.join(".library.summary.json.tmp"))
            .expect("block summary temp creation");

        let new_scan = sqlite_scan_with_discoveries(vec![
            mra_discovery(2, "New Alpha"),
            mra_discovery(3, "New Beta"),
        ]);
        let new_stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\tnew".to_string(),
            "root\t0\tnew".to_string(),
        ]);
        let err = save_sqlite_scan_with_progress_and_stamp_and_projections(
            &db,
            &new_scan,
            &new_stamp,
            Path::new("/media/fat/_Arcade"),
            None,
        )
        .expect_err("summary temp directory should fail projection publish");

        assert!(
            err.contains("create catalog summary temp"),
            "unexpected error: {err}"
        );
        assert!(
            summary_path.exists(),
            "failed atomic summary replacement must preserve the old file"
        );
        assert!(
            navigation_path.exists(),
            "successful atomic navigation replacement must remain readable"
        );
        let retained_summary = catalog_summary::read_catalog_summary(&summary_path)
            .expect("read retained summary")
            .expect("retained summary exists");
        assert_eq!(
            retained_summary.catalog_stamp_fingerprint,
            old_stamp.fingerprint_hex()
        );
        assert!(
            catalog_navigation::read_catalog_navigation_projection(&navigation_path, &new_stamp)
                .expect("read replaced navigation")
                .is_some()
        );
        let loaded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("new sqlite database should remain live");
        assert_eq!(loaded.catalog.len(), 2);
        assert!(
            loaded
                .catalog
                .games
                .iter()
                .any(|game| game.title.as_ref() == "New Beta")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_load_does_not_repair_missing_navigation_projection() {
        let root = unique_temp_dir("sqlite-navigation-read-only");
        let db = root.join("library.sqlite3");
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\ttest".to_string(),
            "root\t0\tfixture".to_string(),
        ]);
        save_sqlite_scan_with_progress_and_stamp_and_catalog(
            &db,
            &sqlite_scan_with_discoveries(vec![
                mra_discovery(1, "Repair Alpha"),
                mra_discovery(2, "Repair Beta"),
            ]),
            Some(&stamp),
            "/media/fat/_Arcade",
            None,
        )
        .expect("write catalog and projection");
        let navigation_path = catalog_navigation::navigation_path_for_sqlite(&db);
        std::fs::remove_file(&navigation_path).expect("remove projection");

        let loaded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load sqlite fallback");

        assert!(
            !navigation_path.exists(),
            "SQLite load should not repair projection on the read path"
        );
        assert_eq!(loaded.catalog.games.len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_projection_repair_writes_missing_projection_pair() {
        let root = unique_temp_dir("sqlite-projection-pair-repair");
        let db = root.join("library.sqlite3");
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\ttest".to_string(),
            "root\t0\tfixture".to_string(),
        ]);
        save_sqlite_scan_with_progress_and_stamp_and_catalog(
            &db,
            &sqlite_scan_with_discoveries(vec![
                mra_discovery(1, "Repair Alpha"),
                mra_discovery(2, "Repair Beta"),
            ]),
            Some(&stamp),
            "/media/fat/_Arcade",
            None,
        )
        .expect("write catalog and projection");
        let summary_path = catalog_summary::summary_path_for_sqlite(&db);
        let navigation_path = catalog_navigation::navigation_path_for_sqlite(&db);
        std::fs::remove_file(&summary_path).expect("remove summary");
        std::fs::remove_file(&navigation_path).expect("remove projection");

        let loaded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load sqlite fallback");
        repair_catalog_projections_for_catalog(&db, &loaded.catalog, &stamp)
            .expect("repair projection pair");

        assert!(
            summary_path.exists(),
            "explicit repair should write summary"
        );
        assert!(
            navigation_path.exists(),
            "explicit repair should write projection"
        );
        let repaired_summary = catalog_summary::read_catalog_summary(&summary_path)
            .expect("read repaired summary")
            .expect("current repaired summary");
        let repaired =
            catalog_navigation::read_catalog_navigation_projection(&navigation_path, &stamp)
                .expect("read repaired projection")
                .expect("current repaired projection");
        let repaired_catalog =
            ArcadeCatalog::from_navigation_projection("/media/fat/_Arcade", repaired);
        assert_eq!(
            repaired_summary.catalog_stamp_fingerprint,
            stamp.fingerprint_hex()
        );
        assert_eq!(
            repaired_summary.total_game_count,
            loaded.catalog.games.len()
        );
        assert_eq!(repaired_catalog.games.len(), loaded.catalog.games.len());
        assert_eq!(repaired_catalog.systems, loaded.catalog.systems);
        assert_eq!(
            repaired_catalog.decade_options("arcade"),
            loaded.catalog.decade_options("arcade")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn materialized_hydration_repairs_current_schema_filter_mismatch() {
        let root = unique_temp_dir("sqlite-projection-filter-parity");
        let db = root.join("library.sqlite3");
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\ttest".to_string(),
            "root\t0\tfixture".to_string(),
        ]);
        let mut shooter = mra_discovery(1, "Repair Shooter");
        shooter.genre = Some("Shooter".to_string());
        shooter.manufacturer = Some("Capcom".to_string());
        let mut maze = mra_discovery(2, "Repair Maze");
        maze.genre = Some("Maze".to_string());
        maze.manufacturer = Some("Namco".to_string());
        save_sqlite_scan_with_progress_and_stamp_and_catalog(
            &db,
            &sqlite_scan_with_discoveries(vec![shooter, maze]),
            Some(&stamp),
            "/media/fat/_Arcade",
            None,
        )
        .expect("write catalog and projection");
        let loaded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load full sqlite catalog");
        assert_eq!(loaded.catalog.manufacturer_option_count("arcade"), 2);

        let mut incomplete_games = loaded.catalog.games.as_ref().clone();
        for game in &mut incomplete_games {
            game.manufacturer = "Arcade".into();
        }
        let incomplete = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            incomplete_games,
            loaded.catalog.systems.clone(),
        );
        let incomplete_embedded =
            catalog_navigation::encode_catalog_navigation_for_storage(&incomplete, &stamp)
                .expect("encode incomplete embedded navigation");
        let conn = Connection::open(&db).expect("open catalog projection table");
        conn.execute(
            "INSERT OR REPLACE INTO catalog_navigation_projection(id,bytes) VALUES(0,?1)",
            [&incomplete_embedded],
        )
        .expect("write valid-schema incomplete embedded navigation");
        drop(conn);
        assert!(catalog_projection_pair_current(&db, &stamp).expect("external pair current"));
        assert!(
            catalog_projection_filter_mismatches(&db, &loaded.catalog, &stamp)
                .expect("compare embedded projection")
                .iter()
                .any(|detail| detail.starts_with("embedded "))
        );
        repair_catalog_projections_for_catalog(&db, &loaded.catalog, &stamp)
            .expect("repair embedded-only mismatch");
        let repaired_embedded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load repaired embedded recovery cache");
        assert_eq!(
            repaired_embedded
                .catalog
                .manufacturer_option_count("arcade"),
            2
        );

        let conn = Connection::open(&db).expect("reopen catalog projection table");
        conn.execute(
            "INSERT OR REPLACE INTO catalog_navigation_projection(id,bytes) VALUES(0,?1)",
            [&incomplete_embedded],
        )
        .expect("restore incomplete embedded navigation");
        drop(conn);
        catalog_navigation::write_catalog_navigation_projection_for_catalog(
            &db,
            &incomplete,
            &stamp,
        )
        .expect("write valid-schema incomplete navigation projection");
        assert!(catalog_projection_pair_current(&db, &stamp).expect("projection current"));

        let recovery_cache = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load embedded recovery cache");
        assert_eq!(
            recovery_cache.catalog.manufacturer_option_count("arcade"),
            1
        );
        let hydrated = load_arcade_catalog_from_materialized_sqlite_at("/media/fat/_Arcade", &db)
            .expect("hydrate retained materialized rows");
        assert_eq!(hydrated.catalog.manufacturer_option_count("arcade"), 2);

        repair_catalog_projections_for_catalog(&db, &hydrated.catalog, &stamp)
            .expect("repair filter mismatch");

        let repaired = catalog_navigation::read_catalog_navigation_projection(
            &catalog_navigation::navigation_path_for_sqlite(&db),
            &stamp,
        )
        .expect("read repaired projection")
        .expect("repaired projection exists");
        let repaired = ArcadeCatalog::from_navigation_projection("/media/fat/_Arcade", repaired);
        assert!(
            hydrated
                .catalog
                .filter_option_mismatches(&repaired)
                .is_empty()
        );
        assert_eq!(repaired.manufacturer_option_count("arcade"), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_projection_repair_rewrites_corrupt_projection_pair() {
        let root = unique_temp_dir("sqlite-projection-corrupt-repair");
        let db = root.join("library.sqlite3");
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\ttest".to_string(),
            "root\t0\tfixture".to_string(),
        ]);
        save_sqlite_scan_with_progress_and_stamp_and_catalog(
            &db,
            &sqlite_scan_with_discoveries(vec![
                mra_discovery(1, "Repair Corrupt Alpha"),
                mra_discovery(2, "Repair Corrupt Beta"),
            ]),
            Some(&stamp),
            "/media/fat/_Arcade",
            None,
        )
        .expect("write catalog and projection");
        let summary_path = catalog_summary::summary_path_for_sqlite(&db);
        let navigation_path = catalog_navigation::navigation_path_for_sqlite(&db);
        std::fs::write(&summary_path, b"{not-json").expect("corrupt summary");
        std::fs::write(&navigation_path, b"not-lz4b").expect("corrupt projection");

        let loaded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load sqlite fallback");
        repair_catalog_projections_for_catalog(&db, &loaded.catalog, &stamp)
            .expect("repair corrupt projection pair");

        let repaired_summary = catalog_summary::read_catalog_summary(&summary_path)
            .expect("read repaired summary")
            .expect("current repaired summary");
        let repaired =
            catalog_navigation::read_catalog_navigation_projection(&navigation_path, &stamp)
                .expect("read repaired projection")
                .expect("current repaired projection");
        assert_eq!(
            repaired_summary.catalog_stamp_fingerprint,
            stamp.fingerprint_hex()
        );
        assert_eq!(repaired.games.len(), loaded.catalog.games.len());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_remove_deletes_catalog_summary_projection() {
        let root = unique_temp_dir("sqlite-remove-summary");
        let db = root.join("library.sqlite3");
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\ttest".to_string(),
        ]);
        save_sqlite_scan_with_progress_and_stamp(
            &db,
            &sqlite_scan_with_discoveries(vec![mra_discovery(1, "Summary Survivor")]),
            Some(&stamp),
            None,
        )
        .expect("write catalog and summary");

        let summary_path = catalog_summary::summary_path_for_sqlite(&db);
        let navigation_path = catalog_navigation::navigation_path_for_sqlite(&db);
        assert!(db.exists(), "database should be published");
        assert!(summary_path.exists(), "summary should be published");
        assert!(navigation_path.exists(), "navigation should be published");
        let duration_path = crate::catalog_build_record::duration_path_for_sqlite(&db);
        std::fs::write(&duration_path, b"119\n").expect("write catalog build duration");

        remove_sqlite_database_at(&db).expect("remove database and summary");

        assert!(!db.exists(), "database should be removed");
        assert!(!summary_path.exists(), "summary should be removed");
        assert!(!navigation_path.exists(), "navigation should be removed");
        assert!(!duration_path.exists(), "build duration should be removed");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn fresh_cleanup_removes_only_catalog_owned_artifacts() {
        let root = unique_temp_dir("fresh-catalog-cleanup");
        let build_dir = root.join("build");
        let snapshot_dir = root.join("snapshots");
        let assets_dir = root.join("assets");
        std::fs::create_dir_all(&build_dir).expect("create build dir");
        std::fs::create_dir_all(&snapshot_dir).expect("create snapshot dir");
        std::fs::create_dir_all(&assets_dir).expect("create assets dir");
        let db = root.join("library.sqlite3");
        let marker = root.join("rebuild-on-next-boot");
        let removable = [
            db.clone(),
            catalog_summary::summary_path_for_sqlite(&db),
            catalog_navigation::navigation_path_for_sqlite(&db),
            crate::catalog_build_record::duration_path_for_sqlite(&db),
            root.join(".library.sqlite3.tmp.42"),
            root.join("library.sqlite3-journal"),
            root.join("library.sqlite3-wal"),
            root.join("library.sqlite3-shm"),
            root.join(".library.summary.json.tmp"),
            root.join(".library.nav.lz4b.tmp"),
            root.join(".library-build-seconds.tmp"),
            build_dir.join(".library.sqlite3.build.42"),
            snapshot_dir.join("catalog-ready-42.nav.lz4b"),
            marker.clone(),
        ];
        for path in &removable {
            std::fs::write(path, b"catalog").expect("write catalog artifact");
        }
        let screenshot = assets_dir.join("arcade-screenshots-320x320.mmlz4b");
        let unrelated = root.join("notes.txt");
        let unrelated_build = build_dir.join("keep.bin");
        for path in [&screenshot, &unrelated, &unrelated_build] {
            std::fs::write(path, b"keep").expect("write retained artifact");
        }

        let removed = remove_catalog_artifacts_at(&db, &build_dir, None, &snapshot_dir, &marker)
            .expect("fresh cleanup");

        assert_eq!(removed, removable.len());
        assert!(removable.iter().all(|path| !path.exists()));
        assert!(screenshot.exists());
        assert!(unrelated.exists());
        assert!(unrelated_build.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn catalog_summary_read_ignores_schema_or_build_mismatch() {
        let root = unique_temp_dir("sqlite-catalog-summary-version");
        let db = root.join("library.sqlite3");
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\ttest".to_string(),
        ]);
        save_sqlite_scan_with_progress_and_stamp(
            &db,
            &sqlite_scan_with_discoveries(vec![mra_discovery(1, "Versioned Summary")]),
            Some(&stamp),
            None,
        )
        .expect("write catalog and summary");

        let summary_path = catalog_summary::summary_path_for_sqlite(&db);
        let original: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&summary_path).expect("read summary"))
                .expect("summary json");
        for (field, value) in [
            (
                "catalog_schema_version",
                serde_json::json!(SCHEMA_VERSION - 1),
            ),
            ("catalog_build_version", serde_json::json!(0)),
        ] {
            let mut mismatched = original.clone();
            mismatched[field] = value;
            std::fs::write(
                &summary_path,
                serde_json::to_vec(&mismatched).expect("json bytes"),
            )
            .expect("write mismatched summary");
            assert!(
                catalog_summary::read_catalog_summary(&summary_path)
                    .expect("read mismatched summary")
                    .is_none(),
                "{field} mismatch should be ignored"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_arcade_load_ignores_hot_rollback_journal() {
        let root = unique_temp_dir("sqlite-hot-journal");
        let db = root.join("library.sqlite3");
        save_sqlite_scan(
            &db,
            &sqlite_scan_with_discoveries(vec![mra_discovery(1, "Hot Journal")]),
        )
        .expect("write catalog database");

        let child = std::process::Command::new(std::env::current_exe().expect("current test exe"))
            .arg("--exact")
            .arg("sqlite_catalog::tests::sqlite_hot_journal_child_abort")
            .env("MISTER_MAGIK_HOT_JOURNAL_DB", &db)
            .output()
            .expect("run hot journal child");
        assert!(
            !child.status.success(),
            "hot journal child should abort, stdout={}, stderr={}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        let journal = PathBuf::from(format!("{}-journal", db.display()));
        assert!(
            journal.exists(),
            "child abort should leave rollback journal at {}",
            journal.display()
        );

        let loaded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load cached catalog despite hot rollback journal");
        assert_eq!(loaded.catalog.games.len(), 1);
        assert_eq!(loaded.catalog.games[0].title.as_ref(), "Hot Journal");
        assert!(
            sqlite_cached_summary(&db, 0).is_ok(),
            "cached summary reads should also ignore the stale rollback journal"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn old_schema_database_is_not_a_usable_cache() {
        let root = unique_temp_dir("sqlite-old-schema");
        let db = root.join("library.sqlite3");
        save_sqlite_scan(
            &db,
            &sqlite_scan_with_discoveries(vec![mra_discovery(1, "Old Schema")]),
        )
        .expect("write catalog database");
        let conn = Connection::open(&db).expect("open sqlite");
        conn.execute(
            "UPDATE meta SET value=?1 WHERE key='version'",
            [i64::from(SCHEMA_VERSION - 1)],
        )
        .expect("downgrade schema");
        drop(conn);

        let load_err = match load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db) {
            Ok(_) => panic!("old schema should not load as cache"),
            Err(err) => err,
        };
        assert!(
            load_err.contains("catalog schema mismatch"),
            "unexpected load error: {load_err}"
        );
        let summary_err =
            sqlite_cached_summary(&db, 0).expect_err("old schema should not summarize as cache");
        assert!(
            summary_err.contains("catalog schema mismatch"),
            "unexpected summary error: {summary_err}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_hot_journal_child_abort() {
        let Some(path) = std::env::var_os("MISTER_MAGIK_HOT_JOURNAL_DB").map(PathBuf::from) else {
            return;
        };
        let conn = Connection::open(&path).expect("open child sqlite");
        conn.execute_batch(
            "PRAGMA journal_mode=DELETE;
             BEGIN IMMEDIATE;
             UPDATE meta SET value = value + 1 WHERE key = 'normal_files';",
        )
        .expect("create hot rollback journal");
        std::process::abort();
    }

    #[test]
    fn sqlite_save_materializes_launcher_catalog_variants() {
        let root = unique_temp_dir("sqlite-launcher-catalog");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(&mame_db, &[]);
        let mut world = mra_discovery(1, "Moon Patrol (World)");
        world.launch_ref = "/media/fat/_Arcade/Moon Patrol (World).mra".to_string();
        world.source_path = world.launch_ref.clone();
        world.setname = Some("mpatrol".to_string());
        let mut us = mra_discovery(2, "Moon Patrol (US)");
        us.launch_ref = "/media/fat/_Arcade/Moon Patrol (US).mra".to_string();
        us.source_path = us.launch_ref.clone();
        us.setname = Some("mpatrol".to_string());
        let pack = preview_worker::PreviewArchiveIndex {
            path: root.join("arcade-screenshots.mmlz4b").display().to_string(),
            codec: "lz4-block",
            entries: vec!["mpatrol".to_string()],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![world, us]),
            &mame_db,
            &pack,
        )
        .expect("write sqlite");
        let conn = open_sqlite_read_only(&db).expect("open sqlite");
        let materialized_rows: i64 = conn
            .query_row("SELECT count(*) FROM launcher_catalog", [], |row| {
                row.get(0)
            })
            .expect("count launcher catalog");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(materialized_rows, 1);
        assert_eq!(loaded.rows, 1);
        assert_eq!(loaded.catalog.games[0].title.as_ref(), "Moon Patrol (US)");
        assert!(loaded.catalog.games[0].has_preview);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_catalog_loads_player_and_control_metadata_from_mame_identity() {
        let root = unique_temp_dir("sqlite-filter-metadata");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        let conn = Connection::open(&mame_db).expect("open mame db");
        conn.execute_batch(
            r#"
            CREATE TABLE mame_machines (
                setname TEXT PRIMARY KEY,
                parent_setname TEXT,
                title TEXT NOT NULL,
                year TEXT,
                manufacturer TEXT,
                players INTEGER,
                control_type TEXT
            ) WITHOUT ROWID;
            INSERT INTO mame_machines(setname,parent_setname,title,year,manufacturer,players,control_type)
            VALUES ('filtergame',NULL,'Filter Game','1986','Capcom',2,'joy');
            "#,
        )
        .expect("write mame metadata");
        drop(conn);
        let mut discovery = mra_discovery(1, "Filter Game");
        discovery.launch_ref = "/media/fat/_Arcade/Filter Game.mra".to_string();
        discovery.source_path = discovery.launch_ref.clone();
        discovery.setname = Some("filtergame".to_string());
        discovery.year = Some(1979);
        discovery.manufacturer = Some("Fallback Maker".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
        )
        .expect("write sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");
        let game = &loaded.catalog.games[0];

        assert_eq!(game.year, Some(1986));
        assert_eq!(game.manufacturer.as_ref(), "Capcom");
        assert_eq!(game.players, Some(2));
        assert_eq!(game.control.as_ref(), "joy");
        assert_eq!(loaded.catalog.decade_options("arcade")[0].label, "1980's");
        assert_eq!(
            loaded
                .catalog
                .filtered_game_count("arcade", &crate::arcade_catalog::ArcadeFilter::Players(2)),
            1
        );
        assert_eq!(
            loaded.catalog.control_options("arcade")[0].label,
            "Joystick"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn player_and_control_metadata_survives_sqlite_load_and_navigation_projection() {
        let root = unique_temp_dir("sqlite-player-control-filters");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        let conn = Connection::open(&mame_db).expect("open mame db");
        conn.execute_batch(
            r#"
            CREATE TABLE mame_machines (
                setname TEXT PRIMARY KEY,
                parent_setname TEXT,
                title TEXT NOT NULL,
                year TEXT,
                manufacturer TEXT,
                players INTEGER,
                control_type TEXT
            ) WITHOUT ROWID;
            INSERT INTO mame_machines(setname,parent_setname,title,year,manufacturer,players,control_type)
            VALUES
                ('game1',NULL,'Game One','1980','Namco',1,'joy'),
                ('game2',NULL,'Game Two','1981','Capcom',2,'joy'),
                ('game3',NULL,'Game Three','1982','Irem',3,'dial'),
                ('game4',NULL,'Game Four','1983','Sega',4,'only_buttons');
            "#,
        )
        .expect("write mame metadata");
        drop(conn);

        let discoveries = [
            ("game1", "Game One"),
            ("game2", "Game Two"),
            ("game3", "Game Three"),
            ("game4", "Game Four"),
        ]
        .into_iter()
        .enumerate()
        .map(|(idx, (setname, title))| {
            let mut discovery = mra_discovery(idx + 1, title);
            discovery.launch_ref = format!("/media/fat/_Arcade/{title}.mra");
            discovery.source_path = discovery.launch_ref.clone();
            discovery.setname = Some(setname.to_string());
            discovery
        })
        .collect::<Vec<_>>();

        let stamp = catalog_stamp::CatalogStamp::from_lines(vec![
            format!("schema\t{SCHEMA_VERSION}"),
            "catalog-build\tplayer-control".to_string(),
            "root\t0\tfixture".to_string(),
        ]);
        write_sqlite_scan_with_sources(
            &db,
            &sqlite_scan_with_discoveries(discoveries),
            SqliteScanSources {
                mame_sqlite_path: &mame_db,
                hbmame_sqlite_path: &PathBuf::new(),
                preview_paths: &PreviewArchivePaths::default(),
                software_hash_cache: SoftwareHashCache::load(&db),
                discovery_history: DiscoveryHistory::load(&db),
                stamp: Some(&stamp),
            },
            Path::new(arcade_catalog::DEFAULT_ARCADE_ROOT),
            None,
        )
        .expect("write sqlite");
        let expected_players = BTreeSet::from([
            "1 Player".to_string(),
            "2 Players".to_string(),
            "3 Players".to_string(),
            "4 Players".to_string(),
        ]);

        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");
        assert_eq!(
            player_options_as_set(&loaded.catalog, "arcade"),
            expected_players,
            "SQLite runtime load must expose all player filter options"
        );
        assert_eq!(loaded.catalog.control_option_count("arcade"), 3);

        repair_catalog_projections_for_catalog(&db, &loaded.catalog, &stamp)
            .expect("repair navigation projection");
        let navigation_path = catalog_navigation::navigation_path_for_sqlite(&db);
        let navigation =
            catalog_navigation::read_catalog_navigation_projection(&navigation_path, &stamp)
                .expect("read navigation projection")
                .expect("current navigation projection");
        let navigation_catalog =
            ArcadeCatalog::from_navigation_projection("/media/fat/_Arcade", navigation);
        assert_eq!(
            player_options_as_set(&navigation_catalog, "arcade"),
            expected_players,
            "navigation projection must expose all player filter options"
        );
        assert_eq!(navigation_catalog.control_option_count("arcade"), 3);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_catalog_omits_malformed_text_year_from_mame_identity() {
        let root = unique_temp_dir("sqlite-filter-bad-year");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        let conn = Connection::open(&mame_db).expect("open mame db");
        conn.execute_batch(
            r#"
            CREATE TABLE mame_machines (
                setname TEXT PRIMARY KEY,
                parent_setname TEXT,
                title TEXT NOT NULL,
                year TEXT,
                manufacturer TEXT,
                players INTEGER,
                control_type TEXT
            ) WITHOUT ROWID;
            INSERT INTO mame_machines(setname,parent_setname,title,year,manufacturer,players,control_type)
            VALUES ('badyear',NULL,'Bad Year','19??','Capcom',2,'joy');
            "#,
        )
        .expect("write mame metadata");
        drop(conn);
        let mut discovery = mra_discovery(1, "Bad Year");
        discovery.launch_ref = "/media/fat/_Arcade/Bad Year.mra".to_string();
        discovery.source_path = discovery.launch_ref.clone();
        discovery.setname = Some("badyear".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
        )
        .expect("write sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");
        let game = &loaded.catalog.games[0];

        assert_eq!(game.year, None);
        assert_eq!(game.manufacturer.as_ref(), "Capcom");
        assert_eq!(game.players, Some(2));
        assert_eq!(game.control.as_ref(), "joy");
        assert!(loaded.catalog.decade_options("arcade").is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn launcher_catalog_appends_console_rows_after_arcade_projection() {
        let root = unique_temp_dir("sqlite-launcher-catalog-order");
        let db = root.join("library.sqlite3");
        let mut arcade = mra_discovery(1, "Zeta Arcade");
        arcade.launch_ref = "/media/fat/_Arcade/Zeta Arcade.mra".to_string();
        arcade.source_path = arcade.launch_ref.clone();
        arcade.setname = Some("zeta".to_string());
        let mut console = payload("/media/fat/games/SNES/Alpha Console.sfc");
        console.platform_id = "snes".to_string();
        console.core_id = "SNES".to_string();
        console.hardware_id = "snes".to_string();

        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(vec![console, arcade]))
            .expect("write sqlite");
        let conn = open_sqlite_read_only(&db).expect("open sqlite");
        let mut stmt = conn
            .prepare(
                "SELECT ordinal,title,system_id
                 FROM launcher_catalog
                 ORDER BY ordinal",
            )
            .expect("prepare launcher catalog query");
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .expect("query launcher catalog rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("read launcher catalog rows");

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            (0, "Zeta Arcade".to_string(), "arcade".to_string())
        );
        assert_eq!(rows[1].0, 1);
        assert_eq!(rows[1].1, "Alpha Console");
        assert_eq!(rows[1].2, "snes");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compact_launch_storage_keeps_hot_tables_keyed_by_launch_id() {
        let root = unique_temp_dir("sqlite-compact-launch-storage");
        let db = root.join("library.sqlite3");
        let saturn = saturn_payload("/media/fat/games/Saturn/Nights.chd");

        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(vec![saturn])).expect("write sqlite");
        let conn = open_sqlite_read_only(&db).expect("open sqlite");

        assert!(sqlite_column_exists(&conn, "launcher_catalog", "launch_id").expect("launch_id"));
        assert!(
            !sqlite_column_exists(&conn, "launcher_catalog", "launch_ref").expect("launch_ref")
        );
        assert!(
            sqlite_column_exists(&conn, "launcher_launch_plans", "launch_id")
                .expect("plan launch_id")
        );
        assert!(
            !sqlite_column_exists(&conn, "launcher_launch_plans", "payload_path")
                .expect("payload_path")
        );
        assert!(sqlite_column_exists(&conn, "games", "game_key_id").expect("game_key_id"));
        assert!(
            sqlite_column_exists(&conn, "launch_targets", "game_key_id")
                .expect("target game_key_id")
        );
        assert!(
            sqlite_column_exists(&conn, "launch_targets", "mount_kind").expect("target mount_kind")
        );
        assert!(
            sqlite_column_exists(&conn, "launch_targets", "mount_index")
                .expect("target mount_index")
        );
        assert!(
            sqlite_column_exists(&conn, "launch_targets", "delay_secs").expect("target delay_secs")
        );
        assert!(
            sqlite_column_exists(&conn, "launch_targets", "launch_ref_kind")
                .expect("target launch_ref_kind")
        );
        assert!(!sqlite_column_exists(&conn, "launch_targets", "game_id").expect("target game_id"));
        assert!(!sqlite_column_exists(&conn, "launch_targets", "plan_id").expect("target plan_id"));
        assert!(
            !sqlite_column_exists(&conn, "launch_targets", "priority").expect("target priority")
        );
        assert!(
            sqlite_column_exists(&conn, "region_metadata_rows", "game_key_id")
                .expect("region game_key_id")
        );
        assert!(
            !sqlite_column_exists(&conn, "region_metadata_rows", "game_id")
                .expect("region game_id")
        );
        assert!(
            !sqlite_column_exists(&conn, "region_metadata_rows", "override_region")
                .expect("region override")
        );
        assert!(
            sqlite_column_exists(&conn, "launchable_identity_rows", "game_key_id")
                .expect("identity game_key_id")
        );
        assert!(
            !sqlite_column_exists(&conn, "launchable_identity_rows", "launchable_id")
                .expect("identity launchable_id")
        );
        assert!(
            !sqlite_column_exists(&conn, "ui_arcade_preferred", "asset_pack_id")
                .expect("preferred asset pack")
        );
        assert!(
            sqlite_column_exists(&conn, "ui_arcade_preferred", "family_id")
                .expect("preferred family")
        );
        assert!(
            sqlite_column_exists(&conn, "ui_arcade_preferred", "variant_ordinal")
                .expect("preferred variant ordinal")
        );
        assert!(
            !sqlite_column_exists(&conn, "ui_arcade_preferred", "title").expect("preferred title")
        );
        assert!(
            !sqlite_column_exists(&conn, "ui_arcade_preferred", "preview_asset_key")
                .expect("preferred preview")
        );
        assert!(
            !sqlite_column_exists(&conn, "ui_arcade_preferred", "system_id")
                .expect("preferred system")
        );
        assert!(
            !sqlite_column_exists(&conn, "ui_arcade_variants", "asset_pack_id")
                .expect("variant asset pack")
        );
        let view_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type='view'
                   AND name IN ('launcher_catalog_text','launcher_launch_plans_text','launchable_identities','region_metadata')",
                [],
                |row| row.get(0),
            )
            .expect("view count");
        assert_eq!(view_count, 4);

        let reconstructed: (String, String, String, String) = conn
            .query_row(
                "SELECT lc.launch_ref, lp.payload_path, lp.plan_id, lp.game_id
                 FROM launcher_catalog_text lc
                 JOIN launcher_launch_plans_text llp ON llp.launch_id = lc.launch_id
                 JOIN launch_plans lp ON lp.launch_id = lc.launch_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("reconstructed launch text");
        assert!(reconstructed.0.starts_with("magik-plan:payload:"));
        assert_eq!(reconstructed.1, "/media/fat/games/Saturn/Nights.chd");
        assert!(reconstructed.2.starts_with("plan:payload:"));
        assert_eq!(
            reconstructed.3,
            "payload:/media/fat/games/Saturn/Nights.chd"
        );

        let region_rows: i64 = conn
            .query_row("SELECT count(*) FROM region_metadata_rows", [], |row| {
                row.get(0)
            })
            .expect("region row count");
        assert_eq!(region_rows, 0);
        let default_region: (Option<String>, String) = conn
            .query_row(
                "SELECT inferred_region, confidence
                 FROM region_metadata
                 WHERE game_id='payload:/media/fat/games/Saturn/Nights.chd'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("default region view row");
        assert_eq!(default_region, (None, "unknown".to_string()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn materialized_catalog_entry_queries_match_hydrator_shape() {
        let root = unique_temp_dir("sqlite-catalog-entry-row-shape");
        let db = root.join("library.sqlite3");
        let mut arcade = mra_discovery(1, "Shape Arcade");
        arcade.launch_ref = "/media/fat/_Arcade/Shape Arcade.mra".to_string();
        arcade.source_path = arcade.launch_ref.clone();
        arcade.setname = Some("shape".to_string());
        let mut console = payload("/media/fat/games/SNES/Shape Console.sfc");
        console.platform_id = "snes".to_string();
        console.core_id = "SNES".to_string();
        console.hardware_id = "snes".to_string();

        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(vec![arcade, console]))
            .expect("write sqlite");
        let conn = open_sqlite_read_only(&db).expect("open sqlite");

        for source in ["ui_arcade_preferred_text", "launcher_catalog_text"] {
            let sql = catalog_game_entry_select_sql(source, "", "ordinal");
            let stmt = conn.prepare(&sql).expect("prepare canonical row query");
            assert_eq!(stmt.column_names(), catalog_game_entry_column_names());
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn launch_targets_store_mounts_without_payload_inventory_table() {
        let root = unique_temp_dir("sqlite-launch-target-mount-storage");
        let db = root.join("library.sqlite3");
        let saturn = saturn_payload("/media/fat/games/Saturn/Nights.chd");
        let neogeo_file = "/media/fat/games/NEOGEO/Neo Geo Mister FGPA Ultra Pack.zip";
        let neogeo_entry =
            "Neo Geo Mister FGPA Ultra Pack/ World A-Z/King of Fighters '99, The (kof99).neo";
        let neogeo_launch_ref = format!("{neogeo_file}/{neogeo_entry}");
        let neogeo = GameDiscovery {
            source_path: format!("{neogeo_file}::{neogeo_entry}"),
            launch_ref: neogeo_launch_ref.clone(),
            source_kind: DiscoverySourceKind::ArchiveEntry,
            title: "King of Fighters '99, The (kof99)".to_string(),
            category: "Console".to_string(),
            platform_id: "neogeo".to_string(),
            core_id: "NeoGeo".to_string(),
            hardware_id: "neogeo".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: Some("kof99".to_string()),
            parent: None,
            arcade_updater_metadata: None,
            covered_payload_path: None,
            prepared: None,
            confidence: crate::game_discovery::DiscoveryConfidence::ArchiveToc,
        };

        let mut scan = sqlite_scan_with_discoveries(vec![saturn, neogeo]);
        scan.normal_files =
            sqlite_scan_with_normal_files(&["/media/fat/games/Saturn/Nights.chd"]).normal_files;
        let neogeo_rule = launch_profiles::builtin_profiles()
            .into_iter()
            .find(|profile| profile.id == "neogeo")
            .expect("neogeo profile")
            .archive_entry_rules[0]
            .clone();
        scan.entries.push(crate::library_db::LibraryContainerEntry {
            file_path: neogeo_file.to_string(),
            entry_path: neogeo_entry.to_string(),
            normalized_title: "king of fighters '99, the (kof99)".to_string(),
            profile_id: "neogeo".to_string(),
            rule: neogeo_rule,
            compressed_size: Some(100),
            uncompressed_size: Some(200),
            crc32: None,
            launchable: true,
            launch_ref: neogeo_launch_ref.clone(),
        });

        save_sqlite_scan(&db, &scan).expect("write sqlite");
        let conn = open_sqlite_read_only(&db).expect("open sqlite");

        assert!(!sqlite_table_exists(&conn, "payloads").expect("payloads absent"));
        let payload_view_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type='view' AND name='payloads_text'",
                [],
                |row| row.get(0),
            )
            .expect("payloads_text view count");
        assert_eq!(payload_view_count, 0);

        let saturn_target: (String, String, i64, i64) = conn
            .query_row(
                "SELECT launch_plans.payload_path,
                        launch_targets.mount_kind,
                        launch_targets.mount_index,
                        launch_targets.delay_secs
                 FROM launch_plans
                 JOIN launch_targets ON launch_targets.launch_id = launch_plans.launch_id
                 WHERE launch_plans.payload_path='/media/fat/games/Saturn/Nights.chd'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("saturn launch target");
        assert_eq!(saturn_target.0, "/media/fat/games/Saturn/Nights.chd");
        assert_eq!(saturn_target.1, "mount-image");
        assert_eq!(saturn_target.2, 0);
        assert_eq!(saturn_target.3, 1);

        let archive_target: (String, String, i64, i64) = conn
            .query_row(
                "SELECT launch_plans.payload_path,
                        launch_targets.mount_kind,
                        launch_targets.mount_index,
                        launch_targets.delay_secs
                 FROM launch_plans
                 JOIN launch_targets ON launch_targets.launch_id = launch_plans.launch_id
                 WHERE launch_plans.payload_path=?1",
                [neogeo_launch_ref.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("archive launch target");
        assert_eq!(archive_target.0, neogeo_launch_ref);
        assert_eq!(archive_target.1, "load-file");
        assert_eq!(archive_target.2, 1);
        assert_eq!(archive_target.3, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn launcher_catalog_hydrates_structured_launch_plans() {
        let root = unique_temp_dir("sqlite-launcher-structured-plans");
        let db = root.join("library.sqlite3");
        let saturn = saturn_payload("/media/fat/games/Saturn/Nights.chd");

        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(vec![saturn])).expect("write sqlite");
        let conn = open_sqlite_read_only(&db).expect("open sqlite");
        let materialized_plans: i64 = conn
            .query_row("SELECT count(*) FROM launcher_launch_plans", [], |row| {
                row.get(0)
            })
            .expect("count launcher launch plans");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");
        let launch_ref = loaded.catalog.games[0].mra_path.as_ref();

        let target = loaded.catalog.launch_target_for_ref(launch_ref);

        assert_eq!(materialized_plans, 1);
        match target {
            crate::arcade_catalog::LaunchTarget::Structured(plan) => {
                assert_eq!(plan.launch_ref.as_ref(), launch_ref);
                assert_eq!(plan.system_id.as_ref(), "saturn");
                assert_eq!(plan.core_path.as_ref(), "_Console/Saturn");
                assert_eq!(
                    plan.payload_path.as_ref(),
                    "/media/fat/games/Saturn/Nights.chd"
                );
                assert_eq!(plan.mount_kind.as_ref(), "mount-image");
                assert_eq!(plan.mount_index, 0);
                assert_eq!(plan.delay_secs, 1);
            }
            crate::arcade_catalog::LaunchTarget::Path(path) => {
                panic!("expected structured plan, got path {path}")
            }
            crate::arcade_catalog::LaunchTarget::Prepared(selection) => {
                panic!(
                    "expected structured plan, got prepared {}",
                    selection.launch_ref
                )
            }
            crate::arcade_catalog::LaunchTarget::MissingStructured(launch_ref) => {
                panic!("expected structured plan, got missing {launch_ref}")
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn virtual_launch_plan_query_returns_system_scoped_rows() {
        let root = unique_temp_dir("sqlite-virtual-launch-plans");
        let db = root.join("library.sqlite3");
        let saturn = saturn_payload("/media/fat/games/Saturn/Nights.chd");
        let mut snes = payload("/media/fat/games/SNES/F-Zero.sfc");
        snes.platform_id = "snes".to_string();
        snes.core_id = "SNES".to_string();
        snes.hardware_id = "snes".to_string();

        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(vec![saturn, snes]))
            .expect("write sqlite");
        let conn = open_sqlite_read_only(&db).expect("open sqlite");
        let plans = load_virtual_launch_plans_for_system_from_conn(&conn, "saturn", 8)
            .expect("load virtual launch plans");

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].system_id, "saturn");
        assert_eq!(plans[0].core_path, "_Console/Saturn");
        assert_eq!(plans[0].payload_path, "/media/fat/games/Saturn/Nights.chd");
        assert_eq!(plans[0].mount_kind, "mount-image");
        assert_eq!(plans[0].mount_index, 0);
        assert_eq!(plans[0].mount_delay_secs, 1);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn ui_arcade_preferred_collapses_family_and_keeps_variants() {
        let root = unique_temp_dir("ui-arcade-preferred-parent");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                ("1942", None, "1942", Some("1984"), Some("Capcom")),
                (
                    "1942b",
                    Some("1942"),
                    "1942 (First Version)",
                    Some("1984"),
                    Some("Capcom"),
                ),
            ],
        );
        let mut parent = mra_discovery(1, "1942");
        parent.setname = Some("1942".to_string());
        let mut clone = mra_discovery(2, "1942 (First Version)");
        clone.setname = Some("1942b".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![clone, parent]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = open_sqlite_read_only(&db).expect("open library sqlite");
        let preferred = conn
            .query_row(
                "SELECT identity_id,family_id,preferred_reason,title,has_preview
                 FROM ui_arcade_preferred_text",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .expect("query preferred row");
        let variant_count: i64 = conn
            .query_row("SELECT count(*) FROM ui_arcade_variants", [], |row| {
                row.get(0)
            })
            .expect("query variant count");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(preferred.0.as_deref(), Some("1942"));
        assert_eq!(preferred.1, "1942");
        assert_eq!(preferred.2, "installed-parent");
        assert_eq!(preferred.3, "1942");
        assert_eq!(preferred.4, 0);
        assert_eq!(variant_count, 2);
        assert_eq!(loaded.catalog.games.len(), 1);
        assert_eq!(loaded.catalog.games[0].title.as_ref(), "1942");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ui_arcade_preferred_uses_deterministic_child_when_parent_missing() {
        let root = unique_temp_dir("ui-arcade-preferred-child");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                (
                    "1942b",
                    Some("1942"),
                    "1942 (First Version)",
                    Some("1984"),
                    Some("Capcom"),
                ),
                (
                    "1942w",
                    Some("1942"),
                    "1942 (World)",
                    Some("1984"),
                    Some("Capcom"),
                ),
            ],
        );
        let mut first = mra_discovery(1, "1942 (First Version)");
        first.setname = Some("1942b".to_string());
        let mut world = mra_discovery(2, "1942 (World)");
        world.setname = Some("1942w".to_string());
        let pack = preview_worker::PreviewArchiveIndex {
            path: root.join("arcade-screenshots.mmlz4b").display().to_string(),
            codec: "lz4-block",
            entries: vec!["1942w".to_string()],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![first, world]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = open_sqlite_read_only(&db).expect("open library sqlite");
        let preferred = conn
            .query_row(
                "SELECT identity_id,family_id,preferred_reason,has_preview
                 FROM ui_arcade_preferred_text",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .expect("query preferred row");
        let variant_count: i64 = conn
            .query_row("SELECT count(*) FROM ui_arcade_variants", [], |row| {
                row.get(0)
            })
            .expect("query variant count");

        assert_eq!(preferred.0.as_deref(), Some("1942b"));
        assert_eq!(preferred.1, "1942");
        assert_eq!(preferred.2, "deterministic-child");
        assert_eq!(preferred.3, 1);
        assert_eq!(variant_count, 2);
        let _ = std::fs::remove_dir_all(root);
    }
}
