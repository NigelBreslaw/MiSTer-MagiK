// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! MAME and software-list identity enrichment.

use crate::game_discovery::{DiscoverySourceKind, GameDiscovery};
use crate::library_db;
use crate::media_identity::{ScreenshotAssetId, screenshot_pack_id_from_filename};
use crate::preview_worker;
use rusqlite::{Connection, params, params_from_iter};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;
#[cfg(feature = "builder")]
use std::path::PathBuf;
use std::time::Instant;

pub(crate) type MachineMetadataRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<u8>,
    Option<String>,
);
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
    pub(crate) players: Option<u8>,
    pub(crate) control: Option<String>,
}

#[derive(Default)]
pub(crate) struct ArcadeMachineMetadata {
    pub(crate) mame: HashMap<String, MameMachineMetadata>,
    pub(crate) hbmame: HashMap<String, MameMachineMetadata>,
    pub(crate) mister_by_setname: HashMap<String, MisterArcadeMetadata>,
    pub(crate) mister_by_mra_name: HashMap<String, MisterArcadeMetadata>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MisterArcadeMetadata {
    pub(crate) title: String,
    pub(crate) category: String,
    pub(crate) year: Option<u16>,
    pub(crate) manufacturer: String,
    pub(crate) players: Option<u8>,
    pub(crate) control: String,
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

#[cfg(test)]
pub(crate) fn load_mame_machine_metadata(path: &Path) -> HashMap<String, MameMachineMetadata> {
    let Ok(conn) = library_db::open_sqlite_read_only(path) else {
        return HashMap::new();
    };
    let has_players =
        library_db::sqlite_column_exists(&conn, "mame_machines", "players").unwrap_or(false);
    let has_control =
        library_db::sqlite_column_exists(&conn, "mame_machines", "control_type").unwrap_or(false);
    let players_expr = if has_players { "players" } else { "NULL" };
    let control_expr = if has_control { "control_type" } else { "NULL" };
    let sql = format!(
        "SELECT setname,parent_setname,title,year,manufacturer,{players_expr},{control_expr}
         FROM mame_machines"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
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
                players: row.get::<_, Option<i64>>(5)?.and_then(valid_player_count),
                control: row.get(6)?,
            },
        ))
    }) else {
        return HashMap::new();
    };
    rows.filter_map(|row| row.ok()).collect()
}

pub(crate) fn load_mame_machine_metadata_for_setnames(
    path: &Path,
    setnames: &HashSet<String>,
) -> HashMap<String, MameMachineMetadata> {
    if setnames.is_empty() {
        return HashMap::new();
    }
    let Ok(conn) = library_db::open_sqlite_read_only(path) else {
        return HashMap::new();
    };
    let has_players =
        library_db::sqlite_column_exists(&conn, "mame_machines", "players").unwrap_or(false);
    let has_control =
        library_db::sqlite_column_exists(&conn, "mame_machines", "control_type").unwrap_or(false);
    let mut out = HashMap::with_capacity(setnames.len());
    let setnames = setnames.iter().map(String::as_str).collect::<Vec<_>>();
    for chunk in setnames.chunks(400) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let players_expr = if has_players { "players" } else { "NULL" };
        let control_expr = if has_control { "control_type" } else { "NULL" };
        let sql = format!(
            "SELECT setname,parent_setname,title,year,manufacturer,{players_expr},{control_expr}
             FROM mame_machines
             WHERE setname IN ({placeholders})"
        );
        let Ok(mut stmt) = conn.prepare(&sql) else {
            continue;
        };
        let Ok(rows) = stmt.query_map(params_from_iter(chunk.iter().copied()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                MameMachineMetadata {
                    parent_setname: row.get(1)?,
                    title: row.get(2)?,
                    year: row.get(3)?,
                    manufacturer: row.get(4)?,
                    players: row.get::<_, Option<i64>>(5)?.and_then(valid_player_count),
                    control: row.get(6)?,
                },
            ))
        }) else {
            continue;
        };
        for row in rows.flatten() {
            out.insert(row.0, row.1);
        }
    }
    out
}

fn valid_player_count(value: i64) -> Option<u8> {
    u8::try_from(value).ok()
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
    ) && let Ok(rows) = stmt.query_map([], |row| {
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
        })
    {
        for row in rows.flatten() {
            let (raw_list, name, item) = row;
            let list = canonical_software_list_name(&raw_list).to_string();
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
    if let Ok(mut stmt) = conn.prepare(
        "SELECT list_name,software_name,size,crc32
         FROM mame_software_hashes
         WHERE size IS NOT NULL AND crc32 IS NOT NULL",
    ) && let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    }) {
        for (raw_list, name, size, crc_hex) in rows.flatten() {
            let list = canonical_software_list_name(&raw_list).to_string();
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
    if let Ok(mut stmt) = conn.prepare(
        "SELECT list_name,software_name,disk_sha1
         FROM mame_software_hashes
         WHERE disk_sha1 IS NOT NULL",
    ) && let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    }) {
        for (raw_list, name, sha1) in rows.flatten() {
            let list = canonical_software_list_name(&raw_list).to_string();
            metadata
                .disk_index
                .entry((list, sha1.to_ascii_lowercase()))
                .or_default()
                .push(name);
        }
    }
    for members in metadata.family_members.values_mut() {
        members.sort();
        members.dedup();
    }
    metadata
}

#[cfg(test)]
pub(crate) fn load_arcade_machine_metadata(
    mame_path: &Path,
    hbmame_path: &Path,
) -> ArcadeMachineMetadata {
    ArcadeMachineMetadata {
        mame: load_mame_machine_metadata(mame_path),
        hbmame: load_mame_machine_metadata(hbmame_path),
        ..load_mister_arcade_metadata(mame_path)
    }
}

pub(crate) fn load_arcade_machine_metadata_for_setnames(
    mame_path: &Path,
    hbmame_path: &Path,
    setnames: &HashSet<String>,
) -> ArcadeMachineMetadata {
    let total_started = Instant::now();
    let mame_started = Instant::now();
    let mame = load_mame_machine_metadata_for_setnames(mame_path, setnames);
    let mame_us = mame_started.elapsed().as_micros() as u64;
    let hbmame_started = Instant::now();
    let hbmame = load_mame_machine_metadata_for_setnames(hbmame_path, setnames);
    let hbmame_us = hbmame_started.elapsed().as_micros() as u64;
    let mister_started = Instant::now();
    let mister = load_mister_arcade_metadata(mame_path);
    let mister_us = mister_started.elapsed().as_micros() as u64;
    eprintln!(
        "library_scan_timing\tarcade_metadata_sources\t{}\trequested={} mame_rows={} hbmame_rows={} mister_setnames={} mister_mra_names={} mame_us={} hbmame_us={} mister_us={}",
        total_started.elapsed().as_micros(),
        setnames.len(),
        mame.len(),
        hbmame.len(),
        mister.mister_by_setname.len(),
        mister.mister_by_mra_name.len(),
        mame_us,
        hbmame_us,
        mister_us,
    );
    ArcadeMachineMetadata {
        mame,
        hbmame,
        ..mister
    }
}

pub(crate) fn load_arcade_machine_metadata_for_fallbacks(
    mame_path: &Path,
    hbmame_path: &Path,
    setnames: &HashSet<String>,
    mra_names: &HashSet<String>,
) -> ArcadeMachineMetadata {
    let total_started = Instant::now();
    let mame_started = Instant::now();
    let mame = load_mame_machine_metadata_for_setnames(mame_path, setnames);
    let mame_us = mame_started.elapsed().as_micros() as u64;
    let hbmame_started = Instant::now();
    let hbmame = load_mame_machine_metadata_for_setnames(hbmame_path, setnames);
    let hbmame_us = hbmame_started.elapsed().as_micros() as u64;
    let mister_started = Instant::now();
    let mister = load_mister_arcade_metadata_for_keys(mame_path, setnames, mra_names);
    let mister_us = mister_started.elapsed().as_micros() as u64;
    eprintln!(
        "library_scan_timing\tarcade_metadata_sources\t{}\trequested={} mra_names={} mame_rows={} hbmame_rows={} mister_setnames={} mister_mra_names={} mame_us={} hbmame_us={} mister_us={}",
        total_started.elapsed().as_micros(),
        setnames.len(),
        mra_names.len(),
        mame.len(),
        hbmame.len(),
        mister.mister_by_setname.len(),
        mister.mister_by_mra_name.len(),
        mame_us,
        hbmame_us,
        mister_us,
    );
    ArcadeMachineMetadata {
        mame,
        hbmame,
        ..mister
    }
}

fn load_mister_arcade_metadata(path: &Path) -> ArcadeMachineMetadata {
    let Ok(conn) = library_db::open_sqlite_read_only(path) else {
        return ArcadeMachineMetadata::default();
    };
    if !library_db::sqlite_table_exists(&conn, "mister_arcade_entries").unwrap_or(false) {
        return ArcadeMachineMetadata::default();
    }
    let Ok(mut statement) = conn.prepare(
        "SELECT setname_key,mra_name_key,name,category,year,manufacturer,players,
                move_inputs,special_controls
         FROM mister_arcade_entries
         ORDER BY ordinal",
    ) else {
        return ArcadeMachineMetadata::default();
    };
    let Ok(rows) = statement.query_map([], |row| {
        let players = row.get::<_, String>(6)?;
        let move_inputs = row.get::<_, String>(7)?;
        let special_controls = row.get::<_, String>(8)?;
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            MisterArcadeMetadata {
                title: row.get(2)?,
                category: row.get(3)?,
                year: row
                    .get::<_, Option<i64>>(4)?
                    .and_then(|value| u16::try_from(value).ok()),
                manufacturer: row.get(5)?,
                players: leading_player_count(&players),
                control: if special_controls.trim().is_empty() {
                    move_inputs
                } else {
                    special_controls
                },
            },
        ))
    }) else {
        return ArcadeMachineMetadata::default();
    };
    let mut metadata = ArcadeMachineMetadata::default();
    for (setname, mra_name, entry) in rows.flatten() {
        metadata
            .mister_by_setname
            .entry(setname)
            .or_insert_with(|| entry.clone());
        metadata.mister_by_mra_name.insert(mra_name, entry);
    }
    metadata
}

fn load_mister_arcade_metadata_for_keys(
    path: &Path,
    setnames: &HashSet<String>,
    mra_names: &HashSet<String>,
) -> ArcadeMachineMetadata {
    if setnames.is_empty() && mra_names.is_empty() {
        return ArcadeMachineMetadata::default();
    }
    let Ok(conn) = library_db::open_sqlite_read_only(path) else {
        return ArcadeMachineMetadata::default();
    };
    if !library_db::sqlite_table_exists(&conn, "mister_arcade_entries").unwrap_or(false) {
        return ArcadeMachineMetadata::default();
    }
    let mut metadata = ArcadeMachineMetadata::default();
    for (column, values) in [("setname_key", setnames), ("mra_name_key", mra_names)] {
        let mut values = values.iter().map(String::as_str).collect::<Vec<_>>();
        values.sort_unstable();
        for chunk in values.chunks(400) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT setname_key,mra_name_key,name,category,year,manufacturer,players,
                        move_inputs,special_controls
                 FROM mister_arcade_entries
                 WHERE {column} IN ({placeholders})
                 ORDER BY ordinal"
            );
            append_mister_arcade_metadata(&conn, &sql, chunk, &mut metadata);
        }
    }
    metadata
}

fn append_mister_arcade_metadata(
    conn: &Connection,
    sql: &str,
    parameters: &[&str],
    metadata: &mut ArcadeMachineMetadata,
) {
    let Ok(mut statement) = conn.prepare(sql) else {
        return;
    };
    let Ok(rows) = statement.query_map(params_from_iter(parameters.iter().copied()), |row| {
        let players = row.get::<_, String>(6)?;
        let move_inputs = row.get::<_, String>(7)?;
        let special_controls = row.get::<_, String>(8)?;
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            MisterArcadeMetadata {
                title: row.get(2)?,
                category: row.get(3)?,
                year: row
                    .get::<_, Option<i64>>(4)?
                    .and_then(|value| u16::try_from(value).ok()),
                manufacturer: row.get(5)?,
                players: leading_player_count(&players),
                control: if special_controls.trim().is_empty() {
                    move_inputs
                } else {
                    special_controls
                },
            },
        ))
    }) else {
        return;
    };
    for (setname, mra_name, entry) in rows.flatten() {
        metadata
            .mister_by_setname
            .entry(setname)
            .or_insert_with(|| entry.clone());
        metadata.mister_by_mra_name.insert(mra_name, entry);
    }
}

fn leading_player_count(value: &str) -> Option<u8> {
    value
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u8>().ok())
}

pub(crate) fn mister_arcade_metadata_for_discovery<'a>(
    metadata: &'a ArcadeMachineMetadata,
    discovery: &GameDiscovery,
    identity_id: &str,
) -> Option<&'a MisterArcadeMetadata> {
    mister_arcade_metadata_for_path(metadata, &discovery.source_path, identity_id)
}

fn mister_arcade_metadata_for_path<'a>(
    metadata: &'a ArcadeMachineMetadata,
    source_path: &str,
    identity_id: &str,
) -> Option<&'a MisterArcadeMetadata> {
    let mra_name = Path::new(source_path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase);
    mra_name
        .as_deref()
        .and_then(|name| metadata.mister_by_mra_name.get(name))
        .or_else(|| {
            metadata
                .mister_by_setname
                .get(&library_db::normalize_id(identity_id))
        })
}

