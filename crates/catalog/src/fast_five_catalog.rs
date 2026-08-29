// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Independent five-system catalog interchange and publication.
//!
//! This deliberately has no dependency on the whole-card scanner, catalog
//! database, resume journal, or catalog state. System-specific fast builders
//! produce final UI rows, and this module publishes only the immutable shards
//! and manifest consumed by the launcher.

use crate::catalog_classify::SystemId;
#[cfg(feature = "builder")]
use crate::catalog_classify::{LauncherSection, system_definition};
use crate::shard_registry::CatalogManifest;
use crate::shard_registry::{RegistryLimits, read_latest_manifest_lazy};
use crate::system_shard::SystemGame;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(feature = "builder")]
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

pub const FAST_FIVE_SNAPSHOT_SCHEMA: &str = "mister-magik-fast-five-snapshot-v2";
const FAST_FIVE_REGISTRY_FINGERPRINT_SCHEMA: &str = "mister-magik-fast-five-snapshot-v1";
pub const FAST_FIVE_SYSTEM_IDS: [&str; 5] = ["amiga", "arcade", "c64", "dos", "x68000"];
pub const GENERIC_EXAMPLE_SYSTEM_IDS: [&str; 4] = ["neogeo", "saturn", "snes", "zx-spectrum"];
pub const EXPANDED_FAST_SYSTEM_IDS: [&str; 9] = [
    "amiga",
    "arcade",
    "c64",
    "dos",
    "neogeo",
    "saturn",
    "snes",
    "x68000",
    "zx-spectrum",
];

#[cfg(feature = "builder")]
struct FastFiveStagingCleanup(Option<std::path::PathBuf>);

#[cfg(feature = "builder")]
impl FastFiveStagingCleanup {
    fn new(path: std::path::PathBuf) -> Self {
        Self(Some(path))
    }

    fn remove(&mut self) -> Result<(), String> {
        let Some(path) = self.0.take() else {
            return Ok(());
        };
        match std::fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("remove staging {}: {error}", path.display())),
        }
    }
}

#[cfg(feature = "builder")]
impl Drop for FastFiveStagingCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[cfg(feature = "builder")]
fn cleanup_fast_five_staging_root(root: &Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect staging {}: {error}", root.display())),
    };
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing to clean unexpected staging path {}",
            root.display()
        ));
    }
    std::fs::remove_dir_all(root)
        .map_err(|error| format!("clean staging {}: {error}", root.display()))
}
#[cfg(feature = "builder")]
const FAST_FIVE_BINARY_MAGIC: &[u8; 8] = b"MGK5SNAP";
#[cfg(feature = "builder")]
const FAST_FIVE_BINARY_VERSION: u32 = 2;
#[cfg(feature = "builder")]
const FAST_FIVE_BINARY_HEADER_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FastFiveSnapshotEncoding {
    Json,
    Postcard,
    PostcardLz4,
    PostcardMmap,
}

