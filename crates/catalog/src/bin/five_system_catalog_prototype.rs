// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Standalone interchange and publication tool for the fast five-system catalog.

use mister_magik_catalog::fast_five_catalog::{
    C64ArtifactExperimentProfile, FastFiveSnapshot, publish_snapshot, replace_arcade_from_active,
    run_c64_artifact_experiment, snapshot_reference,
};
use mister_magik_catalog::shard_registry::production_registry_limits;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

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
        "publish" => run_publish(arguments, false),
        "publish-profile" => run_publish(arguments, true),
        "c64-artifact-experiment" => {
            let command_started = Instant::now();
            let input = required_path(arguments, "--input")?;
            let output_root = required_path(arguments, "--output-root")?;
            let scratch_root = required_path(arguments, "--scratch-root")?;
            let profile =
                C64ArtifactExperimentProfile::parse(&required_value(arguments, "--profile")?)?;
            reject_unknown(
                arguments,
                &["--input", "--output-root", "--scratch-root", "--profile"],
            )?;
            let input_started = Instant::now();
            let input_bytes =
                fs::read(&input).map_err(|error| format!("read {}: {error}", input.display()))?;
            let snapshot: FastFiveSnapshot = serde_json::from_slice(&input_bytes)
                .map_err(|error| format!("decode {}: {error}", input.display()))?;
            let input_read_decode_us = elapsed_us(input_started);
            let report = run_c64_artifact_experiment(
                &output_root,
                &scratch_root,
                &snapshot,
                profile,
                production_registry_limits(),
            )?;
            let mut report = serde_json::to_value(report).map_err(|error| error.to_string())?;
            let object = report
                .as_object_mut()
                .ok_or("C64 artifact experiment report is not an object")?;
            object.insert(
                "input_bytes".to_string(),
                serde_json::json!(input_bytes.len()),
            );
            object.insert(
                "input_read_decode_us".to_string(),
                serde_json::json!(input_read_decode_us),
            );
            object.insert(
                "command_elapsed_us".to_string(),
                serde_json::json!(elapsed_us(command_started)),
            );
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

fn run_publish(arguments: &[String], profile: bool) -> Result<(), String> {
    let command_started = Instant::now();
    let input = required_path(arguments, "--input")?;
    let output_root = required_path(arguments, "--output-root")?;
    let known = if profile {
        &[
            "--input",
            "--output-root",
            "--pprof-svg",
            "--pprof-folded",
            "--pprof-hz",
        ][..]
    } else {
        &["--input", "--output-root"][..]
    };
    reject_unknown(arguments, known)?;
    let profiler = if profile {
        Some(start_cpu_profile(
            required_path(arguments, "--pprof-svg")?,
            required_path(arguments, "--pprof-folded")?,
            required_value(arguments, "--pprof-hz")?
                .parse::<i32>()
                .map_err(|error| format!("invalid --pprof-hz: {error}"))?,
        )?)
    } else {
        None
    };
    let input_started = Instant::now();
    let input_bytes =
        fs::read(&input).map_err(|error| format!("read {}: {error}", input.display()))?;
    let snapshot: FastFiveSnapshot = serde_json::from_slice(&input_bytes)
        .map_err(|error| format!("decode {}: {error}", input.display()))?;
    snapshot.validate()?;
    let input_read_decode_us = elapsed_us(input_started);
    let report = publish_snapshot(&output_root, &snapshot, production_registry_limits())?;
    let mut report = serde_json::to_value(report).map_err(|error| error.to_string())?;
    let command_elapsed_us = elapsed_us(command_started);
    let profile = profiler.map(finish_cpu_profile).transpose()?;
    let object = report
        .as_object_mut()
        .ok_or("fast-five publish report is not an object")?;
    object.insert(
        "input_bytes".to_string(),
        serde_json::json!(input_bytes.len()),
    );
    object.insert(
        "input_read_decode_us".to_string(),
        serde_json::json!(input_read_decode_us),
    );
    object.insert(
        "command_elapsed_us".to_string(),
        serde_json::json!(command_elapsed_us),
    );
    if let Some(profile) = profile {
        object.insert("pprof".to_string(), profile);
    }
    print_json(&report)
}

#[cfg(feature = "profile")]
struct CatalogCpuProfiler {
    guard: pprof::ProfilerGuard<'static>,
    hz: i32,
    svg: PathBuf,
    folded: PathBuf,
}

#[cfg(not(feature = "profile"))]
struct CatalogCpuProfiler;

#[cfg(feature = "profile")]
fn start_cpu_profile(svg: PathBuf, folded: PathBuf, hz: i32) -> Result<CatalogCpuProfiler, String> {
    if !(1..=1_200).contains(&hz) {
        return Err("--pprof-hz must be between 1 and 1200".to_string());
    }
    for path in [&svg, &folded] {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create pprof directory {}: {error}", parent.display()))?;
        }
    }
    // SAFETY: no profiling timer is active yet; pprof temporarily replaces this
    // disposition and restores the harmless ignored action when the guard drops.
    if unsafe { libc::signal(libc::SIGPROF, libc::SIG_IGN) } == libc::SIG_ERR {
        return Err(format!(
            "prepare SIGPROF disposition: {}",
            std::io::Error::last_os_error()
        ));
    }
    let guard =
        pprof::ProfilerGuard::new(hz).map_err(|error| format!("start pprof sampler: {error}"))?;
    Ok(CatalogCpuProfiler {
        guard,
        hz,
        svg,
        folded,
    })
}

