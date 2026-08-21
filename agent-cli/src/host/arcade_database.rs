// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use quick_xml::escape::unescape;
use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use super::Result;

const SOURCE_REPOSITORY: &str = "MiSTer-devel/ArcadeDatabase_MiSTer";
const SOURCE_PATH: &str = "ArcadeDatabase.csv";
const SOURCE_SCHEMA: i64 = 1;
const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ROWS: usize = 10_000;
const MAX_FIELD_BYTES: usize = 4 * 1024;
const REQUIRED_HEADERS: &[&str] = &[
    "setname",
    "name",
    "region",
    "version",
    "alternative",
    "parent_title",
    "platform",
    "series",
    "homebrew",
    "bootleg",
    "year",
    "manufacturer",
    "category",
    "linebreak1",
    "resolution",
    "rotation",
    "flip",
    "linebreak2",
    "players",
    "move_inputs",
    "special_controls",
    "num_buttons",
];

#[derive(Debug)]
struct Entry {
    values: HashMap<String, String>,
    raw_json: String,
}

pub(crate) fn import(sqlite: &Path, csv: &Path, source_sha: &str) -> Result<Value> {
    require_lower_hex("source SHA", source_sha, 40)?;
    let bytes = fs::read(csv)?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "ArcadeDatabase CSV is {} bytes; limit is {MAX_SOURCE_BYTES}",
            bytes.len()
        )
        .into());
    }
    let csv_sha256 = digest_bytes(&bytes);
    let entries = parse_entries(&bytes)?;
    let categories = entries
        .iter()
        .map(|entry| entry.value("category"))
        .filter(|category| !category.is_empty())
        .collect::<BTreeSet<_>>()
        .len();

    let mut connection = Connection::open(sqlite)?;
    let machine_table: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='mame_machines'",
        [],
        |row| row.get(0),
    )?;
    if machine_table != 1 {
        return Err("target SQLite database has no mame_machines table".into());
    }
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        r#"
        DROP TABLE IF EXISTS mister_arcade_entries;
        DROP TABLE IF EXISTS mister_arcade_source;
        CREATE TABLE mister_arcade_source (
            id INTEGER PRIMARY KEY CHECK(id=1),
            schema_version INTEGER NOT NULL,
            repository TEXT NOT NULL,
            source_path TEXT NOT NULL,
            source_sha TEXT NOT NULL,
            csv_sha256 TEXT NOT NULL,
            row_count INTEGER NOT NULL,
            category_count INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE mister_arcade_entries (
            ordinal INTEGER PRIMARY KEY,
            setname TEXT NOT NULL,
            setname_key TEXT NOT NULL,
            name TEXT NOT NULL,
            mra_name_key TEXT NOT NULL,
            region TEXT NOT NULL,
            version TEXT NOT NULL,
            alternative INTEGER NOT NULL,
            parent_title TEXT NOT NULL,
            platform TEXT NOT NULL,
            series TEXT NOT NULL,
            homebrew INTEGER NOT NULL,
            bootleg INTEGER NOT NULL,
            year INTEGER,
            manufacturer TEXT NOT NULL,
            category TEXT NOT NULL,
            resolution TEXT NOT NULL,
            rotation TEXT NOT NULL,
            flip INTEGER,
            players TEXT NOT NULL,
            move_inputs TEXT NOT NULL,
            special_controls TEXT NOT NULL,
            num_buttons INTEGER,
            raw_json TEXT NOT NULL
        );
        CREATE INDEX mister_arcade_entries_setname_idx
            ON mister_arcade_entries(setname_key);
        CREATE INDEX mister_arcade_entries_mra_name_idx
            ON mister_arcade_entries(mra_name_key);
        "#,
    )?;
    transaction.execute(
        "INSERT INTO mister_arcade_source(
            id,schema_version,repository,source_path,source_sha,csv_sha256,row_count,category_count
         ) VALUES (1,?1,?2,?3,?4,?5,?6,?7)",
        params![
            SOURCE_SCHEMA,
            SOURCE_REPOSITORY,
            SOURCE_PATH,
            source_sha,
            csv_sha256,
            i64::try_from(entries.len())?,
            i64::try_from(categories)?
        ],
    )?;
    {
        let mut statement = transaction.prepare(
            "INSERT INTO mister_arcade_entries(
                ordinal,setname,setname_key,name,mra_name_key,region,version,alternative,
                parent_title,platform,series,homebrew,bootleg,year,manufacturer,category,
                resolution,rotation,flip,players,move_inputs,special_controls,num_buttons,raw_json
             ) VALUES (
                ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,
                ?19,?20,?21,?22,?23,?24
             )",
        )?;
        for (ordinal, entry) in entries.iter().enumerate() {
            let setname = entry.value("setname");
            let name = entry.value("name");
            statement.execute(params![
                i64::try_from(ordinal)?,
                setname,
                normalize_key(setname),
                name,
                format!("{}.mra", name.trim()).to_lowercase(),
                entry.value("region"),
                entry.value("version"),
                yes_no(entry.value("alternative"), "alternative")?,
                entry.value("parent_title"),
                entry.value("platform"),
                entry.value("series"),
                source_flag(entry.value("homebrew"), "homebrew")?,
                source_flag(entry.value("bootleg"), "bootleg")?,
                optional_integer(entry.value("year"), "year")?,
                entry.value("manufacturer"),
                entry.value("category"),
                entry.value("resolution"),
                entry.value("rotation"),
                optional_yes_no(entry.value("flip"), "flip")?,
                entry.value("players"),
                entry.value("move_inputs"),
                entry.value("special_controls"),
                optional_integer(entry.value("num_buttons"), "num_buttons")?,
                entry.raw_json,
            ])?;
        }
    }
    transaction.commit()?;
    Ok(json!({
        "source_sha": source_sha,
        "csv_sha256": csv_sha256,
        "rows": entries.len(),
        "categories": categories,
    }))
}

