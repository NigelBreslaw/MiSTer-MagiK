// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only integrity reports for the installed catalog.

use crate::sharded_catalog::CatalogReader;
use sha2::{Digest, Sha256};
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
    let mut identity_digest = Sha256::new();
    let mut ordering_digest = Sha256::new();
    let mut launch_digest = Sha256::new();
    let mut search_digest = Sha256::new();
    let mut artifact_digest = Sha256::new();
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
            let metadata = mapped.metadata(ordinal).map_err(|error| {
                format!(
                    "read system {} metadata {ordinal}: {error}",
                    system.system_id
                )
            })?;
            digest_fields(
                &mut identity_digest,
                [system.system_id.as_str(), row.launch_ref],
            );
            digest_fields(
                &mut ordering_digest,
                [
                    system.system_id.as_str(),
                    &ordinal.to_string(),
                    row.title,
                    row.launch_ref,
                ],
            );
            digest_fields(
                &mut search_digest,
                [
                    system.system_id.as_str(),
                    row.title,
                    metadata.manufacturer,
                    metadata.category,
                    metadata.control,
                ],
            );
            if let Some(launch_index) = row.launch_index {
                let launch = mapped.launch(launch_index).map_err(|error| {
                    format!(
                        "read system {} launch {launch_index}: {error}",
                        system.system_id
                    )
                })?;
                digest_fields(
                    &mut launch_digest,
                    [
                        launch.launch_ref,
                        launch.system_id,
                        launch.core_path,
                        launch.payload_path,
                        launch.mount_kind,
                    ],
                );
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
        writeln!(
            output,
            "catalog_v3_system_tsv\tsystem={}\tregistry_games={}\trole=fast-catalog\tsource_games={}\tvisible_families={}\tcollapsed_variants=0",
            system.system_id, games, games, games
        )
        .expect("write to String");
        for (kind, path, bytes, hash) in [
            (
                "sqlite",
                &system.active.sqlite_path,
                system.active.sqlite_bytes,
                system.active.sqlite_hash.as_str(),
            ),
            (
                "navpack",
                &navpack.path,
                navpack.bytes,
                navpack.hash.as_str(),
            ),
        ] {
            digest_fields(
                &mut artifact_digest,
                [
                    system.system_id.as_str(),
                    kind,
                    path.to_string_lossy().as_ref(),
                    &bytes.to_string(),
                    hash,
                ],
            );
            writeln!(
                output,
                "catalog_v3_artifact_tsv\tsystem={}\tkind={}\tpath={}\tbytes={}\tsha256={}",
                system.system_id,
                kind,
                path.display(),
                bytes,
                hash
            )
            .expect("write to String");
        }
    }
    let identity_sha256 = digest_hex(identity_digest);
    let ordering_sha256 = digest_hex(ordering_digest);
    let launch_sha256 = digest_hex(launch_digest);
    let search_sha256 = digest_hex(search_digest);
    let artifact_set_sha256 = digest_hex(artifact_digest);
    writeln!(
        output,
        "catalog_v3_summary_tsv\tvalid=1\tschema=2\tgeneration={}\ttotal_games={}\tidentity_sha256={}\tordering_sha256={}\tlaunch_sha256={}\tsearch_sha256={}\tartifact_set_sha256={}",
        manifest.generation,
        total_games,
        identity_sha256,
        ordering_sha256,
        launch_sha256,
        search_sha256,
        artifact_set_sha256,
    )
    .expect("write to String");
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

fn digest_fields<'a>(digest: &mut Sha256, fields: impl IntoIterator<Item = &'a str>) {
    for field in fields {
        digest.update(field.as_bytes());
        digest.update([0]);
    }
    digest.update([0xff]);
}

fn digest_hex(digest: Sha256) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = digest.finalize();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}
