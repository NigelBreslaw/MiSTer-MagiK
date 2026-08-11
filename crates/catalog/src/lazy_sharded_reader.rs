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
    NavigationCompatibility, SystemNavigationOpenTiming,
    open_system_navigation_with_compatibility_and_timing,
    open_verified_system_navigation_with_compatibility_and_timing,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct LazyShardedCatalogReader {
    storage_root: PathBuf,
    limits: RegistryLimits,
    manifest: CatalogManifest,
    navigation_compatibility: NavigationCompatibility,
}

#[derive(Clone, Debug, Default)]
pub struct VerifiedCatalogArtifacts {
    manifest_generation: u64,
    systems: BTreeMap<SystemId, Vec<VerifiedNavigationArtifact>>,
}

#[derive(Clone, Debug)]
struct VerifiedNavigationArtifact {
    generation: u64,
    path: PathBuf,
    hash: String,
    bytes: u64,
    modified: Option<SystemTime>,
}

impl VerifiedCatalogArtifacts {
    pub fn verified_count(&self) -> usize {
        self.systems.values().map(Vec::len).sum()
    }

    fn artifact(
        &self,
        manifest_generation: u64,
        system_id: &SystemId,
        generation: &crate::shard_registry::PublishedGeneration,
        storage_root: &Path,
    ) -> Option<&VerifiedNavigationArtifact> {
        if self.manifest_generation != manifest_generation {
            return None;
        }
        let artifact = self.systems.get(system_id)?.iter().find(|artifact| {
            artifact.generation == generation.generation
                && artifact.path == generation.navigation_path
                && artifact.hash == generation.navigation_hash
                && artifact.bytes == generation.navigation_bytes
        })?;
        artifact
            .matches_file(&storage_root.join(&artifact.path))
            .then_some(artifact)
    }
}

