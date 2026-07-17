// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_catalog::synthetic_fixture::{generate_synthetic_fixture, SyntheticFixtureSpec};
use std::path::PathBuf;

fn main() {
    if let Err(error) = run(std::env::args().skip(1)) {
        eprintln!("catalog-lab: {error}");
        std::process::exit(2);
    }
}

fn run(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let Some(command) = args.next() else {
        return Err(usage());
    };
    if command == "--help" || command == "-h" {
        println!("{}", usage());
        return Ok(());
    }
    if command != "fixture" {
        if command == "bootstrap-fixture" {
            return bootstrap_fixture(args);
        }
        return Err(format!("unknown command {command:?}\n{}", usage()));
    }
    let root = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("fixture needs an output path\n{}", usage()))?;
    let mut spec = SyntheticFixtureSpec::default();
    while let Some(option) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{option} needs a value"))?;
        let value = value
            .parse::<usize>()
            .map_err(|_| format!("invalid integer for {option}: {value:?}"))?;
        match option.as_str() {
            "--arcade-games" => spec.arcade_games = value,
            "--small-system-games" => spec.small_system_games = value,
            "--large-system-games" => spec.large_system_games = value,
            "--large-system-depth" => spec.large_system_depth = value,
            _ => return Err(format!("unknown fixture option {option:?}")),
        }
    }
    let summary = generate_synthetic_fixture(&root, &spec)
        .map_err(|error| format!("could not create {}: {error}", root.display()))?;
    println!(
        "catalog_lab_fixture_tsv\troot={}\tfiles={}\tarcade_games={}\tsmall_system_games={}\tlarge_system_games={}\tlarge_system_depth={}",
        root.display(),
        summary.files,
        summary.spec.arcade_games,
        summary.spec.small_system_games,
        summary.spec.large_system_games,
        summary.spec.large_system_depth
    );
    Ok(())
}

fn bootstrap_fixture(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    use mister_magik_catalog::catalog_classify::SystemId;
    use mister_magik_catalog::catalog_vertical_slice::bootstrap_fixture_system;
    use mister_magik_catalog::shard_registry::RegistryLimits;
    use mister_magik_catalog::sharded_catalog::CatalogConfig;
    use mister_magik_catalog::system_shard::SystemShardLimits;

    let source = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("bootstrap-fixture needs SOURCE\n{}", usage()))?;
    let storage = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("bootstrap-fixture needs STORAGE\n{}", usage()))?;
    let system = args
        .next()
        .ok_or_else(|| format!("bootstrap-fixture needs SYSTEM\n{}", usage()))?;
    if args.next().is_some() {
        return Err(format!(
            "bootstrap-fixture has unexpected arguments\n{}",
            usage()
        ));
    }
    let system_id = SystemId::parse(&system).map_err(|error| error.to_string())?;
    let config = CatalogConfig::new(storage, vec![source], 512 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    let outcome = bootstrap_fixture_system(
        &config,
        &system_id,
        RegistryLimits {
            max_manifest_bytes: 8 * 1024 * 1024,
            max_systems: 1024,
            shard: SystemShardLimits {
                max_sqlite_bytes: 8 * 1024 * 1024 * 1024,
                max_navigation_compressed_bytes: 512 * 1024 * 1024,
                max_navigation_decoded_bytes: config.max_navigation_decoded_bytes(),
                max_games: 2_000_000,
            },
        },
    )
    .map_err(|error| error.to_string())?;
    println!(
        "catalog_lab_bootstrap_tsv\tsystem={}\tgeneration={}\tgames={}\tpublished={}\tchanged_inputs={}",
        system_id,
        outcome.generation,
        outcome.games,
        u8::from(outcome.published),
        outcome.changed_inputs
    );
    Ok(())
}

fn usage() -> String {
    "usage: catalog-lab fixture OUTPUT [--arcade-games N] [--small-system-games N] [--large-system-games N] [--large-system-depth N]\n       catalog-lab bootstrap-fixture SOURCE STORAGE snes|c64".to_string()
}
