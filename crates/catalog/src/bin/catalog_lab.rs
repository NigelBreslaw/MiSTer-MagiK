// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_catalog::synthetic_fixture::{SyntheticFixtureSpec, generate_synthetic_fixture};
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
        if command == "rebuild-bench" {
            return rebuild_bench(args);
        }
        if command == "latch-load-scenario" {
            return latch_load_scenario(args);
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

fn latch_load_scenario(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    const DEFAULT_GAMES: usize = 500_000;

    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("latch-load-scenario needs OUTPUT\n{}", usage()))?;
    let games = args
        .next()
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| format!("invalid game count {value:?}"))
        })
        .transpose()?
        .unwrap_or(DEFAULT_GAMES);
    let spec = latch_load_spec(games)?;
    if args.next().is_some() {
        return Err(format!(
            "latch-load-scenario has unexpected arguments\n{}",
            usage()
        ));
    }

    std::fs::create_dir(&output)
        .map_err(|error| format!("could not create {}: {error}", output.display()))?;
    let library_root = output.join("library");
    let summary = generate_synthetic_fixture(&library_root, &spec)
        .map_err(|error| format!("could not create {}: {error}", library_root.display()))?;
    let scenario = serde_json::json!({
        "schema": "mister-magik-latch-load-scenario-v1",
        "files": summary.files,
        "limits": {
            "max_games": 500_000,
        },
        "environment": {
            "MISTER_LIBRARY_ROOTS": library_root,
            "MISTER_SHARDED_CATALOG_DIR": output.join("catalog-v3"),
            "MISTER_LIBRARY_SQLITE": output.join("library.sqlite3"),
            "MISTER_MAGIK_DEV_LATCH_STATUS_TIMEOUT_AT": 1,
        },
        "expected": {
            "compatibility_fallback_observed": true,
            "application_input_blocked": false,
            "eventual_latch_recovery": true,
            "support_report": "diagnostics/latch/latest.json",
        }
    });
    std::fs::write(
        output.join("scenario.json"),
        serde_json::to_vec_pretty(&scenario).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("could not write scenario manifest: {error}"))?;
    println!(
        "catalog_latch_load_scenario_tsv\troot={}\tfiles={}\tgames={}\tmanifest={}",
        output.display(),
        summary.files,
        games,
        output.join("scenario.json").display()
    );
    Ok(())
}

fn latch_load_spec(games: usize) -> Result<SyntheticFixtureSpec, String> {
    const MAX_GAMES: usize = 500_000;
    if games == 0 || games > MAX_GAMES {
        return Err(format!(
            "latch-load-scenario game count must be between 1 and {MAX_GAMES}"
        ));
    }
    Ok(SyntheticFixtureSpec {
        arcade_games: 8,
        small_system_games: 32,
        large_system_games: games,
        large_system_depth: 4,
    })
}

fn rebuild_bench(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    use mister_magik_catalog::rebuild_benchmark::run_rebuild_benchmark;
    use mister_magik_catalog::shard_registry::RegistryLimits;
    use mister_magik_catalog::system_shard::SystemShardLimits;

    let storage = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("rebuild-bench needs STORAGE\n{}", usage()))?;
    let systems = args
        .next()
        .unwrap_or_else(|| "20".to_string())
        .parse::<usize>()
        .map_err(|_| "rebuild-bench SYSTEMS is invalid".to_string())?;
    let games = args
        .next()
        .unwrap_or_else(|| "200".to_string())
        .parse::<usize>()
        .map_err(|_| "rebuild-bench GAMES_PER_SYSTEM is invalid".to_string())?;
    if args.next().is_some() {
        return Err(format!(
            "rebuild-bench has unexpected arguments\n{}",
            usage()
        ));
    }
    let outcome = run_rebuild_benchmark(
        &storage,
        systems,
        games,
        RegistryLimits {
            max_manifest_bytes: 8 * 1024 * 1024,
            max_systems: 4096,
            shard: SystemShardLimits {
                max_sqlite_bytes: 8 * 1024 * 1024 * 1024,
                max_navigation_compressed_bytes: 512 * 1024 * 1024,
                max_navigation_decoded_bytes: 512 * 1024 * 1024,
                max_games: 2_000_000,
            },
        },
    )
    .map_err(|error| error.to_string())?;
    println!(
        "catalog_rebuild_bench_tsv\tfull_us={}\tdelta_us={}\telapsed_speedup={:.3}\tfull_systems={}\tdelta_systems={}\twork_ratio={:.3}\tgames_per_system={}\tfull_logical_bytes={}\tfull_allocated_bytes={}\tfull_files={}\tfull_directories={}\tnavigation_open_p50_us={}\tnavigation_open_p95_us={}\tnavigation_open_p99_us={}\tpeak_rss_bytes={}",
        outcome.full_us,
        outcome.delta_us,
        outcome.elapsed_speedup(),
        outcome.full_systems,
        outcome.delta_systems,
        outcome.work_ratio(),
        outcome.games_per_system,
        outcome.full_logical_bytes,
        outcome.full_allocated_bytes,
        outcome.full_files,
        outcome.full_directories,
        outcome.navigation_open_p50_us,
        outcome.navigation_open_p95_us,
        outcome.navigation_open_p99_us,
        peak_rss_bytes(),
    );
    Ok(())
}

fn peak_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the supplied rusage structure on success.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return 0;
    }
    // macOS and the BSDs report bytes; Linux and Android report KiB.
    let resident = unsafe { usage.assume_init() }.ru_maxrss.max(0) as u64;
    if cfg!(any(target_os = "linux", target_os = "android")) {
        resident.saturating_mul(1024)
    } else {
        resident
    }
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
    "usage: catalog-lab fixture OUTPUT [--arcade-games N] [--small-system-games N] [--large-system-games N] [--large-system-depth N]\n       catalog-lab latch-load-scenario OUTPUT [GAMES]\n       catalog-lab bootstrap-fixture SOURCE STORAGE snes|c64\n       catalog-lab rebuild-bench STORAGE [SYSTEMS] [GAMES_PER_SYSTEM]".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latch_load_scenario_is_bounded_at_500k_games() {
        assert_eq!(
            latch_load_spec(500_000).unwrap().large_system_games,
            500_000
        );
        assert!(latch_load_spec(0).is_err());
        assert!(latch_load_spec(500_001).is_err());
    }
}
