// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only Catalog V3 integrity report used by host/device acceptance.

use crate::catalog_classify::{PlatformKind, platform_kind_for_system};
use crate::catalog_config;
use crate::sharded_catalog::CatalogReader;

pub fn inspect_production_registry() -> Result<String, String> {
    inspect_registry(&catalog_config::default_sharded_catalog_path())
}

pub fn inspect_registry(storage: &std::path::Path) -> Result<String, String> {
    let limits = crate::production_sharded_projection::production_registry_limits();
    let reader = crate::lazy_sharded_reader::LazyShardedCatalogReader::open(storage, limits)
        .map_err(|error| error.to_string())?;
    let registry = reader.open_registry().map_err(|error| error.to_string())?;
    let mut systems = registry.systems().iter().collect::<Vec<_>>();
    systems.sort_by(|a, b| a.system_id.as_str().cmp(b.system_id.as_str()));
    let total_games = systems.iter().try_fold(0_u64, |total, system| {
        total
            .checked_add(system.games)
            .ok_or_else(|| "V3 registry total game count overflow".to_string())
    })?;
    let mut output = String::new();
    for system in &systems {
        output.push_str(&format!(
            "catalog_v3_registry_system_tsv\tsystem={}\tgeneration={}\tgames={}\n",
            system.system_id, system.generation, system.games
        ));
    }
    output.push_str(&format!(
        "catalog_v3_registry_summary_tsv\tvalid=1\tsystems={}\ttotal_games={}\n",
        systems.len(),
        total_games
    ));
    Ok(output)
}

pub fn inspect_production_catalog() -> Result<String, String> {
    inspect_catalog(&catalog_config::default_sharded_catalog_path())
}

pub fn inspect_catalog(storage: &std::path::Path) -> Result<String, String> {
    let limits = crate::production_sharded_projection::production_registry_limits();
    // The eager reader verifies both immutable artifacts and their hashes for
    // every system. Launcher startup deliberately uses the lazy reader instead.
    let manifest = crate::shard_registry::read_latest_manifest(storage, limits)
        .map_err(|error| format!("validate V3 manifest: {error}"))?;
    let fingerprint = crate::production_sharded_projection::validate_production_binding(
        storage,
        manifest.generation,
    )
    .map_err(|error| format!("validate V3 state binding: {error}"))?;
    let state_path = crate::catalog_state::path_for_root(storage);
    let state = crate::catalog_state::read(&state_path)?;
    let scanner_path = state_path.with_file_name("scanner-cache.sqlite3");
    crate::scanner_cache::read(&scanner_path)?;

    let reader = crate::lazy_sharded_reader::LazyShardedCatalogReader::open(storage, limits)
        .map_err(|error| error.to_string())?;
    let registry = reader.open_registry().map_err(|error| error.to_string())?;
    let mut total_games = 0u64;
    let mut arcade_resident = 0u64;
    let mut role_arcade = 0usize;
    let mut role_console = 0usize;
    let mut role_computer = 0usize;
    let mut rows = String::new();
    for summary in registry.systems() {
        let system = reader
            .open_system(&summary.system_id)
            .map_err(|error| format!("open V3 system {}: {error}", summary.system_id))?;
        let games = u64::try_from(system.games().len())
            .map_err(|_| "system game count exceeds u64".to_string())?;
        let manifest_system = manifest
            .systems
            .iter()
            .find(|system| system.system_id == summary.system_id)
            .ok_or_else(|| format!("missing manifest system {}", summary.system_id))?;
        let full_shard = crate::system_shard::open_system_shard(
            &storage.join(&manifest_system.active.sqlite_path),
            &storage.join(&manifest_system.active.navigation_path),
            &summary.system_id,
            summary.generation,
            limits.shard,
        )
        .map_err(|error| format!("open V3 projection metadata {}: {error}", summary.system_id))?;
        let projection_stats = full_shard.projection_stats.unwrap_or(
            crate::system_shard::SystemShardProjectionStats {
                source_games: system.games().len(),
                visible_families: system.games().len(),
                collapsed_variants: 0,
            },
        );
        validate_visible_system_rows(&summary.system_id, &full_shard.games)?;
        let preview_keys = system
            .games()
            .iter()
            .filter(|game| !game.preview_asset_key.is_empty())
            .count();
        let available_previews = system
            .games()
            .iter()
            .filter(|game| game.has_preview)
            .count();
        if games != summary.games {
            return Err(format!(
                "V3 system {} registry/shard mismatch: {} != {}",
                summary.system_id, summary.games, games
            ));
        }
        total_games = total_games
            .checked_add(games)
            .ok_or_else(|| "V3 total game count overflow".to_string())?;
        let role = if summary.system_id.as_str() == "arcade" {
            arcade_resident = games;
            role_arcade += 1;
            "arcade"
        } else {
            match platform_kind_for_system(summary.system_id.as_str()) {
                PlatformKind::Console | PlatformKind::Handheld => {
                    role_console += 1;
                    "console"
                }
                PlatformKind::Computer => {
                    role_computer += 1;
                    "computer"
                }
                _ => "other",
            }
        };
        rows.push_str(&format!(
            "catalog_v3_system_tsv\tsystem={}\trole={role}\tgeneration={}\tregistry_games={}\tshard_games={}\tpreview_keys={preview_keys}\tavailable_previews={available_previews}\tsource_games={}\tvisible_families={}\tcollapsed_variants={}\n",
            summary.system_id, summary.generation, summary.games, games,
            projection_stats.source_games,
            projection_stats.visible_families,
            projection_stats.collapsed_variants,
        ));
    }
    let manifest_total = manifest.systems.iter().try_fold(0u64, |total, system| {
        total
            .checked_add(system.active.games)
            .ok_or_else(|| "V3 manifest game count overflow".to_string())
    })?;
    if total_games != manifest_total {
        return Err(format!(
            "V3 manifest/registry total mismatch: {manifest_total} != {total_games}"
        ));
    }
    let mut output = format!(
        "catalog_v3_summary_tsv\tvalid=1\tschema=1\tgeneration={}\tsystems={}\ttotal_games={}\tarcade_resident_games={}\tstate_discoveries={}\tarcade_roles={}\tconsole_roles={}\tcomputer_roles={}\tfingerprint={}\n",
        manifest.generation,
        manifest.systems.len(),
        total_games,
        arcade_resident,
        state.stats.discoveries,
        role_arcade,
        role_console,
        role_computer,
        fingerprint,
    );
    output.push_str(&rows);
    Ok(output)
}