pub(crate) fn updater_arcade_catalog_metadata(
    source_path: &str,
    header: &crate::mra_header::MraHeader,
    metadata: &ArcadeMachineMetadata,
) -> Option<crate::arcade_updater_index::ArcadeUpdaterCatalogMetadata> {
    let identity_id = header
        .setname
        .as_deref()
        .map(library_db::normalize_id)
        .filter(|identity| !identity.is_empty())?;
    let display_title = header
        .name
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| library_db::title_from_path(source_path));
    let (family_id, _, year, manufacturer, players, control, _) = mame_identity_projection(
        &identity_id,
        metadata,
        header.parent.as_deref(),
        &display_title,
    );
    let mister = mister_arcade_metadata_for_path(metadata, source_path, &identity_id);
    Some(crate::arcade_updater_index::ArcadeUpdaterCatalogMetadata {
        identity_id: identity_id.clone(),
        family_id: if family_id.is_empty() {
            identity_id
        } else {
            family_id
        },
        title: mister
            .filter(|metadata| !metadata.title.is_empty())
            .map(|metadata| metadata.title.clone())
            .unwrap_or(display_title),
        year: mister
            .and_then(|metadata| metadata.year)
            .or_else(|| year.and_then(|value| value.parse::<u16>().ok()))
            .or_else(|| header.year.as_deref().and_then(|value| value.parse().ok())),
        manufacturer: mister
            .filter(|metadata| !metadata.manufacturer.is_empty())
            .map(|metadata| metadata.manufacturer.clone())
            .or_else(|| manufacturer.map(str::to_owned))
            .or_else(|| header.manufacturer.clone())
            .unwrap_or_default(),
        category: mister
            .map(|metadata| metadata.category.clone())
            .unwrap_or_default(),
        players: mister.and_then(|metadata| metadata.players).or(players),
        control: mister
            .filter(|metadata| !metadata.control.is_empty())
            .map(|metadata| metadata.control.clone())
            .or_else(|| control.map(str::to_owned))
            .unwrap_or_default(),
    })
}

pub(crate) fn write_simple_mame_metadata_db(
    path: &Path,
    rows: &MachineMetadataRows,
) -> Result<(), String> {
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
            manufacturer TEXT,
            players INTEGER,
            control_type TEXT
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
                "INSERT INTO mame_machines(setname,parent_setname,title,year,manufacturer,players,control_type)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| format!("prepare metadata insert: {e}"))?;
        for (setname, (parent, title, year, manufacturer, players, control)) in rows {
            stmt.execute(params![
                setname.as_str(),
                parent.as_str(),
                title.as_str(),
                year.as_deref(),
                manufacturer.as_deref(),
                players,
                control.as_deref()
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
        "fds" => Some("fds"),
        "snes" => Some("snes"),
        "n64" => Some("n64"),
        "sms" => Some("sms"),
        "megadrive" => Some("megadriv"),
        "s32x" => Some("32x"),
        "megacd" => Some("megacd"),
        "saturn" => Some("saturn"),
        "atarilynx" => Some("lynx"),
        "amigacd32" => Some("amigacd32"),
        "acornatom" => Some("atom"),
        "acornelectron" => Some("electron"),
        "bbcmicro" => Some("bbc"),
        "archie" => Some("archimedes"),
        "apple-ii" => Some("apple2"),
        "apple-iigs" => Some("apple2gs"),
        "amstrad" => Some("amstrad"),
        "atari2600" => Some("a2600"),
        "atari5200" => Some("a5200"),
        "atari7800" => Some("a7800"),
        "atari800" => Some("a800"),
        "atarist" => Some("atarist"),
        "c64" => Some("c64"),
        "c128" => Some("c128"),
        "c16" => Some("c16"),
        "pet2001" => Some("pet"),
        "vic20" => Some("vic20"),
        "colecovision" => Some("coleco"),
        "megaduck" => Some("megaduck"),
        "wonderswan" => Some("wonderswan"),
        "wonderswancolor" => Some("wsc"),
        "x68000" => Some("x68000"),
        "zx-spectrum" => Some("spectrum"),
        _ => None,
    }
}

/// Collapse the MAME media-specific list names into the stable list namespace
/// used by catalog identities and screenshot asset keys. A platform can expose
/// cartridges, tapes, disks, and ROMs through separate MAME lists; treating
/// them as one namespace keeps family identity and the complete gameplay pack
/// in sync.
pub(crate) fn canonical_software_list_name(list_name: &str) -> &str {
    match list_name {
        "famicom_flop" => "fds",
        "cd32" => "amigacd32",
        "atom_cass" | "atom_flop" | "atom_rom" => "atom",
        "electron_cass" | "electron_flop" | "electron_rom" => "electron",
        "bbc_cass" | "bbc_flop_32016" | "bbc_flop_6502" | "bbc_flop_68000" | "bbc_flop_80186"
        | "bbc_flop_arm" | "bbc_flop_hybrid" | "bbc_flop_torch" | "bbc_flop_z80" | "bbc_hdd"
        | "bbc_rom" | "bbcb_flop" | "bbcb_flop_orig" | "bbcm_cart" | "bbcm_flop" => "bbc",
        "archimedes" | "archimedes_hdd" | "archimedes_rom" => "archimedes",
        "apple2_cass"
        | "apple2_flop_clcracked"
        | "apple2_flop_misc"
        | "apple2_flop_orig"
        | "apple2_rom" => "apple2",
        "apple2gs_flop_clcracked" | "apple2gs_flop_misc" | "apple2gs_flop_orig" => "apple2gs",
        "cpc_cass" | "cpc_flop" | "gx4000" => "amstrad",
        "a2600" | "a2600_cass" => "a2600",
        "a800" | "a800_cass" | "a800_flop" | "xegs" => "a800",
        "st_cart" | "st_flop" | "st_flop_demos" => "atarist",
        "c64_cart" | "c64_cass" | "c64_flop_misc" | "c64_flop_orig" | "c64_quik" => "c64",
        "c128_cart" | "c128_flop" | "c128_rom" => "c128",
        "plus4_cart" | "plus4_cass" | "plus4_flop" | "plus4_quik" => "c16",
        "pet_cass" | "pet_flop" | "pet_hdd" | "pet_quik" => "pet",
        "vic1001_cart" | "vic1001_cass" | "vic1001_flop" => "vic20",
        "coleco" | "coleco_homebrew" => "coleco",
        "wswan" => "wonderswan",
        "wscolor" => "wsc",
        "x68k_flop" => "x68000",
        "spectrum_cart"
        | "spectrum_cass"
        | "spectrum_flop_opus"
        | "spectrum_mgt_flop"
        | "spectrum_microdrive"
        | "spectrum_wafadrive" => "spectrum",
        _ => list_name,
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
    if list_name == "lynx" {
        if let Some(software_name) = hash_matcher(discovery, list_name, metadata) {
            return software_identity_from_metadata(
                list_name,
                &software_name,
                metadata,
                "mame-software",
            );
        }
        return metadata
            .title_index
            .get(&(list_name.to_string(), title_key))
            .filter(|names| !names.is_empty())
            .and_then(|names| {
                software_identity_from_metadata(list_name, &names[0], metadata, "filename")
            });
    }
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
        list_name == "lynx" || library_db::env_bool("MISTER_LIBRARY_SOFTWARE_HASH"),
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
    for (length, crc) in stream_rom_candidate_hashes_from_path(source_path, list_name)? {
        let key = (list_name.to_string(), length, crc);
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

const ROM_IDENTITY_STREAM_BUFFER_BYTES: usize = 256 * 1024;

fn stream_rom_candidate_hashes_from_path(
    source_path: &str,
    list_name: &str,
) -> Option<Vec<(u64, u32)>> {
    let mut file = File::open(source_path).ok()?;
    let expected_size = file.metadata().ok()?.len();
    let mut buffer = vec![0u8; ROM_IDENTITY_STREAM_BUFFER_BYTES];
    let first_bytes = file.read(&mut buffer).ok()?;
    let mut hasher =
        StreamingRomCandidateHasher::new(list_name, expected_size, &buffer[..first_bytes.min(4)]);
    let mut bytes_read = first_bytes as u64;
    if first_bytes > 0 {
        hasher.update(&buffer[..first_bytes]);
        crate::cooperative_work::checkpoint();
    }
    loop {
        let bytes = file.read(&mut buffer).ok()?;
        if bytes == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(bytes as u64);
        hasher.update(&buffer[..bytes]);
        crate::cooperative_work::checkpoint();
    }
    (bytes_read == expected_size).then(|| hasher.finish())
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

pub(crate) fn software_hash_cache_key(
    list_name: &str,
    source_path: &str,
) -> Option<SoftwareHashCacheKey> {
    let signature = library_db::file_signature(Path::new(source_path));
    if signature.size == 0 && signature.mtime_secs == 0 {
        return None;
    }
    Some(SoftwareHashCacheKey {
        list_name: software_hash_cache_namespace(list_name).to_string(),
        file_path: source_path.to_string(),
        size: signature.size,
        mtime_secs: signature.mtime_secs,
    })
}

fn software_hash_cache_namespace(list_name: &str) -> &str {
    match list_name {
        "lynx" => "lynx:v2",
        value => value,
    }
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

#[cfg(test)]
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
        "lynx" => {
            if bytes.len() > 64 && &bytes[..4] == b"LYNX" {
                out.push(bytes[64..].to_vec());
            }
            out.push(bytes.to_vec());
        }
        "sms" | "megadriv" => out.push(bytes.to_vec()),
        _ => out.push(bytes.to_vec()),
    }
    out.dedup();
    out
}

#[cfg(test)]
pub(crate) fn swap_pairs(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for chunk in out.as_chunks_mut::<2>().0 {
        chunk.swap(0, 1);
    }
    out
}

#[cfg(test)]
pub(crate) fn swap_words(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for chunk in out.as_chunks_mut::<4>().0 {
        chunk.swap(0, 2);
        chunk.swap(1, 3);
    }
    out
}

#[cfg(test)]
pub(crate) fn reverse_words(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for chunk in out.as_chunks_mut::<4>().0 {
        chunk.reverse();
    }
    out
}

#[cfg(test)]
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

const CRC32_TABLES: [[u32; 256]; 8] = build_crc32_tables();

const fn build_crc32_tables() -> [[u32; 256]; 8] {
    let mut tables = [[0u32; 256]; 8];
    let mut index = 0usize;
    while index < tables[0].len() {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            bit += 1;
        }
        tables[0][index] = crc;
        index += 1;
    }
    let mut slice = 1usize;
    while slice < tables.len() {
        index = 0;
        while index < tables[slice].len() {
            let crc = tables[slice - 1][index];
            tables[slice][index] = (crc >> 8) ^ tables[0][(crc & 0xff) as usize];
            index += 1;
        }
        slice += 1;
    }
    tables
}

#[derive(Clone, Debug)]
struct IncrementalCrc32 {
    state: u32,
    length: u64,
}

impl Default for IncrementalCrc32 {
    fn default() -> Self {
        Self {
            state: 0xffff_ffff,
            length: 0,
        }
    }
}

impl IncrementalCrc32 {
    fn update(&mut self, bytes: &[u8]) {
        let mut crc = self.state;
        let chunks = bytes.as_chunks::<8>();
        for chunk in chunks.0 {
            crc ^= u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            crc = CRC32_TABLES[7][(crc & 0xff) as usize]
                ^ CRC32_TABLES[6][((crc >> 8) & 0xff) as usize]
                ^ CRC32_TABLES[5][((crc >> 16) & 0xff) as usize]
                ^ CRC32_TABLES[4][((crc >> 24) & 0xff) as usize]
                ^ CRC32_TABLES[3][chunk[4] as usize]
                ^ CRC32_TABLES[2][chunk[5] as usize]
                ^ CRC32_TABLES[1][chunk[6] as usize]
                ^ CRC32_TABLES[0][chunk[7] as usize];
        }
        for &byte in chunks.1 {
            let index = ((crc ^ u32::from(byte)) & 0xff) as usize;
            crc = (crc >> 8) ^ CRC32_TABLES[0][index];
        }
        self.state = crc;
        self.length = self.length.saturating_add(bytes.len() as u64);
    }

    fn finish(&self) -> (u64, u32) {
        (self.length, !self.state)
    }
}

#[derive(Clone, Debug)]
enum StreamingRomCandidateHasher {
    Linear {
        stripped: Option<(usize, IncrementalCrc32)>,
        raw: IncrementalCrc32,
        offset: usize,
    },
    N64 {
        raw: IncrementalCrc32,
        pairs: IncrementalCrc32,
        words: IncrementalCrc32,
        reversed: IncrementalCrc32,
        carry: Vec<u8>,
    },
}

impl StreamingRomCandidateHasher {
    fn new(list_name: &str, file_size: u64, prefix: &[u8]) -> Self {
        if list_name == "n64" {
            return Self::N64 {
                raw: IncrementalCrc32::default(),
                pairs: IncrementalCrc32::default(),
                words: IncrementalCrc32::default(),
                reversed: IncrementalCrc32::default(),
                carry: Vec::with_capacity(3),
            };
        }
        let stripped = match list_name {
            "nes" if file_size > 16 && prefix.starts_with(b"NES\x1a") => Some(16),
            "snes" if file_size > 512 => Some(512),
            "lynx" if file_size > 64 && prefix.starts_with(b"LYNX") => Some(64),
            _ => None,
        }
        .map(|skip| (skip, IncrementalCrc32::default()));
        Self::Linear {
            stripped,
            raw: IncrementalCrc32::default(),
            offset: 0,
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Linear {
                stripped,
                raw,
                offset,
            } => {
                raw.update(bytes);
                if let Some((skip, state)) = stripped {
                    let start = skip.saturating_sub(*offset).min(bytes.len());
                    state.update(&bytes[start..]);
                }
                *offset = offset.saturating_add(bytes.len());
            }
            Self::N64 {
                raw,
                pairs,
                words,
                reversed,
                carry,
            } => {
                raw.update(bytes);
                let mut start = 0usize;
                if !carry.is_empty() {
                    let needed = 4usize.saturating_sub(carry.len());
                    let copied = needed.min(bytes.len());
                    carry.extend_from_slice(&bytes[..copied]);
                    start = copied;
                    if carry.len() == 4 {
                        update_n64_transforms(pairs, words, reversed, carry);
                        carry.clear();
                    }
                }
                let octets = bytes[start..].as_chunks::<8>();
                for chunk in octets.0 {
                    update_n64_transform_octets(pairs, words, reversed, chunk);
                }
                let chunks = octets.1.as_chunks::<4>();
                for chunk in chunks.0 {
                    update_n64_transforms(pairs, words, reversed, chunk);
                }
                carry.extend_from_slice(chunks.1);
            }
        }
    }

    fn finish(mut self) -> Vec<(u64, u32)> {
        let mut candidates = match &mut self {
            Self::Linear { stripped, raw, .. } => {
                let mut candidates = Vec::with_capacity(2);
                if let Some((_, state)) = stripped {
                    candidates.push(state.finish());
                }
                candidates.push(raw.finish());
                candidates
            }
            Self::N64 {
                raw,
                pairs,
                words,
                reversed,
                carry,
            } => {
                if carry.len() >= 2 {
                    pairs.update(&[carry[1], carry[0]]);
                    if carry.len() == 3 {
                        pairs.update(&carry[2..]);
                    }
                } else {
                    pairs.update(carry);
                }
                words.update(carry);
                reversed.update(carry);
                vec![
                    raw.finish(),
                    pairs.finish(),
                    words.finish(),
                    reversed.finish(),
                ]
            }
        };
        candidates.dedup();
        candidates
    }
}

#[inline]
fn update_n64_transforms(
    pairs: &mut IncrementalCrc32,
    words: &mut IncrementalCrc32,
    reversed: &mut IncrementalCrc32,
    chunk: &[u8],
) {
    debug_assert_eq!(chunk.len(), 4);
    pairs.update(&[chunk[1], chunk[0], chunk[3], chunk[2]]);
    words.update(&[chunk[2], chunk[3], chunk[0], chunk[1]]);
    reversed.update(&[chunk[3], chunk[2], chunk[1], chunk[0]]);
}

#[inline]
fn update_n64_transform_octets(
    pairs: &mut IncrementalCrc32,
    words: &mut IncrementalCrc32,
    reversed: &mut IncrementalCrc32,
    chunk: &[u8; 8],
) {
    pairs.update(&[
        chunk[1], chunk[0], chunk[3], chunk[2], chunk[5], chunk[4], chunk[7], chunk[6],
    ]);
    words.update(&[
        chunk[2], chunk[3], chunk[0], chunk[1], chunk[6], chunk[7], chunk[4], chunk[5],
    ]);
    reversed.update(&[
        chunk[3], chunk[2], chunk[1], chunk[0], chunk[7], chunk[6], chunk[5], chunk[4],
    ]);
}

pub(crate) fn parse_hex_u32(value: &str) -> Option<u32> {
    u32::from_str_radix(value.trim(), 16).ok()
}

type MameIdentityProjection<'a> = (
    String,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<u8>,
    Option<&'a str>,
    &'static str,
);

pub(crate) fn mame_identity_projection<'a>(
    identity_id: &str,
    metadata: &'a ArcadeMachineMetadata,
    mra_parent: Option<&str>,
    display_title: &str,
) -> MameIdentityProjection<'a> {
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
            machine.players,
            machine.control.as_deref(),
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
            machine.players,
            machine.control.as_deref(),
            "hbmame",
        )
    } else if let Some(family_id) = normalized_parent_family(mra_parent, identity_id) {
        (family_id, None, None, None, None, None, "mra-parent")
    } else if let Some(parent) = arcade_parent_override(identity_id) {
        (
            parent.to_string(),
            None,
            None,
            None,
            None,
            None,
            "arcade-parent-override",
        )
    } else if let Some(family_id) =
        unique_metadata_family_for_canonical_title(metadata, display_title)
    {
        (family_id, None, None, None, None, None, "canonical-title")
    } else {
        (
            identity_id.to_string(),
            None,
            None,
            None,
            None,
            None,
            "setname",
        )
    }
}

