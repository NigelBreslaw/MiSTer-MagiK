// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Incremental, policy-filtered source facts for sharded reconciliation.

use crate::catalog_domain::{InputId, ScanUnitId};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::fs::{self, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const BUILDER_STATE_SCHEMA_VERSION: u32 = 1;

pub trait InputProbePolicy {
    fn descend_into(&self, relative_directory: &Path) -> bool;
    fn include_file(&self, relative_file: &Path) -> bool;

    fn fingerprint_file(&self, _relative_file: &Path) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputKind {
    Directory,
    File,
}

impl InputKind {
    fn stored(self) -> i64 {
        match self {
            Self::Directory => 1,
            Self::File => 2,
        }
    }

    fn from_stored(value: i64) -> Result<Self, InputFactError> {
        match value {
            1 => Ok(Self::Directory),
            2 => Ok(Self::File),
            _ => Err(InputFactError::new("invalid stored input kind")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputSignature {
    pub kind: InputKind,
    pub len: u64,
    pub modified_ns: u64,
    pub content_fingerprint: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InputSnapshot {
    facts: BTreeMap<InputId, InputSignature>,
}

impl InputSnapshot {
    pub fn facts(&self) -> &BTreeMap<InputId, InputSignature> {
        &self.facts
    }

    pub fn signature(&self, input_id: &InputId) -> Option<&InputSignature> {
        self.facts.get(input_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputChangeKind {
    Added,
    Modified,
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputChange {
    pub input_id: InputId,
    pub kind: InputChangeKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputProbe {
    pub snapshot: InputSnapshot,
    pub changes: Vec<InputChange>,
    pub statted_files: usize,
    pub enumerated_directories: usize,
}

pub fn probe_scan_unit(
    root: &Path,
    scan_unit_id: &ScanUnitId,
    previous: &InputSnapshot,
    policy: &impl InputProbePolicy,
) -> Result<InputProbe, InputFactError> {
    if !root.is_dir() {
        return Err(InputFactError::new("scan-unit root is not a directory"));
    }
    let mut current = BTreeMap::new();
    let mut changes = BTreeMap::new();
    let mut directories_to_enumerate = BTreeSet::from([PathBuf::new()]);
    let mut excluded_directories = BTreeSet::new();
    let mut statted_files = 0;

    for (input_id, old_signature) in previous.facts() {
        if input_id.scan_unit_id() != scan_unit_id {
            continue;
        }
        let relative = input_id.relative_path();
        if excluded_directories
            .iter()
            .any(|directory: &PathBuf| relative.starts_with(directory))
        {
            changes.insert(input_id.clone(), InputChangeKind::Removed);
            continue;
        }
        let absolute = root.join(relative);
        let metadata = match fs::symlink_metadata(&absolute) {
            Ok(metadata) if !metadata.file_type().is_symlink() => metadata,
            Ok(_) | Err(_) => {
                changes.insert(input_id.clone(), InputChangeKind::Removed);
                if old_signature.kind == InputKind::Directory {
                    excluded_directories.insert(relative.to_path_buf());
                }
                continue;
            }
        };
        let signature = signature(
            &absolute,
            &metadata,
            old_signature.kind == InputKind::File && policy.fingerprint_file(relative),
        )?;
        let permitted = match signature.kind {
            InputKind::Directory => policy.descend_into(relative),
            InputKind::File => {
                statted_files += 1;
                policy.include_file(relative)
            }
        };
        if !permitted || signature.kind != old_signature.kind {
            changes.insert(input_id.clone(), InputChangeKind::Removed);
            if old_signature.kind == InputKind::Directory {
                excluded_directories.insert(relative.to_path_buf());
            }
            continue;
        }
        if &signature != old_signature {
            changes.insert(input_id.clone(), InputChangeKind::Modified);
            if signature.kind == InputKind::Directory {
                directories_to_enumerate.insert(relative.to_path_buf());
            }
        }
        current.insert(input_id.clone(), signature);
    }

    let mut queue = directories_to_enumerate
        .into_iter()
        .collect::<VecDeque<_>>();
    let mut enumerated = BTreeSet::new();
    while let Some(relative_directory) = queue.pop_front() {
        if !enumerated.insert(relative_directory.clone()) {
            continue;
        }
        let absolute_directory = root.join(&relative_directory);
        let entries = fs::read_dir(&absolute_directory).map_err(|error| {
            InputFactError::with_io("could not enumerate changed directory", error)
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                InputFactError::with_io("could not read changed directory entry", error)
            })?;
            let file_type = entry.file_type().map_err(|error| {
                InputFactError::with_io("could not read changed entry type", error)
            })?;
            if file_type.is_symlink() {
                continue;
            }
            let relative = relative_directory.join(entry.file_name());
            let include = if file_type.is_dir() {
                policy.descend_into(&relative)
            } else if file_type.is_file() {
                policy.include_file(&relative)
            } else {
                false
            };
            if !include {
                continue;
            }
            let input_id = InputId::new(scan_unit_id.clone(), relative.clone())
                .map_err(|error| InputFactError::new(error.to_string()))?;
            if !current.contains_key(&input_id) {
                let metadata = entry.metadata().map_err(|error| {
                    InputFactError::with_io("could not read new input metadata", error)
                })?;
                current.insert(
                    input_id.clone(),
                    signature(
                        &entry.path(),
                        &metadata,
                        file_type.is_file() && policy.fingerprint_file(&relative),
                    )?,
                );
                changes.insert(input_id, InputChangeKind::Added);
                if file_type.is_dir() {
                    queue.push_back(relative);
                }
            }
        }
    }

    Ok(InputProbe {
        snapshot: InputSnapshot { facts: current },
        changes: changes
            .into_iter()
            .map(|(input_id, kind)| InputChange { input_id, kind })
            .collect(),
        statted_files,
        enumerated_directories: enumerated.len(),
    })
}

pub struct InputFactStore {
    connection: Connection,
}

impl InputFactStore {
    pub fn open(path: &Path) -> Result<Self, InputFactError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                InputFactError::with_io("could not create builder-state directory", error)
            })?;
        }
        let connection = Connection::open(path)
            .map_err(|error| InputFactError::new(format!("open builder state: {error}")))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=FULL;
                 CREATE TABLE IF NOT EXISTS builder_meta (
                     key TEXT PRIMARY KEY,
                     value INTEGER NOT NULL
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS input_facts (
                     scan_unit_id TEXT NOT NULL,
                     relative_path TEXT NOT NULL,
                     kind INTEGER NOT NULL,
                     len INTEGER NOT NULL,
                     modified_ns INTEGER NOT NULL,
                     content_fingerprint INTEGER,
                     PRIMARY KEY(scan_unit_id, relative_path)
                 ) WITHOUT ROWID;",
            )
            .map_err(|error| InputFactError::new(format!("initialize builder state: {error}")))?;
        connection
            .execute(
                "INSERT INTO builder_meta(key,value) VALUES ('schema_version',?1)
                 ON CONFLICT(key) DO NOTHING",
                [BUILDER_STATE_SCHEMA_VERSION],
            )
            .map_err(|error| InputFactError::new(format!("write builder schema: {error}")))?;
        let stored: u32 = connection
            .query_row(
                "SELECT value FROM builder_meta WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| InputFactError::new(format!("read builder schema: {error}")))?;
        if stored != BUILDER_STATE_SCHEMA_VERSION {
            return Err(InputFactError::new("unsupported builder-state schema"));
        }
        Ok(Self { connection })
    }

    pub fn load_scan_unit(
        &self,
        scan_unit_id: &ScanUnitId,
    ) -> Result<InputSnapshot, InputFactError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT relative_path,kind,len,modified_ns,content_fingerprint
                 FROM input_facts WHERE scan_unit_id=?1 ORDER BY relative_path",
            )
            .map_err(|error| InputFactError::new(format!("prepare input load: {error}")))?;
        let rows = statement
            .query_map([scan_unit_id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            })
            .map_err(|error| InputFactError::new(format!("query input facts: {error}")))?;
        let mut facts = BTreeMap::new();
        for row in rows {
            let (relative_path, kind, len, modified_ns, content_fingerprint) =
                row.map_err(|error| InputFactError::new(format!("read input fact: {error}")))?;
            let len = u64::try_from(len)
                .map_err(|_| InputFactError::new("negative stored input length"))?;
            let modified_ns = u64::try_from(modified_ns)
                .map_err(|_| InputFactError::new("negative stored input timestamp"))?;
            let input_id = InputId::new(scan_unit_id.clone(), PathBuf::from(relative_path))
                .map_err(|error| InputFactError::new(error.to_string()))?;
            facts.insert(
                input_id,
                InputSignature {
                    kind: InputKind::from_stored(kind)?,
                    len,
                    modified_ns,
                    content_fingerprint: content_fingerprint.map(|value| value as u64),
                },
            );
        }
        Ok(InputSnapshot { facts })
    }

    pub fn apply_probe(&mut self, probe: &InputProbe) -> Result<(), InputFactError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| InputFactError::new(format!("begin input update: {error}")))?;
        for change in &probe.changes {
            match change.kind {
                InputChangeKind::Removed => {
                    transaction
                        .execute(
                            "DELETE FROM input_facts WHERE scan_unit_id=?1 AND relative_path=?2",
                            params![
                                change.input_id.scan_unit_id().as_str(),
                                path_text(change.input_id.relative_path())?
                            ],
                        )
                        .map_err(|error| {
                            InputFactError::new(format!("delete input fact: {error}"))
                        })?;
                }
                InputChangeKind::Added | InputChangeKind::Modified => {
                    let signature =
                        probe.snapshot.signature(&change.input_id).ok_or_else(|| {
                            InputFactError::new("changed input is absent from new snapshot")
                        })?;
                    transaction
                        .execute(
                            "INSERT INTO input_facts(scan_unit_id,relative_path,kind,len,modified_ns,content_fingerprint)
                             VALUES (?1,?2,?3,?4,?5,?6)
                             ON CONFLICT(scan_unit_id,relative_path) DO UPDATE SET
                               kind=excluded.kind,len=excluded.len,modified_ns=excluded.modified_ns,
                               content_fingerprint=excluded.content_fingerprint",
                            params![
                                change.input_id.scan_unit_id().as_str(),
                                path_text(change.input_id.relative_path())?,
                                signature.kind.stored(),
                                i64::try_from(signature.len).map_err(|_| {
                                    InputFactError::new("input length exceeds SQLite integer")
                                })?,
                                i64::try_from(signature.modified_ns).map_err(|_| {
                                    InputFactError::new("input timestamp exceeds SQLite integer")
                                })?,
                                signature.content_fingerprint.map(|value| value as i64),
                            ],
                        )
                        .map_err(|error| {
                            InputFactError::new(format!("upsert input fact: {error}"))
                        })?;
                }
            }
        }
        transaction
            .commit()
            .map_err(|error| InputFactError::new(format!("commit input update: {error}")))
    }
}

fn signature(
    path: &Path,
    metadata: &Metadata,
    fingerprint_file: bool,
) -> Result<InputSignature, InputFactError> {
    let kind = if metadata.is_dir() {
        InputKind::Directory
    } else if metadata.is_file() {
        InputKind::File
    } else {
        return Err(InputFactError::new("unsupported input file type"));
    };
    let modified_ns = metadata
        .modified()
        .map_err(|error| InputFactError::with_io("read input modification time", error))?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| InputFactError::new("input timestamp predates Unix epoch"))?
        .as_nanos();
    Ok(InputSignature {
        kind,
        len: if kind == InputKind::File {
            metadata.len()
        } else {
            0
        },
        modified_ns: u64::try_from(modified_ns)
            .map_err(|_| InputFactError::new("input timestamp is too large"))?,
        content_fingerprint: if fingerprint_file {
            Some(content_fingerprint(path)?)
        } else {
            None
        },
    })
}

fn content_fingerprint(path: &Path) -> Result<u64, InputFactError> {
    let mut file = fs::File::open(path)
        .map_err(|error| InputFactError::with_io("open fingerprinted input", error))?;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut buffer = [0u8; 8 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| InputFactError::with_io("read fingerprinted input", error))?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Ok(hash)
}

fn path_text(path: &Path) -> Result<&str, InputFactError> {
    path.to_str()
        .ok_or_else(|| InputFactError::new("input path is not UTF-8"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputFactError {
    message: String,
}

impl InputFactError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn with_io(context: &str, error: std::io::Error) -> Self {
        Self::new(format!("{context}: {error}"))
    }
}

impl fmt::Display for InputFactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for InputFactError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct GamesOnly;

    impl InputProbePolicy for GamesOnly {
        fn descend_into(&self, relative_directory: &Path) -> bool {
            !relative_directory.starts_with("screenshots")
                && !relative_directory.starts_with("cache")
        }

        fn include_file(&self, relative_file: &Path) -> bool {
            matches!(
                relative_file.extension().and_then(|value| value.to_str()),
                Some("sfc" | "d64" | "mra")
            )
        }

        fn fingerprint_file(&self, relative_file: &Path) -> bool {
            relative_file.extension().and_then(|value| value.to_str()) == Some("mra")
        }
    }

    #[test]
    fn unchanged_probe_stats_known_files_but_skips_unchanged_subtrees() {
        let root = temporary_root("unchanged");
        write(&root.join("games/nested/one.sfc"), b"one");
        write(&root.join("screenshots/ignored.sfc"), b"ignored");
        let unit = ScanUnitId::parse("snes-root").unwrap();
        let first = probe_scan_unit(&root, &unit, &InputSnapshot::default(), &GamesOnly).unwrap();
        assert_eq!(first.statted_files, 0);
        assert_eq!(first.snapshot.facts().len(), 3);
        let second = probe_scan_unit(&root, &unit, &first.snapshot, &GamesOnly).unwrap();
        assert!(second.changes.is_empty());
        assert_eq!(second.statted_files, 1);
        assert_eq!(second.enumerated_directories, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn probe_detects_file_changes_and_new_directory_entries() {
        let root = temporary_root("changes");
        write(&root.join("games/one.sfc"), b"one");
        let unit = ScanUnitId::parse("snes-root").unwrap();
        let first = probe_scan_unit(&root, &unit, &InputSnapshot::default(), &GamesOnly).unwrap();
        write(&root.join("games/one.sfc"), b"one changed length");
        write(&root.join("games/two.sfc"), b"two");
        let second = probe_scan_unit(&root, &unit, &first.snapshot, &GamesOnly).unwrap();
        assert!(second.changes.iter().any(|change| {
            change.input_id.relative_path() == Path::new("games/one.sfc")
                && change.kind == InputChangeKind::Modified
        }));
        assert!(second.changes.iter().any(|change| {
            change.input_id.relative_path() == Path::new("games/two.sfc")
                && change.kind == InputChangeKind::Added
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_applies_only_changes_and_round_trips_snapshot() {
        let root = temporary_root("store");
        write(&root.join("games/one.sfc"), b"one");
        let unit = ScanUnitId::parse("snes-root").unwrap();
        let first = probe_scan_unit(&root, &unit, &InputSnapshot::default(), &GamesOnly).unwrap();
        let mut store = InputFactStore::open(&root.join("state/builder-state.sqlite3")).unwrap();
        store.apply_probe(&first).unwrap();
        assert_eq!(store.load_scan_unit(&unit).unwrap(), first.snapshot);

        fs::remove_file(root.join("games/one.sfc")).unwrap();
        write(&root.join("games/two.sfc"), b"two");
        let second = probe_scan_unit(&root, &unit, &first.snapshot, &GamesOnly).unwrap();
        store.apply_probe(&second).unwrap();
        assert_eq!(store.load_scan_unit(&unit).unwrap(), second.snapshot);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn policy_selected_semantic_files_receive_content_fingerprints() {
        let root = temporary_root("fingerprint");
        write(&root.join("arcade/game.mra"), b"semantic metadata");
        write(&root.join("games/game.sfc"), b"large payload");
        let unit = ScanUnitId::parse("arcade-root").unwrap();
        let probe = probe_scan_unit(&root, &unit, &InputSnapshot::default(), &GamesOnly).unwrap();
        let mra = InputId::new(unit.clone(), PathBuf::from("arcade/game.mra")).unwrap();
        let payload = InputId::new(unit, PathBuf::from("games/game.sfc")).unwrap();
        assert!(
            probe
                .snapshot
                .signature(&mra)
                .unwrap()
                .content_fingerprint
                .is_some()
        );
        assert_eq!(
            probe
                .snapshot
                .signature(&payload)
                .unwrap()
                .content_fingerprint,
            None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removing_a_directory_removes_its_retained_descendants() {
        let root = temporary_root("remove-directory");
        write(&root.join("games/nested/one.sfc"), b"one");
        write(&root.join("games/nested/two.sfc"), b"two");
        let unit = ScanUnitId::parse("snes-root").unwrap();
        let first = probe_scan_unit(&root, &unit, &InputSnapshot::default(), &GamesOnly).unwrap();
        fs::remove_dir_all(root.join("games/nested")).unwrap();
        let second = probe_scan_unit(&root, &unit, &first.snapshot, &GamesOnly).unwrap();
        assert_eq!(
            second
                .changes
                .iter()
                .filter(|change| change.kind == InputChangeKind::Removed)
                .count(),
            3
        );
        assert!(
            second
                .snapshot
                .facts()
                .keys()
                .all(|input| !input.relative_path().starts_with("games/nested"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn temporary_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mister-magik-input-facts-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
}
