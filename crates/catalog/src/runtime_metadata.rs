// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compact, read-only runtime metadata used by the MagiK catalog and media
//! workers.  The file is intentionally independent from SQLite: opening it
//! reads a small header and index, while individual LZ4-compressed shards are
//! fetched only when requested.

#![allow(clippy::chunks_exact_to_as_chunks)]

use std::collections::BTreeMap;
#[cfg(feature = "builder")]
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
#[cfg(not(unix))]
use std::io::Read;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use lz4_flex::{block, compress_prepend_size};
use sha2::{Digest, Sha256};

pub const FILE_NAME: &str = "magik-metadata-v1.bin";
pub const FORMAT: &str = "mister-magik-runtime-metadata-v1";
pub const VERSION: u32 = 1;
pub const HEADER_LEN: usize = 96;
pub const INDEX_ENTRY_LEN: usize = 128;
pub const MAX_DECODED_SHARD_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_COMPRESSED_SHARD_BYTES: usize = 16 * 1024 * 1024;
pub const SOFTWARE_KIND: u8 = 0;
pub const ARCADE_KIND: u8 = 1;

const HEADER_MAGIC: &[u8; 8] = b"MMMETA1\0";
const SOFTWARE_MAGIC: &[u8; 4] = b"SWM1";
const ARCADE_MAGIC: &[u8; 4] = b"ARC1";
const SOFTWARE_HEADER_LEN: usize = 36;
const ARCADE_HEADER_LEN: usize = 32;
const SOFTWARE_ITEM_LEN: usize = 32;
const SOFTWARE_TITLE_LEN: usize = 12;
const SOFTWARE_HASH_LEN: usize = 20;
const SOFTWARE_DISK_LEN: usize = 28;
const ARCADE_MACHINE_LEN: usize = 32;
const ARCADE_MISTER_LEN: usize = 32;
const ARCADE_KEY_LEN: usize = 8;
static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The mapped source lists consumed by MagiK.  The tuple is platform id,
/// canonical list namespace, and all MAME source lists collapsed into it.
pub const RUNTIME_SOFTWARE_SYSTEMS: &[(&str, &str, &[&str])] = &[
    ("nes", "nes", &["nes"]),
    ("fds", "fds", &["famicom_flop"]),
    ("snes", "snes", &["snes"]),
    ("n64", "n64", &["n64"]),
    ("sms", "sms", &["sms"]),
    ("megadrive", "megadriv", &["megadriv"]),
    ("s32x", "32x", &["32x"]),
    ("megacd", "megacd", &["megacd"]),
    ("saturn", "saturn", &["saturn"]),
    ("amigacd32", "amigacd32", &["cd32"]),
    ("atarilynx", "lynx", &["lynx"]),
    ("acornatom", "atom", &["atom_cass", "atom_flop", "atom_rom"]),
    (
        "acornelectron",
        "electron",
        &["electron_cass", "electron_flop", "electron_rom"],
    ),
    (
        "bbcmicro",
        "bbc",
        &[
            "bbc_cass",
            "bbc_flop_32016",
            "bbc_flop_6502",
            "bbc_flop_68000",
            "bbc_flop_80186",
            "bbc_flop_arm",
            "bbc_flop_hybrid",
            "bbc_flop_torch",
            "bbc_flop_z80",
            "bbc_hdd",
            "bbc_rom",
            "bbcb_flop",
            "bbcb_flop_orig",
            "bbcm_cart",
            "bbcm_flop",
        ],
    ),
    (
        "archie",
        "archimedes",
        &["archimedes", "archimedes_hdd", "archimedes_rom"],
    ),
    (
        "apple-ii",
        "apple2",
        &[
            "apple2_cass",
            "apple2_flop_clcracked",
            "apple2_flop_misc",
            "apple2_flop_orig",
            "apple2_rom",
        ],
    ),
    (
        "apple-iigs",
        "apple2gs",
        &[
            "apple2gs_flop_clcracked",
            "apple2gs_flop_misc",
            "apple2gs_flop_orig",
        ],
    ),
    ("amstrad", "amstrad", &["cpc_cass", "cpc_flop", "gx4000"]),
    ("atari2600", "a2600", &["a2600", "a2600_cass"]),
    ("atari5200", "a5200", &["a5200"]),
    ("atari7800", "a7800", &["a7800"]),
    (
        "atari800",
        "a800",
        &["a800", "a800_cass", "a800_flop", "xegs"],
    ),
    (
        "atarist",
        "atarist",
        &["st_cart", "st_flop", "st_flop_demos"],
    ),
    (
        "c64",
        "c64",
        &[
            "c64_cart",
            "c64_cass",
            "c64_flop_misc",
            "c64_flop_orig",
            "c64_quik",
        ],
    ),
    ("c128", "c128", &["c128_cart", "c128_flop", "c128_rom"]),
    (
        "c16",
        "c16",
        &["plus4_cart", "plus4_cass", "plus4_flop", "plus4_quik"],
    ),
    (
        "pet2001",
        "pet",
        &["pet_cass", "pet_flop", "pet_hdd", "pet_quik"],
    ),
    (
        "vic20",
        "vic20",
        &["vic1001_cart", "vic1001_cass", "vic1001_flop"],
    ),
    ("colecovision", "coleco", &["coleco", "coleco_homebrew"]),
    ("megaduck", "megaduck", &["megaduck"]),
    ("wonderswan", "wonderswan", &["wswan"]),
    ("wonderswancolor", "wsc", &["wscolor"]),
    ("x68000", "x68000", &["x68k_flop"]),
    (
        "zx-spectrum",
        "spectrum",
        &[
            "spectrum_cart",
            "spectrum_cass",
            "spectrum_flop_opus",
            "spectrum_mgt_flop",
            "spectrum_microdrive",
            "spectrum_wafadrive",
        ],
    ),
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SoftwareItem {
    pub name: String,
    pub parent_name: Option<String>,
    pub description: String,
    pub year: Option<String>,
    pub publisher: Option<String>,
    pub region: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SoftwareHashCandidate {
    pub size: u64,
    pub crc32: u32,
    pub software_names: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SoftwareDiskCandidate {
    pub sha1: String,
    pub software_names: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SoftwareShard {
    pub items: Vec<SoftwareItem>,
    pub title_candidates: BTreeMap<String, Vec<String>>,
    pub hash_candidates: Vec<SoftwareHashCandidate>,
    pub disk_candidates: Vec<SoftwareDiskCandidate>,
}

impl SoftwareShard {
    pub fn item(&self, name: &str) -> Option<&SoftwareItem> {
        self.items
            .binary_search_by(|item| item.name.as_str().cmp(name))
            .ok()
            .map(|index| &self.items[index])
    }

    pub fn title_candidates(&self, normalized_title: &str) -> &[String] {
        self.title_candidates
            .get(normalized_title)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn hash_candidates(&self, size: u64, crc32: u32) -> &[String] {
        self.hash_candidates
            .binary_search_by(|candidate| (candidate.size, candidate.crc32).cmp(&(size, crc32)))
            .ok()
            .map(|index| self.hash_candidates[index].software_names.as_slice())
            .unwrap_or(&[])
    }

    pub fn disk_candidates(&self, sha1: &str) -> &[String] {
        self.disk_candidates
            .binary_search_by(|candidate| candidate.sha1.as_str().cmp(sha1))
            .ok()
            .map(|index| self.disk_candidates[index].software_names.as_slice())
            .unwrap_or(&[])
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArcadeMachine {
    pub setname: String,
    pub parent_setname: Option<String>,
    pub title: String,
    pub year: Option<String>,
    pub manufacturer: Option<String>,
    pub players: Option<u8>,
    pub control: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MisterArcadeEntry {
    pub setname_key: String,
    pub mra_name_key: String,
    pub title: String,
    pub category: String,
    pub year: Option<u16>,
    pub manufacturer: String,
    pub players: Option<u8>,
    pub control: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArcadeShard {
    pub mame: Vec<ArcadeMachine>,
    pub hbmame: Vec<ArcadeMachine>,
    pub mister: Vec<MisterArcadeEntry>,
}

impl ArcadeShard {
    pub fn machine(&self, hbmame: bool, setname: &str) -> Option<&ArcadeMachine> {
        let rows = if hbmame { &self.hbmame } else { &self.mame };
        rows.binary_search_by(|row| row.setname.as_str().cmp(setname))
            .ok()
            .map(|index| &rows[index])
    }

    pub fn mister_by_setname(&self, key: &str) -> Option<&MisterArcadeEntry> {
        self.mister
            .binary_search_by(|row| row.setname_key.as_str().cmp(key))
            .ok()
            .map(|index| &self.mister[index])
    }

    pub fn mister_by_mra_name(&self, key: &str) -> Option<&MisterArcadeEntry> {
        self.mister.iter().find(|row| row.mra_name_key == key)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataStatus {
    pub format: &'static str,
    pub shard_count: usize,
    pub file_len: u64,
}

#[derive(Clone, Debug)]
struct IndexEntry {
    id_name: String,
    kind: u8,
    compressed_offset: u64,
    compressed_len: u32,
    decoded_len: u32,
    digest: [u8; 32],
    item_rows: u32,
    hash_rows: u32,
    aux_rows: u32,
}

pub struct MetadataStore {
    file: File,
    entries: Vec<IndexEntry>,
    status: MetadataStatus,
}

impl MetadataStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path)
            .map_err(|error| format!("open metadata {}: {error}", path.display()))?;
        Self::from_file(file)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let fixture_id = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mister-magik-metadata-{}-{fixture_id}",
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .read(true)
            .open(&path)
            .map_err(|error| format!("open metadata fixture: {error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("write metadata fixture: {error}"))?;
        file.flush()
            .map_err(|error| format!("flush metadata fixture: {error}"))?;
        let result = Self::from_file(file);
        let _ = std::fs::remove_file(path);
        result
    }

    fn from_file(file: File) -> Result<Self, String> {
        let file_len = file
            .metadata()
            .map_err(|error| format!("stat metadata: {error}"))?
            .len();
        if file_len < HEADER_LEN as u64 {
            return Err("metadata header is truncated".into());
        }
        let mut header = [0u8; HEADER_LEN];
        read_at(&file, &mut header, 0)?;
        if &header[..8] != HEADER_MAGIC {
            return Err("metadata magic is invalid".into());
        }
        let version = u32_at(&header[8..12]);
        if version != VERSION {
            return Err(format!("metadata version {version} is unsupported"));
        }
        if header[12..16].iter().any(|byte| *byte != 0) {
            return Err("metadata header flags are unsupported".into());
        }
        let declared_len = u64_at(&header[16..24]);
        if declared_len != file_len {
            return Err(format!(
                "metadata length mismatch declared={declared_len} actual={file_len}"
            ));
        }
        let shard_count = usize::try_from(u32_at(&header[24..28]))
            .map_err(|_| "metadata shard count exceeds platform size")?;
        let index_offset = u64_at(&header[28..36]);
        let index_entry_size = usize::try_from(u32_at(&header[36..40]))
            .map_err(|_| "metadata index entry size exceeds platform size")?;
        let index_len = usize::try_from(u32_at(&header[40..44]))
            .map_err(|_| "metadata index length exceeds platform size")?;
        if index_offset != HEADER_LEN as u64 || index_entry_size != INDEX_ENTRY_LEN {
            return Err("metadata index geometry is invalid".into());
        }
        let expected_index_len = shard_count
            .checked_mul(INDEX_ENTRY_LEN)
            .ok_or_else(|| "metadata index length overflows".to_string())?;
        if index_len != expected_index_len
            || index_offset
                .checked_add(index_len as u64)
                .ok_or_else(|| "metadata index end overflows".to_string())?
                > file_len
        {
            return Err("metadata index bounds are invalid".into());
        }
        let mut index = vec![0u8; index_len];
        read_at(&file, &mut index, index_offset)?;
        let digest = Sha256::digest(&index);
        if digest.as_slice() != &header[44..76] {
            return Err("metadata index checksum mismatch".into());
        }
        if header[76..].iter().any(|byte| *byte != 0) {
            return Err("metadata header reserved bytes are nonzero".into());
        }
        let mut entries = Vec::with_capacity(shard_count);
        let mut previous_id = None;
        let mut ranges = Vec::with_capacity(shard_count);
        for chunk in index.chunks_exact(INDEX_ENTRY_LEN) {
            let mut id = [0u8; 32];
            id.copy_from_slice(&chunk[..32]);
            let id_name = id_string(&id)?;
            if previous_id
                .as_ref()
                .is_some_and(|previous: &[u8; 32]| previous >= &id)
            {
                return Err("metadata index is not strictly sorted".into());
            }
            previous_id = Some(id);
            let kind = chunk[32];
            if kind != SOFTWARE_KIND && kind != ARCADE_KIND {
                return Err(format!("metadata shard kind {kind} is invalid"));
            }
            let offset = u64_at(&chunk[40..48]);
            let compressed_len = u32_at(&chunk[48..52]);
            let decoded_len = u32_at(&chunk[52..56]);
            if compressed_len == 0
                || decoded_len == 0
                || decoded_len as usize > MAX_DECODED_SHARD_BYTES
            {
                return Err("metadata shard lengths are invalid".into());
            }
            let end = offset
                .checked_add(compressed_len as u64)
                .ok_or_else(|| "metadata shard end overflows".to_string())?;
            if offset < index_offset + index_len as u64 || end > file_len {
                return Err("metadata shard is outside file bounds".into());
            }
            let mut shard_digest = [0u8; 32];
            shard_digest.copy_from_slice(&chunk[56..88]);
            if chunk[100..].iter().any(|byte| *byte != 0) {
                return Err("metadata index reserved bytes are nonzero".into());
            }
            ranges.push((offset, end));
            entries.push(IndexEntry {
                id_name,
                kind,
                compressed_offset: offset,
                compressed_len,
                decoded_len,
                digest: shard_digest,
                item_rows: u32_at(&chunk[88..92]),
                hash_rows: u32_at(&chunk[92..96]),
                aux_rows: u32_at(&chunk[96..100]),
            });
        }
        ranges.sort_unstable();
        if ranges.windows(2).any(|window| window[0].1 > window[1].0) {
            return Err("metadata shards overlap".into());
        }
        Ok(Self {
            file,
            entries,
            status: MetadataStatus {
                format: FORMAT,
                shard_count,
                file_len,
            },
        })
    }

    pub fn status(&self) -> &MetadataStatus {
        &self.status
    }

    pub fn shard_ids(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.id_name.as_str())
    }

    pub fn shard_digest(&self, id: &str) -> Option<[u8; 32]> {
        self.entry(id).map(|entry| entry.digest)
    }

    pub fn shard_row_counts(&self, id: &str) -> Option<(u32, u32, u32)> {
        self.entry(id)
            .map(|entry| (entry.item_rows, entry.hash_rows, entry.aux_rows))
    }

    pub fn software_shard(&self, system_id: &str) -> Result<Option<SoftwareShard>, String> {
        let Some(entry) = self.entry(system_id) else {
            return Ok(None);
        };
        if entry.kind != SOFTWARE_KIND {
            return Err(format!("metadata shard {system_id} is not software"));
        }
        let payload = self.read_shard(entry)?;
        decode_software(&payload).map(Some)
    }

    pub fn arcade_shard(&self) -> Result<Option<ArcadeShard>, String> {
        let Some(entry) = self.entries.iter().find(|entry| entry.kind == ARCADE_KIND) else {
            return Ok(None);
        };
        let payload = self.read_shard(entry)?;
        decode_arcade(&payload).map(Some)
    }

    fn entry(&self, id: &str) -> Option<&IndexEntry> {
        self.entries
            .binary_search_by(|entry| entry.id_name.as_str().cmp(id))
            .ok()
            .map(|index| &self.entries[index])
    }

    fn read_shard(&self, entry: &IndexEntry) -> Result<Vec<u8>, String> {
        if entry.compressed_len as usize > MAX_COMPRESSED_SHARD_BYTES {
            return Err("metadata compressed shard exceeds maximum".into());
        }
        let mut compressed = vec![0u8; entry.compressed_len as usize];
        read_at(&self.file, &mut compressed, entry.compressed_offset)?;
        let decoded = block::decompress_size_prepended(&compressed)
            .map_err(|error| format!("decompress metadata shard {}: {error}", entry.id_name))?;
        if decoded.len() != entry.decoded_len as usize || decoded.len() > MAX_DECODED_SHARD_BYTES {
            return Err(format!(
                "metadata shard {} decoded length mismatch",
                entry.id_name
            ));
        }
        let digest = Sha256::digest(&decoded);
        if digest.as_slice() != entry.digest {
            return Err(format!(
                "metadata shard {} checksum mismatch",
                entry.id_name
            ));
        }
        Ok(decoded)
    }
}

fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut done = 0;
        while done < buffer.len() {
            let count = file
                .read_at(&mut buffer[done..], offset + done as u64)
                .map_err(|error| format!("read metadata at {offset}: {error}"))?;
            if count == 0 {
                return Err("metadata read reached EOF".into());
            }
            done += count;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let mut clone = file
            .try_clone()
            .map_err(|error| format!("clone metadata: {error}"))?;
        use std::io::Seek;
        clone
            .seek(std::io::SeekFrom::Start(offset))
            .map_err(|error| format!("seek metadata: {error}"))?;
        clone
            .read_exact(buffer)
            .map_err(|error| format!("read metadata: {error}"))
    }
}

fn id_bytes(id: &str) -> Result<[u8; 32], String> {
    if id.is_empty() || id.len() > 32 || !id.is_ascii() {
        return Err(format!("metadata shard id {id:?} is invalid"));
    }
    let mut bytes = [0u8; 32];
    bytes[..id.len()].copy_from_slice(id.as_bytes());
    Ok(bytes)
}

fn id_string(id: &[u8; 32]) -> Result<String, String> {
    let end = id.iter().position(|byte| *byte == 0).unwrap_or(id.len());
    if id[end..].iter().any(|byte| *byte != 0) {
        return Err("metadata shard id padding is invalid".into());
    }
    let value = std::str::from_utf8(&id[..end]).map_err(|_| "metadata shard id is not UTF-8")?;
    if value.is_empty() || value.len() > 32 {
        return Err("metadata shard id is empty or too long".into());
    }
    Ok(value.to_string())
}

fn u32_at(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("fixed metadata integer"))
}

fn u64_at(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("fixed metadata integer"))
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_string(out: &mut Vec<String>, value: Option<&str>) -> u32 {
    let Some(value) = value else { return 0 };
    if value.is_empty() {
        return 0;
    }
    let value = value.to_string();
    if let Some(index) = out.iter().position(|existing| existing == &value) {
        return (index + 1) as u32;
    }
    out.push(value);
    out.len() as u32
}

fn string_value(strings: &[String], index: u32) -> Result<Option<String>, String> {
    if index == 0 {
        return Ok(None);
    }
    strings
        .get(index as usize - 1)
        .cloned()
        .map(Some)
        .ok_or_else(|| "metadata string index is invalid".into())
}

fn required_string(strings: &[String], index: u32, what: &str) -> Result<String, String> {
    if index == 0 {
        return Err(format!("metadata {what} string index is invalid"));
    }
    strings
        .get(index as usize - 1)
        .cloned()
        .ok_or_else(|| format!("metadata {what} string index is invalid"))
}

fn decode_strings(
    payload: &[u8],
    count: u32,
    offset: usize,
    length: usize,
) -> Result<Vec<String>, String> {
    let count =
        usize::try_from(count).map_err(|_| "metadata string count exceeds platform size")?;
    let offsets_len = count
        .checked_add(1)
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "metadata string offsets overflow".to_string())?;
    if offset
        .checked_add(offsets_len)
        .and_then(|end| end.checked_add(length))
        .is_none()
        || offset + offsets_len + length > payload.len()
    {
        return Err("metadata string table is outside shard bounds".into());
    }
    let offsets = &payload[offset..offset + offsets_len];
    let bytes = &payload[offset + offsets_len..offset + offsets_len + length];
    let mut strings = Vec::with_capacity(count);
    let mut previous = 0usize;
    for index in 0..count {
        let start = u32_at(&offsets[index * 4..index * 4 + 4]) as usize;
        let end = u32_at(&offsets[(index + 1) * 4..(index + 2) * 4]) as usize;
        if start < previous || end < start || end > bytes.len() {
            return Err("metadata string offsets are invalid".into());
        }
        let value =
            std::str::from_utf8(&bytes[start..end]).map_err(|_| "metadata string is not UTF-8")?;
        strings.push(value.to_string());
        previous = end;
    }
    if previous != bytes.len() {
        return Err("metadata string table has trailing bytes".into());
    }
    Ok(strings)
}

fn decode_software(payload: &[u8]) -> Result<SoftwareShard, String> {
    if payload.len() < SOFTWARE_HEADER_LEN || &payload[..4] != SOFTWARE_MAGIC {
        return Err("software metadata shard header is invalid".into());
    }
    if u32_at(&payload[4..8]) != VERSION {
        return Err("software metadata shard version is invalid".into());
    }
    let string_count = u32_at(&payload[8..12]);
    let strings_len = usize::try_from(u32_at(&payload[12..16]))
        .map_err(|_| "metadata string bytes exceed platform size")?;
    let item_count = usize::try_from(u32_at(&payload[16..20]))
        .map_err(|_| "metadata item count exceeds platform size")?;
    let title_count = usize::try_from(u32_at(&payload[20..24]))
        .map_err(|_| "metadata title count exceeds platform size")?;
    let hash_count = usize::try_from(u32_at(&payload[24..28]))
        .map_err(|_| "metadata hash count exceeds platform size")?;
    let disk_count = usize::try_from(u32_at(&payload[28..32]))
        .map_err(|_| "metadata disk count exceeds platform size")?;
    let strings_offset = SOFTWARE_HEADER_LEN;
    let strings = decode_strings(payload, string_count, strings_offset, strings_len)?;
    let string_storage = (string_count as usize + 1) * 4 + strings_len;
    let mut cursor = strings_offset + string_storage;
    let items_end = cursor
        .checked_add(
            item_count
                .checked_mul(SOFTWARE_ITEM_LEN)
                .ok_or_else(|| "metadata items overflow".to_string())?,
        )
        .ok_or_else(|| "metadata items overflow".to_string())?;
    let titles_end = items_end
        .checked_add(
            title_count
                .checked_mul(SOFTWARE_TITLE_LEN)
                .ok_or_else(|| "metadata titles overflow".to_string())?,
        )
        .ok_or_else(|| "metadata titles overflow".to_string())?;
    let hashes_end = titles_end
        .checked_add(
            hash_count
                .checked_mul(SOFTWARE_HASH_LEN)
                .ok_or_else(|| "metadata hashes overflow".to_string())?,
        )
        .ok_or_else(|| "metadata hashes overflow".to_string())?;
    let disks_end = hashes_end
        .checked_add(
            disk_count
                .checked_mul(SOFTWARE_DISK_LEN)
                .ok_or_else(|| "metadata disks overflow".to_string())?,
        )
        .ok_or_else(|| "metadata disks overflow".to_string())?;
    let candidate_count = u32_at(&payload[32..36]) as usize;
    let candidates_end = disks_end
        .checked_add(
            candidate_count
                .checked_mul(4)
                .ok_or_else(|| "metadata candidates overflow".to_string())?,
        )
        .ok_or_else(|| "metadata candidates overflow".to_string())?;
    if candidates_end != payload.len() {
        return Err("software metadata shard sections are invalid".into());
    }
    let mut items = Vec::with_capacity(item_count);
    for row in payload[cursor..items_end].chunks_exact(SOFTWARE_ITEM_LEN) {
        let name = required_string(&strings, u32_at(&row[..4]), "item name")?;
        items.push(SoftwareItem {
            name,
            parent_name: string_value(&strings, u32_at(&row[4..8]))?,
            description: required_string(&strings, u32_at(&row[8..12]), "item description")?,
            year: string_value(&strings, u32_at(&row[12..16]))?,
            publisher: string_value(&strings, u32_at(&row[16..20]))?,
            region: string_value(&strings, u32_at(&row[20..24]))?,
        });
    }
    if items
        .windows(2)
        .any(|window| window[0].name >= window[1].name)
    {
        return Err("software metadata items are not sorted".into());
    }
    cursor = items_end;
    let mut title_candidates = BTreeMap::new();
    let mut range_cursor = 0usize;
    let mut previous_title = None;
    for row in payload[cursor..titles_end].chunks_exact(SOFTWARE_TITLE_LEN) {
        let key = required_string(&strings, u32_at(&row[..4]), "title")?;
        if previous_title
            .as_ref()
            .is_some_and(|previous| previous >= &key)
        {
            return Err("metadata title index is not sorted".into());
        }
        previous_title = Some(key.clone());
        let start = u32_at(&row[4..8]) as usize;
        let count = u32_at(&row[8..12]) as usize;
        if start != range_cursor
            || start.checked_add(count).is_none()
            || start + count > candidate_count
        {
            return Err("metadata title candidate range is invalid".into());
        }
        let values = &payload[candidates_end - candidate_count * 4 + start * 4
            ..candidates_end - candidate_count * 4 + (start + count) * 4];
        title_candidates.insert(key, candidate_names(values, &items)?);
        range_cursor = start + count;
    }
    if range_cursor > candidate_count {
        return Err("metadata title candidate ranges are invalid".into());
    }
    cursor = titles_end;
    let mut hash_candidates = Vec::with_capacity(hash_count);
    for row in payload[cursor..hashes_end].chunks_exact(SOFTWARE_HASH_LEN) {
        let size = u64_at(&row[..8]);
        let crc32 = u32_at(&row[8..12]);
        let start = u32_at(&row[12..16]) as usize;
        let count = u32_at(&row[16..20]) as usize;
        if start.checked_add(count).is_none() || start + count > candidate_count {
            return Err("metadata hash candidate range is invalid".into());
        }
        let base = candidates_end - candidate_count * 4;
        hash_candidates.push(SoftwareHashCandidate {
            size,
            crc32,
            software_names: candidate_names(
                &payload[base + start * 4..base + (start + count) * 4],
                &items,
            )?,
        });
    }
    if hash_candidates
        .windows(2)
        .any(|window| (window[0].size, window[0].crc32) >= (window[1].size, window[1].crc32))
    {
        return Err("metadata hash index is not sorted".into());
    }
    cursor = hashes_end;
    let mut disk_candidates = Vec::with_capacity(disk_count);
    for row in payload[cursor..disks_end].chunks_exact(SOFTWARE_DISK_LEN) {
        let sha1 = hex_lower(&row[..20]);
        let start = u32_at(&row[20..24]) as usize;
        let count = u32_at(&row[24..28]) as usize;
        if start.checked_add(count).is_none() || start + count > candidate_count {
            return Err("metadata disk candidate range is invalid".into());
        }
        let base = candidates_end - candidate_count * 4;
        disk_candidates.push(SoftwareDiskCandidate {
            sha1,
            software_names: candidate_names(
                &payload[base + start * 4..base + (start + count) * 4],
                &items,
            )?,
        });
    }
    if disk_candidates
        .windows(2)
        .any(|window| window[0].sha1 >= window[1].sha1)
    {
        return Err("metadata disk index is not sorted".into());
    }
    Ok(SoftwareShard {
        items,
        title_candidates,
        hash_candidates,
        disk_candidates,
    })
}

fn candidate_names(bytes: &[u8], items: &[SoftwareItem]) -> Result<Vec<String>, String> {
    let mut names = Vec::with_capacity(bytes.len() / 4);
    for row in bytes.chunks_exact(4) {
        let index = u32_at(row) as usize;
        names.push(
            items
                .get(index)
                .ok_or_else(|| "metadata candidate item index is invalid".to_string())?
                .name
                .clone(),
        );
    }
    if names.windows(2).any(|window| window[0] >= window[1]) {
        return Err("metadata candidate range is not sorted".into());
    }
    Ok(names)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

fn decode_arcade(payload: &[u8]) -> Result<ArcadeShard, String> {
    if payload.len() < ARCADE_HEADER_LEN
        || &payload[..4] != ARCADE_MAGIC
        || u32_at(&payload[4..8]) != VERSION
    {
        return Err("arcade metadata shard header is invalid".into());
    }
    let string_count = usize::try_from(u32_at(&payload[8..12]))
        .map_err(|_| "arcade string count exceeds platform size")?;
    let strings_len = usize::try_from(u32_at(&payload[12..16]))
        .map_err(|_| "arcade string bytes exceed platform size")?;
    let mame_count = usize::try_from(u32_at(&payload[16..20]))
        .map_err(|_| "arcade mame count exceeds platform size")?;
    let hbmame_count = usize::try_from(u32_at(&payload[20..24]))
        .map_err(|_| "arcade hbmame count exceeds platform size")?;
    let mister_count = usize::try_from(u32_at(&payload[24..28]))
        .map_err(|_| "arcade mister count exceeds platform size")?;
    let key_count = usize::try_from(u32_at(&payload[28..32]))
        .map_err(|_| "arcade key count exceeds platform size")?;
    let strings = decode_strings(payload, string_count as u32, ARCADE_HEADER_LEN, strings_len)?;
    let string_storage = (string_count + 1) * 4 + strings_len;
    let mut cursor = ARCADE_HEADER_LEN + string_storage;
    let mame_end = cursor
        .checked_add(
            mame_count
                .checked_mul(ARCADE_MACHINE_LEN)
                .ok_or_else(|| "arcade mame rows overflow".to_string())?,
        )
        .ok_or_else(|| "arcade mame rows overflow".to_string())?;
    let hbmame_end = mame_end
        .checked_add(
            hbmame_count
                .checked_mul(ARCADE_MACHINE_LEN)
                .ok_or_else(|| "arcade hbmame rows overflow".to_string())?,
        )
        .ok_or_else(|| "arcade hbmame rows overflow".to_string())?;
    let mister_end = hbmame_end
        .checked_add(
            mister_count
                .checked_mul(ARCADE_MISTER_LEN)
                .ok_or_else(|| "arcade mister rows overflow".to_string())?,
        )
        .ok_or_else(|| "arcade mister rows overflow".to_string())?;
    let keys_end = mister_end
        .checked_add(
            key_count
                .checked_mul(ARCADE_KEY_LEN)
                .ok_or_else(|| "arcade keys overflow".to_string())?,
        )
        .ok_or_else(|| "arcade keys overflow".to_string())?;
    if keys_end != payload.len() {
        return Err("arcade metadata shard sections are invalid".into());
    }
    let machines = |bytes: &[u8], count: usize| -> Result<Vec<ArcadeMachine>, String> {
        let mut rows = Vec::with_capacity(count);
        for row in bytes.chunks_exact(ARCADE_MACHINE_LEN) {
            rows.push(ArcadeMachine {
                setname: required_string(&strings, u32_at(&row[..4]), "Arcade setname")?,
                parent_setname: string_value(&strings, u32_at(&row[4..8]))?,
                title: required_string(&strings, u32_at(&row[8..12]), "Arcade title")?,
                year: string_value(&strings, u32_at(&row[12..16]))?,
                manufacturer: string_value(&strings, u32_at(&row[16..20]))?,
                players: (row[20] != 0).then_some(row[20]),
                control: string_value(&strings, u32_at(&row[24..28]))?,
            });
        }
        if rows
            .windows(2)
            .any(|window| window[0].setname >= window[1].setname)
        {
            return Err("arcade machine rows are not sorted".into());
        }
        Ok(rows)
    };
    let mame = machines(&payload[cursor..mame_end], mame_count)?;
    cursor = mame_end;
    let hbmame = machines(&payload[cursor..hbmame_end], hbmame_count)?;
    let mut mister = Vec::with_capacity(mister_count);
    for row in payload[hbmame_end..mister_end].chunks_exact(ARCADE_MISTER_LEN) {
        mister.push(MisterArcadeEntry {
            setname_key: required_string(&strings, u32_at(&row[..4]), "Arcade MRA setname")?,
            mra_name_key: required_string(&strings, u32_at(&row[4..8]), "Arcade MRA filename")?,
            title: required_string(&strings, u32_at(&row[8..12]), "Arcade MRA title")?,
            category: required_string(&strings, u32_at(&row[12..16]), "Arcade MRA category")?,
            year: (u32_at(&row[16..20]) != 0).then_some(u32_at(&row[16..20]) as u16),
            manufacturer: required_string(
                &strings,
                u32_at(&row[20..24]),
                "Arcade MRA manufacturer",
            )?,
            players: (row[24] != 0).then_some(row[24]),
            control: required_string(&strings, u32_at(&row[28..32]), "Arcade MRA control")?,
        });
    }
    if mister
        .windows(2)
        .any(|window| window[0].setname_key >= window[1].setname_key)
    {
        return Err("arcade MRA rows are not sorted".into());
    }
    Ok(ArcadeShard {
        mame,
        hbmame,
        mister,
    })
}

/// Encode software data into the deterministic shard payload format.
pub fn encode_software(shard: &SoftwareShard) -> Result<Vec<u8>, String> {
    let mut strings = Vec::new();
    let mut string_index = |value: Option<&str>| put_string(&mut strings, value);
    let mut item_rows = Vec::with_capacity(shard.items.len());
    for item in &shard.items {
        item_rows.push([
            string_index(Some(&item.name)),
            string_index(item.parent_name.as_deref()),
            string_index(Some(&item.description)),
            string_index(item.year.as_deref()),
            string_index(item.publisher.as_deref()),
            string_index(item.region.as_deref()),
            0,
            0,
        ]);
    }
    let mut title_rows = Vec::new();
    let mut candidate_rows = Vec::new();
    for (title, names) in &shard.title_candidates {
        let start = candidate_rows.len() as u32;
        for name in names {
            candidate_rows.push(
                shard
                    .items
                    .binary_search_by(|item| item.name.cmp(name))
                    .map_err(|_| format!("title candidate {name} is not an item"))?
                    as u32,
            );
        }
        title_rows.push((string_index(Some(title)), start, names.len() as u32));
    }
    let mut hash_rows = Vec::new();
    for candidate in &shard.hash_candidates {
        let start = candidate_rows.len() as u32;
        for name in &candidate.software_names {
            candidate_rows.push(
                shard
                    .items
                    .binary_search_by(|item| item.name.cmp(name))
                    .map_err(|_| format!("hash candidate {name} is not an item"))?
                    as u32,
            );
        }
        hash_rows.push((
            candidate.size,
            candidate.crc32,
            start,
            candidate.software_names.len() as u32,
        ));
    }
    let mut disk_rows = Vec::new();
    for candidate in &shard.disk_candidates {
        if candidate.sha1.len() != 40 || !candidate.sha1.is_ascii() {
            return Err("disk SHA-1 must be 40 ASCII characters".into());
        }
        let start = candidate_rows.len() as u32;
        for name in &candidate.software_names {
            candidate_rows.push(
                shard
                    .items
                    .binary_search_by(|item| item.name.cmp(name))
                    .map_err(|_| format!("disk candidate {name} is not an item"))?
                    as u32,
            );
        }
        let sha = candidate.sha1.as_bytes();
        let mut fixed = [0u8; 20];
        for (position, chunk) in sha.chunks_exact(2).enumerate() {
            fixed[position] = u8::from_str_radix(
                std::str::from_utf8(chunk).map_err(|_| "disk SHA-1 is invalid")?,
                16,
            )
            .map_err(|_| "disk SHA-1 is invalid")?;
        }
        disk_rows.push((fixed, start, candidate.software_names.len() as u32));
    }
    let mut strings_bytes = Vec::new();
    let mut offsets = vec![0u32];
    for value in &strings {
        strings_bytes.extend_from_slice(value.as_bytes());
        offsets.push(strings_bytes.len() as u32);
    }
    let mut payload = Vec::new();
    payload.extend_from_slice(SOFTWARE_MAGIC);
    put_u32(&mut payload, VERSION);
    put_u32(&mut payload, strings.len() as u32);
    put_u32(&mut payload, strings_bytes.len() as u32);
    put_u32(&mut payload, shard.items.len() as u32);
    put_u32(&mut payload, title_rows.len() as u32);
    put_u32(&mut payload, hash_rows.len() as u32);
    put_u32(&mut payload, disk_rows.len() as u32);
    put_u32(&mut payload, candidate_rows.len() as u32);
    for offset in offsets {
        put_u32(&mut payload, offset);
    }
    payload.extend_from_slice(&strings_bytes);
    for row in item_rows {
        for value in row {
            put_u32(&mut payload, value);
        }
    }
    for (key, start, count) in title_rows {
        put_u32(&mut payload, key);
        put_u32(&mut payload, start);
        put_u32(&mut payload, count);
        payload.extend_from_slice(&[0; 0]);
    }
    for (size, crc, start, count) in hash_rows {
        put_u64(&mut payload, size);
        put_u32(&mut payload, crc);
        put_u32(&mut payload, start);
        put_u32(&mut payload, count);
    }
    for (sha, start, count) in disk_rows {
        payload.extend_from_slice(&sha);
        put_u32(&mut payload, start);
        put_u32(&mut payload, count);
    }
    for candidate in candidate_rows {
        put_u32(&mut payload, candidate);
    }
    if payload.len() > MAX_DECODED_SHARD_BYTES {
        return Err("software shard exceeds decoded-size limit".into());
    }
    Ok(payload)
}

/// Encode the Arcade/HBMAME/MiSTer metadata shard.
pub fn encode_arcade(shard: &ArcadeShard) -> Result<Vec<u8>, String> {
    let mut strings = Vec::new();
    let encode_machine = |machine: &ArcadeMachine, strings: &mut Vec<String>| -> [u32; 8] {
        [
            put_string(strings, Some(&machine.setname)),
            put_string(strings, machine.parent_setname.as_deref()),
            put_string(strings, Some(&machine.title)),
            put_string(strings, machine.year.as_deref()),
            put_string(strings, machine.manufacturer.as_deref()),
            machine.players.unwrap_or(0) as u32,
            put_string(strings, machine.control.as_deref()),
            0,
        ]
    };
    let mame_rows = shard
        .mame
        .iter()
        .map(|row| encode_machine(row, &mut strings))
        .collect::<Vec<_>>();
    let hbmame_rows = shard
        .hbmame
        .iter()
        .map(|row| encode_machine(row, &mut strings))
        .collect::<Vec<_>>();
    let mut mister_rows = Vec::new();
    for row in &shard.mister {
        mister_rows.push([
            put_string(&mut strings, Some(&row.setname_key)),
            put_string(&mut strings, Some(&row.mra_name_key)),
            put_string(&mut strings, Some(&row.title)),
            put_string(&mut strings, Some(&row.category)),
            row.year.unwrap_or(0) as u32,
            put_string(&mut strings, Some(&row.manufacturer)),
            row.players.unwrap_or(0) as u32,
            put_string(&mut strings, Some(&row.control)),
        ]);
    }
    let mut strings_bytes = Vec::new();
    let mut offsets = vec![0u32];
    for value in &strings {
        strings_bytes.extend_from_slice(value.as_bytes());
        offsets.push(strings_bytes.len() as u32);
    }
    let mut payload = Vec::new();
    payload.extend_from_slice(ARCADE_MAGIC);
    put_u32(&mut payload, VERSION);
    put_u32(&mut payload, strings.len() as u32);
    put_u32(&mut payload, strings_bytes.len() as u32);
    put_u32(&mut payload, mame_rows.len() as u32);
    put_u32(&mut payload, hbmame_rows.len() as u32);
    put_u32(&mut payload, mister_rows.len() as u32);
    put_u32(&mut payload, 0);
    for offset in offsets {
        put_u32(&mut payload, offset);
    }
    payload.extend_from_slice(&strings_bytes);
    for rows in [mame_rows, hbmame_rows] {
        for row in rows {
            for value in row {
                put_u32(&mut payload, value);
            }
        }
    }
    for row in mister_rows {
        for value in row {
            put_u32(&mut payload, value);
        }
    }
    if payload.len() > MAX_DECODED_SHARD_BYTES {
        return Err("Arcade shard exceeds decoded-size limit".into());
    }
    Ok(payload)
}

pub struct MetadataFileBuilder {
    shards: Vec<(String, u8, Vec<u8>, u32, u32, u32)>,
}

impl MetadataFileBuilder {
    pub fn new() -> Self {
        Self { shards: Vec::new() }
    }

    pub fn add_software(&mut self, id: &str, shard: &SoftwareShard) -> Result<(), String> {
        let payload = encode_software(shard)?;
        self.add_payload(
            id,
            SOFTWARE_KIND,
            payload,
            shard.items.len(),
            shard.hash_candidates.len(),
            shard.disk_candidates.len(),
        )
    }

    pub fn add_arcade(&mut self, shard: &ArcadeShard) -> Result<(), String> {
        let payload = encode_arcade(shard)?;
        self.add_payload(
            "arcade",
            ARCADE_KIND,
            payload,
            shard.mame.len() + shard.hbmame.len(),
            0,
            shard.mister.len(),
        )
    }

    fn add_payload(
        &mut self,
        id: &str,
        kind: u8,
        payload: Vec<u8>,
        item_rows: usize,
        hash_rows: usize,
        aux_rows: usize,
    ) -> Result<(), String> {
        id_bytes(id)?;
        if self.shards.iter().any(|existing| existing.0 == id) {
            return Err(format!("duplicate metadata shard {id}"));
        }
        let item_rows = u32::try_from(item_rows).map_err(|_| "metadata row count exceeds u32")?;
        let hash_rows = u32::try_from(hash_rows).map_err(|_| "metadata row count exceeds u32")?;
        let aux_rows = u32::try_from(aux_rows).map_err(|_| "metadata row count exceeds u32")?;
        self.shards.push((
            id.to_string(),
            kind,
            payload,
            item_rows,
            hash_rows,
            aux_rows,
        ));
        Ok(())
    }

    pub fn encode(mut self) -> Result<Vec<u8>, String> {
        self.shards.sort_by_key(|left| id_bytes(&left.0).unwrap());
        let index_len = self
            .shards
            .len()
            .checked_mul(INDEX_ENTRY_LEN)
            .ok_or_else(|| "metadata index overflows".to_string())?;
        let mut payloads = Vec::new();
        let mut offset = HEADER_LEN + index_len;
        let mut index = Vec::with_capacity(index_len);
        for (id, kind, raw, item_rows, hash_rows, aux_rows) in self.shards {
            let compressed = compress_prepend_size(&raw);
            if compressed.len() > MAX_COMPRESSED_SHARD_BYTES {
                return Err(format!("metadata shard {id} exceeds compressed-size limit"));
            }
            let digest = Sha256::digest(&raw);
            let id_fixed = id_bytes(&id)?;
            let mut row = vec![0u8; INDEX_ENTRY_LEN];
            row[..32].copy_from_slice(&id_fixed);
            row[32] = kind;
            row[40..48].copy_from_slice(&(offset as u64).to_le_bytes());
            row[48..52].copy_from_slice(&(compressed.len() as u32).to_le_bytes());
            row[52..56].copy_from_slice(&(raw.len() as u32).to_le_bytes());
            row[56..88].copy_from_slice(&digest);
            row[88..92].copy_from_slice(&item_rows.to_le_bytes());
            row[92..96].copy_from_slice(&hash_rows.to_le_bytes());
            row[96..100].copy_from_slice(&aux_rows.to_le_bytes());
            index.extend_from_slice(&row);
            payloads.push(compressed);
            offset = offset
                .checked_add(payloads.last().unwrap().len())
                .ok_or_else(|| "metadata file length overflows".to_string())?;
        }
        let file_len = u64::try_from(offset).map_err(|_| "metadata file length exceeds u64")?;
        let index_digest = Sha256::digest(&index);
        let mut output = Vec::with_capacity(offset);
        output.extend_from_slice(HEADER_MAGIC);
        put_u32(&mut output, VERSION);
        put_u32(&mut output, 0);
        put_u64(&mut output, file_len);
        put_u32(&mut output, payloads.len() as u32);
        put_u64(&mut output, HEADER_LEN as u64);
        put_u32(&mut output, INDEX_ENTRY_LEN as u32);
        put_u32(&mut output, index_len as u32);
        output.extend_from_slice(&index_digest);
        output.extend_from_slice(&[0; 20]);
        output.extend_from_slice(&index);
        for payload in payloads {
            output.extend_from_slice(&payload);
        }
        Ok(output)
    }

    pub fn write_to(self, path: &Path) -> Result<MetadataStatus, String> {
        let shard_count = self.shards.len();
        let bytes = self.encode()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create metadata directory: {error}"))?;
        }
        let temp = path.with_extension("bin.tmp");
        std::fs::write(&temp, &bytes).map_err(|error| format!("write metadata: {error}"))?;
        std::fs::rename(&temp, path).map_err(|error| format!("publish metadata: {error}"))?;
        Ok(MetadataStatus {
            format: FORMAT,
            shard_count,
            file_len: bytes.len() as u64,
        })
    }
}

impl Default for MetadataFileBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "builder")]
pub fn build_from_sqlite(
    mame_path: &Path,
    hbmame_path: &Path,
    output: &Path,
) -> Result<MetadataStatus, String> {
    use rusqlite::Connection;
    let mame = Connection::open_with_flags(mame_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("open MAME metadata: {error}"))?;
    let hbmame =
        Connection::open_with_flags(hbmame_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| format!("open HBMAME metadata: {error}"))?;
    let mut builder = MetadataFileBuilder::new();
    for (platform_id, canonical, source_lists) in RUNTIME_SOFTWARE_SYSTEMS {
        let shard = read_software_shard(&mame, source_lists)?;
        if shard.items.is_empty() {
            return Err(format!(
                "runtime metadata system {platform_id} has no items"
            ));
        }
        builder.add_software(platform_id, &shard)?;
        let _ = canonical;
    }
    builder.add_arcade(&read_arcade_shard(&mame, &hbmame)?)?;
    let status = builder.write_to(output)?;
    if status.file_len > 8 * 1024 * 1024 {
        return Err(format!("metadata file {} exceeds 8 MiB", status.file_len));
    }
    Ok(status)
}

#[cfg(feature = "builder")]
fn read_software_shard(
    conn: &rusqlite::Connection,
    source_lists: &[&str],
) -> Result<SoftwareShard, String> {
    use rusqlite::params_from_iter;
    let placeholders = std::iter::repeat_n("?", source_lists.len())
        .collect::<Vec<_>>()
        .join(",");
    let mut items_by_name = BTreeMap::<String, SoftwareItem>::new();
    let mut stmt = conn.prepare(&format!("SELECT list_name,software_name,parent_name,description,year,publisher,region,source_version FROM mame_software_items WHERE list_name IN ({placeholders}) ORDER BY list_name,software_name")) .map_err(|error| format!("prepare software items: {error}"))?;
    let rows = stmt
        .query_map(params_from_iter(source_lists.iter().copied()), |row| {
            Ok((
                row.get::<_, String>(1)?,
                SoftwareItem {
                    name: row.get(1)?,
                    parent_name: row.get(2)?,
                    description: row.get(3)?,
                    year: row.get(4)?,
                    publisher: row.get(5)?,
                    region: row.get(6)?,
                },
            ))
        })
        .map_err(|error| format!("read software items: {error}"))?;
    for row in rows.flatten() {
        items_by_name.insert(row.0, row.1);
    }
    let items = items_by_name.into_values().collect::<Vec<_>>();
    let mut title_candidates = BTreeMap::<String, BTreeSet<String>>::new();
    for item in &items {
        title_candidates
            .entry(crate::library_db::canonical_variant_title(
                &item.description,
            ))
            .or_default()
            .insert(item.name.clone());
    }
    let mut hashes = BTreeMap::<(u64, u32), BTreeSet<String>>::new();
    let mut disks = BTreeMap::<String, BTreeSet<String>>::new();
    let mut stmt = conn.prepare(&format!("SELECT software_name,size,crc32,disk_sha1 FROM mame_software_hashes WHERE list_name IN ({placeholders}) AND ((size IS NOT NULL AND crc32 IS NOT NULL) OR disk_sha1 IS NOT NULL) ORDER BY list_name,software_name")) .map_err(|error| format!("prepare software hashes: {error}"))?;
    let rows = stmt
        .query_map(params_from_iter(source_lists.iter().copied()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|error| format!("read software hashes: {error}"))?;
    for row in rows.flatten() {
        let (name, size, crc, disk) = row;
        if !items.iter().any(|item| item.name == name) {
            continue;
        }
        if let (Some(size), Some(crc)) = (
            size.and_then(|value| u64::try_from(value).ok()),
            crc.and_then(|value| u32::from_str_radix(value.trim(), 16).ok()),
        ) {
            hashes.entry((size, crc)).or_default().insert(name.clone());
        }
        if let Some(disk) = disk.filter(|value| value.len() == 40) {
            disks
                .entry(disk.to_ascii_lowercase())
                .or_default()
                .insert(name);
        }
    }
    Ok(SoftwareShard {
        items,
        title_candidates: title_candidates
            .into_iter()
            .map(|(key, values)| (key, values.into_iter().collect()))
            .collect(),
        hash_candidates: hashes
            .into_iter()
            .map(|((size, crc), values)| SoftwareHashCandidate {
                size,
                crc32: crc,
                software_names: values.into_iter().collect(),
            })
            .collect(),
        disk_candidates: disks
            .into_iter()
            .map(|(sha1, values)| SoftwareDiskCandidate {
                sha1,
                software_names: values.into_iter().collect(),
            })
            .collect(),
    })
}

#[cfg(feature = "builder")]
fn read_arcade_shard(
    mame: &rusqlite::Connection,
    hbmame: &rusqlite::Connection,
) -> Result<ArcadeShard, String> {
    use rusqlite::Connection;
    let read = |conn: &Connection| -> Result<Vec<ArcadeMachine>, String> {
        let mut stmt = conn
            .prepare("SELECT setname,parent_setname,title,year,manufacturer,players,control_type FROM mame_machines ORDER BY setname")
            .map_err(|error| format!("prepare arcade machines: {error}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ArcadeMachine {
                    setname: row.get(0)?,
                    parent_setname: row.get(1)?,
                    title: row.get(2)?,
                    year: row.get(3)?,
                    manufacturer: row.get(4)?,
                    players: row
                        .get::<_, Option<i64>>(5)?
                        .and_then(|value| u8::try_from(value).ok()),
                    control: row.get(6)?,
                })
            })
            .map_err(|error| format!("read arcade machines: {error}"))?;
        Ok(rows.flatten().collect())
    };
    let mut mame_rows = read(mame)?;
    let mut hbmame_rows = read(hbmame)?;
    mame_rows.sort_by(|a, b| a.setname.cmp(&b.setname));
    hbmame_rows.sort_by(|a, b| a.setname.cmp(&b.setname));
    let mut mister = Vec::new();
    if let Ok(mut stmt) = mame.prepare("SELECT setname_key,mra_name_key,name,category,year,manufacturer,players,move_inputs,special_controls FROM mister_arcade_entries ORDER BY ordinal") {
        let rows = stmt
            .query_map([], |row| {
                let players = row.get::<_, String>(6)?;
                let move_inputs = row.get::<_, String>(7)?;
                let special = row.get::<_, String>(8)?;
                Ok(MisterArcadeEntry {
                    setname_key: row.get(0)?,
                    mra_name_key: row.get(1)?,
                    title: row.get(2)?,
                    category: row.get(3)?,
                    year: row
                        .get::<_, Option<i64>>(4)?
                        .and_then(|value| u16::try_from(value).ok()),
                    manufacturer: row.get(5)?,
                    players: players
                        .split_whitespace()
                        .next()
                        .and_then(|value| value.parse().ok()),
                    control: if special.trim().is_empty() {
                        move_inputs
                    } else {
                        special
                    },
                })
            })
            .map_err(|error| format!("read MiSTer Arcade metadata: {error}"))?;
        mister.extend(rows.flatten());
    }
    mister.sort_by(|a, b| {
        a.setname_key
            .cmp(&b.setname_key)
            .then(a.mra_name_key.cmp(&b.mra_name_key))
    });
    Ok(ArcadeShard {
        mame: mame_rows,
        hbmame: hbmame_rows,
        mister,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SoftwareShard {
        let items = vec![
            SoftwareItem {
                name: "clone".into(),
                parent_name: Some("parent".into()),
                description: "Example (USA)".into(),
                year: Some("1990".into()),
                publisher: Some("Pub".into()),
                region: Some("USA".into()),
            },
            SoftwareItem {
                name: "parent".into(),
                parent_name: None,
                description: "Example".into(),
                year: None,
                publisher: None,
                region: None,
            },
        ];
        SoftwareShard {
            items,
            title_candidates: BTreeMap::from([(
                "example".into(),
                vec!["clone".into(), "parent".into()],
            )]),
            hash_candidates: vec![SoftwareHashCandidate {
                size: 4,
                crc32: 0x1234,
                software_names: vec!["parent".into()],
            }],
            disk_candidates: vec![SoftwareDiskCandidate {
                sha1: "0123456789abcdef0123456789abcdef01234567".into(),
                software_names: vec!["clone".into()],
            }],
        }
    }

    #[test]
    fn software_payload_round_trips() {
        let shard = sample();
        let payload = encode_software(&shard).expect("encode");
        assert_eq!(decode_software(&payload).expect("decode"), shard);
    }

    #[test]
    fn container_is_deterministic_and_uses_fixed_sha_vector() {
        let mut builder = MetadataFileBuilder::new();
        builder.add_software("nes", &sample()).expect("add");
        let first = builder.encode().expect("encode");
        let mut builder = MetadataFileBuilder::new();
        builder.add_software("nes", &sample()).expect("add");
        let second = builder.encode().expect("encode");
        assert_eq!(first, second);
        let store = MetadataStore::from_bytes(&first).expect("open");
        assert_eq!(store.status().shard_count, 1);
        assert_eq!(
            store.software_shard("nes").expect("read").expect("shard"),
            sample()
        );
    }

    #[test]
    fn container_rejects_corruption_and_reserved_bytes() {
        let mut builder = MetadataFileBuilder::new();
        builder.add_software("nes", &sample()).expect("add");
        let bytes = builder.encode().expect("encode");
        let mut corrupted = bytes.clone();
        corrupted[96] ^= 1;
        assert!(MetadataStore::from_bytes(&corrupted).is_err());
        let mut corrupted = bytes;
        corrupted[95] = 1;
        assert!(MetadataStore::from_bytes(&corrupted).is_err());
    }

    #[test]
    fn runtime_mapping_has_thirty_four_systems() {
        assert_eq!(RUNTIME_SOFTWARE_SYSTEMS.len(), 34);
    }
}
