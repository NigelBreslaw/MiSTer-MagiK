// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! UI-independent lifecycle state for progressive sharded catalogs.

use crate::catalog_classify::SystemId;
use crate::sharded_builder_protocol::{
    ProtocolSequence, ShardedCatalogEnvelope, ShardedCatalogEvent,
};
use crate::sharded_catalog::{CatalogError, CatalogReader, SystemSummary};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupCatalogRoute {
    ReturnCapsule,
    ShardedRegistry,
    LegacyCatalog,
    EmptyShell,
}

pub fn select_startup_catalog_route(
    return_capsule_ready: bool,
    sharded_registry_ready: bool,
    legacy_catalog_ready: bool,
) -> StartupCatalogRoute {
    if return_capsule_ready {
        StartupCatalogRoute::ReturnCapsule
    } else if sharded_registry_ready {
        StartupCatalogRoute::ShardedRegistry
    } else if legacy_catalog_ready {
        StartupCatalogRoute::LegacyCatalog
    } else {
        StartupCatalogRoute::EmptyShell
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LauncherSystemState {
    Queued,
    Scanning,
    Ready { generation: u64, games: u64 },
    Failed { stage: String, error: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LauncherSystemTile {
    pub system_id: SystemId,
    pub display_title: String,
    pub section: String,
    pub family: String,
    pub order: u32,
    pub state: LauncherSystemState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LauncherCatalogUpdate {
    Handshake,
    PlanReady { systems: usize },
    SystemChanged { system_id: SystemId },
    PausedForUi { system_id: SystemId },
    ManifestPublished { generation: u64 },
    Unchanged { generation: Option<u64> },
    Failure { stage: String, error: String },
    Done,
}

pub struct LauncherCatalogSession {
    sequence: ProtocolSequence,
    tiles: BTreeMap<SystemId, LauncherSystemTile>,
    manifest_generation: Option<u64>,
}

impl LauncherCatalogSession {
    pub fn seed(reader: &impl CatalogReader) -> Result<Self, CatalogError> {
        let registry = reader.open_registry()?;
        let manifest_generation = Some(registry.generation());
        let tiles = registry
            .systems()
            .iter()
            .map(|summary| {
                (
                    summary.system_id.clone(),
                    tile_from_summary(
                        summary,
                        LauncherSystemState::Ready {
                            generation: summary.generation,
                            games: summary.games,
                        },
                    ),
                )
            })
            .collect();
        Ok(Self {
            sequence: ProtocolSequence::default(),
            tiles,
            manifest_generation,
        })
    }

    pub fn empty() -> Self {
        Self {
            sequence: ProtocolSequence::default(),
            tiles: BTreeMap::new(),
            manifest_generation: None,
        }
    }

    pub fn manifest_generation(&self) -> Option<u64> {
        self.manifest_generation
    }

    pub fn tiles(&self) -> Vec<&LauncherSystemTile> {
        let mut tiles = self.tiles.values().collect::<Vec<_>>();
        tiles.sort_by(|left, right| {
            left.order
                .cmp(&right.order)
                .then_with(|| left.system_id.cmp(&right.system_id))
        });
        tiles
    }

    /// Refresh presentation metadata after `ManifestPublished` without
    /// opening any system navigation. Queued/failed placeholders not yet in
    /// the manifest remain visible.
    pub fn refresh_registry(&mut self, reader: &impl CatalogReader) -> Result<(), CatalogError> {
        let registry = reader.open_registry()?;
        if self
            .manifest_generation
            .is_some_and(|generation| generation != registry.generation())
        {
            return Err(CatalogError::new(
                "registry-refresh",
                "registry generation does not match published generation",
            ));
        }
        let published = registry
            .systems()
            .iter()
            .map(|summary| summary.system_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        self.tiles.retain(|system_id, tile| {
            published.contains(system_id)
                || !matches!(tile.state, LauncherSystemState::Ready { .. })
        });
        for summary in registry.systems() {
            self.tiles.insert(
                summary.system_id.clone(),
                tile_from_summary(
                    summary,
                    LauncherSystemState::Ready {
                        generation: summary.generation,
                        games: summary.games,
                    },
                ),
            );
        }
        self.manifest_generation = Some(registry.generation());
        Ok(())
    }

    pub fn handle(
        &mut self,
        envelope: &ShardedCatalogEnvelope,
    ) -> Result<LauncherCatalogUpdate, CatalogError> {
        let mut accepted = self.sequence.clone();
        accepted
            .accept(envelope)
            .map_err(|error| CatalogError::new("protocol-v2", error.to_string()))?;
        let update = self.apply(&envelope.event)?;
        self.sequence = accepted;
        Ok(update)
    }

    fn apply(
        &mut self,
        event: &ShardedCatalogEvent,
    ) -> Result<LauncherCatalogUpdate, CatalogError> {
        Ok(match event {
            ShardedCatalogEvent::Handshake { .. } => LauncherCatalogUpdate::Handshake,
            ShardedCatalogEvent::PlanReady { systems } => {
                LauncherCatalogUpdate::PlanReady { systems: *systems }
            }
            ShardedCatalogEvent::SystemQueued { system_id } => {
                let system_id = parsed_system(system_id)?;
                match self.tiles.get_mut(&system_id) {
                    Some(tile) if matches!(tile.state, LauncherSystemState::Ready { .. }) => {
                        return Err(CatalogError::new(
                            "protocol-v2",
                            "queued event would downgrade a ready system",
                        ));
                    }
                    Some(tile) => tile.state = LauncherSystemState::Queued,
                    None => {
                        self.tiles.insert(
                            system_id.clone(),
                            placeholder(system_id.clone(), LauncherSystemState::Queued),
                        );
                    }
                }
                LauncherCatalogUpdate::SystemChanged { system_id }
            }
            ShardedCatalogEvent::SystemScanning { system_id } => {
                let system_id = parsed_system(system_id)?;
                let tile = self
                    .tiles
                    .entry(system_id.clone())
                    .or_insert_with(|| placeholder(system_id.clone(), LauncherSystemState::Queued));
                if matches!(tile.state, LauncherSystemState::Ready { .. }) {
                    return Err(CatalogError::new(
                        "protocol-v2",
                        "scanning event would downgrade a ready system",
                    ));
                }
                tile.state = LauncherSystemState::Scanning;
                LauncherCatalogUpdate::SystemChanged { system_id }
            }
            ShardedCatalogEvent::SystemReady {
                system_id,
                generation,
                games,
            } => {
                let system_id = parsed_system(system_id)?;
                let tile = self
                    .tiles
                    .entry(system_id.clone())
                    .or_insert_with(|| placeholder(system_id.clone(), LauncherSystemState::Queued));
                tile.state = LauncherSystemState::Ready {
                    generation: *generation,
                    games: *games,
                };
                LauncherCatalogUpdate::SystemChanged { system_id }
            }
            ShardedCatalogEvent::SystemFailed {
                system_id,
                stage,
                error,
            } => {
                let system_id = parsed_system(system_id)?;
                let tile = self
                    .tiles
                    .entry(system_id.clone())
                    .or_insert_with(|| placeholder(system_id.clone(), LauncherSystemState::Queued));
                tile.state = LauncherSystemState::Failed {
                    stage: stage.clone(),
                    error: error.clone(),
                };
                LauncherCatalogUpdate::SystemChanged { system_id }
            }
            ShardedCatalogEvent::PausedForUi { system_id } => LauncherCatalogUpdate::PausedForUi {
                system_id: parsed_system(system_id)?,
            },
            ShardedCatalogEvent::ManifestPublished { generation, .. } => {
                self.manifest_generation = Some(*generation);
                LauncherCatalogUpdate::ManifestPublished {
                    generation: *generation,
                }
            }
            ShardedCatalogEvent::Unchanged { generation } => {
                self.manifest_generation = *generation;
                LauncherCatalogUpdate::Unchanged {
                    generation: *generation,
                }
            }
            ShardedCatalogEvent::Failure { stage, error } => LauncherCatalogUpdate::Failure {
                stage: stage.clone(),
                error: error.clone(),
            },
            ShardedCatalogEvent::Done => LauncherCatalogUpdate::Done,
        })
    }
}

fn tile_from_summary(summary: &SystemSummary, state: LauncherSystemState) -> LauncherSystemTile {
    LauncherSystemTile {
        system_id: summary.system_id.clone(),
        display_title: summary.display_title.clone(),
        section: summary.section.clone(),
        family: summary.family.clone(),
        order: summary.order,
        state,
    }
}

fn placeholder(system_id: SystemId, state: LauncherSystemState) -> LauncherSystemTile {
    LauncherSystemTile {
        display_title: system_id.as_str().to_ascii_uppercase(),
        system_id,
        section: "Scanning".to_string(),
        family: "Scanning".to_string(),
        order: u32::MAX,
        state,
    }
}

fn parsed_system(value: &str) -> Result<SystemId, CatalogError> {
    SystemId::parse(value).map_err(|error| CatalogError::new("protocol-v2", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sharded_builder_protocol::ShardedCatalogEnvelope;
    use crate::sharded_catalog::{CatalogGame, CatalogRegistry, RunId, SystemCatalog};

    #[test]
    fn return_capsule_always_precedes_sharded_and_legacy_catalogs() {
        assert_eq!(
            select_startup_catalog_route(true, true, true),
            StartupCatalogRoute::ReturnCapsule
        );
        assert_eq!(
            select_startup_catalog_route(false, true, true),
            StartupCatalogRoute::ShardedRegistry
        );
    }

    #[test]
    fn registry_seed_builds_shell_without_opening_any_system() {
        let reader = FixtureReader::default();
        let session = LauncherCatalogSession::seed(&reader).unwrap();
        assert_eq!(reader.system_opens.get(), 0);
        assert_eq!(session.manifest_generation(), Some(3));
        assert_eq!(session.tiles()[0].display_title, "Arcade");
    }

    #[test]
    fn registry_refresh_replaces_placeholder_metadata_without_opening_a_system() {
        let run = RunId::new("fixture").unwrap();
        let reader = FixtureReader::default();
        let mut session = LauncherCatalogSession::empty();
        session
            .handle(&ShardedCatalogEnvelope::new(
                &run,
                3,
                0,
                ShardedCatalogEvent::Handshake {
                    operation: "bootstrap".to_string(),
                    current_generation: None,
                },
            ))
            .unwrap();
        session
            .handle(&ShardedCatalogEnvelope::new(
                &run,
                3,
                1,
                ShardedCatalogEvent::SystemQueued {
                    system_id: "arcade".to_string(),
                },
            ))
            .unwrap();
        session
            .handle(&ShardedCatalogEnvelope::new(
                &run,
                3,
                2,
                ShardedCatalogEvent::ManifestPublished {
                    generation: 3,
                    systems: 1,
                },
            ))
            .unwrap();
        session.refresh_registry(&reader).unwrap();
        assert_eq!(reader.system_opens.get(), 0);
        assert_eq!(session.tiles()[0].display_title, "Arcade");
        assert_eq!(session.tiles()[0].section, "Arcade");
    }

    #[test]
    fn progressive_sequence_adds_scanning_then_ready_tiles() {
        let run = RunId::new("fixture").unwrap();
        let mut session = LauncherCatalogSession::empty();
        for (sequence, event) in [
            (
                0,
                ShardedCatalogEvent::Handshake {
                    operation: "bootstrap".to_string(),
                    current_generation: None,
                },
            ),
            (1, ShardedCatalogEvent::PlanReady { systems: 1 }),
            (
                2,
                ShardedCatalogEvent::SystemQueued {
                    system_id: "arcade".to_string(),
                },
            ),
            (
                3,
                ShardedCatalogEvent::SystemScanning {
                    system_id: "arcade".to_string(),
                },
            ),
            (
                4,
                ShardedCatalogEvent::SystemReady {
                    system_id: "arcade".to_string(),
                    generation: 1,
                    games: 42,
                },
            ),
            (
                5,
                ShardedCatalogEvent::ManifestPublished {
                    generation: 1,
                    systems: 1,
                },
            ),
            (6, ShardedCatalogEvent::Done),
        ] {
            session
                .handle(&ShardedCatalogEnvelope::new(&run, 1, sequence, event))
                .unwrap();
        }
        assert_eq!(session.manifest_generation(), Some(1));
        assert_eq!(
            session.tiles()[0].state,
            LauncherSystemState::Ready {
                generation: 1,
                games: 42
            }
        );
    }

    #[test]
    fn rejected_state_transition_does_not_consume_protocol_sequence() {
        let run = RunId::new("fixture").unwrap();
        let mut session = LauncherCatalogSession::seed(&FixtureReader::default()).unwrap();
        session
            .handle(&ShardedCatalogEnvelope::new(
                &run,
                4,
                0,
                ShardedCatalogEvent::Handshake {
                    operation: "reconcile".to_string(),
                    current_generation: Some(3),
                },
            ))
            .unwrap();
        assert!(session
            .handle(&ShardedCatalogEnvelope::new(
                &run,
                4,
                1,
                ShardedCatalogEvent::SystemQueued {
                    system_id: "arcade".to_string(),
                },
            ))
            .is_err());
        session
            .handle(&ShardedCatalogEnvelope::new(
                &run,
                4,
                1,
                ShardedCatalogEvent::SystemReady {
                    system_id: "arcade".to_string(),
                    generation: 4,
                    games: 11,
                },
            ))
            .unwrap();
    }

    #[derive(Default)]
    struct FixtureReader {
        system_opens: std::cell::Cell<usize>,
    }

    impl CatalogReader for FixtureReader {
        fn open_registry(&self) -> Result<CatalogRegistry, CatalogError> {
            Ok(CatalogRegistry::new(
                3,
                vec![SystemSummary {
                    system_id: SystemId::parse("arcade").unwrap(),
                    display_title: "Arcade".to_string(),
                    section: "Arcade".to_string(),
                    family: "Arcade".to_string(),
                    order: 0,
                    generation: 3,
                    games: 10,
                }],
            ))
        }

        fn open_system(&self, _system_id: &SystemId) -> Result<SystemCatalog, CatalogError> {
            self.system_opens.set(self.system_opens.get() + 1);
            Ok(SystemCatalog::new(
                SystemSummary {
                    system_id: SystemId::parse("arcade").unwrap(),
                    display_title: "Arcade".to_string(),
                    section: "Arcade".to_string(),
                    family: "Arcade".to_string(),
                    order: 0,
                    generation: 3,
                    games: 0,
                },
                Vec::<CatalogGame>::new(),
            ))
        }
    }
}
