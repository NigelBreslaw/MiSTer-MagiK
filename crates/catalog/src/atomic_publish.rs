// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs::File;
use std::path::{Path, PathBuf};

pub(crate) fn temp_path(final_path: &Path, fallback_name: &str) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(fallback_name);
    final_path.with_file_name(format!(".{file_name}.tmp"))
}

pub(crate) fn write_atomically(
    final_path: &Path,
    artifact: &str,
    fallback_name: &str,
    fault_prefix: Option<&str>,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> Result<(), String> {
    let mut fault_control = crate::fs_fault::NoopDirectResetFaultControl;
    write_atomically_with_fault_control(
        final_path,
        artifact,
        fallback_name,
        fault_prefix,
        &mut fault_control,
        write,
    )
}

pub(crate) fn write_atomically_with_fault_control(
    final_path: &Path,
    artifact: &str,
    fallback_name: &str,
    fault_prefix: Option<&str>,
    fault_control: &mut dyn crate::fs_fault::DirectResetFaultControl,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> Result<(), String> {
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {artifact} dir {}: {error}", parent.display()))?;
    }
    let temp_path = temp_path(final_path, fallback_name);
    let result = (|| -> Result<(), String> {
        let mut file = File::create(&temp_path)
            .map_err(|error| format!("create {artifact} temp {}: {error}", temp_path.display()))?;
        write(&mut file)
            .map_err(|error| format!("write {artifact} temp {}: {error}", temp_path.display()))?;
        maybe_fault(fault_prefix, "after_temp_write", final_path, fault_control);
        file.sync_all()
            .map_err(|error| format!("sync {artifact} temp {}: {error}", temp_path.display()))?;
        maybe_fault(fault_prefix, "after_temp_sync", final_path, fault_control);
        drop(file);
        std::fs::rename(&temp_path, final_path).map_err(|error| {
            format!(
                "replace {artifact} {} from {}: {error}",
                final_path.display(),
                temp_path.display()
            )
        })?;
        maybe_fault(
            fault_prefix,
            "after_rename_before_parent_sync",
            final_path,
            fault_control,
        );
        crate::sqlite_catalog::sync_parent_dir(final_path);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn maybe_fault(
    prefix: Option<&str>,
    point: &str,
    path: &Path,
    fault_control: &mut dyn crate::fs_fault::DirectResetFaultControl,
) {
    if let Some(prefix) = prefix {
        crate::fs_fault::maybe_fault_with_control(
            &format!("{prefix}.{point}"),
            path,
            fault_control,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[derive(Default)]
    struct RecordingFaultControl {
        points: Vec<String>,
    }

    impl crate::fs_fault::DirectResetFaultControl for RecordingFaultControl {
        fn request_direct_reset(
            &mut self,
            request: &crate::fs_fault::DirectResetFaultRequest,
        ) -> crate::fs_fault::DirectResetFaultOutcome {
            self.points.push(request.point().to_string());
            crate::fs_fault::DirectResetFaultOutcome::Noop
        }
    }

    #[test]
    fn failed_write_removes_temp_and_preserves_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "mister-magik-atomic-publish-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let final_path = dir.join("state.json");
        std::fs::write(&final_path, b"old").unwrap();
        let error = write_atomically(&final_path, "state", "state.json", None, |file| {
            file.write_all(b"new")?;
            Err(std::io::Error::other("injected"))
        })
        .unwrap_err();
        assert!(error.contains("injected"));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"old");
        assert!(!temp_path(&final_path, "state.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn injected_fault_control_observes_atomic_publication_order() {
        let dir = std::env::temp_dir().join(format!(
            "mister-magik-atomic-publish-hook-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let final_path = dir.join("state.json");
        let mut control = RecordingFaultControl::default();

        write_atomically_with_fault_control(
            &final_path,
            "state",
            "state.json",
            Some("catalog.state"),
            &mut control,
            |file| file.write_all(b"new"),
        )
        .expect("publish state");

        assert_eq!(
            control.points,
            vec![
                "catalog.state.after_temp_write",
                "catalog.state.after_temp_sync",
                "catalog.state.after_rename_before_parent_sync",
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
