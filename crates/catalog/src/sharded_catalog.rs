// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared types for the production catalog registry and on-demand readers.

use crate::catalog_classify::SystemId;
use std::error::Error;
use std::fmt;

pub const SHARD_SCHEMA_VERSION: u32 = 6;
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const NAVIGATION_SCHEMA_VERSION: u32 = 3;
pub const PRODUCTION_PROJECTION_CONTRACT: &str = "rich-game-v2";

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
