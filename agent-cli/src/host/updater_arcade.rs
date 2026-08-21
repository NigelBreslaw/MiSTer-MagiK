// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Update_All Arcade index construction.

use md5::{Digest as Md5Digest, Md5};
use mister_magik_catalog::arcade_updater_index::{
    ArcadeUpdaterIndex, ArcadeUpdaterRow, ArcadeUpdaterSource,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as Sha2Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

const INPUT_FORMAT: &str = "mister-magik-arcade-updater-inputs-v1";
const SOURCE_ORDER: [&str; 5] = [
    "distribution",
    "alternatives",
    "jtcores",
    "coinop",
    "arcade-offset",
];

#[derive(Deserialize)]
struct InputManifest {
    format: String,
    sources: Vec<InputSource>,
}

#[derive(Deserialize)]
struct InputSource {
    id: String,
    revision: String,
    database: PathBuf,
    roots: Vec<PathBuf>,
}

#[derive(Deserialize)]
struct Database {
    files: BTreeMap<String, DatabaseFile>,
}

#[derive(Deserialize)]
struct DatabaseFile {
    hash: String,
    size: u64,
    #[serde(default)]
    arc_at: Option<String>,
}

pub(super) fn build(input_manifest: &Path, output: &Path) -> Result<Value, String> {
    let started = Instant::now();
    let manifest: InputManifest = serde_json::from_slice(
        &std::fs::read(input_manifest)
            .map_err(|error| format!("read {}: {error}", input_manifest.display()))?,
    )
    .map_err(|error| format!("parse {}: {error}", input_manifest.display()))?;
    if manifest.format != INPUT_FORMAT
        || manifest.sources.len() != SOURCE_ORDER.len()
        || manifest
            .sources
            .iter()
            .map(|source| source.id.as_str())
            .ne(SOURCE_ORDER)
    {
        return Err(
            "Arcade updater inputs must contain the five canonical sources in precedence order"
                .to_string(),
        );
    }

    let mut resolved = BTreeMap::<String, ArcadeUpdaterRow>::new();
    let mut source_counts = BTreeMap::<String, u64>::new();
    let mut index_sources = Vec::new();
    for source in manifest.sources {
        require_lower_hex(&format!("{} revision", source.id), &source.revision, 40)?;
        let database_bytes = std::fs::read(&source.database)
            .map_err(|error| format!("read {}: {error}", source.database.display()))?;
        let database_sha256 = hex(&Sha256::digest(&database_bytes));
        let database: Database = serde_json::from_slice(&database_bytes)
            .map_err(|error| format!("parse {}: {error}", source.database.display()))?;
        let mut count = 0u64;
        for (installed_path, file) in database.files {
            let normalized_path = normalize_installed_path(&installed_path);
            if !normalized_path.starts_with("_Arcade/")
                || !normalized_path.to_ascii_lowercase().ends_with(".mra")
            {
                continue;
            }
            require_lower_hex("MRA MD5", &file.hash, 32)?;
            let source_path =
                resolve_source_path(&source.roots, &normalized_path, file.arc_at.as_deref())?;
            let bytes = std::fs::read(&source_path)
                .map_err(|error| format!("read {}: {error}", source_path.display()))?;
            if bytes.len() as u64 != file.size {
                return Err(format!(
                    "updater size mismatch for {normalized_path}: database={} source={}",
                    file.size,
                    bytes.len()
                ));
            }
            let md5 = hex(&Md5::digest(&bytes));
            if md5 != file.hash {
                return Err(format!("updater MD5 mismatch for {normalized_path}"));
            }
            let inspection = mister_magik_catalog::mra_header::inspect(&bytes)
                .map_err(|error| format!("inspect {normalized_path}: {error}"))?;
            resolved.insert(
                normalized_path.clone(),
                ArcadeUpdaterRow {
                    path: normalized_path,
                    source_id: source.id.clone(),
                    size: file.size,
                    md5,
                    header: inspection.header,
                    primary_rom: inspection.primary_rom,
                },
            );
            count = count.saturating_add(1);
        }
        source_counts.insert(source.id.clone(), count);
        index_sources.push(ArcadeUpdaterSource {
            id: source.id,
            revision: source.revision,
            database_sha256,
        });
    }
    index_sources.sort_by(|left, right| left.id.cmp(&right.id));
    let index = ArcadeUpdaterIndex {
        sources: index_sources,
        rows: resolved.into_values().collect(),
    };
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let bytes = index.write(output)?;
    Ok(json!({
        "format": mister_magik_catalog::arcade_updater_index::FORMAT,
        "rows": index.rows.len(),
        "source_rows": source_counts,
        "compressed_bytes": bytes,
        "generation_us": started.elapsed().as_micros() as u64,
        "output": output,
    }))
}

fn normalize_installed_path(path: &str) -> String {
    path.trim_start_matches('/').replace('\\', "/")
}

fn resolve_source_path(
    roots: &[PathBuf],
    installed_path: &str,
    archive_path: Option<&str>,
) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    for root in roots {
        if let Some(archive_path) = archive_path {
            candidates.push(root.join(normalize_installed_path(archive_path)));
        }
        candidates.push(root.join(installed_path));
        if let Some(relative) = installed_path.strip_prefix("_Arcade/") {
            candidates.push(root.join(relative));
        }
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| format!("no source MRA for {installed_path} in configured source roots"))
}

fn require_lower_hex(label: &str, value: &str, length: usize) -> Result<(), String> {
    if value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(format!(
            "{label} must be {length} lowercase hexadecimal characters"
        ))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(1);

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "mister-magik-updater-arcade-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn later_sources_replace_paths_and_primary_setname_is_recorded() {
        let root = temp_dir();
        let mut sources = Vec::new();
        for (position, id) in SOURCE_ORDER.into_iter().enumerate() {
            let source_root = root.join(id);
            let mra_path = source_root.join("_Arcade/Puck Man.mra");
            std::fs::create_dir_all(mra_path.parent().unwrap()).unwrap();
            let mra = format!(
                "<misterromdescription><name>Puck {position}</name><setname>puckman</setname><rom zip=\"puckman.zip|namco.zip\"><part>00</part></rom></misterromdescription>"
            );
            std::fs::write(&mra_path, &mra).unwrap();
            let database = source_root.join("db.json");
            std::fs::write(
                &database,
                serde_json::to_vec(&json!({"files": {"_Arcade/Puck Man.mra": {
                    "hash": hex(&Md5::digest(mra.as_bytes())),
                    "size": mra.len()
                }}}))
                .unwrap(),
            )
            .unwrap();
            sources.push(json!({
                "id": id,
                "revision": format!("{:040x}", position + 1),
                "database": database,
                "roots": [source_root],
            }));
        }
        let inputs = root.join("inputs.json");
        std::fs::write(
            &inputs,
            serde_json::to_vec(&json!({"format": INPUT_FORMAT, "sources": sources})).unwrap(),
        )
        .unwrap();
        let output = root.join("index.lz4b");
        let summary = build(&inputs, &output).unwrap();
        let index = ArcadeUpdaterIndex::read(&output).unwrap();
        assert_eq!(summary["rows"], 1);
        assert_eq!(index.rows[0].source_id, "arcade-offset");
        assert_eq!(index.rows[0].header.name.as_deref(), Some("Puck 4"));
        assert!(matches!(
            index.rows[0].primary_rom,
            mister_magik_catalog::mra_header::PrimaryRomRequirement::Archive { ref setname, .. }
                if setname == "puckman"
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}
