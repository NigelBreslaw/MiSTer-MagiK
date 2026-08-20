// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Game discovery and playable filtering.

use crate::catalog_scan::{self, FoundFile};
use crate::launch_profiles::{LaunchProfile, PayloadRule, RuleSourceKind};
use crate::library_db::{
    self, AMIGAVISION_GAME_LAUNCH_PREFIX, AMIGAVISION_LAUNCHER_REF, LibraryContainerEntry,
};
use crate::media_metadata;
use crate::prepared_collections::PreparedLaunchProvenance;
use crate::prepared_collections::{self, PreparedCollectionId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct GameDiscovery {
    pub(crate) source_path: String,
    pub(crate) launch_ref: String,
    pub(crate) source_kind: DiscoverySourceKind,
    pub(crate) title: String,
    pub(crate) category: String,
    pub(crate) platform_id: String,
    pub(crate) core_id: String,
    pub(crate) hardware_id: String,
    pub(crate) manufacturer: Option<String>,
    pub(crate) genre: Option<String>,
    pub(crate) year: Option<u16>,
    pub(crate) setname: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) covered_payload_path: Option<String>,
    pub(crate) prepared: Option<PreparedLaunchProvenance>,
    pub(crate) confidence: DiscoveryConfidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DiscoverySourceKind {
    Mra,
    Mgl,
    PayloadFile,
    ArchiveEntry,
    CatalogEntry,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum DiscoveryConfidence {
    MraHardware,
    MraCore,
    PayloadPath,
    Extension,
    ArchiveToc,
    CatalogMetadata,
}

pub(crate) fn variant_score_from_haystack(haystack: &str) -> i32 {
    let mut score = 0;
    if contains_any(
        haystack,
        &[
            "(usa", "(us,", "(us)", "(u)", "/_usa/", " america", "american",
        ],
    ) {
        score += 1000;
    } else if contains_any(haystack, &["(japan", "(jp", "(j)", "/_japan/"]) {
        score += 900;
    } else if contains_any(haystack, &["(world", "(w,", "(w)", "/_world/"]) {
        score += 800;
    } else if contains_any(haystack, &["(europe", "(eu", "(e)", "/_europe/"]) {
        score += 700;
    }

    for bad in [
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
        if haystack.contains(bad) {
            score -= 300;
        }
    }

    if let Some(disc_number) = first_disc_number_from_haystack(haystack) {
        if disc_number == 1 {
            score += 100;
        } else {
            score -= 100;
        }
    }

    score
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

pub(crate) fn first_disc_number_from_haystack(haystack: &str) -> Option<u32> {
    let haystack = haystack.to_ascii_lowercase();
    let haystack = haystack.as_str();
    for marker in ["disc", "disk", "cd"] {
        if let Some(number) = number_after_marker(haystack, marker) {
            return Some(number);
        }
    }
    for number in 1..=9 {
        if haystack.contains(&format!("({number} of ")) {
            return Some(number);
        }
    }
    None
}

fn number_after_marker(haystack: &str, marker: &str) -> Option<u32> {
    let bytes = haystack.as_bytes();
    let marker_bytes = marker.as_bytes();
    let mut start = 0usize;
    while let Some(pos) = haystack[start..].find(marker) {
        let marker_start = start + pos;
        let marker_end = marker_start + marker_bytes.len();
        let before = marker_start
            .checked_sub(1)
            .and_then(|idx| bytes.get(idx))
            .copied();
        let after_marker = bytes.get(marker_end).copied();
        if before.is_none_or(|byte| !byte.is_ascii_alphanumeric())
            && after_marker.is_some_and(|byte| !byte.is_ascii_alphabetic())
        {
            let mut digit_start = marker_end;
            while bytes
                .get(digit_start)
                .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'-' || *byte == b'_')
            {
                digit_start += 1;
            }
            let mut digit_end = digit_start;
            while bytes
                .get(digit_end)
                .is_some_and(|byte| byte.is_ascii_digit())
            {
                digit_end += 1;
            }
            if digit_end > digit_start
                && let Ok(number) = haystack[digit_start..digit_end].parse()
            {
                return Some(number);
            }
        }
        start = marker_end;
    }
    None
}

fn normalize_launch_path(path: &str) -> String {
    let trimmed = path.trim().trim_start_matches("./");
    let absolute = trimmed.starts_with('/');
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                let _ = parts.pop();
            }
            value => parts.push(value),
        }
    }
    let mut normalized = parts.join("/");
    if absolute {
        normalized.insert(0, '/');
    }
    normalized.to_ascii_lowercase()
}

