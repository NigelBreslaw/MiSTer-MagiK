//! Shared SQLite inspection helpers for read-only catalog query entrypoints.

use std::fmt::Write as _;
use std::path::Path;

use rusqlite::Connection;

pub struct SqliteInspectTiming<'a> {
    pub path: &'a Path,
    pub db_bytes: u64,
    pub query_hash: u64,
    pub open_us: u64,
    pub schema_check_us: Option<u64>,
    pub prepare_us: u64,
    pub first_row_us: u64,
    pub row_read_us: u64,
    pub format_us: u64,
    pub total_us: u64,
    pub rows: usize,
    pub columns: usize,
    pub stdout_bytes: usize,
}

pub fn append_sqlite_timing_row(out: &mut String, timing: SqliteInspectTiming<'_>) {
    let schema_check = timing
        .schema_check_us
        .map(|value| value.to_string())
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "library_sql_timing_tsv\t{}\t{}\t{:016x}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        tsv_field(&timing.path.display().to_string()),
        timing.db_bytes,
        timing.query_hash,
        timing.open_us,
        schema_check,
        timing.prepare_us,
        timing.first_row_us,
        timing.row_read_us,
        timing.format_us,
        timing.total_us,
        timing.rows,
        timing.columns,
        timing.stdout_bytes
    );
}

pub fn sqlite_query_hash(query: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;
    query.as_bytes().iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

pub fn tsv_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

pub fn sqlite_cell_to_string(row: &rusqlite::Row<'_>, col: usize) -> rusqlite::Result<String> {
    use rusqlite::types::ValueRef;

    match row.get_ref(col)? {
        ValueRef::Null => Ok(String::new()),
        ValueRef::Integer(value) => Ok(value.to_string()),
        ValueRef::Real(value) => Ok(value.to_string()),
        ValueRef::Text(value) => Ok(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(value) => Ok(format!("<blob:{}>", value.len())),
    }
}

pub fn sqlite_query_is_read_only(query: &str) -> bool {
    let tokens = sqlite_query_tokens(query);
    let Some(first) = tokens.first().map(String::as_str) else {
        return false;
    };
    (first == "select" || first == "with") && !sqlite_tokens_contain_write(&tokens)
}

pub fn sqlite_query_to_tsv(conn: &Connection, query: &str) -> rusqlite::Result<String> {
    let mut stmt = conn.prepare(query)?;
    let column_count = stmt.column_count();
    let mut out = String::new();
    if column_count > 0 {
        out.push_str(&stmt.column_names().join("\t"));
        out.push('\n');
    }
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
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

fn sqlite_tokens_contain_write(tokens: &[String]) -> bool {
    tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "insert"
                | "update"
                | "delete"
                | "replace"
                | "create"
                | "drop"
                | "alter"
                | "pragma"
                | "attach"
                | "detach"
                | "vacuum"
                | "reindex"
        )
    })
}

fn sqlite_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut chars = query.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' | '"' => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
                while let Some(quoted) = chars.next() {
                    if quoted == ch {
                        if chars.peek() == Some(&ch) {
                            let _ = chars.next();
                            continue;
                        }
                        break;
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
                let _ = chars.next();
                for comment in chars.by_ref() {
                    if comment == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
                let _ = chars.next();
                let mut prev = '\0';
                for comment in chars.by_ref() {
                    if prev == '*' && comment == '/' {
                        break;
                    }
                    prev = comment;
                }
            }
            ch if ch.is_ascii_alphanumeric() || ch == '_' => token.push(ch.to_ascii_lowercase()),
            _ => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}
