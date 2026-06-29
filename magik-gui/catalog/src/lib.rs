//! Catalog, library scan, and preview-loading logic for MiSTer MagiK.

pub mod arcade_catalog;
pub mod catalog_build;
pub mod catalog_classify;
pub mod catalog_config;
pub mod catalog_navigation;
mod catalog_projection;
mod catalog_load_metrics;
mod catalog_progress;
mod catalog_scan;
pub mod catalog_stamp;
pub mod catalog_store;
pub mod catalog_summary;
mod core_audit;
mod game_discovery;
pub mod launch_profiles;
pub mod library_bench;
mod library_cli;
pub mod library_db;
mod library_indexer;
pub mod media_identity;
mod media_metadata;
pub mod preview_worker;
pub mod runtime_thread;
mod software_identity;
mod sqlite_catalog;
#[cfg(test)]
mod test_support;
