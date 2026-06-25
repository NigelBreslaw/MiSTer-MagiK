//! Shared test fixtures for catalog modules.

use crate::arcade_catalog::ArcadeGameEntry;
use crate::catalog_config::SCHEMA_VERSION;
use crate::catalog_projection::CatalogProjectionRow;
use crate::game_discovery::{DiscoveryConfidence, DiscoverySourceKind, GameDiscovery};
use crate::launch_profiles::{self, PayloadDisposition, PayloadRule};
use crate::library_db::{title_from_path, unix_now_secs, LibraryPayloadFile, LibraryScan};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

type MameMachineFixture<'a> = (
    &'a str,
    Option<&'a str>,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
);

pub(crate) fn chd_v5_header(sha1: [u8; 20]) -> [u8; 124] {
    let mut header = [0u8; 124];
    header[..8].copy_from_slice(b"MComprHD");
    header[8..12].copy_from_slice(&124u32.to_be_bytes());
    header[12..16].copy_from_slice(&5u32.to_be_bytes());
    header[56..60].copy_from_slice(&4096u32.to_be_bytes());
    header[60..64].copy_from_slice(&2448u32.to_be_bytes());
    header[64..84].copy_from_slice(&sha1);
    header
}

pub(crate) fn payload(path: &str) -> GameDiscovery {
    GameDiscovery {
        source_path: path.to_string(),
        launch_ref: path.to_string(),
        source_kind: DiscoverySourceKind::PayloadFile,
        title: title_from_path(path),
        category: "Unknown".to_string(),
        platform_id: "unknown".to_string(),
        core_id: "unknown".to_string(),
        hardware_id: "unknown".to_string(),
        manufacturer: None,
        genre: None,
        year: None,
        setname: None,
        parent: None,
        covered_payload_path: None,
        confidence: DiscoveryConfidence::PayloadPath,
    }
}

pub(crate) fn saturn_payload(path: &str) -> GameDiscovery {
    GameDiscovery {
        source_path: path.to_string(),
        launch_ref: path.to_string(),
        source_kind: DiscoverySourceKind::PayloadFile,
        title: title_from_path(path),
        category: "Console".to_string(),
        platform_id: "saturn".to_string(),
        core_id: "Saturn".to_string(),
        hardware_id: "saturn".to_string(),
        manufacturer: None,
        genre: None,
        year: None,
        setname: None,
        parent: None,
        covered_payload_path: None,
        confidence: DiscoveryConfidence::PayloadPath,
    }
}

pub(crate) fn catalog_row(
    title: &str,
    path: &str,
    setname: &str,
    parent: &str,
) -> CatalogProjectionRow {
    CatalogProjectionRow {
        game: ArcadeGameEntry {
            title: title.into(),
            mra_path: path.into(),
            preview_archive_path: "".into(),
            preview_asset_key: "".into(),
            has_preview: false,
            system_id: "arcade".into(),
            is_new: false,
        },
        discovered_at_unix: None,
        source_kind: "mra".to_string(),
        setname: setname.to_string(),
        parent: parent.to_string(),
        family_key: None,
    }
}

pub(crate) fn catalog_launcher_row(title: &str, path: &str) -> CatalogProjectionRow {
    CatalogProjectionRow {
        game: ArcadeGameEntry {
            title: title.into(),
            mra_path: path.into(),
            preview_archive_path: "".into(),
            preview_asset_key: "".into(),
            has_preview: false,
            system_id: "unknown".into(),
            is_new: false,
        },
        discovered_at_unix: None,
        source_kind: "mgl".to_string(),
        setname: String::new(),
        parent: String::new(),
        family_key: None,
    }
}

