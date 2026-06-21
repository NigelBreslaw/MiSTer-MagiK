//! SQLite catalog import, publish, and loading.

use crate::arcade_catalog::{self, ArcadeCatalog, ArcadeGameEntry};
use crate::catalog_config::{
    default_hbmame_sqlite_path, default_mame_sqlite_path, default_sqlite_path,
    DEFAULT_SQLITE_BUILD_DIR, SCHEMA_VERSION,
};
use crate::catalog_stamp;
use crate::catalog_store;
use crate::game_discovery::{
    catalog_system_id_for_discovery, confidence_str, covered_payload_paths, is_launcher_launch_ref,
    launch_kind_for_discovery, launch_ref_for_discovery, preferred_playable_discoveries_by_key,
    profile_id_for_discovery, system_title_for_discovery, unique_discovery_count,
    DiscoverySourceKind,
};
use crate::launch_profiles::{self, MountKind, PayloadDisposition, RuleSourceKind};
use crate::library_db::{
    self, BenchConfig, CatalogRow, CatalogStampCheckSummary, FileSignature, LibraryCatalogLoad,
    LibraryRefreshSummary, LibraryScan, ProgressCallback, VirtualLaunchPlan,
};
use crate::media_metadata;
use crate::preview_worker;
use crate::software_identity::{
    console_preview_asset, load_arcade_machine_metadata, load_mame_software_metadata,
    mame_identity_for_discovery, mame_identity_projection, mame_software_identity_for_discovery,
    PreviewArchivePaths, SoftwareHashCache,
};
use rusqlite::{params, Connection, OpenFlags};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Instant;

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

pub(crate) fn remove_default_sqlite_database() -> Result<(), String> {
    let path = default_sqlite_path();
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("failed to delete {}: {e}", path.display())),
    }
    Ok(())
}

