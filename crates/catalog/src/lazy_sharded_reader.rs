// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Manifest-first reader which opens system navigation only on demand.

use crate::catalog_classify::SystemId;
use crate::catalog_format::CatalogFormatStatus;
use crate::shard_registry::{CatalogManifest, RegistryLimits, read_latest_manifest_lazy};
use crate::sharded_catalog::{
    CatalogError, CatalogGame, CatalogLaunchPlan, CatalogReader, CatalogRegistry, SystemCatalog,
    SystemSummary,
};
use crate::system_shard::{
    LoadedSystemShard, SystemNavigationOpenTiming, open_verified_system_navigation_with_timing,
};
use std::path::{Path, PathBuf};

pub struct LazyShardedCatalogReader {
    storage_root: PathBuf,
    limits: RegistryLimits,
    manifest: CatalogManifest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LazySystemGeneration {
    pub generation: u64,
    pub navpack_path: Option<PathBuf>,
    pub navpack_bytes: u64,
    pub games: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct EntryPreludeWarmupReport {
    pub systems: usize,
    pub viewport_rows: usize,
    pub exact_previews: usize,
    pub terminal_empty: usize,
    pub elapsed_us: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct LazySystemOpenTiming {
    pub descriptor_lookup_us: u64,
    pub navigation: SystemNavigationOpenTiming,
    pub projection_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navpack: Option<crate::arcade_catalog::NavPackCollectionOpenTiming>,
}

impl LazyShardedCatalogReader {
    pub fn open(storage_root: &Path, limits: RegistryLimits) -> Result<Self, CatalogError> {
        let manifest = read_latest_manifest_lazy(storage_root, limits)
            .map_err(|error| CatalogError::new("read-manifest", error.to_string()))?;
        match manifest.format.as_ref() {
            None => {
                return Err(CatalogError::new(
                    "read-manifest",
                    "catalog format descriptor is missing",
                ));
            }
            Some(format) => match crate::catalog_format::classify(format) {
                CatalogFormatStatus::Current => {}
                CatalogFormatStatus::UpgradeRequired {
                    installed,
                    required,
                } => {
                    return Err(CatalogError::new(
                        "read-manifest",
                        format!(
                            "catalog rebuild required: installed {}, required {}",
                            installed.label(),
                            required.label()
                        ),
                    ));
                }
                CatalogFormatStatus::UnsupportedFuture {
                    installed,
                    required,
                } => {
                    return Err(CatalogError::new(
                        "read-manifest",
                        format!(
                            "unsupported future catalog format: installed {}, required {}",
                            installed.label(),
                            required.label()
                        ),
                    ));
                }
                CatalogFormatStatus::Corrupt {
                    installed,
                    required,
                } => {
                    return Err(CatalogError::new(
                        "read-manifest",
                        format!(
                            "incoherent catalog format: installed {}, required {}",
                            installed.label(),
                            required.label()
                        ),
                    ));
                }
            },
        }
        Ok(Self {
            storage_root: storage_root.to_path_buf(),
            limits,
            manifest,
        })
    }

    pub fn open_system_with_timing(
        &self,
        system_id: &SystemId,
    ) -> Result<(SystemCatalog, LazySystemOpenTiming), CatalogError> {
        let (loaded, mut timing) = self.open_system_shard_with_timing(system_id)?;
        let system = self
            .manifest
            .systems
            .iter()
            .find(|system| &system.system_id == system_id)
            .ok_or_else(|| CatalogError::new("open-system", "system is absent from manifest"))?;
        let projection_started = std::time::Instant::now();
        let catalog = SystemCatalog::new(
            SystemSummary {
                system_id: loaded.system_id,
                display_title: system.display_title.clone(),
                section: system.section.clone(),
                family: system.family.clone(),
                order: system.order,
                generation: loaded.generation,
                games: loaded.games.len() as u64,
            },
            loaded
                .games
                .into_iter()
                .map(|game| CatalogGame {
                    stable_key: game.stable_key,
                    title: game.title,
                    launch_ref: game.launch_ref,
                    preview_archive_path: game.preview_archive_path,
                    preview_asset_key: game.preview_asset_key,
                    has_preview: game.has_preview,
                    year: game.year,
                    manufacturer: game.manufacturer,
                    category: game.category,
                    players: game.players,
                    control: game.control,
                    is_new: game.is_new,
                    launch_plan: game.launch_plan.map(|plan| CatalogLaunchPlan {
                        launch_ref: plan.launch_ref,
                        title: plan.title,
                        system_id: plan.system_id,
                        core_path: plan.core_path,
                        payload_path: plan.payload_path,
                        mount_kind: plan.mount_kind,
                        mount_index: plan.mount_index,
                        delay_secs: plan.delay_secs,
                    }),
                })
                .collect(),
        );
        timing.projection_us = elapsed_us(projection_started);
        Ok((catalog, timing))
    }

    pub fn open_system_shard_with_timing(
        &self,
        system_id: &SystemId,
    ) -> Result<(LoadedSystemShard, LazySystemOpenTiming), CatalogError> {
        let descriptor_started = std::time::Instant::now();
        let system = self
            .manifest
            .systems
            .iter()
            .find(|system| &system.system_id == system_id)
            .ok_or_else(|| CatalogError::new("open-system", "system is absent from manifest"))?;
        let descriptor_lookup_us = elapsed_us(descriptor_started);
        let generation = &system.active;
        if generation.is_artifactless() {
            return Ok((
                LoadedSystemShard {
                    system_id: system.system_id.clone(),
                    generation: generation.generation,
                    navigation_hash: String::new(),
                    projection_stats: None,
                    navigation_indexes: Default::default(),
                    games: Vec::new(),
                },
                LazySystemOpenTiming {
                    descriptor_lookup_us,
                    ..Default::default()
                },
            ));
        }
        let navigation_path = generation.navigation_path.as_ref().ok_or_else(|| {
            CatalogError::new(
                "open-system",
                "active system generation has no adjacent navigation; use NavPack",
            )
        })?;
        let navigation_hash = generation.navigation_hash.as_ref().ok_or_else(|| {
            CatalogError::new(
                "open-system",
                "active system generation has no adjacent navigation hash",
            )
        })?;
        let (loaded, navigation) = open_verified_system_navigation_with_timing(
            &self.storage_root.join(navigation_path),
            system_id,
            generation.generation,
            navigation_hash,
            self.limits.shard,
        )
        .map_err(|error| CatalogError::new("open-system", error.to_string()))?;
        Ok((
            loaded,
            LazySystemOpenTiming {
                descriptor_lookup_us,
                navigation,
                projection_us: 0,
                navpack: None,
            },
        ))
    }

    /// Returns the immutable active NavPack descriptor without opening the artifact.
    pub fn active_system_generation(
        &self,
        system_id: &SystemId,
    ) -> Result<LazySystemGeneration, CatalogError> {
        let system = self
            .manifest
            .systems
            .iter()
            .find(|system| &system.system_id == system_id)
            .ok_or_else(|| CatalogError::new("open-system", "system is absent from manifest"))?;
        let generation = &system.active;
        let navpack = generation.navpack.as_ref();
        Ok(LazySystemGeneration {
            generation: generation.generation,
            navpack_path: navpack.map(|navpack| self.storage_root.join(&navpack.path)),
            navpack_bytes: navpack.map_or(0, |navpack| navpack.bytes),
            games: usize::try_from(generation.games).map_err(|_| {
                CatalogError::new("open-system", "system game count exceeds platform size")
            })?,
        })
    }

    /// Maps and faults only each populated system's bounded entry prelude.
    pub fn warm_entry_preludes(&self) -> Result<EntryPreludeWarmupReport, CatalogError> {
        let started = std::time::Instant::now();
        let mut report = EntryPreludeWarmupReport::default();
        for system in &self.manifest.systems {
            if system.active.games == 0 {
                continue;
            }
            let generation = &system.active;
            let navpack = generation.navpack.as_ref().ok_or_else(|| {
                CatalogError::new(
                    "warm-entry-preludes",
                    format!("{} active generation has no NavPack", system.system_id),
                )
            })?;
            let games = usize::try_from(generation.games).map_err(|_| {
                CatalogError::new(
                    "warm-entry-preludes",
                    "system game count exceeds platform size",
                )
            })?;
            let path = self.storage_root.join(&navpack.path);
            let (mapped, _) = crate::navpack::MappedNavPack::open(
                &path,
                navpack.bytes,
                system.system_id.as_str(),
                generation.generation,
                games,
            )
            .map_err(|error| {
                CatalogError::new(
                    "warm-entry-preludes",
                    format!("{} active NavPack is unusable: {error}", system.system_id),
                )
            })?;
            let prelude = mapped.fault_entry_viewport().map_err(|error| {
                CatalogError::new(
                    "warm-entry-preludes",
                    format!("{} entry prelude is unusable: {error}", system.system_id),
                )
            })?;
            let viewport_rows = prelude.first_viewport_ordinals.len();
            let exact_preview = prelude.selected_preview.is_some();
            let terminal_empty = prelude.terminal_empty;
            report.systems += 1;
            report.viewport_rows += viewport_rows;
            report.exact_previews += usize::from(exact_preview);
            report.terminal_empty += usize::from(terminal_empty);
        }
        report.elapsed_us = elapsed_us(started);
        Ok(report)
    }
}

fn elapsed_us(started: std::time::Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

impl CatalogReader for LazyShardedCatalogReader {
    fn open_registry(&self) -> Result<CatalogRegistry, CatalogError> {
        Ok(CatalogRegistry::new(
            self.manifest.generation,
            self.manifest
                .systems
                .iter()
                .map(|system| SystemSummary {
                    system_id: system.system_id.clone(),
                    display_title: system.display_title.clone(),
                    section: system.section.clone(),
                    family: system.family.clone(),
                    order: system.order,
                    generation: system.active.generation,
                    games: system.active.games,
                })
                .collect(),
        ))
    }

    fn open_system(&self, system_id: &SystemId) -> Result<SystemCatalog, CatalogError> {
        self.open_system_with_timing(system_id)
            .map(|(catalog, _)| catalog)
    }
}

#[cfg(all(test, feature = "builder"))]
mod tests {
    use super::*;
    use crate::catalog_domain::ScanUnitId;
    use crate::shard_registry::{
        CatalogManifest, ManifestSystem, publish_manifest, publish_system_artifacts,
    };
    use crate::system_shard::{SystemGame, SystemShardData, SystemShardLimits, write_system_shard};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn registry_open_succeeds_when_every_system_artifact_is_absent() {
        let root = temporary_root("registry-only");
        seed(&root);
        fs::remove_dir_all(root.join("systems")).unwrap();
        let reader = LazyShardedCatalogReader::open(&root, limits()).unwrap();
        let registry = reader.open_registry().unwrap();
        assert_eq!(registry.generation(), 1);
        assert_eq!(registry.systems().len(), 2);
        assert!(reader.open_system(&system("snes")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn entry_prelude_warmup_maps_every_system_without_opening_shards() {
        let root = temporary_root("entry-prelude-warmup");
        seed(&root);
        fs::write(
            root.join("systems/c64/1.nav.lz4b"),
            b"corrupt navigation must remain untouched",
        )
        .unwrap();
        fs::write(
            root.join("systems/snes/1.nav.lz4b"),
            b"corrupt navigation must remain untouched",
        )
        .unwrap();

        let reader = LazyShardedCatalogReader::open(&root, limits()).unwrap();
        let report = reader.warm_entry_preludes().unwrap();
        assert_eq!(report.systems, 2);
        assert_eq!(report.viewport_rows, 2);
        assert_eq!(report.exact_previews, 0);
        assert_eq!(report.terminal_empty, 2);
        assert!(reader.open_system(&system("c64")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opening_one_system_does_not_touch_a_corrupt_unrelated_shard() {
        let root = temporary_root("one-system");
        seed(&root);
        fs::write(
            root.join("systems/c64/1.nav.lz4b"),
            b"corrupt unrelated shard",
        )
        .unwrap();
        fs::remove_file(root.join("systems/snes/1.sqlite3")).unwrap();
        let reader = LazyShardedCatalogReader::open(&root, limits()).unwrap();
        let snes = reader.open_system(&system("snes")).unwrap();
        assert_eq!(snes.summary().system_id.as_str(), "snes");
        assert_eq!(snes.games().len(), 1);
        assert!(reader.open_system(&system("c64")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn launcher_open_returns_owned_persisted_rows_without_catalog_projection() {
        let root = temporary_root("owned-system-rows");
        seed(&root);
        let reader = LazyShardedCatalogReader::open(&root, limits()).unwrap();

        let (loaded, timing) = reader
            .open_system_shard_with_timing(&system("snes"))
            .unwrap();

        assert_eq!(loaded.system_id.as_str(), "snes");
        assert_eq!(loaded.generation, 1);
        assert_eq!(loaded.games.len(), 1);
        assert_eq!(loaded.games[0].title, "SNES Game");
        assert_eq!(timing.projection_us, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_active_system_is_rejected_even_when_a_previous_generation_exists() {
        let root = temporary_root("corrupt-active-system");
        seed(&root);
        let current = crate::shard_registry::read_latest_manifest(&root, limits()).unwrap();
        let snes_id = system("snes");
        let staging = root.join("staging/snes-generation-2");
        fs::create_dir_all(&staging).unwrap();
        let sqlite = staging.join("system.sqlite3");
        let navigation = staging.join("system.nav.lz4b");
        write_system_shard(
            &sqlite,
            &navigation,
            &SystemShardData {
                system_id: snes_id.clone(),
                generation: 2,
                projection_stats: None,
                games: vec![SystemGame {
                    stable_key: "new".to_string(),
                    title: "New SNES Game".to_string(),
                    launch_ref: "/games/SNES/New SNES Game".to_string(),
                    ..SystemGame::default()
                }],
            },
            limits().shard,
        )
        .unwrap();
        let active =
            publish_system_artifacts(&root, &sqlite, &navigation, &snes_id, 2, 1, limits())
                .unwrap();
        let mut systems = current.systems;
        let snes = systems
            .iter_mut()
            .find(|system| system.system_id == snes_id)
            .unwrap();
        snes.previous = Some(snes.active.clone());
        snes.active = active;
        publish_manifest(
            &root,
            &CatalogManifest {
                format: Some(crate::catalog_format::CatalogFormatDescriptor::current()),
                generation: 2,
                systems,
            },
            limits(),
        )
        .unwrap();
        fs::write(
            root.join("systems/snes/2.nav.lz4b"),
            b"corrupt active shard",
        )
        .unwrap();

        let reader = LazyShardedCatalogReader::open(&root, limits()).unwrap();
        assert!(reader.open_system(&snes_id).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    fn seed(root: &Path) {
        fs::create_dir_all(root).unwrap();
        let mut systems = Vec::new();
        for (id, title) in [("c64", "C64 Game"), ("snes", "SNES Game")] {
            let system_id = system(id);
            let staging = root.join("staging").join(id);
            fs::create_dir_all(&staging).unwrap();
            let sqlite = staging.join("system.sqlite3");
            let navigation = staging.join("system.nav.lz4b");
            write_system_shard(
                &sqlite,
                &navigation,
                &SystemShardData {
                    system_id: system_id.clone(),
                    generation: 1,
                    projection_stats: None,
                    games: vec![SystemGame {
                        stable_key: title.to_ascii_lowercase(),
                        title: title.to_string(),
                        launch_ref: format!("/games/{id}/{title}"),
                        ..SystemGame::default()
                    }],
                },
                limits().shard,
            )
            .unwrap();
            let active =
                publish_system_artifacts(root, &sqlite, &navigation, &system_id, 1, 1, limits())
                    .unwrap();
            systems.push(ManifestSystem {
                system_id,
                display_title: id.to_ascii_uppercase(),
                section: "Fixture".to_string(),
                family: "Fixture".to_string(),
                order: 0,
                producers: vec![ScanUnitId::parse(&format!("{id}-root")).unwrap()],
                active,
                previous: None,
            });
        }
        systems.sort_by(|left, right| left.system_id.cmp(&right.system_id));
        publish_manifest(
            root,
            &CatalogManifest {
                format: Some(crate::catalog_format::CatalogFormatDescriptor::current()),
                generation: 1,
                systems,
            },
            limits(),
        )
        .unwrap();
    }

    fn system(value: &str) -> SystemId {
        SystemId::parse(value).unwrap()
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
            "mister-magik-lazy-reader-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
}
