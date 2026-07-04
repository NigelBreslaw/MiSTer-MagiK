//! Library catalog CLI helpers.

use crate::catalog_config::default_sqlite_path;
use crate::catalog_scan;
use crate::library_db::{self, BenchConfig};
use crate::sqlite_inspect::{
    append_sqlite_timing_row, sqlite_cell_to_string, sqlite_query_hash,
    sqlite_query_is_read_only as sqlite_inspect_query_is_read_only, SqliteInspectTiming,
};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub(crate) fn run_scan_bench() {
    let cfg = BenchConfig::from_env();
    let label =
        std::env::var("MISTER_LIBRARY_BENCH_LABEL").unwrap_or_else(|_| "LIB-BENCH".to_string());
    let iterations = std::env::var("MISTER_LIBRARY_BENCH_ITERATIONS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let bench_force_rebuild = library_db::env_bool("MISTER_LIBRARY_BENCH_FORCE_REBUILD");
    let bench_precount = library_db::env_bool("MISTER_LIBRARY_BENCH_PRECOUNT");
    crate::catalog_logln!("library-scan-bench label={label}");
    crate::catalog_logln!("library-scan-bench roots={}", cfg.roots.join("|"));
    crate::catalog_logln!(
        "library-scan-bench sqlite_path={}",
        cfg.sqlite_path.display()
    );
    for iteration in 1..=iterations {
        match std::fs::remove_file(&cfg.sqlite_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => crate::catalog_errln!("library-scan-bench remove old sqlite: {e}"),
        }

        if bench_precount {
            let (candidates, dirs, precount_us) =
                catalog_scan::precount_discovery_candidates(&cfg.roots);
            crate::catalog_logln!(
                "library_scan_bench_tsv\t{label}\t{iteration}\tprecount_discovery\t{precount_us}\tcandidates={candidates}\tdirs={dirs}"
            );
        }

        let build_t = Instant::now();
        let artifact = library_db::scan_library_artifact(&cfg, None);
        let stats = artifact.stats().clone();
        let build_us = build_t.elapsed().as_micros() as u64;

        let import_t = Instant::now();
        std::env::set_var(
            "MISTER_LIBRARY_BENCH_ACTIVE_ITERATION",
            iteration.to_string(),
        );
        let summary = match library_db::save_scan_artifact_to_sqlite(&cfg, artifact, None) {
            Ok(summary) => summary,
            Err(e) => {
                std::env::remove_var("MISTER_LIBRARY_BENCH_ACTIVE_ITERATION");
                crate::catalog_logln!(
                    "library_scan_bench_tsv\t{label}\t{iteration}\timport_error\t{}\t{e}",
                    import_t.elapsed().as_micros()
                );
                continue;
            }
        };
        std::env::remove_var("MISTER_LIBRARY_BENCH_ACTIVE_ITERATION");
        let import_us = import_t.elapsed().as_micros() as u64;
        let bytes = summary.bytes;

        let load_t = Instant::now();
        let loaded = library_db::load_arcade_catalog_from_sqlite("/media/fat/_Arcade");
        let (load_us, arcade_rows) = match loaded {
            Ok(load) => (load.us, load.rows),
            Err(e) => {
                crate::catalog_errln!("library-scan-bench arcade load failed: {e}");
                (load_t.elapsed().as_micros() as u64, 0)
            }
        };

        let stamp_t = Instant::now();
        let stamp_check = library_db::sqlite_catalog_stamp_check(&cfg);
        let stamp_us = stamp_t.elapsed().as_micros() as u64;

        let force_rebuild = if bench_force_rebuild {
            let change_dir = Path::new(&cfg.roots[0]).join("games/NES");
            let change_parent = if change_dir.is_dir() {
                change_dir
            } else {
                PathBuf::from(&cfg.roots[0])
            };
            let change_path =
                change_parent.join(format!("Mister_Magik_Refresh_Bench_{iteration}.nes"));
            if let Err(e) = std::fs::write(&change_path, b"[mister]\nrbf=menu\n") {
                crate::catalog_errln!(
                    "library-scan-bench force rebuild setup failed at {}: {e}",
                    change_path.display()
                );
            }
            let force_rebuild_t = Instant::now();
            let summary = library_db::rebuild_sqlite_database(&cfg, None);
            Some((force_rebuild_t.elapsed().as_micros() as u64, summary))
        } else {
            None
        };

        crate::catalog_logln!(
            "library_scan_bench_tsv\t{label}\t{iteration}\tfresh_build\t{build_us}\tdiscover_us={}\tclassify_us={}\tnormal_files={}\tcontainers={}\tentries={}\taudit_rows={}\tdiscoveries={}",
            stats.discover_us,
            stats.classify_us,
            stats.normal_files,
            stats.containers,
            stats.entries,
            stats.audit_rows,
            stats.discoveries
        );
        crate::catalog_logln!(
            "library_scan_bench_tsv\t{label}\t{iteration}\timport\t{import_us}\tbytes={bytes}"
        );
        crate::catalog_logln!(
            "library_scan_bench_tsv\t{label}\t{iteration}\tcached_arcade_load\t{load_us}\trows={arcade_rows}"
        );
        match stamp_check {
            Ok(check) => crate::catalog_logln!(
                "library_scan_bench_tsv\t{label}\t{iteration}\troot_stamp_check\t{stamp_us}\tunchanged={} check_us={} compute_us={} open_us={} read_us={} checkpoint_read_us={} compare_us={} checkpoint_compare_us={} stored={} current={} stored_checkpoint={} current_checkpoint={} stored_lines={} current_lines={} stored_checkpoint_lines={} current_checkpoint_lines={} drift_detail={}",
                check.unchanged,
                check.check_us,
                check.compute_us,
                check.open_us,
                check.read_us,
                check.checkpoint_read_us,
                check.compare_us,
                check.checkpoint_compare_us,
                check.stored_fingerprint.as_deref().unwrap_or("missing"),
                check.current_fingerprint,
                check.stored_checkpoint_fingerprint.as_deref().unwrap_or("missing"),
                check.current_checkpoint_fingerprint,
                check.stored_lines,
                check.current_lines,
                check.stored_checkpoint_lines,
                check.current_checkpoint_lines,
                check.drift.detail
            ),
            Err(e) => crate::catalog_logln!(
                "library_scan_bench_tsv\t{label}\t{iteration}\troot_stamp_check_error\t{stamp_us}\t{e}"
            ),
        }
        if let Some((force_rebuild_us, force_rebuild_summary)) = force_rebuild {
            match force_rebuild_summary {
                Ok(summary) => crate::catalog_logln!(
                    "library_scan_bench_tsv\t{label}\t{iteration}\tforce_rebuild\t{force_rebuild_us}\tscan_us={}\tdiscover_us={}\tclassify_us={}\timport_us={}\tskipped={}\tdiscoveries={}",
                    summary.scan_us,
                    summary.discover_us,
                    summary.classify_us,
                    summary.import_us,
                    summary.skipped,
                    summary.discoveries
                ),
                Err(e) => crate::catalog_logln!(
                    "library_scan_bench_tsv\t{label}\t{iteration}\tforce_rebuild_error\t{force_rebuild_us}\t{e}"
                ),
            }
        }
    }
}

pub(crate) fn run_sqlite_inspect_cli(args: &[String]) -> Result<String, String> {
    let mut path = default_sqlite_path();
    let mut query_parts = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--path" => {
                let Some(value) = args.get(i + 1) else {
                    return Err("library-sql: --path needs a value".into());
                };
                path = PathBuf::from(value);
                i += 2;
            }
            other => {
                query_parts.push(other.to_string());
                i += 1;
            }
        }
    }
    if query_parts.is_empty() {
        return Err("usage: library-sql [--path PATH] SELECT ...".into());
    }
    let query = query_parts.join(" ");
    if !sqlite_inspect_query_is_read_only(&query) {
        return Err("library-sql only allows read-only SELECT/WITH queries".into());
    }

    let total_t = Instant::now();
    let metadata = std::fs::metadata(&path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    if metadata.len() == 0 {
        return Err(format!("{} is empty", path.display()));
    }

    let open_t = Instant::now();
    let conn = library_db::open_sqlite_read_only(&path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let open_us = open_t.elapsed().as_micros() as u64;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    let prepare_t = Instant::now();
    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| format!("prepare query: {e}"))?;
    let prepare_us = prepare_t.elapsed().as_micros() as u64;
    let column_count = stmt.column_count();
    let mut out = String::new();
    if column_count > 0 {
        let format_t = Instant::now();
        out.push_str(&stmt.column_names().join("\t"));
        out.push('\n');
        let mut format_us = format_t.elapsed().as_micros() as u64;
        let mut row_read_us = 0u64;
        let mut first_row_us = 0u64;
        let mut row_count = 0usize;
        let mut rows = stmt.query([]).map_err(|e| format!("run query: {e}"))?;
        let first_row_t = Instant::now();
        loop {
            let read_t = Instant::now();
            let row = rows.next().map_err(|e| format!("read row: {e}"))?;
            let read_us = read_t.elapsed().as_micros() as u64;
            row_read_us += read_us;
            if row_count == 0 {
                first_row_us = first_row_t.elapsed().as_micros() as u64;
            }
            let Some(row) = row else {
                break;
            };
            row_count += 1;
            let format_t = Instant::now();
            for col in 0..column_count {
                if col > 0 {
                    out.push('\t');
                }
                out.push_str(
                    &sqlite_cell_to_string(row, col)
                        .map_err(|e| format!("read column {col}: {e}"))?,
                );
            }
            out.push('\n');
            format_us += format_t.elapsed().as_micros() as u64;
        }
        let stdout_bytes = out.len();
        append_sqlite_timing_row(
            &mut out,
            SqliteInspectTiming {
                path: &path,
                db_bytes: metadata.len(),
                query_hash: sqlite_query_hash(&query),
                open_us,
                schema_check_us: None,
                prepare_us,
                first_row_us,
                row_read_us,
                format_us,
                total_us: total_t.elapsed().as_micros() as u64,
                rows: row_count,
                columns: column_count,
                stdout_bytes,
            },
        );
        return Ok(out);
    }
    let mut row_read_us = 0u64;
    let mut first_row_us = 0u64;
    let mut row_count = 0usize;
    let mut rows = stmt.query([]).map_err(|e| format!("run query: {e}"))?;
    let first_row_t = Instant::now();
    loop {
        let read_t = Instant::now();
        let row = rows.next().map_err(|e| format!("read row: {e}"))?;
        row_read_us += read_t.elapsed().as_micros() as u64;
        if row_count == 0 {
            first_row_us = first_row_t.elapsed().as_micros() as u64;
        }
        if row.is_none() {
            break;
        }
        row_count += 1;
    }
    let stdout_bytes = out.len();
    append_sqlite_timing_row(
        &mut out,
        SqliteInspectTiming {
            path: &path,
            db_bytes: metadata.len(),
            query_hash: sqlite_query_hash(&query),
            open_us,
            schema_check_us: None,
            prepare_us,
            first_row_us,
            row_read_us,
            format_us: 0,
            total_us: total_t.elapsed().as_micros() as u64,
            rows: row_count,
            columns: column_count,
            stdout_bytes,
        },
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use rusqlite::Connection;

    #[test]
    fn sqlite_inspect_does_not_create_missing_database() {
        let root = unique_temp_dir("sqlite-inspect-missing");
        let db = root.join("library.sqlite3");

        let err = run_sqlite_inspect_cli(&[
            "--path".to_string(),
            db.display().to_string(),
            "SELECT 1".to_string(),
        ])
        .expect_err("missing database should fail before sqlite open");

        assert!(err.starts_with("stat "), "unexpected error: {err}");
        assert!(!db.exists(), "read-only inspect must not create database");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_inspect_rejects_empty_database() {
        let root = unique_temp_dir("sqlite-inspect-empty");
        let db = root.join("library.sqlite3");
        std::fs::write(&db, b"").expect("write empty database");

        let err = run_sqlite_inspect_cli(&[
            "--path".to_string(),
            db.display().to_string(),
            "SELECT 1".to_string(),
        ])
        .expect_err("empty database should fail before sqlite open");

        assert!(
            err.ends_with(" is empty"),
            "unexpected empty database error: {err}"
        );
        assert_eq!(std::fs::metadata(&db).expect("metadata").len(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_inspect_formats_common_cell_types() {
        let root = unique_temp_dir("sqlite-inspect-cell-types");
        let db = root.join("library.sqlite3");
        let conn = Connection::open(&db).expect("open sqlite");
        conn.execute_batch(
            "CREATE TABLE values_fixture(
                int_value INTEGER,
                real_value REAL,
                text_value TEXT,
                blob_value BLOB,
                null_value TEXT
             );
             INSERT INTO values_fixture VALUES(42, 1.5, 'hello', x'010203', NULL);",
        )
        .expect("create inspect fixture");
        drop(conn);

        let out = run_sqlite_inspect_cli(&[
            "--path".to_string(),
            db.display().to_string(),
            "SELECT int_value, real_value, text_value, blob_value, null_value".to_string(),
            "FROM values_fixture".to_string(),
        ])
        .expect("inspect sqlite fixture");

        assert!(
            out.starts_with(
                "int_value\treal_value\ttext_value\tblob_value\tnull_value\n42\t1.5\thello\t<blob:3>\t\n"
            ),
            "{out}"
        );
        let timing = out.lines().last().expect("timing row");
        assert_sqlite_timing_row(timing, &db, 1, 5);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_inspect_appends_timing_row() {
        let root = unique_temp_dir("sqlite-inspect-timing");
        let db = root.join("library.sqlite3");
        let conn = Connection::open(&db).expect("open sqlite");
        conn.execute_batch(
            "CREATE TABLE values_fixture(value INTEGER);
             INSERT INTO values_fixture VALUES(1),(2);",
        )
        .expect("create inspect fixture");
        drop(conn);

        let out = run_sqlite_inspect_cli(&[
            "--path".to_string(),
            db.display().to_string(),
            "SELECT value FROM values_fixture ORDER BY value".to_string(),
        ])
        .expect("inspect sqlite fixture");

        assert!(out.starts_with("value\n1\n2\n"), "{out}");
        let timing = out.lines().last().expect("timing row");
        assert_sqlite_timing_row(timing, &db, 2, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_inspect_allows_comments_before_select_and_with_select() {
        assert!(sqlite_inspect_query_is_read_only(
            "-- comment\n/* more */ SELECT 'delete from games'"
        ));
        assert!(sqlite_inspect_query_is_read_only(
            "WITH recent AS (SELECT 1) SELECT * FROM recent"
        ));
    }

    #[test]
    fn sqlite_inspect_rejects_with_write_statements() {
        for query in [
            "WITH doomed AS (SELECT 1) DELETE FROM games",
            "WITH changed AS (SELECT 1) UPDATE games SET title='x'",
            "WITH created AS (SELECT 1) INSERT INTO games(title) VALUES('x')",
            "SELECT 1; DELETE FROM games",
            "/* comment */ PRAGMA writable_schema=ON",
        ] {
            assert!(!sqlite_inspect_query_is_read_only(query), "{query}");
        }
    }

    fn assert_sqlite_timing_row(row: &str, db: &Path, rows: usize, columns: usize) {
        let fields = row.split('\t').collect::<Vec<_>>();
        assert_eq!(fields.len(), 14, "{row}");
        assert_eq!(fields[0], "library_sql_timing_tsv");
        assert_eq!(fields[1], db.display().to_string());
        assert!(fields[2].parse::<u64>().expect("db bytes") > 0);
        assert_eq!(fields[3].len(), 16);
        assert!(u64::from_str_radix(fields[3], 16).is_ok());
        assert!(fields[4].parse::<u64>().is_ok(), "open_us");
        assert_eq!(fields[5], "");
        assert!(fields[6].parse::<u64>().is_ok(), "prepare_us");
        assert!(fields[7].parse::<u64>().is_ok(), "first_row_us");
        assert!(fields[8].parse::<u64>().is_ok(), "row_read_us");
        assert!(fields[9].parse::<u64>().is_ok(), "format_us");
        assert!(fields[10].parse::<u64>().is_ok(), "total_us");
        assert_eq!(fields[11].parse::<usize>().expect("rows"), rows);
        assert_eq!(fields[12].parse::<usize>().expect("columns"), columns);
        assert!(fields[13].parse::<usize>().is_ok(), "stdout_bytes");
    }
}