fn unique_metadata_family_for_canonical_title(
    metadata: &ArcadeMachineMetadata,
    display_title: &str,
) -> Option<String> {
    let title_key = library_db::canonical_variant_title(display_title);
    if title_key.is_empty() {
        return None;
    }
    let mut matched_family = None::<String>;
    for machines in [&metadata.mame, &metadata.hbmame] {
        for (setname, machine) in machines {
            if library_db::canonical_variant_title(&machine.title) != title_key {
                continue;
            }
            let family_id = library_db::normalize_id(
                machine
                    .parent_setname
                    .as_deref()
                    .filter(|parent| !parent.trim().is_empty())
                    .unwrap_or(setname),
            );
            if family_id.is_empty() {
                continue;
            }
            match matched_family.as_deref() {
                Some(existing) if existing != family_id => return None,
                Some(_) => {}
                None => matched_family = Some(family_id),
            }
        }
    }
    matched_family
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
    if let Some(id) = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(screenshot_pack_id_from_filename)
    {
        return id.as_str();
    }
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
    } else if path.contains("amiga-screenshots") {
        "amiga"
    } else if path.contains("atarilynx-screenshots") {
        "atarilynx"
    } else if path.contains("saturn") {
        "saturn"
    } else {
        "arcade"
    }
}

#[cfg(test)]
pub(crate) fn software_asset_key(list_name: &str, software_name: &str) -> String {
    ScreenshotAssetId::from_mame_software(list_name, software_name).into_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConsolePreviewAsset {
    pub(crate) archive_path: String,
    pub(crate) asset_key: ScreenshotAssetId,
    pub(crate) has_preview: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PreviewArchivePaths {
    pub(crate) by_platform: HashMap<String, String>,
    entries_by_platform: HashMap<String, HashSet<String>>,
}

impl PreviewArchivePaths {
    pub(crate) fn from_paths(paths: Vec<String>) -> Self {
        let mut by_platform = HashMap::new();
        for path in paths {
            by_platform
                .entry(preview_asset_pack_platform(&path).to_string())
                .or_insert(path);
        }
        Self {
            by_platform,
            entries_by_platform: HashMap::new(),
        }
    }

    pub(crate) fn from_paths_with_sidecar_entries(paths: Vec<String>) -> Self {
        let mut this = Self::from_paths(paths);
        for (platform, path) in &this.by_platform {
            let resolved = preview_worker::resolved_preview_archive_path(path);
            let entries = preview_worker::preview_archive_sidecar_entry_stems(Path::new(&resolved))
                .ok()
                .flatten()
                .map(|stems| stems.entries.into_iter().collect::<HashSet<_>>())
                .unwrap_or_default();
            this.entries_by_platform.insert(platform.clone(), entries);
        }
        this
    }

    #[cfg(test)]
    pub(crate) fn from_preview_indexes(indexes: &[preview_worker::PreviewArchiveIndex]) -> Self {
        let mut this = Self::from_paths(indexes.iter().map(|index| index.path.clone()).collect());
        for index in indexes {
            let platform = preview_asset_pack_platform(&index.path).to_string();
            this.entries_by_platform.insert(
                platform,
                index
                    .entries
                    .iter()
                    .filter_map(|name| {
                        Path::new(name)
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .map(str::to_ascii_lowercase)
                    })
                    .collect(),
            );
        }
        this
    }

    pub(crate) fn archive_for_platform(&self, platform: &str) -> Option<&str> {
        self.by_platform.get(platform).map(String::as_str)
    }

    pub(crate) fn has_entry(&self, platform: &str, asset_key: &str) -> bool {
        if asset_key.is_empty() || self.archive_for_platform(platform).is_none() {
            return false;
        }
        match self.entries_by_platform.get(platform) {
            Some(entries) => entries.contains(&asset_key.to_ascii_lowercase()),
            None => true,
        }
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
    let software_name = identity
        .family_id
        .split_once(':')
        .filter(|(list_name, _)| *list_name == identity.list_name)
        .map(|(_, family_name)| family_name)
        .unwrap_or(identity.software_name.as_str());
    let asset_key = ScreenshotAssetId::from_mame_software(&identity.list_name, software_name);
    let has_preview = preview_paths.has_entry(platform, &asset_key.to_string());
    let archive_path = preview_paths
        .archive_for_platform(platform)
        .map(str::to_owned)
        .unwrap_or_else(|| preview_worker::preview_archive_path_for_system(platform));
    Some(ConsolePreviewAsset {
        archive_path,
        asset_key,
        has_preview,
    })
}

pub(crate) fn preview_platform_for_software_list(list_name: &str) -> &str {
    match list_name {
        "megadriv" => "megadrive",
        "32x" => "s32x",
        "lynx" => "atarilynx",
        "atom" => "acornatom",
        "electron" => "acornelectron",
        "bbc" => "bbcmicro",
        "archimedes" => "archie",
        "apple2" => "apple-ii",
        "apple2gs" => "apple-iigs",
        "a2600" => "atari2600",
        "a5200" => "atari5200",
        "a7800" => "atari7800",
        "a800" => "atari800",
        "pet" => "pet2001",
        "coleco" => "colecovision",
        "wsc" => "wonderswancolor",
        "spectrum" => "zx-spectrum",
        value => value,
    }
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug)]
struct RomIdentityBenchmarkInput {
    list_name: &'static str,
    path: PathBuf,
    size: u64,
}

#[cfg(feature = "builder")]
pub fn rom_identity_benchmark_report() -> Result<serde_json::Value, String> {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use walkdir::WalkDir;

    let report_started = Instant::now();
    let roots = crate::catalog_config::library_roots_from_env();
    let scanner_cache_path = crate::scanner_cache::default_path();
    let software_hash_cache = SoftwareHashCache::load(&scanner_cache_path);
    let software_hash_cache_sha256 = rom_benchmark_cache_digest(&software_hash_cache);
    let production_default_catalog_hash_entries = software_hash_cache
        .entries
        .keys()
        .filter(|key| key.list_name == software_hash_cache_namespace("lynx"))
        .count();
    let selection_started = Instant::now();
    let mut eligible = BTreeMap::<&'static str, Vec<RomIdentityBenchmarkInput>>::new();
    let mut seen_paths = std::collections::BTreeSet::new();
    for key in software_hash_cache
        .entries
        .keys()
        .filter(|key| key.list_name == software_hash_cache_namespace("lynx"))
    {
        let path = PathBuf::from(&key.file_path);
        if key.size > 0 && path.is_file() && seen_paths.insert(path.clone()) {
            eligible
                .entry("lynx")
                .or_default()
                .push(RomIdentityBenchmarkInput {
                    list_name: "lynx",
                    path,
                    size: key.size,
                });
        }
    }
    let scan_roots = rom_benchmark_scan_roots(&roots);
    let mut walk_errors = 0u64;
    for root in &scan_roots {
        for entry in WalkDir::new(root).follow_links(true).max_depth(16) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    walk_errors = walk_errors.saturating_add(1);
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Some(list_name) = rom_benchmark_list_for_path(path) else {
                continue;
            };
            if !seen_paths.insert(path.to_path_buf()) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                walk_errors = walk_errors.saturating_add(1);
                continue;
            };
            if metadata.len() == 0 {
                continue;
            }
            eligible
                .entry(list_name)
                .or_default()
                .push(RomIdentityBenchmarkInput {
                    list_name,
                    path: path.to_path_buf(),
                    size: metadata.len(),
                });
        }
    }
    let eligible_counts = eligible
        .iter()
        .map(|(list, files)| ((*list).to_string(), files.len()))
        .collect::<BTreeMap<_, _>>();
    let production_default_eligible = eligible.get("lynx").map_or(0, Vec::len);
    let mut selected = Vec::new();
    for files in eligible.values_mut() {
        files.sort_by(|left, right| {
            left.size
                .cmp(&right.size)
                .then_with(|| left.path.cmp(&right.path))
        });
        for size_class in ["small", "medium", "large"] {
            let class = files
                .iter()
                .filter(|input| rom_benchmark_size_class(input.size) == size_class)
                .collect::<Vec<_>>();
            if let Some(input) = class.get(class.len() / 2) {
                selected.push((*input).clone());
            }
        }
    }
    selected.sort_by(|left, right| {
        left.list_name
            .cmp(right.list_name)
            .then_with(|| left.size.cmp(&right.size))
            .then_with(|| left.path.cmp(&right.path))
    });
    let selection_us = selection_started.elapsed().as_micros() as u64;
    if selected.is_empty() {
        return Err("ROM identity benchmark found no hash-eligible production files".to_string());
    }

    let metadata_started = Instant::now();
    let metadata = load_mame_software_metadata(&crate::catalog_config::default_mame_sqlite_path());
    let metadata_load_us = metadata_started.elapsed().as_micros() as u64;
    let rss_before_kb = proc_status_kb("VmRSS");
    let hwm_before_kb = proc_status_kb("VmHWM");
    let mut cases = Vec::with_capacity(selected.len());
    let mut result_digest = Sha256::new();
    result_digest.update(software_hash_cache_sha256.as_bytes());
    let mut production_default_selected = 0usize;
    for input in selected {
        if input.list_name == "lynx" {
            production_default_selected = production_default_selected.saturating_add(1);
        }
        let case = benchmark_streaming_rom_identity(&input, &metadata, &software_hash_cache)?;
        result_digest.update(input.list_name.as_bytes());
        result_digest.update(input.path.as_os_str().as_encoded_bytes());
        result_digest.update(input.size.to_le_bytes());
        result_digest.update(
            serde_json::to_vec(&case.get("candidates").cloned().unwrap_or_default())
                .map_err(|error| format!("encode ROM benchmark digest: {error}"))?,
        );
        if let Some(identity) = case.get("identity").and_then(serde_json::Value::as_str) {
            result_digest.update(identity.as_bytes());
        }
        for field in [
            "family_id",
            "matched_candidate_index",
            "matched_candidate_rank",
        ] {
            result_digest.update(
                serde_json::to_vec(&case.get(field).cloned().unwrap_or_default())
                    .map_err(|error| format!("encode ROM benchmark {field}: {error}"))?,
            );
        }
        result_digest.update(
            serde_json::to_vec(&case.get("software_cache").cloned().unwrap_or_default())
                .map_err(|error| format!("encode ROM benchmark cache result: {error}"))?,
        );
        cases.push(case);
    }
    let rss_after_kb = proc_status_kb("VmRSS");
    let hwm_after_kb = proc_status_kb("VmHWM");
    Ok(json!({
        "schema": "mister-magik-rom-identity-benchmark-v1",
        "status": "passed",
        "implementation": "streaming-slicing-by-eight-crc32",
        "production_default_policy": "lynx-only",
        "roots": roots,
        "scan_roots": scan_roots,
        "selection_us": selection_us,
        "metadata_load_us": metadata_load_us,
        "walk_errors": walk_errors,
        "eligible_counts": eligible_counts,
        "production_default_eligible": production_default_eligible,
        "production_default_selected": production_default_selected,
        "production_default_catalog_hash_entries": production_default_catalog_hash_entries,
        "software_hash_cache_path": scanner_cache_path,
        "software_hash_cache_entries": software_hash_cache.entries.len(),
        "software_hash_cache_sha256": software_hash_cache_sha256,
        "case_count": cases.len(),
        "cases": cases,
        "result_sha256": library_db::hex_lower(result_digest.finalize().as_slice()),
        "rss_before_kb": rss_before_kb,
        "rss_after_kb": rss_after_kb,
        "hwm_before_kb": hwm_before_kb,
        "hwm_after_kb": hwm_after_kb,
        "total_us": report_started.elapsed().as_micros() as u64,
    }))
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug)]
struct StreamingHashMetrics {
    open_us: u64,
    read_us: u64,
    process_us: u64,
    checkpoint_us: u64,
    checkpoint_max_us: u64,
    checkpoint_count: u64,
    total_us: u64,
    bytes_read: u64,
    read_calls: u64,
    buffer_allocation_bytes: usize,
}

