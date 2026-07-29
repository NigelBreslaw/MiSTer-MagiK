// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Explicitly rooted Catalog V3 builds for host tools.
//!
//! The production builder discovers its paths from the MiSTer environment.
//! Host previews instead pass physical source roots and a separate writable
//! storage root. The scan facts are remapped into the canonical `/media/fat`
//! namespace before the normal Catalog V3 projection is published.

use crate::arcade_catalog::ArcadeCatalog;
use crate::catalog_config::PathMapRule;
use crate::library_db;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct PortableCatalogBuild {
    pub catalog: ArcadeCatalog,
    pub generation: u64,
    pub systems: usize,
    pub games: usize,
}

pub fn publish_portable_catalog(
    source_roots: Vec<PathBuf>,
    source_namespace_root: &Path,
    canonical_namespace_root: &Path,
    arcade_root: &Path,
    storage_root: &Path,
    progress: &mut dyn FnMut(&str, &str),
) -> Result<PortableCatalogBuild, String> {
    if source_roots.is_empty() {
        return Err("portable catalog scan has no existing source roots".into());
    }
    progress("Building game library", "Scanning mounted card…");
    let artifact = library_db::scan_library_ram_foreground_with_roots(
        source_roots,
        Some(progress),
        None,
        false,
    )?;
    let artifact = library_db::apply_library_path_map_to_ram_artifact_with_rules(
        artifact,
        &[PathMapRule {
            from: source_namespace_root.to_string_lossy().into_owned(),
            to: canonical_namespace_root.to_string_lossy().into_owned(),
        }],
    );
    let (prepared, catalog, _timing, scanner_cache) = artifact
        .complete_coverage_audit_and_catalog_foreground_with_progress(arcade_root, progress)?;
    let catalog_state = prepared.into_parts().0;
    let fingerprint = catalog_state.stamp.fingerprint_hex();

    progress("Building game library", "Publishing Mac catalog cache…");
    let published = crate::production_sharded_projection::publish_bound_production_projection(
        storage_root,
        &catalog,
        &fingerprint,
        crate::production_sharded_projection::production_registry_limits(),
    )
    .map_err(|error| format!("publish portable Catalog V3: {error}"))?;
    let scanner_cache_path = crate::scanner_cache::path_for_root(storage_root);
    crate::scanner_cache::stage(&scanner_cache_path, &scanner_cache)?.publish()?;
    crate::catalog_state::write(
        &crate::catalog_state::path_for_root(storage_root),
        &catalog_state,
    )?;
    progress("Building game library", "Catalog ready");
    Ok(PortableCatalogBuild {
        catalog,
        generation: published.generation,
        systems: published.systems,
        games: published.games,
    })
}
