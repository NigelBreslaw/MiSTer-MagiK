// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fast catalog construction for ordinary user-managed ROM directories.
//!
//! Unlike prepared-collection builders, this scanner assumes no fixed release
//! or directory manifest. It discovers launchable payloads from the installed
//! core launch profiles, walks arbitrary nesting, and reads ZIP directories
//! without extracting payload data.

use crate::catalog_scan::{FoundFile, scan_zip_central_directory};
use crate::fast_five_catalog::{FastFiveSnapshot, FastFiveSystem, GENERIC_EXAMPLE_SYSTEM_IDS};
use crate::launch_profiles::{
    BorrowedProfilePathClass, IgnoreReason, IgnoreRule, LaunchProfile, MountKind, MountSpec,
    PayloadDisposition, PayloadRule, ProfilePathClass, ProfileSet, RuleProvenance,
};
use crate::system_shard::{SystemGame, SystemLaunchPlan};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize)]
pub struct GenericSystemScanReport {
    pub elapsed_us: u64,
    pub games: usize,
    pub systems: Vec<GenericSystemStats>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct GenericSystemStats {
    pub system_id: String,
    pub roots: usize,
    pub directories: usize,
    pub files: usize,
    pub candidate_files: usize,
    pub archive_members: usize,
    pub games: usize,
    pub ignored_files: usize,
    pub dependency_files: usize,
    pub unmatched_files: usize,
    pub read_errors: usize,
    pub archive_errors: usize,
    pub elapsed_us: u64,
}

pub const MEDIA_EXPERIMENT_SYSTEM_IDS: [&str; 3] = ["psx", "bbcmicro", "msx"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenericScanImplementation {
    Baseline,
    BorrowedUnsorted,
}

impl GenericScanImplementation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::BorrowedUnsorted => "borrowed-unsorted",
        }
    }
}

/// Run the ordinary recursive scanner against the five prepared-source trees.
///
/// This is an A/B benchmark baseline, not an alternate publication path. It
/// deliberately has no release manifest or precomputed file inventory.
pub fn scan_prepared_system_with_generic_walker(
    storage_root: &Path,
    system_id: &str,
) -> Result<(FastFiveSystem, GenericSystemStats), String> {
    let profiles = ProfileSet::all();
    let profile = match system_id {
        "arcade" => profiles
            .profiles()
            .iter()
            .find(|profile| profile.id == "mra")
            .cloned(),
        "amiga" => profiles
            .profiles()
            .iter()
            .find(|profile| profile.id == "amiga")
            .cloned(),
        "dos" => profiles
            .profiles()
            .iter()
            .find(|profile| profile.id == "dos")
            .cloned(),
        "x68000" => profiles
            .profiles()
            .iter()
            .find(|profile| profile.id == "neon68k")
            .cloned(),
        "c64" => Some(generic_c64_baseline_profile()),
        _ => return Err(format!("unsupported prepared generic baseline {system_id}")),
    }
    .ok_or_else(|| format!("generic baseline profile is missing for {system_id}"))?;
    let roots = match system_id {
        "arcade" => vec![storage_root.join("_Arcade")],
        "amiga" => vec![storage_root.join("games/Amiga")],
        "dos" => vec![storage_root.join("_DOS Games")],
        "x68000" => vec![
            storage_root.join("_Computer/_X68000 Games"),
            storage_root.join("_Computer/X68000 Games"),
        ],
        "c64" => vec![storage_root.join("games/C64")],
        _ => unreachable!(),
    };
    let started = Instant::now();
    let mut stats = GenericSystemStats {
        system_id: system_id.to_string(),
        ..GenericSystemStats::default()
    };
    let mut scanned = Vec::new();
    let mut visited_roots = BTreeSet::new();
    for candidate in roots {
        if !candidate.is_dir() {
            continue;
        }
        let root = candidate.canonicalize().unwrap_or(candidate);
        if visited_roots.insert(root.to_string_lossy().to_ascii_lowercase()) {
            stats.roots += 1;
            scan_directory(&root, &profile, &mut stats, &mut scanned);
        }
    }
    if system_id == "c64" {
        scanned.retain(|row| {
            row.game
                .launch_ref
                .to_ascii_lowercase()
                .contains("oneload64")
        });
    }
    scanned.sort_by(|left, right| {
        left.game
            .title
            .to_ascii_lowercase()
            .cmp(&right.game.title.to_ascii_lowercase())
            .then_with(|| left.game.stable_key.cmp(&right.game.stable_key))
    });
    scanned.dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);
    stats.games = scanned.len();
    stats.elapsed_us = started.elapsed().as_micros() as u64;
    Ok((
        FastFiveSystem {
            system_id: system_id.to_string(),
            display_title: display_title(system_id).to_string(),
            games: scanned.into_iter().map(|row| row.game).collect(),
            variants: Vec::new(),
        },
        stats,
    ))
}

