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
            "catalog_v3_system_tsv\tsystem={}\trole={role}\tgeneration={}\tregistry_games={}\tshard_games={}\n",
            summary.system_id, summary.generation, summary.games, games
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
