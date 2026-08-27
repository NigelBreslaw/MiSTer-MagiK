// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Base compilation and cold active-catalog construction.

use crate::model::{
    ActiveCatalog, ActiveCounts, ActiveRecord, BaseCandidate, BaseCatalog, MraHeader,
    PrimaryRomRequirement, RomNamespace, RomRequirement, UpdaterRow, decode_updater_index, sha256,
};
use crate::scan::{InstalledMra, RomInventory, scan_installed_mras, scan_rom_inventory};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Serialize)]
pub struct CompileReport {
    pub updater_bytes: usize,
    pub source_rows: usize,
    pub enriched_rows: usize,
    pub derived_rows: usize,
    pub ambiguous_rows: usize,
    pub decode_us: u64,
    pub compile_us: u64,
}

#[derive(Debug, Serialize)]
pub struct BuildReport {
    pub trust_mode: &'static str,
    pub discovery_mode: &'static str,
    pub base_bytes: usize,
    pub base_rows: usize,
    pub installed_mras: usize,
    pub rom_preeliminated_candidates: usize,
    pub ambiguous_preeliminated_candidates: usize,
    pub mame_archives: usize,
    pub hbmame_archives: usize,
    pub active_records: usize,
    pub preferred_families: usize,
    pub indexed_path_hits: usize,
    pub fast_path_hits: usize,
    pub size_mismatches: usize,
    pub fallback_count: usize,
    pub skipped_missing_rom: usize,
    pub skipped_ambiguous: usize,
    pub skipped_invalid: usize,
    pub base_decode_us: u64,
    pub mra_scan_us: u64,
    pub rom_scan_us: u64,
    pub inventory_wall_us: u64,
    pub join_and_fallback_us: u64,
    pub selection_us: u64,
    pub fallback_paths: Vec<String>,
    pub invalid_paths: Vec<String>,
}

pub fn compile_base(updater_bytes: &[u8]) -> Result<(BaseCatalog, CompileReport), String> {
    let started = Instant::now();
    let index = decode_updater_index(updater_bytes)?;
    let decode_us = elapsed_us(started);
    let compile_started = Instant::now();
    let mut enriched_rows = 0;
    let mut derived_rows = 0;
    let mut ambiguous_rows = 0;
    let mut candidates = Vec::with_capacity(index.rows.len());
    for row in &index.rows {
        if row.catalog_metadata.is_some() {
            enriched_rows += 1;
        } else {
            derived_rows += 1;
        }
        if row.primary_rom == PrimaryRomRequirement::Ambiguous {
            ambiguous_rows += 1;
        }
        candidates.push(candidate_from_updater(row));
    }
    candidates.sort_by(|left, right| left.path_key.cmp(&right.path_key));
    if candidates
        .windows(2)
        .any(|pair| pair[0].path_key == pair[1].path_key)
    {
        return Err("Update_All paths collide case-insensitively".to_string());
    }
    let report = CompileReport {
        updater_bytes: updater_bytes.len(),
        source_rows: candidates.len(),
        enriched_rows,
        derived_rows,
        ambiguous_rows,
        decode_us,
        compile_us: elapsed_us(compile_started),
    };
    Ok((
        BaseCatalog {
            source_sha256: sha256(updater_bytes),
            candidates,
        },
        report,
    ))
}