impl FastFiveSnapshotEncoding {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "json" => Ok(Self::Json),
            "postcard" => Ok(Self::Postcard),
            "postcard-lz4" => Ok(Self::PostcardLz4),
            "postcard-mmap" => Ok(Self::PostcardMmap),
            _ => Err(format!("unknown fast-five snapshot encoding {value}")),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Postcard => "postcard",
            Self::PostcardLz4 => "postcard-lz4",
            Self::PostcardMmap => "postcard-mmap",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastFiveSnapshot {
    pub schema: String,
    pub source_fingerprint: String,
    pub systems: Vec<FastFiveSystem>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastFiveSystem {
    pub system_id: String,
    pub display_title: String,
    pub games: Vec<SystemGame>,
    #[serde(default)]
    pub variants: Vec<FastFiveGameVariant>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FastFiveVariantRelation {
    LanguageEdition,
    TitleFormatting,
    ArcadeVariant,
}

#[cfg(feature = "builder")]
impl FastFiveVariantRelation {
    fn as_str(self) -> &'static str {
        match self {
            Self::LanguageEdition => "language-edition",
            Self::TitleFormatting => "title-formatting",
            Self::ArcadeVariant => "arcade-variant",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastFiveGameVariant {
    pub family_stable_key: String,
    pub relation: FastFiveVariantRelation,
    pub game: SystemGame,
}

impl FastFiveSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != FAST_FIVE_SNAPSHOT_SCHEMA {
            return Err(format!("unsupported fast-five schema {}", self.schema));
        }
        validate_fingerprint(&self.source_fingerprint)?;
        let actual = self
            .systems
            .iter()
            .map(|system| system.system_id.as_str())
            .collect::<BTreeSet<_>>();
        if self.systems.is_empty() {
            return Err("fast catalog contains no systems".to_string());
        }
        if self.systems.len() != actual.len() {
            return Err("fast catalog contains duplicate system ids".to_string());
        }
        for system in &self.systems {
            SystemId::parse(&system.system_id)
                .map_err(|error| format!("invalid system {}: {error}", system.system_id))?;
            if system.display_title.trim().is_empty() {
                return Err(format!("{} display title is empty", system.system_id));
            }
            let mut stable_keys = BTreeSet::new();
            for game in &system.games {
                if game.title.trim().is_empty() || game.launch_ref.trim().is_empty() {
                    return Err(format!(
                        "{} has an empty title or launch reference",
                        system.system_id
                    ));
                }
                if !game
                    .stable_key
                    .starts_with(&format!("{}\u{1f}", system.system_id))
                {
                    return Err(format!(
                        "{} row has a foreign stable key: {}",
                        system.system_id, game.stable_key
                    ));
                }
                if !stable_keys.insert(game.stable_key.as_str()) {
                    return Err(format!(
                        "{} contains duplicate stable key {}",
                        system.system_id, game.stable_key
                    ));
                }
                if let Some(plan) = &game.launch_plan
                    && (plan.launch_ref != game.launch_ref || plan.system_id != system.system_id)
                {
                    return Err(format!(
                        "{} launch plan is not bound to {}",
                        system.system_id, game.stable_key
                    ));
                }
            }
            for variant in &system.variants {
                if !stable_keys.contains(variant.family_stable_key.as_str()) {
                    return Err(format!(
                        "{} variant {} has a missing family {}",
                        system.system_id, variant.game.stable_key, variant.family_stable_key
                    ));
                }
                if !variant
                    .game
                    .stable_key
                    .starts_with(&format!("{}\u{1f}", system.system_id))
                    || !stable_keys.insert(variant.game.stable_key.as_str())
                {
                    return Err(format!(
                        "{} contains invalid or duplicate variant key {}",
                        system.system_id, variant.game.stable_key
                    ));
                }
                if let Some(plan) = &variant.game.launch_plan
                    && (plan.launch_ref != variant.game.launch_ref
                        || plan.system_id != system.system_id)
                {
                    return Err(format!(
                        "{} variant launch plan is not bound to {}",
                        system.system_id, variant.game.stable_key
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn game_count(&self) -> usize {
        self.systems.iter().map(|system| system.games.len()).sum()
    }

    pub fn variant_count(&self) -> usize {
        self.systems
            .iter()
            .map(|system| system.variants.len())
            .sum()
    }
}

pub fn is_supported_fast_system_set<'a>(system_ids: impl IntoIterator<Item = &'a str>) -> bool {
    let mut any = false;
    for system_id in system_ids {
        any = true;
        if SystemId::parse(system_id).is_err() {
            return false;
        }
    }
    any
}

#[cfg(feature = "builder")]
pub(crate) fn collapse_c64_cross_source_variants(system: &mut FastFiveSystem) -> usize {
    if system.system_id != "c64" || system.games.is_empty() {
        return 0;
    }
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for (index, game) in system.games.iter().enumerate() {
        let key = compact_c64_family_title(&game.title);
        if !key.is_empty() {
            groups.entry(key).or_default().push(index);
        }
    }
    let mut family_for_variant = BTreeMap::<usize, usize>::new();
    for indexes in groups.values() {
        let oneload = indexes
            .iter()
            .copied()
            .filter(|index| is_oneload64_game(&system.games[*index]))
            .collect::<Vec<_>>();
        if oneload.len() != 1 {
            continue;
        }
        let family = oneload[0];
        for index in indexes.iter().copied().filter(|index| *index != family) {
            if !is_oneload64_game(&system.games[index]) {
                family_for_variant.insert(index, family);
            }
        }
    }
    if family_for_variant.is_empty() {
        return 0;
    }
    let family_keys = family_for_variant
        .iter()
        .map(|(variant, family)| (*variant, system.games[*family].stable_key.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut visible = Vec::with_capacity(system.games.len() - family_for_variant.len());
    let mut variants = Vec::with_capacity(family_for_variant.len());
    for (index, game) in system.games.drain(..).enumerate() {
        if let Some(family_stable_key) = family_keys.get(&index) {
            variants.push(FastFiveGameVariant {
                family_stable_key: family_stable_key.clone(),
                relation: if has_language_annotation(&game.title) {
                    FastFiveVariantRelation::LanguageEdition
                } else {
                    FastFiveVariantRelation::TitleFormatting
                },
                game,
            });
        } else {
            visible.push(game);
        }
    }
    variants.sort_by(|left, right| {
        left.family_stable_key
            .cmp(&right.family_stable_key)
            .then_with(|| left.game.stable_key.cmp(&right.game.stable_key))
    });
    let collapsed = variants.len();
    system.games = visible;
    system.variants.extend(variants);
    collapsed
}

#[cfg(feature = "builder")]
fn compact_c64_family_title(title: &str) -> String {
    let mut output = String::new();
    let mut parenthesis_depth = 0usize;
    let mut bracket_depth = 0usize;
    for character in title.trim().chars().flat_map(char::to_lowercase) {
        match character {
            '(' => parenthesis_depth += 1,
            ')' => parenthesis_depth = parenthesis_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            value
                if parenthesis_depth == 0
                    && bracket_depth == 0
                    && value.is_ascii_alphanumeric() =>
            {
                output.push(value);
            }
            _ => {}
        }
    }
    output
}

#[cfg(feature = "builder")]
fn is_oneload64_game(game: &SystemGame) -> bool {
    let contains_marker = |value: &str| value.to_ascii_lowercase().contains("oneload64");
    contains_marker(&game.launch_ref)
        || game.launch_plan.as_ref().is_some_and(|plan| {
            contains_marker(&plan.launch_ref) || contains_marker(&plan.payload_path)
        })
}

#[cfg(feature = "builder")]
fn has_language_annotation(title: &str) -> bool {
    title
        .split(['(', '['])
        .skip(1)
        .filter_map(|part| part.split([')', ']']).next())
        .map(str::trim)
        .any(|annotation| {
            matches!(
                annotation.to_ascii_lowercase().as_str(),
                "de" | "ger"
                    | "german"
                    | "deutsch"
                    | "fr"
                    | "fre"
                    | "french"
                    | "es"
                    | "spa"
                    | "spanish"
                    | "it"
                    | "ita"
                    | "italian"
                    | "nl"
                    | "dut"
                    | "dutch"
            )
        })
}

#[cfg(feature = "builder")]
pub fn encode_snapshot(
    snapshot: &FastFiveSnapshot,
    encoding: FastFiveSnapshotEncoding,
) -> Result<Vec<u8>, String> {
    snapshot.validate()?;
    if encoding == FastFiveSnapshotEncoding::Json {
        return serde_json::to_vec(snapshot)
            .map_err(|error| format!("encode snapshot JSON: {error}"));
    }
    let payload = postcard::to_allocvec(snapshot)
        .map_err(|error| format!("encode snapshot postcard: {error}"))?;
    let stored = if encoding == FastFiveSnapshotEncoding::PostcardLz4 {
        lz4_flex::compress_prepend_size(&payload)
    } else {
        payload.clone()
    };
    let stored_len = u64::try_from(stored.len()).map_err(|_| "snapshot is too large")?;
    let payload_len = u64::try_from(payload.len()).map_err(|_| "snapshot is too large")?;
    let mut digest = Sha256::new();
    digest.update(&payload);
    let mut output = Vec::with_capacity(FAST_FIVE_BINARY_HEADER_BYTES + stored.len());
    output.extend_from_slice(FAST_FIVE_BINARY_MAGIC);
    output.extend_from_slice(&FAST_FIVE_BINARY_VERSION.to_le_bytes());
    output.extend_from_slice(
        &u32::from(encoding == FastFiveSnapshotEncoding::PostcardLz4).to_le_bytes(),
    );
    output.extend_from_slice(&stored_len.to_le_bytes());
    output.extend_from_slice(&payload_len.to_le_bytes());
    output.extend_from_slice(&digest.finalize());
    debug_assert_eq!(output.len(), FAST_FIVE_BINARY_HEADER_BYTES);
    output.extend_from_slice(&stored);
    Ok(output)
}

#[cfg(feature = "builder")]
pub fn decode_snapshot(
    bytes: &[u8],
    encoding: FastFiveSnapshotEncoding,
) -> Result<FastFiveSnapshot, String> {
    if encoding == FastFiveSnapshotEncoding::Json {
        let snapshot = serde_json::from_slice::<FastFiveSnapshot>(bytes)
            .map_err(|error| format!("decode snapshot JSON: {error}"))?;
        snapshot.validate()?;
        return Ok(snapshot);
    }
    let header = bytes
        .get(..FAST_FIVE_BINARY_HEADER_BYTES)
        .ok_or("fast-five binary snapshot header is truncated")?;
    if &header[..8] != FAST_FIVE_BINARY_MAGIC {
        return Err("fast-five binary snapshot magic is invalid".to_string());
    }
    if u32::from_le_bytes(header[8..12].try_into().expect("fixed snapshot version"))
        != FAST_FIVE_BINARY_VERSION
    {
        return Err("fast-five binary snapshot version is unsupported".to_string());
    }
    let compressed = u32::from_le_bytes(header[12..16].try_into().expect("fixed flags")) == 1;
    if compressed != (encoding == FastFiveSnapshotEncoding::PostcardLz4) {
        return Err("fast-five binary snapshot encoding does not match its header".to_string());
    }
    let stored_len = usize::try_from(u64::from_le_bytes(
        header[16..24].try_into().expect("fixed stored length"),
    ))
    .map_err(|_| "fast-five binary snapshot stored length is too large")?;
    let payload_len = usize::try_from(u64::from_le_bytes(
        header[24..32].try_into().expect("fixed payload length"),
    ))
    .map_err(|_| "fast-five binary snapshot payload length is too large")?;
    let stored = bytes
        .get(FAST_FIVE_BINARY_HEADER_BYTES..)
        .ok_or("fast-five binary snapshot payload is truncated")?;
    if stored.len() != stored_len {
        return Err("fast-five binary snapshot stored length is inconsistent".to_string());
    }
    let decoded;
    let payload = if compressed {
        decoded = lz4_flex::decompress_size_prepended(stored)
            .map_err(|error| format!("decompress snapshot postcard: {error}"))?;
        decoded.as_slice()
    } else {
        stored
    };
    if payload.len() != payload_len {
        return Err("fast-five binary snapshot payload length is inconsistent".to_string());
    }
    let mut digest = Sha256::new();
    digest.update(payload);
    let actual: [u8; 32] = digest.finalize().into();
    if actual != header[32..64] {
        return Err("fast-five binary snapshot checksum differs".to_string());
    }
    let snapshot = postcard::from_bytes::<FastFiveSnapshot>(payload)
        .map_err(|error| format!("decode snapshot postcard: {error}"))?;
    snapshot.validate()?;
    Ok(snapshot)
}

pub fn registry_fingerprint(storage_root: &Path, limits: RegistryLimits) -> Result<String, String> {
    let manifest = read_latest_manifest_lazy(storage_root, limits)
        .map_err(|error| format!("read fast-five manifest: {error}"))?;
    Ok(registry_fingerprint_for_manifest(&manifest))
}

pub fn registry_fingerprint_for_manifest(manifest: &CatalogManifest) -> String {
    let mut digest = Sha256::new();
    // The launcher fingerprint identifies immutable published artifacts, not
    // the interchange snapshot version used to build them. Keep this stable so
    // a newer prototype can be opened by the currently installed Dev UI.
    digest.update(FAST_FIVE_REGISTRY_FINGERPRINT_SCHEMA.as_bytes());
    digest.update(manifest.generation.to_le_bytes());
    for system in &manifest.systems {
        digest.update(system.system_id.as_str().as_bytes());
        digest.update([0]);
        digest.update(system.display_title.as_bytes());
        digest.update([0]);
        digest.update(system.active.generation.to_le_bytes());
        digest.update(system.active.games.to_le_bytes());
        digest.update(system.active.sqlite_hash.as_bytes());
        digest.update(system.active.navigation_hash.as_bytes());
        if let Some(navpack) = &system.active.navpack {
            digest.update(navpack.hash.as_bytes());
        }
    }
    hex(&digest.finalize())
}

#[cfg(feature = "builder")]
pub fn replace_arcade_from_active(
    mut snapshot: FastFiveSnapshot,
    active_bytes: &[u8],
) -> Result<FastFiveSnapshot, String> {
    let active = crate::arcade_catalog_prototype_model::decode_active(active_bytes)?;
    let previous_fingerprint = snapshot.source_fingerprint.clone();
    let arcade = snapshot
        .systems
        .iter_mut()
        .find(|system| system.system_id == "arcade")
        .ok_or_else(|| "fast-five snapshot has no Arcade system".to_string())?;
    let previous = arcade
        .games
        .iter()
        .map(|game| (game.launch_ref.as_str(), game))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut games = active
        .records
        .into_iter()
        .map(|record| {
            let old = previous.get(record.path.as_str()).copied();
            SystemGame {
                stable_key: format!("arcade\u{1f}{}\u{1f}{}", record.title, record.path),
                title: record.title,
                launch_ref: record.path,
                preview_archive_path: old
                    .map(|game| game.preview_archive_path.clone())
                    .unwrap_or_default(),
                preview_asset_key: old
                    .map(|game| game.preview_asset_key.clone())
                    .unwrap_or_default(),
                has_preview: old.is_some_and(|game| game.has_preview),
                year: record.year,
                manufacturer: record.manufacturer,
                category: record.category,
                players: record.players,
                control: record.control,
                is_new: old.is_some_and(|game| game.is_new),
                launch_plan: None,
            }
        })
        .collect::<Vec<_>>();
    games.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| left.stable_key.cmp(&right.stable_key))
    });
    arcade.games = games;
    let mut digest = Sha256::new();
    digest.update(previous_fingerprint.as_bytes());
    digest.update(active.source_sha256);
    for game in &arcade.games {
        digest.update(game.stable_key.as_bytes());
        digest.update([0]);
    }
    snapshot.source_fingerprint = hex(&digest.finalize());
    snapshot.validate()?;
    Ok(snapshot)
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug, Serialize)]
pub struct FastFivePublishReport {
    pub artifact_profile: FastFiveArtifactProfile,
    pub generation: u64,
    pub systems: usize,
    pub games: usize,
    pub variants: usize,
    pub elapsed_us: u64,
    pub registry_fingerprint: String,
    pub copied_bytes: u64,
    pub copy_hash_us: u64,
    pub system_builds: Vec<FastFiveSystemBuildReport>,
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug, Serialize)]
pub struct FastFiveSystemBuildReport {
    pub system_id: String,
    pub games: usize,
    pub variants: usize,
    pub sqlite_bytes: u64,
    pub navigation_bytes: u64,
    pub navpack_bytes: u64,
    pub elapsed_us: u64,
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug, Serialize)]
pub struct FastFiveVerificationReport {
    pub status: &'static str,
    pub systems: usize,
    pub games: usize,
    pub variants: usize,
    pub changed: usize,
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug, Serialize)]
pub struct FastFiveSearchProbeReport {
    pub queries: usize,
    pub elapsed_us: u64,
    pub fingerprint: String,
}

#[cfg(feature = "builder")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FastFiveArtifactProfile {
    #[default]
    Legacy,
    SinglePass,
    NoEmbeddedNavigation,
    NoAdjacentNavigation,
    NavpackOnly,
    SearchOnly,
    SearchColumn,
    SearchDetailNone,
}

#[cfg(feature = "builder")]
impl FastFiveArtifactProfile {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "legacy" => Ok(Self::Legacy),
            "single-pass" => Ok(Self::SinglePass),
            "no-embedded-navigation" => Ok(Self::NoEmbeddedNavigation),
            "no-adjacent-navigation" => Ok(Self::NoAdjacentNavigation),
            "navpack-only" => Ok(Self::NavpackOnly),
            "search-only" => Ok(Self::SearchOnly),
            "search-column" => Ok(Self::SearchColumn),
            "search-detail-none" => Ok(Self::SearchDetailNone),
            _ => Err(format!("unknown fast-five artifact profile {value}")),
        }
    }
}

