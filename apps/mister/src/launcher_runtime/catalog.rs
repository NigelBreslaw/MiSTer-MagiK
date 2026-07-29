// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable Catalog V3 startup and system-row projection.

use crate::arcade_catalog::{self, ArcadeCatalog};
use mister_magik_catalog::sharded_catalog::{CatalogGame, CatalogReader};
use mister_magik_catalog::system_shard::SystemGame;
use std::path::{Path, PathBuf};

pub struct ShardedCatalogSeed {
    pub catalog: ArcadeCatalog,
    pub catalog_fingerprint: String,
    pub generation: u64,
}

#[derive(Debug)]
pub struct ShardedCatalogSeedLoadError {
    pub status: &'static str,
    error: String,
}

impl std::fmt::Display for ShardedCatalogSeedLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.error)
    }
}

impl std::error::Error for ShardedCatalogSeedLoadError {}

pub fn load_sharded_registry_seed(
    root: &str,
) -> Result<ShardedCatalogSeed, ShardedCatalogSeedLoadError> {
    load_sharded_registry_seed_at(
        root,
        &mister_magik_catalog::catalog_config::default_sharded_catalog_path(),
    )
}

pub fn load_sharded_registry_seed_at(
    root: &str,
    storage: &Path,
) -> Result<ShardedCatalogSeed, ShardedCatalogSeedLoadError> {
    let reader = mister_magik_catalog::lazy_sharded_reader::LazyShardedCatalogReader::open(
        storage,
        mister_magik_catalog::production_sharded_projection::production_registry_limits(),
    )
    .map_err(|error| ShardedCatalogSeedLoadError {
        status: "unavailable",
        error: error.to_string(),
    })?;
    let registry = reader
        .open_registry()
        .map_err(|error| ShardedCatalogSeedLoadError {
            status: "failed",
            error: error.to_string(),
        })?;
    if registry.systems().is_empty() {
        return Err(ShardedCatalogSeedLoadError {
            status: "empty",
            error: "catalog registry has no systems".to_owned(),
        });
    }
    let catalog_fingerprint =
        mister_magik_catalog::production_sharded_projection::validate_production_binding(
            storage,
            registry.generation(),
        )
        .map_err(|error| ShardedCatalogSeedLoadError {
            status: "stale",
            error: error.to_string(),
        })?;
    let systems = registry
        .systems()
        .iter()
        .map(|system| arcade_catalog::GameSystemEntry {
            id: system.system_id.as_str().to_owned(),
            title: system.display_title.clone(),
            count: usize::try_from(system.games).unwrap_or(usize::MAX),
        })
        .collect::<Vec<_>>();
    let generation = registry.generation();
    let arcade_id = mister_magik_catalog::catalog_classify::SystemId::parse(
        arcade_catalog::MENU_ARCADE_SYSTEM_ID.trim_start_matches("menu:"),
    )
    .ok();
    let (games, launch_plans) = arcade_id
        .as_ref()
        .and_then(|system_id| reader.open_system(system_id).ok())
        .map(|system| arcade_rows_from_shard("arcade", system.games()))
        .unwrap_or_default();
    let platform_kinds = systems
        .iter()
        .map(|system| {
            (
                system.id.clone(),
                mister_magik_catalog::catalog_classify::platform_kind_for_system(&system.id),
            )
        })
        .collect();
    Ok(ShardedCatalogSeed {
        catalog: ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
            PathBuf::from(root),
            games,
            systems,
            launch_plans,
            platform_kinds,
        ),
        catalog_fingerprint,
        generation,
    })
}

