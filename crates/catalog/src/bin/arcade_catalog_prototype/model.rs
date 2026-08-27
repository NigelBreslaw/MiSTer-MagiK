// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Independent input and output models for the Arcade-only prototype.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path};

const UPDATER_FORMAT: &str = "mister-magik-arcade-updater-index-v1";
const MAX_UPDATER_BYTES: usize = 16 * 1024 * 1024;
const BASE_MAGIC: [u8; 8] = *b"MAGAKB01";
const ACTIVE_MAGIC: [u8; 8] = *b"MAGAKA01";
const CONTAINER_VERSION: u32 = 1;
const CONTAINER_HEADER_BYTES: usize = 56;
const BASE_RECORD_BYTES: usize = 60;
const ACTIVE_RECORD_BYTES: usize = 36;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdaterSource {
    pub id: String,
    pub revision: String,
    pub database_sha256: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MraHeader {
    pub name: Option<String>,
    pub rbf: Option<String>,
    pub platform: Option<String>,
    pub manufacturer: Option<String>,
    pub year: Option<String>,
    pub setname: Option<String>,
    pub parent: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum RomNamespace {
    Mame,
    Hbmame,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum PrimaryRomRequirement {
    None,
    Archive {
        namespace: RomNamespace,
        setname: String,
    },
    Ambiguous,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CatalogMetadata {
    pub identity_id: String,
    pub family_id: String,
    pub title: String,
    pub year: Option<u16>,
    pub manufacturer: String,
    pub category: String,
    pub players: Option<u8>,
    pub control: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdaterRow {
    pub path: String,
    pub source_id: String,
    pub size: u64,
    pub md5: String,
    pub header: MraHeader,
    pub primary_rom: PrimaryRomRequirement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_metadata: Option<CatalogMetadata>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdaterIndex {
    pub sources: Vec<UpdaterSource>,
    pub rows: Vec<UpdaterRow>,
}

#[derive(Deserialize, Serialize)]
struct StoredUpdaterIndex {
    format: String,
    payload_sha256: String,
    sources: Vec<UpdaterSource>,
    rows: Vec<UpdaterRow>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RomRequirement {
    None,
    Mame(String),
    Hbmame(String),
    Ambiguous,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseCandidate {
    pub path: String,
    pub path_key: String,
    pub family_id: String,
    pub identity_id: String,
    pub title: String,
    pub manufacturer: String,
    pub category: String,
    pub control: String,
    pub setname: String,
    pub rom: RomRequirement,
    pub expected_size: u64,
    pub year: Option<u16>,
    pub players: Option<u8>,
    pub needs_fallback: bool,
    pub variant_score: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseCatalog {
    pub source_sha256: [u8; 32],
    pub candidates: Vec<BaseCandidate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveRecord {
    pub path: String,
    pub family_id: String,
    pub identity_id: String,
    pub title: String,
    pub manufacturer: String,
    pub category: String,
    pub control: String,
    pub year: Option<u16>,
    pub players: Option<u8>,
    pub preferred: bool,
    pub variant_ordinal: u16,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ActiveCounts {
    pub installed_mras: u32,
    pub index_hits: u32,
    pub fallbacks: u32,
    pub skipped_missing_rom: u32,
    pub skipped_ambiguous: u32,
    pub skipped_invalid: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveCatalog {
    pub source_sha256: [u8; 32],
    pub counts: ActiveCounts,
    pub records: Vec<ActiveRecord>,
}

pub fn decode_updater_index(bytes: &[u8]) -> Result<UpdaterIndex, String> {
    let declared = bytes
        .get(..4)
        .and_then(|prefix| prefix.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or("Arcade updater index is missing its size prefix")? as usize;
    if declared > MAX_UPDATER_BYTES {
        return Err(format!(
            "Arcade updater index expands to {declared} bytes; limit is {MAX_UPDATER_BYTES}"
        ));
    }
    let json = lz4_flex::decompress_size_prepended(bytes)
        .map_err(|error| format!("decompress Arcade updater index: {error}"))?;
    if json.len() != declared {
        return Err("Arcade updater index decoded size does not match its prefix".to_string());
    }
    let stored: StoredUpdaterIndex = serde_json::from_slice(&json)
        .map_err(|error| format!("decode Arcade updater index JSON: {error}"))?;
    if stored.format != UPDATER_FORMAT {
        return Err(format!(
            "unsupported Arcade updater index format {}",
            stored.format
        ));
    }
    let payload = serde_json::to_vec(&(&stored.sources, &stored.rows))
        .map_err(|error| format!("encode Arcade updater checksum payload: {error}"))?;
    if hex(&Sha256::digest(payload)) != stored.payload_sha256 {
        return Err("Arcade updater index payload checksum mismatch".to_string());
    }
    let index = UpdaterIndex {
        sources: stored.sources,
        rows: stored.rows,
    };
    validate_updater_index(&index)?;
    Ok(index)
}

pub fn encode_base(base: &BaseCatalog) -> Result<Vec<u8>, String> {
    let mut strings = StringTable::new();
    let mut records = Vec::with_capacity(base.candidates.len() * BASE_RECORD_BYTES);
    for candidate in &base.candidates {
        put_u32(&mut records, strings.add(&candidate.path)?);
        put_u32(&mut records, strings.add(&candidate.path_key)?);
        put_u32(&mut records, strings.add(&candidate.family_id)?);
        put_u32(&mut records, strings.add(&candidate.identity_id)?);
        put_u32(&mut records, strings.add(&candidate.title)?);
        put_u32(&mut records, strings.add(&candidate.manufacturer)?);
        put_u32(&mut records, strings.add(&candidate.category)?);
        put_u32(&mut records, strings.add(&candidate.control)?);
        put_u32(&mut records, strings.add(&candidate.setname)?);
        let (rom_kind, rom_setname) = match &candidate.rom {
            RomRequirement::None => (0, ""),
            RomRequirement::Mame(setname) => (1, setname.as_str()),
            RomRequirement::Hbmame(setname) => (2, setname.as_str()),
            RomRequirement::Ambiguous => (3, ""),
        };
        put_u32(&mut records, strings.add(rom_setname)?);
        put_u64(&mut records, candidate.expected_size);
        put_u16(&mut records, candidate.year.unwrap_or(0));
        records.push(candidate.players.unwrap_or(0));
        records.push(rom_kind);
        records.push(u8::from(candidate.needs_fallback));
        records.extend_from_slice(&[0; 3]);
        put_i32(&mut records, candidate.variant_score);
    }
    let mut payload = Vec::with_capacity(40 + records.len() + strings.bytes.len());
    payload.extend_from_slice(&base.source_sha256);
    put_len_u32(&mut payload, base.candidates.len(), "base candidate count")?;
    put_len_u32(&mut payload, strings.bytes.len(), "base string table")?;
    payload.extend_from_slice(&records);
    payload.extend_from_slice(&strings.bytes);
    Ok(encode_container(BASE_MAGIC, &payload))
}

pub fn decode_base(bytes: &[u8]) -> Result<BaseCatalog, String> {
    let payload = decode_container(bytes, BASE_MAGIC, "Arcade base")?;
    if payload.len() < 40 {
        return Err("Arcade base payload is truncated".to_string());
    }
    let source_sha256 = payload[..32].try_into().expect("fixed-size source digest");
    let count = read_u32(payload, 32)? as usize;
    let string_bytes = read_u32(payload, 36)? as usize;
    let records_end = 40usize
        .checked_add(
            count
                .checked_mul(BASE_RECORD_BYTES)
                .ok_or("Arcade base record size overflow")?,
        )
        .ok_or("Arcade base payload size overflow")?;
    let expected_len = records_end
        .checked_add(string_bytes)
        .ok_or("Arcade base string table size overflow")?;
    if payload.len() != expected_len {
        return Err("Arcade base payload length is inconsistent".to_string());
    }
    let strings = &payload[records_end..];
    let mut candidates = Vec::with_capacity(count);
    let (records, remainder) = payload[40..records_end].as_chunks::<BASE_RECORD_BYTES>();
    debug_assert!(remainder.is_empty());
    for record in records {
        let rom_setname = read_string(strings, read_u32(record, 36)?)?;
        let rom = match record[51] {
            0 => RomRequirement::None,
            1 => RomRequirement::Mame(rom_setname),
            2 => RomRequirement::Hbmame(rom_setname),
            3 => RomRequirement::Ambiguous,
            other => return Err(format!("Arcade base has invalid ROM kind {other}")),
        };
        candidates.push(BaseCandidate {
            path: read_string(strings, read_u32(record, 0)?)?,
            path_key: read_string(strings, read_u32(record, 4)?)?,
            family_id: read_string(strings, read_u32(record, 8)?)?,
            identity_id: read_string(strings, read_u32(record, 12)?)?,
            title: read_string(strings, read_u32(record, 16)?)?,
            manufacturer: read_string(strings, read_u32(record, 20)?)?,
            category: read_string(strings, read_u32(record, 24)?)?,
            control: read_string(strings, read_u32(record, 28)?)?,
            setname: read_string(strings, read_u32(record, 32)?)?,
            rom,
            expected_size: read_u64(record, 40)?,
            year: nonzero_u16(read_u16(record, 48)?),
            players: nonzero_u8(record[50]),
            needs_fallback: record[52] != 0,
            variant_score: read_i32(record, 56)?,
        });
    }
    if candidates
        .windows(2)
        .any(|pair| pair[0].path_key >= pair[1].path_key)
    {
        return Err("Arcade base paths are not uniquely sorted".to_string());
    }
    Ok(BaseCatalog {
        source_sha256,
        candidates,
    })
}

pub fn encode_active(active: &ActiveCatalog) -> Result<Vec<u8>, String> {
    let mut strings = StringTable::new();
    let mut records = Vec::with_capacity(active.records.len() * ACTIVE_RECORD_BYTES);
    for record in &active.records {
        for value in [
            &record.path,
            &record.family_id,
            &record.identity_id,
            &record.title,
            &record.manufacturer,
            &record.category,
            &record.control,
        ] {
            put_u32(&mut records, strings.add(value)?);
        }
        put_u16(&mut records, record.year.unwrap_or(0));
        records.push(record.players.unwrap_or(0));
        records.push(u8::from(record.preferred));
        put_u16(&mut records, record.variant_ordinal);
        put_u16(&mut records, 0);
    }
    let preferred = active
        .records
        .iter()
        .filter(|record| record.preferred)
        .count();
    let mut payload = Vec::with_capacity(64 + records.len() + strings.bytes.len());
    payload.extend_from_slice(&active.source_sha256);
    put_len_u32(&mut payload, active.records.len(), "active record count")?;
    put_len_u32(&mut payload, preferred, "active preferred count")?;
    put_u32(&mut payload, active.counts.installed_mras);
    put_u32(&mut payload, active.counts.index_hits);
    put_u32(&mut payload, active.counts.fallbacks);
    put_u32(&mut payload, active.counts.skipped_missing_rom);
    put_u32(&mut payload, active.counts.skipped_ambiguous);
    put_u32(&mut payload, active.counts.skipped_invalid);
    put_len_u32(&mut payload, strings.bytes.len(), "active string table")?;
    payload.extend_from_slice(&records);
    payload.extend_from_slice(&strings.bytes);
    Ok(encode_container(ACTIVE_MAGIC, &payload))
}

pub fn decode_active(bytes: &[u8]) -> Result<ActiveCatalog, String> {
    let payload = decode_container(bytes, ACTIVE_MAGIC, "Arcade active")?;
    if payload.len() < 68 {
        return Err("Arcade active payload is truncated".to_string());
    }
    let source_sha256 = payload[..32].try_into().expect("fixed-size source digest");
    let count = read_u32(payload, 32)? as usize;
    let preferred_count = read_u32(payload, 36)? as usize;
    let counts = ActiveCounts {
        installed_mras: read_u32(payload, 40)?,
        index_hits: read_u32(payload, 44)?,
        fallbacks: read_u32(payload, 48)?,
        skipped_missing_rom: read_u32(payload, 52)?,
        skipped_ambiguous: read_u32(payload, 56)?,
        skipped_invalid: read_u32(payload, 60)?,
    };
    let string_bytes = read_u32(payload, 64)? as usize;
    let records_start = 68usize;
    let records_end = records_start
        .checked_add(
            count
                .checked_mul(ACTIVE_RECORD_BYTES)
                .ok_or("Arcade active record size overflow")?,
        )
        .ok_or("Arcade active payload size overflow")?;
    let expected_len = records_end
        .checked_add(string_bytes)
        .ok_or("Arcade active string table size overflow")?;
    if payload.len() != expected_len {
        return Err("Arcade active payload length is inconsistent".to_string());
    }
    let strings = &payload[records_end..];
    let mut records = Vec::with_capacity(count);
    let (active_records, remainder) =
        payload[records_start..records_end].as_chunks::<ACTIVE_RECORD_BYTES>();
    debug_assert!(remainder.is_empty());
    for record in active_records {
        records.push(ActiveRecord {
            path: read_string(strings, read_u32(record, 0)?)?,
            family_id: read_string(strings, read_u32(record, 4)?)?,
            identity_id: read_string(strings, read_u32(record, 8)?)?,
            title: read_string(strings, read_u32(record, 12)?)?,
            manufacturer: read_string(strings, read_u32(record, 16)?)?,
            category: read_string(strings, read_u32(record, 20)?)?,
            control: read_string(strings, read_u32(record, 24)?)?,
            year: nonzero_u16(read_u16(record, 28)?),
            players: nonzero_u8(record[30]),
            preferred: record[31] != 0,
            variant_ordinal: read_u16(record, 32)?,
        });
    }
    if records.iter().filter(|record| record.preferred).count() != preferred_count {
        return Err("Arcade active preferred count is inconsistent".to_string());
    }
    Ok(ActiveCatalog {
        source_sha256,
        counts,
        records,
    })
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn validate_updater_index(index: &UpdaterIndex) -> Result<(), String> {
    let expected_sources = [
        "alternatives",
        "arcade-offset",
        "coinop",
        "distribution",
        "jtcores",
    ];
    if index
        .sources
        .iter()
        .map(|source| source.id.as_str())
        .ne(expected_sources)
    {
        return Err("Arcade updater index does not contain the canonical sources".to_string());
    }
    for source in &index.sources {
        if !is_lower_hex(&source.revision, 40) || !is_lower_hex(&source.database_sha256, 64) {
            return Err(format!("invalid Arcade updater source {}", source.id));
        }
    }
    if index
        .rows
        .windows(2)
        .any(|pair| pair[0].path >= pair[1].path)
    {
        return Err("Arcade updater rows are not uniquely sorted".to_string());
    }
    for row in &index.rows {
        if !is_safe_arcade_mra_path(&row.path)
            || !is_lower_hex(&row.md5, 32)
            || index
                .sources
                .binary_search_by(|source| source.id.as_str().cmp(row.source_id.as_str()))
                .is_err()
        {
            return Err(format!("invalid Arcade updater row {}", row.path));
        }
    }
    Ok(())
}

fn is_safe_arcade_mra_path(value: &str) -> bool {
    if value.as_bytes().contains(&0)
        || value.contains('\\')
        || value
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return false;
    }
    let path = Path::new(value);
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mra"))
    {
        return false;
    }
    let mut components = path.components();
    if !matches!(
        components.next(),
        Some(Component::Normal(component)) if component == "_Arcade"
    ) {
        return false;
    }
    let mut suffix_components = 0usize;
    for component in components {
        if !matches!(component, Component::Normal(_)) {
            return false;
        }
        suffix_components += 1;
    }
    suffix_components > 0
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn encode_container(magic: [u8; 8], payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(CONTAINER_HEADER_BYTES + payload.len());
    bytes.extend_from_slice(&magic);
    put_u32(&mut bytes, CONTAINER_VERSION);
    put_u32(&mut bytes, CONTAINER_HEADER_BYTES as u32);
    put_u64(&mut bytes, payload.len() as u64);
    bytes.extend_from_slice(&sha256(payload));
    bytes.extend_from_slice(payload);
    bytes
}

fn decode_container<'a>(bytes: &'a [u8], magic: [u8; 8], label: &str) -> Result<&'a [u8], String> {
    if bytes.len() < CONTAINER_HEADER_BYTES || bytes[..8] != magic {
        return Err(format!("{label} has an invalid header"));
    }
    if read_u32(bytes, 8)? != CONTAINER_VERSION
        || read_u32(bytes, 12)? as usize != CONTAINER_HEADER_BYTES
    {
        return Err(format!("{label} has an unsupported version"));
    }
    let payload_len = usize::try_from(read_u64(bytes, 16)?)
        .map_err(|_| format!("{label} payload is too large"))?;
    if bytes.len() != CONTAINER_HEADER_BYTES.saturating_add(payload_len) {
        return Err(format!("{label} length is inconsistent"));
    }
    let payload = &bytes[CONTAINER_HEADER_BYTES..];
    if sha256(payload).as_slice() != &bytes[24..56] {
        return Err(format!("{label} checksum mismatch"));
    }
    Ok(payload)
}

struct StringTable {
    offsets: BTreeMap<String, u32>,
    bytes: Vec<u8>,
}

impl StringTable {
    fn new() -> Self {
        let mut offsets = BTreeMap::new();
        offsets.insert(String::new(), 0);
        Self {
            offsets,
            bytes: vec![0],
        }
    }

    fn add(&mut self, value: &str) -> Result<u32, String> {
        if let Some(offset) = self.offsets.get(value) {
            return Ok(*offset);
        }
        if value.as_bytes().contains(&0) {
            return Err("Arcade catalog strings cannot contain NUL bytes".to_string());
        }
        let offset = u32::try_from(self.bytes.len())
            .map_err(|_| "Arcade catalog string table exceeds 4 GiB".to_string())?;
        self.bytes.extend_from_slice(value.as_bytes());
        self.bytes.push(0);
        self.offsets.insert(value.to_string(), offset);
        Ok(offset)
    }
}

fn read_string(strings: &[u8], offset: u32) -> Result<String, String> {
    let offset = offset as usize;
    let tail = strings
        .get(offset..)
        .ok_or("Arcade catalog string offset is out of bounds")?;
    let end = tail
        .iter()
        .position(|byte| *byte == 0)
        .ok_or("Arcade catalog string is not terminated")?;
    std::str::from_utf8(&tail[..end])
        .map(str::to_owned)
        .map_err(|error| format!("Arcade catalog string is not UTF-8: {error}"))
}

fn put_len_u32(out: &mut Vec<u8>, value: usize, label: &str) -> Result<(), String> {
    let value = u32::try_from(value).map_err(|_| format!("{label} exceeds u32"))?;
    put_u32(out, value);
    Ok(())
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    bytes
        .get(offset..offset + 2)
        .and_then(|value| value.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| "Arcade catalog integer is out of bounds".to_string())
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    bytes
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| "Arcade catalog integer is out of bounds".to_string())
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, String> {
    bytes
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(i32::from_le_bytes)
        .ok_or_else(|| "Arcade catalog integer is out of bounds".to_string())
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, String> {
    bytes
        .get(offset..offset + 8)
        .and_then(|value| value.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| "Arcade catalog integer is out of bounds".to_string())
}

fn nonzero_u16(value: u16) -> Option<u16> {
    (value != 0).then_some(value)
}

fn nonzero_u8(value: u8) -> Option<u8> {
    (value != 0).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> BaseCandidate {
        BaseCandidate {
            path: "_Arcade/Puck Man.mra".to_string(),
            path_key: "_arcade/puck man.mra".to_string(),
            family_id: "puckman".to_string(),
            identity_id: "puckman".to_string(),
            title: "Puck Man".to_string(),
            manufacturer: "Namco".to_string(),
            category: "Maze".to_string(),
            control: "Joystick".to_string(),
            setname: "puckman".to_string(),
            rom: RomRequirement::Mame("puckman".to_string()),
            expected_size: 123,
            year: Some(1980),
            players: Some(2),
            needs_fallback: false,
            variant_score: 100,
        }
    }

    #[test]
    fn base_round_trip_is_deterministic() {
        let base = BaseCatalog {
            source_sha256: [7; 32],
            candidates: vec![candidate()],
        };
        let encoded = encode_base(&base).unwrap();
        assert_eq!(encoded, encode_base(&base).unwrap());
        assert_eq!(decode_base(&encoded).unwrap(), base);
    }

    #[test]
    fn active_round_trip_preserves_metrics() {
        let active = ActiveCatalog {
            source_sha256: [9; 32],
            counts: ActiveCounts {
                installed_mras: 1,
                index_hits: 1,
                ..ActiveCounts::default()
            },
            records: vec![ActiveRecord {
                path: "/media/fat/_Arcade/Puck Man.mra".to_string(),
                family_id: "puckman".to_string(),
                identity_id: "puckman".to_string(),
                title: "Puck Man".to_string(),
                manufacturer: "Namco".to_string(),
                category: "Maze".to_string(),
                control: "Joystick".to_string(),
                year: Some(1980),
                players: Some(2),
                preferred: true,
                variant_ordinal: 0,
            }],
        };
        assert_eq!(
            decode_active(&encode_active(&active).unwrap()).unwrap(),
            active
        );
    }

    #[test]
    fn corrupt_container_is_rejected() {
        let base = BaseCatalog {
            source_sha256: [0; 32],
            candidates: vec![candidate()],
        };
        let mut encoded = encode_base(&base).unwrap();
        let last = encoded.len() - 1;
        encoded[last] ^= 1;
        assert!(decode_base(&encoded).is_err());
    }

    #[test]
    fn updater_paths_cannot_escape_arcade_root() {
        assert!(is_safe_arcade_mra_path("_Arcade/Puck Man.mra"));
        assert!(is_safe_arcade_mra_path(
            "_Arcade/_alternatives/Puck Man.mra"
        ));
        assert!(!is_safe_arcade_mra_path("_Arcade/../../games/Puck Man.mra"));
        assert!(!is_safe_arcade_mra_path("_Arcade/./Puck Man.mra"));
        assert!(!is_safe_arcade_mra_path("/_Arcade/Puck Man.mra"));
        assert!(!is_safe_arcade_mra_path("_Arcade\\Puck Man.mra"));
        assert!(!is_safe_arcade_mra_path("_Arcade/readme.txt"));
    }
}
