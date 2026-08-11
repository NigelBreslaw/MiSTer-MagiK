// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable Catalog V3 startup and system-row projection.

use crate::arcade_catalog::{self, ArcadeCatalog};
use mister_magik_catalog::sharded_catalog::{CatalogGame, CatalogReader};
use mister_magik_catalog::system_shard::SystemGame;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    let (games, launch_plans) = match arcade_id.as_ref() {
        Some(system_id)
            if registry
                .systems()
                .iter()
                .any(|system| &system.system_id == system_id) =>
        {
            let system =
                reader
                    .open_system(system_id)
                    .map_err(|error| ShardedCatalogSeedLoadError {
                        status: "failed",
                        error: format!("registered Arcade catalog cannot be opened: {error}"),
                    })?;
            arcade_rows_from_shard("arcade", system.games())
        }
        _ => (Vec::new(), Vec::new()),
    };
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
            category: &game.category,
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
            category: &game.category,
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

pub fn arcade_rows_from_owned_persisted_shard(
    system_id: &str,
    games: Vec<SystemGame>,
) -> (
    Vec<arcade_catalog::ArcadeGameEntry>,
    Vec<arcade_catalog::StructuredLaunchPlan>,
) {
    let shared_system_id = Arc::<str>::from(system_id);
    let mut strings = ArcStringInterner::default();
    strings.retain(shared_system_id.clone());
    let mut projected = Vec::with_capacity(games.len());
    let mut launch_plans = Vec::with_capacity(
        games
            .iter()
            .filter(|game| game.launch_plan.is_some())
            .count(),
    );
    for game in games {
        let title = Arc::<str>::from(game.title);
        let launch_ref = Arc::<str>::from(game.launch_ref);
        if let Some(plan) = game.launch_plan {
            let plan_title = strings.reuse_or_intern(plan.title, &title);
            let plan_launch_ref = strings.reuse_or_intern(plan.launch_ref, &launch_ref);
            let plan_system_id = strings.reuse_or_intern(plan.system_id, &shared_system_id);
            launch_plans.push(arcade_catalog::StructuredLaunchPlan {
                launch_ref: plan_launch_ref,
                title: plan_title,
                system_id: plan_system_id,
                core_path: Arc::from(plan.core_path),
                payload_path: Arc::from(plan.payload_path),
                mount_kind: strings.intern(plan.mount_kind),
                mount_index: plan.mount_index,
                delay_secs: plan.delay_secs,
            });
        }
        projected.push(arcade_catalog::ArcadeGameEntry {
            title,
            mra_path: launch_ref,
            preview_archive_path: strings.intern(game.preview_archive_path),
            preview_asset_key: Arc::from(game.preview_asset_key),
            has_preview: game.has_preview,
            system_id: shared_system_id.clone(),
            year: game.year,
            manufacturer: strings.intern(game.manufacturer),
            category: strings.intern(game.category),
            players: game.players,
            control: strings.intern(game.control),
            is_new: game.is_new,
        });
    }
    (projected, launch_plans)
}

#[derive(Default)]
struct ArcStringInterner {
    values: HashSet<Arc<str>>,
}

impl ArcStringInterner {
    fn retain(&mut self, value: Arc<str>) {
        self.values.insert(value);
    }

    fn intern(&mut self, value: String) -> Arc<str> {
        if let Some(shared) = self.values.get(value.as_str()) {
            return shared.clone();
        }
        let shared = Arc::<str>::from(value);
        self.values.insert(shared.clone());
        shared
    }

    fn reuse_or_intern(&mut self, value: String, preferred: &Arc<str>) -> Arc<str> {
        if value == preferred.as_ref() {
            preferred.clone()
        } else {
            self.intern(value)
        }
    }
}