fn generic_c64_baseline_profile() -> LaunchProfile {
    LaunchProfile {
        id: "generic-c64-baseline".to_string(),
        system_id: "c64".to_string(),
        category: "Computer".to_string(),
        title: "Commodore 64".to_string(),
        core_name: "C64".to_string(),
        core_path: Some("_Computer/C64".to_string()),
        game_dirs: vec!["C64".to_string()],
        payload_rules: vec![PayloadRule {
            extensions: vec!["crt".to_string()],
            mount: MountSpec::load_file(1),
            disposition: PayloadDisposition::Playable,
            provenance: RuleProvenance::conf_str(
                "Generic A/B baseline treats installed C64 cartridges as primary games",
            ),
        }],
        archive_entry_rules: Vec::new(),
        collection_rules: Vec::new(),
        ignore_rules: Vec::new(),
        provenance: RuleProvenance::conf_str(
            "Generic A/B baseline for arbitrary C64 cartridge directories",
        ),
    }
}

pub fn rebuild_generic_system(
    storage_root: &Path,
    system_id: &str,
) -> Result<(FastFiveSystem, GenericSystemStats), String> {
    if !GENERIC_EXAMPLE_SYSTEM_IDS.contains(&system_id) {
        return Err(format!("unsupported generic fast system {system_id}"));
    }
    let profiles = focused_profiles()?;
    let profile = profiles
        .iter()
        .find(|profile| profile.system_id == system_id)
        .ok_or_else(|| format!("no focused launch profile found for {system_id}"))?;
    if !core_is_installed(storage_root, profile) {
        return Err(format!(
            "no installed launch profile found for generic system {system_id}"
        ));
    }
    let started = Instant::now();
    let mut stats = GenericSystemStats {
        system_id: system_id.to_string(),
        ..GenericSystemStats::default()
    };
    let mut scanned = Vec::new();
    let mut visited_roots = BTreeSet::new();
    for game_dir in &profile.game_dirs {
        let candidate = storage_root.join("games").join(game_dir);
        if !candidate.is_dir() {
            continue;
        }
        let root = candidate.canonicalize().unwrap_or(candidate);
        if visited_roots.insert(root.to_string_lossy().to_ascii_lowercase()) {
            stats.roots += 1;
            scan_directory(&root, profile, &mut stats, &mut scanned);
        }
    }
    if stats.roots == 0 {
        return Err(format!(
            "generic system {system_id} has an installed profile but no game directory"
        ));
    }
    scanned.sort_by(|left, right| {
        left.game
            .title
            .to_ascii_lowercase()
            .cmp(&right.game.title.to_ascii_lowercase())
            .then_with(|| left.game.stable_key.cmp(&right.game.stable_key))
    });
    scanned.dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);
    stats.games = scanned.len();
    stats.elapsed_us = started.elapsed().as_micros() as u64;
    Ok((
        FastFiveSystem {
            system_id: system_id.to_string(),
            display_title: display_title(system_id).to_string(),
            games: scanned.into_iter().map(|row| row.game).collect(),
            variants: Vec::new(),
        },
        stats,
    ))
}

