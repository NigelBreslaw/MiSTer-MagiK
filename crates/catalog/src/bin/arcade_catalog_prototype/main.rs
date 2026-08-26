// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Standalone, from-scratch Arcade catalog builder prototype.

mod builder;
mod model;
mod scan;

use builder::{build_active, compile_base};
use model::{decode_active, decode_base, encode_active, encode_base, hex};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

const DEFAULT_ARCADE_ROOT: &str = "/media/fat/_Arcade";
const DEFAULT_MAME_ROM_DIRS: [&str; 2] = ["/media/fat/games/mame", "/media/fat/_Arcade/mame"];
const DEFAULT_HBMAME_ROM_DIRS: [&str; 2] = ["/media/fat/games/hbmame", "/media/fat/_Arcade/hbmame"];

fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("arcade-catalog-prototype: {error}");
        std::process::exit(2);
    }
}

fn run(arguments: Vec<String>) -> Result<(), String> {
    let Some((command, arguments)) = arguments.split_first() else {
        return Err(usage());
    };
    if matches!(command.as_str(), "help" | "--help" | "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let options = Options::parse(arguments)?;
    match command.as_str() {
        "compile-base" => command_compile_base(&options),
        "build-active" => command_build_active(&options),
        "build" => command_build(&options),
        "inspect" => command_inspect(&options),
        other => Err(format!("unknown command {other}\n{}", usage())),
    }
}

fn command_compile_base(options: &Options) -> Result<(), String> {
    options.require_known(&["updater-index", "output"], &[])?;
    let updater_path = options.required_path("updater-index")?;
    let output_path = options.required_path("output")?;
    let total_started = Instant::now();
    let updater_bytes = read_file(&updater_path, "Update_All Arcade index")?;
    let (base, compile) = compile_base(&updater_bytes)?;
    let encoded = encode_base(&base)?;
    let write_started = Instant::now();
    atomic_write(&output_path, &encoded)?;
    let report = serde_json::json!({
        "command": "compile-base",
        "output": output_path,
        "output_bytes": encoded.len(),
        "source_sha256": hex(&base.source_sha256),
        "write_us": elapsed_us(write_started),
        "total_us": elapsed_us(total_started),
        "compile": compile,
    });
    print_json(&report)
}

fn command_build_active(options: &Options) -> Result<(), String> {
    options.require_known(
        &[
            "base",
            "arcade-root",
            "mame-rom-dir",
            "hbmame-rom-dir",
            "output",
        ],
        &["parallel-probe", "verify-index-size", "full-walk"],
    )?;
    let base_path = options.required_path("base")?;
    let output_path = options.required_path("output")?;
    let arcade_root = options
        .optional_path("arcade-root")?
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ARCADE_ROOT));
    let mame_directories = rom_directories(options, "mame-rom-dir", &DEFAULT_MAME_ROM_DIRS);
    let hbmame_directories = rom_directories(options, "hbmame-rom-dir", &DEFAULT_HBMAME_ROM_DIRS);
    let total_started = Instant::now();
    let base_bytes = read_file(&base_path, "Arcade base")?;
    let (active, build) = build_active(
        &base_bytes,
        &arcade_root,
        &mame_directories,
        &hbmame_directories,
        options.flag("parallel-probe"),
        options.flag("verify-index-size"),
        options.flag("full-walk"),
    )?;
    let encoded = encode_active(&active)?;
    let write_started = Instant::now();
    atomic_write(&output_path, &encoded)?;
    let report = serde_json::json!({
        "command": "build-active",
        "output": output_path,
        "output_bytes": encoded.len(),
        "source_sha256": hex(&active.source_sha256),
        "parallel_inventory": options.flag("parallel-probe"),
        "verify_index_size": options.flag("verify-index-size"),
        "full_walk": options.flag("full-walk"),
        "write_us": elapsed_us(write_started),
        "total_us": elapsed_us(total_started),
        "build": build,
    });
    print_json(&report)
}

fn command_build(options: &Options) -> Result<(), String> {
    options.require_known(
        &[
            "updater-index",
            "base-output",
            "active-output",
            "arcade-root",
            "mame-rom-dir",
            "hbmame-rom-dir",
        ],
        &["parallel-probe", "verify-index-size", "full-walk"],
    )?;
    let updater_path = options.required_path("updater-index")?;
    let base_output = options.required_path("base-output")?;
    let active_output = options.required_path("active-output")?;
    let arcade_root = options
        .optional_path("arcade-root")?
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ARCADE_ROOT));
    let mame_directories = rom_directories(options, "mame-rom-dir", &DEFAULT_MAME_ROM_DIRS);
    let hbmame_directories = rom_directories(options, "hbmame-rom-dir", &DEFAULT_HBMAME_ROM_DIRS);

    let total_started = Instant::now();
    let updater_bytes = read_file(&updater_path, "Update_All Arcade index")?;
    let (base, compile) = compile_base(&updater_bytes)?;
    let base_bytes = encode_base(&base)?;
    let base_write_started = Instant::now();
    atomic_write(&base_output, &base_bytes)?;
    let base_write_us = elapsed_us(base_write_started);
    let (active, build) = build_active(
        &base_bytes,
        &arcade_root,
        &mame_directories,
        &hbmame_directories,
        options.flag("parallel-probe"),
        options.flag("verify-index-size"),
        options.flag("full-walk"),
    )?;
    let active_bytes = encode_active(&active)?;
    let active_write_started = Instant::now();
    atomic_write(&active_output, &active_bytes)?;
    let report = serde_json::json!({
        "command": "build",
        "base_output": base_output,
        "base_output_bytes": base_bytes.len(),
        "active_output": active_output,
        "active_output_bytes": active_bytes.len(),
        "source_sha256": hex(&active.source_sha256),
        "parallel_inventory": options.flag("parallel-probe"),
        "verify_index_size": options.flag("verify-index-size"),
        "full_walk": options.flag("full-walk"),
        "base_write_us": base_write_us,
        "active_write_us": elapsed_us(active_write_started),
        "total_us": elapsed_us(total_started),
        "compile": compile,
        "build": build,
    });
    print_json(&report)
}

