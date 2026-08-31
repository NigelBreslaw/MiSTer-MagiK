// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Build the compact runtime metadata artifact from the CI's full SQLite
//! source databases.

use mister_magik_catalog::runtime_metadata::{build_from_sqlite, parity_report};
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "usage: runtime-metadata-builder --mame PATH --hbmame PATH --output PATH [--report PATH]"
    );
    std::process::exit(2);
}

fn value(args: &mut std::env::Args, option: &str) -> PathBuf {
    match args.next() {
        Some(value) => PathBuf::from(value),
        None => {
            eprintln!("missing value for {option}");
            usage();
        }
    }
}

fn main() {
    let mut args = std::env::args();
    let _program = args.next();
    let mut mame = None;
    let mut hbmame = None;
    let mut output = None;
    let mut report = None;
    while let Some(option) = args.next() {
        match option.as_str() {
            "--mame" => mame = Some(value(&mut args, "--mame")),
            "--hbmame" => hbmame = Some(value(&mut args, "--hbmame")),
            "--output" => output = Some(value(&mut args, "--output")),
            "--report" => report = Some(value(&mut args, "--report")),
            "-h" | "--help" => usage(),
            _ => {
                eprintln!("unknown option {option}");
                usage();
            }
        }
    }
    let (Some(mame), Some(hbmame), Some(output)) = (mame, hbmame, output) else {
        usage();
    };
    match build_from_sqlite(&mame, &hbmame, &output) {
        Ok(status) => println!(
            "{{\"format\":\"{}\",\"shards\":{},\"bytes\":{}}}",
            status.format, status.shard_count, status.file_len
        ),
        Err(error) => {
            eprintln!("runtime metadata build failed: {error}");
            std::process::exit(1);
        }
    }
    if let Some(report) = report {
        match parity_report(&output, &mame, &hbmame).and_then(|contents| {
            std::fs::write(&report, contents).map_err(|error| error.to_string())
        }) {
            Ok(()) => {}
            Err(error) => {
                eprintln!("runtime metadata parity failed: {error}");
                std::process::exit(1);
            }
        }
    }
}
