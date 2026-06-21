//! Library catalog CLI helpers.

use crate::arcade_catalog::ArcadeGameEntry;
use crate::catalog_config::default_sqlite_path;
use crate::catalog_scan;
use crate::game_discovery::variant_score_from_haystack;
use crate::library_db::{self, normalize_id, BenchConfig};
use std::collections::HashMap;
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
    println!("library-scan-bench label={label}");
    println!("library-scan-bench roots={}", cfg.roots.join("|"));
    println!(
        "library-scan-bench sqlite_path={}",
        cfg.sqlite_path.display()
    );
    for iteration in 1..=iterations {
        match std::fs::remove_file(&cfg.sqlite_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("library-scan-bench remove old sqlite: {e}"),
        }

        if bench_precount {
            let (candidates, dirs, precount_us) =
                catalog_scan::precount_discovery_candidates(&cfg.roots);
            println!(
                "library_scan_bench_tsv\t{label}\t{iteration}\tprecount_discovery\t{precount_us}\tcandidates={candidates}\tdirs={dirs}"
            );
        }

        let build_t = Instant::now();
        let artifact = library_db::scan_library_artifact(&cfg, None);
        let stats = artifact.stats().clone();
        let build_us = build_t.elapsed().as_micros() as u64;

        let import_t = Instant::now();
        let summary = match library_db::save_scan_artifact_to_sqlite(&cfg, artifact, None) {
            Ok(summary) => summary,
            Err(e) => {
                println!(
                    "library_scan_bench_tsv\t{label}\t{iteration}\timport_error\t{}\t{e}",
                    import_t.elapsed().as_micros()
                );
                continue;
            }
        };
        let import_us = import_t.elapsed().as_micros() as u64;
        let bytes = summary.bytes;

        let load_t = Instant::now();
        let loaded = library_db::load_arcade_catalog_from_sqlite("/media/fat/_Arcade");
        let (load_us, arcade_rows) = match loaded {
            Ok(load) => (load.us, load.rows),
            Err(e) => {
                eprintln!("library-scan-bench arcade load failed: {e}");
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
                eprintln!(
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

        println!(
            "library_scan_bench_tsv\t{label}\t{iteration}\tfresh_build\t{build_us}\tdiscover_us={}\tclassify_us={}\tnormal_files={}\tcontainers={}\tentries={}\tdiscoveries={}",
            stats.discover_us,
            stats.classify_us,
            stats.normal_files,
            stats.containers,
            stats.entries,
            stats.discoveries
        );
        println!(
            "library_scan_bench_tsv\t{label}\t{iteration}\timport\t{import_us}\tbytes={bytes}"
        );
        println!(
            "library_scan_bench_tsv\t{label}\t{iteration}\tcached_arcade_load\t{load_us}\trows={arcade_rows}"
        );
        match stamp_check {
            Ok(check) => println!(
                "library_scan_bench_tsv\t{label}\t{iteration}\troot_stamp_check\t{stamp_us}\tunchanged={} check_us={} compute_us={} open_us={} read_us={} compare_us={} stored={} current={} stored_lines={} current_lines={}",
                check.unchanged,
                check.check_us,
                check.compute_us,
                check.open_us,
                check.read_us,
                check.compare_us,
                check.stored_fingerprint.as_deref().unwrap_or("missing"),
                check.current_fingerprint,
                check.stored_lines,
                check.current_lines
            ),
            Err(e) => println!(
                "library_scan_bench_tsv\t{label}\t{iteration}\troot_stamp_check_error\t{stamp_us}\t{e}"
            ),
        }
        if let Some((force_rebuild_us, force_rebuild_summary)) = force_rebuild {
            match force_rebuild_summary {
                Ok(summary) => println!(
                    "library_scan_bench_tsv\t{label}\t{iteration}\tforce_rebuild\t{force_rebuild_us}\tscan_us={}\tdiscover_us={}\tclassify_us={}\timport_us={}\tskipped={}\tdiscoveries={}",
                    summary.scan_us,
                    summary.discover_us,
                    summary.classify_us,
                    summary.import_us,
                    summary.skipped,
                    summary.discoveries
                ),
                Err(e) => println!(
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
    let trimmed = query.trim_start().to_ascii_lowercase();
    if !trimmed.starts_with("select") && !trimmed.starts_with("with") {
        return Err("library-sql only allows read-only SELECT/WITH queries".into());
    }

    let metadata = std::fs::metadata(&path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    if metadata.len() == 0 {
        return Err(format!("{} is empty", path.display()));
    }

    let conn = library_db::open_sqlite_read_only(&path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| format!("prepare query: {e}"))?;
    let column_count = stmt.column_count();
    let mut out = String::new();
    if column_count > 0 {
        out.push_str(&stmt.column_names().join("\t"));
        out.push('\n');
    }
    let mut rows = stmt.query([]).map_err(|e| format!("run query: {e}"))?;
    while let Some(row) = rows.next().map_err(|e| format!("read row: {e}"))? {
        for col in 0..column_count {
            if col > 0 {
                out.push('\t');
            }
            out.push_str(&sqlite_cell_to_string(row, col)?);
        }
        out.push('\n');
    }
    Ok(out)
}

fn sqlite_cell_to_string(row: &rusqlite::Row<'_>, col: usize) -> Result<String, String> {
    use rusqlite::types::ValueRef;

    match row
        .get_ref(col)
        .map_err(|e| format!("read column {col}: {e}"))?
    {
        ValueRef::Null => Ok(String::new()),
        ValueRef::Integer(value) => Ok(value.to_string()),
        ValueRef::Real(value) => Ok(value.to_string()),
        ValueRef::Text(value) => Ok(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(value) => Ok(format!("<blob:{}>", value.len())),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CatalogRow {
    pub(crate) game: ArcadeGameEntry,
    pub(crate) source_kind: String,
    pub(crate) setname: String,
    pub(crate) parent: String,
    pub(crate) family_key: Option<String>,
}

pub(crate) fn collapse_catalog_variants(rows: Vec<CatalogRow>) -> Vec<ArcadeGameEntry> {
    let mut best_idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<CatalogRow> = Vec::with_capacity(rows.len());

    for row in rows {
        let key = catalog_variant_group_key(&row);
        if let Some(&idx) = best_idx.get(&key) {
            if prefer_catalog_variant(&row, &out[idx]) {
                out[idx] = row;
            }
        } else {
            best_idx.insert(key, out.len());
            out.push(row);
        }
    }

    out.into_iter().map(|row| row.game).collect()
}

fn catalog_variant_group_key(row: &CatalogRow) -> String {
    if let Some(family_key) = row.family_key.as_deref() {
        return format!("family:{}", normalize_id(family_key));
    }
    if row.source_kind == "mra" {
        if !row.setname.trim().is_empty() {
            let parent = row.parent.trim();
            let group = if parent.is_empty() {
                row.setname.as_str()
            } else {
                parent
            };
            return format!("mra:set:{}", normalize_id(group));
        }
        return format!("mra:title:{}", canonical_variant_title(&row.game.title));
    }
    if row.source_kind == "catalog-entry" {
        return format!(
            "catalog-entry:{}:{}",
            row.game.mra_path,
            normalize_id(&row.game.title)
        );
    }
    format!("{}:{}", row.source_kind, row.game.mra_path)
}

fn prefer_catalog_variant(a: &CatalogRow, b: &CatalogRow) -> bool {
    let a_score = catalog_variant_score(a);
    let b_score = catalog_variant_score(b);
    if a_score != b_score {
        return a_score > b_score;
    }
    if a.game.has_preview != b.game.has_preview {
        return a.game.has_preview;
    }
    a.game.mra_path < b.game.mra_path
}

fn catalog_variant_score(row: &CatalogRow) -> i32 {
    let haystack = format!(
        "{} {} {} {}",
        row.game.title, row.game.mra_path, row.setname, row.parent
    )
    .to_ascii_lowercase();

    let mut score = variant_score_from_haystack(&haystack);
    if row.source_kind == "mra" && !row.setname.trim().is_empty() && row.parent.trim().is_empty() {
        score += 1000;
    }
    score
}

pub(crate) fn canonical_variant_title(title: &str) -> String {
    let mut out = String::new();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    for ch in title.chars() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ if paren_depth == 0 && bracket_depth == 0 => out.push(ch),
            _ => {}
        }
    }
    normalize_id(out.trim_matches(|ch: char| ch.is_whitespace() || ch == '-' || ch == ','))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use rusqlite::Connection;

    #[test]
    fn catalog_variants_group_by_parent_and_prefer_us_release() {
        let rows = vec![
            catalog_row(
                "Moon Patrol (Japan)",
                "/media/fat/_Arcade/Moon Patrol (Japan).mra",
                "mpatrolj",
                "mpatrol",
            ),
            catalog_row(
                "Moon Patrol (prototype)",
                "/media/fat/_Arcade/Moon Patrol (prototype).mra",
                "mpatrolp",
                "mpatrol",
            ),
            catalog_row(
                "Moon Patrol (US)",
                "/media/fat/_Arcade/Moon Patrol (US).mra",
                "mpatrol",
                "",
            ),
        ];

        let games = collapse_catalog_variants(rows);

        assert_eq!(games.len(), 1);
        assert_eq!(games[0].title.as_ref(), "Moon Patrol (US)");
    }

    #[test]
    fn catalog_variants_keep_non_mra_launchers_separate() {
        let rows = vec![
            catalog_launcher_row("Amiga", "/media/fat/_Computer/Amiga.mgl"),
            catalog_launcher_row("Amiga 500", "/media/fat/_Computer/Amiga 500.mgl"),
        ];

        let games = collapse_catalog_variants(rows);

        assert_eq!(games.len(), 2);
    }

    #[test]
    fn catalog_entries_with_shared_collection_launch_ref_stay_separate() {
        let rows = vec![
            catalog_entry_row("Agony", "/media/fat/games/Amiga/AmigaVision-MiSTer.7z"),
            catalog_entry_row(
                "Alien Breed",
                "/media/fat/games/Amiga/AmigaVision-MiSTer.7z",
            ),
        ];

        let games = collapse_catalog_variants(rows);

        assert_eq!(games.len(), 2);
        assert!(games.iter().any(|game| game.title.as_ref() == "Agony"));
        assert!(games
            .iter()
            .any(|game| game.title.as_ref() == "Alien Breed"));
    }

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

        assert_eq!(
            out,
            "int_value\treal_value\ttext_value\tblob_value\tnull_value\n42\t1.5\thello\t<blob:3>\t\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