fn parse_entries(bytes: &[u8]) -> Result<Vec<Entry>> {
    let mut records = parse_csv_records(bytes)?;
    if records.is_empty() {
        return Err("ArcadeDatabase CSV is empty".into());
    }
    let headers = records.remove(0);
    let mut seen = BTreeSet::new();
    for header in &headers {
        if !seen.insert(header.as_str()) {
            return Err(format!("duplicate ArcadeDatabase CSV header {header:?}").into());
        }
    }
    for required in REQUIRED_HEADERS {
        if !seen.contains(required) {
            return Err(format!("ArcadeDatabase CSV is missing header {required:?}").into());
        }
    }

    let mut entries = Vec::new();
    for record in records {
        if entries.len() >= MAX_ROWS {
            return Err(format!("ArcadeDatabase CSV exceeds {MAX_ROWS} rows").into());
        }
        if record.len() != headers.len() {
            return Err(format!(
                "ArcadeDatabase row has {} fields; expected {}",
                record.len(),
                headers.len()
            )
            .into());
        }
        let mut values = HashMap::with_capacity(headers.len());
        let mut raw = Map::new();
        for (header, value) in headers.iter().zip(record.iter()) {
            if value.len() > MAX_FIELD_BYTES {
                return Err(format!(
                    "ArcadeDatabase field {header:?} is {} bytes; limit is {MAX_FIELD_BYTES}",
                    value.len()
                )
                .into());
            }
            raw.insert(header.to_string(), Value::String(value.to_string()));
            values.insert(header.to_string(), decode_text(value)?);
        }
        entries.push(Entry {
            values,
            raw_json: serde_json::to_string(&raw)?,
        });
    }
    Ok(entries)
}

fn parse_csv_records(bytes: &[u8]) -> Result<Vec<Vec<String>>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = Vec::new();
    let mut quoted = false;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b'"' if quoted && bytes.get(index + 1) == Some(&b'"') => {
                field.push(b'"');
                index += 1;
            }
            b'"' if quoted => quoted = false,
            b'"' if field.is_empty() => quoted = true,
            b'"' => return Err("quote inside an unquoted ArcadeDatabase CSV field".into()),
            b',' if !quoted => finish_csv_field(&mut record, &mut field)?,
            b'\n' if !quoted => {
                finish_csv_field(&mut record, &mut field)?;
                records.push(std::mem::take(&mut record));
            }
            b'\r' if !quoted && bytes.get(index + 1) == Some(&b'\n') => {}
            _ => {
                field.push(byte);
                if field.len() > MAX_FIELD_BYTES {
                    return Err(
                        format!("ArcadeDatabase field exceeds {MAX_FIELD_BYTES} bytes").into(),
                    );
                }
            }
        }
        index += 1;
    }
    if quoted {
        return Err("unterminated quoted ArcadeDatabase CSV field".into());
    }
    if !field.is_empty() || !record.is_empty() {
        finish_csv_field(&mut record, &mut field)?;
        records.push(record);
    }
    Ok(records)
}

fn finish_csv_field(record: &mut Vec<String>, field: &mut Vec<u8>) -> Result<()> {
    record.push(String::from_utf8(std::mem::take(field))?);
    Ok(())
}

impl Entry {
    fn value(&self, name: &str) -> &str {
        self.values
            .get(name)
            .map(String::as_str)
            .unwrap_or_default()
    }
}

fn decode_text(value: &str) -> Result<String> {
    if !value.contains('&') {
        return Ok(value.to_string());
    }

    let mut escaped = String::with_capacity(value.len());
    let mut remainder = value;
    while let Some(index) = remainder.find('&') {
        escaped.push_str(&remainder[..index]);
        remainder = &remainder[index + 1..];
        if starts_with_xml_entity(remainder) {
            escaped.push('&');
        } else {
            escaped.push_str("&amp;");
        }
    }
    escaped.push_str(remainder);
    Ok(unescape(&escaped)?.into_owned())
}

