// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::archive::{read_zip, MemberLayout};
use crate::error::{AgentError, AgentResult};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub const FORMAT: &str = "mister-magik-game-databases-manifest-v1";
pub const MANIFEST: &str = "game-databases-manifest.json";
pub const CHECKSUMS: &str = "SHA256SUMS";
const DATABASES: [&str; 2] = ["mame.sqlite3", "hbmame.sqlite3"];

pub struct Create<'a> {
    pub mame: &'a Path,
    pub hbmame: &'a Path,
    pub release_version: u64,
    pub mame_tag: &'a str,
    pub mame_sha: &'a str,
    pub listxml_asset: &'a str,
    pub listxml_sha256: &'a str,
    pub hbmame_tag: &'a str,
    pub hbmame_sha: &'a str,
    pub mame_builder_sha: &'a str,
    pub hbmame_builder_sha: &'a str,
    pub output: &'a Path,
}

pub fn create(request: &Create<'_>) -> AgentResult<PathBuf> {
    if request.release_version == 0 {
        return classified(
            "invalid_database_release",
            "release version must be positive",
        );
    }
    validate_tags(request.mame_tag, request.hbmame_tag)?;
    for (name, value, length) in [
        ("mame_sha", request.mame_sha, 40),
        ("hbmame_sha", request.hbmame_sha, 40),
        ("mame_builder_sha", request.mame_builder_sha, 40),
        ("hbmame_builder_sha", request.hbmame_builder_sha, 40),
        ("listxml_sha256", request.listxml_sha256, 64),
    ] {
        require_hex(name, value, length)?;
    }
    if request.listxml_asset.is_empty() || request.listxml_asset.contains('/') {
        return classified("invalid_listxml_asset", request.listxml_asset);
    }
    validate_database(request.mame, DatabaseKind::Mame, Some(request.mame_tag))?;
    validate_database(request.hbmame, DatabaseKind::Hbmame, None)?;
    fs::create_dir_all(request.output).map_err(|error| error.to_string())?;
    let entries = [
        file_entry(DATABASES[0], request.mame)?,
        file_entry(DATABASES[1], request.hbmame)?,
    ];
    let payload = json!({
        "format": FORMAT,
        "release_version": request.release_version,
        "sources": {
            "mame": {"tag": request.mame_tag, "sha": request.mame_sha, "listxml_asset": request.listxml_asset, "listxml_sha256": request.listxml_sha256, "builder_sha": request.mame_builder_sha},
            "hbmame": {"tag": request.hbmame_tag, "sha": request.hbmame_sha, "builder_sha": request.hbmame_builder_sha}
        },
        "files": entries,
    });
    let manifest =
        serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())? + "\n";
    let mut checksums = String::new();
    for (name, path) in [(DATABASES[0], request.mame), (DATABASES[1], request.hbmame)] {
        checksums.push_str(&format!("{}  {name}\n", digest(path)?));
    }
    checksums.push_str(&format!(
        "{}  {MANIFEST}\n",
        digest_bytes(manifest.as_bytes())
    ));
    let archive = request.output.join(format!(
        "mister-magik-game-databases-v{}.zip",
        request.release_version
    ));
    let mut writer = ZipWriter::new(File::create(&archive).map_err(|error| error.to_string())?);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, path) in [(DATABASES[0], request.mame), (DATABASES[1], request.hbmame)] {
        writer
            .start_file(name, options)
            .map_err(|error| error.to_string())?;
        std::io::copy(
            &mut File::open(path).map_err(|error| error.to_string())?,
            &mut writer,
        )
        .map_err(|error| error.to_string())?;
    }
    for (name, bytes) in [
        (MANIFEST, manifest.as_bytes()),
        (CHECKSUMS, checksums.as_bytes()),
    ] {
        writer
            .start_file(name, options)
            .map_err(|error| error.to_string())?;
        writer.write_all(bytes).map_err(|error| error.to_string())?;
    }
    writer.finish().map_err(|error| error.to_string())?;
    fs::write(request.output.join(MANIFEST), manifest).map_err(|error| error.to_string())?;
    fs::write(request.output.join(CHECKSUMS), checksums).map_err(|error| error.to_string())?;
    verify(
        &archive,
        Some(&request.output.join(MANIFEST)),
        Some(&request.output.join(CHECKSUMS)),
    )?;
    Ok(archive)
}

