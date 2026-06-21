//! Game discovery and playable filtering.

use crate::arcade_catalog;
use crate::catalog_scan::{self, FoundFile};
use crate::launch_profiles::{LaunchProfile, PayloadRule, RuleSourceKind};
use crate::library_db::{
    self, LibraryContainerEntry, AMIGAVISION_GAME_LAUNCH_PREFIX, AMIGAVISION_LAUNCHER_REF,
};
use crate::media_metadata;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Clone, Debug)]
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
    pub(crate) confidence: DiscoveryConfidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiscoverySourceKind {
    Mra,
    Mgl,
    PayloadFile,
    ArchiveEntry,
    CatalogEntry,
}

#[derive(Clone, Copy, Debug)]
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
            if digit_end > digit_start {
                if let Ok(number) = haystack[digit_start..digit_end].parse() {
                    return Some(number);
                }
            }
        }
        start = marker_end;
    }
    None
}

fn normalize_launch_path(path: &str) -> String {
    path.replace("/./", "/")
        .trim()
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

pub(crate) fn discovery_from_profile_file(
    file: &FoundFile,
    profile: &LaunchProfile,
    rule: &PayloadRule,
    profiles: &[LaunchProfile],
) -> GameDiscovery {
    if file.ext == "mra" {
        if let Some(mra) = media_metadata::read_mra_metadata(&file.path) {
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
                source_path: file.path.display().to_string(),
                launch_ref: file.path.display().to_string(),
                source_kind: DiscoverySourceKind::Mra,
                title: mra
                    .name
                    .unwrap_or_else(|| library_db::title_from_path(&file.path.display().to_string())),
                category: profile.category.to_string(),
                platform_id: profile.system_id.to_string(),
                core_id,
                hardware_id,
                manufacturer: mra.manufacturer,
                genre: mra.category.or(mra.catver),
                year: mra.year.and_then(|s| s.parse::<u16>().ok()),
                setname: mra.setname,
                parent: mra.parent,
                confidence: if mra.platform.is_some() {
                    DiscoveryConfidence::MraHardware
                } else {
                    DiscoveryConfidence::MraCore
                },
            };
        }
    }
    if file.ext == "mgl" {
        if let Some(mgl) = media_metadata::read_mgl_metadata(&file.path) {
            let payload_profile = mgl
                .file_path
                .as_deref()
                .and_then(|payload| profile_for_mgl_payload(profiles, &file.path, payload));
            let profile = payload_profile.unwrap_or(profile);
            let setname = if profile.system_id == "neogeo" {
                media_metadata::neogeo_mgl_setname(&file.path, mgl.file_path.as_deref())
            } else {
                None
            };
            return GameDiscovery {
                source_path: file.path.display().to_string(),
                launch_ref: file.path.display().to_string(),
                source_kind: DiscoverySourceKind::Mgl,
                title: library_db::title_from_path(&file.path.display().to_string()),
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
                genre: None,
                year: None,
                setname,
                parent: None,
                confidence: DiscoveryConfidence::PayloadPath,
            };
        }
    }

    GameDiscovery {
        source_path: file.path.display().to_string(),
        launch_ref: file.path.display().to_string(),
        source_kind: DiscoverySourceKind::PayloadFile,
        title: library_db::title_from_path(&file.path.display().to_string()),
        category: profile.category.to_string(),
        platform_id: profile.system_id.to_string(),
        core_id: profile.core_name.to_string(),
        hardware_id: profile.system_id.to_string(),
        manufacturer: None,
        genre: None,
        year: None,
        setname: None,
        parent: None,
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
    preferred_playable_discoveries_by_key(discoveries, &covered_payloads).len()
}

pub(crate) fn preferred_playable_discoveries_by_key<'a>(
    discoveries: &'a [GameDiscovery],
    covered_payloads: &HashSet<String>,
) -> BTreeMap<String, &'a GameDiscovery> {
    let mut out = BTreeMap::<String, &'a GameDiscovery>::new();
    for discovery in discoveries {
        if !is_playable_discovery_with_coverage(discovery, covered_payloads) {
            continue;
        }
        let key = discovery_unique_key(discovery);
        match out.get(&key).copied() {
            Some(existing) if prefer_discovery_variant(discovery, existing) => {
                out.insert(key, discovery);
            }
            None => {
                out.insert(key, discovery);
            }
            _ => {}
        }
    }
    out
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
                format!("mra:title:{}:{}", d.hardware_id, library_db::normalize_id(&d.title))
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
        let path = Path::new(&discovery.source_path);
        let Some(mgl) = media_metadata::read_mgl_metadata(path) else {
            continue;
        };
        let Some(payload) = mgl.file_path.as_deref() else {
            continue;
        };
        let resolved = media_metadata::resolve_mgl_payload_path(path, payload);
        covered.insert(normalize_launch_path(&resolved.display().to_string()));
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
    } else {
        discovery.platform_id.clone()
    }
}

pub(crate) fn system_title_for_discovery(_discovery: &GameDiscovery, system_id: &str) -> String {
    arcade_catalog::system_title(system_id)
}