fn command_inspect(options: &Options) -> Result<(), String> {
    options.require_known(&["input"], &[])?;
    let input = options.required_path("input")?;
    let bytes = read_file(&input, "Arcade prototype catalog")?;
    if let Ok(base) = decode_base(&bytes) {
        let report = InspectReport {
            kind: "base",
            bytes: bytes.len(),
            source_sha256: hex(&base.source_sha256),
            records: base.candidates.len(),
            preferred: None,
            counts: None,
        };
        return print_json(&report);
    }
    let active = decode_active(&bytes)?;
    let report = InspectReport {
        kind: "active",
        bytes: bytes.len(),
        source_sha256: hex(&active.source_sha256),
        records: active.records.len(),
        preferred: Some(
            active
                .records
                .iter()
                .filter(|record| record.preferred)
                .count(),
        ),
        counts: Some(active.counts),
    };
    print_json(&report)
}

#[derive(Serialize)]
struct InspectReport {
    kind: &'static str,
    bytes: usize,
    source_sha256: String,
    records: usize,
    preferred: Option<usize>,
    counts: Option<model::ActiveCounts>,
}

#[derive(Default)]
struct Options {
    values: BTreeMap<String, Vec<String>>,
    flags: BTreeSet<String>,
}

impl Options {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut index = 0usize;
        while index < arguments.len() {
            let argument = &arguments[index];
            let name = argument
                .strip_prefix("--")
                .ok_or_else(|| format!("expected an option, found {argument}"))?;
            if matches!(name, "parallel-probe" | "verify-index-size" | "full-walk") {
                options.flags.insert(name.to_string());
                index += 1;
                continue;
            }
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| format!("option --{name} requires a value"))?;
            if value.starts_with("--") {
                return Err(format!("option --{name} requires a value"));
            }
            options
                .values
                .entry(name.to_string())
                .or_default()
                .push(value.clone());
            index += 2;
        }
        Ok(options)
    }

    fn require_known(&self, values: &[&str], flags: &[&str]) -> Result<(), String> {
        for name in self.values.keys() {
            if !values.contains(&name.as_str()) {
                return Err(format!("unknown option --{name}"));
            }
        }
        for name in &self.flags {
            if !flags.contains(&name.as_str()) {
                return Err(format!("unknown flag --{name}"));
            }
        }
        Ok(())
    }

    fn required_path(&self, name: &str) -> Result<PathBuf, String> {
        self.optional_path(name)?
            .ok_or_else(|| format!("missing required option --{name}"))
    }

    fn optional_path(&self, name: &str) -> Result<Option<PathBuf>, String> {
        let Some(values) = self.values.get(name) else {
            return Ok(None);
        };
        if values.len() != 1 {
            return Err(format!("option --{name} must be specified once"));
        }
        Ok(Some(PathBuf::from(&values[0])))
    }

    fn paths(&self, name: &str) -> Vec<PathBuf> {
        self.values
            .get(name)
            .into_iter()
            .flatten()
            .map(PathBuf::from)
            .collect()
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

fn rom_directories(options: &Options, name: &str, defaults: &[&str]) -> Vec<PathBuf> {
    let configured = options.paths(name);
    if configured.is_empty() {
        defaults.iter().map(PathBuf::from).collect()
    } else {
        configured
    }
}

fn read_file(path: &Path, label: &str) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("read {label} {}: {error}", path.display()))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("create output directory {}: {error}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("output path has no UTF-8 file name: {}", path.display()))?;
    let temporary = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("create temporary output {}: {error}", temporary.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("write temporary output {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("sync temporary output {}: {error}", temporary.display()))?;
        fs::rename(&temporary, path).map_err(|error| {
            format!(
                "publish temporary output {} as {}: {error}",
                temporary.display(),
                path.display()
            )
        })?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync output directory {}: {error}", parent.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn print_json(value: &impl Serialize) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| format!("encode report JSON: {error}"))?;
    println!("{json}");
    Ok(())
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros() as u64
}

fn usage() -> String {
    "Usage:\n  arcade-catalog-prototype compile-base --updater-index PATH --output PATH\n  arcade-catalog-prototype build-active --base PATH --output PATH [--arcade-root PATH] [--mame-rom-dir PATH ...] [--hbmame-rom-dir PATH ...] [--parallel-probe] [--verify-index-size] [--full-walk]\n  arcade-catalog-prototype build --updater-index PATH --base-output PATH --active-output PATH [inventory options]\n  arcade-catalog-prototype inspect --input PATH"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn options_keep_repeated_rom_directories() {
        let arguments = vec![
            "--mame-rom-dir".to_string(),
            "/one".to_string(),
            "--mame-rom-dir".to_string(),
            "/two".to_string(),
            "--parallel-probe".to_string(),
        ];
        let options = Options::parse(&arguments).unwrap();
        assert_eq!(
            options.paths("mame-rom-dir"),
            vec![PathBuf::from("/one"), PathBuf::from("/two")]
        );
        assert!(options.flag("parallel-probe"));
    }

    #[test]
    fn atomic_write_replaces_complete_file() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "arcade-catalog-prototype-publish-{}-{nonce}",
            std::process::id()
        ));
        let output = root.join("active.bin");
        atomic_write(&output, b"first").unwrap();
        atomic_write(&output, b"second").unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"second");
        fs::remove_dir_all(root).unwrap();
    }
}