#[cfg(feature = "builder")]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum C64ArtifactExperimentProfile {
    MediaImmediate,
    TmpfsImmediate,
    TmpfsDeferred,
    TmpfsDeferredMemory,
    TmpfsImmediateNoOptimize,
    TmpfsImmediateFtsColumn,
    TmpfsImmediateFtsColumnNoOptimize,
}

#[cfg(feature = "builder")]
impl C64ArtifactExperimentProfile {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "media-immediate" => Ok(Self::MediaImmediate),
            "tmpfs-immediate" => Ok(Self::TmpfsImmediate),
            "tmpfs-deferred" => Ok(Self::TmpfsDeferred),
            "tmpfs-deferred-memory" => Ok(Self::TmpfsDeferredMemory),
            "tmpfs-immediate-no-optimize" => Ok(Self::TmpfsImmediateNoOptimize),
            "tmpfs-immediate-fts-column" => Ok(Self::TmpfsImmediateFtsColumn),
            "tmpfs-immediate-fts-column-no-optimize" => Ok(Self::TmpfsImmediateFtsColumnNoOptimize),
            _ => Err(format!("unknown C64 artifact experiment profile {value}")),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MediaImmediate => "media-immediate",
            Self::TmpfsImmediate => "tmpfs-immediate",
            Self::TmpfsDeferred => "tmpfs-deferred",
            Self::TmpfsDeferredMemory => "tmpfs-deferred-memory",
            Self::TmpfsImmediateNoOptimize => "tmpfs-immediate-no-optimize",
            Self::TmpfsImmediateFtsColumn => "tmpfs-immediate-fts-column",
            Self::TmpfsImmediateFtsColumnNoOptimize => "tmpfs-immediate-fts-column-no-optimize",
        }
    }
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug, Serialize)]
pub struct C64ArtifactExperimentReport {
    pub status: &'static str,
    pub profile: C64ArtifactExperimentProfile,
    pub games: usize,
    pub variants: usize,
    pub build_us: u64,
    pub publish_us: u64,
    pub published_validate_us: u64,
    pub elapsed_us: u64,
    pub sqlite_bytes: u64,
    pub navigation_bytes: u64,
    pub navpack_bytes: u64,
    pub search_probe_us: u64,
    pub search_fingerprint: String,
}