pub fn build_active(
    base_bytes: &[u8],
    arcade_root: &Path,
    mame_directories: &[PathBuf],
    hbmame_directories: &[PathBuf],
    verify_index_size: bool,
    full_walk: bool,
) -> Result<(ActiveCatalog, BuildReport), String> {
    let base_started = Instant::now();
    let base = crate::model::decode_base(base_bytes)?;
    let base_decode_us = elapsed_us(base_started);

    let inventory_started = Instant::now();
    let (roms, rom_scan_us) = timed(|| scan_rom_inventory(mame_directories, hbmame_directories));
    let roms = roms?;
    let (installed, mra_scan_us, rom_preeliminated_candidates, ambiguous_preeliminated_candidates) =
        if full_walk {
            let (installed, scan_us) =
                timed(|| scan_installed_mras(arcade_root, verify_index_size));
            (installed?, scan_us, 0, 0)
        } else {
            let missing = base
                .candidates
                .iter()
                .filter(|candidate| {
                    matches!(
                        rom_eligibility(&candidate.rom, &roms),
                        RomEligibility::Missing
                    )
                })
                .count();
            let ambiguous = base
                .candidates
                .iter()
                .filter(|candidate| {
                    matches!(
                        rom_eligibility(&candidate.rom, &roms),
                        RomEligibility::Ambiguous
                    )
                })
                .count();
            let (installed, probe_us) = timed(|| {
                probe_indexed_candidates(arcade_root, &base.candidates, &roms, verify_index_size)
            });
            (installed?, probe_us, missing, ambiguous)
        };
    let inventory_wall_us = elapsed_us(inventory_started);

    let join_started = Instant::now();
    let mut fast_path_hits = 0usize;
    let mut indexed_path_hits = 0usize;
    let mut size_mismatches = 0usize;
    let mut fallback_paths = Vec::new();
    let mut invalid_paths = Vec::new();
    let mut skipped_missing_rom = rom_preeliminated_candidates;
    let mut skipped_ambiguous = ambiguous_preeliminated_candidates;
    let mut eligible = Vec::with_capacity(installed.len());
    for installed_mra in &installed {
        let indexed = base
            .candidates
            .binary_search_by(|candidate| candidate.path_key.cmp(&installed_mra.path_key))
            .ok()
            .map(|index| &base.candidates[index]);
        if indexed.is_some() {
            indexed_path_hits += 1;
        }
        let mut candidate = match indexed {
            Some(candidate)
                if !candidate.needs_fallback
                    && installed_mra
                        .size
                        .is_none_or(|size| candidate.expected_size == size) =>
            {
                fast_path_hits += 1;
                candidate.clone()
            }
            _ => {
                if let (Some(candidate), Some(size)) = (indexed, installed_mra.size)
                    && candidate.expected_size != size
                {
                    size_mismatches += 1;
                }
                fallback_paths.push(installed_mra.relative_path.clone());
                match fallback_candidate(installed_mra) {
                    Ok(candidate) => candidate,
                    Err(error) => {
                        invalid_paths.push(format!("{}: {error}", installed_mra.relative_path));
                        continue;
                    }
                }
            }
        };
        candidate.path = installed_mra.full_path.to_string_lossy().into_owned();
        match rom_eligibility(&candidate.rom, &roms) {
            RomEligibility::Eligible => eligible.push(candidate),
            RomEligibility::Missing => skipped_missing_rom += 1,
            RomEligibility::Ambiguous => skipped_ambiguous += 1,
        }
    }
    let join_and_fallback_us = elapsed_us(join_started);

    let selection_started = Instant::now();
    let records = select_and_order(eligible)?;
    let preferred_families = records.iter().filter(|record| record.preferred).count();
    let selection_us = elapsed_us(selection_started);
    let counts = ActiveCounts {
        installed_mras: as_u32(installed.len(), "installed MRA count")?,
        index_hits: as_u32(fast_path_hits, "updater-index fast-path hit count")?,
        fallbacks: as_u32(fallback_paths.len(), "fallback count")?,
        skipped_missing_rom: as_u32(skipped_missing_rom, "missing ROM count")?,
        skipped_ambiguous: as_u32(skipped_ambiguous, "ambiguous ROM count")?,
        skipped_invalid: as_u32(invalid_paths.len(), "invalid MRA count")?,
    };
    let report = BuildReport {
        trust_mode: if verify_index_size {
            "installed-path-and-size"
        } else {
            "installed-path-update-all-metadata"
        },
        discovery_mode: if full_walk {
            "full-walk"
        } else {
            "update-all-probe"
        },
        base_bytes: base_bytes.len(),
        base_rows: base.candidates.len(),
        installed_mras: installed.len(),
        rom_preeliminated_candidates,
        ambiguous_preeliminated_candidates,
        mame_archives: roms.mame.len(),
        hbmame_archives: roms.hbmame.len(),
        active_records: records.len(),
        preferred_families,
        indexed_path_hits,
        fast_path_hits,
        size_mismatches,
        fallback_count: fallback_paths.len(),
        skipped_missing_rom,
        skipped_ambiguous,
        skipped_invalid: invalid_paths.len(),
        base_decode_us,
        mra_scan_us,
        rom_scan_us,
        inventory_wall_us,
        join_and_fallback_us,
        selection_us,
        fallback_paths,
        invalid_paths,
    };
    Ok((
        ActiveCatalog {
            source_sha256: base.source_sha256,
            counts,
            records,
        },
        report,
    ))
}