pub(crate) fn load_virtual_launch_plan(
    launch_ref: &str,
) -> Result<Option<VirtualLaunchPlan>, String> {
    let path = default_sqlite_path();
    let conn = open_sqlite_read_only(&path).map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    ensure_sqlite_schema_current(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT launch_plans.launch_ref,
                    games.title,
                    games.system_id,
                    COALESCE(profiles.core_path, launch_plans.core_id),
                    COALESCE(launch_plans.payload_path, ''),
                    COALESCE(payloads.mount_kind, 'mount-image'),
                    COALESCE(payloads.mount_index, 0),
                    COALESCE(payloads.mount_delay_secs, 1)
             FROM launch_plans
             JOIN games ON games.game_id = launch_plans.game_id
             LEFT JOIN profiles ON profiles.profile_id = launch_plans.profile_id
             LEFT JOIN payloads
                    ON payloads.launch_ref = launch_plans.payload_path
                   AND payloads.profile_id = launch_plans.profile_id
             WHERE launch_plans.launch_ref = ?1
               AND launch_plans.launch_kind = 'virtual-mgl'",
        )
        .map_err(|e| format!("prepare virtual launch query: {e}"))?;
    let mut rows = stmt
        .query([launch_ref])
        .map_err(|e| format!("query virtual launch: {e}"))?;
    let Some(row) = rows
        .next()
        .map_err(|e| format!("read virtual launch: {e}"))?
    else {
        return Ok(None);
    };
    Ok(Some(VirtualLaunchPlan {
        launch_ref: row
            .get::<_, String>(0)
            .map_err(|e| format!("read launch_ref: {e}"))?,
        title: row
            .get::<_, String>(1)
            .map_err(|e| format!("read title: {e}"))?,
        system_id: row
            .get::<_, String>(2)
            .map_err(|e| format!("read system_id: {e}"))?,
        core_path: row
            .get::<_, String>(3)
            .map_err(|e| format!("read core_path: {e}"))?,
        payload_path: row
            .get::<_, String>(4)
            .map_err(|e| format!("read payload_path: {e}"))?,
        mount_kind: row
            .get::<_, String>(5)
            .map_err(|e| format!("read mount_kind: {e}"))?,
        mount_index: row
            .get::<_, i64>(6)
            .map_err(|e| format!("read mount_index: {e}"))?
            .clamp(0, u8::MAX as i64) as u8,
        mount_delay_secs: row
            .get::<_, i64>(7)
            .map_err(|e| format!("read mount_delay_secs: {e}"))?
            .clamp(0, u8::MAX as i64) as u8,
    }))
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

pub(crate) fn load_virtual_launch_plans() -> Result<Vec<VirtualLaunchPlan>, String> {
    let path = default_sqlite_path();
    let conn = open_sqlite_read_only(&path).map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    ensure_sqlite_schema_current(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT launch_plans.launch_ref,
                    games.title,
                    games.system_id,
                    COALESCE(profiles.core_path, launch_plans.core_id),
                    COALESCE(launch_plans.payload_path, ''),
                    COALESCE(payloads.mount_kind, 'mount-image'),
                    COALESCE(payloads.mount_index, 0),
                    COALESCE(payloads.mount_delay_secs, 1)
             FROM launch_plans
             JOIN games ON games.game_id = launch_plans.game_id
             LEFT JOIN profiles ON profiles.profile_id = launch_plans.profile_id
             LEFT JOIN payloads
                    ON payloads.launch_ref = launch_plans.payload_path
                   AND payloads.profile_id = launch_plans.profile_id
             WHERE launch_plans.launch_kind = 'virtual-mgl'
             ORDER BY games.system_id, games.sort_title, launch_plans.launch_ref",
        )
        .map_err(|e| format!("prepare virtual launch list query: {e}"))?;
    let rows = stmt
        .query_map([], virtual_launch_plan_from_row)
        .map_err(|e| format!("query virtual launch list: {e}"))?;
    rows.map(|row| row.map_err(|e| format!("read virtual launch row: {e}")))
        .collect()
}

pub(crate) fn load_virtual_launch_plans_for_system_from_conn(
    conn: &Connection,
    system_id: &str,
    limit: usize,
) -> Result<Vec<VirtualLaunchPlan>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT launch_plans.launch_ref,
                    games.title,
                    games.system_id,
                    COALESCE(profiles.core_path, launch_plans.core_id),
                    COALESCE(launch_plans.payload_path, ''),
                    COALESCE(payloads.mount_kind, 'mount-image'),
                    COALESCE(payloads.mount_index, 0),
                    COALESCE(payloads.mount_delay_secs, 1)
             FROM launch_plans
             JOIN games ON games.game_id = launch_plans.game_id
             LEFT JOIN profiles ON profiles.profile_id = launch_plans.profile_id
             LEFT JOIN payloads
                    ON payloads.launch_ref = launch_plans.payload_path
                   AND payloads.profile_id = launch_plans.profile_id
             WHERE launch_plans.launch_kind = 'virtual-mgl'
               AND games.system_id = ?1
             ORDER BY games.sort_title, launch_plans.launch_ref
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

pub(crate) fn load_arcade_catalog_from_sqlite_at(
    root: impl AsRef<Path>,
    path: &Path,
) -> Result<LibraryCatalogLoad, String> {
    let root = root.as_ref().to_path_buf();
    let t = Instant::now();
    let open_t = Instant::now();
    let conn = open_sqlite_read_only(path).map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    ensure_sqlite_schema_current(&conn)?;
    let open_us = open_t.elapsed().as_micros() as u64;
    let query_t = Instant::now();
    let games = match load_materialized_ui_catalog(&conn) {
        Ok(Some(games)) => games,
        Ok(None) => match load_materialized_launcher_catalog(&conn) {
            Ok(Some(games)) => games,
            Ok(None) => load_joined_launcher_catalog(&conn)?,
            Err(e) => return Err(e),
        },
        Err(e) => return Err(e),
    };
    let query_us = query_t.elapsed().as_micros() as u64;
    let rows = games.len();
    let systems_t = Instant::now();
    let systems = arcade_catalog::systems_from_games(&games);
    let systems_us = systems_t.elapsed().as_micros() as u64;
    let catalog_t = Instant::now();
    let catalog = ArcadeCatalog::new(root, games, systems);
    let catalog_us = catalog_t.elapsed().as_micros() as u64;
    Ok(LibraryCatalogLoad {
        catalog,
        us: t.elapsed().as_micros() as u64,
        open_us,
        query_us,
        systems_us,
        catalog_us,
        rows,
    })
}

pub(crate) fn load_materialized_ui_catalog(
    conn: &Connection,
) -> Result<Option<Vec<ArcadeGameEntry>>, String> {
    if !sqlite_table_exists(conn, "ui_arcade_preferred")? {
        return Ok(None);
    }
    let mut games = query_game_entries(
        conn,
        "SELECT title,
                launch_ref,
                preview_archive_path,
                preview_asset_key,
                has_preview,
                system_id
         FROM ui_arcade_preferred
         ORDER BY ordinal",
        "ui arcade preferred",
    )?;
    if sqlite_table_exists(conn, "launcher_catalog")? {
        games.extend(query_game_entries(
            conn,
            "SELECT title,
                    launch_ref,
                    preview_archive_path,
                    preview_asset_key,
                    has_preview,
                    system_id
             FROM launcher_catalog
             WHERE system_id NOT IN ('arcade','neogeo')
             ORDER BY ordinal",
            "launcher catalog extras",
        )?);
    }
    Ok(Some(games))
}

pub(crate) fn load_materialized_launcher_catalog(
    conn: &Connection,
) -> Result<Option<Vec<ArcadeGameEntry>>, String> {
    if !sqlite_table_exists(conn, "launcher_catalog")? {
        return Ok(None);
    }
    Ok(Some(query_game_entries(
        conn,
        "SELECT title,
                launch_ref,
                preview_archive_path,
                preview_asset_key,
                has_preview,
                system_id
         FROM launcher_catalog
         ORDER BY ordinal",
        "launcher catalog",
    )?))
}

pub(crate) fn query_game_entries(
    conn: &Connection,
    sql: &str,
    label: &str,
) -> Result<Vec<ArcadeGameEntry>, String> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("prepare {label} query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ArcadeGameEntry {
                title: row.get::<_, String>(0)?.into(),
                mra_path: row.get::<_, String>(1)?.into(),
                preview_archive_path: row.get::<_, String>(2)?.into(),
                preview_asset_key: row.get::<_, String>(3)?.into(),
                has_preview: row.get::<_, i64>(4)? != 0,
                system_id: row.get::<_, String>(5)?.into(),
            })
        })
        .map_err(|e| format!("query {label}: {e}"))?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row.map_err(|e| format!("read {label} row: {e}"))?);
    }
    Ok(games)
}

pub(crate) fn sqlite_table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists != 0)
    .map_err(|e| format!("check sqlite table {table}: {e}"))
}

pub(crate) fn open_sqlite_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let uri = format!("file:{}?mode=ro&immutable=1", sqlite_uri_path(path));
    Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
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
    let mut stmt = conn
        .prepare(
            "SELECT games.title,
                    launch_plans.launch_ref,
                    '',
                    '',
                    0,
                    COALESCE(games.system_id,'unknown'),
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
            Ok(CatalogRow {
                game: ArcadeGameEntry {
                    title: row.get::<_, String>(0)?.into(),
                    mra_path: row.get::<_, String>(1)?.into(),
                    preview_archive_path: row.get::<_, String>(2)?.into(),
                    preview_asset_key: row.get::<_, String>(3)?.into(),
                    has_preview: row.get::<_, i64>(4)? != 0,
                    system_id: row.get::<_, String>(5)?.into(),
                },
                source_kind: row.get::<_, String>(6)?,
                setname: row.get::<_, String>(7)?,
                parent: row.get::<_, String>(8)?,
                family_key: None,
            })
        })
        .map_err(|e| format!("query arcade catalog: {e}"))?;
    let mut rows_out = Vec::new();
    for row in rows {
        rows_out.push(row.map_err(|e| format!("read arcade catalog row: {e}"))?);
    }
    rows_out.retain(|row| is_launcher_launch_ref(&row.game.mra_path));
    Ok(library_db::collapse_catalog_variants(rows_out))
}