#[cfg(test)]
pub(crate) fn discovery_from_profile_file(
    file: &FoundFile,
    profile: &LaunchProfile,
    rule: &PayloadRule,
    profiles: &[LaunchProfile],
) -> GameDiscovery {
    discovery_from_profile_file_with_prepared_index(file, profile, rule, profiles, None)
}

pub(crate) fn discovery_from_profile_file_with_prepared_index(
    file: &FoundFile,
    profile: &LaunchProfile,
    rule: &PayloadRule,
    profiles: &[LaunchProfile],
    prepared_index: Option<&prepared_collections::PreparedPayloadIndex>,
) -> GameDiscovery {
    discovery_from_profile_file_with_prepared_index_and_mra_metadata(
        file,
        profile,
        rule,
        profiles,
        prepared_index,
        None,
    )
}

pub(crate) fn discovery_from_profile_file_with_prepared_index_and_mra_metadata(
    file: &FoundFile,
    profile: &LaunchProfile,
    rule: &PayloadRule,
    profiles: &[LaunchProfile],
    prepared_index: Option<&prepared_collections::PreparedPayloadIndex>,
    prefetched_mra: Option<Option<media_metadata::MraMetadata>>,
) -> GameDiscovery {
    let source_path = file.path.display().to_string();
    if file.ext == "mra"
        && let Some(mra) =
            prefetched_mra.unwrap_or_else(|| media_metadata::read_mra_metadata(&file.path))
    {
        let core_id = mra
            .rbf
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(library_db::normalize_id)
            .unwrap_or_else(|| profile.core_name.to_string());
        let hardware_id = mra
            .platform
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(library_db::normalize_id)
            .unwrap_or_else(|| core_id.clone());
        return GameDiscovery {
            source_path: source_path.clone(),
            launch_ref: source_path.clone(),
            source_kind: DiscoverySourceKind::Mra,
            title: mra
                .name
                .unwrap_or_else(|| library_db::title_from_path(&source_path)),
            category: profile.category.to_string(),
            platform_id: profile.system_id.to_string(),
            core_id,
            hardware_id,
            manufacturer: mra.manufacturer,
            genre: None,
            year: mra.year.and_then(|s| s.parse::<u16>().ok()),
            setname: mra.setname,
            parent: mra.parent,
            covered_payload_path: None,
            prepared: None,
            confidence: if mra.platform.is_some() {
                DiscoveryConfidence::MraHardware
            } else {
                DiscoveryConfidence::MraCore
            },
        };
    }
    if file.ext == "mgl"
        && let Some(document) = media_metadata::read_mgl_document(&file.path)
    {
        let media_metadata::MglDocument {
            metadata: mgl,
            inspection,
        } = document;
        let payload_profile = mgl
            .file_path
            .as_deref()
            .filter(|_| profile.id == "mgl")
            .and_then(|payload| profile_for_mgl_payload(profiles, &file.path, payload));
        let profile = payload_profile.unwrap_or(profile);
        let setname = if profile.system_id == "neogeo" {
            media_metadata::neogeo_mgl_setname(&file.path, mgl.file_path.as_deref())
        } else if profile.id == "neon68k" {
            mgl.setname.clone()
        } else {
            None
        };
        let covered_payload_path = mgl.file_path.as_deref().map(|payload| {
            let path = if profile.system_id == "dos"
                && file.path.components().any(|component| {
                    component
                        .as_os_str()
                        .to_str()
                        .is_some_and(|value| value.eq_ignore_ascii_case("_DOS Games"))
                }) {
                prepared_index.map_or_else(
                    || prepared_collections::resolve_0mhz_payload_path(&file.path, payload),
                    |index| index.resolve_0mhz_payload_path(&file.path, payload),
                )
            } else {
                media_metadata::resolve_mgl_payload_path(&file.path, payload)
            };
            path.display().to_string()
        });
        let prepared = (profile.system_id == "dos"
            && inspection.as_ref().is_ok_and(|inspection| {
                prepared_index
                    .map_or_else(
                        || {
                            prepared_collections::validate_0mhz_mgl_inspection(
                                &file.path, inspection,
                            )
                        },
                        |index| {
                            prepared_collections::validate_0mhz_mgl_inspection_with_index(
                                &file.path, inspection, index,
                            )
                        },
                    )
                    .is_ok()
            }))
        .then(|| PreparedLaunchProvenance::prepared(PreparedCollectionId::ZeroMhz));
        let prepared = prepared.or_else(|| {
            (profile.id == "neon68k"
                && inspection.as_ref().is_ok_and(|inspection| {
                    prepared_collections::validate_neon68k_mgl_inspection(&file.path, inspection)
                        .is_ok()
                }))
            .then(|| PreparedLaunchProvenance::prepared(PreparedCollectionId::Neon68k))
        });
        let genre = if prepared
            .is_some_and(|value| value.collection_id == PreparedCollectionId::ZeroMhz)
        {
            Some("0MHz".to_string())
        } else if prepared.is_some_and(|value| value.collection_id == PreparedCollectionId::Neon68k)
        {
            Some(
                prepared_collections::neon68k_source_category(&file.path)
                    .map(|category| format!("Neon68K / {category}"))
                    .unwrap_or_else(|| "Neon68K".to_string()),
            )
        } else {
            None
        };
        return GameDiscovery {
            source_path: source_path.clone(),
            launch_ref: source_path.clone(),
            source_kind: DiscoverySourceKind::Mgl,
            title: library_db::title_from_path(&source_path),
            category: profile.category.to_string(),
            platform_id: profile.system_id.to_string(),
            core_id: mgl
                .rbf
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(library_db::normalize_id)
                .unwrap_or_else(|| profile.core_name.to_string()),
            hardware_id: profile.system_id.to_string(),
            manufacturer: None,
            genre,
            year: None,
            setname,
            parent: None,
            covered_payload_path,
            prepared,
            confidence: DiscoveryConfidence::PayloadPath,
        };
    }

    let payload_setname = if profile.system_id == "neogeo" {
        media_metadata::parenthesized_setname(&source_path)
    } else if matches!(profile.id.as_str(), "mame" | "hbmame") {
        file.path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(library_db::normalize_id)
    } else {
        None
    };
    let prepared = (profile.system_id == "c64")
        .then(|| prepared_collections::oneload64_provenance(&file.path))
        .flatten();
    let genre = prepared
        .is_some_and(|value| value.collection_id == PreparedCollectionId::OneLoad64)
        .then(|| "OneLoad64".to_string());

    GameDiscovery {
        source_path: source_path.clone(),
        launch_ref: source_path.clone(),
        source_kind: DiscoverySourceKind::PayloadFile,
        title: library_db::title_from_path(&source_path),
        category: profile.category.to_string(),
        platform_id: profile.system_id.to_string(),
        core_id: profile.core_name.to_string(),
        hardware_id: profile.system_id.to_string(),
        manufacturer: None,
        genre,
        year: None,
        setname: payload_setname,
        parent: None,
        covered_payload_path: None,
        prepared,
        confidence: profile_confidence(rule),
    }
}