#[cfg(feature = "builder")]
fn stream_rom_candidate_hashes(
    input: &RomIdentityBenchmarkInput,
) -> Result<(Vec<(u64, u32)>, StreamingHashMetrics), String> {
    let total_started = Instant::now();
    let open_started = Instant::now();
    let mut file = File::open(&input.path)
        .map_err(|error| format!("open benchmark ROM {}: {error}", input.path.display()))?;
    let open_us = open_started.elapsed().as_micros() as u64;
    let mut buffer = vec![0u8; ROM_IDENTITY_STREAM_BUFFER_BYTES];
    let mut read_us = 0u64;
    let mut process_us = 0u64;
    let mut checkpoint_us = 0u64;
    let mut checkpoint_max_us = 0u64;
    let mut checkpoint_count = 0u64;
    let mut bytes_read = 0u64;
    let mut read_calls = 0u64;

    let prefix_bytes = usize::try_from(input.size.min(4)).unwrap_or(4);
    let mut filled = 0usize;
    while filled < prefix_bytes {
        let started = Instant::now();
        let bytes = file
            .read(&mut buffer[filled..])
            .map_err(|error| format!("read benchmark ROM {}: {error}", input.path.display()))?;
        read_us = read_us.saturating_add(started.elapsed().as_micros() as u64);
        read_calls = read_calls.saturating_add(1);
        if bytes == 0 {
            break;
        }
        filled += bytes;
        bytes_read = bytes_read.saturating_add(bytes as u64);
    }
    let mut hasher =
        StreamingRomCandidateHasher::new(input.list_name, input.size, &buffer[..filled.min(4)]);
    if filled > 0 {
        let started = Instant::now();
        hasher.update(&buffer[..filled]);
        process_us = process_us.saturating_add(started.elapsed().as_micros() as u64);
        streaming_hash_checkpoint(
            &mut checkpoint_us,
            &mut checkpoint_max_us,
            &mut checkpoint_count,
        );
    }
    loop {
        let started = Instant::now();
        let bytes = file
            .read(&mut buffer)
            .map_err(|error| format!("read benchmark ROM {}: {error}", input.path.display()))?;
        read_us = read_us.saturating_add(started.elapsed().as_micros() as u64);
        read_calls = read_calls.saturating_add(1);
        if bytes == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(bytes as u64);
        let started = Instant::now();
        hasher.update(&buffer[..bytes]);
        process_us = process_us.saturating_add(started.elapsed().as_micros() as u64);
        streaming_hash_checkpoint(
            &mut checkpoint_us,
            &mut checkpoint_max_us,
            &mut checkpoint_count,
        );
    }
    if bytes_read != input.size {
        return Err(format!(
            "benchmark ROM changed size while streaming {}",
            input.path.display()
        ));
    }
    let started = Instant::now();
    let candidates = hasher.finish();
    process_us = process_us.saturating_add(started.elapsed().as_micros() as u64);
    Ok((
        candidates,
        StreamingHashMetrics {
            open_us,
            read_us,
            process_us,
            checkpoint_us,
            checkpoint_max_us,
            checkpoint_count,
            total_us: total_started.elapsed().as_micros() as u64,
            bytes_read,
            read_calls,
            buffer_allocation_bytes: buffer.capacity(),
        },
    ))
}

#[cfg(feature = "builder")]
fn streaming_hash_checkpoint(total_us: &mut u64, max_us: &mut u64, count: &mut u64) {
    let started = Instant::now();
    crate::cooperative_work::checkpoint();
    let elapsed_us = started.elapsed().as_micros() as u64;
    *total_us = total_us.saturating_add(elapsed_us);
    *max_us = (*max_us).max(elapsed_us);
    *count = count.saturating_add(1);
}

#[cfg(feature = "builder")]
fn benchmark_streaming_rom_identity(
    input: &RomIdentityBenchmarkInput,
    metadata: &MameSoftwareMetadata,
    software_hash_cache: &SoftwareHashCache,
) -> Result<serde_json::Value, String> {
    use serde_json::json;

    let rss_before_kb = proc_status_kb("VmRSS");
    let hwm_before_kb = proc_status_kb("VmHWM");
    let faults_before = process_faults();
    let cpu_start = current_cpu();
    let total_started = Instant::now();
    let (candidate_hashes, metrics) = stream_rom_candidate_hashes(input)?;
    let lookup_started = Instant::now();
    let matched = candidate_hashes
        .iter()
        .enumerate()
        .find_map(|(index, (size, crc))| {
            metadata
                .hash_index
                .get(&(input.list_name.to_string(), *size, *crc))
                .and_then(|names| names.first())
                .cloned()
                .map(|identity| (index, identity))
        });
    let identity = matched.as_ref().map(|(_, identity)| identity.clone());
    let lookup_us = lookup_started.elapsed().as_micros() as u64;
    let family_id = identity.as_ref().and_then(|software_name| {
        metadata
            .items
            .get(&(input.list_name.to_string(), software_name.clone()))
            .map(|item| {
                item.parent_name
                    .as_deref()
                    .filter(|parent| !parent.trim().is_empty())
                    .unwrap_or(software_name)
                    .to_string()
            })
    });
    let cache_key = software_hash_cache_key(input.list_name, input.path.to_string_lossy().as_ref());
    let cached_identity = cache_key
        .as_ref()
        .and_then(|key| software_hash_cache.entries.get(key));
    let total_us = total_started.elapsed().as_micros() as u64;
    let bounded_production_validation = input.list_name == "lynx" && input.size < 4 * 1024 * 1024;
    let pmu = if bounded_production_validation {
        benchmark_streaming_rom_identity_pmu(input, metadata)
    } else {
        json!({
            "available": false,
            "reason": "bounded-to-small-production-default-case",
        })
    };
    let faults_after = process_faults();
    Ok(json!({
        "list_name": input.list_name,
        "path": input.path,
        "size_bytes": input.size,
        "size_class": rom_benchmark_size_class(input.size),
        "production_default": input.list_name == "lynx",
        "production_parity_executed": false,
        "identity": identity,
        "family_id": family_id,
        "matched_candidate_index": matched.as_ref().map(|(index, _)| index),
        "matched_candidate_rank": matched.as_ref().map(|(index, _)| index + 1),
        "software_cache": {
            "key_available": cache_key.is_some(),
            "entry_present": cached_identity.is_some(),
            "identity": cached_identity.cloned().flatten(),
        },
        "candidates": candidate_hashes.iter().enumerate().map(|(index, (size, crc))| json!({
            "index": index,
            "size_bytes": size,
            "crc32": format!("{crc:08x}"),
        })).collect::<Vec<_>>(),
        "metrics": {
            "open_us": metrics.open_us,
            "read_us": metrics.read_us,
            "transform_us": 0,
            "crc_us": metrics.process_us,
            "transform_crc_us": metrics.process_us,
            "lookup_us": lookup_us,
            "total_us": total_us,
            "stream_total_us": metrics.total_us,
            "bytes_read": metrics.bytes_read,
            "read_calls": metrics.read_calls,
            "whole_file_allocation_bytes": 0,
            "candidate_allocation_bytes": 0,
            "read_buffer_allocation_bytes": metrics.buffer_allocation_bytes,
            "checkpoint_count": metrics.checkpoint_count,
            "checkpoint_total_us": metrics.checkpoint_us,
            "checkpoint_max_us": metrics.checkpoint_max_us,
            "minor_page_faults": faults_after.0.saturating_sub(faults_before.0),
            "major_page_faults": faults_after.1.saturating_sub(faults_before.1),
            "rss_before_kb": rss_before_kb,
            "rss_after_kb": proc_status_kb("VmRSS"),
            "hwm_before_kb": hwm_before_kb,
            "hwm_after_kb": proc_status_kb("VmHWM"),
            "cpu_start": cpu_start,
            "cpu_end": current_cpu(),
        },
        "pmu_attribution": pmu,
    }))
}

#[cfg(feature = "builder")]
fn benchmark_streaming_rom_identity_pmu(
    input: &RomIdentityBenchmarkInput,
    metadata: &MameSoftwareMetadata,
) -> serde_json::Value {
    use serde_json::json;

    let (group, diagnostics) = mister_magik_perf_events::CounterGroup::open_with_diagnostics();
    let Ok(group) = group else {
        return json!({"available": false, "diagnostics": diagnostics});
    };
    let Ok(started) = group.snapshot() else {
        return json!({"available": false, "diagnostics": diagnostics});
    };
    let wall_started = Instant::now();
    let result = stream_rom_candidate_hashes(input).map(|(candidates, _)| {
        candidates.iter().find_map(|(size, crc)| {
            metadata
                .hash_index
                .get(&(input.list_name.to_string(), *size, *crc))
                .and_then(|names| names.first())
                .cloned()
        })
    });
    let wall_us = wall_started.elapsed().as_micros() as u64;
    match (result, group.snapshot()) {
        (Ok(identity), Ok(finished)) => {
            let counters = finished.delta_from(started);
            json!({
                "available": true,
                "wall_us": wall_us,
                "identity": identity,
                "ipc": counters.instructions_per_cycle(),
                "counters": counters,
                "diagnostics": diagnostics,
            })
        }
        (Err(error), _) => json!({
            "available": false,
            "error": error,
            "diagnostics": diagnostics,
        }),
        (_, Err(error)) => json!({
            "available": false,
            "error": error.to_string(),
            "diagnostics": diagnostics,
        }),
    }
}

#[cfg(feature = "builder")]
fn rom_benchmark_list_for_path(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "lnx" => Some("lynx"),
        "nes" => Some("nes"),
        "sfc" | "smc" => Some("snes"),
        "z64" | "n64" | "v64" => Some("n64"),
        "md" | "gen" => Some("megadriv"),
        "bin"
            if path
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains("megadrive")
                || path
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("genesis") =>
        {
            Some("megadriv")
        }
        _ => None,
    }
}

#[cfg(feature = "builder")]
fn rom_benchmark_scan_roots(roots: &[String]) -> Vec<PathBuf> {
    let mut candidates = std::collections::BTreeSet::new();
    for root in roots {
        let root = Path::new(root);
        collect_rom_benchmark_system_dirs(root, &mut candidates);
        if root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| canonical_rom_benchmark_dir(name) == "games")
        {
            continue;
        }
        let games = root.join("games");
        if games.is_dir() {
            collect_rom_benchmark_system_dirs(&games, &mut candidates);
        }
    }
    candidates.into_iter().collect()
}

#[cfg(feature = "builder")]
fn collect_rom_benchmark_system_dirs(
    root: &Path,
    candidates: &mut std::collections::BTreeSet<PathBuf>,
) {
    if root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(rom_benchmark_system_dir)
    {
        candidates.insert(root.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir()
            && entry
                .file_name()
                .to_str()
                .is_some_and(rom_benchmark_system_dir)
        {
            candidates.insert(path);
        }
    }
}

#[cfg(feature = "builder")]
fn rom_benchmark_system_dir(name: &str) -> bool {
    matches!(
        canonical_rom_benchmark_dir(name).as_str(),
        "atarilynx"
            | "lynx"
            | "nes"
            | "nintendoentertainmentsystem"
            | "snes"
            | "supernintendo"
            | "supernintendoentertainmentsystem"
            | "n64"
            | "nintendo64"
            | "megadrive"
            | "segamegadrive"
            | "genesis"
            | "segagenesis"
    )
}

#[cfg(feature = "builder")]
fn canonical_rom_benchmark_dir(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(feature = "builder")]
fn rom_benchmark_size_class(size: u64) -> &'static str {
    if size < 4 * 1024 * 1024 {
        "small"
    } else if size < 32 * 1024 * 1024 {
        "medium"
    } else {
        "large"
    }
}

#[cfg(feature = "builder")]
fn rom_benchmark_cache_digest(cache: &SoftwareHashCache) -> String {
    use sha2::{Digest, Sha256};

    let mut entries = cache.entries.iter().collect::<Vec<_>>();
    entries.sort_by(|(left, _), (right, _)| {
        left.list_name
            .cmp(&right.list_name)
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.size.cmp(&right.size))
            .then_with(|| left.mtime_secs.cmp(&right.mtime_secs))
    });
    let mut digest = Sha256::new();
    for (key, identity) in entries {
        digest.update(key.list_name.as_bytes());
        digest.update([0]);
        digest.update(key.file_path.as_bytes());
        digest.update([0]);
        digest.update(key.size.to_le_bytes());
        digest.update(key.mtime_secs.to_le_bytes());
        match identity {
            Some(identity) => {
                digest.update([1]);
                digest.update(identity.as_bytes());
            }
            None => digest.update([0]),
        }
    }
    library_db::hex_lower(digest.finalize().as_slice())
}