pub fn scan_media_experiment_system(
    storage_root: &Path,
    system_id: &str,
    implementation: GenericScanImplementation,
) -> Result<(FastFiveSystem, GenericSystemStats), String> {
    if !MEDIA_EXPERIMENT_SYSTEM_IDS.contains(&system_id) {
        return Err(format!("unsupported media experiment system {system_id}"));
    }
    let profiles = focused_media_profiles()?;
    let profile = profiles
        .iter()
        .find(|profile| profile.system_id == system_id)
        .ok_or_else(|| format!("no media experiment profile found for {system_id}"))?;
    if !core_is_installed(storage_root, profile) {
        return Err(format!(
            "no installed core found for media system {system_id}"
        ));
    }

    let started = Instant::now();
    let mut stats = GenericSystemStats {
        system_id: system_id.to_string(),
        ..GenericSystemStats::default()
    };
    let mut scanned = Vec::new();
    let mut visited_roots = BTreeSet::new();
    for game_dir in &profile.game_dirs {
        let candidate = storage_root.join("games").join(game_dir);
        if !candidate.is_dir() {
            continue;
        }
        let root = candidate.canonicalize().unwrap_or(candidate);
        if visited_roots.insert(root.to_string_lossy().to_ascii_lowercase()) {
            stats.roots += 1;
            match implementation {
                GenericScanImplementation::Baseline => {
                    scan_directory(&root, profile, &mut stats, &mut scanned)
                }
                GenericScanImplementation::BorrowedUnsorted => {
                    scan_directory_borrowed_unsorted(&root, profile, &mut stats, &mut scanned)
                }
            }
        }
    }
    if stats.roots == 0 {
        return Err(format!("media system {system_id} has no game directory"));
    }

    scanned.sort_by(|left, right| {
        left.game
            .title
            .to_ascii_lowercase()
            .cmp(&right.game.title.to_ascii_lowercase())
            .then_with(|| left.game.stable_key.cmp(&right.game.stable_key))
    });
    scanned.dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);
    stats.games = scanned.len();
    stats.elapsed_us = started.elapsed().as_micros() as u64;
    Ok((
        FastFiveSystem {
            system_id: system_id.to_string(),
            display_title: display_title(system_id).to_string(),
            games: scanned.into_iter().map(|row| row.game).collect(),
            variants: Vec::new(),
        },
        stats,
    ))
}

#[derive(Debug)]
struct ScannedGame {
    game: SystemGame,
    signature: String,
}

/// Replace the four ordinary-filesystem examples in a base snapshot.
///
/// The source snapshot remains independent of the legacy whole-card catalog.
/// Launch profiles are reused only as the core/media contract.
pub fn add_generic_example_systems(
    storage_root: &Path,
    mut snapshot: FastFiveSnapshot,
) -> Result<(FastFiveSnapshot, GenericSystemScanReport), String> {
    snapshot.validate()?;
    let started = Instant::now();
    let profiles = focused_profiles()?;
    let mut systems = Vec::with_capacity(GENERIC_EXAMPLE_SYSTEM_IDS.len());
    let mut all_signatures = Vec::new();
    let mut reports = Vec::with_capacity(GENERIC_EXAMPLE_SYSTEM_IDS.len());

    for system_id in GENERIC_EXAMPLE_SYSTEM_IDS {
        let profile = profiles
            .iter()
            .find(|profile| profile.system_id == system_id)
            .ok_or_else(|| format!("no focused launch profile found for {system_id}"))?;
        if !core_is_installed(storage_root, profile) {
            return Err(format!(
                "no installed launch profile found for generic system {system_id}"
            ));
        }
        let system_started = Instant::now();
        let mut stats = GenericSystemStats {
            system_id: system_id.to_string(),
            ..GenericSystemStats::default()
        };
        let mut scanned = Vec::new();
        let mut visited_roots = BTreeSet::new();
        for game_dir in &profile.game_dirs {
            let candidate = storage_root.join("games").join(game_dir);
            if !candidate.is_dir() {
                continue;
            }
            let root = candidate.canonicalize().unwrap_or(candidate);
            if visited_roots.insert(root.to_string_lossy().to_ascii_lowercase()) {
                stats.roots += 1;
                scan_directory(&root, profile, &mut stats, &mut scanned);
            }
        }
        if stats.roots == 0 {
            return Err(format!(
                "generic system {system_id} has an installed profile but no game directory"
            ));
        }

        scanned.sort_by(|left, right| {
            left.game
                .title
                .to_ascii_lowercase()
                .cmp(&right.game.title.to_ascii_lowercase())
                .then_with(|| left.game.stable_key.cmp(&right.game.stable_key))
        });
        scanned.dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);
        stats.games = scanned.len();
        stats.elapsed_us = system_started.elapsed().as_micros() as u64;
        all_signatures.extend(scanned.iter().map(|row| row.signature.clone()));
        systems.push(FastFiveSystem {
            system_id: system_id.to_string(),
            display_title: display_title(system_id).to_string(),
            games: scanned.into_iter().map(|row| row.game).collect(),
            variants: Vec::new(),
        });
        reports.push(stats);
    }

    snapshot
        .systems
        .retain(|system| !GENERIC_EXAMPLE_SYSTEM_IDS.contains(&system.system_id.as_str()));
    snapshot.systems.extend(systems);
    snapshot
        .systems
        .sort_by(|left, right| left.system_id.cmp(&right.system_id));
    all_signatures.sort();
    let mut fingerprint = Sha256::new();
    fingerprint.update(b"mister-magik-generic-system-scan-v1\0");
    fingerprint.update(snapshot.source_fingerprint.as_bytes());
    for signature in all_signatures {
        fingerprint.update([0]);
        fingerprint.update(signature.as_bytes());
    }
    snapshot.source_fingerprint = hex_lower(&fingerprint.finalize());
    snapshot.validate()?;
    let report = GenericSystemScanReport {
        elapsed_us: started.elapsed().as_micros() as u64,
        games: reports.iter().map(|system| system.games).sum(),
        systems: reports,
    };
    Ok((snapshot, report))
}

