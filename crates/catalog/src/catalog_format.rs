// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Persisted catalog format identity and bounded compatibility classification.

use serde::{Deserialize, Serialize};

pub const BINDING_SCHEMA_VERSION: u32 = 1;
pub const CATALOG_STATE_SCHEMA_VERSION: u32 = 1;
pub const SCANNER_CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CatalogFormatDescriptor {
    pub canonical_schema_version: u32,
    pub catalog_build_version: u32,
    pub manifest_schema_version: u32,
    pub shard_schema_version: u32,
    pub navigation_schema_version: u32,
    pub binding_schema_version: u32,
    pub catalog_state_schema_version: u32,
    pub scanner_cache_schema_version: u32,
    pub builder_protocol_version: u32,
    pub projection_contract: String,
}

impl CatalogFormatDescriptor {
    #[must_use]
    pub fn current() -> Self {
        Self {
            canonical_schema_version: crate::catalog_config::SCHEMA_VERSION,
            catalog_build_version: crate::catalog_config::CATALOG_BUILD_VERSION,
            manifest_schema_version: crate::sharded_catalog::MANIFEST_SCHEMA_VERSION,
            shard_schema_version: crate::sharded_catalog::SHARD_SCHEMA_VERSION,
            navigation_schema_version: crate::sharded_catalog::NAVIGATION_SCHEMA_VERSION,
            binding_schema_version: BINDING_SCHEMA_VERSION,
            catalog_state_schema_version: CATALOG_STATE_SCHEMA_VERSION,
            scanner_cache_schema_version: SCANNER_CACHE_SCHEMA_VERSION,
            builder_protocol_version: crate::builder_protocol::CATALOG_BUILDER_PROTOCOL_VERSION,
            projection_contract: crate::sharded_catalog::PRODUCTION_PROJECTION_CONTRACT.to_string(),
        }
    }

    #[must_use]
    pub fn alpha_predecessor() -> Self {
        Self {
            canonical_schema_version: 66,
            catalog_build_version: 15,
            manifest_schema_version: 1,
            shard_schema_version: 3,
            navigation_schema_version: 1,
            binding_schema_version: 1,
            catalog_state_schema_version: 1,
            scanner_cache_schema_version: 1,
            builder_protocol_version: 1,
            projection_contract: "rich-game-v2".to_string(),
        }
    }

    #[must_use]
    pub fn navigation_index_predecessor() -> Self {
        let mut predecessor = Self::navpack_predecessor();
        predecessor.navigation_schema_version = 2;
        predecessor
    }

    #[must_use]
    pub fn navpack_predecessor() -> Self {
        let mut predecessor = Self::entry_prelude_predecessor();
        predecessor.shard_schema_version = 4;
        predecessor
    }

    #[must_use]
    pub fn entry_prelude_predecessor() -> Self {
        let mut predecessor = Self::current();
        predecessor.catalog_build_version = 16;
        predecessor
    }

    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "canonical={}/{} shard={} navigation={} projection={}",
            self.canonical_schema_version,
            self.catalog_build_version,
            self.shard_schema_version,
            self.navigation_schema_version,
            self.projection_contract
        )
    }

    #[must_use]
    pub fn from_legacy_stamp_lines(lines: &[String]) -> Option<Self> {
        let schema = stamp_value(lines, "schema")?;
        let build = stamp_value(lines, "catalog-build")?;
        if schema == crate::catalog_config::SCHEMA_VERSION
            && build == crate::catalog_config::CATALOG_BUILD_VERSION
        {
            Some(Self::current())
        } else if schema == 66 && build == 15 {
            Some(Self::alpha_predecessor())
        } else {
            let mut descriptor = Self::current();
            descriptor.canonical_schema_version = schema;
            descriptor.catalog_build_version = build;
            Some(descriptor)
        }
    }
}

