// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Small SQLite/source-file helpers shared by metadata consumers.
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub(crate) fn sqlite_table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type IN ('table','view') AND name=?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists != 0)
    .map_err(|e| format!("check sqlite table {table}: {e}"))
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

pub(crate) fn open_sqlite_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let uri = format!("file:{}?mode=ro&immutable=1", sqlite_uri_path(path));
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    Ok(conn)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_reader_escapes_uri_metacharacters_and_cannot_write() {
        let root = crate::test_support::unique_temp_dir("sqlite-support");
        let path = root.join("metadata %?#.sqlite3");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("CREATE TABLE example (value TEXT); INSERT INTO example VALUES ('kept'); CREATE VIEW example_view AS SELECT value FROM example;").unwrap();
        drop(writer);
        let reader = open_sqlite_read_only(&path).unwrap();
        assert!(sqlite_table_exists(&reader, "example").unwrap());
        assert!(sqlite_table_exists(&reader, "example_view").unwrap());
        assert!(!sqlite_table_exists(&reader, "missing").unwrap());
        assert!(sqlite_column_exists(&reader, "example", "value").unwrap());
        assert!(!sqlite_column_exists(&reader, "example", "missing").unwrap());
        assert!(reader.execute("DELETE FROM example", []).is_err());
        assert_eq!(
            reader
                .query_row("SELECT value FROM example", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "kept"
        );
        drop(reader);
        assert!(open_sqlite_read_only(&root.join("missing.sqlite3")).is_err());
        assert!(!root.join("missing.sqlite3").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
