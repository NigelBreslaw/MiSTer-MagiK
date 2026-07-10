//! SQLite catalog storage and publish support.
//!
//! Catalog v2 builds SQLite off SD/exFAT and publishes only the completed file.

use crate::bounded_lz4;
use crate::catalog_checkpoint::CatalogDiscoveryCheckpoint;
use crate::catalog_stamp::CatalogStamp;
use rusqlite::{params, Connection, OptionalExtension};

const MAX_COMPRESSED_LINE_STORE_BYTES: usize = 64 * 1024 * 1024;
const MAX_COMPRESSED_LINE_STORE_PAYLOAD_BYTES: i64 = 64 * 1024 * 1024;

pub fn create_catalog_stamp_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE catalog_stamp (
            id INTEGER PRIMARY KEY CHECK (id=0),
            bytes BLOB NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE catalog_discovery_checkpoint (
            id INTEGER PRIMARY KEY CHECK (id=0),
            bytes BLOB NOT NULL
        ) WITHOUT ROWID;
        "#,
    )
    .map_err(|e| format!("create catalog stamp schema: {e}"))
}

pub fn write_catalog_stamp(conn: &Connection, stamp: &CatalogStamp) -> Result<(), String> {
    conn.execute("DELETE FROM catalog_stamp", [])
        .map_err(|e| format!("clear catalog stamp: {e}"))?;
    write_compressed_lines(conn, "catalog_stamp", stamp.lines())
        .map_err(|e| format!("insert catalog stamp: {e}"))
}

pub fn write_catalog_discovery_checkpoint(
    conn: &Connection,
    checkpoint: &CatalogDiscoveryCheckpoint,
) -> Result<(), String> {
    conn.execute("DELETE FROM catalog_discovery_checkpoint", [])
        .map_err(|e| format!("clear catalog discovery checkpoint: {e}"))?;
    write_compressed_lines(conn, "catalog_discovery_checkpoint", checkpoint.lines())
        .map_err(|e| format!("insert catalog discovery checkpoint: {e}"))
}

pub fn read_catalog_stamp(conn: &Connection) -> Result<Option<CatalogStamp>, String> {
    if !sqlite_table_exists(conn, "catalog_stamp")? {
        return Ok(None);
    }
    let lines = read_line_store(conn, "catalog_stamp")
        .map_err(|e| format!("read catalog stamp row: {e}"))?;
    Ok((!lines.is_empty()).then(|| CatalogStamp::from_lines(lines)))
}

pub fn read_catalog_discovery_checkpoint(
    conn: &Connection,
) -> Result<Option<CatalogDiscoveryCheckpoint>, String> {
    if !sqlite_table_exists(conn, "catalog_discovery_checkpoint")? {
        return Ok(None);
    }
    let lines = read_line_store(conn, "catalog_discovery_checkpoint")
        .map_err(|e| format!("read catalog discovery checkpoint row: {e}"))?;
    Ok((!lines.is_empty()).then(|| CatalogDiscoveryCheckpoint::from_lines(lines)))
}

fn write_compressed_lines(conn: &Connection, table: &str, lines: &[String]) -> Result<(), String> {
    let encoded = encode_lines(lines)?;
    let compressed = lz4_flex::compress_prepend_size(&encoded);
    let sql = format!("INSERT INTO {table}(id,bytes) VALUES (0,?1)");
    conn.execute(&sql, params![compressed])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn read_line_store(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    if sqlite_table_has_column(conn, table, "bytes")? {
        return read_compressed_lines(conn, table);
    }
    read_legacy_line_rows(conn, table)
}

fn read_compressed_lines(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let length_sql = format!("SELECT length(bytes) FROM {table} WHERE id=0");
    let mut stmt = conn
        .prepare(&length_sql)
        .map_err(|e| format!("prepare compressed line length: {e}"))?;
    let compressed_len = stmt
        .query_row([], |row| row.get::<_, Option<i64>>(0))
        .optional()
        .map_err(|e| format!("query compressed line length: {e}"))?
        .flatten();
    let Some(compressed_len) = compressed_len else {
        return Ok(Vec::new());
    };
    if !(0..=MAX_COMPRESSED_LINE_STORE_PAYLOAD_BYTES).contains(&compressed_len) {
        return Err(format!(
            "compressed line store payload size {compressed_len} exceeds max {MAX_COMPRESSED_LINE_STORE_PAYLOAD_BYTES}"
        ));
    }
    let sql = format!("SELECT bytes FROM {table} WHERE id=0");
    let bytes = conn
        .query_row(&sql, [], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|e| format!("read compressed line payload: {e}"))?;
    let decoded = bounded_lz4::decompress_size_prepended(
        &bytes,
        MAX_COMPRESSED_LINE_STORE_BYTES,
        "compressed line store",
    )?;
    decode_lines(&decoded)
}

fn read_legacy_line_rows(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let sql = format!("SELECT line FROM {table} ORDER BY ordinal");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare legacy line read: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("query legacy line read: {e}"))?;
    let mut lines = Vec::new();
    for row in rows {
        lines.push(row.map_err(|e| format!("read legacy line row: {e}"))?);
    }
    Ok(lines)
}

