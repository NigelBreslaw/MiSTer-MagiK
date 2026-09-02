// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only host adapter for the same strict contract used on the MiSTer.
use mister_magik_platform_manifest_contract::{Layout, ValidationProfile, parse};
use std::{env, fs, process};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 4 || args[0] != "--layout" || args[2] != "--manifest" {
        return Err("usage: platform-manifest-check --layout public|dev --manifest PATH".into());
    }
    let layout = Layout::parse(&args[1])?;
    let manifest = parse(
        &fs::read_to_string(&args[3])?,
        layout,
        ValidationProfile::AgentStrict,
    )?;
    print!("{}", manifest.serialize()?);
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}