fn focused_profiles() -> Result<Vec<LaunchProfile>, String> {
    let all = ProfileSet::all();
    let mut profiles = Vec::with_capacity(GENERIC_EXAMPLE_SYSTEM_IDS.len());
    for (system_id, game_dir) in [("neogeo", "NEOGEO"), ("saturn", "Saturn"), ("snes", "SNES")] {
        let mut profile = all
            .profiles()
            .iter()
            .find(|profile| {
                profile.system_id == system_id
                    && profile
                        .game_dirs
                        .iter()
                        .any(|dir| dir.eq_ignore_ascii_case(game_dir))
            })
            .cloned()
            .ok_or_else(|| format!("built-in launch profile is missing for {system_id}"))?;
        if system_id == "snes" {
            profile.game_dirs.extend(
                ["Satellaview", "SGB2", "SNES-Sinden"]
                    .into_iter()
                    .map(str::to_string),
            );
        }
        profiles.push(profile);
    }
    let spectrum_rule = PayloadRule {
        extensions: ["sna", "szx", "tap", "tzx", "z80"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        mount: MountSpec::load_file(1),
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::conf_str(
            "Focused ZX Spectrum profile uses the established runtime payload contract",
        ),
    };
    profiles.push(LaunchProfile {
        id: "runtime-zx-spectrum".to_string(),
        system_id: "zx-spectrum".to_string(),
        category: "Computer".to_string(),
        title: "ZX Spectrum".to_string(),
        core_name: "ZX-Spectrum".to_string(),
        core_path: Some("_Computer/ZX-Spectrum".to_string()),
        game_dirs: vec!["Spectrum".to_string()],
        payload_rules: vec![spectrum_rule.clone()],
        archive_entry_rules: vec![spectrum_rule],
        collection_rules: Vec::new(),
        ignore_rules: vec![IgnoreRule {
            file_names: vec!["boot.rom".to_string()],
            extensions: Vec::new(),
            reason: IgnoreReason::Bios,
            provenance: RuleProvenance::magik("boot.rom is Spectrum firmware, not a game"),
        }],
        provenance: RuleProvenance::conf_str(
            "Focused generic scanner profile for the ZX-Spectrum core",
        ),
    });
    Ok(profiles)
}

fn focused_media_profiles() -> Result<Vec<LaunchProfile>, String> {
    let all = ProfileSet::all();
    let psx = all
        .profiles()
        .iter()
        .find(|profile| profile.id == "psx")
        .cloned()
        .ok_or_else(|| "built-in PSX launch profile is missing".to_string())?;
    let bbc_rule = PayloadRule {
        extensions: ["adl", "dsd", "sdd", "ssd", "uef"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        mount: MountSpec::load_file(1),
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::conf_str(
            "BBC Micro runtime profile accepts maintained disk and tape payload formats",
        ),
    };
    let msx_rule = PayloadRule {
        extensions: ["rom", "mx1", "mx2"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        mount: MountSpec::load_file(1),
        disposition: PayloadDisposition::Playable,
        provenance: RuleProvenance::conf_str(
            "Focused MSX experiment covers cartridge payloads without filesystem snapshots",
        ),
    };
    Ok(vec![
        psx,
        LaunchProfile {
            id: "focused-bbcmicro".to_string(),
            system_id: "bbcmicro".to_string(),
            category: "Computer".to_string(),
            title: "BBC Micro".to_string(),
            core_name: "BBCMicro".to_string(),
            core_path: Some("_Computer/BBCMicro".to_string()),
            game_dirs: vec!["BBCMicro".to_string()],
            payload_rules: vec![bbc_rule.clone()],
            archive_entry_rules: vec![bbc_rule],
            collection_rules: Vec::new(),
            ignore_rules: Vec::new(),
            provenance: RuleProvenance::conf_str(
                "Focused generic BBC Micro media experiment profile",
            ),
        },
        LaunchProfile {
            id: "focused-msx".to_string(),
            system_id: "msx".to_string(),
            category: "Computer".to_string(),
            title: "MSX".to_string(),
            core_name: "MSX".to_string(),
            core_path: Some("_Computer/MSX".to_string()),
            game_dirs: vec!["MSX".to_string()],
            payload_rules: vec![msx_rule.clone()],
            archive_entry_rules: vec![msx_rule],
            collection_rules: Vec::new(),
            ignore_rules: Vec::new(),
            provenance: RuleProvenance::conf_str(
                "Focused generic MSX cartridge experiment profile",
            ),
        },
    ])
}

fn core_is_installed(storage_root: &Path, profile: &LaunchProfile) -> bool {
    let Some(core_path) = profile.core_path.as_deref() else {
        return false;
    };
    let relative = Path::new(core_path);
    let Some(parent) = relative.parent() else {
        return false;
    };
    let Some(expected) = relative.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Ok(entries) = fs::read_dir(storage_root.join(parent)) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
            && path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| {
                    stem.eq_ignore_ascii_case(expected)
                        || stem
                            .get(..expected.len())
                            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(expected))
                            && stem.as_bytes().get(expected.len()) == Some(&b'_')
                })
    })
}

