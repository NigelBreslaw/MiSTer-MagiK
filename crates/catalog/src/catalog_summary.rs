// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compact catalog summary projection for warm launcher startup.

use crate::arcade_catalog::{ArcadeCatalog, ArcadeGameEntry, MENU_ARCADE_SYSTEM_ID, PlatformKind};
use crate::catalog_config::{CATALOG_BUILD_VERSION, SCHEMA_VERSION};
use crate::catalog_load_metrics;
use crate::catalog_stamp::CatalogStamp;
use crate::media_identity;
use crate::preview_worker;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const CATALOG_SUMMARY_SCHEMA_VERSION: u32 = 4;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSummaryProjection {
    pub schema: u32,
    pub catalog_schema_version: u32,
    pub catalog_build_version: u32,
    pub catalog_generation: String,
    pub catalog_stamp_fingerprint: String,
    pub catalog_stamp_lines: Vec<String>,
    pub total_game_count: usize,
    pub systems: Vec<CatalogSummarySystem>,
    pub hot_games: Vec<CatalogSummaryGame>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSummarySystem {
    pub id: String,
    pub title: String,
    pub count: usize,
    #[serde(default)]
    pub platform_kind: PlatformKind,
    pub supported_media: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSummaryGame {
    pub title: String,
    pub launch_ref: String,
    pub preview_asset_key: String,
    pub has_preview: bool,
    pub system_id: String,
    pub year: Option<u16>,
    pub manufacturer: String,
    pub players: Option<u8>,
    pub control: String,
    pub is_new: bool,
}

pub fn summary_path_for_sqlite(sqlite_path: &Path) -> PathBuf {
    sqlite_path.with_extension("summary.json")
}

#[cfg(test)]
pub(crate) fn write_catalog_summary_for_catalog(
    sqlite_path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &CatalogStamp,
) -> Result<(), String> {
    let summary = CatalogSummaryProjection::from_catalog(catalog, stamp);
    write_catalog_summary_projection(sqlite_path, &summary)
}

pub(crate) fn write_catalog_summary_projection(
    sqlite_path: &Path,
    summary: &CatalogSummaryProjection,
) -> Result<(), String> {
    write_catalog_summary(&summary_path_for_sqlite(sqlite_path), summary)
}

pub fn read_catalog_summary(
    summary_path: &Path,
) -> Result<Option<CatalogSummaryProjection>, String> {
    catalog_load_metrics::record_summary_read();
    let text = match std::fs::read_to_string(summary_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "read catalog summary {}: {e}",
                summary_path.display()
            ));
        }
    };
    let summary: CatalogSummaryProjection = serde_json::from_str(&text)
        .map_err(|e| format!("parse catalog summary {}: {e}", summary_path.display()))?;
    if summary.schema != CATALOG_SUMMARY_SCHEMA_VERSION
        || summary.catalog_schema_version != SCHEMA_VERSION
        || summary.catalog_build_version != CATALOG_BUILD_VERSION
    {
        return Ok(None);
    }
    Ok(Some(summary))
}

impl CatalogSummaryProjection {
    pub fn from_catalog(catalog: &ArcadeCatalog, stamp: &CatalogStamp) -> Self {
        let stamp_fingerprint = stamp.fingerprint_hex();
        Self {
            schema: CATALOG_SUMMARY_SCHEMA_VERSION,
            catalog_schema_version: SCHEMA_VERSION,
            catalog_build_version: CATALOG_BUILD_VERSION,
            catalog_generation: stamp_fingerprint.clone(),
            catalog_stamp_fingerprint: stamp_fingerprint,
            catalog_stamp_lines: stamp.lines().to_vec(),
            total_game_count: catalog.games.len(),
            systems: catalog
                .systems
                .iter()
                .map(|system| CatalogSummarySystem {
                    id: system.id.clone(),
                    title: system.title.clone(),
                    count: system.count,
                    platform_kind: catalog.platform_kind(&system.id),
                    supported_media: supported_media_for_system(&system.id),
                })
                .collect(),
            hot_games: catalog
                .system_game_view(MENU_ARCADE_SYSTEM_ID)
                .iter()
                .map(CatalogSummaryGame::from)
                .collect(),
        }
    }

    pub fn platform_kinds(&self) -> HashMap<String, PlatformKind> {
        self.systems
            .iter()
            .map(|system| (system.id.clone(), system.platform_kind))
            .collect()
    }
}

impl From<&ArcadeGameEntry> for CatalogSummaryGame {
    fn from(game: &ArcadeGameEntry) -> Self {
        Self {
            title: game.title.to_string(),
            launch_ref: game.mra_path.to_string(),
            preview_asset_key: game.preview_asset_key.to_string(),
            has_preview: game.has_preview,
            system_id: game.system_id.to_string(),
            year: game.year,
            manufacturer: game.manufacturer.to_string(),
            players: game.players,
            control: game.control.to_string(),
            is_new: game.is_new,
        }
    }
}

impl From<&CatalogSummaryGame> for ArcadeGameEntry {
    fn from(game: &CatalogSummaryGame) -> Self {
        let system_id: Arc<str> = Arc::from(game.system_id.as_str());
        let preview_archive_path: Arc<str> = if game.preview_asset_key.is_empty() {
            Arc::from("")
        } else {
            Arc::from(preview_worker::preview_archive_path_for_system(
                &game.system_id,
            ))
        };
        Self {
            title: Arc::from(game.title.as_str()),
            mra_path: Arc::from(game.launch_ref.as_str()),
            preview_archive_path,
            preview_asset_key: Arc::from(game.preview_asset_key.as_str()),
            has_preview: game.has_preview,
            system_id,
            year: game.year,
            manufacturer: Arc::from(game.manufacturer.as_str()),
            players: game.players,
            control: Arc::from(game.control.as_str()),
            is_new: game.is_new,
        }
    }
}

fn supported_media_for_system(system_id: &str) -> Vec<String> {
    if media_identity::is_supported_screenshot_pack_id(system_id) {
        vec!["screenshots".to_string()]
    } else {
        Vec::new()
    }
}

fn write_catalog_summary(
    summary_path: &Path,
    summary: &CatalogSummaryProjection,
) -> Result<(), String> {
    let bytes =
        serde_json::to_vec(summary).map_err(|e| format!("serialize catalog summary: {e}"))?;
    write_bytes_atomically(summary_path, &bytes)
}

fn write_bytes_atomically(final_path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_bytes_atomically_with(final_path, |file| file.write_all(bytes))
}

fn write_bytes_atomically_with(
    final_path: &Path,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> Result<(), String> {
    crate::atomic_publish::write_atomically(
        final_path,
        "catalog summary",
        "catalog.summary.json",
        Some("catalog.summary"),
        write,
    )
}

#[cfg(test)]
fn summary_temp_path_for(final_path: &Path) -> PathBuf {
    crate::atomic_publish::temp_path(final_path, "catalog.summary.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_failure_preserves_existing_summary() {
        let dir =
            std::env::temp_dir().join(format!("mister-magik-summary-{}-{}", std::process::id(), 1));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let final_path = dir.join("library.summary.json");
        std::fs::write(&final_path, b"old").expect("write old summary");

        let err = write_bytes_atomically_with(&final_path, |file| {
            file.write_all(b"partial")?;
            Err(std::io::Error::other("simulated write failure"))
        })
        .expect_err("simulated failure");

        assert!(err.contains("simulated write failure"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&final_path).expect("read old summary"),
            "old"
        );
        assert!(!summary_temp_path_for(&final_path).exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