#[cfg(feature = "builder")]
fn proc_status_kb(key: &str) -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    text.lines()
        .find_map(|line| {
            let (line_key, rest) = line.split_once(':')?;
            (line_key == key).then(|| {
                rest.split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0)
            })
        })
        .unwrap_or(0)
}

#[cfg(all(feature = "builder", target_os = "linux"))]
fn process_faults() -> (u64, u64) {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the complete rusage structure on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return (0, 0);
    }
    // SAFETY: the successful getrusage call initialized usage.
    let usage = unsafe { usage.assume_init() };
    (
        u64::try_from(usage.ru_minflt).unwrap_or(0),
        u64::try_from(usage.ru_majflt).unwrap_or(0),
    )
}

#[cfg(all(feature = "builder", not(target_os = "linux")))]
fn process_faults() -> (u64, u64) {
    (0, 0)
}

#[cfg(all(feature = "builder", target_os = "linux"))]
fn current_cpu() -> i32 {
    // SAFETY: sched_getcpu has no pointer arguments or caller-side invariants.
    unsafe { libc::sched_getcpu() }
}

#[cfg(all(feature = "builder", not(target_os = "linux")))]
fn current_cpu() -> i32 {
    -1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_db::{file_signature, hex_lower, write_hbmame_metadata_from_library};
    use crate::preview_worker;
    use crate::sqlite_catalog::{
        load_arcade_catalog_from_sqlite_at, save_sqlite_scan, sqlite_table_exists,
        write_sqlite_scan_with_mame, write_sqlite_scan_with_mame_and_hbmame,
        write_sqlite_scan_with_mame_and_preview_pack,
    };
    use crate::test_support::*;

    #[test]
    fn c64_and_zx_spectrum_use_catalog_software_list_identities() {
        assert_eq!(software_list_for_platform("c64"), Some("c64"));
        assert_eq!(software_list_for_platform("zx-spectrum"), Some("spectrum"));
        assert_eq!(preview_platform_for_software_list("c64"), "c64");
        assert_eq!(
            preview_platform_for_software_list("spectrum"),
            "zx-spectrum"
        );

        for (platform, list, title, software) in [
            ("c64", "c64", "California Games", "calgames"),
            ("zx-spectrum", "spectrum", "Manic Miner", "manicmin"),
        ] {
            let mut metadata = MameSoftwareMetadata::default();
            metadata.items.insert(
                (list.to_string(), software.to_string()),
                MameSoftwareItemMetadata {
                    description: title.to_string(),
                    ..Default::default()
                },
            );
            metadata.title_index.insert(
                (list.to_string(), library_db::canonical_variant_title(title)),
                vec![software.to_string()],
            );
            let mut discovery = payload(&format!("/media/fat/games/{platform}/{title}.rom"));
            discovery.platform_id = platform.to_string();
            discovery.title = title.to_string();
            let identity = mame_software_identity_for_discovery_with_hash_matcher(
                &discovery,
                &metadata,
                |_, _, _| None,
            )
            .expect("catalog software identity");
            assert_eq!(identity.list_name, list);
            assert_eq!(identity.software_name, software);
            let preview = console_preview_asset(&identity, &PreviewArchivePaths::default())
                .expect("identity-derived preview asset");
            assert_eq!(
                preview.asset_key.to_string(),
                format!("mame-software__{list}__{software}")
            );
            assert!(
                preview
                    .archive_path
                    .contains(&format!("{platform}-screenshots"))
            );
            assert!(!preview.has_preview);
        }
    }

    #[test]
    fn mame_metadata_collapses_media_lists_into_one_identity_namespace() {
        let root = unique_temp_dir("software-list-canonicalization");
        let db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &db,
            &[
                ("c64_cart", "cartgame", None, "Cart Game", None, None, None),
                (
                    "c64_cass",
                    "cassgame",
                    None,
                    "Cassette Game",
                    None,
                    None,
                    None,
                ),
                (
                    "spectrum_microdrive",
                    "microgame",
                    None,
                    "Microdrive Game",
                    None,
                    None,
                    None,
                ),
            ],
            &[],
        );
        let metadata = load_mame_software_metadata(&db);
        assert!(
            metadata
                .items
                .contains_key(&("c64".to_string(), "cartgame".to_string()))
        );
        assert!(
            metadata
                .items
                .contains_key(&("c64".to_string(), "cassgame".to_string()))
        );
        assert!(
            metadata
                .items
                .contains_key(&("spectrum".to_string(), "microgame".to_string()))
        );
        assert_eq!(canonical_software_list_name("electron_flop"), "electron");
        assert_eq!(canonical_software_list_name("gx4000"), "amstrad");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "builder")]
    #[test]
    fn rom_identity_benchmark_classifies_supported_production_files() {
        assert_eq!(
            rom_benchmark_list_for_path(Path::new("/media/fat/games/Atari Lynx/Game.LNX")),
            Some("lynx")
        );
        assert_eq!(
            rom_benchmark_list_for_path(Path::new("/media/fat/games/NES/Game.nes")),
            Some("nes")
        );
        assert_eq!(
            rom_benchmark_list_for_path(Path::new("/media/fat/games/SNES/Game.smc")),
            Some("snes")
        );
        assert_eq!(
            rom_benchmark_list_for_path(Path::new("/media/fat/games/N64/Game.v64")),
            Some("n64")
        );
        assert_eq!(
            rom_benchmark_list_for_path(Path::new("/media/fat/games/MegaDrive/Game.bin")),
            Some("megadriv")
        );
        assert_eq!(
            rom_benchmark_list_for_path(Path::new("/media/fat/games/SMS/Game.sms")),
            None
        );
    }

    #[cfg(feature = "builder")]
    #[test]
    fn rom_identity_benchmark_uses_fixed_size_classes() {
        assert_eq!(rom_benchmark_size_class(4 * 1024 * 1024 - 1), "small");
        assert_eq!(rom_benchmark_size_class(4 * 1024 * 1024), "medium");
        assert_eq!(rom_benchmark_size_class(32 * 1024 * 1024 - 1), "medium");
        assert_eq!(rom_benchmark_size_class(32 * 1024 * 1024), "large");
    }

    #[cfg(feature = "builder")]
    #[test]
    fn rom_identity_benchmark_scans_only_target_system_directories() {
        let root = unique_temp_dir("rom-identity-scan-roots");
        for directory in ["games/Atari Lynx", "games/SNES", "games/Amiga"] {
            std::fs::create_dir_all(root.join(directory)).expect("create system directory");
        }

        let configured_roots = vec![root.to_string_lossy().into_owned()];
        let roots = rom_benchmark_scan_roots(&configured_roots);

        assert_eq!(
            roots,
            vec![root.join("games/Atari Lynx"), root.join("games/SNES")]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "builder")]
    #[test]
    fn incremental_crc32_matches_the_scalar_oracle() {
        let bytes = (0..4099)
            .map(|index| ((index * 37 + index / 11) & 0xff) as u8)
            .collect::<Vec<_>>();
        for chunk_bytes in [1, 2, 3, 4, 17, 255, 256, 1023] {
            let mut incremental = IncrementalCrc32::default();
            for chunk in bytes.chunks(chunk_bytes) {
                incremental.update(chunk);
            }
            assert_eq!(incremental.finish(), (bytes.len() as u64, crc32(&bytes)));
        }
    }

    #[cfg(feature = "builder")]
    #[test]
    fn streaming_rom_candidates_match_whole_file_candidates_across_boundaries() {
        let mut nes = b"NES\x1a".to_vec();
        nes.extend((4..211).map(|index| (index * 13) as u8));
        let mut lynx = b"LYNX".to_vec();
        lynx.extend((4..193).map(|index| (index * 17) as u8));
        let snes = (0..777).map(|index| (index * 19) as u8).collect::<Vec<_>>();
        let n64 = (0..1031)
            .map(|index| (index * 23) as u8)
            .collect::<Vec<_>>();
        for (list_name, bytes) in [("nes", nes), ("lynx", lynx), ("snes", snes), ("n64", n64)] {
            let expected = rom_hash_candidates(list_name, &bytes)
                .iter()
                .map(|candidate| (candidate.len() as u64, crc32(candidate)))
                .collect::<Vec<_>>();
            for chunk_bytes in [1, 2, 3, 5, 17, 256] {
                let mut streaming = StreamingRomCandidateHasher::new(
                    list_name,
                    bytes.len() as u64,
                    &bytes[..bytes.len().min(4)],
                );
                for chunk in bytes.chunks(chunk_bytes) {
                    streaming.update(chunk);
                }
                assert_eq!(streaming.finish(), expected, "{list_name}/{chunk_bytes}");
            }
        }
    }

    #[test]
    fn production_streaming_rom_file_matches_scalar_candidates() {
        let root = unique_temp_dir("streaming-rom-file");
        std::fs::create_dir_all(&root).expect("create ROM fixture directory");
        let cases = [
            ("nes", b"NES\x1a".as_slice(), 16usize, 131_089usize),
            ("lynx", b"LYNX".as_slice(), 64usize, 262_157usize),
            ("snes", b"".as_slice(), 512usize, 1_048_579usize),
            ("n64", b"".as_slice(), 0usize, 1_048_583usize),
        ];
        for (list_name, prefix, header_bytes, length) in cases {
            let mut bytes = (0..length)
                .map(|index| ((index * 37 + index / 13) & 0xff) as u8)
                .collect::<Vec<_>>();
            bytes[..prefix.len()].copy_from_slice(prefix);
            if header_bytes > prefix.len() {
                bytes[prefix.len()..header_bytes].fill(0x5a);
            }
            let path = root.join(format!("{list_name}.rom"));
            std::fs::write(&path, &bytes).expect("write ROM fixture");
            let expected = rom_hash_candidates(list_name, &bytes)
                .iter()
                .map(|candidate| (candidate.len() as u64, crc32(candidate)))
                .collect::<Vec<_>>();
            let actual =
                stream_rom_candidate_hashes_from_path(path.to_string_lossy().as_ref(), list_name)
                    .expect("stream ROM candidates");
            assert_eq!(actual, expected, "{list_name}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mister_arcade_matching_prefers_mra_filename_then_setname() {
        let filename_match = MisterArcadeMetadata {
            title: "Filename Match".to_string(),
            category: "Shooter".to_string(),
            ..MisterArcadeMetadata::default()
        };
        let setname_match = MisterArcadeMetadata {
            title: "Setname Match".to_string(),
            category: "Maze".to_string(),
            ..MisterArcadeMetadata::default()
        };
        let metadata = ArcadeMachineMetadata {
            mister_by_mra_name: HashMap::from([(
                "special edition.mra".to_string(),
                filename_match.clone(),
            )]),
            mister_by_setname: HashMap::from([("sf2-ce-turbo".to_string(), setname_match.clone())]),
            ..ArcadeMachineMetadata::default()
        };
        let mut discovery = mra_discovery(1, "Special Edition");
        discovery.setname = Some("SF2_CE / Turbo".to_string());

        assert_eq!(
            mister_arcade_metadata_for_discovery(&metadata, &discovery, "sf2-ce-turbo")
                .map(|entry| entry.title.as_str()),
            Some("Filename Match")
        );
        discovery.source_path = "/media/fat/_Arcade/Other Name.mra".to_string();
        assert_eq!(
            mister_arcade_metadata_for_discovery(&metadata, &discovery, "sf2-ce-turbo")
                .map(|entry| entry.title.as_str()),
            Some("Setname Match")
        );
    }

    #[test]
    fn mame_machine_metadata_filter_loads_only_needed_setnames() {
        let root = unique_temp_dir("mame-machine-filter");
        std::fs::create_dir_all(&root).expect("create temp root");
        let db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &db,
            &[
                (
                    "needed",
                    Some("parent"),
                    "Needed Game",
                    Some("1985"),
                    Some("Maker"),
                ),
                ("other", None, "Other Game", Some("1986"), Some("Elsewhere")),
            ],
        );
        let setnames = std::collections::HashSet::from(["needed".to_string()]);

        let metadata = load_mame_machine_metadata_for_setnames(&db, &setnames);

        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata["needed"].title, "Needed Game");
        assert!(!metadata.contains_key("other"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mame_machine_metadata_preserves_zero_players_as_unknown() {
        let root = unique_temp_dir("mame-machine-zero-players");
        std::fs::create_dir_all(&root).expect("create temp root");
        let db = root.join("mame.sqlite3");
        let conn = Connection::open(&db).expect("open MAME fixture");
        conn.execute_batch(
            "CREATE TABLE mame_machines (
                setname TEXT PRIMARY KEY,
                parent_setname TEXT,
                title TEXT NOT NULL,
                year TEXT,
                manufacturer TEXT,
                players INTEGER,
                control_type TEXT
            ) WITHOUT ROWID;
            INSERT INTO mame_machines
                (setname,parent_setname,title,year,manufacturer,players,control_type)
            VALUES
                ('unknown',NULL,'Unknown Players','1980','Example',0,'joy'),
                ('known',NULL,'Known Players','1981','Example',2,'doublejoy');",
        )
        .expect("write MAME fixture");
        drop(conn);

        let metadata = load_mame_machine_metadata(&db);

        assert_eq!(metadata["unknown"].players, Some(0));
        assert_eq!(metadata["known"].players, Some(2));
        assert_eq!(metadata["known"].control.as_deref(), Some("doublejoy"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nes_software_identity_matches_title_and_preview_pack() {
        let root = unique_temp_dir("nes-software-identity");
        let rom_path = root.join("Super Mario Bros.nes");
        let mut rom = b"NES\x1a".to_vec();
        rom.extend_from_slice(&[0; 12]);
        rom.extend_from_slice(b"fixture-rom");
        std::fs::write(&rom_path, &rom).expect("write rom");
        let stripped = &rom[16..];
        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &mame_db,
            &[(
                "nes",
                "smb",
                None,
                "Super Mario Bros. (USA)",
                Some("1985"),
                Some("Nintendo"),
                Some("usa"),
            )],
            &[("nes", "smb", stripped.len() as i64, crc32(stripped))],
        );
        let db = root.join("library.sqlite3");
        let mut discovery = payload(&rom_path.display().to_string());
        discovery.platform_id = "nes".to_string();
        discovery.category = "Console".to_string();
        discovery.core_id = "NES".to_string();
        discovery.hardware_id = "nes".to_string();
        discovery.title = "Super Mario Bros. (USA)".to_string();
        let pack = preview_worker::PreviewArchiveIndex {
            path: "/media/fat/mister-magik/assets/nes-screenshots.mmlz4b".to_string(),
            codec: "mmlz4b",
            entries: vec![software_asset_key("nes", "smb")],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let row: (String, String, String, String, i64, String) = conn
            .query_row(
                "SELECT i.namespace,i.identity_id,i.family_id,l.preview_asset_key,l.has_preview,r.confidence
                 FROM launchable_identities i
                 JOIN launchables lb ON lb.launchable_id=i.launchable_id
                 JOIN launcher_catalog_text l ON l.launch_ref=lb.launch_ref
                 JOIN region_metadata r ON r.game_id=i.launchable_id
                 WHERE i.namespace='mame-software'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("software identity row");

        assert_eq!(
            row,
            (
                "mame-software".to_string(),
                "nes:smb".to_string(),
                "nes:smb".to_string(),
                software_asset_key("nes", "smb"),
                1,
                "filename".to_string()
            )
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn software_identity_title_match_skips_content_hash() {
        let mut metadata = MameSoftwareMetadata::default();
        metadata.items.insert(
            ("snes".to_string(), "example".to_string()),
            MameSoftwareItemMetadata {
                description: "Example Game (USA)".to_string(),
                year: Some("1992".to_string()),
                publisher: Some("Example".to_string()),
                region: Some("usa".to_string()),
                parent_name: None,
            },
        );
        metadata.title_index.insert(
            ("snes".to_string(), "example-game".to_string()),
            vec!["example".to_string()],
        );
        let mut discovery = payload("/media/fat/games/SNES/Example Game (USA).sfc");
        discovery.platform_id = "snes".to_string();
        discovery.title = "Example Game (USA)".to_string();

        let identity = mame_software_identity_for_discovery_with_hash_matcher(
            &discovery,
            &metadata,
            |_, _, _| panic!("title match should not read or hash payload content"),
        )
        .expect("title identity");

        assert_eq!(identity.list_name, "snes");
        assert_eq!(identity.software_name, "example");
        assert_eq!(identity.source, "filename");
    }

    #[test]
    fn lynx_software_identity_prefers_content_hash_over_title() {
        let mut metadata = MameSoftwareMetadata::default();
        for (name, description) in [
            ("titlematch", "Example Game (USA)"),
            ("hashmatch", "Example Game (Europe)"),
        ] {
            metadata.items.insert(
                ("lynx".to_string(), name.to_string()),
                MameSoftwareItemMetadata {
                    description: description.to_string(),
                    ..Default::default()
                },
            );
        }
        metadata.title_index.insert(
            ("lynx".to_string(), "example-game".to_string()),
            vec!["titlematch".to_string()],
        );
        let mut discovery = payload("/media/fat/games/AtariLynx/Example Game (USA).lyx");
        discovery.platform_id = "atarilynx".to_string();
        discovery.title = "Example Game (USA)".to_string();

        let identity = mame_software_identity_for_discovery_with_hash_matcher(
            &discovery,
            &metadata,
            |_, list_name, _| {
                assert_eq!(list_name, "lynx");
                Some("hashmatch".to_string())
            },
        )
        .expect("Lynx software identity");

        assert_eq!(identity.software_name, "hashmatch");
        assert_eq!(identity.source, "mame-software");
    }

    #[test]
    fn lynx_software_identity_falls_back_to_normalized_title_after_hash_miss() {
        let mut metadata = MameSoftwareMetadata::default();
        metadata.items.insert(
            ("lynx".to_string(), "beast".to_string()),
            MameSoftwareItemMetadata {
                description: "Shadow of the Beast (Europe, USA)".to_string(),
                ..Default::default()
            },
        );
        metadata.title_index.insert(
            ("lynx".to_string(), "shadow-of-the-beast".to_string()),
            vec!["beast".to_string()],
        );
        let mut discovery =
            payload("/media/fat/games/AtariLynx/Shadow of the Beast (USA, Europe).lyx");
        discovery.platform_id = "atarilynx".to_string();
        discovery.title = "Shadow of the Beast (USA, Europe)".to_string();

        let identity = mame_software_identity_for_discovery_with_hash_matcher(
            &discovery,
            &metadata,
            |_, _, _| None,
        )
        .expect("title fallback identity");

        assert_eq!(identity.software_name, "beast");
        assert_eq!(identity.source, "filename");
    }

    #[test]
    fn unmatched_lynx_software_has_no_identity() {
        let mut discovery = payload("/media/fat/games/AtariLynx/Unknown Homebrew.lyx");
        discovery.platform_id = "atarilynx".to_string();
        discovery.title = "Unknown Homebrew".to_string();

        assert!(
            mame_software_identity_for_discovery_with_hash_matcher(
                &discovery,
                &MameSoftwareMetadata::default(),
                |_, _, _| None,
            )
            .is_none()
        );
    }

    #[test]
    fn software_identity_hash_match_is_disabled_by_default() {
        let root = unique_temp_dir("software-hash-disabled");
        let rom_path = root.join("Fixture.sfc");
        std::fs::write(&rom_path, b"fixture-rom").expect("write rom");
        let mut metadata = MameSoftwareMetadata::default();
        metadata.hash_index.insert(
            ("snes".to_string(), 11, crc32(b"fixture-rom")),
            vec!["fixture".to_string()],
        );
        let discovery = payload(&rom_path.display().to_string());

        let mut cache = SoftwareHashCache::default();
        let matched = match_software_by_file_hash_with_cache(
            &discovery, "snes", &metadata, false, &mut cache,
        );

        assert_eq!(matched, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn software_identity_hash_match_can_be_enabled() {
        let root = unique_temp_dir("software-hash-enabled");
        let rom_path = root.join("Fixture.sfc");
        std::fs::write(&rom_path, b"fixture-rom").expect("write rom");
        let mut metadata = MameSoftwareMetadata::default();
        metadata.hash_index.insert(
            ("snes".to_string(), 11, crc32(b"fixture-rom")),
            vec!["fixture".to_string()],
        );
        let discovery = payload(&rom_path.display().to_string());

        let mut cache = SoftwareHashCache::default();
        let matched =
            match_software_by_file_hash_with_cache(&discovery, "snes", &metadata, true, &mut cache);

        assert_eq!(matched, Some("fixture".to_string()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lynx_hash_match_projects_family_screenshot_key_without_global_opt_in() {
        let root = unique_temp_dir("lynx-default-hash-preview");
        let rom_path = root.join("Collection Name.lyx");
        std::fs::write(&rom_path, b"raw-lynx-rom").expect("write Lynx ROM");
        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &mame_db,
            &[
                (
                    "lynx",
                    "parent",
                    None,
                    "Example Lynx Game (USA)",
                    Some("1991"),
                    Some("Example"),
                    Some("usa"),
                ),
                (
                    "lynx",
                    "child",
                    Some("parent"),
                    "Example Lynx Game (Europe)",
                    Some("1991"),
                    Some("Example"),
                    Some("europe"),
                ),
            ],
            &[("lynx", "child", 12, crc32(b"raw-lynx-rom"))],
        );
        let db = root.join("library.sqlite3");
        let mut discovery = payload(&rom_path.display().to_string());
        discovery.platform_id = "atarilynx".to_string();
        discovery.category = "Console".to_string();
        discovery.core_id = "AtariLynx".to_string();
        discovery.hardware_id = "atarilynx".to_string();
        discovery.title = "Collection Name".to_string();
        let pack = preview_worker::PreviewArchiveIndex {
            path: "/media/fat/mister-magik/assets/atarilynx-screenshots-160x102.mmlz4b".to_string(),
            codec: "mmlz4b",
            entries: vec![software_asset_key("lynx", "parent")],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
            &pack,
        )
        .expect("save Lynx catalog");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let row: (String, String, String, i64) = conn
            .query_row(
                "SELECT i.identity_id,i.family_id,l.preview_asset_key,l.has_preview
                 FROM launchable_identities i
                 JOIN launchables lb ON lb.launchable_id=i.launchable_id
                 JOIN launcher_catalog_text l ON l.launch_ref=lb.launch_ref
                 WHERE i.namespace='mame-software'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("Lynx software identity row");

        assert_eq!(row.0, "lynx:child");
        assert_eq!(row.1, "lynx:parent");
        assert_eq!(row.2, software_asset_key("lynx", "parent"));
        assert_eq!(row.3, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lynx_projection_hides_variants_but_retains_games_and_launch_plans() {
        let root = unique_temp_dir("lynx-hidden-variants");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(&mame_db, &[], &[]);
        let mut first = payload("/media/fat/games/AtariLynx/Alien (World) (v1.02).lnx");
        first.platform_id = "atarilynx".to_string();
        first.title = "Alien (World) (v1.02)".to_string();
        let mut second = payload("/media/fat/games/AtariLynx/Alien (World) (v1.06).lnx");
        second.platform_id = "atarilynx".to_string();
        second.title = "Alien (World) (v1.06)".to_string();
        let pack = preview_worker::PreviewArchiveIndex {
            path: "/media/fat/mister-magik/assets/atarilynx-screenshots.mmlz4b".to_string(),
            codec: "mmlz4b",
            entries: Vec::new(),
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![first, second]),
            &mame_db,
            &pack,
        )
        .expect("save collapsed Lynx catalog");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        for (table, expected) in [
            ("game_rows", 2),
            ("launch_target_rows", 2),
            ("launch_plans", 2),
            ("launcher_catalog_rows", 1),
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("count Lynx projection rows");
            assert_eq!(count, expected, "{table}");
        }
        let title: String = conn
            .query_row("SELECT title FROM launcher_catalog_text", [], |row| {
                row.get(0)
            })
            .expect("preferred Lynx title");
        assert_eq!(title, "Alien (World) (v1.06)");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn software_identity_hash_cache_hit_avoids_file_read() {
        let root = unique_temp_dir("software-hash-cache-hit");
        let payload_dir = root.join("Cached.sfc");
        std::fs::create_dir(&payload_dir).expect("create payload dir");
        let signature = file_signature(&payload_dir);
        let db = root.join("library.sqlite3");
        write_software_hash_cache_fixture(
            &db,
            &[(
                "snes",
                &payload_dir.display().to_string(),
                signature.size,
                signature.mtime_secs,
                Some("cached"),
            )],
        );
        let mut cache = SoftwareHashCache::load(&db);
        let mut metadata = MameSoftwareMetadata::default();
        metadata
            .hash_index
            .insert(("snes".to_string(), 123, 456), vec!["wrong".to_string()]);
        let discovery = payload(&payload_dir.display().to_string());

        let matched =
            match_software_by_file_hash_with_cache(&discovery, "snes", &metadata, true, &mut cache);

        assert_eq!(matched, Some("cached".to_string()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn software_identity_hash_cache_stale_signature_recomputes() {
        let root = unique_temp_dir("software-hash-cache-stale");
        let rom_path = root.join("Fixture.sfc");
        std::fs::write(&rom_path, b"fresh-rom").expect("write rom");
        let signature = file_signature(&rom_path);
        let db = root.join("library.sqlite3");
        write_software_hash_cache_fixture(
            &db,
            &[(
                "snes",
                &rom_path.display().to_string(),
                signature.size + 1,
                signature.mtime_secs,
                Some("stale"),
            )],
        );
        let mut cache = SoftwareHashCache::load(&db);
        let mut metadata = MameSoftwareMetadata::default();
        metadata.hash_index.insert(
            ("snes".to_string(), 9, crc32(b"fresh-rom")),
            vec!["fresh".to_string()],
        );
        let discovery = payload(&rom_path.display().to_string());

        let matched =
            match_software_by_file_hash_with_cache(&discovery, "snes", &metadata, true, &mut cache);

        assert_eq!(matched, Some("fresh".to_string()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lynx_hash_policy_ignores_cache_rows_from_the_old_algorithm() {
        let root = unique_temp_dir("lynx-versioned-hash-cache");
        let rom_path = root.join("Fixture.lyx");
        std::fs::write(&rom_path, b"fresh-lynx").expect("write ROM");
        let signature = file_signature(&rom_path);
        let db = root.join("library.sqlite3");
        write_software_hash_cache_fixture(
            &db,
            &[(
                "lynx",
                &rom_path.display().to_string(),
                signature.size,
                signature.mtime_secs,
                None,
            )],
        );
        let mut cache = SoftwareHashCache::load(&db);
        let mut metadata = MameSoftwareMetadata::default();
        metadata.hash_index.insert(
            ("lynx".to_string(), 10, crc32(b"fresh-lynx")),
            vec!["fresh".to_string()],
        );
        let discovery = payload(&rom_path.display().to_string());

        let matched =
            match_software_by_file_hash_with_cache(&discovery, "lynx", &metadata, true, &mut cache);

        assert_eq!(matched, Some("fresh".to_string()));
        assert!(cache.entries.keys().any(|key| key.list_name == "lynx:v2"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn console_preview_uses_parent_family_fallback() {
        let root = unique_temp_dir("software-family-preview");
        let rom_path = root.join("Variant.sfc");
        std::fs::write(&rom_path, b"variant-rom").expect("write rom");
        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &mame_db,
            &[
                (
                    "snes",
                    "parent",
                    None,
                    "Example Game (USA)",
                    Some("1992"),
                    Some("Example"),
                    Some("usa"),
                ),
                (
                    "snes",
                    "child",
                    Some("parent"),
                    "Example Game (Rev 1) (USA)",
                    Some("1992"),
                    Some("Example"),
                    Some("usa"),
                ),
            ],
            &[("snes", "child", 11, crc32(b"variant-rom"))],
        );
        let db = root.join("library.sqlite3");
        let mut discovery = payload(&rom_path.display().to_string());
        discovery.platform_id = "snes".to_string();
        discovery.category = "Console".to_string();
        discovery.core_id = "SNES".to_string();
        discovery.hardware_id = "snes".to_string();
        discovery.title = "Example Game (Rev 1) (USA)".to_string();
        let pack = preview_worker::PreviewArchiveIndex {
            path: "/media/fat/mister-magik/assets/snes-screenshots.mmlz4b".to_string(),
            codec: "mmlz4b",
            entries: vec![software_asset_key("snes", "parent")],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let row: (String, i64, String) = conn
            .query_row(
                "SELECT preview_asset_key,has_preview,system_id FROM launcher_catalog",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("launcher row");

        assert_eq!(row.0, software_asset_key("snes", "parent"));
        assert_eq!(row.1, 1);
        assert_eq!(row.2, "snes");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn console_preview_pack_platform_distinguishes_nes_and_snes() {
        assert_eq!(
            preview_asset_pack_platform("/media/fat/mister-magik/assets/nes-screenshots.mmlz4b"),
            "nes"
        );
        assert_eq!(
            preview_asset_pack_platform("/media/fat/mister-magik/assets/snes-screenshots.mmlz4b"),
            "snes"
        );
    }

    #[test]
    fn console_preview_derives_key_but_requires_index_membership() {
        let root = unique_temp_dir("derived-console-preview");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &mame_db,
            &[(
                "saturn",
                "albert",
                None,
                "Albert Odyssey: Legend of Eldean (USA)",
                Some("1997"),
                Some("Working Designs"),
                Some("usa"),
            )],
            &[],
        );
        let mut discovery = saturn_payload("/media/fat/games/Saturn/Albert Odyssey.chd");
        discovery.title = "Albert Odyssey: Legend of Eldean (USA)".to_string();
        let pack = preview_worker::PreviewArchiveIndex {
            path: "/media/fat/mister-magik/assets/saturn-screenshots.mmlz4b".to_string(),
            codec: "mmlz4b",
            entries: vec!["albert-odyssey-legend-of-eldean-us".to_string()],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let row: (String, i64) = conn
            .query_row(
                "SELECT l.preview_asset_key,l.has_preview
                 FROM launcher_catalog l
                 WHERE l.system_id='saturn'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("launcher row");

        assert_eq!(row, (software_asset_key("saturn", "albert"), 0));
        assert!(!sqlite_table_exists(&conn, "asset_entries").expect("check asset_entries table"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn console_preview_ignores_wrong_system_canonical_entries() {
        let root = unique_temp_dir("wrong-system-console-preview");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        let rom_path = root.join("Fixture.nes");
        std::fs::write(&rom_path, b"fixture-rom").expect("write rom");
        write_mame_software_fixture_db(
            &mame_db,
            &[(
                "nes",
                "fixture",
                None,
                "Fixture Game (USA)",
                Some("1985"),
                Some("Example"),
                Some("usa"),
            )],
            &[("nes", "fixture", 11, crc32(b"fixture-rom"))],
        );
        let mut discovery = payload(&rom_path.display().to_string());
        discovery.platform_id = "nes".to_string();
        discovery.category = "Console".to_string();
        discovery.core_id = "NES".to_string();
        discovery.hardware_id = "nes".to_string();
        let pack = preview_worker::PreviewArchiveIndex {
            path: "/media/fat/mister-magik/assets/saturn-screenshots.mmlz4b".to_string(),
            codec: "mmlz4b",
            entries: vec![software_asset_key("nes", "fixture")],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let row: (String, i64) = conn
            .query_row(
                "SELECT preview_asset_key,has_preview FROM launcher_catalog",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("launcher row");

        assert_eq!(row, (String::new(), 0));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn saturn_software_identity_matches_chd_raw_sha1() {
        let root = unique_temp_dir("saturn-chd-identity");
        let chd_path = root.join("Disc.chd");
        let sha1 = [0x42u8; 20];
        let mut header = [0u8; 124];
        header[..8].copy_from_slice(b"MComprHD");
        header[8..12].copy_from_slice(&124u32.to_be_bytes());
        header[12..16].copy_from_slice(&5u32.to_be_bytes());
        header[56..60].copy_from_slice(&4096u32.to_be_bytes());
        header[60..64].copy_from_slice(&2448u32.to_be_bytes());
        header[64..84].copy_from_slice(&sha1);
        std::fs::write(&chd_path, header).expect("write chd header");
        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &mame_db,
            &[(
                "saturn",
                "nights",
                None,
                "Nights into Dreams (USA)",
                Some("1996"),
                Some("Sega"),
                Some("usa"),
            )],
            &[],
        );
        let conn = Connection::open(&mame_db).expect("open mame fixture");
        conn.execute(
            "INSERT INTO mame_software_hashes(list_name,software_name,disk_sha1)
             VALUES ('saturn','nights',?1)",
            [hex_lower(&sha1)],
        )
        .expect("insert disk hash");
        drop(conn);
        let db = root.join("library.sqlite3");
        let mut discovery = saturn_payload(&chd_path.display().to_string());
        discovery.title = "Untrusted Scraper Name".to_string();

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let identity: String = conn
            .query_row(
                "SELECT identity_id FROM launchable_identities WHERE namespace='mame-software'",
                [],
                |row| row.get(0),
            )
            .expect("software identity");
        assert_eq!(identity, "saturn:nights");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn saturn_multidisc_software_identity_materializes_one_launcher_game() {
        let root = unique_temp_dir("saturn-multidisc-identity");
        let disc1_path = root.join("Fixture RPG (Disc 1).chd");
        let disc2_path = root.join("Fixture RPG (Disc 2).chd");
        let sha1_disc1 = [0x41u8; 20];
        let sha1_disc2 = [0x42u8; 20];
        std::fs::write(&disc1_path, chd_v5_header(sha1_disc1)).expect("write disc 1 chd");
        std::fs::write(&disc2_path, chd_v5_header(sha1_disc2)).expect("write disc 2 chd");

        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &mame_db,
            &[(
                "saturn",
                "fixturerpg",
                None,
                "Fixture RPG (USA)",
                Some("1997"),
                Some("Example"),
                Some("usa"),
            )],
            &[],
        );
        let conn = Connection::open(&mame_db).expect("open mame fixture");
        conn.execute(
            "INSERT INTO mame_software_hashes(list_name,software_name,disk_sha1)
             VALUES ('saturn','fixturerpg',?1)",
            [hex_lower(&sha1_disc1)],
        )
        .expect("insert disc 1 hash");
        conn.execute(
            "INSERT INTO mame_software_hashes(list_name,software_name,disk_sha1)
             VALUES ('saturn','fixturerpg',?1)",
            [hex_lower(&sha1_disc2)],
        )
        .expect("insert disc 2 hash");
        drop(conn);

        let mut disc1 = saturn_payload(&disc1_path.display().to_string());
        disc1.title = "Fixture RPG Disc 1".to_string();
        let mut disc2 = saturn_payload(&disc2_path.display().to_string());
        disc2.title = "Fixture RPG Disc 2".to_string();
        let db = root.join("library.sqlite3");

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![disc2, disc1]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let launcher: (i64, String, String) = conn
            .query_row(
                "SELECT
                    (SELECT count(*) FROM launcher_catalog WHERE system_id='saturn'),
                    title,
                    launch_ref
                 FROM launcher_catalog_text
                 WHERE system_id='saturn'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("saturn launcher row");
        let identity_count: i64 = conn
            .query_row(
                "SELECT count(*)
                 FROM launchable_identities
                 WHERE namespace='mame-software'
                   AND identity_id='saturn:fixturerpg'",
                [],
                |row| row.get(0),
            )
            .expect("software identity count");

        assert_eq!(launcher.0, 1);
        assert_eq!(launcher.1, "Fixture RPG Disc 1");
        assert!(launcher.2.ends_with("Fixture RPG (Disc 1).chd"));
        assert_eq!(identity_count, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rom_normalization_covers_lynx_headers_snes_and_n64_byte_orders() {
        let lynx_payload = b"raw-lynx-rom";
        let mut lnx = vec![0; 64];
        lnx[..4].copy_from_slice(b"LYNX");
        lnx.extend_from_slice(lynx_payload);
        let lynx_candidates = rom_hash_candidates("lynx", &lnx);
        assert_eq!(
            lynx_candidates.first().map(Vec::as_slice),
            Some(&lynx_payload[..])
        );
        assert!(lynx_candidates.iter().any(|candidate| candidate == &lnx));

        let snes = [0xaa; 512]
            .into_iter()
            .chain(b"plain-snes".iter().copied())
            .collect::<Vec<_>>();
        assert!(
            rom_hash_candidates("snes", &snes)
                .iter()
                .any(|candidate| candidate == b"plain-snes")
        );

        let z64 = [0x12, 0x34, 0x56, 0x78];
        let candidates = rom_hash_candidates("n64", &z64);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate == &[0x34, 0x12, 0x78, 0x56])
        );
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate == &[0x56, 0x78, 0x12, 0x34])
        );
    }

    #[test]
    fn arcade_mra_identity_uses_mame_parent_family() {
        let root = unique_temp_dir("arcade-mame-identity");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                (
                    "1942",
                    None,
                    "1942 (Revision B)",
                    Some("1984"),
                    Some("Capcom"),
                ),
                (
                    "1942b",
                    Some("1942"),
                    "1942 (First Version)",
                    Some("1984"),
                    Some("Capcom"),
                ),
            ],
        );
        let mut discovery = mra_discovery(1, "1942 (First Version)");
        discovery.setname = Some("1942b".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let row = conn
            .query_row(
                "SELECT l.system_id,l.launch_kind,i.identity_id,i.family_id,i.metadata_title,i.year,i.manufacturer,i.source
                 FROM launchables l
                 JOIN launchable_identities i ON i.launchable_id=l.launchable_id",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .expect("query identity row");

        assert_eq!(row.0, "arcade");
        assert_eq!(row.1, "mra");
        assert_eq!(row.2, "1942b");
        assert_eq!(row.3, "1942");
        assert_eq!(row.4.as_deref(), Some("1942 (First Version)"));
        assert_eq!(row.5.as_deref(), Some("1984"));
        assert_eq!(row.6.as_deref(), Some("Capcom"));
        assert_eq!(row.7, "mame");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn neogeo_mgl_identity_uses_mame_setname() {
        let root = unique_temp_dir("neogeo-mame-identity");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[(
                "mslug3",
                None,
                "Metal Slug 3 (NGM-2560)",
                Some("2000"),
                Some("SNK"),
            )],
        );
        let path = "/media/fat/_Games/_Neo Geo MVS & AES/Metal Slug 3 (mslug3).mgl";
        let mut discovery = mgl(path, path);
        discovery.title = "Metal Slug 3".to_string();
        discovery.platform_id = "neogeo".to_string();
        discovery.core_id = "neogeo".to_string();
        discovery.hardware_id = "neogeo".to_string();
        discovery.setname = Some("mslug3".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let row = conn
            .query_row(
                "SELECT identity_id,family_id,metadata_title,year,manufacturer,source
                 FROM launchable_identities",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .expect("query identity row");

        assert_eq!(row.0, "mslug3");
        assert_eq!(row.1, "mslug3");
        assert_eq!(row.2.as_deref(), Some("Metal Slug 3 (NGM-2560)"));
        assert_eq!(row.3.as_deref(), Some("2000"));
        assert_eq!(row.4.as_deref(), Some("SNK"));
        assert_eq!(row.5, "mame");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_mame_identity_remains_launchable_without_enrichment() {
        let root = unique_temp_dir("unknown-mame-identity");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(&mame_db, &[]);
        let mut discovery = mra_discovery(1, "Mystery Arcade Game");
        discovery.setname = Some("mystery".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let launchable_count: i64 = conn
            .query_row("SELECT count(*) FROM launchables", [], |row| row.get(0))
            .expect("query launchable count");
        let row = conn
            .query_row(
                "SELECT identity_id,family_id,metadata_title,year,manufacturer,source
                 FROM launchable_identities",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .expect("query identity row");

        assert_eq!(launchable_count, 1);
        assert_eq!(row.0, "mystery");
        assert_eq!(row.1, "mystery");
        assert!(row.2.is_none());
        assert!(row.3.is_none());
        assert!(row.4.is_none());
        assert_eq!(row.5, "setname");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_arcade_set_collapses_into_unique_metadata_family_with_same_canonical_title() {
        let root = unique_temp_dir("arcade-title-family-fallback");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[(
                "cybots",
                None,
                "Cyberbots: Fullmetal Madness (Europe 950424)",
                Some("1995"),
                Some("Capcom"),
            )],
        );
        let mut parent = mra_discovery(1, "Cyberbots: Fullmetal Madness (Europe 950424)");
        parent.setname = Some("cybots".to_string());
        let mut access_mod =
            mra_discovery(2, "Cyberbots: Fullmetal Madness (Euro 950424 Access Mod)");
        access_mod.setname = Some("cybotsam".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![parent, access_mod]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let access_identity = conn
            .query_row(
                "SELECT family_id,source
                 FROM launchable_identities
                 WHERE identity_id='cybotsam'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("query access mod identity");
        let preferred_count: i64 = conn
            .query_row("SELECT count(*) FROM ui_arcade_preferred", [], |row| {
                row.get(0)
            })
            .expect("query preferred count");
        let variant_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants WHERE family_id='cybots'",
                [],
                |row| row.get(0),
            )
            .expect("query variant count");

        assert_eq!(access_identity.0, "cybots");
        assert_eq!(access_identity.1, "canonical-title");
        assert_eq!(preferred_count, 1);
        assert_eq!(variant_count, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_arcade_set_does_not_collapse_when_canonical_title_is_ambiguous() {
        let root = unique_temp_dir("arcade-title-family-ambiguous");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                ("examplea", None, "Example Game (Set A)", None, None),
                ("exampleb", None, "Example Game (Set B)", None, None),
            ],
        );
        let mut first = mra_discovery(1, "Example Game (Set A)");
        first.setname = Some("examplea".to_string());
        let mut second = mra_discovery(2, "Example Game (Set B)");
        second.setname = Some("exampleb".to_string());
        let mut unknown = mra_discovery(3, "Example Game (Unknown Mod)");
        unknown.setname = Some("examplemod".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![first, second, unknown]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let unknown_identity = conn
            .query_row(
                "SELECT family_id,source
                 FROM launchable_identities
                 WHERE identity_id='examplemod'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("query unknown identity");

        assert_eq!(unknown_identity.0, "examplemod");
        assert_eq!(unknown_identity.1, "setname");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arcade_identity_uses_hbmame_metadata_after_mame_miss() {
        let root = unique_temp_dir("hbmame-identity");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        let hbmame_db = root.join("hbmame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[("bombjack", None, "Bomb Jack", Some("1984"), Some("Tehkan"))],
        );
        write_mame_fixture_db(
            &hbmame_db,
            &[(
                "bombjckb",
                Some("bombjack"),
                "Bomb Jack (Bootleg)",
                Some("1984"),
                Some("Tehkan"),
            )],
        );
        let mut parent = mra_discovery(1, "Bomb Jack");
        parent.setname = Some("bombjack".to_string());
        let mut hbmame_clone = mra_discovery(2, "Bomb Jack");
        hbmame_clone.setname = Some("bombjckb".to_string());
        hbmame_clone.parent = Some("bombjack".to_string());
        hbmame_clone.source_path =
            "/media/fat/_Arcade/_alternatives/_Bomb Jack/Bomb Jack (Bootleg) - HBMame.mra"
                .to_string();
        hbmame_clone.launch_ref = hbmame_clone.source_path.clone();

        write_sqlite_scan_with_mame_and_hbmame(
            &db,
            &sqlite_scan_with_discoveries(vec![parent, hbmame_clone]),
            &mame_db,
            &hbmame_db,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let identity = conn
            .query_row(
                "SELECT identity_id,family_id,metadata_title,manufacturer,source
                 FROM launchable_identities
                 WHERE identity_id='bombjckb'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .expect("query hbmame identity");
        assert_eq!(identity.0, "bombjckb");
        assert_eq!(identity.1, "bombjack");
        assert_eq!(identity.2.as_deref(), Some("Bomb Jack (Bootleg)"));
        assert_eq!(identity.3.as_deref(), Some("Tehkan"));
        assert_eq!(identity.4, "hbmame");

        let preferred_count: i64 = conn
            .query_row("SELECT count(*) FROM ui_arcade_preferred", [], |row| {
                row.get(0)
            })
            .expect("query preferred count");
        let variant_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants WHERE family_id='bombjack'",
                [],
                |row| row.get(0),
            )
            .expect("query variant count");
        assert_eq!(preferred_count, 1);
        assert_eq!(variant_count, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn hbmame_metadata_from_library_uses_mra_parent_rows() {
        let root = unique_temp_dir("hbmame-from-library");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        let hbmame_db = root.join("hbmame.sqlite3");
        let mut parent = mra_discovery(1, "Bomb Jack");
        parent.setname = Some("bombjack".to_string());
        parent.parent = Some("bombjack".to_string());
        let mut hbmame_clone = mra_discovery(2, "Bomb Jack");
        hbmame_clone.setname = Some("bombjckb".to_string());
        hbmame_clone.parent = Some("bombjack".to_string());

        save_sqlite_scan(
            &db,
            &sqlite_scan_with_discoveries(vec![parent, hbmame_clone]),
        )
        .expect("save sqlite");
        let mame_rows = BTreeMap::from([(
            "bombjack".to_string(),
            (
                String::new(),
                "Bomb Jack".to_string(),
                Some("1984".to_string()),
                Some("Tehkan".to_string()),
                Some(2),
                Some("joy".to_string()),
            ),
        )]);
        write_simple_mame_metadata_db(&mame_db, &mame_rows).expect("write MAME metadata");
        let summary =
            write_hbmame_metadata_from_library(&db, &hbmame_db).expect("write hbmame metadata");
        assert_eq!(summary.rows, 1);

        let conn = Connection::open(&hbmame_db).expect("open hbmame db");
        let row = conn
            .query_row(
                "SELECT parent_setname,title,players,control_type
                 FROM mame_machines WHERE setname='bombjckb'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .expect("query hbmame row");
        assert_eq!(row.0, "bombjack");
        assert_eq!(row.1, "Bomb Jack");
        assert_eq!(row.2, 2);
        assert_eq!(row.3, "joy");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arcade_parent_override_collapses_mvsc_unlocked_variants() {
        let root = unique_temp_dir("arcade-parent-override-mvsc");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[(
                "mvsc",
                None,
                "Marvel Vs. Capcom: Clash of Super Heroes (Europe 980123)",
                Some("1998"),
                Some("Capcom"),
            )],
        );
        let pack = preview_worker::PreviewArchiveIndex {
            path: root.join("arcade-screenshots.mmlz4b").display().to_string(),
            codec: "lz4-block",
            entries: vec!["mvsc".to_string()],
        };
        let mut parent = mra_discovery(1, "Marvel Vs. Capcom: Clash of Super Heroes");
        parent.setname = Some("mvsc".to_string());
        let mut variants = (1..=4)
            .map(|idx| {
                let mut discovery = mra_discovery(
                    idx + 1,
                    &format!("Marvel Vs. Capcom: Clash of Super Heroes [Unlocked {idx}]"),
                );
                discovery.setname = Some(format!("mvsc_{idx}"));
                discovery.source_path = format!(
                    "/media/fat/_Arcade/_Arcade Offset/_CP System II/_Unlocked/mvsc_{idx}.mra"
                );
                discovery.launch_ref = discovery.source_path.clone();
                discovery
            })
            .collect::<Vec<_>>();
        let mut discoveries = vec![parent];
        discoveries.append(&mut variants);

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(discoveries),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let preferred_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_preferred WHERE family_id='mvsc'",
                [],
                |row| row.get(0),
            )
            .expect("query preferred mvsc count");
        let variant_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants WHERE family_id='mvsc'",
                [],
                |row| row.get(0),
            )
            .expect("query mvsc variant count");
        let override_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM launchable_identities
                 WHERE family_id='mvsc' AND source='arcade-parent-override'",
                [],
                |row| row.get(0),
            )
            .expect("query override identity count");
        let missing_preview_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants
                 WHERE family_id='mvsc' AND has_preview=0",
                [],
                |row| row.get(0),
            )
            .expect("query mvsc missing previews");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(preferred_count, 1);
        assert_eq!(variant_count, 5);
        assert_eq!(override_count, 4);
        assert_eq!(missing_preview_count, 0);
        assert_eq!(loaded.catalog.system_game_count("arcade"), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arcade_parent_override_collapses_street_fighter_offset_variants() {
        let root = unique_temp_dir("arcade-parent-override-street-fighter");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                (
                    "hsf2",
                    None,
                    "Hyper Street Fighter II: The Anniversary Edition (USA 040202)",
                    Some("2004"),
                    Some("Capcom"),
                ),
                (
                    "sf2ce",
                    None,
                    "Street Fighter II': Champion Edition (World 920513)",
                    Some("1992"),
                    Some("Capcom"),
                ),
            ],
        );
        let pack = preview_worker::PreviewArchiveIndex {
            path: root.join("arcade-screenshots.mmlz4b").display().to_string(),
            codec: "lz4-block",
            entries: vec!["hsf2".to_string(), "sf2ce".to_string()],
        };
        let mut hsf2 = mra_discovery(1, "Hyper Street Fighter II");
        hsf2.setname = Some("hsf2".to_string());
        let mut sf2ce = mra_discovery(2, "Street Fighter II': Champion Edition");
        sf2ce.setname = Some("sf2ce".to_string());
        let aliases = [
            ("hsf2j1gouki", "hsf2"),
            ("hsf2j1tgouki", "hsf2"),
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
        ];
        let mut discoveries = vec![hsf2, sf2ce];
        discoveries.extend(aliases.iter().enumerate().map(|(idx, (alias, _))| {
            let mut discovery =
                mra_discovery(idx + 3, &format!("Street Fighter offset variant {alias}"));
            discovery.setname = Some((*alias).to_string());
            discovery.source_path = format!("/media/fat/_Arcade/_Arcade Offset/{alias}.mra");
            discovery.launch_ref = discovery.source_path.clone();
            discovery
        }));

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(discoveries),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let preferred_count: i64 = conn
            .query_row("SELECT count(*) FROM ui_arcade_preferred", [], |row| {
                row.get(0)
            })
            .expect("query preferred count");
        let override_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM launchable_identities
                 WHERE source='arcade-parent-override'
                   AND family_id IN ('hsf2','sf2ce')",
                [],
                |row| row.get(0),
            )
            .expect("query override identity count");
        let hsf2_variants: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants
                 WHERE family_id='hsf2' AND has_preview=1",
                [],
                |row| row.get(0),
            )
            .expect("query hsf2 variants");
        let sf2ce_variants: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants
                 WHERE family_id='sf2ce' AND has_preview=1",
                [],
                |row| row.get(0),
            )
            .expect("query sf2ce variants");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(preferred_count, 2);
        assert_eq!(override_count, aliases.len() as i64);
        assert_eq!(hsf2_variants, 3);
        assert_eq!(sf2ce_variants, 13);
        assert_eq!(loaded.catalog.system_game_count("arcade"), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arcade_mra_parent_tag_collapses_unknown_metadata_variants() {
        let root = unique_temp_dir("arcade-mra-parent-fallback");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(&mame_db, &[]);
        let mut parent = mra_discovery(1, "Mystery Parent");
        parent.setname = Some("mystery".to_string());
        let mut clone = mra_discovery(2, "Mystery Parent [Hack]");
        clone.setname = Some("mystery_hack".to_string());
        clone.parent = Some("mystery".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![parent, clone]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let clone_identity = conn
            .query_row(
                "SELECT identity_id,family_id,source
                 FROM launchable_identities
                 WHERE identity_id='mystery-hack'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .expect("query clone identity");
        let preferred_count: i64 = conn
            .query_row("SELECT count(*) FROM ui_arcade_preferred", [], |row| {
                row.get(0)
            })
            .expect("query preferred count");
        let variant_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants WHERE family_id='mystery'",
                [],
                |row| row.get(0),
            )
            .expect("query variant count");

        assert_eq!(clone_identity.0, "mystery-hack");
        assert_eq!(clone_identity.1, "mystery");
        assert_eq!(clone_identity.2, "mra-parent");
        assert_eq!(preferred_count, 1);
        assert_eq!(variant_count, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arcade_preview_keys_are_derived_from_family_without_pack_index() {
        let root = unique_temp_dir("arcade-family-preview-key");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                ("1941", None, "1941", Some("1990"), Some("Capcom")),
                (
                    "1941j",
                    Some("1941"),
                    "1941: Counter Attack (Japan)",
                    Some("1990"),
                    Some("Capcom"),
                ),
                (
                    "1941r1",
                    Some("1941"),
                    "1941: Counter Attack (World, earlier)",
                    Some("1990"),
                    Some("Capcom"),
                ),
                (
                    "1941u",
                    Some("1941"),
                    "1941: Counter Attack (USA)",
                    Some("1990"),
                    Some("Capcom"),
                ),
            ],
        );
        let pack = preview_worker::PreviewArchiveIndex {
            path: root.join("arcade-screenshots.mmlz4b").display().to_string(),
            codec: "lz4-block",
            entries: vec!["1941u".to_string()],
        };
        let mut parent = mra_discovery(1, "1941");
        parent.setname = Some("1941".to_string());
        let mut japan = mra_discovery(2, "1941: Counter Attack (Japan)");
        japan.setname = Some("1941j".to_string());
        let mut world = mra_discovery(3, "1941: Counter Attack (World, earlier)");
        world.setname = Some("1941r1".to_string());
        let mut usa = mra_discovery(4, "1941: Counter Attack (USA)");
        usa.setname = Some("1941u".to_string());

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![parent, japan, world, usa]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = library_db::open_sqlite_read_only(&db).expect("open library sqlite");
        let mut stmt = conn
            .prepare(
                "SELECT identity_id,asset_key,asset_link_reason,preview_asset_key,has_preview
                 FROM ui_arcade_variants_text
                 ORDER BY identity_id",
            )
            .expect("prepare variant asset query");
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .expect("query variant assets")
            .map(|row| row.expect("read variant asset row"))
            .collect::<Vec<_>>();
        let preferred = conn
            .query_row(
                "SELECT identity_id,asset_key,asset_link_reason,preview_asset_key,has_preview
                 FROM ui_arcade_preferred_text",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .expect("query preferred asset");

        assert_eq!(rows.len(), 4);
        for row in &rows {
            assert_eq!(row.1.as_deref(), Some("1941"));
            assert_eq!(row.2, "derived-family");
            assert_eq!(row.3, "1941");
            assert_eq!(row.4, 1);
        }
        assert_eq!(preferred.0.as_deref(), Some("1941"));
        assert_eq!(preferred.1.as_deref(), Some("1941"));
        assert_eq!(preferred.2, "derived-family");
        assert_eq!(preferred.3, "1941");
        assert_eq!(preferred.4, 1);
        let _ = std::fs::remove_dir_all(root);
    }
}
