// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Typed Interface for the production-inactive sharded catalog architecture.

use crate::catalog_classify::SystemId;
use crate::catalog_domain::{InputId, ScanUnitId};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

pub const SHARD_SCHEMA_VERSION: u32 = 6;
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const NAVIGATION_SCHEMA_VERSION: u32 = 3;
pub const PRODUCTION_PROJECTION_CONTRACT: &str = "rich-game-v2";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogConfig {
    storage_root: PathBuf,
    source_roots: Vec<PathBuf>,
    mame_database: Option<PathBuf>,
    hbmame_database: Option<PathBuf>,
    max_navigation_decoded_bytes: usize,
}

impl CatalogConfig {
    pub fn new(
        storage_root: PathBuf,
        source_roots: Vec<PathBuf>,
        max_navigation_decoded_bytes: usize,
    ) -> Result<Self, CatalogError> {
        if storage_root.as_os_str().is_empty() {
            return Err(CatalogError::configuration("storage root is empty"));
        }
        if source_roots.is_empty() {
            return Err(CatalogError::configuration("source roots are empty"));
        }
        if source_roots.iter().any(|root| root.as_os_str().is_empty()) {
            return Err(CatalogError::configuration("a source root is empty"));
        }
        if max_navigation_decoded_bytes == 0 {
            return Err(CatalogError::configuration(
                "navigation decoded-byte limit is zero",
            ));
        }
        Ok(Self {
            storage_root,
            source_roots,
            mame_database: None,
            hbmame_database: None,
            max_navigation_decoded_bytes,
        })
    }

    pub fn with_mame_database(mut self, path: PathBuf) -> Self {
        self.mame_database = Some(path);
        self
    }

    pub fn with_hbmame_database(mut self, path: PathBuf) -> Self {
        self.hbmame_database = Some(path);
        self
    }

    pub fn storage_root(&self) -> &Path {
        &self.storage_root
    }

    pub fn source_roots(&self) -> &[PathBuf] {
        &self.source_roots
    }

    pub fn mame_database(&self) -> Option<&Path> {
        self.mame_database.as_deref()
    }

    pub fn hbmame_database(&self) -> Option<&Path> {
        self.hbmame_database.as_deref()
    }