fn scan_directory(
    root: &Path,
    profile: &LaunchProfile,
    stats: &mut GenericSystemStats,
    games: &mut Vec<ScannedGame>,
) {
    stats.directories += 1;
    let mut entries = match fs::read_dir(root) {
        Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
        Err(_) => {
            stats.read_errors += 1;
            return;
        }
    };
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                stats.read_errors += 1;
                continue;
            }
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            scan_directory(&path, profile, stats, games);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        stats.files += 1;
        match profile.classify_path(&path) {
            ProfilePathClass::Payload { rule }
                if rule.disposition == PayloadDisposition::Playable =>
            {
                stats.candidate_files += 1;
                games.push(direct_game(profile, &path, &rule));
            }
            ProfilePathClass::NotMatched
                if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
                    && !profile.archive_entry_rules.is_empty() =>
            {
                stats.candidate_files += 1;
                scan_archive(profile, &path, stats, games);
            }
            ProfilePathClass::Ignored { reason, .. } => {
                stats.ignored_files += 1;
                if reason == IgnoreReason::CueTrack {
                    stats.dependency_files += 1;
                }
            }
            ProfilePathClass::Payload { .. } => stats.dependency_files += 1,
            _ => stats.unmatched_files += 1,
        }
    }
}

fn scan_directory_borrowed_unsorted(
    root: &Path,
    profile: &LaunchProfile,
    stats: &mut GenericSystemStats,
    games: &mut Vec<ScannedGame>,
) {
    stats.directories += 1;
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => {
            stats.read_errors += 1;
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                stats.read_errors += 1;
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                stats.read_errors += 1;
                continue;
            }
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            scan_directory_borrowed_unsorted(&path, profile, stats, games);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        stats.files += 1;
        match profile.classify_path_borrowed(&path) {
            BorrowedProfilePathClass::Payload { rule }
                if rule.disposition == PayloadDisposition::Playable =>
            {
                stats.candidate_files += 1;
                games.push(direct_game(profile, &path, rule));
            }
            BorrowedProfilePathClass::NotMatched
                if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
                    && !profile.archive_entry_rules.is_empty() =>
            {
                stats.candidate_files += 1;
                scan_archive(profile, &path, stats, games);
            }
            BorrowedProfilePathClass::Ignored { reason } => {
                stats.ignored_files += 1;
                if reason == IgnoreReason::CueTrack {
                    stats.dependency_files += 1;
                }
            }
            BorrowedProfilePathClass::Payload { .. } => stats.dependency_files += 1,
            _ => stats.unmatched_files += 1,
        }
    }
}

