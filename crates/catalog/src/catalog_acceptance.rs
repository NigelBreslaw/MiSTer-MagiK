// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only Catalog V3 integrity report used by host/device acceptance.

use crate::catalog_classify::{platform_kind_for_system, PlatformKind};
use crate::catalog_config;
use crate::sharded_catalog::CatalogReader;

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
    use std::collections::HashMap;
    use std::path::PathBuf;

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
            players: None,
            control: "".into(),
            is_new: false,
        }
    }
}
