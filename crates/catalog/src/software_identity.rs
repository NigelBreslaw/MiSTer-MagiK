// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! MAME and software-list identity enrichment.

use crate::library_db;
use crate::runtime_metadata::{ArcadeShard, MetadataStore};
use rusqlite::params_from_iter;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

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

pub(crate) fn load_arcade_machine_metadata_for_setnames(
    mame_path: &Path,
    hbmame_path: &Path,
    setnames: &HashSet<String>,
) -> ArcadeMachineMetadata {
    if let Some(metadata) = load_runtime_arcade_metadata() {
        return filter_runtime_arcade_metadata(metadata, setnames, &HashSet::new());
    }
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

fn load_runtime_arcade_metadata() -> Option<ArcadeMachineMetadata> {
    let store =
        MetadataStore::open(&crate::catalog_config::default_runtime_metadata_path()).ok()?;
    let shard = store.arcade_shard().ok()??;
    Some(arcade_metadata_from_runtime_shard(shard))
}

fn arcade_metadata_from_runtime_shard(shard: ArcadeShard) -> ArcadeMachineMetadata {
    let mut mister_by_setname = HashMap::new();
    let mut mister_by_mra_name = HashMap::new();
    for row in shard.mister {
        let entry = MisterArcadeMetadata {
            title: row.title,
            category: row.category,
            year: row.year,
            manufacturer: row.manufacturer,
            players: row.players,
            control: row.control,
        };
        mister_by_setname
            .entry(row.setname_key)
            .or_insert_with(|| entry.clone());
        mister_by_mra_name.insert(row.mra_name_key, entry);
    }
    ArcadeMachineMetadata {
        mame: shard
            .mame
            .into_iter()
            .map(|row| {
                (
                    row.setname,
                    MameMachineMetadata {
                        parent_setname: row.parent_setname,
                        title: row.title,
                        year: row.year,
                        manufacturer: row.manufacturer,
                        players: row.players,
                        control: row.control,
                    },
                )
            })
            .collect(),
        hbmame: shard
            .hbmame
            .into_iter()
            .map(|row| {
                (
                    row.setname,
                    MameMachineMetadata {
                        parent_setname: row.parent_setname,
                        title: row.title,
                        year: row.year,
                        manufacturer: row.manufacturer,
                        players: row.players,
                        control: row.control,
                    },
                )
            })
            .collect(),
        mister_by_setname,
        mister_by_mra_name,
    }
}

fn filter_runtime_arcade_metadata(
    metadata: ArcadeMachineMetadata,
    setnames: &HashSet<String>,
    mra_names: &HashSet<String>,
) -> ArcadeMachineMetadata {
    ArcadeMachineMetadata {
        mame: metadata
            .mame
            .into_iter()
            .filter(|(key, _)| setnames.contains(key))
            .collect(),
        hbmame: metadata
            .hbmame
            .into_iter()
            .filter(|(key, _)| setnames.contains(key))
            .collect(),
        mister_by_setname: metadata
            .mister_by_setname
            .into_iter()
            .filter(|(key, _)| setnames.contains(key))
            .collect(),
        // The setname-only catalog path historically loaded every MRA row so
        // filename precedence remained available during identity projection.
        // Keep that behavior for compact metadata; the fallback path supplies
        // a non-empty key set when it can bound the lookup.
        mister_by_mra_name: if mra_names.is_empty() {
            metadata.mister_by_mra_name
        } else {
            metadata
                .mister_by_mra_name
                .into_iter()
                .filter(|(key, _)| mra_names.contains(key))
                .collect()
        },
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

fn leading_player_count(value: &str) -> Option<u8> {
    value
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u8>().ok())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use rusqlite::Connection;

    fn machine(title: &str, parent: Option<&str>) -> MameMachineMetadata {
        MameMachineMetadata {
            title: title.into(),
            parent_setname: parent.map(str::to_string),
            year: Some("1994".into()),
            manufacturer: Some("Maker".into()),
            players: Some(2),
            control: Some("joystick".into()),
        }
    }

    #[test]
    fn current_identity_resolver_preserves_metadata_precedence_and_fields() {
        let mut metadata = ArcadeMachineMetadata::default();
        metadata
            .mame
            .insert("clone".into(), machine("MAME Game", Some("parent")));
        metadata
            .hbmame
            .insert("clone".into(), machine("HB Game", Some("hb-parent")));
        let identity = mame_identity_projection("clone", &metadata, Some("mra-parent"), "Game");
        assert_eq!(
            identity,
            (
                "parent".into(),
                Some("MAME Game"),
                Some("1994"),
                Some("Maker"),
                Some(2),
                Some("joystick"),
                "mame"
            )
        );
        metadata.mame.clear();
        let identity = mame_identity_projection("clone", &metadata, Some("mra-parent"), "Game");
        assert_eq!(identity.0, "hb-parent");
        assert_eq!(identity.1, Some("HB Game"));
        assert_eq!(identity.6, "hbmame");
        metadata.hbmame.clear();
        let identity = mame_identity_projection("clone", &metadata, Some("mra-parent"), "Game");
        assert_eq!(identity.0, "mra-parent");
        assert_eq!(identity.6, "mra-parent");
    }

    #[test]
    fn current_identity_resolver_preserves_overrides_and_unknown_fallback() {
        let metadata = ArcadeMachineMetadata::default();
        for (alias, parent) in ARCADE_PARENT_OVERRIDES {
            let identity = mame_identity_projection(alias, &metadata, None, "Unmatched title");
            assert_eq!(identity.0, *parent, "{alias}");
            assert_eq!(identity.6, "arcade-parent-override");
        }
        let identity = mame_identity_projection("unknown", &metadata, Some("unknown"), "Unknown");
        assert_eq!(identity.0, "unknown");
        assert_eq!(identity.6, "setname");
        assert!(identity.1.is_none());
    }

    #[test]
    fn current_identity_resolver_collapses_only_unambiguous_canonical_families() {
        let mut metadata = ArcadeMachineMetadata::default();
        metadata
            .mame
            .insert("parent".into(), machine("Example (World)", None));
        metadata
            .mame
            .insert("clone".into(), machine("Example (Japan)", Some("parent")));
        let identity = mame_identity_projection("unlisted", &metadata, None, "Example (Hack)");
        assert_eq!(identity.0, "parent");
        assert_eq!(identity.6, "canonical-title");
        metadata
            .hbmame
            .insert("other".into(), machine("Example (USA)", None));
        let identity = mame_identity_projection("unlisted", &metadata, None, "Example (Hack)");
        assert_eq!(identity.0, "unlisted");
        assert_eq!(identity.6, "setname");
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
}
