// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic cold catalog build workload for Hotpath.

use mister_magik_catalog::fast_catalog_refresh::build_fresh_catalog;
use mister_magik_catalog::synthetic_fixture::{SyntheticFixtureSpec, generate_synthetic_fixture};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Serialize)]
struct ProfileSummary {
    schema: &'static str,
    profile_root: PathBuf,
    fixture: mister_magik_catalog::synthetic_fixture::SyntheticFixtureSummary,
    build: mister_magik_catalog::fast_catalog_refresh::FastCatalogFreshBuildReport,
    mcp_hold_ms: u64,
}

struct Options {
    root: PathBuf,
    spec: SyntheticFixtureSpec,
    mcp_hold_ms: u64,
}

#[hotpath::main]
fn main() {
    if let Err(error) = run() {
        eprintln!("catalog-hotpath-profile: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let options = parse_options(std::env::args().skip(1).collect())?;
    if options.root.exists() {
        return Err(format!(
            "profile root must not exist so the build is cold: {}",
            options.root.display()
        ));
    }
    std::fs::create_dir(&options.root)
        .map_err(|error| format!("create {}: {error}", options.root.display()))?;
    let fixture_root = options.root.join("fixture");
    let catalog_root = options.root.join("catalog");
    let fixture = generate_synthetic_fixture(&fixture_root, &options.spec)
        .map_err(|error| format!("generate synthetic fixture: {error}"))?;
    let build = build_fresh_catalog(&fixture_root, &catalog_root)?;
    let summary = ProfileSummary {
        schema: "mister-magik-catalog-hotpath-profile-v1",
        profile_root: options.root,
        fixture,
        build,
        mcp_hold_ms: options.mcp_hold_ms,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&summary).map_err(|error| error.to_string())?
    );
    if options.mcp_hold_ms > 0 {
        eprintln!(
            "catalog-hotpath-profile: holding the Hotpath MCP server open for {} ms",
            options.mcp_hold_ms
        );
        std::thread::sleep(Duration::from_millis(options.mcp_hold_ms));
    }
    Ok(())
}

fn parse_options(arguments: Vec<String>) -> Result<Options, String> {
    let mut root = None;
    let mut spec = SyntheticFixtureSpec::default();
    let mut mcp_hold_ms = 0;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("{argument} requires a value\n{}", usage()))?;
        match argument.as_str() {
            "--root" => root = Some(PathBuf::from(value)),
            "--arcade-games" => spec.arcade_games = parse_usize(&argument, &value)?,
            "--small-system-games" => {
                spec.small_system_games = parse_usize(&argument, &value)?;
            }
            "--large-system-games" => {
                spec.large_system_games = parse_usize(&argument, &value)?;
            }
            "--large-system-depth" => {
                spec.large_system_depth = parse_usize(&argument, &value)?;
            }
            "--mcp-hold-ms" => mcp_hold_ms = parse_u64(&argument, &value)?,
            _ => return Err(format!("unknown option {argument}\n{}", usage())),
        }
    }
    Ok(Options {
        root: root.ok_or_else(usage)?,
        spec,
        mcp_hold_ms,
    })
}

fn parse_usize(option: &str, value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|error| format!("invalid {option} value {value:?}: {error}"))
}

fn parse_u64(option: &str, value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|error| format!("invalid {option} value {value:?}: {error}"))
}

fn usage() -> String {
    "usage: catalog-hotpath-profile --root PATH [--arcade-games N] [--small-system-games N] [--large-system-games N] [--large-system-depth N] [--mcp-hold-ms N]".to_owned()
}