pub fn verify(
    archive: &Path,
    manifest: Option<&Path>,
    checksums: Option<&Path>,
) -> AgentResult<Value> {
    let files = read_zip(archive, MemberLayout::Flat)?;
    let names: BTreeSet<_> = files.keys().map(String::as_str).collect();
    let expected: BTreeSet<_> = [DATABASES[0], DATABASES[1], MANIFEST, CHECKSUMS]
        .into_iter()
        .collect();
    if names != expected {
        return classified(
            "database_archive_shape",
            "archive has unexpected or missing files",
        );
    }
    let payload: Value =
        serde_json::from_slice(&files[MANIFEST]).map_err(|error| AgentError::Classified {
            code: "invalid_database_manifest",
            detail: error.to_string(),
        })?;
    validate_manifest(&payload)?;
    if manifest.is_some_and(|path| fs::read(path).ok().as_deref() != Some(&files[MANIFEST])) {
        return classified(
            "database_manifest_mismatch",
            "release manifest differs from archive",
        );
    }
    if checksums.is_some_and(|path| fs::read(path).ok().as_deref() != Some(&files[CHECKSUMS])) {
        return classified(
            "database_checksums_mismatch",
            "release checksums differ from archive",
        );
    }
    let version = payload["release_version"].as_u64().unwrap();
    if archive.file_name().and_then(|name| name.to_str())
        != Some(&format!("mister-magik-game-databases-v{version}.zip"))
    {
        return classified(
            "database_archive_name",
            "archive name does not match release version",
        );
    }
    verify_file_entries(&payload, &files)?;
    verify_checksums(&files)?;
    with_extracted_databases(&files, |mame, hbmame| {
        validate_database(
            mame,
            DatabaseKind::Mame,
            payload.pointer("/sources/mame/tag").and_then(Value::as_str),
        )?;
        validate_database(hbmame, DatabaseKind::Hbmame, None)
    })?;
    Ok(payload)
}

pub fn extract_release(release: &Path, output: &Path) -> AgentResult<Value> {
    let manifest = release.join(MANIFEST);
    let checksums = release.join(CHECKSUMS);
    let payload: Value =
        serde_json::from_slice(&fs::read(&manifest).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    validate_manifest(&payload)?;
    let archive = release.join(format!(
        "mister-magik-game-databases-v{}.zip",
        payload["release_version"]
    ));
    let candidates: Vec<_> = fs::read_dir(release)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("mister-magik-game-databases-v") && name.ends_with(".zip")
                })
        })
        .collect();
    if candidates != [archive.clone()] {
        return classified(
            "database_release_shape",
            "release must contain exactly its numbered archive",
        );
    }
    let verified = verify(&archive, Some(&manifest), Some(&checksums))?;
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    if fs::read_dir(output)
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        return classified("database_extract_not_empty", output.display().to_string());
    }
    for (name, bytes) in read_zip(&archive, MemberLayout::Flat)? {
        fs::write(output.join(name), bytes).map_err(|error| error.to_string())?;
    }
    Ok(verified)
}

