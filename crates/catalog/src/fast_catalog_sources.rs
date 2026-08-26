// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Independent source adapters for the fast nine-system catalog.
//!
//! These adapters consume installed files and the dedicated Arcade metadata
//! contract directly. They never read Catalog V3, its scanner cache, or its
//! generated sidecars.

use crate::fast_five_catalog::{
    EXPANDED_FAST_SYSTEM_IDS, FAST_FIVE_SNAPSHOT_SCHEMA, FastFiveSnapshot, FastFiveSystem,
    collapse_c64_cross_source_variants,
};
use crate::generic_system_catalog::{add_generic_example_systems, rebuild_generic_system};
use crate::mra_header::{PrimaryRomRequirement, RomNamespace};
use crate::prepared_collections::validate_prepared_launch_path;
use crate::system_shard::{SystemGame, SystemLaunchPlan};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const FAST_SOURCE_ADAPTER_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize)]
pub struct FastSourceBuildReport {
    pub elapsed_us: u64,
    pub systems: Vec<FastSourceSystemReport>,
    pub legacy_inputs: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FastSourceSystemReport {
    pub system_id: String,
    pub files_visited: usize,
    pub games: usize,
    pub invalid: usize,
    pub elapsed_us: u64,
}

pub fn build_independent_fast_snapshot(
    storage_root: &Path,
) -> Result<(FastFiveSnapshot, FastSourceBuildReport), String> {
    let started = Instant::now();
    let mut reports = Vec::new();
    let mut systems = Vec::new();
    for system_id in ["amiga", "arcade", "c64", "dos", "x68000"] {
        let system_started = Instant::now();
        let (mut system, mut report) = build_prepared_system(storage_root, system_id)?;
        if system_id == "c64" {
            collapse_c64_cross_source_variants(&mut system);
        }
        report.elapsed_us = elapsed_us(system_started);
        report.games = system.games.len();
        systems.push(system);
        reports.push(report);
    }
    systems.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    let mut snapshot = FastFiveSnapshot {
        schema: FAST_FIVE_SNAPSHOT_SCHEMA.to_string(),
        source_fingerprint: fingerprint_systems(&systems)?,
        systems,
    };
    snapshot.validate()?;
    let (expanded, generic) = add_generic_example_systems(storage_root, snapshot)?;
    snapshot = expanded;
    reports.extend(
        generic
            .systems
            .into_iter()
            .map(|system| FastSourceSystemReport {
                system_id: system.system_id,
                files_visited: system.files,
                games: system.games,
                invalid: system.read_errors.saturating_add(system.archive_errors),
                elapsed_us: system.elapsed_us,
            }),
    );
    reports.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    snapshot.validate()?;
    Ok((
        snapshot,
        FastSourceBuildReport {
            elapsed_us: elapsed_us(started),
            systems: reports,
            legacy_inputs: 0,
        },
    ))
}

pub fn rebuild_independent_system(
    storage_root: &Path,
    _snapshot: &FastFiveSnapshot,
    system_id: &str,
) -> Result<(FastFiveSystem, FastSourceSystemReport), String> {
    if !EXPANDED_FAST_SYSTEM_IDS.contains(&system_id) {
        return Err(format!("unsupported fast source system {system_id}"));
    }
    if matches!(system_id, "neogeo" | "saturn" | "snes" | "zx-spectrum") {
        let (system, report) = rebuild_generic_system(storage_root, system_id)?;
        return Ok((
            system,
            FastSourceSystemReport {
                system_id: report.system_id,
                files_visited: report.files,
                games: report.games,
                invalid: report.read_errors.saturating_add(report.archive_errors),
                elapsed_us: report.elapsed_us,
            },
        ));
    }
    let started = Instant::now();
    let (mut system, mut report) = build_prepared_system(storage_root, system_id)?;
    if system_id == "c64" {
        collapse_c64_cross_source_variants(&mut system);
    }
    report.elapsed_us = elapsed_us(started);
    report.games = system.games.len();
    Ok((system, report))
}

fn build_prepared_system(
    storage_root: &Path,
    system_id: &str,
) -> Result<(FastFiveSystem, FastSourceSystemReport), String> {
    let mut report = FastSourceSystemReport {
        system_id: system_id.to_string(),
        ..FastSourceSystemReport::default()
    };
    let mut games = match system_id {
        "arcade" => scan_arcade(storage_root, &mut report),
        "amiga" => scan_amiga(storage_root, &mut report),
        "dos" => scan_prepared_mgl(
            &[storage_root.join("_DOS Games")],
            "dos",
            "DOS",
            &mut report,
        ),
        "x68000" => scan_prepared_mgl(
            &[
                storage_root.join("_Computer/_X68000 Games"),
                storage_root.join("_Computer/X68000 Games"),
            ],
            "x68000",
            "X68000",
            &mut report,
        ),
        "c64" => scan_oneload64(storage_root, &mut report),
        _ => return Err(format!("unsupported prepared fast system {system_id}")),
    };
    games.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| left.stable_key.cmp(&right.stable_key))
    });
    games.dedup_by(|left, right| left.launch_ref == right.launch_ref);
    Ok((
        FastFiveSystem {
            system_id: system_id.to_string(),
            display_title: display_title(system_id).to_string(),
            games,
            variants: Vec::new(),
        },
        report,
    ))
}