fn direct_game(profile: &LaunchProfile, path: &Path, rule: &PayloadRule) -> ScannedGame {
    let launch_ref = path.to_string_lossy().into_owned();
    let signature = format!("{}\u{1f}{}", profile.system_id, launch_ref);
    ScannedGame {
        game: system_game(profile, path, &launch_ref, rule),
        signature,
    }
}

fn scan_archive(
    profile: &LaunchProfile,
    path: &Path,
    stats: &mut GenericSystemStats,
    games: &mut Vec<ScannedGame>,
) {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            stats.read_errors += 1;
            return;
        }
    };
    let found = FoundFile {
        path: path.to_path_buf(),
        ext: "zip".to_string(),
        size: metadata.len(),
        mtime_secs: mtime_secs(&metadata),
    };
    match scan_zip_central_directory(&found, profile) {
        Ok(entries) => {
            stats.archive_members += entries.len();
            for entry in entries {
                let member_path = PathBuf::from(&entry.entry_path);
                let signature = format!("{}\u{1f}{}", profile.system_id, entry.launch_ref);
                games.push(ScannedGame {
                    game: system_game(profile, &member_path, &entry.launch_ref, &entry.rule),
                    signature,
                });
            }
        }
        Err(_) => stats.archive_errors += 1,
    }
}

fn system_game(
    profile: &LaunchProfile,
    title_path: &Path,
    launch_ref: &str,
    rule: &PayloadRule,
) -> SystemGame {
    let title = display_name(title_path);
    let normalized_title = title.to_ascii_lowercase();
    let core_path = profile
        .core_path
        .clone()
        .unwrap_or_else(|| profile.core_name.clone());
    SystemGame {
        stable_key: format!(
            "{}\u{1f}{}\u{1f}{}",
            profile.system_id, normalized_title, launch_ref
        ),
        title: title.clone(),
        launch_ref: launch_ref.to_string(),
        preview_archive_path: String::new(),
        preview_asset_key: String::new(),
        has_preview: false,
        year: None,
        manufacturer: String::new(),
        category: profile.category.clone(),
        players: None,
        control: String::new(),
        is_new: false,
        launch_plan: Some(SystemLaunchPlan {
            launch_ref: launch_ref.to_string(),
            title,
            system_id: profile.system_id.clone(),
            core_path,
            payload_path: launch_ref.to_string(),
            mount_kind: mount_kind(rule.mount.kind).to_string(),
            mount_index: rule.mount.index,
            delay_secs: rule.mount.delay_secs,
        }),
    }
}

fn display_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("Unknown")
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_title(system_id: &str) -> &'static str {
    match system_id {
        "bbcmicro" => "BBC Micro",
        "msx" => "MSX",
        "neogeo" => "Neo Geo",
        "psx" => "PlayStation",
        "saturn" => "Sega Saturn",
        "snes" => "Super Nintendo",
        "zx-spectrum" => "ZX Spectrum",
        _ => "Games",
    }
}

fn mount_kind(kind: MountKind) -> &'static str {
    match kind {
        MountKind::Launcher => "launcher",
        MountKind::LoadFile => "load-file",
        MountKind::MountImage => "mount-image",
        MountKind::Core => "core",
    }
}