pub fn update_plan(
    current: Option<&Value>,
    mame_tag: &str,
    mame_sha: &str,
    hbmame_tag: &str,
    hbmame_sha: &str,
) -> AgentResult<Value> {
    validate_tags(mame_tag, hbmame_tag)?;
    require_hex("mame_sha", mame_sha, 40)?;
    require_hex("hbmame_sha", hbmame_sha, 40)?;
    let Some(current) = current else {
        return Ok(
            json!({"current_version":0,"next_version":1,"mame_changed":true,"hbmame_changed":true,"update_needed":true}),
        );
    };
    validate_manifest(current)?;
    let mame_changed = current.pointer("/sources/mame/tag")
        != Some(&Value::String(mame_tag.into()))
        || current.pointer("/sources/mame/sha") != Some(&Value::String(mame_sha.into()));
    let hbmame_changed = current.pointer("/sources/hbmame/tag")
        != Some(&Value::String(hbmame_tag.into()))
        || current.pointer("/sources/hbmame/sha") != Some(&Value::String(hbmame_sha.into()));
    let version = current["release_version"].as_u64().unwrap();
    Ok(
        json!({"current_version":version,"next_version":version+1,"mame_changed":mame_changed,"hbmame_changed":hbmame_changed,"update_needed":mame_changed||hbmame_changed}),
    )
}

fn validate_manifest(payload: &Value) -> AgentResult<()> {
    if payload["format"] != FORMAT
        || payload["release_version"]
            .as_u64()
            .is_none_or(|value| value == 0)
    {
        return classified("invalid_database_manifest", "format or release version");
    }
    let mame_tag = payload
        .pointer("/sources/mame/tag")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let hbmame_tag = payload
        .pointer("/sources/hbmame/tag")
        .and_then(Value::as_str)
        .unwrap_or_default();
    validate_tags(mame_tag, hbmame_tag)?;
    for pointer in [
        "/sources/mame/sha",
        "/sources/mame/builder_sha",
        "/sources/hbmame/sha",
        "/sources/hbmame/builder_sha",
    ] {
        require_hex(
            pointer,
            payload
                .pointer(pointer)
                .and_then(Value::as_str)
                .unwrap_or_default(),
            40,
        )?;
    }
    require_hex(
        "listxml_sha256",
        payload
            .pointer("/sources/mame/listxml_sha256")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        64,
    )
}

