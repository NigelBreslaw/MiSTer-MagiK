// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runtime-only screenshot-pack reconciliation for immutable catalog rows.

use crate::catalog_classify::SystemId;
use crate::shard_registry::{ManifestSystem, RegistryLimits, read_latest_manifest};
use crate::system_shard::{SystemGame, SystemLaunchPlan};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// State of the optional MAME software-list metadata used for preview identity
/// enrichment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewIdentityResolverStatus {
    /// No software-list lookup has been needed for the current worker lifetime.
    NotNeeded,
    /// The compact v1 runtime metadata shard is available for lookups.
    CompactV1,
    /// The compact metadata file or required shard could not be read.
    Unavailable,
}

#[derive(Clone, Debug, Default)]
struct PreviewIdentityIndex {
    titles: BTreeMap<(String, String), BTreeSet<String>>,
}

/// Lazily loaded, read-only MAME software-list title index.
///
/// The resolver deliberately owns no catalog or pack state. It is safe to keep
/// one instance with the media worker and reuse it for every installed pack.
/// Metadata I/O occurs only when a reconciliation sees an empty catalog preview
/// key for a system backed by a MAME software list.
pub struct PreviewIdentityResolver {
    runtime_metadata: PathBuf,
    compact_store: Option<crate::runtime_metadata::MetadataStore>,
    state: PreviewIdentityResolverState,
}

enum PreviewIdentityResolverState {
    NotNeeded,
    Compact {
        system_id: String,
        index: PreviewIdentityIndex,
    },
    Unavailable,
}

impl std::fmt::Debug for PreviewIdentityResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreviewIdentityResolver")
            .field("runtime_metadata", &self.runtime_metadata)
            .field("status", &self.status())
            .finish()
    }
}

impl Default for PreviewIdentityResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewIdentityResolver {
    /// Create a resolver without opening the compact metadata file.
    pub fn new() -> Self {
        Self::with_runtime_metadata(crate::catalog_config::default_runtime_metadata_path())
    }

    /// Create a resolver with an explicit compact metadata path.  This is
    /// useful for focused checks and keeps path selection outside the hot lookup
    /// code.
    pub fn with_runtime_metadata(runtime_metadata: impl Into<PathBuf>) -> Self {
        Self {
            runtime_metadata: runtime_metadata.into(),
            compact_store: None,
            state: PreviewIdentityResolverState::NotNeeded,
        }
    }

    pub fn status(&self) -> PreviewIdentityResolverStatus {
        match self.state {
            PreviewIdentityResolverState::NotNeeded => PreviewIdentityResolverStatus::NotNeeded,
            PreviewIdentityResolverState::Compact { .. } => {
                PreviewIdentityResolverStatus::CompactV1
            }
            PreviewIdentityResolverState::Unavailable => PreviewIdentityResolverStatus::Unavailable,
        }
    }

    /// Return the unique asset-key candidates for a catalog title.
    ///
    /// `None` means the system has no MAME software-list mapping or the
    /// metadata could not be loaded. An empty vector is a successful lookup
    /// with no matching title.
    pub fn candidates(&mut self, system_id: &SystemId, title: &str) -> Option<Vec<String>> {
        let list_name = crate::software_identity::software_list_for_platform(system_id.as_str())?;
        self.ensure_loaded(system_id.as_str(), list_name);
        let index = match &self.state {
            PreviewIdentityResolverState::Compact { index, .. } => index,
            PreviewIdentityResolverState::NotNeeded | PreviewIdentityResolverState::Unavailable => {
                return None;
            }
        };
        let title_key = crate::library_db::canonical_variant_title(title);
        Some(
            index
                .titles
                .get(&(list_name.to_string(), title_key))
                .map(|keys| keys.iter().cloned().collect())
                .unwrap_or_default(),
        )
    }