impl VerifiedNavigationArtifact {
    fn matches_file(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_file()
                && metadata.len() == self.bytes
                && metadata.modified().ok() == self.modified
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct LazySystemOpenTiming {
    pub descriptor_lookup_us: u64,
    pub navigation: SystemNavigationOpenTiming,
    pub projection_us: u64,
}

impl LazyShardedCatalogReader {
    pub fn open(storage_root: &Path, limits: RegistryLimits) -> Result<Self, CatalogError> {
        let manifest = read_latest_manifest_lazy(storage_root, limits)
            .map_err(|error| CatalogError::new("read-manifest", error.to_string()))?;
        let navigation_compatibility = match manifest.format.as_ref() {
            None => NavigationCompatibility::CurrentOrAlphaV1,
            Some(format) => match crate::catalog_format::classify(format) {
                CatalogFormatStatus::Current => NavigationCompatibility::CurrentOnly,
                CatalogFormatStatus::UpgradeRequired { .. } => {
                    NavigationCompatibility::CurrentOrAlphaV1
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
        };
        Ok(Self {
            storage_root: storage_root.to_path_buf(),
            limits,
            manifest,
            navigation_compatibility,
        })
    }

    pub fn verify_navigation_artifacts(&self) -> VerifiedCatalogArtifacts {
        let mut verified = VerifiedCatalogArtifacts {
            manifest_generation: self.manifest.generation,
            systems: BTreeMap::new(),
        };
        for system in &self.manifest.systems {
            for generation in std::iter::once(&system.active).chain(system.previous.iter()) {
                let path = self.storage_root.join(&generation.navigation_path);
                let Ok(metadata) = fs::symlink_metadata(&path) else {
                    continue;
                };
                if !metadata.file_type().is_file()
                    || metadata.len() != generation.navigation_bytes
                    || metadata.len() > self.limits.shard.max_navigation_compressed_bytes as u64
                {
                    continue;
                }
                let Ok(bytes) = fs::read(&path) else {
                    continue;
                };
                if crate::system_shard::checksum_hex(&bytes) != generation.navigation_hash {
                    continue;
                }
                verified
                    .systems
                    .entry(system.system_id.clone())
                    .or_default()
                    .push(VerifiedNavigationArtifact {
                        generation: generation.generation,
                        path: generation.navigation_path.clone(),
                        hash: generation.navigation_hash.clone(),
                        bytes: generation.navigation_bytes,
                        modified: metadata.modified().ok(),
                    });
            }
        }
        verified
    }

    pub fn open_system_with_timing(
        &self,
        system_id: &SystemId,
    ) -> Result<(SystemCatalog, LazySystemOpenTiming), CatalogError> {
        self.open_system_with_verified_timing(system_id, None)
    }

    pub fn open_system_with_verified_timing(
        &self,
        system_id: &SystemId,
        verified: Option<&VerifiedCatalogArtifacts>,
    ) -> Result<(SystemCatalog, LazySystemOpenTiming), CatalogError> {
        let descriptor_started = std::time::Instant::now();
        let system = self
            .manifest
            .systems
            .binary_search_by(|system| system.system_id.cmp(system_id))
            .ok()
            .and_then(|index| self.manifest.systems.get(index))
            .ok_or_else(|| CatalogError::new("open-system", "system is absent from manifest"))?;
        let descriptor_lookup_us = elapsed_us(descriptor_started);
        let open = |generation: &crate::shard_registry::PublishedGeneration| {
            let path = self.storage_root.join(&generation.navigation_path);
            let verified_artifact = verified.and_then(|verified| {
                verified.artifact(
                    self.manifest.generation,
                    system_id,
                    generation,
                    &self.storage_root,
                )
            });
            let result = match verified_artifact {
                Some(artifact) => open_verified_system_navigation_with_compatibility_and_timing(
                    &path,
                    system_id,
                    generation.generation,
                    &artifact.hash,
                    self.limits.shard,
                    self.navigation_compatibility,
                ),
                None => open_system_navigation_with_compatibility_and_timing(
                    &path,
                    system_id,
                    generation.generation,
                    self.limits.shard,
                    self.navigation_compatibility,
                ),
            };
            let (loaded, timing) =
                result.map_err(|error| CatalogError::new("open-system", error.to_string()))?;
            if loaded.navigation_hash != generation.navigation_hash {
                return Err(CatalogError::new(
                    "open-system",
                    "navigation checksum does not match manifest",
                ));
            }
            Ok((loaded, timing))
        };
        let (loaded, navigation) = match open(&system.active) {
            Ok(loaded) => loaded,
            Err(active_error) => match system.previous.as_ref() {
                Some(previous) => open(previous).map_err(|previous_error| {
                    CatalogError::new(
                        "open-system",
                        format!(
                            "active shard failed: {active_error}; previous shard failed: {previous_error}"
                        ),
                    )
                })?,
                None => {
                    return Err(CatalogError::new("open-system", active_error.to_string()));
                }
            },
        };
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
        Ok((
            catalog,
            LazySystemOpenTiming {
                descriptor_lookup_us,
                navigation,
                projection_us: elapsed_us(projection_started),
            },
        ))
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
    fn verified_artifacts_skip_entry_hashing_but_stale_tokens_do_not() {
        let root = temporary_root("verified-artifacts");
        seed(&root);
        let reader = LazyShardedCatalogReader::open(&root, limits()).unwrap();
        let verified = reader.verify_navigation_artifacts();
        assert_eq!(verified.verified_count(), 2);

        let (_, timing) = reader
            .open_system_with_verified_timing(&system("snes"), Some(&verified))
            .unwrap();
        assert_eq!(timing.navigation.hash_us, 0);

        let stale = VerifiedCatalogArtifacts {
            manifest_generation: 0,
            ..VerifiedCatalogArtifacts::default()
        };
        let (_, timing) = reader
            .open_system_with_verified_timing(&system("snes"), Some(&stale))
            .unwrap();
        assert!(timing.navigation.hash_us > 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_active_system_falls_back_to_its_previous_generation() {
        let root = temporary_root("system-fallback");
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
        let reader = LazyShardedCatalogReader::open(&root, limits()).unwrap();
        let verified = reader.verify_navigation_artifacts();
        fs::write(
            root.join("systems/snes/2.nav.lz4b"),
            b"corrupt active shard",
        )
        .unwrap();

        let (snes, timing) = reader
            .open_system_with_verified_timing(&snes_id, Some(&verified))
            .unwrap();
        assert_eq!(snes.summary().generation, 1);
        assert_eq!(snes.games()[0].title, "SNES Game");
        assert_eq!(
            timing.navigation.hash_us, 0,
            "previous generation stays verified"
        );
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