    pub fn max_navigation_decoded_bytes(&self) -> usize {
        self.max_navigation_decoded_bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogCommand {
    Bootstrap { first: SystemId },
    Reconcile,
    RebuildSystem { system_id: SystemId },
    Check,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconcilePlan {
    pub current_generation: Option<u64>,
    pub intended_generation: u64,
    pub scan_units: Vec<PlannedScanUnit>,
    pub systems: Vec<PlannedSystem>,
    pub global_rebuild: bool,
    pub manifest_only: bool,
}

impl ReconcilePlan {
    pub fn is_unchanged(&self) -> bool {
        !self.global_rebuild
            && !self.manifest_only
            && self.scan_units.is_empty()
            && self.systems.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedScanUnit {
    pub scan_unit_id: ScanUnitId,
    pub inputs: Vec<PlannedInput>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedInput {
    pub input_id: InputId,
    pub change: PlannedInputChange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannedInputChange {
    Added,
    Modified,
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedSystem {
    pub system_id: SystemId,
    pub action: PlannedSystemAction,
    pub reasons: Vec<ReconcileReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannedSystemAction {
    Rebuild,
    Remove,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ReconcileReason {
    MissingCatalog,
    SourceChanged,
    SharedClaimChanged,
    MetadataChanged,
    SemanticVersionChanged,
    ExplicitRequest,
    RemovedSystem,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEvent {
    pub run_id: RunId,
    pub intended_generation: u64,
    pub kind: CatalogEventKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogEventKind {
    PlanReady { systems: usize },
    SystemQueued { system_id: SystemId },
    SystemBuildStarted { system_id: SystemId },
    SystemReady { summary: SystemSummary },
    SystemPersisted { summary: SystemSummary },
    SystemFailed { failure: SystemFailure },
    ManifestPublished { generation: u64 },
    Done { outcome: RunOutcome },
    Failure { error: CatalogError },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RunId(String);

impl RunId {
    pub fn new(value: impl Into<String>) -> Result<Self, CatalogError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
            return Err(CatalogError::configuration("invalid run ID"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    Unchanged {
        generation: Option<u64>,
    },
    Complete {
        generation: u64,
        systems: usize,
    },
    Partial {
        generation: u64,
        systems: usize,
        failures: Vec<SystemFailure>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemFailure {
    pub system_id: SystemId,
    pub stage: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemSummary {
    pub system_id: SystemId,
    pub display_title: String,
    pub section: String,
    pub family: String,
    pub order: u32,
    pub generation: u64,
    pub games: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogRegistry {
    generation: u64,
    systems: Vec<SystemSummary>,
}

impl CatalogRegistry {
    pub(crate) fn new(generation: u64, systems: Vec<SystemSummary>) -> Self {
        Self {
            generation,
            systems,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn systems(&self) -> &[SystemSummary] {
        &self.systems
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemCatalog {
    summary: SystemSummary,
    games: Vec<CatalogGame>,
}

impl SystemCatalog {
    pub(crate) fn new(summary: SystemSummary, games: Vec<CatalogGame>) -> Self {
        Self { summary, games }
    }

    pub fn summary(&self) -> &SystemSummary {
        &self.summary
    }

    pub fn games(&self) -> &[CatalogGame] {
        &self.games
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogGame {
    pub stable_key: String,
    pub title: String,
    pub launch_ref: String,
    pub preview_archive_path: String,
    pub preview_asset_key: String,
    pub has_preview: bool,
    pub year: Option<u16>,
    pub manufacturer: String,
    pub category: String,
    pub players: Option<u8>,
    pub control: String,
    pub is_new: bool,
    pub launch_plan: Option<CatalogLaunchPlan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogLaunchPlan {
    pub launch_ref: String,
    pub title: String,
    pub system_id: String,
    pub core_path: String,
    pub payload_path: String,
    pub mount_kind: String,
    pub mount_index: u8,
    pub delay_secs: u8,
}

pub trait CatalogEngine {
    fn plan(&mut self) -> Result<ReconcilePlan, CatalogError>;

    fn execute(
        &mut self,
        command: CatalogCommand,
        emit: &mut dyn FnMut(CatalogEvent),
    ) -> Result<RunOutcome, CatalogError>;
}

pub trait CatalogReader {
    fn open_registry(&self) -> Result<CatalogRegistry, CatalogError>;

    fn open_system(&self, system_id: &SystemId) -> Result<SystemCatalog, CatalogError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogError {
    stage: &'static str,
    message: String,
}

impl CatalogError {
    pub fn new(stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }

    pub fn configuration(message: impl Into<String>) -> Self {
        Self::new("configuration", message)
    }

    pub fn stage(&self) -> &'static str {
        self.stage
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.stage, self.message)
    }
}

impl Error for CatalogError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_configuration_needs_roots_and_bounds() {
        assert_eq!(
            CatalogConfig::new(PathBuf::from("catalog"), vec![], 1)
                .unwrap_err()
                .message(),
            "source roots are empty"
        );
        assert_eq!(
            CatalogConfig::new(PathBuf::from("catalog"), vec![PathBuf::from("games")], 0)
                .unwrap_err()
                .message(),
            "navigation decoded-byte limit is zero"
        );
        let config =
            CatalogConfig::new(PathBuf::from("catalog"), vec![PathBuf::from("games")], 1024)
                .unwrap();
        assert_eq!(config.storage_root(), Path::new("catalog"));
        assert_eq!(config.source_roots(), &[PathBuf::from("games")]);
        assert_eq!(config.max_navigation_decoded_bytes(), 1024);
    }

    #[test]
    fn every_event_carries_run_and_generation_correlation() {
        let event = CatalogEvent {
            run_id: RunId::new("fixture-run").unwrap(),
            intended_generation: 7,
            kind: CatalogEventKind::SystemQueued {
                system_id: SystemId::parse("snes").unwrap(),
            },
        };
        assert_eq!(event.run_id.as_str(), "fixture-run");
        assert_eq!(event.intended_generation, 7);
    }

    #[test]
    fn unchanged_plan_has_no_system_work() {
        assert!(
            ReconcilePlan {
                current_generation: Some(4),
                intended_generation: 4,
                scan_units: Vec::new(),
                systems: Vec::new(),
                global_rebuild: false,
                manifest_only: false,
            }
            .is_unchanged()
        );
    }
}