fn scan_arcade(storage_root: &Path, report: &mut FastSourceSystemReport) -> Vec<SystemGame> {
    let mut files = Vec::new();
    collect_matching_files(
        &storage_root.join("_Arcade"),
        &mut report.files_visited,
        &mut files,
        |path| extension_is(path, "mra"),
    );
    let mut games = Vec::new();
    for path in files {
        let bytes = match fs::read(&path) {
            Ok(bytes) if bytes.len() <= 1024 * 1024 => bytes,
            _ => {
                report.invalid += 1;
                continue;
            }
        };
        let inspection = match crate::mra_header::inspect(&bytes) {
            Ok(inspection) => inspection,
            Err(_) => {
                report.invalid += 1;
                continue;
            }
        };
        let valid_rom = match &inspection.primary_rom {
            PrimaryRomRequirement::None => true,
            PrimaryRomRequirement::Archive { namespace, setname } => {
                arcade_rom_exists(storage_root, namespace, setname)
            }
            PrimaryRomRequirement::Ambiguous => false,
        };
        if !valid_rom || !arcade_core_exists(storage_root, inspection.header.rbf.as_deref()) {
            report.invalid += 1;
            continue;
        }
        let title = inspection
            .catalog_metadata
            .as_ref()
            .map(|metadata| metadata.title.clone())
            .or(inspection.header.name)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| display_name(&path));
        let mut game = direct_row("arcade", "Arcade", &path, title);
        game.year = inspection
            .header
            .year
            .as_deref()
            .and_then(|year| year.parse::<u16>().ok());
        game.manufacturer = inspection.header.manufacturer.unwrap_or_default();
        games.push(game);
    }
    games
}

fn arcade_rom_exists(storage_root: &Path, namespace: &RomNamespace, setname: &str) -> bool {
    let directory = match namespace {
        RomNamespace::Mame => "mame",
        RomNamespace::Hbmame => "hbmame",
    };
    [
        storage_root.join("games").join(directory),
        storage_root.join("_Arcade").join(directory),
    ]
    .into_iter()
    .any(|root| case_insensitive_file_exists(&root, setname, "zip"))
}

fn arcade_core_exists(storage_root: &Path, rbf: Option<&str>) -> bool {
    let Some(expected) = rbf.map(normalize_name).filter(|value| !value.is_empty()) else {
        return false;
    };
    let mut files = Vec::new();
    let mut visited = 0;
    collect_matching_files(
        &storage_root.join("_Arcade/cores"),
        &mut visited,
        &mut files,
        |path| extension_is(path, "rbf"),
    );
    files.into_iter().any(|path| {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| normalize_name(stem).starts_with(&expected))
    })
}

