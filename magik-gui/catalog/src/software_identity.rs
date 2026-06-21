//! MAME and software-list identity enrichment.

use crate::game_discovery::{DiscoverySourceKind, GameDiscovery};
use crate::library_db;
use rusqlite::{params, Connection};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub(crate) type MachineMetadataRow = (String, String, Option<String>, Option<String>);
pub(crate) type MachineMetadataRows = BTreeMap<String, MachineMetadataRow>;

const ARCADE_PARENT_OVERRIDES: &[(&str, &str)] = &[
    ("dimahoo-1", "dimahoo"),
    ("dimahoo-2", "dimahoo"),
    ("dimahoo-3", "dimahoo"),
    ("esprade-fp", "esprade"),
    ("espradej-fp", "esprade"),
    ("ffightae-cps2", "ffightae"),
    ("msh-1", "msh"),
    ("msh-2", "msh"),
    ("mshvsf-1", "mshvsf"),
    ("mshvsf-2", "mshvsf"),
    ("mvsc-1", "mvsc"),
    ("mvsc-2", "mvsc"),
    ("mvsc-3", "mvsc"),
    ("mvsc-4", "mvsc"),
    ("progear-1", "progear"),
    ("progear-2", "progear"),
    ("progear-3", "progear"),
    ("sfa2-1", "sfa2"),
    ("sfa2-2", "sfa2"),
    ("sfa3-1", "sfa3"),
    ("sfa3-2", "sfa3"),
    ("sf2ceaimedb", "sf2ce"),
    ("sf2ceaimedf", "sf2ce"),
    ("sf2cebfire", "sf2ce"),
    ("sf2cebih", "sf2ce"),
    ("sf2cebof", "sf2ce"),
    ("sf2cefires", "sf2ce"),
    ("sf2ces15", "sf2ce"),
    ("sf2ces17", "sf2ce"),
    ("sf2ces21", "sf2ce"),
    ("sf2ces22", "sf2ce"),
    ("sf2ces23", "sf2ce"),
    ("sf2cevampiric", "sf2ce"),
    ("sfz2al-1", "sfz2al"),
    ("sfz2al-2", "sfz2al"),
    ("sfz2al-3", "sfz2al"),
    ("hsf2j1gouki", "hsf2"),
    ("hsf2j1tgouki", "hsf2"),
    ("ssf2t-3", "ssf2t"),
    ("ssf2t-4", "ssf2t"),
    ("ssf2t-5", "ssf2t"),
    ("strider-fix", "strider"),
    ("vsav-4", "vsav"),
    ("vsav-5", "vsav"),
    ("wofch-1", "wofch"),
    ("xmcota-1", "xmcota"),
    ("xmcota-2", "xmcota"),
];

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SoftwareHashCacheKey {
    pub(crate) list_name: String,
    pub(crate) file_path: String,
    pub(crate) size: u64,
    pub(crate) mtime_secs: i64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SoftwareHashCache {
    pub(crate) entries: HashMap<SoftwareHashCacheKey, Option<String>>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MameMachineMetadata {
    pub(crate) parent_setname: Option<String>,
    pub(crate) title: String,
    pub(crate) year: Option<String>,
    pub(crate) manufacturer: Option<String>,
}

#[derive(Default)]
pub(crate) struct ArcadeMachineMetadata {
    pub(crate) mame: HashMap<String, MameMachineMetadata>,
    pub(crate) hbmame: HashMap<String, MameMachineMetadata>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MameSoftwareItemMetadata {
    pub(crate) parent_name: Option<String>,
    pub(crate) description: String,
    pub(crate) year: Option<String>,
    pub(crate) publisher: Option<String>,
    pub(crate) region: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MameSoftwareMetadata {
    pub(crate) items: HashMap<(String, String), MameSoftwareItemMetadata>,
    pub(crate) hash_index: HashMap<(String, u64, u32), Vec<String>>,
    pub(crate) disk_index: HashMap<(String, String), Vec<String>>,
    pub(crate) title_index: HashMap<(String, String), Vec<String>>,
    pub(crate) family_members: HashMap<(String, String), Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SoftwareIdentity {
    pub(crate) list_name: String,
    pub(crate) software_name: String,
    pub(crate) family_id: String,
    pub(crate) metadata_title: Option<String>,
    pub(crate) year: Option<String>,
    pub(crate) manufacturer: Option<String>,
    pub(crate) region: Option<String>,
    pub(crate) source: &'static str,
}

pub(crate) fn load_mame_machine_metadata(path: &Path) -> HashMap<String, MameMachineMetadata> {
    let Ok(conn) = library_db::open_sqlite_read_only(path) else {
        return HashMap::new();
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT setname,parent_setname,title,year,manufacturer FROM mame_machines")
    else {
        return HashMap::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            MameMachineMetadata {
                parent_setname: row.get(1)?,
                title: row.get(2)?,
                year: row.get(3)?,
                manufacturer: row.get(4)?,
            },
        ))
    }) else {
        return HashMap::new();
    };
    rows.filter_map(|row| row.ok()).collect()
}

pub(crate) fn load_mame_software_metadata(path: &Path) -> MameSoftwareMetadata {
    let Ok(conn) = library_db::open_sqlite_read_only(path) else {
        return MameSoftwareMetadata::default();
    };
    if !library_db::sqlite_table_exists(&conn, "mame_software_items").unwrap_or(false) {
        return MameSoftwareMetadata::default();
    }
    let mut metadata = MameSoftwareMetadata::default();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT list_name,software_name,parent_name,description,year,publisher,region,source_version
         FROM mame_software_items",
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            let _source_version = row.get::<_, String>(7)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                MameSoftwareItemMetadata {
                    parent_name: row.get(2)?,
                    description: row.get(3)?,
                    year: row.get(4)?,
                    publisher: row.get(5)?,
                    region: row.get(6)?,
                },
            ))
        }) {
            for row in rows.flatten() {
                let (list, name, item) = row;
                let title_key = library_db::canonical_variant_title(&item.description);
                metadata
                    .title_index
                    .entry((list.clone(), title_key))
                    .or_default()
                    .push(name.clone());
                let family = item
                    .parent_name
                    .as_deref()
                    .filter(|parent| !parent.trim().is_empty())
                    .unwrap_or(&name)
                    .to_string();
                metadata
                    .family_members
                    .entry((list.clone(), family))
                    .or_default()
                    .push(name.clone());
                metadata.items.insert((list, name), item);
            }
        }
    }
    if let Ok(mut stmt) = conn.prepare(
        "SELECT list_name,software_name,size,crc32
         FROM mame_software_hashes
         WHERE size IS NOT NULL AND crc32 IS NOT NULL",
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        }) {
            for (list, name, size, crc_hex) in rows.flatten() {
                let Ok(size) = u64::try_from(size) else {
                    continue;
                };
                let Some(crc) = parse_hex_u32(&crc_hex) else {
                    continue;
                };
                metadata
                    .hash_index
                    .entry((list, size, crc))
                    .or_default()
                    .push(name);
            }
        }
    }
    if let Ok(mut stmt) = conn.prepare(
        "SELECT list_name,software_name,disk_sha1
         FROM mame_software_hashes
         WHERE disk_sha1 IS NOT NULL",
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        }) {
            for (list, name, sha1) in rows.flatten() {
                metadata
                    .disk_index
                    .entry((list, sha1.to_ascii_lowercase()))
                    .or_default()
                    .push(name);
            }
        }
    }
    for members in metadata.family_members.values_mut() {
        members.sort();
        members.dedup();
    }
    metadata
}

