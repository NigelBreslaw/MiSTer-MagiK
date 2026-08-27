// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runtime-only screenshot-pack reconciliation for immutable catalog rows.

use crate::catalog_classify::SystemId;
use crate::shard_registry::{ManifestSystem, RegistryLimits, read_latest_manifest};
use crate::system_shard::{SystemGame, SystemLaunchPlan};
use std::collections::HashSet;
use std::path::Path;

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