pub fn arcade_rows_from_shard(
    system_id: &str,
    games: &[CatalogGame],
) -> (
    Vec<arcade_catalog::ArcadeGameEntry>,
    Vec<arcade_catalog::StructuredLaunchPlan>,
) {
    project_rows(
        system_id,
        games.iter().map(|game| ProjectedGame {
            title: &game.title,
            launch_ref: &game.launch_ref,
            preview_archive_path: &game.preview_archive_path,
            preview_asset_key: &game.preview_asset_key,
            has_preview: game.has_preview,
            year: game.year,
            manufacturer: &game.manufacturer,
            players: game.players,
            control: &game.control,
            is_new: game.is_new,
            launch_plan: game.launch_plan.as_ref().map(|plan| ProjectedLaunchPlan {
                launch_ref: &plan.launch_ref,
                title: &plan.title,
                system_id: &plan.system_id,
                core_path: &plan.core_path,
                payload_path: &plan.payload_path,
                mount_kind: &plan.mount_kind,
                mount_index: plan.mount_index,
                delay_secs: plan.delay_secs,
            }),
        }),
    )
}

pub fn arcade_rows_from_persisted_shard(
    system_id: &str,
    games: &[SystemGame],
) -> (
    Vec<arcade_catalog::ArcadeGameEntry>,
    Vec<arcade_catalog::StructuredLaunchPlan>,
) {
    project_rows(
        system_id,
        games.iter().map(|game| ProjectedGame {
            title: &game.title,
            launch_ref: &game.launch_ref,
            preview_archive_path: &game.preview_archive_path,
            preview_asset_key: &game.preview_asset_key,
            has_preview: game.has_preview,
            year: game.year,
            manufacturer: &game.manufacturer,
            players: game.players,
            control: &game.control,
            is_new: game.is_new,
            launch_plan: game.launch_plan.as_ref().map(|plan| ProjectedLaunchPlan {
                launch_ref: &plan.launch_ref,
                title: &plan.title,
                system_id: &plan.system_id,
                core_path: &plan.core_path,
                payload_path: &plan.payload_path,
                mount_kind: &plan.mount_kind,
                mount_index: plan.mount_index,
                delay_secs: plan.delay_secs,
            }),
        }),
    )
}

struct ProjectedGame<'a> {
    title: &'a str,
    launch_ref: &'a str,
    preview_archive_path: &'a str,
    preview_asset_key: &'a str,
    has_preview: bool,
    year: Option<u16>,
    manufacturer: &'a str,
    players: Option<u8>,
    control: &'a str,
    is_new: bool,
    launch_plan: Option<ProjectedLaunchPlan<'a>>,
}

struct ProjectedLaunchPlan<'a> {
    launch_ref: &'a str,
    title: &'a str,
    system_id: &'a str,
    core_path: &'a str,
    payload_path: &'a str,
    mount_kind: &'a str,
    mount_index: u8,
    delay_secs: u8,
}

fn project_rows<'a>(
    system_id: &str,
    games: impl IntoIterator<Item = ProjectedGame<'a>>,
) -> (
    Vec<arcade_catalog::ArcadeGameEntry>,
    Vec<arcade_catalog::StructuredLaunchPlan>,
) {
    let mut launch_plans = Vec::new();
    let games = games
        .into_iter()
        .map(|game| {
            if let Some(plan) = game.launch_plan {
                launch_plans.push(arcade_catalog::StructuredLaunchPlan {
                    launch_ref: plan.launch_ref.into(),
                    title: plan.title.into(),
                    system_id: plan.system_id.into(),
                    core_path: plan.core_path.into(),
                    payload_path: plan.payload_path.into(),
                    mount_kind: plan.mount_kind.into(),
                    mount_index: plan.mount_index,
                    delay_secs: plan.delay_secs,
                });
            }
            arcade_catalog::ArcadeGameEntry {
                title: game.title.into(),
                mra_path: game.launch_ref.into(),
                preview_archive_path: game.preview_archive_path.into(),
                preview_asset_key: game.preview_asset_key.into(),
                has_preview: game.has_preview,
                system_id: system_id.into(),
                year: game.year,
                manufacturer: game.manufacturer.into(),
                players: game.players,
                control: game.control.into(),
                is_new: game.is_new,
            }
        })
        .collect();
    (games, launch_plans)
}