pub(crate) fn load_arcade_machine_metadata(mame_path: &Path, hbmame_path: &Path) -> ArcadeMachineMetadata {
    ArcadeMachineMetadata {
        mame: load_mame_machine_metadata(mame_path),
        hbmame: load_mame_machine_metadata(hbmame_path),
    }
}

pub(crate) fn write_simple_mame_metadata_db(path: &Path, rows: &MachineMetadataRows) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create metadata dir {}: {e}", parent.display()))?;
    }
    let tmp = library_db::sqlite_temp_path(path);
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove stale metadata temp {}: {e}", tmp.display())),
    }
    let mut conn =
        Connection::open(&tmp).map_err(|e| format!("open metadata temp {}: {e}", tmp.display()))?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=OFF;
        PRAGMA synchronous=OFF;
        CREATE TABLE mame_machines (
            setname TEXT PRIMARY KEY,
            parent_setname TEXT,
            title TEXT NOT NULL,
            year TEXT,
            manufacturer TEXT
        ) WITHOUT ROWID;
        "#,
    )
    .map_err(|e| format!("create metadata schema: {e}"))?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("begin metadata tx: {e}"))?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO mame_machines(setname,parent_setname,title,year,manufacturer)
                 VALUES (?1,?2,?3,?4,?5)",
            )
            .map_err(|e| format!("prepare metadata insert: {e}"))?;
        for (setname, (parent, title, year, manufacturer)) in rows {
            stmt.execute(params![
                setname.as_str(),
                parent.as_str(),
                title.as_str(),
                year.as_deref(),
                manufacturer.as_deref()
            ])
            .map_err(|e| format!("insert metadata row {setname}: {e}"))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("commit metadata tx: {e}"))?;
    library_db::sync_parent_dir(&tmp);
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("replace metadata db {}: {e}", path.display()))?;
    library_db::sync_parent_dir(path);
    Ok(())
}

