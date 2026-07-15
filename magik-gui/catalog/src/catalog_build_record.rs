// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const BUILD_DURATION_FILE_NAME: &str = "database-build-time.txt";

pub fn duration_path_for_sqlite(sqlite_path: &Path) -> PathBuf {
    sqlite_path.with_file_name(BUILD_DURATION_FILE_NAME)
}

pub fn rounded_seconds(elapsed: Duration) -> u64 {
    let rounded = elapsed.as_micros().saturating_add(500_000) / 1_000_000;
    u64::try_from(rounded).unwrap_or(u64::MAX)
}

pub fn format_duration(seconds: u64) -> String {
    if seconds == 1 {
        "1 second".to_string()
    } else {
        format!("{seconds} seconds")
    }
}

pub fn write_completed_build_duration(
    sqlite_path: &Path,
    elapsed: Duration,
) -> Result<u64, String> {
    let seconds = rounded_seconds(elapsed);
    write_seconds_atomically(&duration_path_for_sqlite(sqlite_path), seconds)?;
    Ok(seconds)
}

pub fn read_completed_build_duration(sqlite_path: &Path) -> Result<Option<u64>, String> {
    let path = duration_path_for_sqlite(sqlite_path);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "read catalog build duration {}: {error}",
                path.display()
            ))
        }
    };
    let seconds = text
        .trim()
        .parse::<u64>()
        .map_err(|error| format!("parse catalog build duration {}: {error}", path.display()))?;
    Ok(Some(seconds))
}

fn write_seconds_atomically(final_path: &Path, seconds: u64) -> Result<(), String> {
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "create catalog build duration dir {}: {error}",
                parent.display()
            )
        })?;
    }
    let temp_path = temp_path_for(final_path);
    let result = (|| -> Result<(), String> {
        let mut file = File::create(&temp_path).map_err(|error| {
            format!(
                "create catalog build duration temp {}: {error}",
                temp_path.display()
            )
        })?;
        writeln!(file, "{seconds}").map_err(|error| {
            format!(
                "write catalog build duration temp {}: {error}",
                temp_path.display()
            )
        })?;
        file.sync_all().map_err(|error| {
            format!(
                "sync catalog build duration temp {}: {error}",
                temp_path.display()
            )
        })?;
        drop(file);
        std::fs::rename(&temp_path, final_path).map_err(|error| {
            format!(
                "replace catalog build duration {} from {}: {error}",
                final_path.display(),
                temp_path.display()
            )
        })?;
        crate::sqlite_catalog::sync_parent_dir(final_path);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn temp_path_for(final_path: &Path) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(BUILD_DURATION_FILE_NAME);
    final_path.with_file_name(format!(".{file_name}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_sqlite_path(label: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "mister-magik-build-duration-{label}-{}",
                std::process::id()
            ))
            .join("library.sqlite3")
    }

    #[test]
    fn duration_rounds_to_nearest_second() {
        assert_eq!(rounded_seconds(Duration::from_millis(499)), 0);
        assert_eq!(rounded_seconds(Duration::from_millis(500)), 1);
        assert_eq!(rounded_seconds(Duration::from_millis(1_499)), 1);
        assert_eq!(rounded_seconds(Duration::from_millis(1_500)), 2);
    }

    #[test]
    fn completed_duration_is_written_and_read_next_to_catalog() {
        let sqlite_path = unique_sqlite_path("round-trip");
        let duration_path = duration_path_for_sqlite(&sqlite_path);
        let _ = std::fs::remove_dir_all(sqlite_path.parent().unwrap());

        let seconds = write_completed_build_duration(&sqlite_path, Duration::from_millis(118_514))
            .expect("write duration");

        assert_eq!(seconds, 119);
        assert_eq!(
            read_completed_build_duration(&sqlite_path).expect("read duration"),
            Some(119)
        );
        assert_eq!(
            std::fs::read_to_string(&duration_path).expect("read duration file"),
            "119\n"
        );
        assert!(!temp_path_for(&duration_path).exists());
        let _ = std::fs::remove_dir_all(sqlite_path.parent().unwrap());
    }

    #[test]
    fn duration_label_uses_singular_and_plural_seconds() {
        assert_eq!(format_duration(1), "1 second");
        assert_eq!(format_duration(119), "119 seconds");
    }
}
