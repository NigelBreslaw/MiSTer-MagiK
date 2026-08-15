// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_catalog::builder_protocol::CatalogBuilderEvent;
use mister_magik_catalog::builder_service::{BuilderOperation, run_with_paths};
use mister_magik_catalog::device_layout::CatalogPaths;
use std::io::{self, Write};

fn main() {
    let paths = CatalogPaths::capture_process();
    // SAFETY: this is the first operation in the single-threaded process
    // entrypoint, before catalog workers or output handles are created.
    unsafe { std::env::set_var("MISTER_CATALOG_PROTOCOL_STDOUT", "1") };
    let mut args = std::env::args();
    let _binary = args.next();
    let operation = match args.next().as_deref() {
        Some("check") => BuilderOperation::Check,
        Some("build") => BuilderOperation::Build,
        Some("rebuild") => BuilderOperation::Rebuild,
        Some("rebuild-all") => BuilderOperation::RebuildAll,
        Some("fresh-build") => BuilderOperation::FreshBuild,
        Some("-h" | "--help") => {
            let _ = writeln!(
                io::stdout().lock(),
                "usage: mister-magik-catalog-builder check|build|rebuild|rebuild-all|fresh-build"
            );
            return;
        }
        _ => {
            let _ = writeln!(
                io::stderr().lock(),
                "usage: mister-magik-catalog-builder check|build|rebuild|rebuild-all|fresh-build"
            );
            std::process::exit(2);
        }
    };
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let result = run_with_paths(operation, &paths, |event: CatalogBuilderEvent| {
        serde_json::to_writer(&mut output, &event).expect("write catalog builder event");
        output
            .write_all(b"\n")
            .expect("terminate catalog builder event");
        output.flush().expect("flush catalog builder event");
    });
    if result.is_err() {
        std::process::exit(1);
    }
}
