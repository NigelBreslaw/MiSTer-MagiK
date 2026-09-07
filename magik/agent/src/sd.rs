//! Read-only SD browsing; retained bounded MRA and image parsing.
use quick_xml::Reader as XmlReader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

pub const ROOT_PATH: &str = "/";
pub const MRA_PARSE_LIMIT_BYTES: u64 = 512 * 1024;
pub const MRA_RAW_DISPLAY_LIMIT_BYTES: u64 = 256 * 1024;
pub const IMAGE_PREVIEW_LIMIT_BYTES: u64 = 16 * 1024 * 1024;

pub fn list_dir_fast_at_root(
    root: &Path,
    requested_path: &str,
    show_hidden: bool,
) -> Result<Value, String> {
    let start = Instant::now();
    let relative_path = normalize_sd_relative_path(requested_path)?;
    let host_path = checked_sd_host_path(root, &relative_path)
        .map_err(|err| format!("read_dir {relative_path}: {err}"))?;
    let enumerate_start = Instant::now();
    let mut entries = Vec::new();
    for entry in
        fs::read_dir(&host_path).map_err(|err| format!("read_dir {relative_path}: {err}"))?
    {
        let entry = entry.map_err(|err| format!("read_dir {relative_path}: {err}"))?;
        if !show_hidden && is_hidden_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        if entries.len() == 100_000 {
            return Err("directory exceeds 100000 entries".into());
        }
        entries.push(sd_entry_fast_json(&relative_path, entry)?);
    }
    let enumerate_us = enumerate_start.elapsed().as_micros() as u64;
    let sort_start = Instant::now();
    entries.sort_by(sd_entry_value_cmp);
    let sort_us = sort_start.elapsed().as_micros() as u64;
    let serialization_start = Instant::now();
    let serialized_bytes = serde_json::to_vec(&entries)
        .map_err(|err| format!("serialize directory entries: {err}"))?
        .len() as u64;
    let serialization_us = serialization_start.elapsed().as_micros() as u64;
    let entry_count = entries.len() as u64;
    Ok(json!({
        "schema": "mister-magik-sd-list-dir-v2",
        "path": relative_path,
        "show_hidden": show_hidden,
        "entries": entries,
        "elapsed_ms": start.elapsed().as_millis() as u64,
        "io_phases_us": {
            "directory_enumeration": enumerate_us,
            "sort": sort_us,
            "serialization": serialization_us,
        },
        "io_counts": {
            "entries": entry_count,
            "serialized_bytes": serialized_bytes,
        },
    }))
}

pub fn stat_item_at_root(root: &Path, requested_path: &str) -> Result<Value, String> {
    let start = Instant::now();
    let relative_path = normalize_sd_relative_path(requested_path)?;
    let host_path = checked_sd_host_path(root, &relative_path)
        .map_err(|err| format!("stat {relative_path}: {err}"))?;
    let metadata =
        fs::metadata(&host_path).map_err(|err| format!("stat {relative_path}: {err}"))?;
    let name = item_name(&relative_path);
    let extension = file_extension(&name);
    let kind = if metadata.is_dir() {
        "directory"
    } else {
        "file"
    };
    Ok(json!({
        "schema": "mister-magik-sd-stat-item-v1",
        "path": relative_path,
        "name": name,
        "parent_path": parent_sd_path(requested_path),
        "kind": kind,
        "size": if metadata.is_dir() { 0 } else { metadata.len() },
        "modified_unix_ms": modified_unix_ms(&metadata),
        "readonly": metadata.permissions().readonly(),
        "hidden": is_hidden_name(&name),
        "extension": extension,
        "capabilities": item_capabilities(kind, &extension),
        "elapsed_ms": start.elapsed().as_millis() as u64,
    }))
}

pub struct SdPreviewImage {
    pub result: Value,
    pub payload: Vec<u8>,
}

