//! Compact catalog summary projection for warm launcher startup.

use crate::arcade_catalog::ArcadeCatalog;
use crate::catalog_config::{CATALOG_BUILD_VERSION, SCHEMA_VERSION};
use crate::catalog_load_metrics;
use crate::catalog_stamp::CatalogStamp;
use crate::media_identity;
use crate::sqlite_catalog;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CATALOG_SUMMARY_SCHEMA_VERSION: u32 = 1;

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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSummarySystem {
    pub id: String,
    pub title: String,
    pub count: usize,
    pub supported_media: Vec<String>,
}

pub fn summary_path_for_sqlite(sqlite_path: &Path) -> PathBuf {
    sqlite_path.with_extension("summary.json")
}

pub(crate) fn write_catalog_summary_for_catalog(
    sqlite_path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &CatalogStamp,
) -> Result<(), String> {
    let summary = CatalogSummaryProjection::from_catalog(catalog, stamp);
    write_catalog_summary(&summary_path_for_sqlite(sqlite_path), &summary)
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
            ))
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
                    supported_media: supported_media_for_system(&system.id),
                })
                .collect(),
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
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create catalog summary dir {}: {e}", parent.display()))?;
    }
    let temp_path = summary_temp_path_for(final_path);
    let result = (|| -> Result<(), String> {
        let mut file = File::create(&temp_path)
            .map_err(|e| format!("create catalog summary temp {}: {e}", temp_path.display()))?;
        write(&mut file)
            .map_err(|e| format!("write catalog summary temp {}: {e}", temp_path.display()))?;
        file.sync_all()
            .map_err(|e| format!("sync catalog summary temp {}: {e}", temp_path.display()))?;
        drop(file);
        std::fs::rename(&temp_path, final_path).map_err(|e| {
            format!(
                "replace catalog summary {} from {}: {e}",
                final_path.display(),
                temp_path.display()
            )
        })?;
        sqlite_catalog::sync_parent_dir(final_path);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn summary_temp_path_for(final_path: &Path) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("catalog.summary.json");
    final_path.with_file_name(format!(".{file_name}.tmp"))
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