pub(crate) fn catalog_entry_row(title: &str, path: &str) -> CatalogProjectionRow {
    CatalogProjectionRow {
        game: ArcadeGameEntry {
            title: title.into(),
            mra_path: path.into(),
            preview_archive_path: "".into(),
            preview_asset_key: "".into(),
            has_preview: false,
            system_id: "amiga".into(),
            is_new: false,
        },
        discovered_at_unix: None,
        source_kind: "catalog-entry".to_string(),
        setname: String::new(),
        parent: String::new(),
        family_key: None,
    }
}

pub(crate) fn write_stored_zip(path: &Path, entries: &[(&str, &[u8])]) {
    write_stored_zip_with_central_metadata(path, entries, &[], &[]);
}

pub(crate) fn write_stored_zip_with_central_metadata(
    path: &Path,
    entries: &[(&str, &[u8])],
    central_extra: &[u8],
    central_comment: &[u8],
) {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let local_offset = out.len() as u32;
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
        push_u16(&mut central, 20);
        push_u16(&mut central, 20);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, data.len() as u32);
        push_u32(&mut central, data.len() as u32);
        push_u16(&mut central, name.len() as u16);
        push_u16(&mut central, central_extra.len() as u16);
        push_u16(&mut central, central_comment.len() as u16);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, local_offset);
        central.extend_from_slice(name.as_bytes());
        central.extend_from_slice(central_extra);
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

type SoftwareItemFixture<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
);

pub(crate) fn write_mame_software_fixture_db(
    path: &Path,
    items: &[SoftwareItemFixture<'_>],
    hashes: &[(&str, &str, i64, u32)],
) {
    let conn = Connection::open(path).expect("open software fixture");
    conn.execute_batch(
        r#"
        CREATE TABLE mame_machines (
            setname TEXT PRIMARY KEY,
            parent_setname TEXT,
            title TEXT NOT NULL,
            year TEXT,
            manufacturer TEXT
        ) WITHOUT ROWID;
        CREATE TABLE mame_software_items (
            list_name TEXT NOT NULL,
            software_name TEXT NOT NULL,
            parent_name TEXT,
            description TEXT NOT NULL,
            year TEXT,
            publisher TEXT,
            region TEXT,
            source_version TEXT NOT NULL,
            PRIMARY KEY(list_name, software_name)
        ) WITHOUT ROWID;
        CREATE TABLE mame_software_hashes (
            list_name TEXT NOT NULL,
            software_name TEXT NOT NULL,
            part_name TEXT,
            rom_name TEXT,
            size INTEGER,
            crc32 TEXT,
            sha1 TEXT,
            data_area TEXT,
            disk_sha1 TEXT
        );
        "#,
    )
    .expect("create software fixture");
    let mut item_stmt = conn
        .prepare(
            "INSERT INTO mame_software_items(
                list_name,software_name,parent_name,description,year,publisher,region,source_version
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,'fixture')",
        )
        .expect("prepare software fixture item insert");
    for (list, name, parent, description, year, publisher, region) in items {
        item_stmt
            .execute(params![
                list,
                name,
                parent,
                description,
                year,
                publisher,
                region
            ])
            .expect("insert software fixture item");
    }
    let mut hash_stmt = conn
        .prepare(
            "INSERT INTO mame_software_hashes(list_name,software_name,size,crc32)
             VALUES (?1,?2,?3,?4)",
        )
        .expect("prepare software fixture hash insert");
    for (list, name, size, crc) in hashes {
        hash_stmt
            .execute(params![list, name, size, format!("{crc:08x}")])
            .expect("insert software fixture hash");
    }
}