fn encode_lines(lines: &[String]) -> Result<Vec<u8>, String> {
    let count = u32::try_from(lines.len()).map_err(|_| "too many catalog lines".to_string())?;
    let mut out = Vec::new();
    out.extend_from_slice(&count.to_le_bytes());
    for line in lines {
        let bytes = line.as_bytes();
        let len = u32::try_from(bytes.len()).map_err(|_| "catalog line too long".to_string())?;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(bytes);
    }
    Ok(out)
}

fn decode_lines(bytes: &[u8]) -> Result<Vec<String>, String> {
    let mut pos = 0usize;
    let count = read_u32(bytes, &mut pos)? as usize;
    let max_count = (bytes.len() - pos) / 4;
    if count > max_count {
        return Err(format!(
            "catalog line count {count} exceeds remaining encoded bytes"
        ));
    }
    let mut lines = Vec::new();
    lines
        .try_reserve_exact(count)
        .map_err(|err| format!("allocate catalog lines ({count}): {err}"))?;
    for _ in 0..count {
        let len = read_u32(bytes, &mut pos)? as usize;
        let end = pos
            .checked_add(len)
            .ok_or_else(|| "catalog line length overflow".to_string())?;
        if end > bytes.len() {
            return Err("truncated catalog line store".to_string());
        }
        let line = std::str::from_utf8(&bytes[pos..end])
            .map_err(|e| format!("catalog line store utf8: {e}"))?
            .to_string();
        lines.push(line);
        pos = end;
    }
    if pos != bytes.len() {
        return Err("trailing bytes in catalog line store".to_string());
    }
    Ok(lines)
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32, String> {
    let end = pos
        .checked_add(4)
        .ok_or_else(|| "catalog line offset overflow".to_string())?;
    if end > bytes.len() {
        return Err("truncated catalog line header".to_string());
    }
    let value = u32::from_le_bytes(bytes[*pos..end].try_into().unwrap());
    *pos = end;
    Ok(value)
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

fn sqlite_table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let sql = format!("PRAGMA table_info({table})");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare sqlite column check: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("query sqlite column check: {e}"))?;
    for row in rows {
        if row.map_err(|e| format!("read sqlite column check: {e}"))? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_line_decoder_rejects_count_without_encoded_headers() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());

        assert!(decode_lines(&bytes).is_err());
    }

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
    fn catalog_discovery_checkpoint_round_trips_through_sqlite() {
        let conn = Connection::open_in_memory().expect("open sqlite");
        create_catalog_stamp_schema(&conn).expect("create schema");
        let checkpoint = CatalogDiscoveryCheckpoint::from_lines(vec![
            "schema\t45".to_string(),
            "game-dir\t0\t/tmp/root/games/NES\tNES\tknown\tpayloadish\t0\t1".to_string(),
        ]);

        write_catalog_discovery_checkpoint(&conn, &checkpoint).expect("write checkpoint");
        let stored = read_catalog_discovery_checkpoint(&conn)
            .expect("read checkpoint")
            .expect("stored checkpoint");

        assert_eq!(stored, checkpoint);
        assert_eq!(stored.fingerprint_hex(), checkpoint.fingerprint_hex());
    }

    #[test]
    fn missing_catalog_stamp_table_reads_as_none() {
        let conn = Connection::open_in_memory().expect("open sqlite");

        assert!(read_catalog_stamp(&conn).expect("read missing").is_none());
        assert!(read_catalog_discovery_checkpoint(&conn)
            .expect("read missing checkpoint")
            .is_none());
    }
}