pub(crate) fn discovery_from_profile_archive_entry(
    entry: &LibraryContainerEntry,
    profile: &LaunchProfile,
    rule: &PayloadRule,
) -> GameDiscovery {
    GameDiscovery {
        source_path: format!("{}::{}", entry.file_path, entry.entry_path),
        launch_ref: entry.launch_ref.clone(),
        source_kind: DiscoverySourceKind::ArchiveEntry,
        title: library_db::title_from_path(&entry.entry_path),
        category: profile.category.to_string(),
        platform_id: profile.system_id.to_string(),
        core_id: profile.core_name.to_string(),
        hardware_id: profile.system_id.to_string(),
        manufacturer: None,
        genre: None,
        year: None,
        setname: media_metadata::parenthesized_setname(&entry.entry_path),
        parent: None,
        covered_payload_path: None,
        prepared: None,
        confidence: match rule.provenance.kind {
            RuleSourceKind::MainSource | RuleSourceKind::Mgl | RuleSourceKind::Mra => {
                DiscoveryConfidence::ArchiveToc
            }
            RuleSourceKind::ConfStr | RuleSourceKind::MagikProfile => profile_confidence(rule),
        },
    }
}

fn profile_for_mgl_payload<'a>(
    profiles: &'a [LaunchProfile],
    mgl_path: &Path,
    payload: &str,
) -> Option<&'a LaunchProfile> {
    let path = media_metadata::resolve_mgl_payload_path(mgl_path, payload);
    catalog_scan::profile_for_path(profiles, &path)
}

