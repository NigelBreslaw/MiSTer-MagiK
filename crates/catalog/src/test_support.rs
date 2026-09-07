// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared test fixtures for catalog modules.

use crate::arcade_catalog::ArcadeGameEntry;
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

type MameMachineFixture<'a> = (
    &'a str,
    Option<&'a str>,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
);

pub(crate) struct ArcadeGameFixture {
    title: String,
    mra_path: String,
    preview_archive_path: String,
    preview_asset_key: String,
    system_id: String,
    year: Option<u16>,
    manufacturer: String,
    category: String,
    players: Option<u8>,
    control: String,
    is_new: bool,
}

impl ArcadeGameFixture {
    pub(crate) fn path(mut self, path: impl Into<String>) -> Self {
        self.mra_path = path.into();
        self
    }

    pub(crate) fn preview(mut self, asset_key: impl Into<String>) -> Self {
        self.preview_archive_path =
            "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b".to_string();
        self.preview_asset_key = asset_key.into();
        self
    }

    pub(crate) fn system_id(mut self, system_id: impl Into<String>) -> Self {
        self.system_id = system_id.into();
        self
    }

    pub(crate) fn build(self) -> ArcadeGameEntry {
        let has_preview =
            !self.preview_archive_path.is_empty() && !self.preview_asset_key.is_empty();
        ArcadeGameEntry {
            title: self.title.into(),
            mra_path: self.mra_path.into(),
            preview_archive_path: self.preview_archive_path.into(),
            preview_asset_key: self.preview_asset_key.into(),
            has_preview,
            system_id: self.system_id.into(),
            year: self.year,
            manufacturer: self.manufacturer.into(),
            category: self.category.into(),
            players: self.players,
            control: self.control.into(),
            is_new: self.is_new,
        }
    }
}

pub(crate) fn arcade_game(title: impl Into<String>) -> ArcadeGameFixture {
    let title = title.into();
    let mra_path = format!("/media/fat/_Arcade/{title}.mra");
    ArcadeGameFixture {
        title,
        mra_path,
        preview_archive_path: String::new(),
        preview_asset_key: String::new(),
        system_id: "arcade".to_string(),
        year: None,
        manufacturer: String::new(),
        category: String::new(),
        players: None,
        control: String::new(),
        is_new: false,
    }
}

pub(crate) fn write_stored_zip(path: &Path, entries: &[(&str, &[u8])]) {
    write_stored_zip_with_central_metadata(path, entries, &[], &[]);
}

pub(crate) fn write_stored_zip64_member(path: &Path, name: &str, data: &[u8]) {
    write_stored_zip_fixture(path, &[(name, data)], &[], &[], true);
}

pub(crate) fn write_stored_zip_with_central_metadata(
    path: &Path,
    entries: &[(&str, &[u8])],
    central_extra: &[u8],
    central_comment: &[u8],
) {
    write_stored_zip_fixture(path, entries, central_extra, central_comment, false);
}

fn write_stored_zip_fixture(
    path: &Path,
    entries: &[(&str, &[u8])],
    central_extra: &[u8],
    central_comment: &[u8],
    zip64_members: bool,
) {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let local_offset = out.len() as u64;
        push_u32(&mut out, 0x0403_4b50);
        push_u16(&mut out, 20);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u32(&mut out, 0);
        push_u32(&mut out, data.len() as u32);
        push_u32(&mut out, data.len() as u32);
        push_u16(&mut out, name.len() as u16);
        push_u16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        push_u32(&mut central, 0x0201_4b50);
        push_u16(&mut central, if zip64_members { 45 } else { 20 });
        push_u16(&mut central, if zip64_members { 45 } else { 20 });
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(
            &mut central,
            if zip64_members {
                u32::MAX
            } else {
                data.len() as u32
            },
        );
        push_u32(
            &mut central,
            if zip64_members {
                u32::MAX
            } else {
                data.len() as u32
            },
        );
        push_u16(&mut central, name.len() as u16);
        let mut entry_extra = Vec::new();
        if zip64_members {
            push_u16(&mut entry_extra, 0x0001);
            push_u16(&mut entry_extra, 24);
            push_u64(&mut entry_extra, data.len() as u64);
            push_u64(&mut entry_extra, data.len() as u64);
            push_u64(&mut entry_extra, local_offset);
        }
        entry_extra.extend_from_slice(central_extra);
        push_u16(&mut central, entry_extra.len() as u16);
        push_u16(&mut central, central_comment.len() as u16);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(
            &mut central,
            if zip64_members {
                u32::MAX
            } else {
                local_offset as u32
            },
        );
        central.extend_from_slice(name.as_bytes());
        central.extend_from_slice(&entry_extra);
        central.extend_from_slice(central_comment);
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    let zip64_eocd_offset = out.len() as u64;
    push_u32(&mut out, 0x0606_4b50);
    push_u64(&mut out, 44);
    push_u16(&mut out, 45);
    push_u16(&mut out, 45);
    push_u32(&mut out, 0);
    push_u32(&mut out, 0);
    push_u64(&mut out, entries.len() as u64);
    push_u64(&mut out, entries.len() as u64);
    push_u64(&mut out, central_size as u64);
    push_u64(&mut out, central_offset as u64);
    push_u32(&mut out, 0x0706_4b50);
    push_u32(&mut out, 0);
    push_u64(&mut out, zip64_eocd_offset);
    push_u32(&mut out, 1);
    push_u32(&mut out, 0x0605_4b50);
    push_u16(&mut out, 0);
    push_u16(&mut out, 0);
    push_u16(&mut out, u16::MAX);
    push_u16(&mut out, u16::MAX);
    push_u32(&mut out, u32::MAX);
    push_u32(&mut out, u32::MAX);
    push_u16(&mut out, 0);
    std::fs::write(path, out).expect("write zip fixture");
}

pub(crate) fn write_mame_fixture_db(path: &Path, rows: &[MameMachineFixture<'_>]) {
    let conn = Connection::open(path).expect("open mame fixture");
    conn.execute_batch(
        r#"
        CREATE TABLE mame_machines (
            setname TEXT PRIMARY KEY,
            parent_setname TEXT,
            title TEXT NOT NULL,
            year TEXT,
            manufacturer TEXT
        ) WITHOUT ROWID;
        "#,
    )
    .expect("create mame fixture");
    let mut stmt = conn
        .prepare(
            "INSERT INTO mame_machines(setname,parent_setname,title,year,manufacturer)
             VALUES (?1,?2,?3,?4,?5)",
        )
        .expect("prepare mame fixture insert");
    for (setname, parent, title, year, manufacturer) in rows {
        stmt.execute(params![setname, parent, title, year, manufacturer])
            .expect("insert mame fixture row");
    }
}

pub(crate) fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn unique_temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mister-magik-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create temp dir");
    path
}
