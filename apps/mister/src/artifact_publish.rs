// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub struct ArtifactPublishLabels<'a> {
    pub destination: &'a str,
    pub parent: &'a str,
}

#[derive(Clone, Debug)]
pub struct ArtifactPublishPlan {
    final_path: PathBuf,
    temp_path: PathBuf,
    parent: PathBuf,
}

impl ArtifactPublishPlan {
    pub fn temp_path(&self) -> &Path {
        &self.temp_path
    }

    pub fn parent(&self) -> &Path {
        &self.parent
    }

    pub fn cleanup_temp(&self) {
        let _ = fs::remove_file(&self.temp_path);
    }

    pub fn install_temp(&self, rename_context: Option<&str>) -> Result<(), String> {
        fs::rename(&self.temp_path, &self.final_path).map_err(|e| {
            self.cleanup_temp();
            match rename_context {
                Some(context) if !context.is_empty() => format!(
                    "rename {context} {} -> {}: {e}",
                    self.temp_path.display(),
                    self.final_path.display()
                ),
                _ => format!(
                    "rename {} -> {}: {e}",
                    self.temp_path.display(),
                    self.final_path.display()
                ),
            }
        })
    }
}

pub fn prepare_artifact_publish(
    final_path: &Path,
    temp_path: PathBuf,
    labels: ArtifactPublishLabels<'_>,
) -> Result<ArtifactPublishPlan, String> {
    let parent = final_path.parent().ok_or_else(|| {
        format!(
            "{} has no parent: {}",
            labels.destination,
            final_path.display()
        )
    })?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("create {} {}: {e}", labels.parent, parent.display()))?;
    let _ = fs::remove_file(&temp_path);
    Ok(ArtifactPublishPlan {
        final_path: final_path.to_path_buf(),
        temp_path,
        parent: parent.to_path_buf(),
    })
}

pub fn static_temp_path_for(final_path: &Path, fallback_name: &str) -> PathBuf {
    final_path.with_file_name(format!("{}.tmp", file_name_or(final_path, fallback_name)))
}

pub fn timestamped_temp_path_for(
    final_path: &Path,
    fallback_name: &str,
    stamp: impl std::fmt::Display,
) -> PathBuf {
    final_path.with_file_name(format!(
        "{}.tmp-{stamp}",
        file_name_or(final_path, fallback_name)
    ))
}

pub fn hidden_bench_temp_path_for(final_path: &Path, fallback_name: &str, stamp: &str) -> PathBuf {
    final_path.with_file_name(format!(
        ".{}.bench-{stamp}.tmp",
        file_name_or(final_path, fallback_name)
    ))
}

pub fn hidden_timestamped_temp_path_for(
    final_path: &Path,
    fallback_name: &str,
    stamp: impl std::fmt::Display,
) -> PathBuf {
    final_path.with_file_name(format!(
        ".{}.tmp-{stamp}",
        file_name_or(final_path, fallback_name)
    ))
}

pub fn cleanup_static_and_timestamped_temps(final_path: &Path, fallback_name: &str) {
    let _ = fs::remove_file(static_temp_path_for(final_path, fallback_name));
    let Some(parent) = final_path.parent() else {
        return;
    };
    let base_name = file_name_or(final_path, fallback_name);
    let prefix = format!("{base_name}.tmp-");
    let hidden_prefix = format!(".{base_name}.tmp-");
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with(&prefix) || name.starts_with(&hidden_prefix) {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn sync_path_rust_best_effort(path: &Path) {
    let _ = sync_path_rust(path);
}

fn sync_path_rust(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn file_name_or(path: &Path, fallback_name: &str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(fallback_name)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn temp_path_helpers_preserve_existing_pack_names() {
        let final_path = Path::new("/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b");

        assert_eq!(
            static_temp_path_for(final_path, "screenshot-pack")
                .display()
                .to_string(),
            "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b.tmp"
        );
        assert_eq!(
            timestamped_temp_path_for(final_path, "screenshot-pack", 123)
                .display()
                .to_string(),
            "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b.tmp-123"
        );
        assert_eq!(
            hidden_bench_temp_path_for(final_path, "screenshot-pack", "bench")
                .display()
                .to_string(),
            "/media/fat/mister-magik/assets/.arcade-screenshots.mmlz4b.bench-bench.tmp"
        );
        assert_eq!(
            hidden_timestamped_temp_path_for(final_path, "screenshot-pack", 123)
                .display()
                .to_string(),
            "/media/fat/mister-magik/assets/.arcade-screenshots.mmlz4b.tmp-123"
        );
    }

    #[test]
    fn cleanup_static_and_timestamped_temps_keeps_other_artifacts() {
        let dir = temp_dir("mister-magik-artifact-cleanup");
        let final_path = dir.join("arcade-screenshots-320x320.mmlz4b");
        let static_tmp = dir.join("arcade-screenshots-320x320.mmlz4b.tmp");
        let stamped_tmp = dir.join("arcade-screenshots-320x320.mmlz4b.tmp-1");
        let hidden_stamped_tmp = dir.join(".arcade-screenshots-320x320.mmlz4b.tmp-2");
        let other_tmp = dir.join("neogeo-screenshots-320x320.mmlz4b.tmp-1");
        let final_file = dir.join("arcade-screenshots-320x320.mmlz4b");
        fs::write(&static_tmp, b"partial").unwrap();
        fs::write(&stamped_tmp, b"partial").unwrap();
        fs::write(&hidden_stamped_tmp, b"partial").unwrap();
        fs::write(&other_tmp, b"partial").unwrap();
        fs::write(&final_file, b"current").unwrap();

        cleanup_static_and_timestamped_temps(&final_path, "screenshot-pack");

        assert!(!static_tmp.exists());
        assert!(!stamped_tmp.exists());
        assert!(!hidden_stamped_tmp.exists());
        assert!(other_tmp.exists());
        assert_eq!(fs::read(final_file).unwrap(), b"current");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn prepare_publish_creates_parent_and_removes_stale_temp() {
        let dir = temp_dir("mister-magik-artifact-prepare");
        let final_path = dir.join("nested/out.bin");
        let temp_path = static_temp_path_for(&final_path, "artifact");
        fs::create_dir_all(temp_path.parent().unwrap()).unwrap();
        fs::write(&temp_path, b"stale").unwrap();

        let plan = prepare_artifact_publish(
            &final_path,
            temp_path.clone(),
            ArtifactPublishLabels {
                destination: "artifact destination",
                parent: "artifact destination parent",
            },
        )
        .unwrap();

        assert_eq!(plan.temp_path(), temp_path);
        assert!(final_path.parent().unwrap().exists());
        assert!(!temp_path.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rust_sync_best_effort_accepts_existing_path() {
        let dir = temp_dir("mister-magik-artifact-rust-sync");
        let file = dir.join("out.bin");
        fs::write(&file, b"synced").unwrap();

        sync_path_rust_best_effort(&file);
        sync_path_rust_best_effort(&dir);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rust_sync_best_effort_ignores_missing_path() {
        let missing =
            std::env::temp_dir().join(format!("mister-magik-missing-sync-{}", std::process::id()));

        sync_path_rust_best_effort(&missing);
    }
}