pub(crate) fn profile_confidence(rule: &PayloadRule) -> DiscoveryConfidence {
    match rule.provenance.kind {
        RuleSourceKind::Mra => DiscoveryConfidence::MraCore,
        RuleSourceKind::Mgl | RuleSourceKind::MainSource | RuleSourceKind::MagikProfile => {
            DiscoveryConfidence::PayloadPath
        }
        RuleSourceKind::ConfStr => DiscoveryConfidence::Extension,
    }
}

pub(crate) fn unique_discovery_count(discoveries: &[GameDiscovery]) -> usize {
    let covered_payloads = covered_payload_paths(discoveries);
    preferred_playable_discovery_indices_by_key(discoveries, &covered_payloads).len()
}

pub(crate) fn preferred_playable_discovery_indices_by_key(
    discoveries: &[GameDiscovery],
    covered_payloads: &HashSet<String>,
) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::<String, usize>::new();
    for (index, discovery) in discoveries.iter().enumerate() {
        if !is_playable_discovery_with_coverage(discovery, covered_payloads) {
            continue;
        }
        let key = discovery_unique_key(discovery);
        match out.get(&key).copied() {
            Some(existing) if prefer_discovery_variant(discovery, &discoveries[existing]) => {
                out.insert(key, index);
            }
            None => {
                out.insert(key, index);
            }
            _ => {}
        }
    }
    out
}

pub(crate) fn preferred_playable_discoveries_by_key<'a>(
    discoveries: &'a [GameDiscovery],
    covered_payloads: &HashSet<String>,
) -> BTreeMap<String, &'a GameDiscovery> {
    preferred_playable_discovery_indices_by_key(discoveries, covered_payloads)
        .into_iter()
        .map(|(key, index)| (key, &discoveries[index]))
        .collect()
}

fn prefer_discovery_variant(a: &GameDiscovery, b: &GameDiscovery) -> bool {
    let a_score = discovery_variant_score(a);
    let b_score = discovery_variant_score(b);
    if a_score != b_score {
        return a_score > b_score;
    }
    normalize_launch_path(&a.launch_ref) < normalize_launch_path(&b.launch_ref)
}

fn discovery_variant_score(discovery: &GameDiscovery) -> i32 {
    let haystack = format!(
        "{} {} {} {}",
        discovery.title,
        discovery.launch_ref,
        discovery.setname.as_deref().unwrap_or(""),
        discovery.parent.as_deref().unwrap_or("")
    )
    .to_ascii_lowercase();

    variant_score_from_haystack(&haystack)
}

pub(crate) fn confidence_str(confidence: DiscoveryConfidence) -> &'static str {
    match confidence {
        DiscoveryConfidence::MraHardware => "mra-hardware",
        DiscoveryConfidence::MraCore => "mra-core",
        DiscoveryConfidence::PayloadPath => "payload-path",
        DiscoveryConfidence::Extension => "extension",
        DiscoveryConfidence::ArchiveToc => "archive-toc",
        DiscoveryConfidence::CatalogMetadata => "catalog-metadata",
    }
}

pub(crate) fn discovery_unique_key(d: &GameDiscovery) -> String {
    match d.source_kind {
        DiscoverySourceKind::Mra => {
            if let Some(setname) = d.setname.as_deref().filter(|s| !s.trim().is_empty()) {
                format!("mra:set:{setname}")
            } else {
                format!(
                    "mra:title:{}:{}",
                    d.hardware_id,
                    library_db::normalize_id(&d.title)
                )
            }
        }
        DiscoverySourceKind::Mgl => format!("payload:{}", d.launch_ref),
        DiscoverySourceKind::PayloadFile => format!("payload:{}", d.launch_ref),
        DiscoverySourceKind::ArchiveEntry => format!("archive:{}", d.launch_ref),
        DiscoverySourceKind::CatalogEntry => format!("catalog:{}:{}", d.launch_ref, d.title),
    }
}

