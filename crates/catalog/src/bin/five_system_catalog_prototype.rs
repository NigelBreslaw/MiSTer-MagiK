// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Standalone interchange and publication tool for the fast five-system catalog.

use mister_magik_catalog::fast_five_catalog::{
    FastFiveSnapshot, publish_snapshot, replace_arcade_from_active, snapshot_reference,
};
use mister_magik_catalog::shard_registry::production_registry_limits;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("five-system-catalog-prototype: {error}");
        std::process::exit(2);
    }
}

fn run(arguments: Vec<String>) -> Result<(), String> {
    let Some((command, arguments)) = arguments.split_first() else {
        return Err(usage());
    };
    match command.as_str() {
        "snapshot-reference" => {
            let catalog_root = required_path(arguments, "--catalog-root")?;
            let output = required_path(arguments, "--output")?;
            reject_unknown(arguments, &["--catalog-root", "--output"])?;
            let snapshot = snapshot_reference(&catalog_root, production_registry_limits())?;
            write_json_atomic(&output, &snapshot)?;
            print_json(&serde_json::json!({
                "command": "snapshot-reference",
                "systems": snapshot.systems.len(),
                "games": snapshot.game_count(),
                "output": output,
                "source_fingerprint": snapshot.source_fingerprint,
            }))
        }
        "publish" => {
            let input = required_path(arguments, "--input")?;
            let output_root = required_path(arguments, "--output-root")?;
            reject_unknown(arguments, &["--input", "--output-root"])?;
            let snapshot: FastFiveSnapshot = serde_json::from_slice(
                &fs::read(&input).map_err(|error| format!("read {}: {error}", input.display()))?,
            )
            .map_err(|error| format!("decode {}: {error}", input.display()))?;
            let report = publish_snapshot(&output_root, &snapshot, production_registry_limits())?;
            print_json(&report)
        }
        "replace-arcade" => {
            let input = required_path(arguments, "--input")?;
            let arcade_active = required_path(arguments, "--arcade-active")?;
            let output = required_path(arguments, "--output")?;
            reject_unknown(arguments, &["--input", "--arcade-active", "--output"])?;
            let snapshot: FastFiveSnapshot = serde_json::from_slice(
                &fs::read(&input).map_err(|error| format!("read {}: {error}", input.display()))?,
            )
            .map_err(|error| format!("decode {}: {error}", input.display()))?;
            let active = fs::read(&arcade_active)
                .map_err(|error| format!("read {}: {error}", arcade_active.display()))?;
            let snapshot = replace_arcade_from_active(snapshot, &active)?;
            write_json_atomic(&output, &snapshot)?;
            print_json(&serde_json::json!({
                "command": "replace-arcade",
                "games": snapshot.game_count(),
                "arcade_games": snapshot.systems.iter().find(|system| system.system_id == "arcade").map(|system| system.games.len()),
                "output": output,
                "source_fingerprint": snapshot.source_fingerprint,
            }))
        }
        "inspect" => {
            let input = required_path(arguments, "--input")?;
            reject_unknown(arguments, &["--input"])?;
            let snapshot: FastFiveSnapshot = serde_json::from_slice(
                &fs::read(&input).map_err(|error| format!("read {}: {error}", input.display()))?,
            )
            .map_err(|error| format!("decode {}: {error}", input.display()))?;
            snapshot.validate()?;
            print_json(&serde_json::json!({
                "schema": snapshot.schema,
                "source_fingerprint": snapshot.source_fingerprint,
                "systems": snapshot.systems.iter().map(|system| serde_json::json!({
                    "system_id": system.system_id,
                    "games": system.games.len(),
                })).collect::<Vec<_>>(),
                "games": snapshot.game_count(),
            }))
        }
        "compare" => {
            let reference_root = required_path(arguments, "--reference-root")?;
            let candidate_root = required_path(arguments, "--candidate-root")?;
            reject_unknown(arguments, &["--reference-root", "--candidate-root"])?;
            let reference = snapshot_reference(&reference_root, production_registry_limits())?;
            let candidate = snapshot_reference(&candidate_root, production_registry_limits())?;
            let systems = reference
                .systems
                .iter()
                .map(|reference_system| {
                    let candidate_system = candidate
                        .systems
                        .iter()
                        .find(|system| system.system_id == reference_system.system_id)
                        .expect("validated candidate contains every fast-five system");
                    let reference_rows = reference_system
                        .games
                        .iter()
                        .map(|game| (game.stable_key.as_str(), game))
                        .collect::<BTreeMap<_, _>>();
                    let candidate_rows = candidate_system
                        .games
                        .iter()
                        .map(|game| (game.stable_key.as_str(), game))
                        .collect::<BTreeMap<_, _>>();
                    let missing = reference_rows
                        .keys()
                        .filter(|key| !candidate_rows.contains_key(*key))
                        .copied()
                        .collect::<Vec<_>>();
                    let unexpected = candidate_rows
                        .keys()
                        .filter(|key| !reference_rows.contains_key(*key))
                        .copied()
                        .collect::<Vec<_>>();
                    let changed = reference_rows
                        .iter()
                        .filter(|(key, game)| {
                            candidate_rows.get(*key).is_some_and(|row| row != *game)
                        })
                        .map(|(key, _)| *key)
                        .collect::<Vec<_>>();
                    serde_json::json!({
                        "system_id": reference_system.system_id,
                        "reference_games": reference_rows.len(),
                        "candidate_games": candidate_rows.len(),
                        "missing": missing.len(),
                        "unexpected": unexpected.len(),
                        "changed": changed.len(),
                        "missing_sample": missing.into_iter().take(20).collect::<Vec<_>>(),
                        "unexpected_sample": unexpected.into_iter().take(20).collect::<Vec<_>>(),
                        "changed_sample": changed.into_iter().take(20).collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>();
            let exact = systems.iter().all(|system| {
                system.get("missing").and_then(serde_json::Value::as_u64) == Some(0)
                    && system.get("unexpected").and_then(serde_json::Value::as_u64) == Some(0)
                    && system.get("changed").and_then(serde_json::Value::as_u64) == Some(0)
            });
            print_json(&serde_json::json!({
                "command": "compare",
                "status": if exact { "exact" } else { "different" },
                "systems": systems,
            }))
        }
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            Ok(())
        }
        _ => Err(usage()),
    }
}

fn required_path(arguments: &[String], name: &str) -> Result<PathBuf, String> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| PathBuf::from(&pair[1]))
        .ok_or_else(|| format!("missing {name}"))
}

fn reject_unknown(arguments: &[String], known: &[&str]) -> Result<(), String> {
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if !known.contains(&argument.as_str()) || index + 1 >= arguments.len() {
            return Err(format!("unknown or incomplete option {argument}"));
        }
        index += 2;
    }
    Ok(())
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("snapshot"),
        std::process::id()
    ));
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("publish {}: {error}", path.display()))
}

fn print_json(value: &impl Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn usage() -> String {
    "Usage:\n  five-system-catalog-prototype snapshot-reference --catalog-root PATH --output PATH\n  five-system-catalog-prototype replace-arcade --input PATH --arcade-active PATH --output PATH\n  five-system-catalog-prototype publish --input PATH --output-root PATH\n  five-system-catalog-prototype inspect --input PATH\n  five-system-catalog-prototype compare --reference-root PATH --candidate-root PATH"
        .to_string()
}