pub fn preview_image_at_root(root: &Path, requested_path: &str) -> Result<SdPreviewImage, String> {
    let start = Instant::now();
    let relative_path = normalize_sd_relative_path(requested_path)?;
    let host_path = checked_sd_host_path(root, &relative_path)
        .map_err(|err| format!("stat {relative_path}: {err}"))?;
    let metadata =
        fs::metadata(&host_path).map_err(|err| format!("stat {relative_path}: {err}"))?;
    if !metadata.is_file() {
        return Err(format!("preview target is not a file: {relative_path}"));
    }
    if metadata.len() > IMAGE_PREVIEW_LIMIT_BYTES {
        return Err(format!(
            "image {} bytes exceeds preview limit {}",
            metadata.len(),
            IMAGE_PREVIEW_LIMIT_BYTES
        ));
    }
    let extension = file_extension(&item_name(&relative_path));
    if !matches!(extension.as_str(), "png" | "jpg" | "jpeg") {
        return Err(format!("unsupported preview extension: {extension}"));
    }
    let payload = bounded_read(&host_path, IMAGE_PREVIEW_LIMIT_BYTES)?;
    let (format, width, height) = image_dimensions(&payload)
        .ok_or_else(|| "could not identify PNG/JPEG dimensions".to_string())?;
    Ok(SdPreviewImage {
        result: json!({
            "schema": "mister-magik-sd-preview-image-v1",
            "path": relative_path,
            "format": format,
            "width": width,
            "height": height,
            "raw_bytes": payload.len() as u64,
            "payload_bytes": payload.len() as u64,
            "encoding": "identity",
            "elapsed_ms": start.elapsed().as_millis() as u64,
        }),
        payload,
    })
}

pub fn parse_mra_at_root(root: &Path, requested_path: &str) -> Result<Value, String> {
    let start = Instant::now();
    let relative_path = normalize_sd_relative_path(requested_path)?;
    let host_path = checked_sd_host_path(root, &relative_path)
        .map_err(|err| format!("stat {relative_path}: {err}"))?;
    let metadata =
        fs::metadata(&host_path).map_err(|err| format!("stat {relative_path}: {err}"))?;
    if !metadata.is_file() {
        return Err(format!("MRA target is not a file: {relative_path}"));
    }
    if file_extension(&item_name(&relative_path)) != "mra" {
        return Err(format!(
            "MRA parser only accepts .mra files: {relative_path}"
        ));
    }
    if metadata.len() > MRA_PARSE_LIMIT_BYTES {
        return Ok(json!({
            "schema": "mister-magik-sd-parse-mra-v1",
            "path": relative_path,
            "size": metadata.len(),
            "parse_limit_bytes": MRA_PARSE_LIMIT_BYTES,
            "raw_display_limit_bytes": MRA_RAW_DISPLAY_LIMIT_BYTES,
            "truncated": true,
            "summary": [],
            "xml_rows": [],
            "path_rows": [],
            "warnings": [format!("MRA is {} bytes; parse limit is {}", metadata.len(), MRA_PARSE_LIMIT_BYTES)],
            "raw_xml": "",
            "raw_xml_truncated": true,
            "elapsed_ms": start.elapsed().as_millis() as u64,
        }));
    }
    let text = String::from_utf8(bounded_read(&host_path, MRA_PARSE_LIMIT_BYTES)?)
        .map_err(|e| e.to_string())?;
    let parsed = parse_mra_text(&text, metadata.len());
    Ok(json!({
        "schema": "mister-magik-sd-parse-mra-v1",
        "path": relative_path,
        "size": metadata.len(),
        "parse_limit_bytes": MRA_PARSE_LIMIT_BYTES,
        "raw_display_limit_bytes": MRA_RAW_DISPLAY_LIMIT_BYTES,
        "truncated": false,
        "summary": parsed.summary,
        "xml_rows": parsed.xml_rows,
        "path_rows": parsed.path_rows,
        "warnings": parsed.warnings,
        "raw_xml": if metadata.len() <= MRA_RAW_DISPLAY_LIMIT_BYTES { text } else { String::new() },
        "raw_xml_truncated": metadata.len() > MRA_RAW_DISPLAY_LIMIT_BYTES,
        "elapsed_ms": start.elapsed().as_millis() as u64,
    }))
}

pub fn normalize_sd_relative_path(path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == ROOT_PATH {
        return Ok(ROOT_PATH.to_string());
    }
    if trimmed.starts_with("/media/fat/") || trimmed == "/media/fat" {
        return Err("sd path must be relative to /media/fat".to_string());
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err("sd path may not contain ..".to_string());
        }
        if part.contains('\0') {
            return Err("sd path may not contain NUL".to_string());
        }
        parts.push(part);
    }
    if parts.is_empty() {
        Ok(ROOT_PATH.to_string())
    } else {
        Ok(format!("/{}", parts.join("/")))
    }
}