pub(crate) fn mame_identity_for_discovery(discovery: &GameDiscovery) -> Option<String> {
    if discovery.platform_id != "arcade" && discovery.platform_id != "neogeo" {
        return None;
    }
    discovery
        .setname
        .as_deref()
        .map(str::trim)
        .filter(|setname| !setname.is_empty())
        .map(library_db::normalize_id)
}

pub(crate) fn software_list_for_platform(platform_id: &str) -> Option<&'static str> {
    match platform_id {
        "nes" => Some("nes"),
        "snes" => Some("snes"),
        "n64" => Some("n64"),
        "sms" => Some("sms"),
        "megadrive" => Some("megadriv"),
        "saturn" => Some("saturn"),
        _ => None,
    }
}

pub(crate) fn mame_software_identity_for_discovery(
    discovery: &GameDiscovery,
    metadata: &MameSoftwareMetadata,
    software_hash_cache: &mut SoftwareHashCache,
) -> Option<SoftwareIdentity> {
    mame_software_identity_for_discovery_with_hash_matcher(
        discovery,
        metadata,
        |discovery, list_name, metadata| {
            match_software_by_file_hash(discovery, list_name, metadata, software_hash_cache)
        },
    )
}

pub(crate) fn mame_software_identity_for_discovery_with_hash_matcher(
    discovery: &GameDiscovery,
    metadata: &MameSoftwareMetadata,
    hash_matcher: impl FnOnce(&GameDiscovery, &str, &MameSoftwareMetadata) -> Option<String>,
) -> Option<SoftwareIdentity> {
    let list_name = software_list_for_platform(&discovery.platform_id)?;
    let title_key = library_db::canonical_variant_title(&discovery.title);
    if let Some(names) = metadata
        .title_index
        .get(&(list_name.to_string(), title_key))
        .filter(|names| !names.is_empty())
    {
        return software_identity_from_metadata(list_name, &names[0], metadata, "filename");
    }
    if let Some(software_name) = hash_matcher(discovery, list_name, metadata) {
        return software_identity_from_metadata(
            list_name,
            &software_name,
            metadata,
            "mame-software",
        );
    }
    None
}

pub(crate) fn software_identity_from_metadata(
    list_name: &str,
    software_name: &str,
    metadata: &MameSoftwareMetadata,
    source: &'static str,
) -> Option<SoftwareIdentity> {
    let item = metadata
        .items
        .get(&(list_name.to_string(), software_name.to_string()))?;
    let family = item
        .parent_name
        .as_deref()
        .filter(|parent| !parent.trim().is_empty())
        .unwrap_or(software_name)
        .to_string();
    Some(SoftwareIdentity {
        list_name: list_name.to_string(),
        software_name: software_name.to_string(),
        family_id: format!("{list_name}:{family}"),
        metadata_title: Some(item.description.clone()),
        year: item.year.clone(),
        manufacturer: item.publisher.clone(),
        region: item.region.clone(),
        source,
    })
}

pub(crate) fn match_software_by_file_hash(
    discovery: &GameDiscovery,
    list_name: &str,
    metadata: &MameSoftwareMetadata,
    software_hash_cache: &mut SoftwareHashCache,
) -> Option<String> {
    match_software_by_file_hash_with_cache(
        discovery,
        list_name,
        metadata,
        library_db::env_bool("MISTER_LIBRARY_SOFTWARE_HASH"),
        software_hash_cache,
    )
}

pub(crate) fn match_software_by_file_hash_with_cache(
    discovery: &GameDiscovery,
    list_name: &str,
    metadata: &MameSoftwareMetadata,
    full_rom_hashing_enabled: bool,
    software_hash_cache: &mut SoftwareHashCache,
) -> Option<String> {
    if !matches!(
        discovery.source_kind,
        DiscoverySourceKind::PayloadFile | DiscoverySourceKind::ArchiveEntry
    ) {
        return None;
    }
    let source_path = discovery
        .source_path
        .split("::")
        .next()
        .unwrap_or(&discovery.source_path);
    if list_name == "saturn" && library_db::path_ext(source_path).as_deref() == Some("chd") {
        if let Some(disk_sha1) = chd_raw_sha1(source_path) {
            let key = (list_name.to_string(), disk_sha1);
            if let Some(names) = metadata
                .disk_index
                .get(&key)
                .filter(|names| !names.is_empty())
            {
                return Some(names[0].clone());
            }
        }
        return None;
    }
    if list_name == "saturn" {
        return None;
    }
    if !full_rom_hashing_enabled {
        return None;
    }
    software_hash_cache.get_or_compute(list_name, source_path, || {
        match_software_by_full_rom_hash(source_path, list_name, metadata)
    })
}