fn candidate_from_updater(row: &UpdaterRow) -> BaseCandidate {
    let path_stem = Path::new(&row.path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Arcade");
    let header_setname = row.header.setname.as_deref().unwrap_or("");
    let derived_identity = normalize_id(if header_setname.is_empty() {
        row.header.name.as_deref().unwrap_or(path_stem)
    } else {
        header_setname
    });
    let metadata = row.catalog_metadata.as_ref();
    let identity_id = metadata
        .map(|metadata| metadata.identity_id.clone())
        .filter(|identity| !identity.is_empty())
        .unwrap_or(derived_identity);
    let family_id = metadata
        .map(|metadata| metadata.family_id.clone())
        .filter(|family| !family.is_empty())
        .unwrap_or_else(|| {
            row.header
                .parent
                .as_deref()
                .map(normalize_id)
                .filter(|parent| parent != "unknown")
                .unwrap_or_else(|| identity_id.clone())
        });
    let title = metadata
        .map(|metadata| metadata.title.clone())
        .filter(|title| !title.is_empty())
        .or_else(|| row.header.name.clone())
        .unwrap_or_else(|| path_stem.to_string());
    let year = metadata
        .and_then(|metadata| metadata.year)
        .or_else(|| row.header.year.as_deref().and_then(parse_year));
    let manufacturer = metadata
        .map(|metadata| metadata.manufacturer.clone())
        .filter(|manufacturer| !manufacturer.is_empty())
        .or_else(|| row.header.manufacturer.clone())
        .unwrap_or_default();
    let category = metadata
        .map(|metadata| metadata.category.clone())
        .filter(|category| !category.is_empty())
        .unwrap_or_else(|| "Arcade".to_string());
    let control = metadata
        .map(|metadata| metadata.control.clone())
        .unwrap_or_default();
    let players = metadata.and_then(|metadata| metadata.players);
    let rom = requirement_from_updater(&row.primary_rom);
    let variant_score = variant_score(&format!(
        "{} {} {} {}",
        title,
        row.path,
        header_setname,
        row.header.parent.as_deref().unwrap_or("")
    ));
    BaseCandidate {
        path: row.path.clone(),
        path_key: row.path.to_ascii_lowercase(),
        family_id,
        identity_id,
        title,
        manufacturer,
        category,
        control,
        setname: header_setname.to_ascii_lowercase(),
        needs_fallback: matches!(&rom, RomRequirement::Ambiguous),
        rom,
        expected_size: row.size,
        year,
        players,
        variant_score,
    }
}

fn probe_indexed_candidates(
    arcade_root: &Path,
    candidates: &[BaseCandidate],
    roms: &RomInventory,
    verify_index_size: bool,
) -> Result<Vec<InstalledMra>, String> {
    let mut directories = BTreeMap::<PathBuf, Vec<&BaseCandidate>>::new();
    for candidate in candidates {
        if !matches!(
            rom_eligibility(&candidate.rom, roms),
            RomEligibility::Eligible
        ) {
            continue;
        }
        let suffix = candidate
            .path
            .strip_prefix("_Arcade/")
            .ok_or_else(|| format!("invalid updater Arcade path {}", candidate.path))?;
        let suffix = Path::new(suffix);
        let parent = suffix.parent().unwrap_or_else(|| Path::new(""));
        directories
            .entry(parent.to_path_buf())
            .or_default()
            .push(candidate);
    }
    let directories = directories.into_iter().collect::<Vec<_>>();
    let probe = |slice: &[(PathBuf, Vec<&BaseCandidate>)]| -> Result<Vec<InstalledMra>, String> {
        let mut installed = Vec::new();
        for (relative_directory, candidates) in slice {
            let full_directory = arcade_root.join(relative_directory);
            let entries = match fs::read_dir(&full_directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "read indexed Arcade directory {}: {error}",
                        full_directory.display()
                    ));
                }
            };
            let mut expected = HashMap::with_capacity(candidates.len());
            for candidate in candidates {
                let file_name = Path::new(&candidate.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| format!("invalid updater Arcade path {}", candidate.path))?;
                expected.insert(file_name.to_ascii_lowercase(), *candidate);
            }
            for entry in entries {
                let entry = entry.map_err(|error| {
                    format!(
                        "enumerate indexed Arcade directory {}: {error}",
                        full_directory.display()
                    )
                })?;
                let name_key = entry.file_name().to_string_lossy().to_ascii_lowercase();
                let Some(candidate) = expected.remove(&name_key) else {
                    continue;
                };
                let file_type = entry.file_type().map_err(|error| {
                    format!(
                        "inspect indexed Arcade path {}: {error}",
                        entry.path().display()
                    )
                })?;
                if file_type.is_symlink() || !file_type.is_file() {
                    continue;
                }
                let size = if verify_index_size {
                    Some(
                        entry
                            .metadata()
                            .map_err(|error| {
                                format!(
                                    "read indexed Arcade size {}: {error}",
                                    entry.path().display()
                                )
                            })?
                            .len(),
                    )
                } else {
                    None
                };
                installed.push(InstalledMra {
                    full_path: entry.path(),
                    relative_path: candidate.path.clone(),
                    path_key: candidate.path_key.clone(),
                    size,
                });
            }
        }
        Ok(installed)
    };
    let mut installed = probe(&directories)?;
    installed.sort_by(|left, right| left.path_key.cmp(&right.path_key));
    Ok(installed)
}