#[cfg(test)]
pub(crate) fn is_playable_discovery(d: &GameDiscovery) -> bool {
    is_playable_discovery_with_coverage(d, &HashSet::new())
}

pub(crate) fn is_playable_discovery_with_coverage(
    d: &GameDiscovery,
    covered_payloads: &HashSet<String>,
) -> bool {
    if is_raw_arcade_zip_set_discovery(d) {
        return false;
    }
    match d.source_kind {
        DiscoverySourceKind::Mra => true,
        DiscoverySourceKind::Mgl => is_launcher_launch_ref(&d.launch_ref),
        DiscoverySourceKind::PayloadFile | DiscoverySourceKind::ArchiveEntry => {
            !covered_payloads.contains(&normalize_launch_path(&d.launch_ref))
        }
        DiscoverySourceKind::CatalogEntry => is_launcher_launch_ref(&d.launch_ref),
    }
}

pub(crate) fn covered_payload_paths(discoveries: &[GameDiscovery]) -> HashSet<String> {
    let mut covered = HashSet::new();
    for discovery in discoveries {
        if discovery.source_kind != DiscoverySourceKind::Mgl {
            continue;
        }
        if let Some(payload) = discovery.covered_payload_path.as_deref() {
            covered.insert(normalize_launch_path(payload));
        }
    }
    covered
}

pub(crate) fn launch_kind_for_discovery(discovery: &GameDiscovery) -> &'static str {
    match discovery.source_kind {
        DiscoverySourceKind::Mra => "mra",
        DiscoverySourceKind::Mgl => "mgl",
        DiscoverySourceKind::PayloadFile | DiscoverySourceKind::ArchiveEntry => "virtual-mgl",
        DiscoverySourceKind::CatalogEntry => "catalog-entry",
    }
}

pub(crate) fn launch_ref_for_discovery(game_id: &str, discovery: &GameDiscovery) -> String {
    match discovery.source_kind {
        DiscoverySourceKind::Mra | DiscoverySourceKind::Mgl | DiscoverySourceKind::CatalogEntry => {
            discovery.launch_ref.clone()
        }
        DiscoverySourceKind::PayloadFile | DiscoverySourceKind::ArchiveEntry => {
            virtual_launch_ref(game_id)
        }
    }
}

pub(crate) fn virtual_launch_ref(game_id: &str) -> String {
    format!("magik-plan:{game_id}")
}

pub(crate) fn profile_id_for_discovery(discovery: &GameDiscovery) -> Option<&str> {
    if discovery.platform_id == "unknown" || discovery.platform_id.is_empty() {
        None
    } else {
        Some(discovery.platform_id.as_str())
    }
}

pub(crate) fn is_launcher_launch_ref(path: &str) -> bool {
    if path.starts_with("magik-plan:")
        || path.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX)
        || path == AMIGAVISION_LAUNCHER_REF
    {
        return true;
    }
    match library_db::path_ext(path).as_deref() {
        Some("mra" | "mgl") => !path.contains("::"),
        _ => false,
    }
}

pub(crate) fn catalog_system_id_for_discovery(discovery: &GameDiscovery) -> String {
    if discovery.platform_id == "arcade"
        || (discovery.category == "Arcade" && discovery.source_kind == DiscoverySourceKind::Mra)
    {
        "arcade".to_string()
    } else if discovery.platform_id.is_empty() {
        "unknown".to_string()
    } else if discovery.platform_id == "amiga500" {
        "amiga".to_string()
    } else {
        discovery.platform_id.clone()
    }
}