pub(crate) fn match_software_by_full_rom_hash(
    source_path: &str,
    list_name: &str,
    metadata: &MameSoftwareMetadata,
) -> Option<String> {
    let bytes = std::fs::read(source_path).ok()?;
    for candidate in rom_hash_candidates(list_name, &bytes) {
        let crc = crc32(&candidate);
        let key = (list_name.to_string(), candidate.len() as u64, crc);
        if let Some(names) = metadata
            .hash_index
            .get(&key)
            .filter(|names| !names.is_empty())
        {
            return Some(names[0].clone());
        }
    }
    None
}

impl SoftwareHashCache {
    pub(crate) fn load(path: &Path) -> Self {
        let Ok(conn) = library_db::open_sqlite_read_only(path) else {
            return Self::default();
        };
        if !library_db::sqlite_table_exists(&conn, "software_hash_cache").unwrap_or(false) {
            return Self::default();
        }
        let Ok(mut stmt) = conn.prepare(
            "SELECT list_name,file_path,size,mtime_secs,software_name FROM software_hash_cache",
        ) else {
            return Self::default();
        };
        let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                SoftwareHashCacheKey {
                    list_name: row.get(0)?,
                    file_path: row.get(1)?,
                    size: row.get::<_, i64>(2)?.max(0) as u64,
                    mtime_secs: row.get(3)?,
                },
                row.get::<_, Option<String>>(4)?,
            ))
        }) else {
            return Self::default();
        };
        let mut cache = Self::default();
        for row in rows.flatten() {
            cache.entries.insert(row.0, row.1);
        }
        cache
    }

    pub(crate) fn get_or_compute(
        &mut self,
        list_name: &str,
        source_path: &str,
        compute: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        let Some(key) = software_hash_cache_key(list_name, source_path) else {
            return compute();
        };
        if let Some(cached) = self.entries.get(&key) {
            return cached.clone();
        }
        let computed = compute();
        self.entries.insert(key, computed.clone());
        computed
    }
}

pub(crate) fn software_hash_cache_key(list_name: &str, source_path: &str) -> Option<SoftwareHashCacheKey> {
    let signature = library_db::file_signature(Path::new(source_path));
    if signature.size == 0 && signature.mtime_secs == 0 {
        return None;
    }
    Some(SoftwareHashCacheKey {
        list_name: list_name.to_string(),
        file_path: source_path.to_string(),
        size: signature.size,
        mtime_secs: signature.mtime_secs,
    })
}

pub(crate) fn chd_raw_sha1(path: &str) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut header = [0u8; 124];
    file.read_exact(&mut header).ok()?;
    chd_raw_sha1_from_header(&header)
}

pub(crate) fn chd_raw_sha1_from_header(header: &[u8]) -> Option<String> {
    if header.len() < 124 || &header[..8] != b"MComprHD" {
        return None;
    }
    let length = library_db::be_u32(&header[8..12]) as usize;
    let version = library_db::be_u32(&header[12..16]);
    let range = match version {
        3 if length == 120 => 80..100,
        4 if length == 108 => 88..108,
        5 if length == 124 => 64..84,
        _ => return None,
    };
    Some(library_db::hex_lower(&header[range]))
}

pub(crate) fn rom_hash_candidates(list_name: &str, bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    match list_name {
        "nes" => {
            if bytes.len() > 16 && &bytes[..4] == b"NES\x1a" {
                out.push(bytes[16..].to_vec());
            }
            out.push(bytes.to_vec());
        }
        "snes" => {
            if bytes.len() > 512 {
                out.push(bytes[512..].to_vec());
            }
            out.push(bytes.to_vec());
        }
        "n64" => {
            out.push(bytes.to_vec());
            out.push(swap_pairs(bytes));
            out.push(swap_words(bytes));
            out.push(reverse_words(bytes));
        }
        "sms" | "megadriv" => out.push(bytes.to_vec()),
        _ => out.push(bytes.to_vec()),
    }
    out.dedup();
    out
}

pub(crate) fn swap_pairs(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for chunk in out.chunks_exact_mut(2) {
        chunk.swap(0, 1);
    }
    out
}

pub(crate) fn swap_words(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for chunk in out.chunks_exact_mut(4) {
        chunk.swap(0, 2);
        chunk.swap(1, 3);
    }
    out
}