pub fn sd_host_path(root: &Path, relative_path: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in relative_path.split('/').filter(|part| !part.is_empty()) {
        path.push(part);
    }
    path
}

fn checked_sd_host_path(root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let canonical_root = fs::canonicalize(root)
        .map_err(|err| format!("resolve SD root {}: {err}", root.display()))?;
    let host_path = sd_host_path(root, relative_path);
    let canonical_path =
        fs::canonicalize(&host_path).map_err(|err| format!("resolve {relative_path}: {err}"))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(format!(
            "resolved path is outside SD root: {}",
            canonical_path.display()
        ));
    }
    Ok(canonical_path)
}

fn sd_entry_fast_json(parent_path: &str, entry: fs::DirEntry) -> Result<Value, String> {
    let name = entry.file_name().to_string_lossy().to_string();
    let entry_path = child_sd_path(parent_path, &name);
    let file_type = entry
        .file_type()
        .map_err(|err| format!("file_type {entry_path}: {err}"))?;
    Ok(json!({
        "name": name,
        "path": entry_path,
        "kind": if file_type.is_dir() { "directory" } else { "file" },
    }))
}

fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}

fn modified_unix_ms(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn item_name(path: &str) -> String {
    if path == ROOT_PATH {
        "SD Card".to_string()
    } else {
        path.rsplit('/').next().unwrap_or(path).to_string()
    }
}

fn parent_sd_path(path: &str) -> String {
    let normalized = normalize_sd_relative_path(path).unwrap_or_else(|_| ROOT_PATH.to_string());
    if normalized == ROOT_PATH {
        return ROOT_PATH.to_string();
    }
    match normalized.rsplit_once('/') {
        Some(("", _)) | None => ROOT_PATH.to_string(),
        Some((parent, _)) => parent.to_string(),
    }
}

fn file_extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default()
}

fn item_capabilities(kind: &str, extension: &str) -> Value {
    json!({
        "stat": true,
        "image_preview": kind == "file" && matches!(extension, "png" | "jpg" | "jpeg"),
        "mra_parse": kind == "file" && extension == "mra",
        "raw_xml": kind == "file" && extension == "mra",
        "folder_analysis": kind == "directory",
        "ini_summary": kind == "file" && extension == "ini",
        "rbf_summary": kind == "file" && extension == "rbf",
        "save_hint": kind == "file" && matches!(extension, "sav" | "srm"),
        "archive_summary": kind == "file" && matches!(extension, "zip" | "7z"),
        "sqlite_summary": kind == "file" && matches!(extension, "sqlite" | "sqlite3" | "db"),
    })
}

pub(super) fn image_dimensions(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    if bytes.len() >= 24
        && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        && bytes[8..12] == 13u32.to_be_bytes()
        && &bytes[12..16] == b"IHDR"
    {
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        if width != 0 && height != 0 {
            return Some(("png", width, height));
        }
        return None;
    }
    if bytes.len() >= 4 && bytes[0..2] == [0xff, 0xd8] {
        let mut i = 2usize;
        while i + 9 < bytes.len() {
            while i < bytes.len() && bytes[i] == 0xff {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }
            let marker = bytes[i];
            i += 1;
            if marker == 0xd9 || marker == 0xda {
                break;
            }
            if i + 2 > bytes.len() {
                break;
            }
            let len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
            if len < 2 || i + len > bytes.len() {
                break;
            }
            if matches!(
                marker,
                0xc0 | 0xc1
                    | 0xc2
                    | 0xc3
                    | 0xc5
                    | 0xc6
                    | 0xc7
                    | 0xc9
                    | 0xca
                    | 0xcb
                    | 0xcd
                    | 0xce
                    | 0xcf
            ) && len >= 8
            {
                let height = u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]) as u32;
                let width = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                if width != 0 && height != 0 {
                    return Some(("jpeg", width, height));
                }
                return None;
            }
            i += len;
        }
    }
    None
}

struct ParsedMra {
    summary: Vec<Value>,
    xml_rows: Vec<Value>,
    path_rows: Vec<Value>,
    warnings: Vec<String>,
}