pub(crate) fn write_software_hash_cache_fixture(
    path: &Path,
    rows: &[(&str, &str, u64, i64, Option<&str>)],
) {
    let conn = Connection::open(path).expect("open software hash cache fixture");
    conn.execute_batch(
        r#"
        CREATE TABLE software_hash_cache (
            list_name TEXT NOT NULL,
            file_path TEXT NOT NULL,
            size INTEGER NOT NULL,
            mtime_secs INTEGER NOT NULL,
            software_name TEXT,
            PRIMARY KEY(list_name, file_path, size, mtime_secs)
        ) WITHOUT ROWID;
        "#,
    )
    .expect("create software hash cache fixture");
    let mut stmt = conn
        .prepare(
            "INSERT INTO software_hash_cache(list_name,file_path,size,mtime_secs,software_name)
             VALUES (?1,?2,?3,?4,?5)",
        )
        .expect("prepare software hash cache fixture insert");
    for (list, path, size, mtime, software_name) in rows {
        stmt.execute(params![list, path, *size as i64, mtime, software_name])
            .expect("insert software hash cache fixture");
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

pub(crate) fn mgl(source_path: &str, launch_ref: &str) -> GameDiscovery {
    GameDiscovery {
        source_path: source_path.to_string(),
        launch_ref: launch_ref.to_string(),
        source_kind: DiscoverySourceKind::Mgl,
        title: title_from_path(source_path),
        category: "Unknown".to_string(),
        platform_id: "unknown".to_string(),
        core_id: "unknown".to_string(),
        hardware_id: "unknown".to_string(),
        manufacturer: None,
        genre: None,
        year: None,
        setname: None,
        parent: None,
        covered_payload_path: None,
        confidence: DiscoveryConfidence::PayloadPath,
    }
}

pub(crate) fn mra_discovery(idx: usize, title: &str) -> GameDiscovery {
    let path = format!("/media/fat/_Arcade/{title}.mra");
    GameDiscovery {
        source_path: path.clone(),
        launch_ref: path,
        source_kind: DiscoverySourceKind::Mra,
        title: title.to_string(),
        category: "Arcade".to_string(),
        platform_id: "arcade".to_string(),
        core_id: "arcade".to_string(),
        hardware_id: "arcade-unknown".to_string(),
        manufacturer: None,
        genre: None,
        year: None,
        setname: Some(format!("game{idx:05}")),
        parent: None,
        covered_payload_path: None,
        confidence: DiscoveryConfidence::MraCore,
    }
}

pub(crate) fn sqlite_scan_with_normal_files(paths: &[&str]) -> LibraryScan {
    LibraryScan {
        version: SCHEMA_VERSION,
        scanned_at_unix: 1,
        normal_files: paths
            .iter()
            .map(|path| LibraryPayloadFile {
                path: path.to_string(),
                profile_id: "mgl".to_string(),
                size: 0,
                mtime_secs: 0,
                rule: PayloadRule {
                    extensions: &["mgl"],
                    mount: launch_profiles::MountSpec::launcher(),
                    disposition: PayloadDisposition::Playable,
                    provenance: launch_profiles::RuleProvenance::mgl(
                        "test fixture launcher payload",
                    ),
                },
            })
            .collect(),
        containers: Vec::new(),
        entries: Vec::new(),
        ignored_files: 0,
        discoveries: Vec::new(),
        discover_us: 0,
        classify_us: 0,
    }
}

pub(crate) fn sqlite_scan_with_discoveries(discoveries: Vec<GameDiscovery>) -> LibraryScan {
    LibraryScan {
        version: SCHEMA_VERSION,
        scanned_at_unix: 1,
        normal_files: Vec::new(),
        containers: Vec::new(),
        entries: Vec::new(),
        ignored_files: 0,
        discoveries,
        discover_us: 0,
        classify_us: 0,
    }
}

pub(crate) fn unique_temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mister-magik-{label}-{}-{}",
        std::process::id(),
        unix_now_secs()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create temp dir");
    path
}

#[cfg(unix)]
pub(crate) fn set_file_mtime_for_test(path: &Path, sec: i64, nsec: i64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).expect("path cstring");
    let times = [
        libc::timespec {
            tv_sec: sec as libc::time_t,
            tv_nsec: nsec as libc::c_long,
        },
        libc::timespec {
            tv_sec: sec as libc::time_t,
            tv_nsec: nsec as libc::c_long,
        },
    ];
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(rc, 0, "utimensat failed for {}", path.display());
}
