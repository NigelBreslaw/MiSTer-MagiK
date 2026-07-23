// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Catalog-only vertical slice used to prove the sharded Module end to end.

use crate::catalog_classify::SystemId;
use crate::catalog_domain::ScanUnitId;
use crate::incremental_inputs::{
    InputFactStore, InputKind, InputProbePolicy, InputSnapshot, probe_scan_unit,
};
use crate::shard_registry::{
    CatalogManifest, ManifestSystem, RegistryLimits, garbage_collect_unreferenced,
    manifest_slots_present, publish_manifest, publish_system_artifacts, read_latest_manifest,
};
use crate::sharded_catalog::CatalogConfig;
use crate::system_shard::{
    LoadedSystemShard, SystemGame, SystemShardData, open_system_shard, write_system_shard,
};
use std::error::Error;
use std::fmt;
use std::fs;
#[cfg(test)]
use std::path::PathBuf;
use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerticalSliceOutcome {
    pub generation: u64,
    pub games: usize,
    pub published: bool,
    pub changed_inputs: usize,
}

pub fn bootstrap_fixture_system(
    config: &CatalogConfig,
    system_id: &SystemId,
    limits: RegistryLimits,
) -> Result<VerticalSliceOutcome, VerticalSliceError> {
    let source_root = config
        .source_roots()
        .first()
        .ok_or_else(|| VerticalSliceError::new("source", "missing source root"))?;
    let (fixture_directory, extension, title, section, family) = fixture_system(system_id)?;
    let scan_root = source_root.join("games").join(fixture_directory);
    let scan_unit_id = ScanUnitId::parse(&format!("{}-root", system_id.as_str()))
        .map_err(|error| VerticalSliceError::new("source", error.to_string()))?;
    fs::create_dir_all(config.storage_root())
        .map_err(|error| VerticalSliceError::with("create storage root", error))?;
    let mut state = InputFactStore::open(
        &config
            .storage_root()
            .join("state")
            .join("builder-state.sqlite3"),
    )
    .map_err(|error| VerticalSliceError::new("state", error.to_string()))?;
    let previous_snapshot = state
        .load_scan_unit(&scan_unit_id)
        .map_err(|error| VerticalSliceError::new("state", error.to_string()))?;
    let probe = probe_scan_unit(
        &scan_root,
        &scan_unit_id,
        &previous_snapshot,
        &FixturePolicy { extension },
    )
    .map_err(|error| VerticalSliceError::new("probe", error.to_string()))?;
    let current_manifest = match read_latest_manifest(config.storage_root(), limits) {
        Ok(manifest) => Some(manifest),
        Err(_) if manifest_slots_present(config.storage_root()) => {
            return Err(VerticalSliceError::new(
                "read",
                "manifest slots exist but none is valid",
            ));
        }
        Err(_) => None,
    };
    if probe.changes.is_empty()
        && let Some(existing) = current_manifest.as_ref().and_then(|manifest| {
            manifest
                .systems
                .iter()
                .find(|system| &system.system_id == system_id)
        })
    {
        let loaded = open_system_shard(
            &config.storage_root().join(&existing.active.sqlite_path),
            &config.storage_root().join(&existing.active.navigation_path),
            system_id,
            existing.active.generation,
            limits.shard,
        )
        .map_err(|error| VerticalSliceError::new("read", error.to_string()))?;
        return Ok(outcome(&loaded, false, 0));
    }

    let manifest_generation = current_manifest.as_ref().map_or(Ok(1), |manifest| {
        manifest
            .generation
            .checked_add(1)
            .ok_or_else(|| VerticalSliceError::new("plan", "manifest generation overflow"))
    })?;
    garbage_collect_unreferenced(
        config.storage_root(),
        current_manifest.as_ref().unwrap_or(&CatalogManifest {
            generation: 0,
            systems: Vec::new(),
        }),
    )
    .map_err(|error| VerticalSliceError::new("garbage-collect", error.to_string()))?;
    let games = project_games(&probe.snapshot, source_root, &scan_root)?;
    let run_nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| VerticalSliceError::new("stage", "clock predates Unix epoch"))?
        .as_nanos();
    let staging = config.storage_root().join("staging").join(format!(
        "vertical-{}-{manifest_generation}-{run_nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&staging)
        .map_err(|error| VerticalSliceError::with("create staging directory", error))?;
    let staged_sqlite = staging.join("system.sqlite3");
    let staged_navigation = staging.join("system.nav.lz4b");
    write_system_shard(
        &staged_sqlite,
        &staged_navigation,
        &SystemShardData {
            system_id: system_id.clone(),
            generation: manifest_generation,
            projection_stats: None,
            games: games.clone(),
        },
        limits.shard,
    )
    .map_err(|error| VerticalSliceError::new("write", error.to_string()))?;
    let published = publish_system_artifacts(
        config.storage_root(),
        &staged_sqlite,
        &staged_navigation,
        system_id,
        manifest_generation,
        games.len() as u64,
        limits,
    )
    .map_err(|error| VerticalSliceError::new("publish-artifact", error.to_string()))?;
    let previous = current_manifest
        .as_ref()
        .and_then(|manifest| {
            manifest
                .systems
                .iter()
                .find(|system| &system.system_id == system_id)
        })
        .map(|system| system.active.clone());
    let replacement = ManifestSystem {
        system_id: system_id.clone(),
        display_title: title.to_string(),
        section: section.to_string(),
        family: family.to_string(),
        order: 0,
        producers: vec![scan_unit_id.clone()],
        active: published.clone(),
        previous,
    };
    let mut systems = current_manifest.map_or_else(Vec::new, |manifest| manifest.systems);
    systems.retain(|system| &system.system_id != system_id);
    systems.push(replacement);
    systems.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    publish_manifest(
        config.storage_root(),
        &CatalogManifest {
            generation: manifest_generation,
            systems,
        },
        limits,
    )
    .map_err(|error| VerticalSliceError::new("publish-manifest", error.to_string()))?;
    state
        .apply_probe(&probe)
        .map_err(|error| VerticalSliceError::new("state", error.to_string()))?;
    let _ = fs::remove_dir(&staging);
    let loaded = open_system_shard(
        &config.storage_root().join(&published.sqlite_path),
        &config.storage_root().join(&published.navigation_path),
        system_id,
        manifest_generation,
        limits.shard,
    )
    .map_err(|error| VerticalSliceError::new("read", error.to_string()))?;
    Ok(outcome(&loaded, true, probe.changes.len()))
}

fn project_games(
    snapshot: &InputSnapshot,
    source_root: &Path,
    scan_root: &Path,
) -> Result<Vec<SystemGame>, VerticalSliceError> {
    let launch_base = scan_root
        .strip_prefix(source_root)
        .map_err(|_| VerticalSliceError::new("project", "scan root is outside source root"))?;
    let mut games = snapshot
        .facts()
        .iter()
        .filter(|(_, signature)| signature.kind == InputKind::File)
        .map(|(input, _)| {
            let relative = input.relative_path();
            let title = relative
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or_else(|| VerticalSliceError::new("project", "game title is not UTF-8"))?;
            let launch_ref = Path::new("/").join(launch_base).join(relative);
            Ok(SystemGame {
                stable_key: path_text(relative)?,
                title: title.to_string(),
                launch_ref: path_text(&launch_ref)?,
                ..SystemGame::default()
            })
        })
        .collect::<Result<Vec<_>, VerticalSliceError>>()?;
    games.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| left.stable_key.cmp(&right.stable_key))
    });
    Ok(games)
}