    fn ensure_loaded(&mut self, system_id: &str, list_name: &str) {
        match &self.state {
            PreviewIdentityResolverState::Compact {
                system_id: loaded, ..
            } if loaded == system_id => return,
            PreviewIdentityResolverState::Unavailable => return,
            PreviewIdentityResolverState::NotNeeded
            | PreviewIdentityResolverState::Compact { .. } => {
                // A compact shard is cached for only one system. Clear it
                // before trying another system so a missing/corrupt shard
                // cannot accidentally query the previous system's index.
                self.state = PreviewIdentityResolverState::NotNeeded;
            }
        }
        if self.compact_store.is_none()
            && let Ok(store) = crate::runtime_metadata::MetadataStore::open(&self.runtime_metadata)
        {
            self.compact_store = Some(store);
        }
        if let Some(store) = self.compact_store.as_ref()
            && let Ok(Some(shard)) = store.software_shard(system_id)
        {
            let mut index = PreviewIdentityIndex::default();
            for (title, names) in shard.title_candidates.iter() {
                for software_name in names {
                    let Some(item) = shard.item(software_name) else {
                        continue;
                    };
                    let family_name = item
                        .parent_name
                        .as_deref()
                        .filter(|parent| !parent.trim().is_empty())
                        .unwrap_or(software_name);
                    let asset_key = crate::media_identity::ScreenshotAssetId::from_mame_software(
                        list_name,
                        family_name,
                    )
                    .into_string();
                    index
                        .titles
                        .entry((list_name.to_string(), title.clone()))
                        .or_default()
                        .insert(asset_key);
                }
            }
            self.state = PreviewIdentityResolverState::Compact {
                system_id: system_id.to_string(),
                index,
            };
            return;
        }
        self.state = PreviewIdentityResolverState::Unavailable;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewAvailabilityReconciliationOutcome {
    pub system_id: SystemId,
    pub previous_generation: u64,
    pub generation: u64,
    pub candidate_rows: usize,
    pub available_rows: usize,
    pub changed_rows: usize,
    pub existing_identity_rows: usize,
    pub derived_identity_rows: usize,
    pub ambiguous_identity_rows: usize,
    pub resolver_status: PreviewIdentityResolverStatus,
    pub games: Vec<SystemGame>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewAvailabilityError {
    detail: String,
}

impl PreviewAvailabilityError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for PreviewAvailabilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for PreviewAvailabilityError {}

/// Apply one installed screenshot pack to the launcher's in-memory rows.
///
/// Media availability never republishes the immutable catalog artifacts.
pub fn reconcile_preview_availability(
    storage_root: &Path,
    system_id: &SystemId,
    pack_path: &Path,
    limits: RegistryLimits,
) -> Result<PreviewAvailabilityReconciliationOutcome, PreviewAvailabilityError> {
    let mut resolver = PreviewIdentityResolver::new();
    reconcile_preview_availability_with_resolver(
        storage_root,
        system_id,
        pack_path,
        limits,
        &mut resolver,
    )
}

/// Apply an installed screenshot pack with a caller-owned identity resolver.
///
/// Keeping the resolver outside this function allows the media worker to load
/// MAME metadata once and reuse it for every system and pack reconciliation.
pub fn reconcile_preview_availability_with_resolver(
    storage_root: &Path,
    system_id: &SystemId,
    pack_path: &Path,
    limits: RegistryLimits,
    resolver: &mut PreviewIdentityResolver,
) -> Result<PreviewAvailabilityReconciliationOutcome, PreviewAvailabilityError> {
    let manifest = read_latest_manifest(storage_root, limits)
        .map_err(|error| PreviewAvailabilityError::new(error.to_string()))?;
    let published = manifest
        .systems
        .iter()
        .find(|system| &system.system_id == system_id)
        .ok_or_else(|| PreviewAvailabilityError::new("system is absent"))?;
    let games = open_navpack_games(storage_root, published)?;
    let reconciliation = reconcile_preview_rows(system_id, pack_path, games, resolver)?;
    Ok(PreviewAvailabilityReconciliationOutcome {
        system_id: system_id.clone(),
        previous_generation: manifest.generation,
        generation: manifest.generation,
        candidate_rows: reconciliation.candidate_rows,
        available_rows: reconciliation.available_rows,
        changed_rows: reconciliation.changed_rows,
        existing_identity_rows: reconciliation.existing_identity_rows,
        derived_identity_rows: reconciliation.derived_identity_rows,
        ambiguous_identity_rows: reconciliation.ambiguous_identity_rows,
        resolver_status: resolver.status(),
        games: reconciliation.games,
    })
}

fn open_navpack_games(
    storage_root: &Path,
    published: &ManifestSystem,
) -> Result<Vec<SystemGame>, PreviewAvailabilityError> {
    let generation = &published.active;
    let descriptor = generation
        .navpack
        .as_ref()
        .ok_or_else(|| PreviewAvailabilityError::new("active system has no NavPack"))?;
    let game_count = usize::try_from(generation.games)
        .map_err(|_| PreviewAvailabilityError::new("system game count exceeds platform size"))?;
    let (navpack, _) = crate::navpack::MappedNavPack::open(
        &storage_root.join(&descriptor.path),
        descriptor.bytes,
        published.system_id.as_str(),
        generation.generation,
        game_count,
    )
    .map_err(PreviewAvailabilityError::new)?;
    let mut games = Vec::with_capacity(game_count);
    for ordinal in 0..game_count {
        let row = navpack
            .row(ordinal)
            .map_err(PreviewAvailabilityError::new)?;
        let metadata = navpack
            .metadata(ordinal)
            .map_err(PreviewAvailabilityError::new)?;
        let launch_plan = row
            .launch_index
            .map(|index| {
                navpack.launch(index).map(|launch| SystemLaunchPlan {
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
            .transpose()
            .map_err(PreviewAvailabilityError::new)?;
        games.push(SystemGame {
            stable_key: format!(
                "{}\u{1f}{}\u{1f}{}",
                published.system_id,
                row.title.to_ascii_lowercase(),
                row.launch_ref
            ),
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
        });
    }
    Ok(games)
}

struct PreviewRowsReconciliation {
    games: Vec<SystemGame>,
    candidate_rows: usize,
    available_rows: usize,
    changed_rows: usize,
    existing_identity_rows: usize,
    derived_identity_rows: usize,
    ambiguous_identity_rows: usize,
}

fn reconcile_preview_rows(
    system_id: &SystemId,
    pack_path: &Path,
    mut games: Vec<SystemGame>,
    resolver: &mut PreviewIdentityResolver,
) -> Result<PreviewRowsReconciliation, PreviewAvailabilityError> {
    let stems = crate::preview_worker::preview_archive_sidecar_entry_stems(pack_path)
        .map_err(PreviewAvailabilityError::new)?
        .ok_or_else(|| PreviewAvailabilityError::new("pack index is missing"))?;
    let entries = stems.entries.into_iter().collect::<HashSet<_>>();
    let stable_archive_path =
        crate::preview_worker::preview_archive_path_for_system(system_id.as_str());
    let mut candidate_rows = 0;
    let mut available_rows = 0;
    let mut changed_rows = 0;
    let mut existing_identity_rows = 0;
    let mut derived_identity_rows = 0;
    let mut ambiguous_identity_rows = 0;
    for game in &mut games {
        if game.preview_asset_key.is_empty() {
            let Some(candidates) = resolver.candidates(system_id, &game.title) else {
                continue;
            };
            let candidates = candidates
                .into_iter()
                .filter(|candidate| entries.contains(&candidate.to_ascii_lowercase()))
                .collect::<Vec<_>>();
            if candidates.len() > 1 {
                ambiguous_identity_rows += 1;
                continue;
            }
            let Some(asset_key) = candidates.into_iter().next() else {
                continue;
            };
            game.preview_asset_key = asset_key;
            derived_identity_rows += 1;
        } else {
            existing_identity_rows += 1;
        }
        candidate_rows += 1;
        let available = entries.contains(&game.preview_asset_key.to_ascii_lowercase());
        available_rows += usize::from(available);
        let archive_path = if available {
            stable_archive_path.as_str()
        } else {
            ""
        };
        if game.has_preview != available || game.preview_archive_path != archive_path {
            game.has_preview = available;
            game.preview_archive_path = archive_path.to_string();
            changed_rows += 1;
        }
    }
    Ok(PreviewRowsReconciliation {
        games,
        candidate_rows,
        available_rows,
        changed_rows,
        existing_identity_rows,
        derived_identity_rows,
        ambiguous_identity_rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_temp_dir;

    fn game(title: &str, preview_asset_key: &str) -> SystemGame {
        SystemGame {
            title: title.to_string(),
            preview_asset_key: preview_asset_key.to_string(),
            ..SystemGame::default()
        }
    }

    fn write_runtime_metadata(
        root: &Path,
        system_id: &str,
        rows: &[(&str, Option<&str>, &str)],
    ) -> PathBuf {
        let mut title_candidates = BTreeMap::<String, Vec<String>>::new();
        let items = rows
            .iter()
            .map(|(name, parent, description)| {
                title_candidates
                    .entry(crate::library_db::canonical_variant_title(description))
                    .or_default()
                    .push((*name).to_string());
                crate::runtime_metadata::SoftwareItem {
                    name: (*name).to_string(),
                    parent_name: parent.map(str::to_string),
                    description: (*description).to_string(),
                    year: None,
                    publisher: None,
                    region: None,
                }
            })
            .collect();
        let metadata_path = root.join(crate::runtime_metadata::FILE_NAME);
        let mut builder = crate::runtime_metadata::MetadataFileBuilder::new();
        builder
            .add_software(
                system_id,
                &crate::runtime_metadata::SoftwareShard {
                    items,
                    title_candidates,
                    ..crate::runtime_metadata::SoftwareShard::default()
                },
            )
            .expect("add runtime metadata fixture");
        builder
            .write_to(&metadata_path)
            .expect("write runtime metadata fixture");
        metadata_path
    }

    fn write_pack_index(root: &Path, entry_stems: &[&str]) -> PathBuf {
        let archive = root.join("fixture.mmlz4b");
        std::fs::write(&archive, [0_u8; 2]).expect("write pack fixture");
        let mut index = Vec::new();
        index.extend_from_slice(b"MMIDX02\0");
        index.extend_from_slice(&2_u64.to_le_bytes());
        index
            .extend_from_slice(b"0000000000000000000000000000000000000000000000000000000000000000");
        index.extend_from_slice(&(entry_stems.len() as u32).to_le_bytes());
        for stem in entry_stems {
            let name = format!("{stem}.rgb565");
            index.extend_from_slice(&(name.len() as u16).to_le_bytes());
            index.extend_from_slice(&1_u32.to_le_bytes());
            index.extend_from_slice(&1_u32.to_le_bytes());
            index.extend_from_slice(&2_u32.to_le_bytes());
            index.extend_from_slice(&2_u32.to_le_bytes());
            index.push(1);
            index.extend_from_slice(&2_u32.to_le_bytes());
            index.extend_from_slice(&0_u64.to_le_bytes());
            index.extend_from_slice(name.as_bytes());
        }
        std::fs::write(
            crate::preview_worker::preview_archive_sidecar_path_for_archive(&archive),
            index,
        )
        .expect("write pack index fixture");
        archive
    }

    #[test]
    fn resolver_does_not_open_metadata_until_a_supported_lookup() {
        let root = unique_temp_dir("preview-identity-lazy");
        let resolver = PreviewIdentityResolver::new();
        assert_eq!(resolver.status(), PreviewIdentityResolverStatus::NotNeeded);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolver_uses_one_compact_system_shard_and_reports_compact_status() {
        let root = unique_temp_dir("preview-identity-compact");
        let metadata_path = root.join(crate::runtime_metadata::FILE_NAME);
        let shard = crate::runtime_metadata::SoftwareShard {
            items: vec![crate::runtime_metadata::SoftwareItem {
                name: "familygame".into(),
                parent_name: None,
                description: "Family Game".into(),
                year: None,
                publisher: None,
                region: None,
            }],
            title_candidates: BTreeMap::from([("family-game".into(), vec!["familygame".into()])]),
            ..crate::runtime_metadata::SoftwareShard::default()
        };
        let mut builder = crate::runtime_metadata::MetadataFileBuilder::new();
        builder
            .add_software("c64", &shard)
            .expect("add compact shard");
        builder
            .write_to(&metadata_path)
            .expect("write compact metadata");

        let system = SystemId::parse("c64").expect("valid system");
        let mut resolver = PreviewIdentityResolver::with_runtime_metadata(&metadata_path);
        let candidates = resolver
            .candidates(&system, "Family Game")
            .expect("candidates");
        assert_eq!(
            candidates,
            vec!["mame-software__c64__familygame".to_string()]
        );
        assert_eq!(resolver.status(), PreviewIdentityResolverStatus::CompactV1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolver_rejects_a_missing_compact_system_shard_without_sqlite_fallback() {
        let root = unique_temp_dir("preview-identity-compact-missing");
        let database = root.join("mame.sqlite3");
        std::fs::write(&database, b"legacy metadata must not be opened").unwrap();
        let metadata_path = root.join(crate::runtime_metadata::FILE_NAME);
        let shard = crate::runtime_metadata::SoftwareShard {
            items: vec![crate::runtime_metadata::SoftwareItem {
                name: "compactgame".into(),
                parent_name: None,
                description: "Compact Game".into(),
                year: None,
                publisher: None,
                region: None,
            }],
            title_candidates: BTreeMap::from([("compact-game".into(), vec!["compactgame".into()])]),
            ..crate::runtime_metadata::SoftwareShard::default()
        };
        let mut builder = crate::runtime_metadata::MetadataFileBuilder::new();
        builder
            .add_software("c64", &shard)
            .expect("add compact shard");
        builder
            .write_to(&metadata_path)
            .expect("write compact metadata");

        let mut resolver = PreviewIdentityResolver::with_runtime_metadata(&metadata_path);
        let c64 = SystemId::parse("c64").expect("valid C64 system");
        assert_eq!(
            resolver.candidates(&c64, "Compact Game"),
            Some(vec!["mame-software__c64__compactgame".to_string()])
        );
        assert_eq!(resolver.status(), PreviewIdentityResolverStatus::CompactV1);

        let nes = SystemId::parse("nes").expect("valid NES system");
        assert_eq!(resolver.candidates(&nes, "Legacy Game"), None);
        assert_eq!(
            resolver.status(),
            PreviewIdentityResolverStatus::Unavailable
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolver_reports_unavailable_database_without_failing_the_lookup() {
        let root = unique_temp_dir("preview-identity-unavailable");
        let system = SystemId::parse("nes").expect("valid system");
        let mut resolver = PreviewIdentityResolver::new();
        assert_eq!(resolver.candidates(&system, "Missing Game"), None);
        assert_eq!(
            resolver.status(),
            PreviewIdentityResolverStatus::Unavailable
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolver_collapses_media_lists_and_parent_variants() {
        let root = unique_temp_dir("preview-identity-canonical");
        let metadata = write_runtime_metadata(
            &root,
            "c64",
            &[
                ("cartgame", Some("familygame"), "Family Game (USA)"),
                ("cassgame", Some("familygame"), "Family Game (Europe)"),
            ],
        );
        let system = SystemId::parse("c64").expect("valid system");
        let mut resolver = PreviewIdentityResolver::with_runtime_metadata(metadata);
        assert_eq!(
            resolver.candidates(&system, "Family Game"),
            Some(vec!["mame-software__c64__familygame".to_string()])
        );
        assert_eq!(resolver.status(), PreviewIdentityResolverStatus::CompactV1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolver_preserves_distinct_families_as_ambiguous_candidates() {
        let root = unique_temp_dir("preview-identity-ambiguous");
        let metadata = write_runtime_metadata(
            &root,
            "c64",
            &[
                ("firstgame", None, "Shared Game"),
                ("secondgame", None, "Shared Game"),
            ],
        );
        let system = SystemId::parse("c64").expect("valid system");
        let mut resolver = PreviewIdentityResolver::with_runtime_metadata(metadata);
        assert_eq!(
            resolver.candidates(&system, "Shared Game"),
            Some(vec![
                "mame-software__c64__firstgame".to_string(),
                "mame-software__c64__secondgame".to_string(),
            ])
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reconciliation_preserves_existing_identity_without_loading_metadata() {
        let root = unique_temp_dir("preview-identity-existing");
        let archive = write_pack_index(&root, &["existing-key"]);
        let system = SystemId::parse("nes").expect("valid system");
        let mut resolver = PreviewIdentityResolver::new();

        let outcome = reconcile_preview_rows(
            &system,
            &archive,
            vec![game("Existing Game", "existing-key")],
            &mut resolver,
        )
        .expect("reconcile existing identity");

        assert_eq!(outcome.existing_identity_rows, 1);
        assert_eq!(outcome.derived_identity_rows, 0);
        assert_eq!(outcome.available_rows, 1);
        assert_eq!(outcome.games[0].preview_asset_key, "existing-key");
        assert!(outcome.games[0].has_preview);
        assert_eq!(resolver.status(), PreviewIdentityResolverStatus::NotNeeded);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reconciliation_derives_unique_pack_member_from_normalized_title() {
        let root = unique_temp_dir("preview-identity-derived");
        let metadata = write_runtime_metadata(&root, "nes", &[("metroid", None, "Metroid (USA)")]);
        let archive = write_pack_index(&root, &["mame-software__nes__metroid"]);
        let system = SystemId::parse("nes").expect("valid system");
        let mut resolver = PreviewIdentityResolver::with_runtime_metadata(metadata);

        let outcome = reconcile_preview_rows(
            &system,
            &archive,
            vec![game("Metroid [!]", "")],
            &mut resolver,
        )
        .expect("derive identity");

        assert_eq!(outcome.existing_identity_rows, 0);
        assert_eq!(outcome.derived_identity_rows, 1);
        assert_eq!(outcome.ambiguous_identity_rows, 0);
        assert_eq!(outcome.available_rows, 1);
        assert_eq!(
            outcome.games[0].preview_asset_key,
            "mame-software__nes__metroid"
        );
        assert!(outcome.games[0].has_preview);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reconciliation_uses_pack_membership_to_narrow_title_ambiguity() {
        let root = unique_temp_dir("preview-identity-pack-narrowing");
        let metadata = write_runtime_metadata(
            &root,
            "nes",
            &[("first", None, "Shared"), ("second", None, "Shared")],
        );
        let archive = write_pack_index(&root, &["mame-software__nes__second"]);
        let system = SystemId::parse("nes").expect("valid system");
        let mut resolver = PreviewIdentityResolver::with_runtime_metadata(metadata);

        let outcome =
            reconcile_preview_rows(&system, &archive, vec![game("Shared", "")], &mut resolver)
                .expect("narrow ambiguous title");

        assert_eq!(outcome.derived_identity_rows, 1);
        assert_eq!(outcome.ambiguous_identity_rows, 0);
        assert_eq!(
            outcome.games[0].preview_asset_key,
            "mame-software__nes__second"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reconciliation_leaves_multiple_pack_members_unresolved() {
        let root = unique_temp_dir("preview-identity-ambiguous-pack");
        let metadata = write_runtime_metadata(
            &root,
            "nes",
            &[("first", None, "Shared"), ("second", None, "Shared")],
        );
        let archive = write_pack_index(
            &root,
            &["mame-software__nes__first", "mame-software__nes__second"],
        );
        let system = SystemId::parse("nes").expect("valid system");
        let mut resolver = PreviewIdentityResolver::with_runtime_metadata(metadata);

        let outcome =
            reconcile_preview_rows(&system, &archive, vec![game("Shared", "")], &mut resolver)
                .expect("reject ambiguous title");

        assert_eq!(outcome.derived_identity_rows, 0);
        assert_eq!(outcome.ambiguous_identity_rows, 1);
        assert!(outcome.games[0].preview_asset_key.is_empty());
        assert!(!outcome.games[0].has_preview);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reconciliation_keeps_exact_matches_when_metadata_is_unavailable() {
        let root = unique_temp_dir("preview-identity-metadata-unavailable");
        let archive = write_pack_index(&root, &["existing-key"]);
        let system = SystemId::parse("nes").expect("valid system");
        let mut resolver = PreviewIdentityResolver::new();

        let outcome = reconcile_preview_rows(
            &system,
            &archive,
            vec![game("Existing", "existing-key"), game("Unknown", "")],
            &mut resolver,
        )
        .expect("reconcile without metadata");

        assert_eq!(outcome.existing_identity_rows, 1);
        assert_eq!(outcome.derived_identity_rows, 0);
        assert_eq!(outcome.available_rows, 1);
        assert!(outcome.games[0].has_preview);
        assert!(outcome.games[1].preview_asset_key.is_empty());
        assert_eq!(
            resolver.status(),
            PreviewIdentityResolverStatus::Unavailable
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reconciliation_does_not_load_metadata_for_unsupported_systems() {
        let root = unique_temp_dir("preview-identity-unsupported");
        let archive = write_pack_index(&root, &[]);
        let system = SystemId::parse("amiga").expect("valid system");
        let mut resolver = PreviewIdentityResolver::new();

        let outcome = reconcile_preview_rows(
            &system,
            &archive,
            vec![game("Unsupported", "")],
            &mut resolver,
        )
        .expect("ignore unsupported system");

        assert_eq!(outcome.candidate_rows, 0);
        assert_eq!(resolver.status(), PreviewIdentityResolverStatus::NotNeeded);
        let _ = std::fs::remove_dir_all(root);
    }
}
