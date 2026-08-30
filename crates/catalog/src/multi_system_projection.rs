// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Transactional multi-system projection from one globally resolved scan.

use crate::catalog_classify::{LauncherSection, SystemId, system_definition};
use crate::catalog_domain::ScanUnitId;
use crate::catalog_navigation::CatalogNavigationProjection;
use crate::library_db::{BenchConfig, scan_library_artifact};
use crate::shard_registry::{
    CatalogManifest, ManifestSystem, RegistryLimits, garbage_collect_unreferenced,
    manifest_slots_present, publish_manifest, publish_system_artifacts, read_latest_manifest,
};
use crate::sharded_catalog::CatalogConfig;
use crate::system_shard::{SystemGame, SystemShardData, write_system_shard};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultiSystemOutcome {
    pub manifest_generation: u64,
    pub systems: Vec<(SystemId, usize)>,
}

pub fn bootstrap_global_fixture(
    config: &CatalogConfig,
    scan_unit_id: &ScanUnitId,
    limits: RegistryLimits,
) -> Result<MultiSystemOutcome, MultiSystemError> {
    let _lease = crate::catalog_lease::CatalogMutationLease::acquire_default()
        .map_err(|error| MultiSystemError::new("lease", error.to_string()))?;
    let source_root = config
        .source_roots()
        .first()
        .ok_or_else(|| MultiSystemError::new("source", "missing source root"))?;
    fs::create_dir_all(config.storage_root())
        .map_err(|error| MultiSystemError::with("create storage root", error))?;
    let current_manifest = match read_latest_manifest(config.storage_root(), limits) {
        Ok(manifest) => Some(manifest),
        Err(_) if manifest_slots_present(config.storage_root()) => {
            return Err(MultiSystemError::new(
                "read",
                "manifest slots exist but none is valid",
            ));
        }
        Err(_) => None,
    };
    garbage_collect_unreferenced(
        config.storage_root(),
        current_manifest.as_ref().unwrap_or(&CatalogManifest {
            format: None,
            generation: 0,
            systems: Vec::new(),
        }),
    )
    .map_err(|error| MultiSystemError::new("garbage-collect", error.to_string()))?;
    let generation = current_manifest.as_ref().map_or(Ok(1), |manifest| {
        manifest
            .generation
            .checked_add(1)
            .ok_or_else(|| MultiSystemError::new("plan", "manifest generation overflow"))
    })?;

    let scan = scan_library_artifact(
        &BenchConfig {
            roots: vec![source_root.display().to_string()],
            sqlite_path: config.storage_root().join("state/unused-v2.sqlite3"),
        },
        None,
    );
    let catalog = scan.catalog(source_root);
    let projection = CatalogNavigationProjection::from_catalog(&catalog, scan.stamp());
    let mut games_by_system = BTreeMap::<SystemId, Vec<SystemGame>>::new();
    for game in projection.games {
        let system_id = SystemId::parse(&game.system_id)
            .map_err(|error| MultiSystemError::new("project", error.to_string()))?;
        let stable_key = format!("{}\u{1f}{}\u{1f}{}", system_id, game.title, game.launch_ref);
        games_by_system
            .entry(system_id)
            .or_default()
            .push(SystemGame {
                stable_key,
                title: game.title.to_string(),
                launch_ref: game.launch_ref.to_string(),
                ..SystemGame::default()
            });
    }
    for games in games_by_system.values_mut() {
        games.sort_by(|left, right| {
            left.title
                .to_ascii_lowercase()
                .cmp(&right.title.to_ascii_lowercase())
                .then_with(|| left.stable_key.cmp(&right.stable_key))
        });
    }

    let titles = projection
        .systems
        .into_iter()
        .map(|system| (system.id, system.title))
        .collect::<BTreeMap<_, _>>();
    let run_nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| MultiSystemError::new("stage", "clock predates Unix epoch"))?
        .as_nanos();
    let mut replacements = Vec::new();
    for (system_id, games) in &games_by_system {
        let staging = config.storage_root().join("staging").join(format!(
            "multi-{}-{generation}-{run_nonce}-{}",
            std::process::id(),
            system_id.as_str()
        ));
        fs::create_dir_all(&staging)
            .map_err(|error| MultiSystemError::with("create system staging", error))?;
        let sqlite = staging.join("system.sqlite3");
        let navigation = staging.join("system.nav.lz4b");
        write_system_shard(
            &sqlite,
            &navigation,
            &SystemShardData {
                system_id: system_id.clone(),
                generation,
                projection_stats: None,
                games: games.clone(),
            },
            limits.shard,
        )
        .map_err(|error| MultiSystemError::new("write", error.to_string()))?;
        let active = publish_system_artifacts(
            config.storage_root(),
            &sqlite,
            &navigation,
            system_id,
            generation,
            games.len() as u64,
            limits,
        )
        .map_err(|error| MultiSystemError::new("publish-artifact", error.to_string()))?;
        let _ = fs::remove_dir(staging);
        let previous = current_manifest.as_ref().and_then(|manifest| {
            manifest
                .systems
                .iter()
                .find(|system| &system.system_id == system_id)
                .map(|system| system.active.clone())
        });
        let definition = system_definition(system_id.as_str());
        replacements.push(ManifestSystem {
            system_id: system_id.clone(),
            display_title: titles
                .get(system_id.as_str())
                .cloned()
                .unwrap_or_else(|| system_id.as_str().to_string()),
            section: definition
                .map(|value| section_label(value.section))
                .unwrap_or("Other")
                .to_string(),
            family: definition
                .map(|value| value.family.as_str())
                .filter(|value| !value.is_empty())
                .unwrap_or("Other")
                .to_string(),
            order: definition.map_or(1000, |value| u32::from(value.order)),
            producers: vec![scan_unit_id.clone()],
            active,
            previous,
        });
    }
    let mut systems = current_manifest.map_or_else(Vec::new, |manifest| manifest.systems);
    systems.retain(|system| !system.producers.contains(scan_unit_id));
    systems.extend(replacements);
    systems.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    publish_manifest(
        config.storage_root(),
        &CatalogManifest {
            format: Some(crate::catalog_format::CatalogFormatDescriptor::current()),
            generation,
            systems,
        },
        limits,
    )
    .map_err(|error| MultiSystemError::new("publish-manifest", error.to_string()))?;
    Ok(MultiSystemOutcome {
        manifest_generation: generation,
        systems: games_by_system
            .into_iter()
            .map(|(system_id, games)| (system_id, games.len()))
            .collect(),
    })
}

