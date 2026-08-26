// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Disposable source state for the independent fast catalog.
//!
//! The watch index is intentionally separate from cached rows. An unchanged
//! refresh reads and validates only the manifest and watch indexes; large row
//! snapshots are opened only after a source change is proven.

use crate::fast_five_catalog::FastFiveGameVariant;
use crate::system_shard::SystemGame;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const REFRESH_SCHEMA: u32 = 1;
const ENVELOPE_VERSION: u32 = 1;
const ENVELOPE_BYTES: usize = 64;
const MANIFEST_MAGIC: &[u8; 8] = b"MGKRFSMF";
const WATCH_MAGIC: &[u8; 8] = b"MGKRFSWI";
const ROWS_MAGIC: &[u8; 8] = b"MGKRFSRW";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_WATCH_BYTES: usize = 16 * 1024 * 1024;
const MAX_ROWS_BYTES: usize = 128 * 1024 * 1024;
const STATE_DIRECTORY: &str = "fast-refresh-v1";
const MANIFEST_A: &str = "manifest-a.bin";
const MANIFEST_B: &str = "manifest-b.bin";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastRefreshManifest {
    pub schema: u32,
    pub generation: u64,
    pub catalog_generation: u64,
    pub catalog_fingerprint: String,
    pub builder_identity: String,
    pub systems: Vec<FastRefreshSystemRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastRefreshSystemRef {
    pub system_id: String,
    pub watch_path: String,
    pub watch_sha256: String,
    pub rows_path: String,
    pub rows_sha256: String,
    pub source_fingerprint: String,
    pub row_fingerprint: String,
    pub games: u64,
    pub variants: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastSystemWatchIndex {
    pub schema: u32,
    pub system_id: String,
    pub adapter_version: u32,
    pub core_profile_fingerprint: String,
    pub roots: Vec<String>,
    pub directories: Vec<FastWatchedDirectory>,
    pub containers: Vec<FastWatchedContainer>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastWatchedDirectory {
    pub path: String,
    pub modified_ns: i128,
    pub entry_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastWatchedContainer {
    pub path: String,
    pub size: u64,
    pub modified_ns: i128,
    pub changed_ns: i128,
    pub inode: u64,
    pub content_directory_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastSystemRowsSnapshot {
    pub schema: u32,
    pub system_id: String,
    pub games: Vec<SystemGame>,
    pub variants: Vec<FastFiveGameVariant>,
}

#[derive(Clone, Debug)]
pub struct FastRefreshSystemState {
    pub watch: FastSystemWatchIndex,
    pub rows: FastSystemRowsSnapshot,
}

impl FastRefreshManifest {
    pub fn new(
        generation: u64,
        catalog_generation: u64,
        catalog_fingerprint: String,
        builder_identity: String,
        systems: Vec<FastRefreshSystemRef>,
    ) -> Result<Self, String> {
        let manifest = Self {
            schema: REFRESH_SCHEMA,
            generation,
            catalog_generation,
            catalog_fingerprint,
            builder_identity,
            systems,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != REFRESH_SCHEMA || self.generation == 0 {
            return Err("unsupported fast refresh manifest".to_string());
        }
        validate_sha256(&self.catalog_fingerprint, "catalog fingerprint")?;
        if self.builder_identity.trim().is_empty() {
            return Err("fast refresh builder identity is empty".to_string());
        }
        let mut systems = BTreeSet::new();
        for system in &self.systems {
            if !systems.insert(system.system_id.as_str()) {
                return Err(format!("duplicate refresh system {}", system.system_id));
            }
            validate_relative_path(&system.watch_path)?;
            validate_relative_path(&system.rows_path)?;
            validate_sha256(&system.watch_sha256, "watch checksum")?;
            validate_sha256(&system.rows_sha256, "row checksum")?;
            validate_sha256(&system.source_fingerprint, "source fingerprint")?;
            validate_sha256(&system.row_fingerprint, "row fingerprint")?;
        }
        Ok(())
    }
}

impl FastSystemWatchIndex {
    pub fn validate(&self, expected_system_id: &str) -> Result<(), String> {
        if self.schema != REFRESH_SCHEMA || self.system_id != expected_system_id {
            return Err(format!("invalid watch index for {expected_system_id}"));
        }
        validate_sha256(&self.core_profile_fingerprint, "core/profile fingerprint")?;
        validate_unique_paths(self.roots.iter().map(String::as_str), "watch root")?;
        validate_unique_paths(
            self.directories.iter().map(|entry| entry.path.as_str()),
            "watched directory",
        )?;
        validate_unique_paths(
            self.containers.iter().map(|entry| entry.path.as_str()),
            "watched container",
        )?;
        for directory in &self.directories {
            validate_sha256(&directory.entry_fingerprint, "directory fingerprint")?;
        }
        for container in &self.containers {
            validate_sha256(
                &container.content_directory_fingerprint,
                "container directory fingerprint",
            )?;
        }
        Ok(())
    }
}

impl FastSystemRowsSnapshot {
    pub fn validate(&self, expected_system_id: &str) -> Result<(), String> {
        if self.schema != REFRESH_SCHEMA || self.system_id != expected_system_id {
            return Err(format!("invalid row snapshot for {expected_system_id}"));
        }
        let prefix = format!("{expected_system_id}\u{1f}");
        let mut keys = BTreeSet::new();
        for game in &self.games {
            if !game.stable_key.starts_with(&prefix)
                || game.title.trim().is_empty()
                || game.launch_ref.trim().is_empty()
                || !keys.insert(game.stable_key.as_str())
            {
                return Err(format!("invalid cached row in {expected_system_id}"));
            }
        }
        for variant in &self.variants {
            if !variant.game.stable_key.starts_with(&prefix)
                || !keys.insert(variant.game.stable_key.as_str())
            {
                return Err(format!("invalid cached variant in {expected_system_id}"));
            }
        }
        Ok(())
    }
}

pub fn refresh_state_root(catalog_root: &Path) -> PathBuf {
    catalog_root.join("state").join(STATE_DIRECTORY)
}

pub fn read_latest_refresh_manifest(catalog_root: &Path) -> Result<FastRefreshManifest, String> {
    let root = refresh_state_root(catalog_root);
    let mut candidates = [MANIFEST_A, MANIFEST_B]
        .into_iter()
        .filter_map(|name| {
            let path = root.join(name);
            read_envelope::<FastRefreshManifest>(&path, MANIFEST_MAGIC, MAX_MANIFEST_BYTES)
                .ok()
                .and_then(|manifest| manifest.validate().ok().map(|()| manifest))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|manifest| manifest.generation);
    candidates
        .pop()
        .ok_or_else(|| "no valid fast refresh manifest".to_string())
}

pub fn read_system_watch(
    catalog_root: &Path,
    reference: &FastRefreshSystemRef,
) -> Result<FastSystemWatchIndex, String> {
    let path = refresh_state_root(catalog_root).join(&reference.watch_path);
    let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    verify_file_checksum(&bytes, &reference.watch_sha256, &path)?;
    let watch = decode_envelope(&bytes, WATCH_MAGIC, MAX_WATCH_BYTES)?;
    watch.validate(&reference.system_id)?;
    Ok(watch)
}

pub fn read_system_rows(
    catalog_root: &Path,
    reference: &FastRefreshSystemRef,
) -> Result<FastSystemRowsSnapshot, String> {
    let path = refresh_state_root(catalog_root).join(&reference.rows_path);
    let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    verify_file_checksum(&bytes, &reference.rows_sha256, &path)?;
    let rows = decode_envelope(&bytes, ROWS_MAGIC, MAX_ROWS_BYTES)?;
    rows.validate(&reference.system_id)?;
    Ok(rows)
}

pub fn publish_refresh_state(
    catalog_root: &Path,
    generation: u64,
    catalog_generation: u64,
    catalog_fingerprint: String,
    builder_identity: String,
    systems: &[FastRefreshSystemState],
) -> Result<FastRefreshManifest, String> {
    if generation == 0 {
        return Err("fast refresh generation must be non-zero".to_string());
    }
    let root = refresh_state_root(catalog_root);
    fs::create_dir_all(root.join("systems"))
        .map_err(|error| format!("create refresh state root: {error}"))?;
    let mut references = Vec::with_capacity(systems.len());
    for state in systems {
        state.watch.validate(&state.rows.system_id)?;
        state.rows.validate(&state.watch.system_id)?;
        let system_dir = root.join("systems").join(&state.watch.system_id);
        fs::create_dir_all(&system_dir)
            .map_err(|error| format!("create {} refresh state: {error}", state.watch.system_id))?;
        let watch_relative = format!("systems/{}/{generation}.watch", state.watch.system_id);
        let rows_relative = format!("systems/{}/{generation}.rows", state.watch.system_id);
        let watch_bytes = encode_envelope(&state.watch, WATCH_MAGIC)?;
        let rows_bytes = encode_envelope(&state.rows, ROWS_MAGIC)?;
        write_new_file(&root.join(&watch_relative), &watch_bytes)?;
        write_new_file(&root.join(&rows_relative), &rows_bytes)?;
        references.push(FastRefreshSystemRef {
            system_id: state.watch.system_id.clone(),
            watch_path: watch_relative,
            watch_sha256: sha256_hex(&watch_bytes),
            rows_path: rows_relative,
            rows_sha256: sha256_hex(&rows_bytes),
            source_fingerprint: source_fingerprint(&state.watch),
            row_fingerprint: row_fingerprint(&state.rows)?,
            games: state.rows.games.len().try_into().unwrap_or(u64::MAX),
            variants: state.rows.variants.len().try_into().unwrap_or(u64::MAX),
        });
    }
    references.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    let manifest = FastRefreshManifest::new(
        generation,
        catalog_generation,
        catalog_fingerprint,
        builder_identity,
        references,
    )?;
    let bytes = encode_envelope(&manifest, MANIFEST_MAGIC)?;
    let slot = if generation.is_multiple_of(2) {
        MANIFEST_A
    } else {
        MANIFEST_B
    };
    write_replace_file(&root.join(slot), &bytes)?;
    sync_directory(&root)?;
    Ok(manifest)
}

pub fn source_fingerprint(watch: &FastSystemWatchIndex) -> String {
    let bytes = postcard::to_allocvec(watch).expect("serializable watch index");
    sha256_hex(&bytes)
}

pub fn row_fingerprint(rows: &FastSystemRowsSnapshot) -> Result<String, String> {
    rows.validate(&rows.system_id)?;
    postcard::to_allocvec(rows)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| format!("encode row fingerprint: {error}"))
}

fn encode_envelope<T: Serialize>(value: &T, magic: &[u8; 8]) -> Result<Vec<u8>, String> {
    let payload = postcard::to_allocvec(value).map_err(|error| format!("encode state: {error}"))?;
    let payload_len = u64::try_from(payload.len()).map_err(|_| "state is too large")?;
    let mut output = Vec::with_capacity(ENVELOPE_BYTES + payload.len());
    output.extend_from_slice(magic);
    output.extend_from_slice(&ENVELOPE_VERSION.to_le_bytes());
    output.extend_from_slice(&REFRESH_SCHEMA.to_le_bytes());
    output.extend_from_slice(&payload_len.to_le_bytes());
    output.extend_from_slice(&Sha256::digest(&payload));
    output.extend_from_slice(&[0; 8]);
    debug_assert_eq!(output.len(), ENVELOPE_BYTES);
    output.extend_from_slice(&payload);
    Ok(output)
}

fn read_envelope<T: DeserializeOwned>(
    path: &Path,
    magic: &[u8; 8],
    maximum: usize,
) -> Result<T, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("stat {}: {error}", path.display()))?;
    if metadata.len() > maximum.try_into().unwrap_or(u64::MAX) {
        return Err(format!("{} exceeds its size bound", path.display()));
    }
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    decode_envelope(&bytes, magic, maximum)
}

fn decode_envelope<T: DeserializeOwned>(
    bytes: &[u8],
    magic: &[u8; 8],
    maximum: usize,
) -> Result<T, String> {
    if bytes.len() > maximum || bytes.len() < ENVELOPE_BYTES {
        return Err("fast refresh state length is invalid".to_string());
    }
    let header = &bytes[..ENVELOPE_BYTES];
    if &header[..8] != magic
        || u32::from_le_bytes(header[8..12].try_into().expect("version bytes")) != ENVELOPE_VERSION
        || u32::from_le_bytes(header[12..16].try_into().expect("schema bytes")) != REFRESH_SCHEMA
    {
        return Err("fast refresh state header is invalid".to_string());
    }
    let payload_len = usize::try_from(u64::from_le_bytes(
        header[16..24].try_into().expect("length bytes"),
    ))
    .map_err(|_| "fast refresh payload is too large")?;
    let payload = &bytes[ENVELOPE_BYTES..];
    if payload.len() != payload_len || Sha256::digest(payload).as_slice() != &header[24..56] {
        return Err("fast refresh payload checksum differs".to_string());
    }
    postcard::from_bytes(payload).map_err(|error| format!("decode refresh state: {error}"))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        let existing =
            fs::read(path).map_err(|error| format!("read existing {}: {error}", path.display()))?;
        if existing == bytes {
            return Ok(());
        }
        return Err(format!(
            "immutable refresh state already exists: {}",
            path.display()
        ));
    }
    write_synced(path, bytes, false)
}

fn write_replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    write_synced(&temporary, bytes, true)?;
    fs::rename(&temporary, path).map_err(|error| format!("publish {}: {error}", path.display()))
}

fn write_synced(path: &Path, bytes: &[u8], replace: bool) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create(true);
    if replace {
        options.truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write {}: {error}", path.display()))
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("sync {}: {error}", path.display()))
}

fn verify_file_checksum(bytes: &[u8], expected: &str, path: &Path) -> Result<(), String> {
    if sha256_hex(bytes) == expected {
        Ok(())
    } else {
        Err(format!("{} checksum differs", path.display()))
    }
}

fn validate_relative_path(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
    {
        Err(format!("unsafe refresh state path {value}"))
    } else {
        Ok(())
    }
}

fn validate_unique_paths<'a>(
    paths: impl IntoIterator<Item = &'a str>,
    label: &str,
) -> Result<(), String> {
    let mut unique = BTreeSet::new();
    for path in paths {
        if path.trim().is_empty() || !unique.insert(path) {
            return Err(format!("invalid or duplicate {label}: {path}"));
        }
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(format!("{label} is not SHA-256"))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_five_catalog::FastFiveVariantRelation;

    fn state(system_id: &str) -> FastRefreshSystemState {
        let game = SystemGame {
            stable_key: format!("{system_id}\u{1f}game"),
            title: "Game".to_string(),
            launch_ref: "/media/fat/games/Game.rom".to_string(),
            preview_archive_path: String::new(),
            preview_asset_key: String::new(),
            has_preview: false,
            year: None,
            manufacturer: String::new(),
            category: "Games".to_string(),
            players: None,
            control: String::new(),
            is_new: false,
            launch_plan: None,
        };
        FastRefreshSystemState {
            watch: FastSystemWatchIndex {
                schema: REFRESH_SCHEMA,
                system_id: system_id.to_string(),
                adapter_version: 1,
                core_profile_fingerprint: "1".repeat(64),
                roots: vec!["/media/fat/games".to_string()],
                directories: vec![FastWatchedDirectory {
                    path: "/media/fat/games".to_string(),
                    modified_ns: 7,
                    entry_fingerprint: "2".repeat(64),
                }],
                containers: Vec::new(),
            },
            rows: FastSystemRowsSnapshot {
                schema: REFRESH_SCHEMA,
                system_id: system_id.to_string(),
                games: vec![game.clone()],
                variants: vec![FastFiveGameVariant {
                    family_stable_key: game.stable_key.clone(),
                    relation: FastFiveVariantRelation::LanguageEdition,
                    game: SystemGame {
                        stable_key: format!("{system_id}\u{1f}variant"),
                        ..game
                    },
                }],
            },
        }
    }

    #[test]
    fn publishes_two_slot_manifest_and_separate_system_state() {
        let root = crate::test_support::unique_temp_dir("fast-refresh-state");
        let first = publish_refresh_state(
            &root,
            1,
            10,
            "a".repeat(64),
            "builder-1".to_string(),
            &[state("snes")],
        )
        .expect("publish first state");
        assert!(refresh_state_root(&root).join(MANIFEST_B).is_file());
        assert_eq!(read_latest_refresh_manifest(&root).unwrap(), first);
        let reference = &first.systems[0];
        assert_eq!(
            read_system_watch(&root, reference).unwrap().system_id,
            "snes"
        );
        assert_eq!(read_system_rows(&root, reference).unwrap().games.len(), 1);

        let second = publish_refresh_state(
            &root,
            2,
            11,
            "b".repeat(64),
            "builder-1".to_string(),
            &[state("snes")],
        )
        .expect("publish second state");
        assert!(refresh_state_root(&root).join(MANIFEST_A).is_file());
        assert_eq!(read_latest_refresh_manifest(&root).unwrap(), second);
    }

    #[test]
    fn rejects_corrupt_envelopes_and_unsafe_paths() {
        let value = state("snes").watch;
        let mut encoded = encode_envelope(&value, WATCH_MAGIC).unwrap();
        let last = encoded.len() - 1;
        encoded[last] ^= 0x55;
        assert!(
            decode_envelope::<FastSystemWatchIndex>(&encoded, WATCH_MAGIC, MAX_WATCH_BYTES)
                .is_err()
        );

        let manifest = FastRefreshManifest::new(
            1,
            1,
            "a".repeat(64),
            "builder".to_string(),
            vec![FastRefreshSystemRef {
                system_id: "snes".to_string(),
                watch_path: "../watch".to_string(),
                watch_sha256: "b".repeat(64),
                rows_path: "rows".to_string(),
                rows_sha256: "c".repeat(64),
                source_fingerprint: "d".repeat(64),
                row_fingerprint: "e".repeat(64),
                games: 0,
                variants: 0,
            }],
        );
        assert!(manifest.is_err());
    }

    #[test]
    fn bounded_decoder_rejects_oversized_state() {
        let encoded = vec![0; MAX_MANIFEST_BYTES + 1];
        assert!(
            decode_envelope::<FastRefreshManifest>(&encoded, MANIFEST_MAGIC, MAX_MANIFEST_BYTES)
                .is_err()
        );
    }
}