struct ProjectedGame<'a> {
    title: &'a str,
    launch_ref: &'a str,
    preview_archive_path: &'a str,
    preview_asset_key: &'a str,
    has_preview: bool,
    year: Option<u16>,
    manufacturer: &'a str,
    category: &'a str,
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
                category: game.category.into(),
                players: game.players,
                control: game.control.into(),
                is_new: game.is_new,
            }
        })
        .collect();
    (games, launch_plans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_catalog::system_shard::SystemLaunchPlan;

    #[test]
    fn owned_projection_matches_borrowed_rows_and_structured_launch_plan() {
        let source = vec![
            SystemGame {
                stable_key: "snes:first".into(),
                title: "First Game".into(),
                launch_ref: "magik-plan:snes:first".into(),
                preview_archive_path: "/media/fat/preview/snes.zip".into(),
                preview_asset_key: "First Game".into(),
                has_preview: true,
                year: Some(1992),
                manufacturer: "Example".into(),
                category: "Platform".into(),
                players: Some(2),
                control: "joy".into(),
                is_new: true,
                launch_plan: Some(SystemLaunchPlan {
                    launch_ref: "magik-plan:snes:first".into(),
                    title: "First Game".into(),
                    system_id: "snes".into(),
                    core_path: "/media/fat/_Console/SNES_20200101.rbf".into(),
                    payload_path: "/media/fat/games/SNES/First Game.sfc".into(),
                    mount_kind: "rom".into(),
                    mount_index: 0,
                    delay_secs: 1,
                }),
            },
            SystemGame {
                stable_key: "snes:second".into(),
                title: "Second Game".into(),
                launch_ref: "/media/fat/games/SNES/Second Game.sfc".into(),
                preview_archive_path: "/media/fat/preview/snes.zip".into(),
                preview_asset_key: String::new(),
                has_preview: false,
                year: None,
                manufacturer: "Example".into(),
                category: "Platform".into(),
                players: None,
                control: "joy".into(),
                is_new: false,
                launch_plan: None,
            },
        ];
        let (expected_rows, expected_plans) = arcade_rows_from_persisted_shard("snes", &source);
        let (actual_rows, actual_plans) = arcade_rows_from_owned_persisted_shard("snes", source);

        assert_eq!(actual_plans, expected_plans);
        assert_eq!(actual_rows.len(), expected_rows.len());
        for (actual, expected) in actual_rows.iter().zip(&expected_rows) {
            assert_eq!(actual.title, expected.title);
            assert_eq!(actual.mra_path, expected.mra_path);
            assert_eq!(actual.preview_archive_path, expected.preview_archive_path);
            assert_eq!(actual.preview_asset_key, expected.preview_asset_key);
            assert_eq!(actual.has_preview, expected.has_preview);
            assert_eq!(actual.system_id, expected.system_id);
            assert_eq!(actual.year, expected.year);
            assert_eq!(actual.manufacturer, expected.manufacturer);
            assert_eq!(actual.category, expected.category);
            assert_eq!(actual.players, expected.players);
            assert_eq!(actual.control, expected.control);
            assert_eq!(actual.is_new, expected.is_new);
        }
    }

    #[test]
    fn owned_projection_interns_system_and_repeated_system_metadata() {
        let repeated = |stable_key: &str, title: &str| SystemGame {
            stable_key: stable_key.into(),
            title: title.into(),
            launch_ref: format!("/games/{title}.rom"),
            preview_archive_path: "/preview/system.zip".into(),
            manufacturer: "Example".into(),
            category: "Platform".into(),
            control: "joy".into(),
            ..SystemGame::default()
        };
        let (rows, _) = arcade_rows_from_owned_persisted_shard(
            "snes",
            vec![repeated("one", "One"), repeated("two", "Two")],
        );

        assert!(Arc::ptr_eq(&rows[0].system_id, &rows[1].system_id));
        assert!(Arc::ptr_eq(
            &rows[0].preview_archive_path,
            &rows[1].preview_archive_path
        ));
        assert!(Arc::ptr_eq(&rows[0].manufacturer, &rows[1].manufacturer));
        assert!(Arc::ptr_eq(&rows[0].category, &rows[1].category));
        assert!(Arc::ptr_eq(&rows[0].control, &rows[1].control));
    }
}
