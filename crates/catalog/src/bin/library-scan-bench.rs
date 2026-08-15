// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    let paths = mister_magik_catalog::device_layout::CatalogPaths::capture_process();
    mister_magik_catalog::library_db::run_scan_bench_with_paths(&paths);
}
