// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Fixed Dev card-pack installation. No caller-selected destination or archive paths.
use mister_magik_catalog::bounded_file;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const SOURCE_BYTES: usize = 360 * 504 * 3;
const MAX_INDEX: usize = 128 * 1024;
const MAX_FILES: usize = 120;
const STAMP: &str = ".installed-pack.json";
const ROOTS: [&str; 6] = [
    "arcade",
    "consoles",
    "computers",
    "handhelds",
    "favourites",
    "settings",
];

#[derive(Deserialize)]
struct Index {
    schema: u32,
    width: u32,
    height: u32,
    format: String,
    cards: BTreeMap<String, Source>,
}
#[derive(Deserialize)]
struct Source {
    file: String,
    sha256: String,
    #[serde(default)]
    contains_name: bool,
    #[serde(default)]
    prepared: Option<Prepared>,
}
#[derive(Deserialize)]
struct Prepared {
    file: String,
    sha256: String,
    bytes: usize,
}
#[derive(Serialize, Deserialize)]
struct Stamp {
    sha256: String,
    index_sha256: String,
}
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn files(index: &[u8]) -> Result<BTreeMap<String, (String, usize)>, String> {
    let index: Index = serde_json::from_slice(index).map_err(|e| e.to_string())?;
    if index.schema != 1
        || index.width != 360
        || index.height != 504
        || index.format != "RGB888"
        || ROOTS
            .iter()
            .any(|name| !index.cards.contains_key(&format!("root:{name}")))
    {
        return Err("unsupported or incomplete card artwork index".into());
    }
    let mut files = BTreeMap::new();
    for source in index.cards.values() {
        // Deserializing also validates optional presentation metadata.
        let _ = source.contains_name;
        if !source.file.ends_with(".rgb888")
            || !source
                .file
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !source
                .file
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            || source.sha256.len() != 64
            || !source
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("invalid card artwork filename or checksum".into());
        }
        if let Some(previous) =
            files.insert(source.file.clone(), (source.sha256.clone(), SOURCE_BYTES))
            && previous != (source.sha256.clone(), SOURCE_BYTES)
        {
            return Err("conflicting card artwork checksums".into());
        }
    }
    for source in index.cards.values() {
        if let Some(prepared) = &source.prepared {
            if !prepared.file.ends_with(".cardtex")
                || prepared.file.starts_with('.')
                || !prepared
                    .file
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                || !valid_digest(&prepared.sha256)
                || !(17..=2 * 1024 * 1024).contains(&prepared.bytes)
            {
                return Err("invalid prepared card artwork".into());
            }
            let entry = (prepared.sha256.clone(), prepared.bytes);
            if files
                .insert(prepared.file.clone(), entry.clone())
                .is_some_and(|previous| previous != entry)
            {
                return Err("conflicting prepared artwork declarations".into());
            }
        }
    }
    if files.is_empty() || files.len() > MAX_FILES {
        return Err("card artwork file count exceeds limit".into());
    }
    Ok(files)
}