#[cfg(not(feature = "profile"))]
fn start_cpu_profile(
    _svg: PathBuf,
    _folded: PathBuf,
    _hz: i32,
) -> Result<CatalogCpuProfiler, String> {
    Err("publish-profile requires the profile feature".to_string())
}

#[cfg(feature = "profile")]
fn finish_cpu_profile(profiler: CatalogCpuProfiler) -> Result<serde_json::Value, String> {
    use std::fmt::Write as _;
    use std::io::Write as _;

    let report = profiler
        .guard
        .report()
        .build()
        .map_err(|error| format!("build pprof report: {error}"))?;
    let sample_stacks = report.data.len();
    let sample_hits: isize = report.data.values().sum();
    if sample_hits == 0 {
        return Err("pprof produced no CPU samples".to_string());
    }
    let mut lines = report
        .data
        .iter()
        .map(|(key, value)| {
            let mut line = key.thread_name_or_id();
            line.push(';');
            for frame in key.frames.iter().rev() {
                for symbol in frame.iter().rev() {
                    write!(&mut line, "{symbol};")
                        .expect("writing a folded pprof stack to String cannot fail");
                }
            }
            line.pop();
            write!(&mut line, " {value}")
                .expect("writing a folded pprof count to String cannot fail");
            line
        })
        .collect::<Vec<_>>();
    lines.sort();
    let mut folded = fs::File::create(&profiler.folded)
        .map_err(|error| format!("create {}: {error}", profiler.folded.display()))?;
    for line in lines {
        writeln!(folded, "{line}")
            .map_err(|error| format!("write {}: {error}", profiler.folded.display()))?;
    }
    let mut svg = fs::File::create(&profiler.svg)
        .map_err(|error| format!("create {}: {error}", profiler.svg.display()))?;
    report
        .flamegraph(&mut svg)
        .map_err(|error| format!("write {}: {error}", profiler.svg.display()))?;
    let duration_secs = report.timing.duration.as_secs_f64();
    Ok(serde_json::json!({
        "schema": "mister-magik-fast-five-pprof-v1",
        "hz": profiler.hz,
        "duration_secs": duration_secs,
        "sample_stacks": sample_stacks,
        "sample_hits": sample_hits,
        "svg": profiler.svg,
        "svg_bytes": fs::metadata(&profiler.svg).map_err(|error| error.to_string())?.len(),
        "folded": profiler.folded,
        "folded_bytes": fs::metadata(&profiler.folded).map_err(|error| error.to_string())?.len(),
    }))
}

#[cfg(not(feature = "profile"))]
fn finish_cpu_profile(_profiler: CatalogCpuProfiler) -> Result<serde_json::Value, String> {
    Err("publish-profile requires the profile feature".to_string())
}

fn required_path(arguments: &[String], name: &str) -> Result<PathBuf, String> {
    required_value(arguments, name).map(PathBuf::from)
}

fn required_value(arguments: &[String], name: &str) -> Result<String, String> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
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

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn usage() -> String {
    "Usage:\n  five-system-catalog-prototype snapshot-reference --catalog-root PATH --output PATH\n  five-system-catalog-prototype replace-arcade --input PATH --arcade-active PATH --output PATH\n  five-system-catalog-prototype publish --input PATH --output-root PATH\n  five-system-catalog-prototype publish-profile --input PATH --output-root PATH --pprof-svg PATH --pprof-folded PATH --pprof-hz HZ\n  five-system-catalog-prototype c64-artifact-experiment --input PATH --output-root PATH --scratch-root PATH --profile PROFILE\n  five-system-catalog-prototype inspect --input PATH\n  five-system-catalog-prototype compare --reference-root PATH --candidate-root PATH"
        .to_string()
}