fn parse_mra_text(text: &str, source_size: u64) -> ParsedMra {
    let mut reader = XmlReader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<String> = Vec::new();
    let mut xml_rows = Vec::new();
    let mut warnings = Vec::new();
    let mut order = 0u64;

    loop {
        if stack.len() > 128 || xml_rows.len() > 20_000 {
            warnings.push("MRA XML display truncated at depth/row limit".into());
            break;
        }
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                push_start_row(&mut xml_rows, &mut stack, &e, &mut order, false);
                stack.push(xml_name(e.name().as_ref()));
            }
            Ok(Event::Empty(e)) => {
                push_start_row(&mut xml_rows, &mut stack, &e, &mut order, true);
            }
            Ok(Event::Text(e)) => {
                let text_value = e.xml10_content().trim().to_string();
                if !text_value.is_empty() {
                    let path = format!("/{}", stack.join("/"));
                    xml_rows.push(json!({
                        "order": order,
                        "depth": stack.len(),
                        "path": path,
                        "kind": "text",
                        "name": "",
                        "value": text_value,
                    }));
                    order += 1;
                }
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Eof) => {
                if !stack.is_empty() {
                    warnings.push("Unexpected end of MRA XML".into());
                }
                break;
            }
            Err(err) => {
                warnings.push(format!("XML parse warning: {err}"));
                break;
            }
            _ => {}
        }
    }

    let summary = mra_summary_rows(&xml_rows, source_size);
    let path_rows = xml_rows
        .iter()
        .filter(|row| {
            let value = row.get("value").and_then(Value::as_str).unwrap_or("");
            looks_path_like(row.get("name").and_then(Value::as_str).unwrap_or(""))
                || looks_path_like(value)
        })
        .cloned()
        .collect::<Vec<_>>();
    ParsedMra {
        summary,
        xml_rows,
        path_rows,
        warnings,
    }
}

fn push_start_row(
    rows: &mut Vec<Value>,
    stack: &mut [String],
    e: &BytesStart<'_>,
    order: &mut u64,
    empty: bool,
) {
    let name = xml_name(e.name().as_ref());
    let path = if stack.is_empty() {
        format!("/{name}")
    } else {
        format!("/{}/{}", stack.join("/"), name)
    };
    rows.push(json!({
        "order": *order,
        "depth": stack.len() + 1,
        "path": path,
        "kind": if empty { "empty-element" } else { "element" },
        "name": name,
        "value": "",
    }));
    *order += 1;
    for attr in e.attributes().flatten() {
        rows.push(json!({
            "order": *order,
            "depth": stack.len() + 1,
            "path": path,
            "kind": "attribute",
            "name": format!("@{}", xml_name(attr.key.as_ref())),
            "value": attr
                .normalized_value(XmlVersion::Implicit1_0)
                .unwrap_or_default()
                .into_owned(),
        }));
        *order += 1;
    }
}

fn xml_name(name: &str) -> String {
    name.to_string()
}

fn mra_summary_rows(rows: &[Value], source_size: u64) -> Vec<Value> {
    let mut out = vec![json!({"label": "MRA size", "value": format!("{source_size} bytes")})];
    for (label, names) in [
        ("Title", &["name", "title"][..]),
        ("Set", &["setname", "set", "rom"][..]),
        ("Year", &["year"][..]),
        ("Manufacturer", &["manufacturer", "maker"][..]),
        ("Core/RBF", &["rbf", "core"][..]),
        ("Rotation", &["rotation", "rotate"][..]),
        ("Buttons", &["buttons"][..]),
    ] {
        if let Some(value) = first_named_value(rows, names) {
            out.push(json!({"label": label, "value": value}));
        }
    }
    out
}

fn first_named_value(rows: &[Value], names: &[&str]) -> Option<String> {
    rows.iter().find_map(|row| {
        let kind = row.get("kind").and_then(Value::as_str).unwrap_or("");
        let name = row.get("name").and_then(Value::as_str).unwrap_or("");
        let path_name = row
            .get("path")
            .and_then(Value::as_str)
            .and_then(|path| path.rsplit('/').next())
            .unwrap_or("");
        let key = name.trim_start_matches('@').to_ascii_lowercase();
        let path_key = path_name.to_ascii_lowercase();
        if (kind == "attribute" && names.contains(&key.as_str()))
            || (kind == "text" && names.contains(&path_key.as_str()))
        {
            let value = row
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        None
    })
}

fn looks_path_like(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    lowered.contains('/')
        || lowered.ends_with(".rbf")
        || lowered.ends_with(".rom")
        || lowered.ends_with(".zip")
        || lowered.ends_with(".bin")
        || lowered.ends_with(".mra")
}

pub fn child_sd_path(parent_path: &str, name: &str) -> String {
    if parent_path == ROOT_PATH {
        format!("/{name}")
    } else {
        format!("{parent_path}/{name}")
    }
}

pub fn sd_entry_value_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    let a_dir = a.get("kind").and_then(Value::as_str) == Some("directory");
    let b_dir = b.get("kind").and_then(Value::as_str) == Some("directory");
    b_dir
        .cmp(&a_dir)
        .then_with(|| natural_name_cmp(entry_name(a), entry_name(b)))
        .then_with(|| entry_name(a).cmp(entry_name(b)))
}

