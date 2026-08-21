// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::archive::{MemberLayout, read_zip};
use crate::error::{AgentError, AgentResult};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub const FORMAT: &str = "mister-magik-game-databases-manifest-v3";
pub const PREVIOUS_FORMAT: &str = "mister-magik-game-databases-manifest-v2";
pub const LEGACY_FORMAT: &str = "mister-magik-game-databases-manifest-v1";
pub const MANIFEST: &str = "game-databases-manifest.json";
pub const CHECKSUMS: &str = "SHA256SUMS";
const DATABASES: [&str; 2] = ["mame.sqlite3", "hbmame.sqlite3"];
const ARCADE_DATABASE_CSV: &str = "ArcadeDatabase.csv";
const ARCADE_DATABASE_LICENSE: &str = "ArcadeDatabase-LICENSE.txt";
const ARCADE_UPDATER_INDEX: &str = mister_magik_catalog::arcade_updater_index::FILE_NAME;

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
    pub arcade_database_csv: &'a Path,
    pub arcade_database_license: &'a Path,
    pub arcade_database_sha: &'a str,
    pub arcade_database_builder_sha: &'a str,
    pub arcade_updater_index: &'a Path,
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
        ("arcade_database_sha", request.arcade_database_sha, 40),
        (
            "arcade_database_builder_sha",
            request.arcade_database_builder_sha,
            40,
        ),
        ("listxml_sha256", request.listxml_sha256, 64),
    ] {
        require_hex(name, value, length)?;
    }
    if request.listxml_asset.is_empty() || request.listxml_asset.contains('/') {
        return classified("invalid_listxml_asset", request.listxml_asset);
    }
    validate_database(request.mame, DatabaseKind::Mame, Some(request.mame_tag))?;
    validate_database(request.hbmame, DatabaseKind::Hbmame, None)?;
    let arcade_csv_sha256 = digest(request.arcade_database_csv)?;
    let arcade_license_sha256 = digest(request.arcade_database_license)?;
    validate_arcade_database(
        request.mame,
        request.arcade_database_sha,
        &arcade_csv_sha256,
    )?;
    let updater_index = mister_magik_catalog::arcade_updater_index::ArcadeUpdaterIndex::read(
        request.arcade_updater_index,
    )?;
    let updater_index_sha256 = digest(request.arcade_updater_index)?;
    fs::create_dir_all(request.output).map_err(|error| error.to_string())?;
    let entries = vec![
        file_entry(DATABASES[0], request.mame)?,
        file_entry(DATABASES[1], request.hbmame)?,
        file_entry(ARCADE_DATABASE_CSV, request.arcade_database_csv)?,
        file_entry(ARCADE_DATABASE_LICENSE, request.arcade_database_license)?,
        file_entry(ARCADE_UPDATER_INDEX, request.arcade_updater_index)?,
    ];
    let payload = json!({
        "format": FORMAT,
        "release_version": request.release_version,
        "sources": {
            "mame": {"tag": request.mame_tag, "sha": request.mame_sha, "listxml_asset": request.listxml_asset, "listxml_sha256": request.listxml_sha256, "builder_sha": request.mame_builder_sha},
            "hbmame": {"tag": request.hbmame_tag, "sha": request.hbmame_sha, "builder_sha": request.hbmame_builder_sha},
            "arcade_database": {
                "repository": "MiSTer-devel/ArcadeDatabase_MiSTer",
                "path": ARCADE_DATABASE_CSV,
                "sha": request.arcade_database_sha,
                "csv_sha256": arcade_csv_sha256,
                "license_sha256": arcade_license_sha256,
                "builder_sha": request.arcade_database_builder_sha
            },
            "arcade_updater": {
                "format": mister_magik_catalog::arcade_updater_index::FORMAT,
                "sha256": updater_index_sha256,
                "sources": updater_index.sources,
                "builder_sha": request.arcade_database_builder_sha
            }
        },
        "files": entries,
    });
    let manifest =
        serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())? + "\n";
    let mut checksums = String::new();
    for (name, path) in [
        (DATABASES[0], request.mame),
        (DATABASES[1], request.hbmame),
        (ARCADE_DATABASE_CSV, request.arcade_database_csv),
        (ARCADE_DATABASE_LICENSE, request.arcade_database_license),
        (ARCADE_UPDATER_INDEX, request.arcade_updater_index),
    ] {
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
    for (name, path) in [
        (DATABASES[0], request.mame),
        (DATABASES[1], request.hbmame),
        (ARCADE_DATABASE_CSV, request.arcade_database_csv),
        (ARCADE_DATABASE_LICENSE, request.arcade_database_license),
        (ARCADE_UPDATER_INDEX, request.arcade_updater_index),
    ] {
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
    let manifest_bytes = files.get(MANIFEST).ok_or_else(|| AgentError::Classified {
        code: "database_archive_shape",
        detail: "archive is missing its manifest".to_owned(),
    })?;
    let payload: Value =
        serde_json::from_slice(manifest_bytes).map_err(|error| AgentError::Classified {
            code: "invalid_database_manifest",
            detail: error.to_string(),
        })?;
    validate_manifest(&payload)?;
    let names: BTreeSet<_> = files.keys().map(String::as_str).collect();
    let expected: BTreeSet<_> = expected_archive_members(&payload).into_iter().collect();
    if names != expected {
        return classified(
            "database_archive_shape",
            "archive has unexpected or missing files",
        );
    }
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
    if has_arcade_database(&payload) {
        verify_arcade_database_source_files(&payload, &files)?;
    }
    if payload["format"] == FORMAT {
        let index = mister_magik_catalog::arcade_updater_index::ArcadeUpdaterIndex::decode(
            &files[ARCADE_UPDATER_INDEX],
        )?;
        let expected_sha256 = payload
            .pointer("/sources/arcade_updater/sha256")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if digest_bytes(&files[ARCADE_UPDATER_INDEX]) != expected_sha256 {
            return classified(
                "database_source_checksum",
                "Arcade updater index does not match its manifest SHA-256",
            );
        }
        if serde_json::to_value(&index.sources).map_err(|error| error.to_string())?
            != payload["sources"]["arcade_updater"]["sources"]
        {
            return classified(
                "invalid_database_manifest",
                "Arcade updater sources differ from the index",
            );
        }
    }
    with_extracted_databases(&files, |mame, hbmame| {
        validate_database(
            mame,
            DatabaseKind::Mame,
            payload.pointer("/sources/mame/tag").and_then(Value::as_str),
        )?;
        validate_database(hbmame, DatabaseKind::Hbmame, None)?;
        if has_arcade_database(&payload) {
            validate_arcade_database(
                mame,
                payload
                    .pointer("/sources/arcade_database/sha")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                payload
                    .pointer("/sources/arcade_database/csv_sha256")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )?;
        }
        Ok(())
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
    arcade_database_sha: &str,
    arcade_updater_revisions: &[String],
) -> AgentResult<Value> {
    validate_tags(mame_tag, hbmame_tag)?;
    require_hex("mame_sha", mame_sha, 40)?;
    require_hex("hbmame_sha", hbmame_sha, 40)?;
    require_hex("arcade_database_sha", arcade_database_sha, 40)?;
    let arcade_updater_revisions = parse_updater_revisions(arcade_updater_revisions)?;
    let Some(current) = current else {
        return Ok(
            json!({"current_version":0,"next_version":1,"mame_changed":true,"hbmame_changed":true,"arcade_database_changed":true,"arcade_updater_changed":true,"update_needed":true}),
        );
    };
    validate_manifest(current)?;
    let mame_changed = current.pointer("/sources/mame/tag")
        != Some(&Value::String(mame_tag.into()))
        || current.pointer("/sources/mame/sha") != Some(&Value::String(mame_sha.into()));
    let hbmame_changed = current.pointer("/sources/hbmame/tag")
        != Some(&Value::String(hbmame_tag.into()))
        || current.pointer("/sources/hbmame/sha") != Some(&Value::String(hbmame_sha.into()));
    let arcade_database_changed = current.pointer("/sources/arcade_database/sha")
        != Some(&Value::String(arcade_database_sha.into()));
    let current_updater_revisions = current
        .pointer("/sources/arcade_updater/sources")
        .and_then(Value::as_array)
        .map(|sources| {
            sources
                .iter()
                .filter_map(|source| {
                    Some((
                        source.get("id")?.as_str()?.to_owned(),
                        source.get("revision")?.as_str()?.to_owned(),
                    ))
                })
                .collect::<BTreeMap<_, _>>()
        });
    let arcade_updater_changed =
        current_updater_revisions.as_ref() != Some(&arcade_updater_revisions);
    let version = current["release_version"].as_u64().unwrap();
    Ok(
        json!({"current_version":version,"next_version":version+1,"mame_changed":mame_changed,"hbmame_changed":hbmame_changed,"arcade_database_changed":arcade_database_changed,"arcade_updater_changed":arcade_updater_changed,"update_needed":mame_changed||hbmame_changed||arcade_database_changed||arcade_updater_changed}),
    )
}

fn parse_updater_revisions(values: &[String]) -> AgentResult<BTreeMap<String, String>> {
    const IDS: [&str; 5] = [
        "distribution",
        "alternatives",
        "jtcores",
        "coinop",
        "arcade-offset",
    ];
    let mut revisions = BTreeMap::new();
    for value in values {
        let (id, revision) = value
            .split_once('=')
            .ok_or_else(|| AgentError::Classified {
                code: "invalid_database_identity",
                detail: format!("arcade updater revision: {value}"),
            })?;
        if !IDS.contains(&id)
            || revisions
                .insert(id.to_owned(), revision.to_owned())
                .is_some()
        {
            return classified(
                "invalid_database_identity",
                format!("arcade updater source: {id}"),
            );
        }
        require_hex("arcade_updater_revision", revision, 40)?;
    }
    if revisions.len() != IDS.len() {
        return classified(
            "invalid_database_identity",
            "all five Arcade updater revisions are required",
        );
    }
    Ok(revisions)
}

fn validate_manifest(payload: &Value) -> AgentResult<()> {
    if !matches!(
        payload["format"].as_str(),
        Some(FORMAT) | Some(PREVIOUS_FORMAT) | Some(LEGACY_FORMAT)
    ) || payload["release_version"]
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
    )?;
    if has_arcade_database(payload) {
        if payload
            .pointer("/sources/arcade_database/repository")
            .and_then(Value::as_str)
            != Some("MiSTer-devel/ArcadeDatabase_MiSTer")
            || payload
                .pointer("/sources/arcade_database/path")
                .and_then(Value::as_str)
                != Some(ARCADE_DATABASE_CSV)
        {
            return classified("invalid_database_manifest", "ArcadeDatabase source");
        }
        for pointer in [
            "/sources/arcade_database/sha",
            "/sources/arcade_database/builder_sha",
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
        for pointer in [
            "/sources/arcade_database/csv_sha256",
            "/sources/arcade_database/license_sha256",
        ] {
            require_hex(
                pointer,
                payload
                    .pointer(pointer)
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                64,
            )?;
        }
    }
    if payload["format"] == FORMAT {
        if payload
            .pointer("/sources/arcade_updater/format")
            .and_then(Value::as_str)
            != Some(mister_magik_catalog::arcade_updater_index::FORMAT)
        {
            return classified("invalid_database_manifest", "Arcade updater format");
        }
        for pointer in ["/sources/arcade_updater/sha256"] {
            require_hex(
                pointer,
                payload
                    .pointer(pointer)
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                64,
            )?;
        }
        require_hex(
            "/sources/arcade_updater/builder_sha",
            payload
                .pointer("/sources/arcade_updater/builder_sha")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            40,
        )?;
        if payload
            .pointer("/sources/arcade_updater/sources")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
        {
            return classified("invalid_database_manifest", "Arcade updater sources");
        }
    }
    Ok(())
}

fn has_arcade_database(payload: &Value) -> bool {
    matches!(
        payload["format"].as_str(),
        Some(FORMAT) | Some(PREVIOUS_FORMAT)
    )
}

fn expected_archive_members(payload: &Value) -> Vec<&'static str> {
    let mut members = vec![DATABASES[0], DATABASES[1], MANIFEST, CHECKSUMS];
    if has_arcade_database(payload) {
        members.extend([ARCADE_DATABASE_CSV, ARCADE_DATABASE_LICENSE]);
    }
    if payload["format"] == FORMAT {
        members.push(ARCADE_UPDATER_INDEX);
    }
    members
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
        statement
            .query_map([], |row| row.get(1))
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .collect()
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
    let expected_files: BTreeSet<_> = if payload["format"] == FORMAT {
        [
            DATABASES[0],
            DATABASES[1],
            ARCADE_DATABASE_CSV,
            ARCADE_DATABASE_LICENSE,
            ARCADE_UPDATER_INDEX,
        ]
        .into_iter()
        .collect()
    } else if payload["format"] == PREVIOUS_FORMAT {
        [
            DATABASES[0],
            DATABASES[1],
            ARCADE_DATABASE_CSV,
            ARCADE_DATABASE_LICENSE,
        ]
        .into_iter()
        .collect()
    } else {
        DATABASES.into_iter().collect()
    };
    if entries.len() != expected_files.len() {
        return classified("database_file_manifest", "unexpected file count");
    }
    for entry in entries {
        let name = entry["path"].as_str().unwrap_or_default();
        let bytes = files.get(name).ok_or("manifest database is missing")?;
        if !expected_files.contains(name)
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
    let expected: BTreeSet<_> = expected_archive_members(
        &serde_json::from_slice(&files[MANIFEST]).map_err(|error| error.to_string())?,
    )
    .into_iter()
    .filter(|name| *name != CHECKSUMS)
    .collect();
    if seen != expected {
        return classified("database_checksum_shape", "unexpected checksum set");
    }
    Ok(())
}

fn validate_arcade_database(path: &Path, source_sha: &str, csv_sha256: &str) -> AgentResult<()> {
    let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("invalid SQLite database {}: {error}", path.display()))?;
    let source: (i64, String, String, String, String, i64, i64) = database
        .query_row(
            "SELECT schema_version,repository,source_path,source_sha,csv_sha256,
                    row_count,category_count
             FROM mister_arcade_source WHERE id=1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .map_err(|error| format!("invalid ArcadeDatabase source metadata: {error}"))?;
    if source.0 != 1
        || source.1 != "MiSTer-devel/ArcadeDatabase_MiSTer"
        || source.2 != ARCADE_DATABASE_CSV
        || source.3 != source_sha
        || source.4 != csv_sha256
    {
        return classified("database_source_version", "ArcadeDatabase source mismatch");
    }
    let rows: i64 = database
        .query_row("SELECT count(*) FROM mister_arcade_entries", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if rows != source.5 || rows < 2_800 || source.6 < 100 {
        return classified(
            "database_row_count",
            format!("ArcadeDatabase has rows={rows} categories={}", source.6),
        );
    }
    Ok(())
}

fn verify_arcade_database_source_files(
    payload: &Value,
    files: &BTreeMap<String, Vec<u8>>,
) -> AgentResult<()> {
    for (name, pointer) in [
        (ARCADE_DATABASE_CSV, "/sources/arcade_database/csv_sha256"),
        (
            ARCADE_DATABASE_LICENSE,
            "/sources/arcade_database/license_sha256",
        ),
    ] {
        let expected = payload
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default();
        let actual = files
            .get(name)
            .map(|bytes| digest_bytes(bytes))
            .unwrap_or_default();
        if actual != expected {
            return classified(
                "database_source_checksum",
                format!("{name} does not match {pointer}"),
            );
        }
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

    fn updater_revisions() -> Vec<String> {
        [
            "distribution",
            "alternatives",
            "jtcores",
            "coinop",
            "arcade-offset",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, id)| format!("{id}={}", format!("{:x}", index + 1).repeat(40)))
        .collect()
    }

    #[test]
    fn update_plan_starts_at_version_one() {
        let result = update_plan(
            None,
            "mame0288",
            &"a".repeat(40),
            "tag24532",
            &"b".repeat(40),
            &"c".repeat(40),
            &updater_revisions(),
        )
        .unwrap();
        assert_eq!(result["next_version"], 1);
        assert_eq!(result["update_needed"], true);
    }

    #[test]
    fn legacy_manifest_schedules_arcade_database_upgrade() {
        let current = json!({
            "format": LEGACY_FORMAT,
            "release_version": 4,
            "sources": {
                "mame": {
                    "tag": "mame0288",
                    "sha": "a".repeat(40),
                    "builder_sha": "b".repeat(40),
                    "listxml_sha256": "c".repeat(64)
                },
                "hbmame": {
                    "tag": "tag24532",
                    "sha": "d".repeat(40),
                    "builder_sha": "e".repeat(40)
                }
            }
        });
        let result = update_plan(
            Some(&current),
            "mame0288",
            &"a".repeat(40),
            "tag24532",
            &"d".repeat(40),
            &"f".repeat(40),
            &updater_revisions(),
        )
        .unwrap();

        assert_eq!(result["next_version"], 5);
        assert_eq!(result["mame_changed"], false);
        assert_eq!(result["hbmame_changed"], false);
        assert_eq!(result["arcade_database_changed"], true);
        assert_eq!(result["arcade_updater_changed"], true);
        assert_eq!(result["update_needed"], true);
    }

    #[test]
    fn updater_revision_contract_requires_each_canonical_source_once() {
        assert!(parse_updater_revisions(&updater_revisions()).is_ok());
        let mut incomplete = updater_revisions();
        incomplete.pop();
        assert!(parse_updater_revisions(&incomplete).is_err());
        let mut duplicate = updater_revisions();
        duplicate[4] = duplicate[0].clone();
        assert!(parse_updater_revisions(&duplicate).is_err());
    }

    #[test]
    fn arcade_source_hashes_must_match_bundled_files() {
        let csv = b"setname,name\n1942,1942\n".to_vec();
        let license = b"GPL-3.0-or-later\n".to_vec();
        let payload = json!({
            "sources": {
                "arcade_database": {
                    "csv_sha256": digest_bytes(&csv),
                    "license_sha256": digest_bytes(&license)
                }
            }
        });
        let files = BTreeMap::from([
            (ARCADE_DATABASE_CSV.to_string(), csv),
            (ARCADE_DATABASE_LICENSE.to_string(), license),
        ]);

        assert!(verify_arcade_database_source_files(&payload, &files).is_ok());
        let mut mismatched = payload;
        mismatched["sources"]["arcade_database"]["license_sha256"] = Value::String("0".repeat(64));
        assert!(verify_arcade_database_source_files(&mismatched, &files).is_err());
    }

    #[test]
    fn source_tags_are_closed() {
        assert!(validate_tags("mame0288", "tag24532").is_ok());
        assert!(validate_tags("0288", "tag24532").is_err());
        assert!(validate_tags("mame0288", "branch").is_err());
    }

    #[test]
    fn identities_and_archive_members_are_format_specific() {
        assert!(require_hex("sha", &"a".repeat(40), 40).is_ok());
        assert!(require_hex("sha", &"A".repeat(40), 40).is_err());
        assert!(require_hex("sha", &"g".repeat(40), 40).is_err());
        assert_eq!(
            expected_archive_members(&json!({"format": LEGACY_FORMAT})).len(),
            4
        );
        assert_eq!(
            expected_archive_members(&json!({"format": FORMAT})).len(),
            7
        );
        assert_eq!(
            expected_archive_members(&json!({"format": PREVIOUS_FORMAT})).len(),
            6
        );
    }

    #[test]
    fn synthetic_archive_checksums_require_exact_unique_members() {
        let manifest = serde_json::to_vec(&json!({"format": LEGACY_FORMAT})).unwrap();
        let mut files = BTreeMap::from([
            (DATABASES[0].to_string(), b"mame-db".to_vec()),
            (DATABASES[1].to_string(), b"hbmame-db".to_vec()),
            (MANIFEST.to_string(), manifest),
        ]);
        let checksums = [DATABASES[0], DATABASES[1], MANIFEST]
            .into_iter()
            .map(|name| format!("{}  {name}", digest_bytes(&files[name])))
            .collect::<Vec<_>>()
            .join("\n");
        files.insert(CHECKSUMS.to_string(), checksums.into_bytes());
        verify_checksums(&files).unwrap();

        let mut tampered = files.clone();
        tampered.insert(DATABASES[0].to_string(), b"changed".to_vec());
        assert!(verify_checksums(&tampered).is_err());
        let mut duplicate = files;
        duplicate.get_mut(CHECKSUMS).unwrap().extend_from_slice(
            format!("\n{}  {}", digest_bytes(b"mame-db"), DATABASES[0]).as_bytes(),
        );
        assert!(verify_checksums(&duplicate).is_err());
    }
}