pub(crate) fn sqlite_catalog_stamp_check(
    cfg: &BenchConfig,
) -> Result<CatalogStampCheckSummary, String> {
    let started = Instant::now();
    let open_t = Instant::now();
    let conn = open_sqlite_read_only(&cfg.sqlite_path)
        .map_err(|e| format!("open catalog stamp db {}: {e}", cfg.sqlite_path.display()))?;
    let open_us = open_t.elapsed().as_micros() as u64;
    let read_t = Instant::now();
    let stored = catalog_store::read_catalog_stamp(&conn)?;
    let read_us = read_t.elapsed().as_micros() as u64;
    let compute_t = Instant::now();
    let current = catalog_stamp::compute_default_catalog_stamp(&cfg.roots);
    let current_fingerprint = current.fingerprint_hex();
    let current_lines = current.lines().len();
    let compute_us = compute_t.elapsed().as_micros() as u64;
    let compare_t = Instant::now();
    let (stored_fingerprint, stored_lines, unchanged) = match stored {
        Some(stored) => {
            let stored_fingerprint = stored.fingerprint_hex();
            let stored_lines = stored.lines().len();
            let unchanged = stored == current;
            (Some(stored_fingerprint), stored_lines, unchanged)
        }
        None => (None, 0, false),
    };
    let compare_us = compare_t.elapsed().as_micros() as u64;
    Ok(CatalogStampCheckSummary {
        unchanged,
        check_us: started.elapsed().as_micros() as u64,
        compute_us,
        open_us,
        read_us,
        compare_us,
        stored_fingerprint,
        current_fingerprint,
        stored_lines,
        current_lines,
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

pub(crate) fn save_sqlite_scan_with_progress_and_stamp(
    path: &Path,
    scan: &LibraryScan,
    stamp: Option<&catalog_stamp::CatalogStamp>,
    progress: ProgressCallback<'_>,
) -> Result<u64, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create sqlite dir: {e}"))?;
    }

    let mut writer =
        |build_path: &Path, scan: &LibraryScan, progress: &mut ProgressCallback<'_>| {
            let software_hash_cache = SoftwareHashCache::load(path);
            write_sqlite_scan(
                build_path,
                scan,
                reborrow_progress(progress),
                software_hash_cache,
                stamp,
            )
        };
    save_sqlite_scan_with_progress_using_writer(
        path,
        scan,
        progress,
        sqlite_build_temp_plan(path),
        &mut writer,
    )
}

