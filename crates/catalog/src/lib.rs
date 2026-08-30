// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Catalog, library scan, and preview-loading logic for MiSTer MagiK.

pub mod arcade_catalog;
#[cfg(feature = "builder")]
#[path = "bin/arcade_catalog_prototype/model.rs"]
pub mod arcade_catalog_prototype_model;
mod arcade_rom_inventory;
pub mod archive_member;
mod atomic_publish;
mod bounded_lz4;
pub mod build_progress;
pub mod builder_protocol;
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
pub mod catalog_lease;
mod catalog_load_metrics;
pub mod catalog_navigation;
pub mod catalog_progress;
mod catalog_projection;
mod catalog_scan;
pub use catalog_scan::catalog_corpus_inventory_tsv;
pub mod catalog_stamp;
pub mod catalog_state;
pub mod catalog_store;
pub mod catalog_summary;
mod cooperative_work;
mod core_audit;
pub mod device_layout;
mod fallible_log;
#[cfg(feature = "builder")]
pub mod fast_catalog_refresh;
#[cfg(feature = "builder")]
pub mod fast_catalog_sources;
pub mod fast_five_catalog;
pub mod fs_fault;
mod game_discovery;
#[cfg(feature = "builder")]
pub mod generic_system_catalog;
#[cfg(feature = "builder")]
pub mod incremental_inputs;
pub mod launch_profiles;
pub mod lazy_sharded_reader;
pub mod legacy_user_state_import;
pub mod library_bench;
mod library_cli;
pub mod library_db;
mod library_indexer;
mod machine_family;
#[cfg(feature = "builder")]
mod machine_family_projection;
pub mod media_identity;
mod media_metadata;
pub mod mra_header {
    //! Bounded MRA header parsing shared by release tooling and the device scanner.

    pub use crate::media_metadata::{
        MraInspection, MraMetadata as MraHeader, PrimaryRomRequirement, RomNamespace,
    };

    /// Parse only the descriptive MRA prefix used by catalog discovery.
    pub fn parse(bytes: &[u8]) -> Option<MraHeader> {
        crate::media_metadata::parse_mra_metadata_bytes(bytes)
    }

    /// Inspect the complete MRA for catalog metadata and its primary ROM archive.
    pub fn inspect(bytes: &[u8]) -> Result<MraInspection, String> {
        crate::media_metadata::inspect_mra_bytes(bytes)
    }
}
pub mod arcade_updater_index;
#[cfg(feature = "builder")]
pub mod multi_system_projection;
mod namespace_walk;
pub mod navpack;
pub mod persisted_search;
pub mod prepared_bundle_helper;
pub mod prepared_collections;
mod prepared_release_manifest;
mod preview_archive;
pub mod preview_availability;
pub mod preview_worker;
pub mod runtime_thread;
pub mod scanner_cache;
pub mod shard_registry;
pub mod sharded_catalog;
mod software_identity;
#[cfg(feature = "builder")]
pub use software_identity::rom_identity_benchmark_report;
mod sqlite_catalog;
pub mod sqlite_inspect;
#[cfg(feature = "builder")]
pub mod synthetic_fixture;
pub mod system_shard;
#[cfg(test)]
mod test_support;
pub mod user_state;
pub mod work_coordinator;

pub(crate) mod pmu_phase {
    pub const WALK: &str = "catalog.walk";
    pub const WALK_EXECUTION: &str = "catalog.scan.execution-walk";
    pub const WALK_RESUME_VALIDATION: &str = "catalog.scan.resume-validation";
    #[cfg(feature = "builder")]
    pub const SHARD_NAVIGATION: &str = "catalog.shard.navigation";
    #[cfg(feature = "builder")]
    pub const SHARD_SQLITE_SCHEMA: &str = "catalog.shard.sqlite-schema";
    #[cfg(feature = "builder")]
    pub const SHARD_GAMES: &str = "catalog.shard.games";
    #[cfg(feature = "builder")]
    pub const SHARD_SEARCH_INDEX: &str = "catalog.shard.search-index";
    #[cfg(feature = "builder")]
    pub const SEARCH_ROWS: &str = "catalog.shard.search.rows";
    #[cfg(feature = "builder")]
    pub const SEARCH_AUTOCOMPLETE_INSERT: &str = "catalog.shard.search.autocomplete-insert";
    #[cfg(feature = "builder")]
    pub const SEARCH_AUTOCOMPLETE_SORT: &str = "catalog.shard.search.autocomplete-sort";
    #[cfg(feature = "builder")]
    pub const SEARCH_OPTIMIZE: &str = "catalog.shard.search.optimize";
    #[cfg(feature = "builder")]
    pub const SEARCH_INTEGRITY: &str = "catalog.shard.search.integrity";
    #[cfg(feature = "builder")]
    pub const SHARD_COMMIT: &str = "catalog.shard.commit";
    #[cfg(all(feature = "builder", any(test, target_os = "linux")))]
    pub const SHARD_ALLOCATOR_TRIM: &str = "catalog.shard.allocator-trim";
    #[cfg(feature = "builder")]
    pub const SHARD_VALIDATE: &str = "catalog.shard.validate";
    #[cfg(all(test, feature = "builder"))]
    pub const PUBLISH_COPY_HASH: &str = "catalog.publish.copy-hash";

    #[cfg(all(test, feature = "builder"))]
    mod tests {
        use super::*;

        #[test]
        fn catalog_pmu_phase_names_are_stable() {
            assert_eq!(
                [
                    WALK,
                    WALK_EXECUTION,
                    WALK_RESUME_VALIDATION,
                    SHARD_NAVIGATION,
                    SHARD_SQLITE_SCHEMA,
                    SHARD_GAMES,
                    SHARD_SEARCH_INDEX,
                    SEARCH_ROWS,
                    SEARCH_AUTOCOMPLETE_INSERT,
                    SEARCH_AUTOCOMPLETE_SORT,
                    SEARCH_OPTIMIZE,
                    SEARCH_INTEGRITY,
                    SHARD_COMMIT,
                    SHARD_ALLOCATOR_TRIM,
                    SHARD_VALIDATE,
                    PUBLISH_COPY_HASH,
                ],
                [
                    "catalog.walk",
                    "catalog.scan.execution-walk",
                    "catalog.scan.resume-validation",
                    "catalog.shard.navigation",
                    "catalog.shard.sqlite-schema",
                    "catalog.shard.games",
                    "catalog.shard.search-index",
                    "catalog.shard.search.rows",
                    "catalog.shard.search.autocomplete-insert",
                    "catalog.shard.search.autocomplete-sort",
                    "catalog.shard.search.optimize",
                    "catalog.shard.search.integrity",
                    "catalog.shard.commit",
                    "catalog.shard.allocator-trim",
                    "catalog.shard.validate",
                    "catalog.publish.copy-hash",
                ]
            );
        }
    }
}