fn fallback_candidate(installed: &InstalledMra) -> Result<BaseCandidate, String> {
    let bytes = fs::read(&installed.full_path)
        .map_err(|error| format!("read MRA {}: {error}", installed.full_path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let lower = text.to_ascii_lowercase();
    if !lower.contains("<misterromdescription") {
        return Err("missing misterromdescription root".to_string());
    }
    let header = MraHeader {
        name: element_text(&text, &lower, "name"),
        rbf: element_text(&text, &lower, "rbf"),
        platform: element_text(&text, &lower, "platform"),
        manufacturer: element_text(&text, &lower, "manufacturer"),
        year: element_text(&text, &lower, "year"),
        setname: element_text(&text, &lower, "setname"),
        parent: element_text(&text, &lower, "parent"),
    };
    let rom = inspect_rom_requirement(&text, &lower, header.setname.as_deref());
    let stem = installed
        .full_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Arcade");
    let title = header.name.clone().unwrap_or_else(|| stem.to_string());
    let setname = header.setname.clone().unwrap_or_default();
    let identity_id = normalize_id(if setname.is_empty() { &title } else { &setname });
    let family_id = header
        .parent
        .as_deref()
        .map(normalize_id)
        .filter(|parent| parent != "unknown")
        .unwrap_or_else(|| identity_id.clone());
    let variant_score = variant_score(&format!(
        "{} {} {} {}",
        title,
        installed.relative_path,
        setname,
        header.parent.as_deref().unwrap_or("")
    ));
    Ok(BaseCandidate {
        path: installed.relative_path.clone(),
        path_key: installed.path_key.clone(),
        family_id,
        identity_id,
        title,
        manufacturer: header.manufacturer.unwrap_or_default(),
        category: "Arcade".to_string(),
        control: String::new(),
        setname: setname.to_ascii_lowercase(),
        rom,
        expected_size: bytes.len() as u64,
        year: header.year.as_deref().and_then(parse_year),
        players: None,
        needs_fallback: false,
        variant_score,
    })
}

fn select_and_order(candidates: Vec<BaseCandidate>) -> Result<Vec<ActiveRecord>, String> {
    let mut families = BTreeMap::<String, Vec<BaseCandidate>>::new();
    for candidate in candidates {
        families
            .entry(candidate.family_id.clone())
            .or_default()
            .push(candidate);
    }
    let mut records = Vec::new();
    for (family_id, mut variants) in families {
        variants.sort_by(|left, right| {
            let left_parent = left.identity_id == family_id;
            let right_parent = right.identity_id == family_id;
            right_parent
                .cmp(&left_parent)
                .then_with(|| right.variant_score.cmp(&left.variant_score))
                .then_with(|| left.path_key.cmp(&right.path_key))
        });
        for (ordinal, candidate) in variants.into_iter().enumerate() {
            records.push(ActiveRecord {
                path: candidate.path,
                family_id: candidate.family_id,
                identity_id: candidate.identity_id,
                title: candidate.title,
                manufacturer: candidate.manufacturer,
                category: candidate.category,
                control: candidate.control,
                year: candidate.year,
                players: candidate.players,
                preferred: ordinal == 0,
                variant_ordinal: u16::try_from(ordinal)
                    .map_err(|_| format!("Arcade family {family_id} has too many variants"))?,
            });
        }
    }
    records.sort_by_cached_key(|record| {
        (
            record.title.to_ascii_lowercase(),
            record.path.to_ascii_lowercase(),
        )
    });
    Ok(records)
}

fn requirement_from_updater(requirement: &PrimaryRomRequirement) -> RomRequirement {
    match requirement {
        PrimaryRomRequirement::None => RomRequirement::None,
        PrimaryRomRequirement::Archive {
            namespace: RomNamespace::Mame,
            setname,
        } => RomRequirement::Mame(setname.to_ascii_lowercase()),
        PrimaryRomRequirement::Archive {
            namespace: RomNamespace::Hbmame,
            setname,
        } => RomRequirement::Hbmame(setname.to_ascii_lowercase()),
        PrimaryRomRequirement::Ambiguous => RomRequirement::Ambiguous,
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RomEligibility {
    Eligible,
    Missing,
    Ambiguous,
}

fn rom_eligibility(requirement: &RomRequirement, inventory: &RomInventory) -> RomEligibility {
    match requirement {
        RomRequirement::None => RomEligibility::Eligible,
        RomRequirement::Mame(setname) => {
            if inventory.mame.contains(setname) {
                RomEligibility::Eligible
            } else {
                RomEligibility::Missing
            }
        }
        RomRequirement::Hbmame(setname) => {
            if inventory.hbmame.contains(setname) {
                RomEligibility::Eligible
            } else {
                RomEligibility::Missing
            }
        }
        RomRequirement::Ambiguous => RomEligibility::Ambiguous,
    }
}

fn inspect_rom_requirement(text: &str, lower: &str, setname: Option<&str>) -> RomRequirement {
    let mut groups = Vec::<Vec<(RomNamespace, String)>>::new();
    for tag in rom_tags(text, lower) {
        let archives = xml_attribute(tag, "zip")
            .into_iter()
            .flat_map(|value| value.split('|'))
            .filter_map(normalize_rom_archive)
            .collect::<Vec<_>>();
        if !archives.is_empty() {
            groups.push(archives);
        }
    }
    let mut archives = groups.iter().flatten().cloned().collect::<Vec<_>>();
    archives.sort_by(|left, right| {
        rom_namespace_key(left.0)
            .cmp(&rom_namespace_key(right.0))
            .then_with(|| left.1.cmp(&right.1))
    });
    archives.dedup();
    if archives.is_empty() {
        return RomRequirement::None;
    }
    if let Some(setname) = setname
        .map(normalize_rom_setname)
        .filter(|setname| !setname.is_empty())
    {
        let matches = archives
            .iter()
            .filter(|(_, candidate)| candidate == &setname)
            .collect::<Vec<_>>();
        if let [requirement] = matches.as_slice() {
            return concrete_rom_requirement(requirement.0, &requirement.1);
        }
        if matches.is_empty() && groups.len() == 1 {
            let requirement = &groups[0][0];
            return concrete_rom_requirement(requirement.0, &requirement.1);
        }
        return RomRequirement::Ambiguous;
    }
    if let [requirement] = archives.as_slice() {
        concrete_rom_requirement(requirement.0, &requirement.1)
    } else {
        RomRequirement::Ambiguous
    }
}

fn rom_tags<'a>(text: &'a str, lower: &str) -> Vec<&'a str> {
    let mut tags = Vec::new();
    let mut offset = 0usize;
    while let Some(relative) = lower[offset..].find('<') {
        let start = offset + relative;
        let Some(end_relative) = lower[start..].find('>') else {
            break;
        };
        let end = start + end_relative + 1;
        let tag = lower[start + 1..end - 1].trim_start();
        let name_end = tag
            .find(|character: char| character.is_ascii_whitespace() || character == '/')
            .unwrap_or(tag.len());
        if matches!(&tag[..name_end], "rom" | "part") {
            tags.push(&text[start..end]);
        }
        offset = end;
    }
    tags
}

fn xml_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut offset = 0usize;
    while let Some(relative) = lower[offset..].find(name) {
        let start = offset + relative;
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let mut cursor = start + name.len();
        let after_ok = cursor >= bytes.len() || !bytes[cursor].is_ascii_alphanumeric();
        if before_ok && after_ok {
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'=') {
                cursor += 1;
                while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                    cursor += 1;
                }
                let quote = *bytes.get(cursor)?;
                if matches!(quote, b'\'' | b'"') {
                    let value_start = cursor + 1;
                    let value_end = bytes[value_start..]
                        .iter()
                        .position(|byte| *byte == quote)?
                        + value_start;
                    return tag.get(value_start..value_end);
                }
            }
        }
        offset = start + name.len();
    }
    None
}