fn section_label(section: LauncherSection) -> &'static str {
    match section {
        LauncherSection::Arcade => "Arcade",
        LauncherSection::SnkNeogeo => "SNK Neo Geo",
        LauncherSection::Consoles => "Consoles",
        LauncherSection::Handhelds => "Handhelds",
        LauncherSection::Computers => "Computers",
        LauncherSection::Other => "Other",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultiSystemError {
    stage: &'static str,
    message: String,
}

impl MultiSystemError {
    fn new(stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }

    fn with(stage: &'static str, error: impl fmt::Display) -> Self {
        Self::new(stage, error.to_string())
    }
}

impl fmt::Display for MultiSystemError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.stage, self.message)
    }
}

impl Error for MultiSystemError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_shard::{SystemShardLimits, open_system_shard};
    use std::path::PathBuf;

    #[test]
    fn one_source_scan_unit_publishes_multiple_logical_system_shards() {
        let root = temporary_root("source-multi");
        let source = root.join("source");
        let arcade = source.join("_Arcade");
        let snes = source.join("games/SNES");
        let consoles = source.join("_Console");
        fs::create_dir_all(&arcade).unwrap();
        fs::create_dir_all(&snes).unwrap();
        fs::create_dir_all(&consoles).unwrap();
        fs::write(consoles.join("SNES_20260717.rbf"), b"synthetic core").unwrap();
        fs::write(
            arcade.join("Arcade Game.mra"),
            "<misterromdescription><name>Arcade Game</name><setname>arcadegame</setname></misterromdescription>",
        )
        .unwrap();
        fs::write(snes.join("Console Game.sfc"), b"synthetic rom").unwrap();
        let config = CatalogConfig::new(root.join("catalog"), vec![source], 1024 * 1024).unwrap();
        let outcome = bootstrap_global_fixture(
            &config,
            &ScanUnitId::parse("source-root").unwrap(),
            limits(),
        )
        .unwrap();
        let ids = outcome
            .systems
            .iter()
            .map(|(system_id, _)| system_id.as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"arcade"), "{ids:?}");
        assert!(ids.contains(&"snes"), "{ids:?}");
        assert_eq!(ids.len(), 2, "{ids:?}");
        let manifest = read_latest_manifest(config.storage_root(), limits()).unwrap();
        assert_eq!(manifest.generation, 1);
        assert_eq!(manifest.systems.len(), outcome.systems.len());
        assert!(
            manifest
                .systems
                .iter()
                .all(|system| system.producers == vec![ScanUnitId::parse("source-root").unwrap()])
        );
        for system in &manifest.systems {
            let navigation_path = system.active.navigation_path.as_ref().unwrap();
            let loaded = open_system_shard(
                &config.storage_root().join(&system.active.sqlite_path),
                &config.storage_root().join(navigation_path),
                &system.system_id,
                manifest.generation,
                limits().shard,
            )
            .unwrap();
            assert_eq!(loaded.games.len(), 1);
        }
        fs::remove_dir_all(root).unwrap();
    }

    fn limits() -> RegistryLimits {
        RegistryLimits {
            max_manifest_bytes: 1024 * 1024,
            max_systems: 100,
            shard: SystemShardLimits {
                max_sqlite_bytes: 4 * 1024 * 1024,
                max_navigation_compressed_bytes: 1024 * 1024,
                max_navigation_decoded_bytes: 1024 * 1024,
                max_games: 10_000,
            },
        }
    }

    fn temporary_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mister-magik-multi-system-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
}
