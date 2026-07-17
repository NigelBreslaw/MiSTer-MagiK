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

fn usage() -> String {
    "usage: catalog-lab fixture OUTPUT [--arcade-games N] [--small-system-games N] [--large-system-games N] [--large-system-depth N]".to_string()
}
