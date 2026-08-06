// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Catalog, library scan, and preview-loading logic for MiSTer MagiK.

#[cfg(feature = "builder")]
mod arcade_bootstrap_index;
pub mod arcade_catalog;
pub mod archive_member;
mod atomic_publish;
mod bounded_lz4;
pub mod build_progress;
pub mod builder_protocol;
#[cfg(feature = "builder")]
pub mod builder_service;
#[cfg(feature = "builder")]
pub mod catalog_acceptance;
pub mod catalog_build;
pub mod catalog_build_record;
pub mod catalog_checkpoint;
pub mod catalog_classify;
pub mod catalog_config;
mod catalog_discovery;
pub mod catalog_domain;
pub mod catalog_format;
mod catalog_load_metrics;
pub mod catalog_navigation;
mod catalog_progress;
mod catalog_projection;
mod catalog_scan;
pub mod catalog_stamp;
pub mod catalog_state;
pub mod catalog_store;
pub mod catalog_summary;
#[cfg(feature = "builder")]
pub mod catalog_vertical_slice;
mod cooperative_work;
mod core_audit;
pub mod device_layout;
mod fallible_log;
pub mod fs_fault;
mod game_discovery;
#[cfg(feature = "builder")]
pub mod incremental_inputs;
pub mod launch_profiles;
pub mod lazy_sharded_reader;
pub mod library_bench;
mod library_cli;
pub mod library_db;
mod library_indexer;
pub mod media_identity;
mod media_metadata;
#[cfg(feature = "builder")]
pub mod multi_system_projection;
mod namespace_walk;
pub mod persisted_search;
#[cfg(feature = "builder")]
pub mod portable_catalog_builder;
pub mod prepared_collections;
mod preview_archive;
pub mod preview_worker;
#[cfg(feature = "builder")]
pub mod production_sharded_projection;
#[cfg(feature = "builder")]
pub mod progressive_scheduler;
#[cfg(feature = "builder")]
pub mod rebuild_benchmark;
#[cfg(feature = "builder")]
pub mod reconciliation_executor;
#[cfg(feature = "builder")]
pub mod reconciliation_planner;
pub mod runtime_thread;
pub mod scanner_cache;
pub mod shard_registry;
pub mod sharded_catalog;
mod software_identity;
mod sqlite_catalog;
pub mod sqlite_inspect;
#[cfg(feature = "builder")]
pub mod synthetic_fixture;
pub mod system_shard;
#[cfg(test)]
mod test_support;
pub mod work_coordinator;

pub(crate) mod pmu_phase {
    pub const WALK: &str = "catalog.walk";
    #[cfg(feature = "builder")]
    pub const SHARD_NAVIGATION: &str = "catalog.shard.navigation";
    #[cfg(feature = "builder")]
    pub const SHARD_SQLITE_SCHEMA: &str = "catalog.shard.sqlite-schema";
    #[cfg(feature = "builder")]
    pub const SHARD_GAMES: &str = "catalog.shard.games";
    #[cfg(feature = "builder")]
    pub const SHARD_SEARCH_INDEX: &str = "catalog.shard.search-index";
    #[cfg(feature = "builder")]
    pub const SHARD_COMMIT: &str = "catalog.shard.commit";
    #[cfg(feature = "builder")]
    pub const SHARD_VALIDATE: &str = "catalog.shard.validate";
    #[cfg(feature = "builder")]
    pub const PUBLISH_COPY_HASH: &str = "catalog.publish.copy-hash";

    #[cfg(all(test, feature = "builder"))]
    mod tests {
        use super::*;

        #[test]
        fn catalog_pmu_phase_names_are_stable() {
            assert_eq!(
                [
                    WALK,
                    SHARD_NAVIGATION,
                    SHARD_SQLITE_SCHEMA,
                    SHARD_GAMES,
                    SHARD_SEARCH_INDEX,
                    SHARD_COMMIT,
                    SHARD_VALIDATE,
                    PUBLISH_COPY_HASH,
                ],
                [
                    "catalog.walk",
                    "catalog.shard.navigation",
                    "catalog.shard.sqlite-schema",
                    "catalog.shard.games",
                    "catalog.shard.search-index",
                    "catalog.shard.commit",
                    "catalog.shard.validate",
                    "catalog.publish.copy-hash",
                ]
            );
        }
    }
}