#[cfg(feature = "builder")]
fn elapsed_us(started: std::time::Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

#[cfg(feature = "builder")]
fn c64_search_probe(sqlite_path: &Path, games: &[SystemGame]) -> Result<(u64, String), String> {
    use crate::persisted_search::search_system_shard;
    use std::time::Instant;

    let mut queries = BTreeSet::new();
    let stride = (games.len() / 8).max(1);
    for game in games.iter().step_by(stride).take(8) {
        let Some(word) = game
            .title
            .split(|character: char| !character.is_alphanumeric())
            .find(|word| word.chars().count() >= 3)
        else {
            continue;
        };
        let word = word.to_lowercase();
        queries.insert(word.clone());
        queries.insert(word.chars().take(3).collect::<String>());
    }
    let started = Instant::now();
    let mut fingerprint = Sha256::new();
    for query in queries {
        let result = search_system_shard(sqlite_path, &query)
            .map_err(|error| format!("probe C64 search {query}: {error}"))?;
        fingerprint.update((query.len() as u64).to_le_bytes());
        fingerprint.update(query.as_bytes());
        fingerprint.update((result.matches.len() as u64).to_le_bytes());
        for matched in result.matches {
            fingerprint.update((matched.ordinal as u64).to_le_bytes());
            fingerprint.update(matched.rank.to_bits().to_le_bytes());
        }
        if let Some(autocomplete) = result.autocomplete {
            fingerprint.update([1]);
            fingerprint.update((autocomplete.word.len() as u64).to_le_bytes());
            fingerprint.update(autocomplete.word.as_bytes());
            fingerprint.update([autocomplete.source_rank]);
            fingerprint.update(autocomplete.score.to_le_bytes());
        } else {
            fingerprint.update([0]);
        }
    }
    Ok((elapsed_us(started), hex(&fingerprint.finalize())))
}

#[cfg(feature = "builder")]
pub fn run_c64_artifact_experiment(
    storage_root: &Path,
    scratch_root: &Path,
    snapshot: &FastFiveSnapshot,
    profile: C64ArtifactExperimentProfile,
    limits: RegistryLimits,
) -> Result<C64ArtifactExperimentReport, String> {
    use crate::shard_registry::{
        publish_prevalidated_system_artifacts_deferred, publish_system_artifacts,
        sync_artifact_batch,
    };
    use crate::system_shard::{
        ShardDurability, ShardSearchTuning, ShardSqliteTuning, SystemShardData, open_system_shard,
        write_system_shard_with_options,
    };
    use std::fs;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    let _lease = crate::catalog_lease::CatalogMutationLease::acquire_default()
        .map_err(|error| error.to_string())?;
    let started = Instant::now();
    snapshot.validate()?;
    let source = snapshot
        .systems
        .iter()
        .find(|system| system.system_id == "c64")
        .ok_or("fast-five snapshot has no C64 system")?;
    let system_id = SystemId::parse("c64").map_err(|error| error.to_string())?;
    let (staging_parent, durability, sqlite_tuning, search_tuning) = match profile {
        C64ArtifactExperimentProfile::MediaImmediate => (
            storage_root.join("staging"),
            ShardDurability::Immediate,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::FullOptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsImmediate => (
            scratch_root.to_path_buf(),
            ShardDurability::Immediate,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::FullOptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsDeferred => (
            scratch_root.to_path_buf(),
            ShardDurability::Deferred,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::FullOptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsDeferredMemory => (
            scratch_root.to_path_buf(),
            ShardDurability::Deferred,
            ShardSqliteTuning::MemoryHeavy,
            ShardSearchTuning::FullOptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsImmediateNoOptimize => (
            scratch_root.to_path_buf(),
            ShardDurability::Immediate,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::FullUnoptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsImmediateFtsColumn => (
            scratch_root.to_path_buf(),
            ShardDurability::Immediate,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::ColumnOptimized,
        ),
        C64ArtifactExperimentProfile::TmpfsImmediateFtsColumnNoOptimize => (
            scratch_root.to_path_buf(),
            ShardDurability::Immediate,
            ShardSqliteTuning::Conservative,
            ShardSearchTuning::ColumnUnoptimized,
        ),
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock predates Unix epoch".to_string())?
        .as_nanos();
    let staging = staging_parent.join(format!(
        "c64-artifact-{}-{nonce}-{}",
        profile.as_str(),
        std::process::id()
    ));
    fs::create_dir_all(&staging)
        .map_err(|error| format!("create C64 experiment staging: {error}"))?;
    fs::create_dir_all(storage_root)
        .map_err(|error| format!("create C64 experiment root: {error}"))?;
    let sqlite = staging.join("system.sqlite3");
    let navigation = staging.join("system.nav.lz4b");
    let build_started = Instant::now();
    let staged = write_system_shard_with_options(
        &sqlite,
        &navigation,
        SystemShardData {
            system_id: system_id.clone(),
            generation: 1,
            projection_stats: (!source.variants.is_empty()).then_some(
                crate::system_shard::SystemShardProjectionStats {
                    source_games: source.games.len() + source.variants.len(),
                    visible_families: source.games.len(),
                    collapsed_variants: source.variants.len(),
                },
            ),
            games: source.games.clone(),
        },
        limits.shard,
        durability,
        sqlite_tuning,
        search_tuning,
    )
    .map_err(|error| format!("build C64 experiment shard: {error}"))?;
    write_fast_five_variants(&sqlite, source)?;
    let build_us = elapsed_us(build_started);
    if staged.games != source.games {
        return Err("staged C64 experiment rows differ from the snapshot".to_string());
    }
    let publish_started = Instant::now();
    let published = if profile == C64ArtifactExperimentProfile::MediaImmediate {
        publish_system_artifacts(
            storage_root,
            &sqlite,
            &navigation,
            &system_id,
            1,
            source.games.len() as u64,
            limits,
        )
        .map_err(|error| format!("publish C64 experiment shard: {error}"))?
    } else {
        let publication = publish_prevalidated_system_artifacts_deferred(
            storage_root,
            &sqlite,
            &navigation,
            &system_id,
            1,
            source.games.len() as u64,
            limits,
        )
        .map_err(|error| format!("copy C64 experiment shard: {error}"))?;
        sync_artifact_batch(storage_root)
            .map_err(|error| format!("sync C64 experiment shard: {error}"))?;
        publication.generation
    };
    let publish_us = elapsed_us(publish_started);
    let validate_started = Instant::now();
    let loaded = open_system_shard(
        &storage_root.join(&published.sqlite_path),
        &storage_root.join(&published.navigation_path),
        &system_id,
        1,
        limits.shard,
    )
    .map_err(|error| format!("validate published C64 experiment shard: {error}"))?;
    let published_validate_us = elapsed_us(validate_started);
    if loaded.games != source.games {
        return Err("published C64 experiment rows differ from the snapshot".to_string());
    }
    let (search_probe_us, search_fingerprint) =
        c64_search_probe(&storage_root.join(&published.sqlite_path), &source.games)?;
    let _ = fs::remove_dir_all(&staging);
    Ok(C64ArtifactExperimentReport {
        status: "exact",
        profile,
        games: source.games.len(),
        variants: source.variants.len(),
        build_us,
        publish_us,
        published_validate_us,
        elapsed_us: elapsed_us(started),
        sqlite_bytes: published.sqlite_bytes,
        navigation_bytes: published.navigation_bytes,
        navpack_bytes: published.navpack.map_or(0, |navpack| navpack.bytes),
        search_probe_us,
        search_fingerprint,
    })
}

#[cfg(feature = "builder")]
pub fn publish_snapshot(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
    limits: RegistryLimits,
) -> Result<FastFivePublishReport, String> {
    publish_snapshot_with_profile(
        storage_root,
        snapshot,
        limits,
        FastFiveArtifactProfile::Legacy,
    )
}

#[cfg(feature = "builder")]
fn write_fast_five_variants(sqlite_path: &Path, source: &FastFiveSystem) -> Result<(), String> {
    if source.variants.is_empty() {
        return Ok(());
    }
    let mut connection = rusqlite::Connection::open(sqlite_path)
        .map_err(|error| format!("open {} variant SQLite: {error}", source.system_id))?;
    connection
        .execute_batch(
            "CREATE TABLE fast_five_game_variants (
                 variant_stable_key TEXT PRIMARY KEY,
                 family_stable_key TEXT NOT NULL,
                 relation TEXT NOT NULL,
                 title TEXT NOT NULL,
                 launch_ref TEXT NOT NULL,
                 game_json TEXT NOT NULL
             ) WITHOUT ROWID;
             CREATE INDEX fast_five_game_variants_family
                 ON fast_five_game_variants(family_stable_key, title);",
        )
        .map_err(|error| format!("create {} variant schema: {error}", source.system_id))?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("begin {} variant transaction: {error}", source.system_id))?;
    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO fast_five_game_variants(
                     variant_stable_key,family_stable_key,relation,title,launch_ref,game_json
                 ) VALUES (?1,?2,?3,?4,?5,?6)",
            )
            .map_err(|error| format!("prepare {} variants: {error}", source.system_id))?;
        for variant in &source.variants {
            let game_json = serde_json::to_string(&variant.game)
                .map_err(|error| format!("encode {} variant: {error}", source.system_id))?;
            statement
                .execute(rusqlite::params![
                    variant.game.stable_key,
                    variant.family_stable_key,
                    variant.relation.as_str(),
                    variant.game.title,
                    variant.game.launch_ref,
                    game_json,
                ])
                .map_err(|error| format!("insert {} variant: {error}", source.system_id))?;
        }
    }
    transaction
        .execute(
            "INSERT INTO shard_meta(key,value) VALUES ('fast_five_variant_count',?1)",
            [source.variants.len().to_string()],
        )
        .map_err(|error| format!("record {} variant count: {error}", source.system_id))?;
    transaction
        .commit()
        .map_err(|error| format!("commit {} variants: {error}", source.system_id))
}

#[cfg(feature = "builder")]
pub fn publish_snapshot_with_profile(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
    limits: RegistryLimits,
    artifact_profile: FastFiveArtifactProfile,
) -> Result<FastFivePublishReport, String> {
    let lease = crate::catalog_lease::CatalogMutationLease::acquire_default()
        .map_err(|error| error.to_string())?;
    publish_snapshot_with_profile_held(storage_root, snapshot, limits, artifact_profile, &lease)
}

#[cfg(feature = "builder")]
pub(crate) fn publish_snapshot_with_profile_held(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
    limits: RegistryLimits,
    artifact_profile: FastFiveArtifactProfile,
    _lease: &crate::catalog_lease::CatalogMutationLease,
) -> Result<FastFivePublishReport, String> {
    publish_snapshot_selection(storage_root, snapshot, limits, artifact_profile, None)
}

#[cfg(feature = "builder")]
pub fn publish_changed_snapshot_with_profile(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
    changed_system_ids: &BTreeSet<String>,
    limits: RegistryLimits,
    artifact_profile: FastFiveArtifactProfile,
) -> Result<FastFivePublishReport, String> {
    let lease = crate::catalog_lease::CatalogMutationLease::acquire_default()
        .map_err(|error| error.to_string())?;
    publish_changed_snapshot_with_profile_held(
        storage_root,
        snapshot,
        changed_system_ids,
        limits,
        artifact_profile,
        &lease,
    )
}

#[cfg(feature = "builder")]
pub(crate) fn publish_changed_snapshot_with_profile_held(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
    changed_system_ids: &BTreeSet<String>,
    limits: RegistryLimits,
    artifact_profile: FastFiveArtifactProfile,
    _lease: &crate::catalog_lease::CatalogMutationLease,
) -> Result<FastFivePublishReport, String> {
    publish_snapshot_selection(
        storage_root,
        snapshot,
        limits,
        artifact_profile,
        Some(changed_system_ids),
    )
}

#[cfg(feature = "builder")]
fn publish_snapshot_selection(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
    limits: RegistryLimits,
    artifact_profile: FastFiveArtifactProfile,
    changed_system_ids: Option<&BTreeSet<String>>,
) -> Result<FastFivePublishReport, String> {
    use crate::catalog_domain::ScanUnitId;
    use crate::catalog_format::CatalogFormatDescriptor;
    use crate::shard_registry::{
        ManifestSystem, garbage_collect_unreferenced, manifest_slots_present,
        publish_manifest_with_trusted_artifacts, publish_prevalidated_system_artifacts_deferred,
        publish_system_artifacts, sync_artifact_batch,
    };
    use crate::system_shard::{
        ShardArtifactProfile, ShardDurability, SystemShardData, write_system_shard,
        write_system_shard_with_artifact_profile, write_system_shard_with_durability,
    };
    use std::fs;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    let started = Instant::now();
    snapshot.validate()?;
    #[cfg(target_os = "linux")]
    cleanup_fast_five_staging_root(Path::new("/tmp/mister-magik/fast-five-catalog"))?;
    fs::create_dir_all(storage_root)
        .map_err(|error| format!("create fast-five root {}: {error}", storage_root.display()))?;
    crate::shard_registry::cleanup_registry_temporary_files(storage_root)
        .map_err(|error| error.to_string())?;
    let current = match read_latest_manifest_lazy(storage_root, limits) {
        Ok(manifest) => Some(manifest),
        Err(_) if manifest_slots_present(storage_root) => {
            return Err("fast-five manifest slots exist but neither is valid".to_string());
        }
        Err(_) => None,
    };
    if changed_system_ids.is_some_and(BTreeSet::is_empty) {
        let current = current
            .as_ref()
            .ok_or_else(|| "selective publication requires an active catalog".to_string())?;
        return Ok(FastFivePublishReport {
            artifact_profile,
            generation: current.generation,
            systems: snapshot.systems.len(),
            games: snapshot.game_count(),
            variants: snapshot.variant_count(),
            elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
            registry_fingerprint: registry_fingerprint(storage_root, limits)?,
            copied_bytes: 0,
            copy_hash_us: 0,
            system_builds: snapshot
                .systems
                .iter()
                .map(|source| {
                    let published = current
                        .systems
                        .iter()
                        .find(|system| system.system_id.as_str() == source.system_id)
                        .ok_or_else(|| format!("active catalog is missing {}", source.system_id))?;
                    Ok(FastFiveSystemBuildReport {
                        system_id: source.system_id.clone(),
                        games: source.games.len(),
                        variants: source.variants.len(),
                        sqlite_bytes: published.active.sqlite_bytes,
                        navigation_bytes: published.active.navigation_bytes,
                        navpack_bytes: published
                            .active
                            .navpack
                            .as_ref()
                            .map_or(0, |navpack| navpack.bytes),
                        elapsed_us: 0,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        });
    }
    garbage_collect_unreferenced(
        storage_root,
        current.as_ref().unwrap_or(&CatalogManifest {
            format: None,
            generation: 0,
            systems: Vec::new(),
        }),
    )
    .map_err(|error| format!("collect interrupted fast-five artifacts: {error}"))?;
    let generation = current.as_ref().map_or(Ok(1), |manifest| {
        manifest
            .generation
            .checked_add(1)
            .ok_or_else(|| "fast-five generation overflow".to_string())
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock predates Unix epoch".to_string())?
        .as_nanos();
    let mut manifest_systems = Vec::with_capacity(snapshot.systems.len());
    let mut system_builds = Vec::with_capacity(snapshot.systems.len());
    let mut copied_bytes = 0u64;
    let mut copy_hash_us = 0u64;
    for source in &snapshot.systems {
        let system_started = Instant::now();
        let system_id = SystemId::parse(&source.system_id)
            .map_err(|error| format!("invalid system {}: {error}", source.system_id))?;
        if changed_system_ids.is_some_and(|changed| !changed.contains(&source.system_id)) {
            let published = current
                .as_ref()
                .and_then(|manifest| {
                    manifest
                        .systems
                        .iter()
                        .find(|system| system.system_id == system_id)
                })
                .cloned()
                .ok_or_else(|| format!("active catalog is missing {}", source.system_id))?;
            system_builds.push(FastFiveSystemBuildReport {
                system_id: source.system_id.clone(),
                games: source.games.len(),
                variants: source.variants.len(),
                sqlite_bytes: published.active.sqlite_bytes,
                navigation_bytes: published.active.navigation_bytes,
                navpack_bytes: published
                    .active
                    .navpack
                    .as_ref()
                    .map_or(0, |navpack| navpack.bytes),
                elapsed_us: 0,
            });
            manifest_systems.push(published);
            continue;
        }
        let stage_all_in_tmpfs = matches!(
            artifact_profile,
            FastFiveArtifactProfile::SinglePass
                | FastFiveArtifactProfile::SearchOnly
                | FastFiveArtifactProfile::SearchColumn
                | FastFiveArtifactProfile::SearchDetailNone
        );
        let stage_in_tmpfs =
            cfg!(target_os = "linux") && (stage_all_in_tmpfs || source.system_id == "c64");
        let staging_parent = if stage_in_tmpfs {
            std::path::PathBuf::from("/tmp/mister-magik/fast-five-catalog")
        } else {
            storage_root.join("staging")
        };
        let staging = staging_parent.join(format!(
            "fast-five-{}-{generation}-{nonce}-{}",
            std::process::id(),
            source.system_id
        ));
        fs::create_dir_all(&staging)
            .map_err(|error| format!("create {} staging: {error}", source.system_id))?;
        let mut staging_cleanup = FastFiveStagingCleanup::new(staging.clone());
        let sqlite = staging.join("system.sqlite3");
        let navigation = staging.join("system.nav.lz4b");
        let data = SystemShardData {
            system_id: system_id.clone(),
            generation,
            projection_stats: (!source.variants.is_empty()).then_some(
                crate::system_shard::SystemShardProjectionStats {
                    source_games: source.games.len() + source.variants.len(),
                    visible_families: source.games.len(),
                    collapsed_variants: source.variants.len(),
                },
            ),
            games: source.games.clone(),
        };
        let shard_profile = match artifact_profile {
            FastFiveArtifactProfile::Legacy | FastFiveArtifactProfile::SinglePass => {
                ShardArtifactProfile::Legacy
            }
            FastFiveArtifactProfile::NoEmbeddedNavigation => {
                ShardArtifactProfile::NoEmbeddedNavigation
            }
            FastFiveArtifactProfile::NoAdjacentNavigation => {
                ShardArtifactProfile::NoAdjacentNavigation
            }
            FastFiveArtifactProfile::NavpackOnly => ShardArtifactProfile::NavpackOnly,
            FastFiveArtifactProfile::SearchOnly => ShardArtifactProfile::SearchOnly,
            FastFiveArtifactProfile::SearchColumn => ShardArtifactProfile::SearchColumn,
            FastFiveArtifactProfile::SearchDetailNone => ShardArtifactProfile::SearchDetailNone,
        };
        if artifact_profile == FastFiveArtifactProfile::Legacy && stage_in_tmpfs {
            write_system_shard_with_durability(
                &sqlite,
                &navigation,
                data,
                limits.shard,
                ShardDurability::Immediate,
            )
        } else if artifact_profile == FastFiveArtifactProfile::Legacy {
            write_system_shard(&sqlite, &navigation, &data, limits.shard)
        } else {
            write_system_shard_with_artifact_profile(
                &sqlite,
                &navigation,
                data,
                limits.shard,
                ShardDurability::Immediate,
                shard_profile,
            )
        }
        .map_err(|error| format!("write {} shard: {error}", source.system_id))?;
        write_fast_five_variants(&sqlite, source)?;
        let active = if stage_in_tmpfs || artifact_profile != FastFiveArtifactProfile::Legacy {
            let publication = publish_prevalidated_system_artifacts_deferred(
                storage_root,
                &sqlite,
                &navigation,
                &system_id,
                generation,
                source.games.len() as u64,
                limits,
            )
            .map_err(|error| format!("publish {} shard: {error}", source.system_id))?;
            copied_bytes = copied_bytes.saturating_add(publication.copied_bytes);
            copy_hash_us = copy_hash_us.saturating_add(
                publication
                    .copy_hash_time
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
            publication.generation
        } else {
            publish_system_artifacts(
                storage_root,
                &sqlite,
                &navigation,
                &system_id,
                generation,
                source.games.len() as u64,
                limits,
            )
            .map_err(|error| format!("publish {} shard: {error}", source.system_id))?
        };
        staging_cleanup.remove()?;
        let previous = current.as_ref().and_then(|manifest| {
            manifest
                .systems
                .iter()
                .find(|system| system.system_id == system_id)
                .map(|system| system.active.clone())
        });
        let definition = system_definition(&source.system_id);
        let active_sqlite_bytes = active.sqlite_bytes;
        let active_navigation_bytes = active.navigation_bytes;
        let active_navpack_bytes = active.navpack.as_ref().map_or(0, |navpack| navpack.bytes);
        manifest_systems.push(ManifestSystem {
            system_id,
            display_title: source.display_title.clone(),
            section: definition
                .map(|value| section_label(value.section))
                .unwrap_or("Other")
                .to_string(),
            family: definition
                .map(|value| value.family.as_str())
                .filter(|value| !value.is_empty())
                .unwrap_or("Other")
                .to_string(),
            order: definition.map_or(1000, |value| u32::from(value.order)),
            producers: vec![
                ScanUnitId::parse(&format!("fast-five-{}", source.system_id))
                    .map_err(|error| format!("create producer id: {error}"))?,
            ],
            active,
            previous,
        });
        system_builds.push(FastFiveSystemBuildReport {
            system_id: source.system_id.clone(),
            games: source.games.len(),
            variants: source.variants.len(),
            sqlite_bytes: active_sqlite_bytes,
            navigation_bytes: active_navigation_bytes,
            navpack_bytes: active_navpack_bytes,
            elapsed_us: system_started
                .elapsed()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX),
        });
    }
    sync_artifact_batch(storage_root)
        .map_err(|error| format!("sync fast-five artifacts: {error}"))?;
    manifest_systems.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    let manifest = CatalogManifest {
        format: Some(CatalogFormatDescriptor::current()),
        generation,
        systems: manifest_systems,
    };
    publish_manifest_with_trusted_artifacts(storage_root, &manifest, limits)
        .map_err(|error| format!("publish fast-five manifest: {error}"))?;
    garbage_collect_unreferenced(storage_root, &manifest)
        .map_err(|error| format!("collect fast-five artifacts: {error}"))?;
    Ok(FastFivePublishReport {
        artifact_profile,
        generation,
        systems: snapshot.systems.len(),
        games: snapshot.game_count(),
        variants: snapshot.variant_count(),
        elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        registry_fingerprint: registry_fingerprint(storage_root, limits)?,
        copied_bytes,
        copy_hash_us,
        system_builds,
    })
}

#[cfg(feature = "builder")]
pub fn verify_snapshot_artifacts(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
    limits: RegistryLimits,
) -> Result<FastFiveVerificationReport, String> {
    use crate::navpack::MappedNavPack;
    use crate::system_shard::SystemLaunchPlan;
    use rusqlite::Connection;

    snapshot.validate()?;
    let manifest = read_latest_manifest_lazy(storage_root, limits)
        .map_err(|error| format!("read candidate manifest: {error}"))?;
    let mut changed = 0usize;
    let mut games = 0usize;
    let mut variants = 0usize;
    for source in &snapshot.systems {
        let system_id = SystemId::parse(&source.system_id).map_err(|error| error.to_string())?;
        let system = manifest
            .systems
            .iter()
            .find(|candidate| candidate.system_id == system_id)
            .ok_or_else(|| format!("candidate is missing {}", source.system_id))?;
        let navpack = system
            .active
            .navpack
            .as_ref()
            .ok_or_else(|| format!("candidate {} has no NavPack", source.system_id))?;
        let (mapped, _) = MappedNavPack::open(
            &storage_root.join(&navpack.path),
            navpack.bytes,
            &source.system_id,
            system.active.generation,
            source.games.len(),
        )?;
        let connection = Connection::open(storage_root.join(&system.active.sqlite_path))
            .map_err(|error| format!("open candidate {} SQLite: {error}", source.system_id))?;
        let has_full_games: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='games')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("inspect candidate {} SQLite: {error}", source.system_id))?;
        let identity_sql = if has_full_games {
            "SELECT stable_key FROM games ORDER BY ordinal"
        } else {
            "SELECT stable_key FROM game_identity ORDER BY ordinal"
        };
        let mut statement = connection.prepare(identity_sql).map_err(|error| {
            format!("prepare candidate {} identities: {error}", source.system_id)
        })?;
        let stable_keys = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("query candidate {} identities: {error}", source.system_id))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("read candidate {} identities: {error}", source.system_id))?;
        if stable_keys.len() != source.games.len() {
            return Err(format!(
                "candidate {} identity count differs",
                source.system_id
            ));
        }
        for (ordinal, expected) in source.games.iter().enumerate() {
            let row = mapped.row(ordinal)?;
            let metadata = mapped.metadata(ordinal)?;
            let launch_plan = row
                .launch_index
                .map(|index| {
                    let launch = mapped.launch(index)?;
                    Ok::<_, String>(SystemLaunchPlan {
                        launch_ref: launch.launch_ref.to_string(),
                        title: launch.title.to_string(),
                        system_id: launch.system_id.to_string(),
                        core_path: launch.core_path.to_string(),
                        payload_path: launch.payload_path.to_string(),
                        mount_kind: launch.mount_kind.to_string(),
                        mount_index: launch.mount_index,
                        delay_secs: launch.delay_secs,
                    })
                })
                .transpose()?;
            let actual = SystemGame {
                stable_key: stable_keys[ordinal].clone(),
                title: row.title.to_string(),
                launch_ref: row.launch_ref.to_string(),
                preview_archive_path: row.preview_archive_path.to_string(),
                preview_asset_key: row.preview_asset_key.to_string(),
                has_preview: row.has_preview,
                year: metadata.year,
                manufacturer: metadata.manufacturer.to_string(),
                category: metadata.category.to_string(),
                players: metadata.players,
                control: metadata.control.to_string(),
                is_new: row.is_new,
                launch_plan,
            };
            changed = changed.saturating_add(usize::from(&actual != expected));
        }
        let has_variants: bool = connection
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                     WHERE type='table' AND name='fast_five_game_variants'
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("inspect candidate {} variants: {error}", source.system_id))?;
        let stored_variants = if has_variants {
            let mut statement = connection
                .prepare(
                    "SELECT family_stable_key,relation,game_json
                     FROM fast_five_game_variants
                     ORDER BY family_stable_key,variant_stable_key",
                )
                .map_err(|error| {
                    format!("prepare candidate {} variants: {error}", source.system_id)
                })?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|error| format!("query candidate {} variants: {error}", source.system_id))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("read candidate {} variants: {error}", source.system_id))?
                .into_iter()
                .map(|(family_stable_key, relation, game_json)| {
                    let relation = match relation.as_str() {
                        "language-edition" => FastFiveVariantRelation::LanguageEdition,
                        "title-formatting" => FastFiveVariantRelation::TitleFormatting,
                        "arcade-variant" => FastFiveVariantRelation::ArcadeVariant,
                        _ => {
                            return Err(format!(
                                "candidate {} has unknown variant relation {relation}",
                                source.system_id
                            ));
                        }
                    };
                    let game = serde_json::from_str(&game_json).map_err(|error| {
                        format!("decode candidate {} variant: {error}", source.system_id)
                    })?;
                    Ok(FastFiveGameVariant {
                        family_stable_key,
                        relation,
                        game,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?
        } else {
            Vec::new()
        };
        changed = changed.saturating_add(source.variants.len().abs_diff(stored_variants.len()));
        changed = changed.saturating_add(
            source
                .variants
                .iter()
                .zip(&stored_variants)
                .filter(|(expected, actual)| expected != actual)
                .count(),
        );
        games = games.saturating_add(source.games.len());
        variants = variants.saturating_add(source.variants.len());
    }
    Ok(FastFiveVerificationReport {
        status: if changed == 0 { "exact" } else { "different" },
        systems: snapshot.systems.len(),
        games,
        variants,
        changed,
    })
}

#[cfg(feature = "builder")]
pub fn fast_five_search_probe(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
    limits: RegistryLimits,
) -> Result<FastFiveSearchProbeReport, String> {
    use std::time::Instant;

    let started = Instant::now();
    let manifest = read_latest_manifest_lazy(storage_root, limits)
        .map_err(|error| format!("read search candidate manifest: {error}"))?;
    let mut digest = Sha256::new();
    let mut query_count = 0usize;
    for source in &snapshot.systems {
        let system_id = SystemId::parse(&source.system_id).map_err(|error| error.to_string())?;
        let system = manifest
            .systems
            .iter()
            .find(|candidate| candidate.system_id == system_id)
            .ok_or_else(|| format!("search candidate is missing {}", source.system_id))?;
        let mut queries = source
            .games
            .iter()
            .step_by((source.games.len() / 4).max(1))
            .take(4)
            .filter_map(|game| {
                game.title
                    .split(|character: char| !character.is_alphanumeric())
                    .find(|word| word.chars().count() >= 3)
                    .map(str::to_lowercase)
            })
            .collect::<Vec<_>>();
        queries.sort();
        queries.dedup();
        for query in queries {
            let result = crate::persisted_search::search_system_shard(
                &storage_root.join(&system.active.sqlite_path),
                &query,
            )
            .map_err(|error| format!("search {} for {query}: {error}", source.system_id))?;
            digest.update(source.system_id.as_bytes());
            digest.update([0]);
            digest.update(query.as_bytes());
            digest.update([0]);
            for matched in result.matches {
                digest.update(matched.ordinal.to_le_bytes());
            }
            if let Some(autocomplete) = result.autocomplete {
                digest.update(autocomplete.word.as_bytes());
            }
            query_count = query_count.saturating_add(1);
        }
    }
    Ok(FastFiveSearchProbeReport {
        queries: query_count,
        elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        fingerprint: hex(&digest.finalize()),
    })
}

#[cfg(feature = "builder")]
pub fn snapshot_reference(
    storage_root: &Path,
    limits: RegistryLimits,
) -> Result<FastFiveSnapshot, String> {
    use crate::lazy_sharded_reader::LazyShardedCatalogReader;
    use crate::sharded_catalog::CatalogReader;
    use crate::system_shard::SystemLaunchPlan;

    let reader = LazyShardedCatalogReader::open(storage_root, limits)
        .map_err(|error| format!("open reference catalog: {error}"))?;
    let registry = reader
        .open_registry()
        .map_err(|error| format!("open reference registry: {error}"))?;
    let titles = registry
        .systems()
        .iter()
        .map(|summary| {
            (
                summary.system_id.as_str().to_string(),
                summary.display_title.clone(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut systems = Vec::with_capacity(FAST_FIVE_SYSTEM_IDS.len());
    for id in FAST_FIVE_SYSTEM_IDS {
        let system_id = SystemId::parse(id).map_err(|error| error.to_string())?;
        let catalog = reader
            .open_system(&system_id)
            .map_err(|error| format!("open reference {id}: {error}"))?;
        let games = catalog
            .games()
            .iter()
            .map(|game| SystemGame {
                stable_key: game.stable_key.clone(),
                title: game.title.clone(),
                launch_ref: game.launch_ref.clone(),
                preview_archive_path: game.preview_archive_path.clone(),
                preview_asset_key: game.preview_asset_key.clone(),
                has_preview: game.has_preview,
                year: game.year,
                manufacturer: game.manufacturer.clone(),
                category: game.category.clone(),
                players: game.players,
                control: game.control.clone(),
                is_new: game.is_new,
                launch_plan: game.launch_plan.as_ref().map(|plan| SystemLaunchPlan {
                    launch_ref: plan.launch_ref.clone(),
                    title: plan.title.clone(),
                    system_id: plan.system_id.clone(),
                    core_path: plan.core_path.clone(),
                    payload_path: plan.payload_path.clone(),
                    mount_kind: plan.mount_kind.clone(),
                    mount_index: plan.mount_index,
                    delay_secs: plan.delay_secs,
                }),
            })
            .collect();
        let mut system = FastFiveSystem {
            system_id: id.to_string(),
            display_title: titles.get(id).cloned().unwrap_or_else(|| id.to_string()),
            games,
            variants: Vec::new(),
        };
        if id == "c64" {
            collapse_c64_cross_source_variants(&mut system);
        }
        systems.push(system);
    }
    let snapshot = FastFiveSnapshot {
        schema: FAST_FIVE_SNAPSHOT_SCHEMA.to_string(),
        source_fingerprint: registry_fingerprint(storage_root, limits)?,
        systems,
    };
    snapshot.validate()?;
    Ok(snapshot)
}

#[cfg(feature = "builder")]
fn section_label(section: LauncherSection) -> &'static str {
    match section {
        LauncherSection::Arcade => "Arcade",
        LauncherSection::SnkNeogeo => "SNK Neo Geo",
        LauncherSection::Consoles => "Consoles",
        LauncherSection::Handhelds => "Handhelds",
        LauncherSection::Computers => "Computers",
        LauncherSection::Other => "Other",
    }
}

fn validate_fingerprint(value: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("fast-five source fingerprint is not SHA-256".to_string())
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_rejects_missing_system() {
        let snapshot = FastFiveSnapshot {
            schema: FAST_FIVE_SNAPSHOT_SCHEMA.to_string(),
            source_fingerprint: "0".repeat(64),
            systems: Vec::new(),
        };
        assert!(snapshot.validate().is_err());
    }
}

#[cfg(all(test, feature = "builder"))]
mod builder_tests {
    use super::*;
    use crate::lazy_sharded_reader::LazyShardedCatalogReader;
    use crate::sharded_catalog::CatalogReader;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn empty_snapshot() -> FastFiveSnapshot {
        FastFiveSnapshot {
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
        }
    }

    #[test]
    fn staging_cleanup_removes_only_the_scoped_directory() {
        let parent = std::env::temp_dir().join(format!(
            "mister-magik-fast-five-staging-cleanup-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let staging = parent.join("fast-five-catalog");
        fs::create_dir_all(staging.join("run-1")).unwrap();
        fs::write(staging.join("run-1/system.sqlite3"), b"partial").unwrap();
        fs::write(parent.join("keep"), b"unrelated").unwrap();

        cleanup_fast_five_staging_root(&staging).unwrap();

        assert!(!staging.exists());
        assert!(parent.join("keep").exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn public_publisher_fails_busy_before_mutating() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-fast-five-lease-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let lease = crate::catalog_lease::CatalogMutationLease::acquire_default().unwrap();

        let error = publish_snapshot_with_profile(
            &root,
            &empty_snapshot(),
            crate::shard_registry::production_registry_limits(),
            FastFiveArtifactProfile::SearchOnly,
        )
        .expect_err("publisher must respect held mutation lease");

        assert!(error.contains("busy"));
        assert!(!root.exists());
        drop(lease);
    }

    fn c64_game(title: &str, launch_ref: &str) -> SystemGame {
        SystemGame {
            stable_key: format!("c64\u{1f}{title}\u{1f}{launch_ref}"),
            title: title.to_string(),
            launch_ref: launch_ref.to_string(),
            ..SystemGame::default()
        }
    }

    #[test]
    fn c64_cross_source_formatting_and_language_editions_become_variants() {
        let mut system = FastFiveSystem {
            system_id: "c64".to_string(),
            display_title: "C64".to_string(),
            games: vec![
                c64_game(
                    "Night Racer",
                    "/media/fat/games/C64/OneLoad64-v5/Games/Night Racer.crt",
                ),
                c64_game(
                    "Nightracer (1990)(Markt & Technik)(de)",
                    "/media/fat/games/C64/German/Nightracer.d64",
                ),
                c64_game("Unique Game (de)", "/media/fat/games/C64/Unique.d64"),
            ],
            variants: Vec::new(),
        };
        assert_eq!(collapse_c64_cross_source_variants(&mut system), 1);
        assert_eq!(system.games.len(), 2);
        assert_eq!(system.variants.len(), 1);
        assert_eq!(
            system.variants[0].relation,
            FastFiveVariantRelation::LanguageEdition
        );
        assert_eq!(
            system.variants[0].game.title,
            "Nightracer (1990)(Markt & Technik)(de)"
        );
        assert_eq!(
            system.variants[0].family_stable_key,
            system.games[0].stable_key
        );
    }

    #[test]
    fn c64_variants_are_persisted_outside_the_navpack_rows() {
        let mut snapshot = empty_snapshot();
        let c64 = snapshot
            .systems
            .iter_mut()
            .find(|system| system.system_id == "c64")
            .unwrap();
        c64.games = vec![
            c64_game(
                "BoneCruncher",
                "/media/fat/games/C64/OneLoad64-v5/Games/BoneCruncher.crt",
            ),
            c64_game(
                "Bone Cruncher (1987)(Superior Software)",
                "/media/fat/games/C64/Bone Cruncher.d64",
            ),
        ];
        assert_eq!(collapse_c64_cross_source_variants(c64), 1);
        snapshot.validate().unwrap();
        let root = std::env::temp_dir().join(format!(
            "mister-magik-fast-five-variants-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let limits = crate::shard_registry::production_registry_limits();
        publish_snapshot(&root, &snapshot, limits).unwrap();
        let verified = verify_snapshot_artifacts(&root, &snapshot, limits).unwrap();
        assert_eq!(verified.status, "exact");
        assert_eq!(verified.games, 1);
        assert_eq!(verified.variants, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_snapshot_encodings_round_trip() {
        let snapshot = empty_snapshot();
        for encoding in [
            FastFiveSnapshotEncoding::Postcard,
            FastFiveSnapshotEncoding::PostcardLz4,
            FastFiveSnapshotEncoding::PostcardMmap,
        ] {
            let encoded = encode_snapshot(&snapshot, encoding).unwrap();
            assert_eq!(decode_snapshot(&encoded, encoding).unwrap(), snapshot);
        }
    }

    #[test]
    fn artifact_profiles_publish_exact_navpacks() {
        let snapshot = empty_snapshot();
        let limits = crate::shard_registry::production_registry_limits();
        for profile in [
            FastFiveArtifactProfile::NoEmbeddedNavigation,
            FastFiveArtifactProfile::NoAdjacentNavigation,
            FastFiveArtifactProfile::NavpackOnly,
            FastFiveArtifactProfile::SinglePass,
            FastFiveArtifactProfile::SearchOnly,
            FastFiveArtifactProfile::SearchColumn,
            FastFiveArtifactProfile::SearchDetailNone,
        ] {
            let root = std::env::temp_dir().join(format!(
                "mister-magik-fast-five-profile-{profile:?}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let report = publish_snapshot_with_profile(&root, &snapshot, limits, profile).unwrap();
            assert_eq!(report.artifact_profile, profile);
            assert_eq!(
                verify_snapshot_artifacts(&root, &snapshot, limits)
                    .unwrap()
                    .status,
                "exact"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn publishes_only_the_five_expected_systems() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-fast-five-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let snapshot = empty_snapshot();
        let limits = crate::shard_registry::production_registry_limits();
        let report = publish_snapshot(&root, &snapshot, limits).unwrap();
        assert_eq!(report.systems, 5);
        let reader = LazyShardedCatalogReader::open(&root, limits).unwrap();
        let registry = reader.open_registry().unwrap();
        assert_eq!(registry.systems().len(), 5);
        assert_eq!(registry_fingerprint(&root, limits).unwrap().len(), 64);
        fs::remove_dir_all(root).unwrap();
    }
}