fn validate_visible_system_rows(
    system_id: &crate::catalog_classify::SystemId,
    games: &[crate::system_shard::SystemGame],
) -> Result<(), String> {
    for game in games {
        let Some(plan) = game.launch_plan.as_ref() else {
            continue;
        };
        if plan.system_id != system_id.as_str() {
            return Err(format!(
                "V3 system {system_id} launch plan has system {}",
                plan.system_id
            ));
        }
        crate::launch_profiles::validate_canonical_core_profile(&plan.system_id, &plan.core_path)
            .map_err(|error| format!("V3 launch plan {}: {error}", plan.launch_ref))?;
        if let Some(member) = crate::archive_member::decode_archive_member_ref(&plan.payload_path)?
        {
            if !std::path::Path::new(&member.archive_path).is_file() {
                return Err(format!(
                    "V3 launch plan {} archive is unreadable: {}",
                    plan.launch_ref, member.archive_path
                ));
            }
        } else if !std::path::Path::new(&plan.payload_path).is_file() {
            return Err(format!(
                "V3 launch plan {} payload is unreadable: {}",
                plan.launch_ref, plan.payload_path
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arcade_catalog::{
        ArcadeCatalog, ArcadeGameEntry, GameSystemEntry, SystemProjectionStats,
    };
    use crate::catalog_checkpoint::CatalogDiscoveryCheckpoint;
    use crate::catalog_stamp::CatalogStamp;
    use crate::catalog_state::{CatalogState, CatalogStateStats};
    use crate::scanner_cache::ScannerCacheState;
    use crate::test_support::unique_temp_dir;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn visible_row_validation_accepts_shared_cores_and_rejects_undeclared_cores() {
        let system_id = crate::catalog_classify::SystemId::parse("atari2600").expect("system");
        let root = unique_temp_dir("atari2600-shared-core-acceptance");
        let payload = root.join("Acid Drop (Europe).a26");
        std::fs::write(&payload, b"rom").expect("write readable payload");
        let game = crate::system_shard::SystemGame {
            stable_key: "acid-drop".to_string(),
            title: "Acid Drop (Europe)".to_string(),
            launch_ref: "acid-drop".to_string(),
            launch_plan: Some(crate::system_shard::SystemLaunchPlan {
                launch_ref: "acid-drop".to_string(),
                title: "Acid Drop".to_string(),
                system_id: "atari2600".to_string(),
                core_path: "_Console/Atari7800".to_string(),
                payload_path: payload.display().to_string(),
                mount_kind: "load-file".to_string(),
                mount_index: 1,
                delay_secs: 1,
            }),
            ..Default::default()
        };
        validate_visible_system_rows(&system_id, std::slice::from_ref(&game))
            .expect("Atari 7800 is a compatible Atari 2600 core");

        let mut unreadable = game.clone();
        unreadable.launch_plan.as_mut().expect("plan").payload_path =
            root.join("missing.a26").display().to_string();
        let error = validate_visible_system_rows(&system_id, &[unreadable])
            .expect_err("unreadable payload");
        assert!(error.contains("payload is unreadable"));

        let mut incompatible = game;
        incompatible.launch_plan.as_mut().expect("plan").core_path =
            "_Console/Atari5200".to_string();
        let error = validate_visible_system_rows(&system_id, &[incompatible])
            .expect_err("undeclared shared core");
        assert!(error.contains("compatible cores [Atari2600, Atari7800]"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn visible_row_validation_allows_same_title_for_distinct_arcade_families() {
        let system_id = crate::catalog_classify::SystemId::parse("arcade").expect("system");
        let deco = crate::system_shard::SystemGame {
            stable_key: "arcade-deco-burger-time".to_string(),
            title: "Burger Time".to_string(),
            launch_ref: "/media/fat/_Arcade/Burger Time (DECO).mra".to_string(),
            preview_asset_key: "cbtime".to_string(),
            ..Default::default()
        };
        let data_east = crate::system_shard::SystemGame {
            stable_key: "arcade-data-east-burger-time".to_string(),
            title: "Burger Time".to_string(),
            launch_ref: "/media/fat/_Arcade/Burger Time (Set 1).mra".to_string(),
            preview_asset_key: "btime".to_string(),
            ..Default::default()
        };

        validate_visible_system_rows(&system_id, &[deco, data_east]).unwrap();
    }

    #[test]
    fn inspector_reports_lynx_keyed_available_and_unmatched_coverage() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-lynx-inspection-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let state = CatalogState {
            stamp: CatalogStamp::from_lines(vec!["lynx-inspection".to_string()]),
            checkpoint: CatalogDiscoveryCheckpoint::from_lines(vec!["lynx-inspection".to_string()]),
            stats: CatalogStateStats {
                discoveries: 5,
                ..Default::default()
            },
        };
        let fingerprint = state.stamp.fingerprint_hex();
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/fixture"),
            vec![
                lynx_game("Available", "mame-software__lynx__available", true),
                lynx_game("Unavailable", "mame-software__lynx__unavailable", false),
                lynx_game("Unmatched", "", false),
            ],
            vec![GameSystemEntry {
                id: "atarilynx".to_string(),
                title: "Atari Lynx".to_string(),
                count: 3,
            }],
        )
        .with_projection_stats(HashMap::from([(
            "atarilynx".to_string(),
            SystemProjectionStats {
                source_games: 5,
                visible_families: 3,
                collapsed_variants: 2,
            },
        )]));
        let outcome = crate::production_sharded_projection::publish_bound_production_projection(
            &root,
            &catalog,
            &fingerprint,
            crate::production_sharded_projection::production_registry_limits(),
        )
        .expect("publish Lynx catalog");
        crate::catalog_state::write(&crate::catalog_state::path_for_root(&root), &state)
            .expect("write catalog state");
        let scanner_path =
            crate::catalog_state::path_for_root(&root).with_file_name("scanner-cache.sqlite3");
        crate::scanner_cache::stage(&scanner_path, &ScannerCacheState::default())
            .and_then(|staged| staged.publish())
            .expect("publish scanner cache");

        let report = inspect_catalog(&root).expect("inspect Lynx catalog");

        assert!(report.contains(&format!(
            "catalog_v3_system_tsv\tsystem=atarilynx\trole=console\tgeneration={}\tregistry_games=3\tshard_games=3\tpreview_keys=2\tavailable_previews=1\tsource_games=5\tvisible_families=3\tcollapsed_variants=2",
            outcome.generation
        )));
        std::fs::remove_dir_all(root.join("systems")).expect("remove system artifacts");
        let registry_report = inspect_registry(&root).expect("inspect registry without shards");
        assert!(registry_report.contains(&format!(
            "catalog_v3_registry_system_tsv\tsystem=atarilynx\tgeneration={}\tgames=3",
            outcome.generation
        )));
        assert!(
            registry_report
                .contains("catalog_v3_registry_summary_tsv\tvalid=1\tsystems=1\ttotal_games=3")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn lynx_game(title: &str, preview_asset_key: &str, has_preview: bool) -> ArcadeGameEntry {
        let preview_archive_path = if preview_asset_key.is_empty() {
            ""
        } else {
            "/assets/atarilynx-screenshots-160x102.mmlz4b"
        };
        ArcadeGameEntry {
            title: title.into(),
            mra_path: format!("/games/AtariLynx/{title}.lyx").into(),
            preview_archive_path: preview_archive_path.into(),
            preview_asset_key: preview_asset_key.into(),
            has_preview,
            system_id: "atarilynx".into(),
            year: None,
            manufacturer: "".into(),
            category: "".into(),
            players: None,
            control: "".into(),
            is_new: false,
        }
    }
}
