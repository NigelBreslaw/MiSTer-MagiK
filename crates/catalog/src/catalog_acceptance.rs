// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only integrity reports for the installed catalog.

use std::fmt::Write as _;
use std::path::Path;

pub fn inspect_production_registry() -> Result<String, String> {
    inspect_registry(&crate::catalog_config::default_sharded_catalog_path())
}

pub fn inspect_registry(storage: &Path) -> Result<String, String> {
    let reader = crate::lazy_sharded_reader::LazyShardedCatalogReader::open(
        storage,
        crate::shard_registry::production_registry_limits(),
    )
    .map_err(|error| error.to_string())?;
    let registry = reader.open_registry().map_err(|error| error.to_string())?;
    let mut systems = registry.systems().iter().collect::<Vec<_>>();
    systems.sort_by(|left, right| left.system_id.as_str().cmp(right.system_id.as_str()));
    let total_games = systems.iter().try_fold(0_u64, |total, system| {
        total
            .checked_add(system.games)
            .ok_or_else(|| "catalog game count overflow".to_string())
    })?;
    let mut output = String::new();
    for system in &systems {
        writeln!(
            output,
            "catalog_registry_system_tsv\tsystem={}\tgeneration={}\tgames={}",
            system.system_id, system.generation, system.games
        )
        .expect("write to String");
    }
    writeln!(
        output,
        "catalog_registry_summary_tsv\tvalid=1\tsystems={}\ttotal_games={}",
        systems.len(),
        total_games
    )
    .expect("write to String");
    Ok(output)
}

pub fn inspect_production_catalog() -> Result<String, String> {
    inspect_catalog(&crate::catalog_config::default_sharded_catalog_path())
}

pub fn inspect_catalog(storage: &Path) -> Result<String, String> {
    let limits = crate::shard_registry::production_registry_limits();
    let manifest = crate::shard_registry::read_latest_manifest(storage, limits)
        .map_err(|error| format!("validate catalog manifest: {error}"))?;
    let fingerprint = crate::fast_five_catalog::registry_fingerprint(storage, limits)?;
    let refresh = crate::fast_catalog_refresh::read_latest_refresh_manifest(storage)?;
    if refresh.catalog_generation != manifest.generation
        || refresh.catalog_fingerprint != fingerprint
    {
        return Err("catalog source snapshot is not bound to the active manifest".to_string());
    }
    let mut total_games = 0_u64;
    let mut output = String::new();
    for system in &manifest.systems {
        let navpack = system.active.navpack.as_ref().ok_or_else(|| {
            format!(
                "open system {}: active generation has no NavPack",
                system.system_id
            )
        })?;
        let (mapped, _) = crate::navpack::MappedNavPack::open(
            &storage.join(&navpack.path),
            navpack.bytes,
            system.system_id.as_str(),
            system.active.generation,
            usize::try_from(system.active.games)
                .map_err(|_| "system game count exceeds platform size".to_string())?,
        )
        .map_err(|error| format!("open system {}: {error}", system.system_id))?;
        for ordinal in 0..mapped.identity().games {
            let row = mapped.row(ordinal).map_err(|error| {
                format!("read system {} row {ordinal}: {error}", system.system_id)
            })?;
            mapped.metadata(ordinal).map_err(|error| {
                format!(
                    "read system {} metadata {ordinal}: {error}",
                    system.system_id
                )
            })?;
            if let Some(launch_index) = row.launch_index {
                mapped.launch(launch_index).map_err(|error| {
                    format!(
                        "read system {} launch {launch_index}: {error}",
                        system.system_id
                    )
                })?;
            }
        }
        let games = u64::try_from(mapped.identity().games)
            .map_err(|_| "system game count exceeds u64".to_string())?;
        if games != system.active.games {
            return Err(format!(
                "system {} registry/artifact count mismatch: {} != {}",
                system.system_id, system.active.games, games
            ));
        }
        total_games = total_games
            .checked_add(games)
            .ok_or_else(|| "catalog game count overflow".to_string())?;
        writeln!(
            output,
            "catalog_system_tsv\tsystem={}\tgeneration={}\tgames={}",
            system.system_id, system.active.generation, games
        )
        .expect("write to String");
    }
    writeln!(
        output,
        "catalog_summary_tsv\tvalid=1\tgeneration={}\trefresh_generation={}\tsystems={}\ttotal_games={}\tfingerprint={}",
        manifest.generation,
        refresh.generation,
        manifest.systems.len(),
        total_games,
        fingerprint
    )
    .expect("write to String");
    Ok(output)
}
