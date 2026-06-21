//! SQLite catalog storage and publish support.
//!
//! Catalog v2 builds SQLite off SD/exFAT and publishes only the completed file.

use crate::catalog_stamp::CatalogStamp;
use rusqlite::{params, Connection};

pub fn create_catalog_stamp_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE catalog_stamp (
            ordinal INTEGER PRIMARY KEY,
            line TEXT NOT NULL
        );
        "#,
    )
    .map_err(|e| format!("create catalog stamp schema: {e}"))
}

pub fn write_catalog_stamp(conn: &Connection, stamp: &CatalogStamp) -> Result<(), String> {
    conn.execute("DELETE FROM catalog_stamp", [])
        .map_err(|e| format!("clear catalog stamp: {e}"))?;
    let mut stmt = conn
        .prepare("INSERT INTO catalog_stamp(ordinal,line) VALUES (?1,?2)")
        .map_err(|e| format!("prepare catalog stamp insert: {e}"))?;
    for (idx, line) in stamp.lines().iter().enumerate() {
        stmt.execute(params![idx as i64, line.as_str()])
            .map_err(|e| format!("insert catalog stamp: {e}"))?;
    }
    Ok(())
}

pub fn read_catalog_stamp(conn: &Connection) -> Result<Option<CatalogStamp>, String> {
    if !sqlite_table_exists(conn, "catalog_stamp")? {
        return Ok(None);
    }
    let mut stmt = conn
        .prepare("SELECT line FROM catalog_stamp ORDER BY ordinal")
        .map_err(|e| format!("prepare catalog stamp read: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("query catalog stamp: {e}"))?;
    let mut lines = Vec::new();
    for row in rows {
        lines.push(row.map_err(|e| format!("read catalog stamp row: {e}"))?);
    }
    Ok((!lines.is_empty()).then(|| CatalogStamp::from_lines(lines)))
}

fn sqlite_table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1 LIMIT 1")
        .map_err(|e| format!("prepare sqlite table check: {e}"))?;
    let mut rows = stmt
        .query([table])
        .map_err(|e| format!("query sqlite table check: {e}"))?;
    rows.next()
        .map(|row| row.is_some())
        .map_err(|e| format!("read sqlite table check: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_stamp_round_trips_through_sqlite() {
        let conn = Connection::open_in_memory().expect("open sqlite");
        create_catalog_stamp_schema(&conn).expect("create schema");
        let stamp = CatalogStamp::from_lines(vec![
            "schema\t30".to_string(),
            "root\t0\t/tmp/root\tdir\t1\t2".to_string(),
        ]);

        write_catalog_stamp(&conn, &stamp).expect("write stamp");
        let stored = read_catalog_stamp(&conn)
            .expect("read stamp")
            .expect("stored stamp");

        assert_eq!(stored, stamp);
        assert_eq!(stored.fingerprint_hex(), stamp.fingerprint_hex());
    }

    #[test]
    fn missing_catalog_stamp_table_reads_as_none() {
        let conn = Connection::open_in_memory().expect("open sqlite");

        assert!(read_catalog_stamp(&conn).expect("read missing").is_none());
    }
}