fn recover(parent: &Path) -> Result<(), String> {
    let current = parent.join("launcher-cards");
    let previous = parent.join(".launcher-cards.previous");
    if !current.exists() && previous.exists() {
        fs::rename(&previous, &current).map_err(|e| e.to_string())?;
    }
    if current.exists() && previous.exists() {
        fs::remove_dir_all(previous).map_err(|e| e.to_string())?;
    }
    let next = parent.join(".launcher-cards.next");
    if next.exists() {
        fs::remove_dir_all(next).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// A stamp is trusted only while the manifest matches and all named files have
/// their exact installed size. Content hashes are checked during installation.
pub fn state(parent: &Path) -> Option<String> {
    recover(parent).ok()?;
    let current = parent.join("launcher-cards");
    let raw = bounded_file::read(current.join("index.json"), MAX_INDEX as u64).ok()?;
    let stamp: Stamp =
        serde_json::from_slice(&bounded_file::read(current.join(STAMP), 1024).ok()?).ok()?;
    if stamp.index_sha256 != digest(&raw) {
        return None;
    }
    for (name, (_, size)) in files(&raw).ok()? {
        let metadata = fs::symlink_metadata(current.join(name)).ok()?;
        if !metadata.is_file() || metadata.len() != size as u64 {
            return None;
        }
    }
    Some(stamp.sha256)
}

/// Bundle: big-endian index byte length, exact index bytes, then one RGB888
/// image per unique filename in lexical order. Transport SHA is verified by upload.
pub fn install(parent: &Path, reader: &mut impl Read, hash: &str) -> Result<usize, String> {
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    recover(parent)?;
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix).map_err(|e| e.to_string())?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_INDEX {
        return Err("oversized card artwork index".into());
    }
    let mut raw = vec![0; length];
    reader.read_exact(&mut raw).map_err(|e| e.to_string())?;
    let sources = files(&raw)?;
    let next = Staging(parent.join(".launcher-cards.next"));
    fs::create_dir(&next.0).map_err(|e| e.to_string())?;
    for (name, (expected, size)) in &sources {
        let mut bytes = vec![0; *size];
        reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        if digest(&bytes) != *expected {
            return Err(format!("card artwork checksum mismatch: {name}"));
        }
        let mut file = File::create(next.0.join(name)).map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
    }
    let mut extra = [0];
    if reader.read(&mut extra).map_err(|e| e.to_string())? != 0 {
        return Err("trailing card artwork bytes".into());
    }
    for (name, bytes) in [
        ("index.json", raw.clone()),
        (
            STAMP,
            serde_json::to_vec(&Stamp {
                sha256: hash.into(),
                index_sha256: digest(&raw),
            })
            .map_err(|e| e.to_string())?,
        ),
    ] {
        let mut file = File::create(next.0.join(name)).map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
    }
    File::open(&next.0)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    let current = parent.join("launcher-cards");
    let previous = parent.join(".launcher-cards.previous");
    if current.exists() {
        fs::rename(&current, &previous).map_err(|e| e.to_string())?;
    }
    if let Err(error) = fs::rename(&next.0, &current) {
        if previous.exists() {
            fs::rename(&previous, &current)
                .map_err(|rollback| format!("{error}; rollback: {rollback}"))?;
        }
        return Err(error.to_string());
    }
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    if previous.exists() {
        fs::remove_dir_all(previous).map_err(|e| e.to_string())?;
    }
    Ok(sources.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bundle() -> Vec<u8> {
        let pixels = vec![123; SOURCE_BYTES];
        let cards: serde_json::Map<String, serde_json::Value> = ROOTS
            .iter()
            .map(|name| {
                (
                    format!("root:{name}"),
                    serde_json::json!({"file":"fixture.rgb888","sha256":digest(&pixels)}),
                )
            })
            .collect();
        let index = serde_json::to_vec(&serde_json::json!({"schema":1,"width":360,"height":504,"format":"RGB888","cards":cards})).unwrap();
        let mut body = (index.len() as u32).to_be_bytes().to_vec();
        body.extend(index);
        body.extend(pixels);
        body
    }
    #[test]
    fn prepared_entries_are_installed_atomically_and_have_exact_lengths() {
        let raw = bundle();
        let length = u32::from_be_bytes(raw[..4].try_into().unwrap()) as usize;
        let mut index: serde_json::Value = serde_json::from_slice(&raw[4..4 + length]).unwrap();
        let prepared = b"MGCART01 bounded prepared fixture";
        for source in index["cards"].as_object_mut().unwrap().values_mut() {
            source["prepared"] = serde_json::json!({"file":"fixture.cardtex", "bytes":prepared.len(), "sha256":digest(prepared)});
        }
        let index = serde_json::to_vec(&index).unwrap();
        let mut body = (index.len() as u32).to_be_bytes().to_vec();
        body.extend(index);
        body.extend(prepared);
        body.extend(&raw[4 + length..]);
        let root =
            std::env::temp_dir().join(format!("magik-prepared-installer-{}", std::process::id()));
        let hash = digest(&body);
        assert_eq!(install(&root, &mut body.as_slice(), &hash).unwrap(), 2);
        assert_eq!(
            fs::read(root.join("launcher-cards/fixture.cardtex")).unwrap(),
            prepared
        );
        assert_eq!(state(&root), Some(hash.clone()));
        assert!(install(&root, &mut &body[..body.len() - 1], &hash).is_err());
        assert_eq!(state(&root), Some(hash));
        fs::write(root.join("launcher-cards/fixture.cardtex"), b"short").unwrap();
        assert!(state(&root).is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn valid_pack_installs_and_failed_replacement_keeps_previous_pack() {
        let root = std::env::temp_dir().join(format!(
            "magik-artwork-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let body = bundle();
        let hash = digest(&body);
        assert_eq!(install(&root, &mut body.as_slice(), &hash).unwrap(), 1);
        assert_eq!(state(&root), Some(hash.clone()));
        let mut corrupt = body.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(install(&root, &mut corrupt.as_slice(), &digest(&corrupt)).is_err());
        assert!(install(&root, &mut &body[..body.len() - 1], &hash).is_err());
        let mut extra = body.clone();
        extra.push(0);
        assert!(install(&root, &mut extra.as_slice(), &digest(&extra)).is_err());
        assert_eq!(state(&root), Some(hash));
        fs::rename(
            root.join("launcher-cards"),
            root.join(".launcher-cards.previous"),
        )
        .unwrap();
        assert!(
            state(&root).is_some(),
            "interrupted publication restores the previous pack"
        );
        fs::write(root.join("launcher-cards/fixture.rgb888"), b"short").unwrap();
        assert!(state(&root).is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_index_cannot_select_paths_or_change_pixel_layout() {
        let body = bundle();
        let length = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
        let original: serde_json::Value = serde_json::from_slice(&body[4..4 + length]).unwrap();
        for bad in [
            "../production.rgb888",
            ".hidden.rgb888",
            "dir/file.rgb888",
            "file.png",
        ] {
            let mut index = original.clone();
            index["cards"]["root:arcade"]["file"] = bad.into();
            assert!(files(&serde_json::to_vec(&index).unwrap()).is_err());
        }
        let mut index = original.clone();
        index["cards"]["root:arcade"]["contains_name"] = "true".into();
        assert!(files(&serde_json::to_vec(&index).unwrap()).is_err());
        let mut index = original.clone();
        index["width"] = 180.into();
        assert!(files(&serde_json::to_vec(&index).unwrap()).is_err());
        let mut index = original;
        index["cards"]
            .as_object_mut()
            .unwrap()
            .remove("root:arcade");
        assert!(files(&serde_json::to_vec(&index).unwrap()).is_err());
    }
}