fn fixture_system(
    system_id: &SystemId,
) -> Result<
    (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ),
    VerticalSliceError,
> {
    match system_id.as_str() {
        "snes" => Ok(("SNES", "sfc", "SNES", "Consoles", "Nintendo")),
        "c64" => Ok(("C64", "d64", "Commodore 64", "Computers", "Commodore")),
        _ => Err(VerticalSliceError::new(
            "source",
            "catalog-lab vertical slice supports only snes and c64 fixtures",
        )),
    }
}

fn outcome(
    loaded: &LoadedSystemShard,
    published: bool,
    changed_inputs: usize,
) -> VerticalSliceOutcome {
    VerticalSliceOutcome {
        generation: loaded.generation,
        games: loaded.games.len(),
        published,
        changed_inputs,
    }
}

struct FixturePolicy {
    extension: &'static str,
}

impl InputProbePolicy for FixturePolicy {
    fn descend_into(&self, relative_directory: &Path) -> bool {
        relative_directory.components().all(|component| {
            let Component::Normal(value) = component else {
                return false;
            };
            !matches!(
                value.to_str().map(str::to_ascii_lowercase).as_deref(),
                Some("screenshots" | "cache" | "media")
            )
        })
    }

    fn include_file(&self, relative_file: &Path) -> bool {
        relative_file.file_name().and_then(|value| value.to_str()) != Some("gamelist.xml")
            && relative_file.extension().and_then(|value| value.to_str()) == Some(self.extension)
    }
}