pub(crate) fn reverse_words(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for chunk in out.chunks_exact_mut(4) {
        chunk.reverse();
    }
    out
}

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

pub(crate) fn parse_hex_u32(value: &str) -> Option<u32> {
    u32::from_str_radix(value.trim(), 16).ok()
}

pub(crate) fn mame_identity_projection<'a>(
    identity_id: &str,
    metadata: &'a ArcadeMachineMetadata,
    mra_parent: Option<&str>,
) -> (
    String,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    &'static str,
) {
    if let Some(machine) = metadata.mame.get(identity_id) {
        let family_id = machine
            .parent_setname
            .as_deref()
            .filter(|parent| !parent.trim().is_empty())
            .unwrap_or(identity_id)
            .to_string();
        (
            family_id,
            Some(machine.title.as_str()),
            machine.year.as_deref(),
            machine.manufacturer.as_deref(),
            "mame",
        )
    } else if let Some(machine) = metadata.hbmame.get(identity_id) {
        let family_id = machine
            .parent_setname
            .as_deref()
            .filter(|parent| !parent.trim().is_empty())
            .unwrap_or(identity_id)
            .to_string();
        (
            family_id,
            Some(machine.title.as_str()),
            machine.year.as_deref(),
            machine.manufacturer.as_deref(),
            "hbmame",
        )
    } else if let Some(family_id) = normalized_parent_family(mra_parent, identity_id) {
        (family_id, None, None, None, "mra-parent")
    } else if let Some(parent) = arcade_parent_override(identity_id) {
        (
            parent.to_string(),
            None,
            None,
            None,
            "arcade-parent-override",
        )
    } else {
        (identity_id.to_string(), None, None, None, "setname")
    }
}

pub(crate) fn normalized_parent_family(parent: Option<&str>, identity_id: &str) -> Option<String> {
    let parent_id = library_db::normalize_id(parent?.trim());
    if parent_id.is_empty() || parent_id == identity_id {
        None
    } else {
        Some(parent_id)
    }
}

pub(crate) fn arcade_parent_override(identity_id: &str) -> Option<&'static str> {
    ARCADE_PARENT_OVERRIDES
        .iter()
        .find_map(|(alias, parent)| (*alias == identity_id).then_some(*parent))
}

pub(crate) fn preview_asset_pack_platform(path: &str) -> &'static str {
    let path = path.to_ascii_lowercase();
    if path.contains("neogeo") {
        "neogeo"
    } else if path.contains("snes-screenshots") {
        "snes"
    } else if path.contains("nes-screenshots") {
        "nes"
    } else if path.contains("n64-screenshots") {
        "n64"
    } else if path.contains("sms-screenshots") {
        "sms"
    } else if path.contains("megadrive-screenshots") {
        "megadrive"
    } else if path.contains("saturn") {
        "saturn"
    } else {
        "arcade"
    }
}

pub(crate) fn software_asset_key(list_name: &str, software_name: &str) -> String {
    format!("mame-software__{list_name}__{software_name}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConsolePreviewAsset {
    pub(crate) archive_path: String,
    pub(crate) asset_key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PreviewArchivePaths {
    pub(crate) by_platform: HashMap<String, String>,
}

impl PreviewArchivePaths {
    pub(crate) fn from_paths(paths: Vec<String>) -> Self {
        let mut by_platform = HashMap::new();
        for path in paths {
            by_platform
                .entry(preview_asset_pack_platform(&path).to_string())
                .or_insert(path);
        }
        Self { by_platform }
    }

    pub(crate) fn archive_for_platform(&self, platform: &str) -> Option<&str> {
        self.by_platform.get(platform).map(String::as_str)
    }

    pub(crate) fn len(&self) -> usize {
        self.by_platform.len()
    }
}

pub(crate) fn console_preview_asset(
    identity: &SoftwareIdentity,
    preview_paths: &PreviewArchivePaths,
) -> Option<ConsolePreviewAsset> {
    let platform = preview_platform_for_software_list(&identity.list_name);
    let archive_path = preview_paths.archive_for_platform(platform)?;
    let software_name = identity
        .family_id
        .split_once(':')
        .filter(|(list_name, _)| *list_name == identity.list_name)
        .map(|(_, family_name)| family_name)
        .unwrap_or(identity.software_name.as_str());
    Some(ConsolePreviewAsset {
        archive_path: archive_path.to_string(),
        asset_key: software_asset_key(&identity.list_name, software_name),
    })
}

pub(crate) fn preview_platform_for_software_list(list_name: &str) -> &str {
    match list_name {
        "megadriv" => "megadrive",
        value => value,
    }
}
