// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic, copyright-free catalog fixtures for tests and profiling.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const FILES_PER_BUCKET: usize = 256;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SyntheticFixtureSpec {
    pub arcade_games: usize,
    pub small_system_games: usize,
    pub large_system_games: usize,
    pub large_system_depth: usize,
}

impl Default for SyntheticFixtureSpec {
    fn default() -> Self {
        Self {
            arcade_games: 8,
            small_system_games: 32,
            large_system_games: 4_096,
            large_system_depth: 4,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SyntheticFixtureSummary {
    pub format: String,
    pub spec: SyntheticFixtureSpec,
    pub files: usize,
}

pub fn generate_synthetic_fixture(
    root: &Path,
    spec: &SyntheticFixtureSpec,
) -> io::Result<SyntheticFixtureSummary> {
    validate_spec(spec)?;
    fs::create_dir(root)?;

    write_core(root, "_Console", "SNES")?;
    write_core(root, "_Computer", "C64")?;
    write_arcade_games(root, spec.arcade_games)?;
    write_system_games(root, "SNES", "sfc", spec.small_system_games, 1)?;
    write_system_games(
        root,
        "C64",
        "d64",
        spec.large_system_games,
        spec.large_system_depth,
    )?;

    let summary = SyntheticFixtureSummary {
        format: "mister-magik-synthetic-catalog-fixture-v1".to_string(),
        spec: spec.clone(),
        files: 3 + spec.arcade_games + spec.small_system_games + spec.large_system_games,
    };
    let manifest = serde_json::to_vec_pretty(&summary)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(root.join("fixture.json"), manifest)?;
    Ok(summary)
}

pub fn add_synthetic_snes_game(root: &Path, index: usize) -> io::Result<PathBuf> {
    let directory = bucket_path(&root.join("games").join("SNES"), 0, 1);
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("Synthetic SNES {index:08}.sfc"));
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("synthetic SNES game already exists: {}", path.display()),
        ));
    }
    fs::write(
        &path,
        format!("MISTER-MAGIK-SYNTHETIC-GAME\nSNES\n{index:08}\n"),
    )?;
    Ok(path)
}

fn validate_spec(spec: &SyntheticFixtureSpec) -> io::Result<()> {
    if spec.large_system_depth == 0 || spec.large_system_depth > 16 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "large-system depth must be between 1 and 16",
        ));
    }
    Ok(())
}

fn write_core(root: &Path, class: &str, system: &str) -> io::Result<()> {
    let directory = root.join(class);
    fs::create_dir_all(&directory)?;
    fs::write(
        directory.join(format!("{system}_20260717.rbf")),
        format!("MISTER-MAGIK-SYNTHETIC-CORE\n{system}\n"),
    )
}

fn write_arcade_games(root: &Path, count: usize) -> io::Result<()> {
    let directory = root.join("_Arcade");
    fs::create_dir_all(&directory)?;
    for index in 0..count {
        let setname = format!("synthetic{index:08}");
        let document = format!(
            "<misterromdescription><name>Synthetic Arcade {index:08}</name><setname>{setname}</setname></misterromdescription>\n"
        );
        fs::write(
            directory.join(format!("Synthetic Arcade {index:08}.mra")),
            document,
        )?;
    }
    Ok(())
}

fn write_system_games(
    root: &Path,
    system: &str,
    extension: &str,
    count: usize,
    depth: usize,
) -> io::Result<()> {
    let base = root.join("games").join(system);
    for index in 0..count {
        let bucket = index / FILES_PER_BUCKET;
        let directory = bucket_path(&base, bucket, depth);
        fs::create_dir_all(&directory)?;
        fs::write(
            directory.join(format!("Synthetic {system} {index:08}.{extension}")),
            format!("MISTER-MAGIK-SYNTHETIC-GAME\n{system}\n{index:08}\n"),
        )?;
    }
    Ok(())
}

fn bucket_path(base: &Path, bucket: usize, depth: usize) -> PathBuf {
    let mut path = base.to_path_buf();
    for level in 0..depth {
        let divisor = 32usize.saturating_pow(level as u32);
        let part = (bucket / divisor) % 32;
        path.push(format!("level-{level:02}-{part:02}"));
    }
    path.push(format!("bucket-{bucket:08}"));
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn fixture_is_byte_for_byte_deterministic() {
        let first = temporary_path("first");
        let second = temporary_path("second");
        let spec = SyntheticFixtureSpec {
            arcade_games: 2,
            small_system_games: 3,
            large_system_games: 260,
            large_system_depth: 3,
        };
        let first_summary = generate_synthetic_fixture(&first, &spec).unwrap();
        let second_summary = generate_synthetic_fixture(&second, &spec).unwrap();
        assert_eq!(first_summary, second_summary);
        assert_eq!(inventory(&first), inventory(&second));
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn fixture_refuses_to_overwrite_an_existing_directory() {
        let root = temporary_path("existing");
        fs::create_dir(&root).unwrap();
        let error =
            generate_synthetic_fixture(&root, &SyntheticFixtureSpec::default()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fixture_rejects_unbounded_depth() {
        let root = temporary_path("depth");
        let error = generate_synthetic_fixture(
            &root,
            &SyntheticFixtureSpec {
                large_system_depth: 17,
                ..SyntheticFixtureSpec::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!root.exists());
    }

    #[test]
    fn appending_a_snes_game_is_deterministic_and_non_destructive() {
        let root = temporary_path("append");
        generate_synthetic_fixture(
            &root,
            &SyntheticFixtureSpec {
                arcade_games: 0,
                small_system_games: 1,
                large_system_games: 0,
                large_system_depth: 1,
            },
        )
        .unwrap();

        let path = add_synthetic_snes_game(&root, 1).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "MISTER-MAGIK-SYNTHETIC-GAME\nSNES\n00000001\n"
        );
        assert_eq!(
            add_synthetic_snes_game(&root, 1).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        fs::read(path).unwrap(),
                    );
                }
            }
        }
        files
    }

    fn temporary_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "mister-magik-catalog-fixture-{label}-{}-{nonce}",
            std::process::id()
        ))
    }
}