fn starts_with_xml_entity(value: &str) -> bool {
    ["amp;", "apos;", "gt;", "lt;", "quot;"]
        .iter()
        .any(|entity| value.starts_with(entity))
        || value
            .strip_prefix("#x")
            .is_some_and(|value| has_numeric_entity(value, 16))
        || value
            .strip_prefix('#')
            .is_some_and(|value| has_numeric_entity(value, 10))
}

fn has_numeric_entity(value: &str, radix: u32) -> bool {
    value.find(';').is_some_and(|end| {
        end > 0
            && value[..end]
                .chars()
                .all(|character| character.is_digit(radix))
    })
}

fn yes_no(value: &str, field: &str) -> Result<i64> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" => Ok(1),
        "no" => Ok(0),
        _ => Err(format!("ArcadeDatabase {field} must be yes or no; got {value:?}").into()),
    }
}

fn source_flag(value: &str, field: &str) -> Result<i64> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "ys" => Ok(1),
        "" | "no" => Ok(0),
        _ => Err(
            format!("ArcadeDatabase {field} must be yes, no, ys, or blank; got {value:?}").into(),
        ),
    }
}

fn optional_yes_no(value: &str, field: &str) -> Result<Option<i64>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "n-a" => Ok(None),
        "yes" => Ok(Some(1)),
        "no" => Ok(Some(0)),
        _ => Err(format!("ArcadeDatabase {field} must be yes, no, n-a, or blank").into()),
    }
}

fn optional_integer(value: &str, field: &str) -> Result<Option<i64>> {
    let value = value.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        value
            .parse()
            .map(Some)
            .map_err(|error| format!("invalid ArcadeDatabase {field} {value:?}: {error}").into())
    }
}

pub(super) fn normalize_key(value: &str) -> String {
    let mut normalized = String::new();
    let mut last_dash = false;
    for character in value.trim().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            normalized.push(character);
            last_dash = false;
        } else if !last_dash && !normalized.is_empty() {
            normalized.push('-');
            last_dash = true;
        }
    }
    while normalized.ends_with('-') {
        normalized.pop();
    }
    normalized
}

fn digest_bytes(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn require_lower_hex(label: &str, value: &str, length: usize) -> Result<()> {
    if value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(format!("{label} must be {length} lowercase hexadecimal characters").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    const HEADER: &str = "setname,name,region,version,alternative,parent_title,platform,series,homebrew,bootleg,year,manufacturer,category,linebreak1,resolution,rotation,flip,linebreak2,players,move_inputs,special_controls,num_buttons\n";
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "mister-arcade-database-{}-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ))
    }

    fn database() -> std::path::PathBuf {
        let path = temp_path("mame.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("CREATE TABLE mame_machines(setname TEXT PRIMARY KEY)", [])
            .unwrap();
        path
    }

    #[test]
    fn imports_entities_literal_ampersands_and_source_flag_quirks() {
        let path = database();
        let csv_path = temp_path("ArcadeDatabase.csv");
        let csv = format!(
            "{HEADER},\"Snow Bros. - Nick & Tom\",World,,no,,Board,Series,,ys,1985,Maker,\"Platform - Run, Jump &amp; Scrolling\",,15kHz,horizontal,n-a,,2-4 (simultaneous),8-way,,2\n"
        );
        fs::write(&csv_path, csv).unwrap();

        let summary = import(&path, &csv_path, &"a".repeat(40)).unwrap();
        let connection = Connection::open(&path).unwrap();
        let row: (String, String, String, i64, i64, i64) = connection
            .query_row(
                "SELECT setname,name,category,year,homebrew,bootleg FROM mister_arcade_entries",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(summary["rows"], 1);
        assert_eq!(
            row,
            (
                String::new(),
                "Snow Bros. - Nick & Tom".to_string(),
                "Platform - Run, Jump & Scrolling".to_string(),
                1985,
                0,
                1
            )
        );
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(csv_path);
    }

    #[test]
    fn failed_import_preserves_existing_tables() {
        let path = database();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("CREATE TABLE mister_arcade_entries(value TEXT)", [])
            .unwrap();
        connection
            .execute("INSERT INTO mister_arcade_entries VALUES ('kept')", [])
            .unwrap();
        drop(connection);
        let csv_path = temp_path("bad.csv");
        fs::write(&csv_path, "setname,name\n1942,1942\n").unwrap();

        assert!(import(&path, &csv_path, &"b".repeat(40)).is_err());
        let connection = Connection::open(&path).unwrap();
        let value: String = connection
            .query_row("SELECT value FROM mister_arcade_entries", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(value, "kept");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(csv_path);
    }

    #[test]
    fn setname_keys_match_catalog_normalization() {
        assert_eq!(normalize_key(" SF2_CE / Turbo "), "sf2-ce-turbo");
        assert_eq!(normalize_key("1942GXC64"), "1942gxc64");
        assert_eq!(normalize_key(""), "");
    }
}