pub(crate) fn is_raw_arcade_zip_set_discovery(discovery: &GameDiscovery) -> bool {
    discovery.platform_id == "arcade"
        && discovery.core_id == "Arcade"
        && discovery.source_kind == DiscoverySourceKind::PayloadFile
        && library_db::path_ext(&discovery.source_path)
            .as_deref()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
        && Path::new(&discovery.source_path)
            .components()
            .any(|component| {
                component.as_os_str().to_str().is_some_and(|part| {
                    part.eq_ignore_ascii_case("mame") || part.eq_ignore_ascii_case("hbmame")
                })
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog_scan::{FoundFile, profile_for_path};
    use crate::launch_profiles;
    use crate::library_db::mtime_secs;
    use crate::test_support::*;

    #[test]
    fn amiga_500_content_joins_the_amiga_launcher_system() {
        let mut discovery = payload("/media/fat/games/Amiga500/Game.adf");
        discovery.platform_id = "amiga500".to_string();

        assert_eq!(catalog_system_id_for_discovery(&discovery), "amiga");
    }

    #[test]
    fn raw_profile_payloads_generate_virtual_games() {
        let discoveries = vec![
            payload("/media/fat/games/NES/Super Mario Bros.nes"),
            payload("/media/fat/games/Saturn/Guardian Heroes.cue"),
        ];

        assert_eq!(unique_discovery_count(&discoveries), 2);
    }

    #[test]
    fn dos_mgl_games_still_count_as_games() {
        let discoveries = vec![mgl(
            "/media/fat/_DOS Games/Doom (Ultimate).mgl",
            "/media/fat/_DOS Games/Doom (Ultimate).mgl",
        )];

        assert_eq!(unique_discovery_count(&discoveries), 1);
    }

    #[test]
    fn dos_mgl_discovery_uses_dos_system_without_payload_inference() {
        let root = unique_temp_dir("dos-mgl-profile");
        let dos_dir = root.join("_DOS Games");
        std::fs::create_dir_all(&dos_dir).expect("create dos dir");
        let path = dos_dir.join("Doom (Ultimate).mgl");
        let payload = dos_dir.join("Doom.vhd");
        std::fs::write(&payload, b"vhd").expect("write DOS payload");
        std::fs::write(
            &path,
            r#"<mistergamedescription><rbf>AO486</rbf><file delay="1" type="s" index="2" path="Doom.vhd"/><reset/></mistergamedescription>"#,
        )
        .expect("write dos mgl fixture");
        let meta = std::fs::metadata(&path).expect("stat dos mgl fixture");
        let file = FoundFile {
            path: path.clone(),
            ext: "mgl".to_string(),
            size: meta.len(),
            mtime_secs: mtime_secs(&meta),
        };

        let profiles = launch_profiles::builtin_profiles();
        let profile = profile_for_path(&profiles, &path).expect("dos profile");
        let payload_rule = &profile.payload_rules[0];
        let discovery = discovery_from_profile_file(&file, profile, payload_rule, &profiles);

        assert_eq!(profile.id, "dos");
        assert_eq!(discovery.platform_id, "dos");
        assert_eq!(catalog_system_id_for_discovery(&discovery), "dos");
        assert_eq!(crate::catalog_classify::system_title("dos"), "DOS Games");
        assert_eq!(discovery.genre.as_deref(), Some("0MHz"));
        assert_eq!(
            discovery.prepared.map(|value| value.collection_id),
            Some(PreparedCollectionId::ZeroMhz)
        );
    }

    #[test]
    fn mgl_discovery_preserves_script_as_launch_ref() {
        let path =
            std::env::temp_dir().join(format!("mister-magik-mgl-test-{}.mgl", std::process::id()));
        std::fs::write(
            &path,
            r#"<mistergamelist><file delay="2" type="s" path="games/NES/Mario.nes"/></mistergamelist>"#,
        )
        .expect("write mgl fixture");
        let meta = std::fs::metadata(&path).expect("stat mgl fixture");
        let file = FoundFile {
            path: path.clone(),
            ext: "mgl".to_string(),
            size: meta.len(),
            mtime_secs: mtime_secs(&meta),
        };

        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "mgl")
            .expect("mgl profile");
        let payload_rule = &profile.payload_rules[0];
        let discovery = discovery_from_profile_file(&file, profile, payload_rule, &profiles);

        assert_eq!(discovery.source_path, path.display().to_string());
        assert_eq!(discovery.launch_ref, path.display().to_string());
        assert_eq!(discovery.platform_id, "nes");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn neogeo_mgl_discovery_uses_payload_setname() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-neogeo-mgl-test-{}.mgl",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"<mistergamelist><rbf>NeoGeo</rbf><file delay="2" type="s" path="/media/fat/games/NeoGeo/Neo Geo Mister FGPA Ultra Pack.zip/Neo Geo Mister FGPA Ultra Pack/ World A-Z/Metal Slug 3 (mslug3).neo"/></mistergamelist>"#,
        )
        .expect("write mgl fixture");
        let meta = std::fs::metadata(&path).expect("stat mgl fixture");
        let file = FoundFile {
            path: path.clone(),
            ext: "mgl".to_string(),
            size: meta.len(),
            mtime_secs: mtime_secs(&meta),
        };

        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "mgl")
            .expect("mgl profile");
        let payload_rule = &profile.payload_rules[0];
        let discovery = discovery_from_profile_file(&file, profile, payload_rule, &profiles);

        assert_eq!(discovery.platform_id, "neogeo");
        assert_eq!(discovery.setname.as_deref(), Some("mslug3"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn neogeo_payload_discovery_uses_filename_setname() {
        let path = std::path::PathBuf::from(
            "/media/fat/games/NEOGEO/Neo Geo Mister FGPA Ultra Pack/ World A-Z/3 Count Bout (3countb).neo",
        );
        let file = FoundFile {
            path,
            ext: "neo".to_string(),
            size: 123,
            mtime_secs: 0,
        };

        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "neogeo")
            .expect("neogeo profile");
        let payload_rule = &profile.payload_rules[0];
        let discovery = discovery_from_profile_file(&file, profile, payload_rule, &profiles);

        assert_eq!(discovery.platform_id, "neogeo");
        assert_eq!(discovery.setname.as_deref(), Some("3countb"));
    }

    #[test]
    fn mgl_covered_payload_does_not_get_virtual_duplicate() {
        let path = "/media/fat/_Console/NES/Mario.mgl";
        let discoveries = vec![
            GameDiscovery {
                source_path: path.to_string(),
                launch_ref: path.to_string(),
                source_kind: DiscoverySourceKind::Mgl,
                title: "Mario".to_string(),
                category: "Console".to_string(),
                platform_id: "nes".to_string(),
                core_id: "nes".to_string(),
                hardware_id: "nes".to_string(),
                manufacturer: None,
                genre: None,
                year: None,
                setname: None,
                parent: None,
                covered_payload_path: Some("/media/fat/games/NES/Mario.nes".to_string()),
                prepared: None,
                confidence: DiscoveryConfidence::PayloadPath,
            },
            payload("/media/fat/games/NES/Mario.nes"),
        ];

        assert_eq!(unique_discovery_count(&discoveries), 1);
    }

    #[test]
    fn mgl_covered_payload_normalizes_parent_components() {
        let mut launcher = mgl("/media/fat/_Games/Mario.mgl", "/media/fat/_Games/Mario.mgl");
        launcher.covered_payload_path =
            Some("/media/fat/_Games/../games/NES/Mario.nes".to_string());

        let discoveries = vec![launcher, payload("/media/fat/games/NES/Mario.nes")];

        assert_eq!(unique_discovery_count(&discoveries), 1);
    }

    #[test]
    fn mra_files_remain_playable_launchers() {
        let discovery = GameDiscovery {
            source_path: "/media/fat/_Arcade/BIOS.mra".to_string(),
            launch_ref: "/media/fat/_Arcade/BIOS.mra".to_string(),
            source_kind: DiscoverySourceKind::Mra,
            title: "BIOS".to_string(),
            category: "Arcade".to_string(),
            platform_id: "arcade".to_string(),
            core_id: "arcade".to_string(),
            hardware_id: "arcade-unknown".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: None,
            parent: None,
            covered_payload_path: None,
            prepared: None,
            confidence: DiscoveryConfidence::MraCore,
        };

        assert!(is_playable_discovery(&discovery));
    }

    #[test]
    fn disc_variant_scoring_does_not_treat_disc_ten_as_disc_one() {
        assert_eq!(
            first_disc_number_from_haystack("/media/fat/games/Saturn/Game Disc 1.chd"),
            Some(1)
        );
        assert_eq!(
            first_disc_number_from_haystack("/media/fat/games/Saturn/Game Disc 10.chd"),
            Some(10)
        );
        assert!(
            variant_score_from_haystack("/media/fat/games/Saturn/Game Disc 1.chd")
                > variant_score_from_haystack("/media/fat/games/Saturn/Game Disc 10.chd")
        );
    }
}
