// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::{AgentError, AgentResult};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

#[derive(Clone, Copy)]
pub(crate) enum MemberLayout {
    Flat,
    Nested,
}

pub(crate) fn read_zip(
    path: &Path,
    layout: MemberLayout,
) -> AgentResult<BTreeMap<String, Vec<u8>>> {
    let mut archive = ZipArchive::new(File::open(path).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let mut files = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = entry.name().to_owned();
        let enclosed = entry.enclosed_name().ok_or_else(|| unsafe_member(&name))?;
        validate_member(
            &name,
            &enclosed,
            entry.is_dir(),
            layout,
            files.contains_key(&name),
        )?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        files.insert(name, bytes);
    }
    Ok(files)
}

fn validate_member(
    name: &str,
    enclosed: &Path,
    is_directory: bool,
    layout: MemberLayout,
    duplicate: bool,
) -> AgentResult<()> {
    let nested = enclosed.components().count() != 1;
    if is_directory || matches!(layout, MemberLayout::Flat) && nested || duplicate {
        Err(unsafe_member(name))
    } else {
        Ok(())
    }
}

fn unsafe_member(name: &str) -> AgentError {
    AgentError::Classified {
        code: "unsafe_archive_member",
        detail: name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    static ARCHIVE_ID: AtomicU64 = AtomicU64::new(0);

    enum Entry<'a> {
        File(&'a str, &'a [u8]),
        Directory(&'a str),
    }

    fn archive(entries: &[Entry<'_>]) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = ARCHIVE_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agent-cli-archive-{}-{nonce}-{id}.zip",
            std::process::id()
        ));
        let mut archive = ZipWriter::new(File::create(&path).unwrap());
        for entry in entries {
            match entry {
                Entry::File(name, bytes) => {
                    archive
                        .start_file(*name, SimpleFileOptions::default())
                        .unwrap();
                    archive.write_all(bytes).unwrap();
                }
                Entry::Directory(name) => archive
                    .add_directory(*name, SimpleFileOptions::default())
                    .unwrap(),
            }
        }
        archive.finish().unwrap();
        path
    }

    #[test]
    fn flat_and_nested_layouts_are_explicit() {
        let flat = archive(&[Entry::File("manifest.json", b"manifest")]);
        assert_eq!(
            read_zip(&flat, MemberLayout::Flat).unwrap()["manifest.json"],
            b"manifest"
        );
        fs::remove_file(flat).unwrap();

        let nested = archive(&[Entry::File("component/artifact", b"artifact")]);
        assert!(read_zip(&nested, MemberLayout::Flat).is_err());
        assert_eq!(
            read_zip(&nested, MemberLayout::Nested).unwrap()["component/artifact"],
            b"artifact"
        );
        fs::remove_file(nested).unwrap();
    }

    #[test]
    fn unsafe_members_are_rejected() {
        for entries in [
            vec![Entry::File("../escape", b"bad")],
            vec![Entry::Directory("directory/")],
        ] {
            let path = archive(&entries);
            assert!(read_zip(&path, MemberLayout::Nested).is_err());
            fs::remove_file(path).unwrap();
        }
        assert!(
            validate_member("same", Path::new("same"), false, MemberLayout::Nested, true).is_err()
        );
    }
}