#[derive(Clone, Copy)]
enum DatabaseKind {
    Mame,
    Hbmame,
}
fn validate_database(path: &Path, kind: DatabaseKind, mame_tag: Option<&str>) -> AgentResult<()> {
    let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("invalid SQLite database {}: {error}", path.display()))?;
    let integrity: String = database
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if integrity != "ok" {
        return classified("database_integrity", path.display().to_string());
    }
    let rows: i64 = database
        .query_row("SELECT count(*) FROM mame_machines", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    let minimum = if matches!(kind, DatabaseKind::Mame) {
        50_000
    } else {
        5_000
    };
    if rows < minimum {
        return classified(
            "database_row_count",
            format!("{} has {rows}", path.display()),
        );
    }
    let columns: BTreeSet<String> = {
        let mut statement = database
            .prepare("PRAGMA table_info(mame_machines)")
            .map_err(|error| error.to_string())?;
        let columns = statement
            .query_map([], |row| row.get(1))
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .collect();
        columns
    };
    if !columns.contains("players") || !columns.contains("control_type") {
        return classified("database_schema", "missing player/control metadata");
    }
    if matches!(kind, DatabaseKind::Mame) {
        let expected = format!(
            "0.{} ({})",
            mame_tag
                .unwrap_or_default()
                .trim_start_matches("mame")
                .parse::<u64>()
                .map_err(|_| "invalid MAME tag")?,
            mame_tag.unwrap_or_default()
        );
        let distinct: i64 = database
            .query_row(
                "SELECT count(DISTINCT source_version) FROM mame_machines WHERE source_version=?1",
                [expected],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if distinct != 1 {
            return classified("database_source_version", "MAME source tag mismatch");
        }
    } else {
        let parent: String = database
            .query_row(
                "SELECT COALESCE(parent_setname,'') FROM mame_machines WHERE setname='marpy'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if parent != "mappy" {
            return classified("database_sentinel", "HBMAME marpy parent mismatch");
        }
    }
    Ok(())
}

fn verify_file_entries(payload: &Value, files: &BTreeMap<String, Vec<u8>>) -> AgentResult<()> {
    let entries = payload["files"]
        .as_array()
        .ok_or("manifest files are missing")?;
    if entries.len() != 2 {
        return classified("database_file_manifest", "expected two databases");
    }
    for entry in entries {
        let name = entry["path"].as_str().unwrap_or_default();
        let bytes = files.get(name).ok_or("manifest database is missing")?;
        if !DATABASES.contains(&name)
            || entry["size"].as_u64() != Some(bytes.len() as u64)
            || entry["sha256"] != digest_bytes(bytes)
        {
            return classified("database_file_manifest", name);
        }
    }
    Ok(())
}
fn verify_checksums(files: &BTreeMap<String, Vec<u8>>) -> AgentResult<()> {
    let text = std::str::from_utf8(&files[CHECKSUMS]).map_err(|error| error.to_string())?;
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let (hash, name) = line.split_once("  ").ok_or("malformed checksums")?;
        if files
            .get(name)
            .is_none_or(|bytes| digest_bytes(bytes) != hash)
            || !seen.insert(name)
        {
            return classified("database_checksum", name);
        }
    }
    if seen != [DATABASES[0], DATABASES[1], MANIFEST].into_iter().collect() {
        return classified("database_checksum_shape", "unexpected checksum set");
    }
    Ok(())
}
fn with_extracted_databases<T>(
    files: &BTreeMap<String, Vec<u8>>,
    action: impl FnOnce(&Path, &Path) -> AgentResult<T>,
) -> AgentResult<T> {
    let root = std::env::temp_dir().join(format!("agent-cli-databases-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
    }
    fs::create_dir(&root).map_err(|error| error.to_string())?;
    let mame = root.join(DATABASES[0]);
    let hbmame = root.join(DATABASES[1]);
    fs::write(&mame, &files[DATABASES[0]]).map_err(|e| e.to_string())?;
    fs::write(&hbmame, &files[DATABASES[1]]).map_err(|e| e.to_string())?;
    let result = action(&mame, &hbmame);
    let _ = fs::remove_dir_all(root);
    result
}
fn file_entry(name: &str, path: &Path) -> AgentResult<Value> {
    Ok(
        json!({"path":name,"size":fs::metadata(path).map_err(|e|e.to_string())?.len(),"sha256":digest(path)?}),
    )
}
fn digest(path: &Path) -> AgentResult<String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut HashWriter(&mut hash)).map_err(|e| e.to_string())?;
    Ok(hex(&hash.finalize()))
}
fn digest_bytes(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    hex(&hash.finalize())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
struct HashWriter<'a>(&'a mut Sha256);
impl Write for HashWriter<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.update(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn validate_tags(mame: &str, hbmame: &str) -> AgentResult<()> {
    if mame
        .strip_prefix("mame")
        .is_none_or(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()))
        || hbmame
            .strip_prefix("tag")
            .is_none_or(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()))
    {
        classified("invalid_database_tag", format!("{mame}/{hbmame}"))
    } else {
        Ok(())
    }
}
fn require_hex(name: &str, value: &str, length: usize) -> AgentResult<()> {
    if value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        Ok(())
    } else {
        classified("invalid_database_identity", format!("{name}: {value}"))
    }
}
fn classified<T>(code: &'static str, detail: impl Into<String>) -> AgentResult<T> {
    Err(AgentError::Classified {
        code,
        detail: detail.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_plan_starts_at_version_one() {
        let result = update_plan(
            None,
            "mame0288",
            &"a".repeat(40),
            "tag24532",
            &"b".repeat(40),
        )
        .unwrap();
        assert_eq!(result["next_version"], 1);
        assert_eq!(result["update_needed"], true);
    }

    #[test]
    fn source_tags_are_closed() {
        assert!(validate_tags("mame0288", "tag24532").is_ok());
        assert!(validate_tags("0288", "tag24532").is_err());
        assert!(validate_tags("mame0288", "branch").is_err());
    }
}
