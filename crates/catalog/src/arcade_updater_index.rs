// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Immutable Update_All Arcade metadata used as a disposable startup hint.

use crate::bounded_lz4;
use crate::mra_header::{MraHeader, PrimaryRomRequirement};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const FILE_NAME: &str = "arcade-updater-index-v1.lz4b";
pub const FORMAT: &str = "mister-magik-arcade-updater-index-v1";
const MAX_DECODED_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ArcadeUpdaterSource {
    pub id: String,
    pub revision: String,
    pub database_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ArcadeUpdaterRow {
    pub path: String,
    pub source_id: String,
    pub size: u64,
    pub md5: String,
    pub header: MraHeader,
    pub primary_rom: PrimaryRomRequirement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_metadata: Option<ArcadeUpdaterCatalogMetadata>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct ArcadeUpdaterCatalogMetadata {
    pub identity_id: String,
    pub family_id: String,
    pub title: String,
    pub year: Option<u16>,
    pub manufacturer: String,
    pub category: String,
    pub players: Option<u8>,
    pub control: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ArcadeUpdaterIndex {
    pub sources: Vec<ArcadeUpdaterSource>,
    pub rows: Vec<ArcadeUpdaterRow>,
}

#[derive(Deserialize, Serialize)]
struct StoredIndex {
    format: String,
    payload_sha256: String,
    sources: Vec<ArcadeUpdaterSource>,
    rows: Vec<ArcadeUpdaterRow>,
}

impl ArcadeUpdaterIndex {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let payload_sha256 = payload_digest(&self.sources, &self.rows)?;
        let stored = StoredIndex {
            format: FORMAT.to_string(),
            payload_sha256,
            sources: self.sources.clone(),
            rows: self.rows.clone(),
        };
        let json = serde_json::to_vec(&stored).map_err(|error| error.to_string())?;
        Ok(lz4_flex::compress_prepend_size(&json))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let json = bounded_lz4::decompress_size_prepended(
            bytes,
            MAX_DECODED_BYTES,
            "Arcade updater index",
        )?;
        let stored: StoredIndex =
            serde_json::from_slice(&json).map_err(|error| error.to_string())?;
        if stored.format != FORMAT {
            return Err(format!(
                "unsupported Arcade updater index {}",
                stored.format
            ));
        }
        let digest = payload_digest(&stored.sources, &stored.rows)?;
        if digest != stored.payload_sha256 {
            return Err("Arcade updater index payload checksum mismatch".to_string());
        }
        let index = Self {
            sources: stored.sources,
            rows: stored.rows,
        };
        index.validate()?;
        Ok(index)
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        Self::read_with_file_sha256(path).map(|(index, _)| index)
    }

    pub fn read_with_file_sha256(path: &Path) -> Result<(Self, String), String> {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("read Arcade updater index {}: {error}", path.display()))?;
        let file_sha256 = hex(&Sha256::digest(&bytes));
        Self::decode(&bytes).map(|index| (index, file_sha256))
    }

    pub fn write(&self, path: &Path) -> Result<u64, String> {
        let bytes = self.encode()?;
        std::fs::write(path, &bytes)
            .map_err(|error| format!("write Arcade updater index {}: {error}", path.display()))?;
        Ok(bytes.len() as u64)
    }

    fn validate(&self) -> Result<(), String> {
        let expected_sources = [
            "alternatives",
            "arcade-offset",
            "coinop",
            "distribution",
            "jtcores",
        ];
        if self
            .sources
            .iter()
            .map(|source| source.id.as_str())
            .ne(expected_sources)
        {
            return Err("Arcade updater index does not contain the canonical sources".to_string());
        }
        if self.sources.windows(2).any(|pair| pair[0].id >= pair[1].id) {
            return Err("Arcade updater sources are not uniquely sorted".to_string());
        }
        for source in &self.sources {
            if !is_lower_hex(&source.revision, 40) || !is_lower_hex(&source.database_sha256, 64) {
                return Err(format!("invalid Arcade updater source {}", source.id));
            }
        }
        if self
            .rows
            .windows(2)
            .any(|pair| pair[0].path >= pair[1].path)
        {
            return Err("Arcade updater rows are not uniquely sorted".to_string());
        }
        for row in &self.rows {
            if !row.path.starts_with("_Arcade/")
                || !row.path.to_ascii_lowercase().ends_with(".mra")
                || !is_lower_hex(&row.md5, 32)
                || self
                    .sources
                    .binary_search_by(|source| source.id.as_str().cmp(row.source_id.as_str()))
                    .is_err()
            {
                return Err(format!("invalid Arcade updater row {}", row.path));
            }
            if row.catalog_metadata.as_ref().is_some_and(|metadata| {
                metadata.identity_id.is_empty()
                    || metadata.family_id.is_empty()
                    || metadata.title.is_empty()
            }) {
                return Err(format!(
                    "incomplete Arcade catalog metadata for {}",
                    row.path
                ));
            }
        }
        Ok(())
    }

    pub fn enrich_catalog_metadata(&mut self, mame_path: &Path, hbmame_path: &Path) {
        let setnames = self
            .rows
            .iter()
            .filter_map(|row| row.header.setname.as_deref())
            .map(crate::library_db::normalize_id)
            .collect();
        let metadata = crate::software_identity::load_arcade_machine_metadata_for_setnames(
            mame_path,
            hbmame_path,
            &setnames,
        );
        for row in &mut self.rows {
            row.catalog_metadata = crate::software_identity::updater_arcade_catalog_metadata(
                &row.path,
                &row.header,
                &metadata,
            );
        }
    }
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn payload_digest(
    sources: &[ArcadeUpdaterSource],
    rows: &[ArcadeUpdaterRow],
) -> Result<String, String> {
    let payload = serde_json::to_vec(&(sources, rows)).map_err(|error| error.to_string())?;
    Ok(hex(&Sha256::digest(payload)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mra_header::{PrimaryRomRequirement, RomNamespace};

    fn index() -> ArcadeUpdaterIndex {
        ArcadeUpdaterIndex {
            sources: [
                "alternatives",
                "arcade-offset",
                "coinop",
                "distribution",
                "jtcores",
            ]
            .into_iter()
            .map(|id| ArcadeUpdaterSource {
                id: id.to_string(),
                revision: "a".repeat(40),
                database_sha256: "b".repeat(64),
            })
            .collect(),
            rows: vec![ArcadeUpdaterRow {
                path: "_Arcade/Puck Man.mra".to_string(),
                source_id: "distribution".to_string(),
                size: 123,
                md5: "c".repeat(32),
                header: MraHeader {
                    setname: Some("puckman".to_string()),
                    ..MraHeader::default()
                },
                primary_rom: PrimaryRomRequirement::Archive {
                    namespace: RomNamespace::Mame,
                    setname: "puckman".to_string(),
                },
                catalog_metadata: None,
            }],
        }
    }

    #[test]
    fn round_trip_is_deterministic_and_checksummed() {
        let encoded = index().encode().unwrap();
        assert_eq!(encoded, index().encode().unwrap());
        assert_eq!(ArcadeUpdaterIndex::decode(&encoded).unwrap(), index());
        let mut decoded = lz4_flex::decompress_size_prepended(&encoded).unwrap();
        let position = decoded
            .windows(7)
            .position(|window| window == b"puckman")
            .unwrap();
        decoded[position] = b'x';
        let corrupt = lz4_flex::compress_prepend_size(&decoded);
        assert!(ArcadeUpdaterIndex::decode(&corrupt).is_err());
    }

    #[test]
    fn file_identity_matches_the_encoded_sidecar() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-arcade-updater-index-{}-{}.lz4b",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let encoded = index().encode().unwrap();
        std::fs::write(&path, &encoded).unwrap();

        let (loaded, sha256) = ArcadeUpdaterIndex::read_with_file_sha256(&path).unwrap();

        let _ = std::fs::remove_file(path);
        assert_eq!(loaded, index());
        assert_eq!(sha256, hex(&Sha256::digest(encoded)));
    }
}
