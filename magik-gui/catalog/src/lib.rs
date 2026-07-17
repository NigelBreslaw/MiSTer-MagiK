// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Catalog, library scan, and preview-loading logic for MiSTer MagiK.

pub mod arcade_catalog;
mod atomic_publish;
mod bounded_lz4;
pub mod builder_protocol;
#[cfg(feature = "builder")]
pub mod builder_service;
pub mod catalog_build;
pub mod catalog_build_record;
pub mod catalog_checkpoint;
pub mod catalog_classify;
pub mod catalog_config;
mod catalog_discovery;
mod catalog_load_metrics;
pub mod catalog_navigation;
mod catalog_progress;
mod catalog_projection;
mod catalog_scan;
pub mod catalog_stamp;
pub mod catalog_store;
pub mod catalog_summary;
mod core_audit;
pub mod device_layout;
mod fallible_log;
pub mod fs_fault;
mod game_discovery;
pub mod launch_profiles;
pub mod library_bench;
mod library_cli;
pub mod library_db;
mod library_indexer;
pub mod media_identity;
mod media_metadata;
mod namespace_walk;
pub mod prepared_collections;
mod preview_archive;
pub mod preview_worker;
pub mod runtime_thread;
mod software_identity;
mod sqlite_catalog;
pub mod sqlite_inspect;
#[cfg(feature = "builder")]
pub mod synthetic_fixture;
#[cfg(test)]
mod test_support;
pub mod work_coordinator;
