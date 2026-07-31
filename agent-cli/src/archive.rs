// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::{AgentError, AgentResult};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

const MAX_DISTRIBUTION_MEMBERS: usize = 512;
const MAX_DISTRIBUTION_MEMBER_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DISTRIBUTION_BYTES: u64 = 512 * 1024 * 1024;

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

pub(crate) fn read_distribution_zip(path: &Path) -> AgentResult<BTreeMap<String, Vec<u8>>> {
    let mut archive = ZipArchive::new(File::open(path).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    if archive.len() > MAX_DISTRIBUTION_MEMBERS {
        return Err(unsafe_member("too many distribution members"));
    }
    let mut files = BTreeMap::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = entry.name().to_owned();
        let enclosed = entry.enclosed_name().ok_or_else(|| unsafe_member(&name))?;
        if entry.is_dir() {
            if !name.ends_with('/') {
                return Err(unsafe_member(&name));
            }
            continue;
        }
        validate_member(
            &name,
            &enclosed,
            false,
            MemberLayout::Nested,
            files.contains_key(&name),
        )?;
        if entry.size() > MAX_DISTRIBUTION_MEMBER_BYTES {
            return Err(unsafe_member(&name));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| unsafe_member(&name))?;
        if total > MAX_DISTRIBUTION_BYTES {
            return Err(unsafe_member("distribution is too large"));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
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
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    enum Entry<'a> {
        File(&'a str, &'a [u8]),
        Directory(&'a str),
    }

    fn archive(entries: &[Entry<'_>]) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-cli-archive-{}-{nonce}.zip",
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

    #[test]
    fn distribution_reader_accepts_safe_directory_entries() {
        let path = archive(&[
            Entry::Directory("mister-magik/"),
            Entry::File("mister-magik/release-v1.txt", b"release"),
        ]);
        assert_eq!(
            read_distribution_zip(&path).unwrap()["mister-magik/release-v1.txt"],
            b"release"
        );
        fs::remove_file(path).unwrap();
    }
}