pub(crate) fn save_sqlite_scan_with_progress_using_writer(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
    initial_plan: SqliteBuildTempPlan,
    writer: &mut dyn FnMut(&Path, &LibraryScan, &mut ProgressCallback<'_>) -> Result<(), String>,
) -> Result<u64, String> {
    let mut progress = progress;
    let first =
        save_sqlite_scan_attempt_with_writer(path, scan, &mut progress, &initial_plan, writer);
    match first {
        Ok(bytes) => Ok(bytes),
        Err(e)
            if initial_plan.source == SqliteBuildTempSource::DefaultTmpfs
                && sqlite_build_error_should_retry_beside_final(&e) =>
        {
            eprintln!(
                "library sqlite build temp failed at {}; retrying beside final DB: {e}",
                initial_plan.build_tmp_path.display()
            );
            let fallback_plan = sqlite_build_temp_plan_beside_final(path);
            save_sqlite_scan_attempt_with_writer(path, scan, &mut progress, &fallback_plan, writer)
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn save_sqlite_scan_attempt_with_writer(
    path: &Path,
    scan: &LibraryScan,
    progress: &mut ProgressCallback<'_>,
    plan: &SqliteBuildTempPlan,
    writer: &mut dyn FnMut(&Path, &LibraryScan, &mut ProgressCallback<'_>) -> Result<(), String>,
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
    sync_file_best_effort(&plan.build_tmp_path, "sqlite build temp")?;
    if plan.build_tmp_path != plan.final_tmp_path {
        std::fs::copy(&plan.build_tmp_path, &plan.final_tmp_path)
            .map_err(|e| format!("copy sqlite temp into final dir: {e}"))?;
        let _ = std::fs::remove_file(&plan.build_tmp_path);
    }
    sync_file_best_effort(&plan.final_tmp_path, "sqlite temp")?;
    std::fs::rename(&plan.final_tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&plan.final_tmp_path);
        let _ = std::fs::remove_file(&plan.build_tmp_path);
        format!("replace sqlite: {e}")
    })?;
    sync_parent_dir(path);
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("stat sqlite: {e}"))
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
    sqlite_build_temp_plan_for(
        path,
        std::env::var_os("MISTER_LIBRARY_SQLITE_BUILD_DIR")
            .map(PathBuf::from)
            .as_deref(),
    )
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
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
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

pub(crate) fn materialize_arcade_ui_projections(
    tx: &rusqlite::Transaction<'_>,
    arcade_preview_archive_path: &str,
    neogeo_preview_archive_path: &str,
) -> Result<(), String> {
    tx.execute(
        r#"
        INSERT INTO ui_arcade_variants(
            family_id,
            variant_ordinal,
            launchable_id,
            title,
            sort_title,
            launch_ref,
            preview_archive_path,
            preview_asset_key,
            has_preview,
            system_id,
            identity_id,
            parent_setname,
            asset_pack_id,
            asset_key,
            asset_link_reason,
            preferred,
            preferred_reason
        )
        WITH candidates AS (
            SELECT
                COALESCE(i.family_id, l.launchable_id) AS family_id,
                l.launchable_id AS launchable_id,
                l.title AS title,
                lower(l.title) AS sort_title,
                l.launch_ref AS launch_ref,
                l.system_id AS system_id,
                l.setname AS setname,
                i.identity_id AS identity_id,
                CASE
                    WHEN i.identity_id IS NOT NULL
                     AND i.family_id IS NOT NULL
                     AND i.identity_id != i.family_id
                    THEN i.family_id
                    ELSE NULL
                END AS parent_setname,
                CASE
                    WHEN i.identity_id IS NOT NULL
                     AND i.identity_id = COALESCE(i.family_id, i.identity_id)
                    THEN 1
                    ELSE 0
                END AS is_parent
            FROM launchables l
            JOIN games g ON g.game_id = l.launchable_id
            LEFT JOIN launchable_identities i
              ON i.launchable_id = l.launchable_id
             AND i.namespace = 'mame'
            WHERE l.system_id IN ('arcade','neogeo')
              AND l.launch_ref != ''
        ),
        resolved AS (
            SELECT
                *,
                CASE
                    WHEN system_id = 'neogeo' THEN ?2
                    ELSE ?1
                END AS preview_archive_path,
                COALESCE(NULLIF(family_id, ''), NULLIF(identity_id, ''), NULLIF(setname, ''), '') AS preview_key
            FROM candidates
        ),
        resolved_with_preview AS (
            SELECT
                *,
                CASE
                    WHEN preview_archive_path != '' AND preview_key != '' THEN 1
                    ELSE 0
                END AS preview_available
            FROM resolved
        ),
        ranked AS (
            SELECT
                *,
                row_number() OVER (
                    PARTITION BY family_id
                    ORDER BY is_parent DESC,
                             sort_title ASC,
                             launch_ref ASC
                ) AS family_rank,
                row_number() OVER (
                    PARTITION BY family_id
                    ORDER BY is_parent DESC,
                             sort_title ASC,
                             launch_ref ASC
                ) - 1 AS variant_ordinal
            FROM resolved_with_preview
        )
        SELECT
            family_id,
            variant_ordinal,
            launchable_id,
            title,
            sort_title,
            launch_ref,
            preview_archive_path,
            preview_key,
            preview_available,
            system_id,
            identity_id,
            parent_setname,
            NULL,
            preview_key,
            CASE WHEN preview_available = 1 THEN 'derived-family' ELSE 'none' END,
            CASE WHEN family_rank = 1 THEN 1 ELSE 0 END,
            CASE
                WHEN family_rank = 1 AND is_parent = 1 THEN 'installed-parent'
                WHEN family_rank = 1 THEN 'deterministic-child'
                ELSE 'variant'
            END
        FROM ranked
        ORDER BY family_id, variant_ordinal;
        "#,
        params![arcade_preview_archive_path, neogeo_preview_archive_path],
    )
    .map_err(|e| format!("materialize arcade ui variants: {e}"))?;
    tx.execute(
        r#"
        INSERT INTO ui_arcade_preferred(
            ordinal,
            launchable_id,
            title,
            sort_title,
            launch_ref,
            preview_archive_path,
            preview_asset_key,
            has_preview,
            system_id,
            identity_id,
            family_id,
            parent_setname,
            asset_pack_id,
            asset_key,
            asset_link_reason,
            preferred_reason
        )
        SELECT
            row_number() OVER (ORDER BY sort_title ASC, launch_ref ASC) - 1,
            launchable_id,
            title,
            sort_title,
            launch_ref,
            preview_archive_path,
            preview_asset_key,
            has_preview,
            system_id,
            identity_id,
            family_id,
            parent_setname,
            asset_pack_id,
            asset_key,
            asset_link_reason,
            preferred_reason
        FROM ui_arcade_variants
        WHERE preferred = 1
        ORDER BY sort_title ASC, launch_ref ASC;
        "#,
        [],
    )
    .map(|_| ())
    .map_err(|e| format!("materialize arcade ui projections: {e}"))
}

pub(crate) fn write_sqlite_scan(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
    software_hash_cache: SoftwareHashCache,
    stamp: Option<&catalog_stamp::CatalogStamp>,
) -> Result<(), String> {
    let preview_paths = PreviewArchivePaths::from_paths(
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
            stamp,
        },
        progress,
    )
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
            stamp: None,
        },
        None,
    )
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
            stamp: None,
        },
        None,
    )
}

#[cfg(test)]
pub(crate) fn write_sqlite_scan_with_mame_and_preview_pack(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
    preview_asset_pack: &preview_worker::PreviewArchiveIndex,
) -> Result<(), String> {
    let preview_paths = PreviewArchivePaths::from_paths(vec![preview_asset_pack.path.clone()]);
    write_sqlite_scan_with_sources(
        path,
        scan,
        SqliteScanSources {
            mame_sqlite_path,
            hbmame_sqlite_path: &PathBuf::new(),
            preview_paths: &preview_paths,
            software_hash_cache: SoftwareHashCache::load(path),
            stamp: None,
        },
        None,
    )
}

struct SqliteScanSources<'a> {
    mame_sqlite_path: &'a Path,
    hbmame_sqlite_path: &'a Path,
    preview_paths: &'a PreviewArchivePaths,
    software_hash_cache: SoftwareHashCache,
    stamp: Option<&'a catalog_stamp::CatalogStamp>,
}