fn stamp_value(lines: &[String], name: &str) -> Option<u32> {
    lines.iter().find_map(|line| {
        let (key, value) = line.split_once('\t')?;
        (key == name).then(|| value.parse().ok()).flatten()
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogFormatStatus {
    Current,
    UpgradeRequired {
        installed: CatalogFormatDescriptor,
        required: CatalogFormatDescriptor,
    },
    UnsupportedFuture {
        installed: CatalogFormatDescriptor,
        required: CatalogFormatDescriptor,
    },
    Corrupt {
        installed: CatalogFormatDescriptor,
        required: CatalogFormatDescriptor,
    },
}

#[must_use]
pub fn classify(descriptor: &CatalogFormatDescriptor) -> CatalogFormatStatus {
    let current = CatalogFormatDescriptor::current();
    if descriptor == &current {
        return CatalogFormatStatus::Current;
    }
    if descriptor == &CatalogFormatDescriptor::alpha_predecessor() {
        return CatalogFormatStatus::UpgradeRequired {
            installed: descriptor.clone(),
            required: current,
        };
    }
    if descriptor == &CatalogFormatDescriptor::navigation_index_predecessor() {
        return CatalogFormatStatus::UpgradeRequired {
            installed: descriptor.clone(),
            required: current,
        };
    }
    if descriptor == &CatalogFormatDescriptor::navpack_predecessor() {
        return CatalogFormatStatus::UpgradeRequired {
            installed: descriptor.clone(),
            required: current,
        };
    }
    if descriptor == &CatalogFormatDescriptor::entry_prelude_predecessor() {
        return CatalogFormatStatus::UpgradeRequired {
            installed: descriptor.clone(),
            required: current,
        };
    }
    if has_future_component(descriptor, &current) {
        return CatalogFormatStatus::UnsupportedFuture {
            installed: descriptor.clone(),
            required: current,
        };
    }
    CatalogFormatStatus::Corrupt {
        installed: descriptor.clone(),
        required: current,
    }
}

fn has_future_component(
    installed: &CatalogFormatDescriptor,
    required: &CatalogFormatDescriptor,
) -> bool {
    installed.canonical_schema_version > required.canonical_schema_version
        || installed.catalog_build_version > required.catalog_build_version
        || installed.manifest_schema_version > required.manifest_schema_version
        || installed.shard_schema_version > required.shard_schema_version
        || installed.navigation_schema_version > required.navigation_schema_version
        || installed.binding_schema_version > required.binding_schema_version
        || installed.catalog_state_schema_version > required.catalog_state_schema_version
        || installed.scanner_cache_schema_version > required.scanner_cache_schema_version
        || installed.builder_protocol_version > required.builder_protocol_version
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_current_and_alpha_predecessor_are_distinct() {
        assert_eq!(
            classify(&CatalogFormatDescriptor::current()),
            CatalogFormatStatus::Current
        );
        assert!(matches!(
            classify(&CatalogFormatDescriptor::alpha_predecessor()),
            CatalogFormatStatus::UpgradeRequired { .. }
        ));
        assert!(matches!(
            classify(&CatalogFormatDescriptor::navigation_index_predecessor()),
            CatalogFormatStatus::UpgradeRequired { .. }
        ));
        assert!(matches!(
            classify(&CatalogFormatDescriptor::navpack_predecessor()),
            CatalogFormatStatus::UpgradeRequired { .. }
        ));
        assert!(matches!(
            classify(&CatalogFormatDescriptor::entry_prelude_predecessor()),
            CatalogFormatStatus::UpgradeRequired { .. }
        ));
        assert_eq!(
            CatalogFormatDescriptor::entry_prelude_predecessor().catalog_build_version,
            16
        );
    }

    #[test]
    fn future_and_mixed_descriptors_fail_closed() {
        let mut future = CatalogFormatDescriptor::current();
        future.navigation_schema_version += 1;
        assert!(matches!(
            classify(&future),
            CatalogFormatStatus::UnsupportedFuture { .. }
        ));

        let mut mixed = CatalogFormatDescriptor::alpha_predecessor();
        mixed.navigation_schema_version =
            CatalogFormatDescriptor::current().navigation_schema_version;
        assert!(matches!(
            classify(&mixed),
            CatalogFormatStatus::Corrupt { .. }
        ));
    }

    #[test]
    fn legacy_stamp_recovers_the_known_predecessor_only() {
        let lines = vec!["schema\t66".to_string(), "catalog-build\t15".to_string()];
        assert_eq!(
            CatalogFormatDescriptor::from_legacy_stamp_lines(&lines),
            Some(CatalogFormatDescriptor::alpha_predecessor())
        );
    }
}