fn normalize_rom_archive(value: &str) -> Option<(RomNamespace, String)> {
    let normalized = value.trim().trim_start_matches('/').replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    let namespace = if lower.starts_with("hbmame/") {
        RomNamespace::Hbmame
    } else {
        RomNamespace::Mame
    };
    let filename = lower.rsplit('/').next()?;
    let setname = filename.strip_suffix(".zip")?;
    (!setname.is_empty()).then(|| (namespace, normalize_rom_setname(setname)))
}

fn concrete_rom_requirement(namespace: RomNamespace, setname: &str) -> RomRequirement {
    match namespace {
        RomNamespace::Mame => RomRequirement::Mame(setname.to_string()),
        RomNamespace::Hbmame => RomRequirement::Hbmame(setname.to_string()),
    }
}

fn rom_namespace_key(namespace: RomNamespace) -> u8 {
    match namespace {
        RomNamespace::Mame => 0,
        RomNamespace::Hbmame => 1,
    }
}

fn normalize_rom_setname(value: &str) -> String {
    let lower = value.trim().to_ascii_lowercase();
    lower.strip_suffix(".zip").unwrap_or(&lower).to_string()
}

fn element_text(text: &str, lower: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut offset = 0usize;
    while let Some(relative) = lower[offset..].find(&open) {
        let start = offset + relative;
        let after_name = start + open.len();
        let boundary = lower.as_bytes().get(after_name).copied();
        if !boundary.is_some_and(|byte| byte == b'>' || byte.is_ascii_whitespace()) {
            offset = after_name;
            continue;
        }
        let content_start = lower[after_name..].find('>')? + after_name + 1;
        let content_end = lower[content_start..].find(&close)? + content_start;
        let value = decode_xml_text(text.get(content_start..content_end)?.trim());
        return (!value.is_empty()).then_some(value);
    }
    None
}