fn write_sqlite_scan_with_sources(
    path: &Path,
    scan: &LibraryScan,
    mut sources: SqliteScanSources<'_>,
    mut progress: ProgressCallback<'_>,
) -> Result<(), String> {
    let total_t = Instant::now();
    let mut conn = Connection::open(path).map_err(|e| format!("open sqlite: {e}"))?;
    let schema_t = Instant::now();
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=OFF;
        PRAGMA synchronous=OFF;
        PRAGMA temp_store=MEMORY;
        PRAGMA locking_mode=EXCLUSIVE;
        CREATE TABLE profiles (
            profile_id TEXT PRIMARY KEY,
            system_id TEXT NOT NULL,
            category TEXT NOT NULL,
            title TEXT NOT NULL,
            core_name TEXT NOT NULL,
            core_path TEXT,
            source_kind TEXT NOT NULL,
            source_detail TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE payloads (
            payload_id TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            entry_path TEXT,
            launch_ref TEXT NOT NULL,
            profile_id TEXT,
            title TEXT NOT NULL,
            mount_kind TEXT,
            mount_index INTEGER,
            mount_delay_secs INTEGER,
            disposition TEXT NOT NULL,
            size INTEGER NOT NULL,
            mtime_secs INTEGER NOT NULL,
            source_kind TEXT NOT NULL,
            source_detail TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE systems (
            system_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            category TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE games (
            game_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            sort_title TEXT NOT NULL,
            system_id TEXT NOT NULL,
            manufacturer TEXT,
            genre TEXT,
            year INTEGER
        ) WITHOUT ROWID;
        CREATE TABLE launch_plans (
            plan_id TEXT PRIMARY KEY,
            game_id TEXT NOT NULL,
            profile_id TEXT,
            launch_kind TEXT NOT NULL,
            source_path TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            launcher_path TEXT,
            payload_path TEXT,
            core_id TEXT NOT NULL,
            hardware_id TEXT NOT NULL,
            setname TEXT,
            parent TEXT,
            priority INTEGER NOT NULL,
            confidence TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE launchables (
            launchable_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            system_id TEXT NOT NULL,
            launch_kind TEXT NOT NULL,
            source_path TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            setname TEXT,
            core_id TEXT NOT NULL,
            hardware_id TEXT NOT NULL,
            confidence TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE launchable_identities (
            launchable_id TEXT NOT NULL,
            namespace TEXT NOT NULL,
            identity_id TEXT NOT NULL,
            family_id TEXT,
            metadata_title TEXT,
            year TEXT,
            manufacturer TEXT,
            source TEXT NOT NULL,
            PRIMARY KEY(launchable_id, namespace, identity_id)
        ) WITHOUT ROWID;
        CREATE TABLE ui_arcade_preferred (
            ordinal INTEGER PRIMARY KEY,
            launchable_id TEXT NOT NULL,
            title TEXT NOT NULL,
            sort_title TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            preview_archive_path TEXT NOT NULL,
            preview_asset_key TEXT NOT NULL,
            has_preview INTEGER NOT NULL,
            system_id TEXT NOT NULL,
            identity_id TEXT,
            family_id TEXT NOT NULL,
            parent_setname TEXT,
            asset_pack_id TEXT,
            asset_key TEXT,
            asset_link_reason TEXT NOT NULL,
            preferred_reason TEXT NOT NULL
        );
        CREATE TABLE ui_arcade_variants (
            family_id TEXT NOT NULL,
            variant_ordinal INTEGER NOT NULL,
            launchable_id TEXT NOT NULL,
            title TEXT NOT NULL,
            sort_title TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            preview_archive_path TEXT NOT NULL,
            preview_asset_key TEXT NOT NULL,
            has_preview INTEGER NOT NULL,
            system_id TEXT NOT NULL,
            identity_id TEXT,
            parent_setname TEXT,
            asset_pack_id TEXT,
            asset_key TEXT,
            asset_link_reason TEXT NOT NULL,
            preferred INTEGER NOT NULL,
            preferred_reason TEXT NOT NULL,
            PRIMARY KEY(family_id, variant_ordinal)
        ) WITHOUT ROWID;
        CREATE TABLE launcher_catalog (
            ordinal INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            sort_title TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            preview_archive_path TEXT NOT NULL,
            preview_asset_key TEXT NOT NULL,
            has_preview INTEGER NOT NULL,
            system_id TEXT NOT NULL
        );
        CREATE TABLE region_metadata (
            game_id TEXT PRIMARY KEY,
            inferred_region TEXT,
            confidence TEXT NOT NULL,
            override_region TEXT
        ) WITHOUT ROWID;
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
            ordinal INTEGER PRIMARY KEY,
            line TEXT NOT NULL
        );
        "#,
    )
    .map_err(|e| format!("create sqlite schema: {e}"))?;
    report_library_import_timing("schema", schema_t, "tables=14");

    let metadata_t = Instant::now();
    let mame_signature = library_db::file_signature(sources.mame_sqlite_path);
    let hbmame_signature = library_db::file_signature(sources.hbmame_sqlite_path);
    let software_metadata = load_mame_software_metadata(sources.mame_sqlite_path);
    let arcade_metadata =
        load_arcade_machine_metadata(sources.mame_sqlite_path, sources.hbmame_sqlite_path);
    report_library_import_timing(
        "metadata_load",
        metadata_t,
        format!(
            "mame={} hbmame={} software_lists={} preview_paths={}",
            arcade_metadata.mame.len(),
            arcade_metadata.hbmame.len(),
            software_metadata.items.len(),
            sources.preview_paths.len()
        ),
    );
    let tx_t = Instant::now();
    let tx = conn
        .transaction()
        .map_err(|e| format!("begin sqlite tx: {e}"))?;
    report_library_import_timing("begin_tx", tx_t, "");
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare(
                "INSERT INTO profiles(profile_id,system_id,category,title,core_name,core_path,source_kind,source_detail)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(|e| format!("prepare profile insert: {e}"))?;
        for profile in launch_profiles::builtin_profiles() {
            stmt.execute(params![
                profile.id,
                profile.system_id,
                profile.category,
                profile.title,
                profile.core_name,
                profile.core_path,
                source_kind_name(profile.provenance.kind),
                profile.provenance.detail
            ])
            .map_err(|e| format!("insert profile: {e}"))?;
        }
        report_library_import_timing(
            "insert_profiles",
            stage_t,
            format!("rows={}", launch_profiles::builtin_profiles().len()),
        );
    }
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare(
                "INSERT INTO payloads(payload_id,file_path,entry_path,launch_ref,profile_id,title,mount_kind,mount_index,mount_delay_secs,disposition,size,mtime_secs,source_kind,source_detail)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            )
            .map_err(|e| format!("prepare payload insert: {e}"))?;
        for payload in &scan.normal_files {
            let path = &payload.path;
            stmt.execute(params![
                format!("file:{path}"),
                path.as_str(),
                Option::<&str>::None,
                path.as_str(),
                payload.profile_id.as_str(),
                library_db::title_from_path(path),
                mount_kind_str(payload.rule.mount.kind),
                payload.rule.mount.index as i64,
                payload.rule.mount.delay_secs as i64,
                payload_disposition_str(payload.rule.disposition),
                payload.size as i64,
                payload.mtime_secs,
                source_kind_name(payload.rule.provenance.kind),
                payload.rule.provenance.detail
            ])
            .map_err(|e| format!("insert payload file: {e}"))?;
        }
        for entry in &scan.entries {
            stmt.execute(params![
                format!("entry:{}", entry.launch_ref),
                entry.file_path.as_str(),
                entry.entry_path.as_str(),
                entry.launch_ref.as_str(),
                entry.profile_id.as_str(),
                entry.normalized_title.as_str(),
                mount_kind_str(entry.rule.mount.kind),
                entry.rule.mount.index as i64,
                entry.rule.mount.delay_secs as i64,
                if entry.launchable {
                    "candidate"
                } else {
                    "support"
                },
                entry
                    .uncompressed_size
                    .or(entry.compressed_size)
                    .unwrap_or(0) as i64,
                0i64,
                source_kind_name(entry.rule.provenance.kind),
                entry.rule.provenance.detail
            ])
            .map_err(|e| format!("insert payload entry: {e}"))?;
        }
        report_library_import_timing(
            "insert_payloads",
            stage_t,
            format!(
                "normal_files={} entries={}",
                scan.normal_files.len(),
                scan.entries.len()
            ),
        );
    }
    {
        let stage_t = Instant::now();
        let mut launcher_rows = Vec::<CatalogRow>::new();
        let mut system_stmt = tx
            .prepare("INSERT OR IGNORE INTO systems(system_id,title,category) VALUES (?1,?2,?3)")
            .map_err(|e| format!("prepare system insert: {e}"))?;
        let mut game_stmt = tx
            .prepare(
                "INSERT INTO games(game_id,title,sort_title,system_id,manufacturer,genre,year)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| format!("prepare game insert: {e}"))?;
        let mut plan_stmt = tx
            .prepare(
                "INSERT INTO launch_plans(plan_id,game_id,profile_id,launch_kind,source_path,launch_ref,launcher_path,payload_path,core_id,hardware_id,setname,parent,priority,confidence)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            )
            .map_err(|e| format!("prepare launch plan insert: {e}"))?;
        let mut launchable_stmt = tx
            .prepare(
                "INSERT INTO launchables(launchable_id,title,system_id,launch_kind,source_path,launch_ref,setname,core_id,hardware_id,confidence)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )
            .map_err(|e| format!("prepare launchable insert: {e}"))?;
        let mut identity_stmt = tx
            .prepare(
                "INSERT INTO launchable_identities(launchable_id,namespace,identity_id,family_id,metadata_title,year,manufacturer,source)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(|e| format!("prepare launchable identity insert: {e}"))?;
        let mut region_stmt = tx
            .prepare(
                "INSERT INTO region_metadata(game_id,inferred_region,confidence,override_region)
                 VALUES (?1,?2,?3,?4)",
            )
            .map_err(|e| format!("prepare region metadata insert: {e}"))?;
        let covered_payloads = covered_payload_paths(&scan.discoveries);
        let discoveries =
            preferred_playable_discoveries_by_key(&scan.discoveries, &covered_payloads);
        let discovery_total = discoveries.len();
        report_sqlite_import_progress(&mut progress, 0, discovery_total);
        let mut chunk_t = Instant::now();
        let mut chunk_start = 0usize;
        for (idx, (key, discovery)) in discoveries.into_iter().enumerate() {
            if idx > 0 && idx % 250 == 0 {
                report_sqlite_import_progress(&mut progress, idx, discovery_total);
            }
            let system_id = catalog_system_id_for_discovery(discovery);
            let software_identity = mame_software_identity_for_discovery(
                discovery,
                &software_metadata,
                &mut sources.software_hash_cache,
            );
            let preview_asset = software_identity
                .as_ref()
                .and_then(|identity| console_preview_asset(identity, sources.preview_paths));
            let game_has_preview = preview_asset.is_some();
            system_stmt
                .execute(params![
                    system_id.as_str(),
                    system_title_for_discovery(discovery, &system_id),
                    discovery.category.as_str()
                ])
                .map_err(|e| format!("insert system: {e}"))?;
            game_stmt
                .execute(params![
                    key.as_str(),
                    discovery.title.as_str(),
                    library_db::normalize_title(&discovery.title),
                    system_id.as_str(),
                    discovery.manufacturer.as_deref(),
                    discovery.genre.as_deref(),
                    discovery.year.map(|n| n as i64)
                ])
                .map_err(|e| format!("insert game: {e}"))?;
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
            if is_launcher_launch_ref(&plan_launch_ref)
                && system_id != "arcade"
                && system_id != "neogeo"
            {
                let software_family_key = software_identity
                    .as_ref()
                    .map(|identity| format!("mame-software:{}", identity.family_id));
                launcher_rows.push(CatalogRow {
                    game: ArcadeGameEntry {
                        title: discovery.title.clone().into(),
                        mra_path: plan_launch_ref.clone().into(),
                        preview_archive_path: preview_asset
                            .as_ref()
                            .map(|asset| asset.archive_path.as_str())
                            .unwrap_or_default()
                            .into(),
                        preview_asset_key: preview_asset
                            .as_ref()
                            .map(|asset| asset.asset_key.as_str())
                            .unwrap_or_default()
                            .into(),
                        has_preview: game_has_preview,
                        system_id: system_id.clone().into(),
                    },
                    source_kind: launch_kind_for_discovery(discovery).to_string(),
                    setname: discovery.setname.clone().unwrap_or_default(),
                    parent: discovery.parent.clone().unwrap_or_default(),
                    family_key: software_family_key,
                });
            }
            plan_stmt
                .execute(params![
                    format!("plan:{key}"),
                    key.as_str(),
                    profile_id_for_discovery(discovery),
                    launch_kind_for_discovery(discovery),
                    discovery.source_path.as_str(),
                    plan_launch_ref.as_str(),
                    launcher_path,
                    payload_path,
                    discovery.core_id.as_str(),
                    discovery.hardware_id.as_str(),
                    discovery.setname.as_deref(),
                    discovery.parent.as_deref(),
                    0i64,
                    confidence_str(discovery.confidence)
                ])
                .map_err(|e| format!("insert launch plan: {e}"))?;
            launchable_stmt
                .execute(params![
                    key.as_str(),
                    discovery.title.as_str(),
                    system_id.as_str(),
                    launch_kind_for_discovery(discovery),
                    discovery.source_path.as_str(),
                    plan_launch_ref.as_str(),
                    discovery.setname.as_deref(),
                    discovery.core_id.as_str(),
                    discovery.hardware_id.as_str(),
                    confidence_str(discovery.confidence)
                ])
                .map_err(|e| format!("insert launchable: {e}"))?;
            if let Some(identity_id) = mame_identity_for_discovery(discovery) {
                let (family_id, title, year, manufacturer, source) = mame_identity_projection(
                    &identity_id,
                    &arcade_metadata,
                    discovery.parent.as_deref(),
                );
                identity_stmt
                    .execute(params![
                        key.as_str(),
                        "mame",
                        identity_id.as_str(),
                        family_id.as_str(),
                        title,
                        year,
                        manufacturer,
                        source
                    ])
                    .map_err(|e| format!("insert launchable identity: {e}"))?;
            }
            if let Some(identity) = software_identity.as_ref() {
                let identity_id = format!("{}:{}", identity.list_name, identity.software_name);
                identity_stmt
                    .execute(params![
                        key.as_str(),
                        "mame-software",
                        identity_id.as_str(),
                        identity.family_id.as_str(),
                        identity.metadata_title.as_deref(),
                        identity.year.as_deref(),
                        identity.manufacturer.as_deref(),
                        identity.source
                    ])
                    .map_err(|e| format!("insert software launchable identity: {e}"))?;
            }
            let region = media_metadata::infer_region_metadata(discovery);
            let region = if let Some(identity) = software_identity.as_ref() {
                if let Some(region) = identity
                    .region
                    .as_deref()
                    .and_then(media_metadata::canonical_region_static)
                {
                    media_metadata::RegionInference {
                        region: Some(region),
                        confidence: identity.source,
                    }
                } else {
                    region
                }
            } else {
                region
            };
            region_stmt
                .execute(params![
                    key.as_str(),
                    region.region,
                    region.confidence,
                    Option::<&str>::None
                ])
                .map_err(|e| format!("insert region metadata: {e}"))?;
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
        drop(region_stmt);
        drop(identity_stmt);
        drop(launchable_stmt);
        drop(plan_stmt);
        drop(game_stmt);
        drop(system_stmt);
        report_library_import_timing(
            "insert_games_total",
            stage_t,
            format!(
                "rows={discovery_total} launcher_rows={}",
                launcher_rows.len()
            ),
        );
        report_sqlite_import_finalizing(&mut progress);
        let projection_t = Instant::now();
        materialize_arcade_ui_projections(
            &tx,
            sources
                .preview_paths
                .archive_for_platform("arcade")
                .unwrap_or_default(),
            sources
                .preview_paths
                .archive_for_platform("neogeo")
                .unwrap_or_default(),
        )?;
        report_library_import_timing("materialize_arcade_ui", projection_t, "");
        let launcher_arcade_t = Instant::now();
        tx.execute(
            "INSERT INTO launcher_catalog(ordinal,title,sort_title,launch_ref,preview_archive_path,preview_asset_key,has_preview,system_id)
             SELECT ordinal,title,sort_title,launch_ref,preview_archive_path,preview_asset_key,has_preview,system_id
             FROM ui_arcade_preferred
             ORDER BY ordinal",
            [],
        )
        .map_err(|e| format!("insert preferred launcher catalog: {e}"))?;
        report_library_import_timing("insert_launcher_arcade", launcher_arcade_t, "");
        let ordinal_offset = tx
            .query_row("SELECT count(*) FROM launcher_catalog", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|e| format!("query launcher catalog offset: {e}"))?;
        let launcher_console_t = Instant::now();
        launcher_rows.sort_by_cached_key(|row| row.game.title.to_ascii_lowercase());
        let launcher_games = library_db::collapse_catalog_variants(launcher_rows);
        let mut launcher_stmt = tx
            .prepare(
                "INSERT INTO launcher_catalog(ordinal,title,sort_title,launch_ref,preview_archive_path,preview_asset_key,has_preview,system_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(|e| format!("prepare launcher catalog insert: {e}"))?;
        for (idx, game) in launcher_games.iter().enumerate() {
            launcher_stmt
                .execute(params![
                    ordinal_offset + idx as i64,
                    game.title.as_ref(),
                    library_db::normalize_title(&game.title),
                    game.mra_path.as_ref(),
                    game.preview_archive_path.as_ref(),
                    game.preview_asset_key.as_ref(),
                    if game.has_preview { 1 } else { 0 },
                    game.system_id.as_ref()
                ])
                .map_err(|e| format!("insert launcher catalog: {e}"))?;
        }
        report_library_import_timing(
            "insert_launcher_console",
            launcher_console_t,
            format!("rows={}", launcher_games.len()),
        );
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
        stmt.execute(params!["ignored_files", scan.ignored_files as i64])
            .map_err(|e| format!("insert ignored count: {e}"))?;
        stmt.execute(params![
            "discoveries",
            unique_discovery_count(&scan.discoveries) as i64
        ])
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
        report_library_import_timing("insert_meta", stage_t, "rows=11");
    }
    if let Some(stamp) = sources.stamp {
        let stage_t = Instant::now();
        catalog_store::write_catalog_stamp(&tx, stamp)?;
        report_library_import_timing(
            "insert_catalog_stamp",
            stage_t,
            format!("rows={}", stamp.lines().len()),
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
    let commit_t = Instant::now();
    tx.commit().map_err(|e| format!("commit sqlite tx: {e}"))?;
    report_library_import_timing("commit", commit_t, "");
    report_library_import_timing("total", total_t, format!("path={}", path.display()));
    Ok(())
}

pub(crate) fn report_library_import_timing(
    stage: &str,
    started: Instant,
    detail: impl std::fmt::Display,
) {
    println!(
        "library_import_timing\t{stage}\t{}\t{detail}",
        started.elapsed().as_micros()
    );
}

fn report_sqlite_import_progress(
    progress: &mut ProgressCallback<'_>,
    written: usize,
    total: usize,
) {
    if let Some(report) = progress.as_mut() {
        report(
            "Saving library",
            &format!("Writing {written} of {total} games into SQLite..."),
        );
    }
}

fn report_sqlite_import_finalizing(progress: &mut ProgressCallback<'_>) {
    if let Some(report) = progress.as_mut() {
        report(
            "Saving library",
            "Finalizing catalog views and search indexes...",
        );
    }
}

pub(crate) fn sqlite_cached_summary(
    path: &Path,
    scan_us: u64,
) -> Result<LibraryRefreshSummary, String> {
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
        discoveries: sqlite_meta_usize(&conn, "discoveries").unwrap_or(0),
    })
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

pub(crate) fn mount_kind_str(kind: MountKind) -> &'static str {
    match kind {
        MountKind::Launcher => "launcher",
        MountKind::LoadFile => "load-file",
        MountKind::MountImage => "mount-image",
        MountKind::Core => "core",
    }
}

pub(crate) fn payload_disposition_str(disposition: PayloadDisposition) -> &'static str {
    match disposition {
        PayloadDisposition::Playable => "playable",
        PayloadDisposition::AttachedMedia => "attached-media",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog_config::{DEFAULT_SQLITE_BUILD_DIR, SCHEMA_VERSION};
    use crate::library_db::{
        save_scan_artifact_to_sqlite, scan_library_artifact, BenchConfig, ProgressCallback,
    };
    use crate::preview_worker;
    use crate::test_support::*;
    use rusqlite::Connection;
    use std::path::PathBuf;

    #[test]
    fn sqlite_save_keeps_previous_database_when_replacement_fails() {
        let root = unique_temp_dir("sqlite-atomic-replace");
        let db = root.join("library.sqlite3");
        save_sqlite_scan(&db, &sqlite_scan_with_normal_files(&["/old/game.mra"]))
            .expect("write old database");
        let old_summary = sqlite_cached_summary(&db, 0).expect("old database readable");
        assert_eq!(old_summary.normal_files, 1);

        let err = save_sqlite_scan(
            &db,
            &sqlite_scan_with_normal_files(&["/new/game.mra", "/new/game.mra"]),
        )
        .expect_err("duplicate normal_files row should fail temp import");

        assert!(
            err.contains("insert payload file"),
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
    fn sqlite_build_temp_defaults_to_tmpfs_for_media_fat_database() {
        let path = Path::new("/media/fat/mister-magik/library.sqlite3");
        let plan = sqlite_build_temp_plan_for(path, None);

        assert_eq!(plan.source, SqliteBuildTempSource::DefaultTmpfs);
        assert!(plan
            .build_tmp_path
            .starts_with(Path::new(DEFAULT_SQLITE_BUILD_DIR)));
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
            Err("insert payload file: UNIQUE constraint failed".to_string())
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
            err.contains("insert payload file"),
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
        assert!(loaded
            .catalog
            .games
            .iter()
            .any(|game| game.title.as_ref() == "Game 20004"));
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
            path: root
                .join("320x320-screenshots.mmlz4b")
                .display()
                .to_string(),
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
        let conn = Connection::open(&db).expect("open sqlite");
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
        let conn = Connection::open(&db).expect("open sqlite");
        let plans = load_virtual_launch_plans_for_system_from_conn(&conn, "saturn", 8)
            .expect("load virtual launch plans");

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].system_id, "saturn");
        assert_eq!(plans[0].core_path, "_Console/Saturn");
        assert_eq!(plans[0].payload_path, "/media/fat/games/Saturn/Nights.chd");
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

        let conn = Connection::open(&db).expect("open library sqlite");
        let preferred = conn
            .query_row(
                "SELECT identity_id,family_id,preferred_reason,title,has_preview
                 FROM ui_arcade_preferred",
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
            path: root
                .join("320x320-screenshots.mmlz4b")
                .display()
                .to_string(),
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

        let conn = Connection::open(&db).expect("open library sqlite");
        let preferred = conn
            .query_row(
                "SELECT identity_id,family_id,preferred_reason,has_preview
                 FROM ui_arcade_preferred",
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