fn scan_amiga(storage_root: &Path, report: &mut FastSourceSystemReport) -> Vec<SystemGame> {
    let amiga = storage_root.join("games/Amiga");
    let listings = amiga.join("listings");
    let has_payload = ["AmigaVision.hdf", "MegaAGS.hdf"]
        .into_iter()
        .any(|name| amiga.join(name).is_file());
    let mut games = Vec::new();
    if has_payload {
        for (name, kind) in [("games.txt", "games"), ("demos.txt", "demos")] {
            let path = listings.join(name);
            let contents = match fs::read_to_string(&path) {
                Ok(contents) => contents,
                Err(_) => continue,
            };
            report.files_visited += 1;
            for title in contents
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
            {
                let launch_ref = format!("magik-amigavision:{kind}:{}", encode_component(title));
                games.push(row("amiga", "Computer", title, &launch_ref, None));
            }
        }
    }
    let mut files = Vec::new();
    collect_matching_files(&amiga, &mut report.files_visited, &mut files, |path| {
        ["adf", "cue", "chd", "iso"]
            .into_iter()
            .any(|extension| extension_is(path, extension))
    });
    games.extend(
        files
            .into_iter()
            .map(|path| direct_row("amiga", "Computer", &path, display_name(&path))),
    );
    games
}

fn scan_prepared_mgl(
    roots: &[PathBuf],
    system_id: &str,
    category: &str,
    report: &mut FastSourceSystemReport,
) -> Vec<SystemGame> {
    let mut files = Vec::new();
    for root in roots {
        collect_matching_files(root, &mut report.files_visited, &mut files, |path| {
            extension_is(path, "mgl")
        });
    }
    files
        .into_iter()
        .filter_map(|path| match validate_prepared_launch_path(&path) {
            Ok(true) => Some(direct_row(system_id, category, &path, display_name(&path))),
            _ => {
                report.invalid += 1;
                None
            }
        })
        .collect()
}

fn scan_oneload64(storage_root: &Path, report: &mut FastSourceSystemReport) -> Vec<SystemGame> {
    let mut files = Vec::new();
    collect_matching_files(
        &storage_root.join("games/C64"),
        &mut report.files_visited,
        &mut files,
        |path| extension_is(path, "crt"),
    );
    files
        .into_iter()
        .filter_map(|path| match validate_prepared_launch_path(&path) {
            Ok(true) => Some(direct_row("c64", "Computer", &path, display_name(&path))),
            Ok(false) => None,
            Err(_) => {
                report.invalid += 1;
                None
            }
        })
        .collect()
}

fn collect_matching_files(
    root: &Path,
    visited: &mut usize,
    output: &mut Vec<PathBuf>,
    matches: impl Fn(&Path) -> bool + Copy,
) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    for entry in entries {
        *visited += 1;
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_matching_files(&path, visited, output, matches);
        } else if file_type.is_file() && matches(&path) {
            output.push(path);
        }
    }
}

fn direct_row(system_id: &str, category: &str, path: &Path, title: String) -> SystemGame {
    let launch_ref = path.to_string_lossy().into_owned();
    let core_path = match system_id {
        "amiga" => "_Computer/Minimig",
        "c64" => "_Computer/C64",
        "dos" => "_Computer/ao486",
        "x68000" => "_Computer/X68000",
        _ => "",
    };
    let launch_plan =
        (!core_path.is_empty() && !extension_is(path, "mgl")).then(|| SystemLaunchPlan {
            launch_ref: launch_ref.clone(),
            title: title.clone(),
            system_id: system_id.to_string(),
            core_path: core_path.to_string(),
            payload_path: launch_ref.clone(),
            mount_kind: if system_id == "c64" {
                "load-file"
            } else {
                "mount-image"
            }
            .to_string(),
            mount_index: if system_id == "c64" { 1 } else { 0 },
            delay_secs: 0,
        });
    row(system_id, category, &title, &launch_ref, launch_plan)
}