fn decode_xml_text(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn parse_year(value: &str) -> Option<u16> {
    value
        .as_bytes()
        .windows(4)
        .find(|window| window.iter().all(u8::is_ascii_digit))
        .and_then(|digits| std::str::from_utf8(digits).ok())
        .and_then(|digits| digits.parse().ok())
}

fn normalize_id(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for character in value.trim().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            out.push(character);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

fn variant_score(haystack: &str) -> i32 {
    let haystack = haystack.to_ascii_lowercase();
    let mut score = 0;
    if contains_any(
        &haystack,
        &[
            "(usa", "(us,", "(us)", "(u)", "/_usa/", " america", "american",
        ],
    ) {
        score += 1000;
    } else if contains_any(&haystack, &["(japan", "(jp", "(j)", "/_japan/"]) {
        score += 900;
    } else if contains_any(&haystack, &["(world", "(w,", "(w)", "/_world/"]) {
        score += 800;
    } else if contains_any(&haystack, &["(europe", "(eu", "(e)", "/_europe/"]) {
        score += 700;
    }
    for penalty in [
        "prototype",
        "bootleg",
        "[hack",
        " hack",
        "hbmame",
        "homebrew",
        "[hb]",
        "training",
        "unlocked",
        "free play",
        "low lag",
        "fix",
        "patched",
        "beta",
        "sample",
    ] {
        if haystack.contains(penalty) {
            score -= 300;
        }
    }
    score
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn timed<T>(operation: impl FnOnce() -> T) -> (T, u64) {
    let started = Instant::now();
    let result = operation();
    (result, elapsed_us(started))
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros() as u64
}

fn as_u32(value: usize, label: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{label} exceeds u32"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_inspection_selects_setname_archive() {
        let text = r#"<misterromdescription>
            <name>Puck Man</name><setname>puckman</setname><parent>pacman</parent>
            <rom zip="bios.zip|puckman.zip" />
        </misterromdescription>"#;
        assert_eq!(
            inspect_rom_requirement(text, &text.to_ascii_lowercase(), Some("puckman")),
            RomRequirement::Mame("puckman".to_string())
        );
    }

    #[test]
    fn parent_is_preferred_even_when_child_region_scores_higher() {
        let mut parent = BaseCandidate {
            path: "_Arcade/Parent.mra".to_string(),
            path_key: "_arcade/parent.mra".to_string(),
            family_id: "family".to_string(),
            identity_id: "family".to_string(),
            title: "Parent".to_string(),
            manufacturer: String::new(),
            category: "Arcade".to_string(),
            control: String::new(),
            setname: "family".to_string(),
            rom: RomRequirement::None,
            expected_size: 1,
            year: None,
            players: None,
            needs_fallback: false,
            variant_score: 0,
        };
        let mut child = parent.clone();
        child.path = "_Arcade/Child (USA).mra".to_string();
        child.path_key = child.path.to_ascii_lowercase();
        child.identity_id = "child".to_string();
        child.variant_score = 1000;
        let records = select_and_order(vec![child, parent.clone()]).unwrap();
        parent.path = records
            .iter()
            .find(|record| record.preferred)
            .unwrap()
            .path
            .clone();
        assert_eq!(parent.path, "_Arcade/Parent.mra");
    }
}