fn path_text(path: &Path) -> Result<String, VerticalSliceError> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| VerticalSliceError::new("project", "fixture path is not UTF-8"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerticalSliceError {
    stage: &'static str,
    message: String,
}

impl VerticalSliceError {
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

impl fmt::Display for VerticalSliceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.stage, self.message)
    }
}

impl Error for VerticalSliceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shard_registry::RegistryLimits;
    use crate::synthetic_fixture::{SyntheticFixtureSpec, generate_synthetic_fixture};
    use crate::system_shard::SystemShardLimits;

    #[test]
    fn fixture_bootstrap_publishes_then_no_ops_without_magik() {
        let root = temporary_root("bootstrap");
        let source = root.join("source");
        let storage = root.join("catalog");
        generate_synthetic_fixture(
            &source,
            &SyntheticFixtureSpec {
                arcade_games: 0,
                small_system_games: 3,
                large_system_games: 0,
                large_system_depth: 2,
            },
        )
        .unwrap();
        let config = CatalogConfig::new(storage, vec![source], 1024 * 1024).unwrap();
        let system_id = SystemId::parse("snes").unwrap();
        let first = bootstrap_fixture_system(&config, &system_id, limits()).unwrap();
        assert_eq!(first.games, 3);
        assert!(first.published);
        assert_eq!(first.generation, 1);
        let manifest = read_latest_manifest(config.storage_root(), limits()).unwrap();
        let active = &manifest.systems[0].active;
        let sqlite = config.storage_root().join(&active.sqlite_path);
        let navigation = config.storage_root().join(&active.navigation_path);
        let sqlite_before = fs::read(&sqlite).unwrap();
        let navigation_before = fs::read(&navigation).unwrap();
        let sqlite_mtime = fs::metadata(&sqlite).unwrap().modified().unwrap();
        let navigation_mtime = fs::metadata(&navigation).unwrap().modified().unwrap();
        let second = bootstrap_fixture_system(&config, &system_id, limits()).unwrap();
        assert_eq!(second.games, 3);
        assert!(!second.published);
        assert_eq!(second.generation, 1);
        assert_eq!(fs::read(&sqlite).unwrap(), sqlite_before);
        assert_eq!(fs::read(&navigation).unwrap(), navigation_before);
        assert_eq!(
            fs::metadata(sqlite).unwrap().modified().unwrap(),
            sqlite_mtime
        );
        assert_eq!(
            fs::metadata(navigation).unwrap().modified().unwrap(),
            navigation_mtime
        );
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
            "mister-magik-vertical-slice-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
}
