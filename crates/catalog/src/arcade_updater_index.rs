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
        let bytes = std::fs::read(path)
            .map_err(|error| format!("read Arcade updater index {}: {error}", path.display()))?;
        Self::decode(&bytes)
    }

    pub fn write(&self, path: &Path) -> Result<u64, String> {
        let bytes = self.encode()?;
        std::fs::write(path, &bytes)
            .map_err(|error| format!("write Arcade updater index {}: {error}", path.display()))?;
        Ok(bytes.len() as u64)
    }

    fn validate(&self) -> Result<(), String> {
        if self.sources.is_empty() {
            return Err("Arcade updater index has no sources".to_string());
        }
        if self.sources.windows(2).any(|pair| pair[0].id >= pair[1].id) {
            return Err("Arcade updater sources are not uniquely sorted".to_string());
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
                || row.md5.len() != 32
                || !row.md5.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(format!("invalid Arcade updater row {}", row.path));
            }
        }
        Ok(())
    }
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
            sources: vec![ArcadeUpdaterSource {
                id: "distribution".to_string(),
                revision: "a".repeat(40),
                database_sha256: "b".repeat(64),
            }],
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
}