fn row(
    system_id: &str,
    category: &str,
    title: &str,
    launch_ref: &str,
    launch_plan: Option<SystemLaunchPlan>,
) -> SystemGame {
    SystemGame {
        stable_key: format!(
            "{}\u{1f}{}\u{1f}{}",
            system_id,
            title.to_ascii_lowercase(),
            launch_ref
        ),
        title: title.to_string(),
        launch_ref: launch_ref.to_string(),
        preview_archive_path: String::new(),
        preview_asset_key: String::new(),
        has_preview: false,
        year: None,
        manufacturer: String::new(),
        category: category.to_string(),
        players: None,
        control: String::new(),
        is_new: false,
        launch_plan,
    }
}

fn fingerprint_systems(systems: &[FastFiveSystem]) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(b"mister-magik-independent-fast-sources-v1\0");
    for system in systems {
        digest.update(
            postcard::to_allocvec(system)
                .map_err(|error| format!("encode {} source rows: {error}", system.system_id))?,
        );
    }
    Ok(hex_lower(&digest.finalize()))
}

fn display_title(system_id: &str) -> &'static str {
    match system_id {
        "amiga" => "Commodore Amiga",
        "arcade" => "Arcade",
        "c64" => "Commodore 64",
        "dos" => "DOS",
        "x68000" => "Sharp X68000",
        _ => "Games",
    }
}

fn display_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("Unknown")
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn case_insensitive_file_exists(root: &Path, stem: &str, extension: &str) -> bool {
    fs::read_dir(root).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let path = entry.path();
            extension_is(&path, extension)
                && path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case(stem))
        })
    })
}

fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn normalize_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn encode_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(*byte as char);
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcade_requires_external_rom_and_core() {
        let root = crate::test_support::unique_temp_dir("fast-source-arcade");
        fs::create_dir_all(root.join("_Arcade/cores")).unwrap();
        fs::create_dir_all(root.join("games/mame")).unwrap();
        fs::write(root.join("_Arcade/cores/TestCore_20260826.rbf"), b"core").unwrap();
        fs::write(
            root.join("_Arcade/Test Game.mra"),
            br#"<misterromdescription><name>Test Game</name><rbf>TestCore</rbf><rom zip="test.zip"><part>00</part></rom></misterromdescription>"#,
        )
        .unwrap();
        let mut report = FastSourceSystemReport::default();
        assert!(scan_arcade(&root, &mut report).is_empty());
        fs::write(root.join("games/mame/test.zip"), b"rom").unwrap();
        assert_eq!(scan_arcade(&root, &mut report).len(), 1);
    }

    #[test]
    fn amigavision_rows_use_materialized_launch_contract() {
        let root = crate::test_support::unique_temp_dir("fast-source-amiga");
        fs::create_dir_all(root.join("games/Amiga/listings")).unwrap();
        fs::write(root.join("games/Amiga/AmigaVision.hdf"), b"hdf").unwrap();
        fs::write(
            root.join("games/Amiga/listings/games.txt"),
            "Alien Breed\nAgony & Pain\n",
        )
        .unwrap();
        let mut report = FastSourceSystemReport::default();
        let games = scan_amiga(&root, &mut report);
        assert_eq!(games.len(), 2);
        assert_eq!(
            games[1].launch_ref,
            "magik-amigavision:games:Agony%20%26%20Pain"
        );
    }

    #[test]
    fn c64_source_ignores_non_oneload_collections() {
        let root = crate::test_support::unique_temp_dir("fast-source-c64");
        fs::create_dir_all(root.join("games/C64/Personal")).unwrap();
        fs::write(root.join("games/C64/Personal/Game.crt"), b"rom").unwrap();
        let mut report = FastSourceSystemReport::default();
        assert!(scan_oneload64(&root, &mut report).is_empty());
    }

    #[test]
    fn independent_source_set_contains_no_legacy_input_kind() {
        assert_eq!(FAST_SOURCE_ADAPTER_VERSION, 1);
        assert_eq!(EXPANDED_FAST_SYSTEM_IDS.len(), 9);
    }
}