fn entry_name(value: &Value) -> &str {
    value.get("name").and_then(Value::as_str).unwrap_or("")
}

fn natural_name_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a_chars = a.char_indices().peekable();
    let mut b_chars = b.char_indices().peekable();
    loop {
        match (a_chars.peek().copied(), b_chars.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some((_, ac)), Some((_, bc))) if ac.is_ascii_digit() && bc.is_ascii_digit() => {
                let a_digits = take_ascii_digits(a, &mut a_chars);
                let b_digits = take_ascii_digits(b, &mut b_chars);
                let a_trimmed = a_digits.trim_start_matches('0');
                let b_trimmed = b_digits.trim_start_matches('0');
                let a_number = if a_trimmed.is_empty() { "0" } else { a_trimmed };
                let b_number = if b_trimmed.is_empty() { "0" } else { b_trimmed };
                let by_len = a_number.len().cmp(&b_number.len());
                if by_len != std::cmp::Ordering::Equal {
                    return by_len;
                }
                let by_value = a_number.cmp(b_number);
                if by_value != std::cmp::Ordering::Equal {
                    return by_value;
                }
                let by_raw_len = a_digits.len().cmp(&b_digits.len());
                if by_raw_len != std::cmp::Ordering::Equal {
                    return by_raw_len;
                }
            }
            (Some((_, ac)), Some((_, bc))) => {
                a_chars.next();
                b_chars.next();
                let by_char = ac.to_ascii_lowercase().cmp(&bc.to_ascii_lowercase());
                if by_char != std::cmp::Ordering::Equal {
                    return by_char;
                }
            }
        }
    }
}

fn take_ascii_digits<'a>(
    text: &'a str,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'a>>,
) -> &'a str {
    let start = chars.peek().map(|(index, _)| *index).unwrap_or(text.len());
    let mut end = start;
    while let Some((index, ch)) = chars.peek().copied() {
        if !ch.is_ascii_digit() {
            break;
        }
        end = index + ch.len_utf8();
        chars.next();
    }
    &text[start..end]
}
fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("file exceeds read limit".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_browsing_and_bounded_formats() {
        let root = std::env::temp_dir().join(format!("native-sd-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("game10.mra"),b"<misterromdescription><name>Game</name><rom><part name=\"game.bin\"/></rom></misterromdescription>").unwrap();
        fs::write(root.join("game2.mra"), b"<broken>").unwrap();
        fs::write(root.join(".hidden"), b"secret").unwrap();
        let listing = list_dir_fast_at_root(&root, "/", false).unwrap();
        assert_eq!(listing["entries"].as_array().unwrap().len(), 2);
        assert_eq!(listing["entries"][0]["name"], "game2.mra");
        assert!(stat_item_at_root(&root, "/missing").is_err());
        assert!(stat_item_at_root(&root, "/../secret").is_err());
        assert_eq!(
            stat_item_at_root(&root, "/game10.mra").unwrap()["capabilities"]["mra_parse"],
            true
        );
        assert!(
            parse_mra_at_root(&root, "/game10.mra").unwrap()["xml_rows"]
                .as_array()
                .unwrap()
                .len()
                > 1
        );
        assert!(
            !parse_mra_at_root(&root, "/game2.mra").unwrap()["warnings"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        fs::write(root.join("bad.png"), b"not a png").unwrap();
        assert!(preview_image_at_root(&root, "/bad.png").is_err());
        fs::write(
            root.join("large.mra"),
            vec![b'x'; MRA_PARSE_LIMIT_BYTES as usize + 1],
        )
        .unwrap();
        assert_eq!(
            parse_mra_at_root(&root, "/large.mra").unwrap()["raw_xml_truncated"],
            true
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(std::env::temp_dir(), root.join("escape")).unwrap();
            assert!(stat_item_at_root(&root, "/escape").is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
