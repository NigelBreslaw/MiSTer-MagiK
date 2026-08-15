// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    let paths = mister_magik_catalog::device_layout::CatalogPaths::capture_process();
    let archive_cache =
        mister_magik_catalog::catalog_config::ArchiveCacheConfig::capture_process(&paths);
    mister_magik_catalog::library_db::run_scan_bench_with_config(&paths, &archive_cache);
}
