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
    /// The MAME software-list title index is available for lookups.
    Ready,
    /// The database or required table could not be read.
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
/// Database I/O occurs only when a reconciliation sees an empty catalog preview
/// key for a system backed by a MAME software list.
pub struct PreviewIdentityResolver {
    mame_sqlite: PathBuf,
    state: PreviewIdentityResolverState,
}

enum PreviewIdentityResolverState {
    NotNeeded,
    Ready(PreviewIdentityIndex),
    Unavailable,
}

impl std::fmt::Debug for PreviewIdentityResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreviewIdentityResolver")
            .field("mame_sqlite", &self.mame_sqlite)
            .field("status", &self.status())
            .finish()
    }
}

impl PreviewIdentityResolver {
    /// Create a resolver without opening the database.
    pub fn new(mame_sqlite: impl Into<PathBuf>) -> Self {
        Self {
            mame_sqlite: mame_sqlite.into(),
            state: PreviewIdentityResolverState::NotNeeded,
        }
    }

    pub fn status(&self) -> PreviewIdentityResolverStatus {
        match self.state {
            PreviewIdentityResolverState::NotNeeded => PreviewIdentityResolverStatus::NotNeeded,
            PreviewIdentityResolverState::Ready(_) => PreviewIdentityResolverStatus::Ready,
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
        self.ensure_loaded();
        let PreviewIdentityResolverState::Ready(index) = &self.state else {
            return None;
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

    fn ensure_loaded(&mut self) {
        if !matches!(self.state, PreviewIdentityResolverState::NotNeeded) {
            return;
        }
        let Ok(connection) = crate::library_db::open_sqlite_read_only(&self.mame_sqlite) else {
            self.state = PreviewIdentityResolverState::Unavailable;
            return;
        };
        let Ok(true) = crate::library_db::sqlite_table_exists(&connection, "mame_software_items")
        else {
            self.state = PreviewIdentityResolverState::Unavailable;
            return;
        };
        let mut index = PreviewIdentityIndex::default();
        let Ok(mut statement) = connection.prepare(
            "SELECT list_name,software_name,parent_name,description FROM mame_software_items",
        ) else {
            self.state = PreviewIdentityResolverState::Unavailable;
            return;
        };
        let Ok(rows) = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        }) else {
            self.state = PreviewIdentityResolverState::Unavailable;
            return;
        };
        for row in rows.flatten() {
            let (raw_list, software_name, parent_name, description) = row;
            let list_name = crate::software_identity::canonical_software_list_name(&raw_list);
            let family_name = parent_name
                .as_deref()
                .filter(|parent| !parent.trim().is_empty())
                .unwrap_or(software_name.as_str());
            let asset_key = crate::media_identity::ScreenshotAssetId::from_mame_software(
                list_name,
                family_name,
            )
            .into_string();
            index
                .titles
                .entry((
                    list_name.to_string(),
                    crate::library_db::canonical_variant_title(&description),
                ))
                .or_default()
                .insert(asset_key);
        }
        self.state = PreviewIdentityResolverState::Ready(index);
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
    let manifest = read_latest_manifest(storage_root, limits)
        .map_err(|error| PreviewAvailabilityError::new(error.to_string()))?;
    let published = manifest
        .systems
        .iter()
        .find(|system| &system.system_id == system_id)
        .ok_or_else(|| PreviewAvailabilityError::new("system is absent"))?;
    let games = open_navpack_games(storage_root, published)?;
    let (games, candidate_rows, available_rows, changed_rows) =
        reconcile_preview_rows(system_id, pack_path, games)?;
    Ok(PreviewAvailabilityReconciliationOutcome {
        system_id: system_id.clone(),
        previous_generation: manifest.generation,
        generation: manifest.generation,
        candidate_rows,
        available_rows,
        changed_rows,
        games,
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

fn reconcile_preview_rows(
    system_id: &SystemId,
    pack_path: &Path,
    mut games: Vec<SystemGame>,
) -> Result<(Vec<SystemGame>, usize, usize, usize), PreviewAvailabilityError> {
    let stems = crate::preview_worker::preview_archive_sidecar_entry_stems(pack_path)
        .map_err(PreviewAvailabilityError::new)?
        .ok_or_else(|| PreviewAvailabilityError::new("pack index is missing"))?;
    let entries = stems.entries.into_iter().collect::<HashSet<_>>();
    let stable_archive_path =
        crate::preview_worker::preview_archive_path_for_system(system_id.as_str());
    let mut candidate_rows = 0;
    let mut available_rows = 0;
    let mut changed_rows = 0;
    for game in &mut games {
        if game.preview_asset_key.is_empty() {
            continue;
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
    Ok((games, candidate_rows, available_rows, changed_rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{unique_temp_dir, write_mame_software_fixture_db};

    #[test]
    fn resolver_does_not_open_metadata_until_a_supported_lookup() {
        let root = unique_temp_dir("preview-identity-lazy");
        let database = root.join("missing-mame.sqlite3");
        let resolver = PreviewIdentityResolver::new(&database);
        assert_eq!(resolver.status(), PreviewIdentityResolverStatus::NotNeeded);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolver_reports_unavailable_database_without_failing_the_lookup() {
        let root = unique_temp_dir("preview-identity-unavailable");
        let database = root.join("missing-mame.sqlite3");
        let system = SystemId::parse("nes").expect("valid system");
        let mut resolver = PreviewIdentityResolver::new(&database);
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
        let database = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &database,
            &[
                (
                    "c64_cart",
                    "cartgame",
                    Some("familygame"),
                    "Family Game (USA)",
                    None,
                    None,
                    None,
                ),
                (
                    "c64_cass",
                    "cassgame",
                    Some("familygame"),
                    "Family Game (Europe)",
                    None,
                    None,
                    None,
                ),
            ],
            &[],
        );
        let system = SystemId::parse("c64").expect("valid system");
        let mut resolver = PreviewIdentityResolver::new(&database);
        assert_eq!(
            resolver.candidates(&system, "Family Game"),
            Some(vec!["mame-software__c64__familygame".to_string()])
        );
        assert_eq!(resolver.status(), PreviewIdentityResolverStatus::Ready);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolver_preserves_distinct_families_as_ambiguous_candidates() {
        let root = unique_temp_dir("preview-identity-ambiguous");
        let database = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &database,
            &[
                (
                    "c64_cart",
                    "firstgame",
                    None,
                    "Shared Game",
                    None,
                    None,
                    None,
                ),
                (
                    "c64_cass",
                    "secondgame",
                    None,
                    "Shared Game",
                    None,
                    None,
                    None,
                ),
            ],
            &[],
        );
        let system = SystemId::parse("c64").expect("valid system");
        let mut resolver = PreviewIdentityResolver::new(&database);
        assert_eq!(
            resolver.candidates(&system, "Shared Game"),
            Some(vec![
                "mame-software__c64__firstgame".to_string(),
                "mame-software__c64__secondgame".to_string(),
            ])
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