fn mtime_secs(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs() as i64)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_five_catalog::{FAST_FIVE_SNAPSHOT_SCHEMA, FAST_FIVE_SYSTEM_IDS};

    #[test]
    fn scans_nested_user_collections_without_a_release_manifest() {
        let root = crate::test_support::unique_temp_dir("generic-system-catalog");
        for core in [
            "_Console/SNES_20260826.rbf",
            "_Console/Saturn_20260826.rbf",
            "_Console/NeoGeo_20260826.rbf",
            "_Computer/ZX-Spectrum_20260826.rbf",
        ] {
            let path = root.join(core);
            fs::create_dir_all(path.parent().expect("core parent")).expect("create core parent");
            fs::write(path, b"core").expect("write core");
        }
        let files = [
            ("games/SNES/Publisher/Super Game.sfc", b"rom".as_slice()),
            ("games/Saturn/Disc Game.chd", b"disc".as_slice()),
            ("games/Saturn/Disc Game.bin", b"track".as_slice()),
            ("games/NEOGEO/Arcade Game.neo", b"rom".as_slice()),
            ("games/Spectrum/Tape Game.tzx", b"tape".as_slice()),
        ];
        for (relative, bytes) in files {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("game parent")).expect("create game parent");
            fs::write(path, bytes).expect("write game");
        }
        let base = FastFiveSnapshot {
            schema: FAST_FIVE_SNAPSHOT_SCHEMA.to_string(),
            source_fingerprint: "0".repeat(64),
            systems: FAST_FIVE_SYSTEM_IDS
                .into_iter()
                .map(|system_id| FastFiveSystem {
                    system_id: system_id.to_string(),
                    display_title: system_id.to_string(),
                    games: Vec::new(),
                    variants: Vec::new(),
                })
                .collect(),
        };

        let (snapshot, report) = add_generic_example_systems(&root, base).expect("scan");

        assert_eq!(snapshot.systems.len(), 9);
        assert_eq!(report.games, 4);
        for system_id in GENERIC_EXAMPLE_SYSTEM_IDS {
            let system = snapshot
                .systems
                .iter()
                .find(|system| system.system_id == system_id)
                .expect("generic system");
            assert_eq!(system.games.len(), 1, "{system_id}");
            assert!(system.games[0].launch_plan.is_some());
        }
        let saturn = snapshot
            .systems
            .iter()
            .find(|system| system.system_id == "saturn")
            .expect("saturn");
        assert!(
            saturn
                .games
                .iter()
                .all(|game| !game.launch_ref.ends_with(".bin"))
        );
    }

    #[test]
    fn media_experiment_implementations_have_exact_launch_parity() {
        let root = crate::test_support::unique_temp_dir("generic-media-experiment");
        for core in [
            "_Console/PSX_20260826.rbf",
            "_Computer/BBCMicro_20260826.rbf",
            "_Computer/MSX_20260826.rbf",
        ] {
            let path = root.join(core);
            fs::create_dir_all(path.parent().expect("core parent")).expect("create core parent");
            fs::write(path, b"core").expect("write core");
        }
        for (relative, bytes) in [
            ("games/PSX/Disc Game.chd", b"disc".as_slice()),
            ("games/PSX/Disc Game.bin", b"track".as_slice()),
            ("games/PSX/boot.rom", b"bios".as_slice()),
            ("games/PSX/Disc Game.sbi", b"sidecar".as_slice()),
            ("games/BBCMicro/Elite.ssd", b"disk".as_slice()),
            ("games/BBCMicro/Exile.uef", b"tape".as_slice()),
            ("games/MSX/Metal Gear.rom", b"rom".as_slice()),
        ] {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("game parent")).expect("create game parent");
            fs::write(path, bytes).expect("write game");
        }

        for system_id in MEDIA_EXPERIMENT_SYSTEM_IDS {
            let (baseline, baseline_stats) =
                scan_media_experiment_system(&root, system_id, GenericScanImplementation::Baseline)
                    .expect("baseline scan");
            let (optimized, optimized_stats) = scan_media_experiment_system(
                &root,
                system_id,
                GenericScanImplementation::BorrowedUnsorted,
            )
            .expect("optimized scan");
            assert_eq!(
                baseline
                    .games
                    .iter()
                    .map(|game| &game.launch_ref)
                    .collect::<Vec<_>>(),
                optimized
                    .games
                    .iter()
                    .map(|game| &game.launch_ref)
                    .collect::<Vec<_>>(),
                "{system_id}"
            );
            assert_eq!(baseline_stats.games, optimized_stats.games, "{system_id}");
            assert_eq!(
                baseline_stats.ignored_files, optimized_stats.ignored_files,
                "{system_id}"
            );
        }
        let (_, psx_stats) =
            scan_media_experiment_system(&root, "psx", GenericScanImplementation::BorrowedUnsorted)
                .expect("PSX scan");
        assert_eq!(psx_stats.games, 1);
        assert_eq!(psx_stats.ignored_files, 3);
        assert_eq!(psx_stats.dependency_files, 1);
        let _ = fs::remove_dir_all(root);
    }
}
